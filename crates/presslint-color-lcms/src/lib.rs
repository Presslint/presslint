//! Little CMS (`lcms2-sys`) DeviceLink executor.
//!
//! This crate opens an ICC **DeviceLink** profile from bytes and applies it to a
//! single scalar device colour. A DeviceLink bakes rendering intent, black
//! generation, and GCR/UCR into one device→device LUT, so applying it is a
//! deterministic lookup with no runtime intent choice — exactly what the
//! project's determinism invariant needs.
//!
//! Scope is deliberately narrow: pure colour math over ONE scalar colour. There
//! is no PDF I/O, no selector matching, and no content-stream rewrite here (that
//! belongs to the caller). This crate exists to isolate the C dependency
//! (Little CMS) and its `unsafe` FFI away from the pure-Rust, report-only
//! `presslint-color` crate, which stays `#![forbid(unsafe_code)]`.
//!
//! # Why raw `lcms2-sys` and not the safe `lcms2` wrapper
//!
//! The safe wrapper can build the transform, but it depends on `lcms2-sys` with
//! its default features, which include `parallel`. That pulls
//! `cc/parallel → jobserver → getrandom → r-efi / wasip2 / wit-bindgen`, whose
//! LGPL / LLVM-exception licences fail the mandatory `check_licenses.sh` gate.
//! Cargo cannot subtract a feature an intermediate crate turns on, so the only
//! way to keep the gate green is to depend on `lcms2-sys` directly with
//! `default-features = false, features = ["static"]`. All `unsafe` FFI is
//! contained in this one module.
//!
//! # Determinism
//!
//! Same DeviceLink bytes + same input → bit-identical `f64` output. This is
//! guaranteed by: an EXACT-pinned `lcms2-sys` compiled from the bundled Little
//! CMS source (`static`); fixed `TYPE_*_DBL` input/output formats; a
//! DeviceLink-only transform (one profile, no profile-connection-space
//! chaining); and minimal transform flags (`0` — no `NOCACHE`, no
//! `HIGHRESPRECALC`, no plugins, no mutable global lcms state). The rendering
//! intent passed to the transform builder is irrelevant to the result because
//! the intent is already baked into the DeviceLink LUT; a fixed value is used
//! for reproducibility.
//!
//! # Copy / build budget
//!
//! This crate builds one transform per call (a bounded cache can be added later
//! using `presslint-color::TransformCacheKey`). Each call opens one profile
//! handle, builds one transform handle, and allocates one small output
//! `Vec<f64>` (at most a few components). The input slice is read in place — no
//! byte copy. Both handles are released via RAII on every return path. No
//! batching.

// This crate intentionally isolates the Little CMS C FFI; `unsafe` lives here so
// `presslint-color` can stay pure-Rust. Override the workspace `unsafe_code`
// lint for this crate only.
#![allow(unsafe_code)]
// "DeviceLink" is the ICC profile-class domain term used throughout these docs
// as prose, not always as a code identifier; do not force backticks on it.
#![allow(clippy::doc_markdown)]

use lcms2_sys::ffi::{
    ColorSpaceSignature, HPROFILE, HTRANSFORM, Intent, PixelFormat, ProfileClassSignature,
    cmsChannelsOf, cmsCloseProfile, cmsCreateMultiprofileTransform, cmsDeleteTransform,
    cmsDoTransform, cmsGetColorSpace, cmsGetDeviceClass, cmsGetPCS, cmsOpenProfileFromMem,
};
use serde::{Deserialize, Serialize};

/// A colour space understood by the DeviceLink gate.
///
/// This is a narrow map from the ICC / Little CMS colour-space signature. The
/// caller uses the source and destination spaces to enforce its source-space
/// gate (only convert operands whose space matches the DeviceLink source).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLinkSpace {
    /// Single-channel grayscale (`GRAY`).
    Gray,
    /// Three-channel additive RGB (`RGB `).
    Rgb,
    /// Four-channel process CMYK (`CMYK`).
    Cmyk,
    /// CIE L*a*b* (`Lab `). Reported by [`inspect_device_link`] so the caller
    /// can see a Lab-sided DeviceLink, but [`apply_device_link_f64`] rejects it
    /// with [`LcmsError::UnsupportedColorSpace`]: Little CMS `TYPE_Lab_DBL`
    /// carries the `cmsCIELab` encoding (L in 0..100, signed a/b), NOT this
    /// API's normalized `0.0..=1.0` scalar domain, and this crate defines no Lab
    /// encoding policy.
    Lab,
    /// Any other colour-space signature, carried as its raw 32-bit ICC value so
    /// the caller can report it without this crate having to model it.
    Unsupported(u32),
}

/// What an inspected DeviceLink profile reports about its two sides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLinkInfo {
    /// The DeviceLink's A-side (source / input) colour space.
    pub source_space: DeviceLinkSpace,
    /// The DeviceLink's B-side (destination / output) colour space.
    pub destination_space: DeviceLinkSpace,
    /// Number of input components the DeviceLink consumes.
    pub input_channels: usize,
    /// Number of output components the DeviceLink produces.
    pub output_channels: usize,
}

/// A structured, serde-tagged failure from opening or applying a DeviceLink.
///
/// Every failure path returns one of these variants; the public API never
/// panics on malformed profiles or malformed input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum LcmsError {
    /// The bytes could not be parsed as an ICC profile.
    InvalidProfile,
    /// The profile parsed, but its device class is not DeviceLink (`link`).
    NotADeviceLink,
    /// The source or destination colour space has no supported scalar format.
    UnsupportedColorSpace,
    /// The input component count does not match the DeviceLink input channels.
    ChannelCountMismatch {
        /// Channels the DeviceLink expects on its input side.
        expected: usize,
        /// Channels the caller actually supplied.
        got: usize,
    },
    /// An input component was NaN or infinite.
    NonFiniteComponent,
    /// An input component was outside the normalized `0.0..=1.0` domain.
    ComponentOutOfRange,
    /// Little CMS could not build the DeviceLink transform.
    TransformBuildFailed,
    /// Little CMS reported a failure while applying the transform. Reserved:
    /// `cmsDoTransform` has no fallible return in this configuration, so it is
    /// not constructed in this slice, but it keeps the contract stable for
    /// future backends.
    TransformFailed,
}

/// Open an ICC DeviceLink profile from bytes and report its two colour spaces
/// and channel counts.
///
/// # Errors
///
/// Returns [`LcmsError::InvalidProfile`] if the bytes are not a valid ICC
/// profile, and [`LcmsError::NotADeviceLink`] if the profile's device class is
/// not DeviceLink. An unsupported colour space is reported as
/// [`DeviceLinkSpace::Unsupported`], not an error — the caller decides whether
/// it can be used.
pub fn inspect_device_link(bytes: &[u8]) -> Result<DeviceLinkInfo, LcmsError> {
    let profile = OpenProfile::open_device_link(bytes)?;
    // SAFETY: `profile.handle` is a live DeviceLink profile owned by `profile`.
    let source = unsafe { cmsGetColorSpace(profile.handle) };
    let destination = unsafe { cmsGetPCS(profile.handle) };
    Ok(DeviceLinkInfo {
        source_space: map_space(source),
        destination_space: map_space(destination),
        input_channels: channels_of(source),
        output_channels: channels_of(destination),
    })
}

/// Apply a DeviceLink to ONE scalar colour and return the output components.
///
/// `input` holds the source components normalized to `0.0..=1.0` (ISO 32000
/// §8.6 scalar operand domain). Output components are returned as raw `f64`;
/// this slice does not round or quantise (the caller owns quantisation).
///
/// Only Gray, RGB, and CMYK DeviceLink sides are applicable here — their
/// `TYPE_*_DBL` components share the normalized `0.0..=1.0` domain. A Lab-sided
/// DeviceLink is reported by [`inspect_device_link`] but rejected here with
/// [`LcmsError::UnsupportedColorSpace`] (its lcms encoding is not normalized).
///
/// Applying the same DeviceLink to the same input twice yields bit-identical
/// output (see the crate-level determinism note).
///
/// # Errors
///
/// - [`LcmsError::InvalidProfile`] / [`LcmsError::NotADeviceLink`]: as
///   [`inspect_device_link`].
/// - [`LcmsError::ChannelCountMismatch`]: `input.len()` differs from the
///   DeviceLink's input channel count.
/// - [`LcmsError::NonFiniteComponent`]: an input component is NaN or infinite.
/// - [`LcmsError::ComponentOutOfRange`]: an input component is outside
///   `0.0..=1.0`.
/// - [`LcmsError::UnsupportedColorSpace`]: the source or destination space has
///   no supported scalar (`TYPE_*_DBL`) format.
/// - [`LcmsError::TransformBuildFailed`]: Little CMS could not build the
///   transform.
pub fn apply_device_link_f64(bytes: &[u8], input: &[f64]) -> Result<Vec<f64>, LcmsError> {
    let profile = OpenProfile::open_device_link(bytes)?;
    // SAFETY: `profile.handle` is a live DeviceLink profile owned by `profile`.
    let source = unsafe { cmsGetColorSpace(profile.handle) };
    let destination = unsafe { cmsGetPCS(profile.handle) };

    let input_channels = channels_of(source);
    let output_channels = channels_of(destination);

    if input.len() != input_channels {
        return Err(LcmsError::ChannelCountMismatch {
            expected: input_channels,
            got: input.len(),
        });
    }

    // Resolve the scalar `TYPE_*_DBL` formats FIRST. A space with no supported
    // scalar format — notably a Lab-sided DeviceLink, whose lcms `TYPE_Lab_DBL`
    // encoding is the `cmsCIELab` domain (L 0..100, signed a/b), not this API's
    // normalized `0.0..=1.0` scalar domain — is rejected as
    // `UnsupportedColorSpace` regardless of the component values, before the
    // range validation below (which assumes the normalized domain).
    let input_format = dbl_format(map_space(source)).ok_or(LcmsError::UnsupportedColorSpace)?;
    let output_format =
        dbl_format(map_space(destination)).ok_or(LcmsError::UnsupportedColorSpace)?;

    for &component in input {
        if !component.is_finite() {
            return Err(LcmsError::NonFiniteComponent);
        }
        if !(0.0..=1.0).contains(&component) {
            return Err(LcmsError::ComponentOutOfRange);
        }
    }

    // DeviceLink-only transform: one profile that is the whole LUT. The intent
    // is baked into the link, so the fixed value below does not affect the
    // result. Flags `0` keeps them minimal (no NOCACHE, no HIGHRESPRECALC).
    let mut handles = [profile.handle];
    // SAFETY: `handles` points to one live DeviceLink profile; the formats are
    // valid `TYPE_*_DBL` constants matching the profile's channel counts.
    let transform = unsafe {
        cmsCreateMultiprofileTransform(
            handles.as_mut_ptr(),
            1,
            input_format,
            output_format,
            Intent::RelativeColorimetric,
            0,
        )
    };
    let transform = OwnedTransform::from_raw(transform).ok_or(LcmsError::TransformBuildFailed)?;

    let mut output = vec![0.0f64; output_channels];
    // SAFETY: the transform reads `input_channels` f64 from `input` (validated
    // to that length) and writes `output_channels` f64 into `output` (sized to
    // that length) for exactly one pixel.
    unsafe {
        cmsDoTransform(
            transform.handle,
            input.as_ptr().cast(),
            output.as_mut_ptr().cast(),
            1,
        );
    }

    Ok(output)
}

/// RAII wrapper closing an opened profile handle on drop.
struct OpenProfile {
    handle: HPROFILE,
}

impl OpenProfile {
    /// Open a profile from bytes and require that it is a DeviceLink.
    fn open_device_link(bytes: &[u8]) -> Result<Self, LcmsError> {
        let size = u32::try_from(bytes.len()).map_err(|_| LcmsError::InvalidProfile)?;
        // SAFETY: `bytes` is a valid readable slice of `size` bytes.
        let handle = unsafe { cmsOpenProfileFromMem(bytes.as_ptr().cast(), size) };
        if handle.is_null() {
            return Err(LcmsError::InvalidProfile);
        }
        let profile = Self { handle };
        // SAFETY: `handle` is a live profile owned by `profile`.
        match unsafe { cmsGetDeviceClass(handle) } {
            ProfileClassSignature::LinkClass => Ok(profile),
            _ => Err(LcmsError::NotADeviceLink),
        }
    }
}

impl Drop for OpenProfile {
    fn drop(&mut self) {
        // SAFETY: `handle` was returned by `cmsOpenProfileFromMem` and is closed
        // exactly once here.
        unsafe {
            cmsCloseProfile(self.handle);
        }
    }
}

/// RAII wrapper deleting a transform handle on drop.
struct OwnedTransform {
    handle: HTRANSFORM,
}

impl OwnedTransform {
    /// Adopt a transform handle, returning `None` if the build failed (null).
    const fn from_raw(handle: HTRANSFORM) -> Option<Self> {
        if handle.is_null() {
            None
        } else {
            Some(Self { handle })
        }
    }
}

impl Drop for OwnedTransform {
    fn drop(&mut self) {
        // SAFETY: `handle` was returned by a `cms*Transform` builder and is
        // deleted exactly once here.
        unsafe {
            cmsDeleteTransform(self.handle);
        }
    }
}

/// Channel count for a colour-space signature.
fn channels_of(signature: ColorSpaceSignature) -> usize {
    // SAFETY: `cmsChannelsOf` reads only the enum value; it has no pointer args.
    (unsafe { cmsChannelsOf(signature) }) as usize
}

/// Narrow a Little CMS colour-space signature to a [`DeviceLinkSpace`].
const fn map_space(signature: ColorSpaceSignature) -> DeviceLinkSpace {
    match signature {
        ColorSpaceSignature::GrayData => DeviceLinkSpace::Gray,
        ColorSpaceSignature::RgbData => DeviceLinkSpace::Rgb,
        ColorSpaceSignature::CmykData => DeviceLinkSpace::Cmyk,
        ColorSpaceSignature::LabData => DeviceLinkSpace::Lab,
        other => DeviceLinkSpace::Unsupported(other as u32),
    }
}

/// Pick the fixed `TYPE_*_DBL` scalar format for a space whose components live
/// in the normalized `0.0..=1.0` scalar domain (Gray, RGB, CMYK).
///
/// `Lab` returns `None` on purpose: lcms `TYPE_Lab_DBL` uses the `cmsCIELab`
/// encoding (L 0..100, signed a/b), which is NOT this API's normalized scalar
/// domain, and this crate defines no Lab encoding policy. A Lab-sided DeviceLink is
/// therefore unsupported for `apply` (reported as `UnsupportedColorSpace`),
/// though `inspect` still reports the space.
const fn dbl_format(space: DeviceLinkSpace) -> Option<PixelFormat> {
    match space {
        DeviceLinkSpace::Gray => Some(PixelFormat::GRAY_DBL),
        DeviceLinkSpace::Rgb => Some(PixelFormat::RGB_DBL),
        DeviceLinkSpace::Cmyk => Some(PixelFormat::CMYK_DBL),
        DeviceLinkSpace::Lab | DeviceLinkSpace::Unsupported(_) => None,
    }
}
