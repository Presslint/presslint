//! Integration tests for the DeviceLink executor.
//!
//! Every fixture is a tiny SYNTHETIC DeviceLink built in-memory through the same
//! `lcms2-sys` FFI the library uses: create built-in / synthetic profiles, link
//! them with `cmsTransform2DeviceLink`, serialize with `cmsSaveProfileToMem`,
//! then feed those bytes back through the public API. No ECI/FOGRA profiles are
//! used, and nothing is vendored to disk.
#![allow(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc
)]

use core::ptr;

use lcms2_sys::ffi::{
    CIExyY, HPROFILE, HTRANSFORM, Intent, PixelFormat, cmsBuildGamma, cmsCloseProfile,
    cmsCreate_sRGBProfile, cmsCreateGrayProfile, cmsCreateLab4Profile, cmsCreateTransform,
    cmsDeleteTransform, cmsFreeToneCurve, cmsSaveProfileToMem, cmsTransform2DeviceLink,
};
use presslint_color_lcms::{
    DeviceLinkSpace, LcmsError, apply_device_link_f64, inspect_device_link,
};

/// Serialize an open profile handle to its ICC bytes.
///
/// # Safety
///
/// `profile` must be a live profile handle.
unsafe fn save_profile(profile: HPROFILE) -> Result<Vec<u8>, String> {
    let mut needed: u32 = 0;
    if unsafe { cmsSaveProfileToMem(profile, ptr::null_mut(), &raw mut needed) } == 0 {
        return Err("cmsSaveProfileToMem size probe failed".to_string());
    }
    let mut buffer = vec![0u8; needed as usize];
    if unsafe { cmsSaveProfileToMem(profile, buffer.as_mut_ptr().cast(), &raw mut needed) } == 0 {
        return Err("cmsSaveProfileToMem write failed".to_string());
    }
    buffer.truncate(needed as usize);
    Ok(buffer)
}

/// Turn a source→destination transform into a serialized DeviceLink.
///
/// # Safety
///
/// `transform` must be a live transform handle. It is deleted by this call.
unsafe fn transform_to_device_link_bytes(transform: HTRANSFORM) -> Result<Vec<u8>, String> {
    if transform.is_null() {
        return Err("transform build returned null".to_string());
    }
    let link = unsafe { cmsTransform2DeviceLink(transform, 4.3, 0) };
    unsafe { cmsDeleteTransform(transform) };
    if link.is_null() {
        return Err("cmsTransform2DeviceLink returned null".to_string());
    }
    let bytes = unsafe { save_profile(link) };
    unsafe { cmsCloseProfile(link) };
    bytes
}

/// A synthetic RGB→RGB DeviceLink (sRGB identity), 3→3 channels.
fn rgb_to_rgb_link() -> Result<Vec<u8>, String> {
    unsafe {
        let srgb = cmsCreate_sRGBProfile();
        if srgb.is_null() {
            return Err("cmsCreate_sRGBProfile returned null".to_string());
        }
        let transform = cmsCreateTransform(
            srgb,
            PixelFormat::RGB_DBL,
            srgb,
            PixelFormat::RGB_DBL,
            Intent::RelativeColorimetric,
            0,
        );
        let bytes = transform_to_device_link_bytes(transform);
        cmsCloseProfile(srgb);
        bytes
    }
}

/// A synthetic Gray→RGB DeviceLink, 1→3 channels, distinct source/dest spaces.
fn gray_to_rgb_link() -> Result<Vec<u8>, String> {
    unsafe {
        let white: *const CIExyY = CIExyY::d50();
        let curve = cmsBuildGamma(ptr::null_mut(), 2.2);
        if curve.is_null() {
            return Err("cmsBuildGamma returned null".to_string());
        }
        let gray = cmsCreateGrayProfile(white, curve);
        cmsFreeToneCurve(curve);
        let srgb = cmsCreate_sRGBProfile();
        if gray.is_null() || srgb.is_null() {
            if !gray.is_null() {
                cmsCloseProfile(gray);
            }
            if !srgb.is_null() {
                cmsCloseProfile(srgb);
            }
            return Err("gray/sRGB profile creation returned null".to_string());
        }
        let transform = cmsCreateTransform(
            gray,
            PixelFormat::GRAY_DBL,
            srgb,
            PixelFormat::RGB_DBL,
            Intent::RelativeColorimetric,
            0,
        );
        let bytes = transform_to_device_link_bytes(transform);
        cmsCloseProfile(gray);
        cmsCloseProfile(srgb);
        bytes
    }
}

/// A synthetic Lab→RGB DeviceLink, 3→3 channels, Lab on the SOURCE side.
///
/// Used to prove that a Lab-sided DeviceLink is inspectable but NOT applicable:
/// lcms `TYPE_Lab_DBL` is the `cmsCIELab` domain, not the normalized `0.0..=1.0`
/// scalar domain the public `apply` API accepts.
fn lab_to_rgb_link() -> Result<Vec<u8>, String> {
    unsafe {
        // Null white point → lcms uses the D50 default.
        let lab = cmsCreateLab4Profile(ptr::null());
        let srgb = cmsCreate_sRGBProfile();
        if lab.is_null() || srgb.is_null() {
            if !lab.is_null() {
                cmsCloseProfile(lab);
            }
            if !srgb.is_null() {
                cmsCloseProfile(srgb);
            }
            return Err("lab/sRGB profile creation returned null".to_string());
        }
        let transform = cmsCreateTransform(
            lab,
            PixelFormat::Lab_DBL,
            srgb,
            PixelFormat::RGB_DBL,
            Intent::RelativeColorimetric,
            0,
        );
        let bytes = transform_to_device_link_bytes(transform);
        cmsCloseProfile(lab);
        cmsCloseProfile(srgb);
        bytes
    }
}

/// Serialized ICC bytes of a NON-DeviceLink profile (sRGB display class).
fn srgb_display_profile_bytes() -> Result<Vec<u8>, String> {
    unsafe {
        let srgb = cmsCreate_sRGBProfile();
        if srgb.is_null() {
            return Err("cmsCreate_sRGBProfile returned null".to_string());
        }
        let bytes = save_profile(srgb);
        cmsCloseProfile(srgb);
        bytes
    }
}

#[test]
fn inspect_reports_rgb_source_and_destination() -> Result<(), String> {
    let info = inspect_device_link(&rgb_to_rgb_link()?).map_err(|error| format!("{error:?}"))?;
    assert_eq!(info.source_space, DeviceLinkSpace::Rgb);
    assert_eq!(info.destination_space, DeviceLinkSpace::Rgb);
    assert_eq!(info.input_channels, 3);
    assert_eq!(info.output_channels, 3);
    Ok(())
}

#[test]
fn inspect_reports_distinct_spaces_and_channel_counts() -> Result<(), String> {
    let info = inspect_device_link(&gray_to_rgb_link()?).map_err(|error| format!("{error:?}"))?;
    assert_eq!(info.source_space, DeviceLinkSpace::Gray);
    assert_eq!(info.destination_space, DeviceLinkSpace::Rgb);
    assert_eq!(info.input_channels, 1);
    assert_eq!(info.output_channels, 3);
    Ok(())
}

#[test]
fn apply_rgb_is_deterministic_across_repeated_calls() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    let input = [0.25, 0.5, 0.75];

    let first = apply_device_link_f64(&bytes, &input).map_err(|error| format!("{error:?}"))?;
    let second = apply_device_link_f64(&bytes, &input).map_err(|error| format!("{error:?}"))?;

    assert_eq!(first.len(), 3);
    assert!(first.iter().all(|value| value.is_finite()));

    // Bit-identical: compare the raw IEEE-754 bit patterns, not just ==.
    let first_bits: Vec<u64> = first.iter().map(|value| value.to_bits()).collect();
    let second_bits: Vec<u64> = second.iter().map(|value| value.to_bits()).collect();
    assert_eq!(first_bits, second_bits);
    Ok(())
}

#[test]
fn apply_gray_returns_three_output_components() -> Result<(), String> {
    let bytes = gray_to_rgb_link()?;
    let output = apply_device_link_f64(&bytes, &[0.5]).map_err(|error| format!("{error:?}"))?;
    assert_eq!(output.len(), 3);
    assert!(output.iter().all(|value| value.is_finite()));
    Ok(())
}

#[test]
fn inspect_reports_lab_source_side() -> Result<(), String> {
    // A Lab-sided DeviceLink is inspectable: the caller needs to SEE the Lab
    // source to enforce its source-space gate, even though `apply` can't run
    // it in this slice.
    let info = inspect_device_link(&lab_to_rgb_link()?).map_err(|error| format!("{error:?}"))?;
    assert_eq!(info.source_space, DeviceLinkSpace::Lab);
    assert_eq!(info.destination_space, DeviceLinkSpace::Rgb);
    assert_eq!(info.input_channels, 3);
    assert_eq!(info.output_channels, 3);
    Ok(())
}

#[test]
fn apply_rejects_lab_sided_device_link() -> Result<(), String> {
    let bytes = lab_to_rgb_link()?;
    // In-range components: proves the rejection is driven by the unsupported Lab
    // encoding, NOT by range validation (which is checked only after the space
    // is confirmed applicable).
    assert_eq!(
        apply_device_link_f64(&bytes, &[0.5, 0.5, 0.5]),
        Err(LcmsError::UnsupportedColorSpace)
    );
    Ok(())
}

#[test]
fn apply_rejects_channel_count_mismatch() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    let result = apply_device_link_f64(&bytes, &[0.1, 0.2]);
    assert_eq!(
        result,
        Err(LcmsError::ChannelCountMismatch {
            expected: 3,
            got: 2,
        })
    );
    Ok(())
}

#[test]
fn apply_rejects_nan_and_infinite_components() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    assert_eq!(
        apply_device_link_f64(&bytes, &[0.1, f64::NAN, 0.3]),
        Err(LcmsError::NonFiniteComponent)
    );
    assert_eq!(
        apply_device_link_f64(&bytes, &[0.1, f64::INFINITY, 0.3]),
        Err(LcmsError::NonFiniteComponent)
    );
    Ok(())
}

#[test]
fn apply_rejects_out_of_range_components() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    assert_eq!(
        apply_device_link_f64(&bytes, &[0.1, 1.5, 0.3]),
        Err(LcmsError::ComponentOutOfRange)
    );
    assert_eq!(
        apply_device_link_f64(&bytes, &[0.1, -0.5, 0.3]),
        Err(LcmsError::ComponentOutOfRange)
    );
    Ok(())
}

#[test]
fn non_device_link_profile_is_rejected() -> Result<(), String> {
    let srgb_bytes = srgb_display_profile_bytes()?;
    assert_eq!(
        inspect_device_link(&srgb_bytes),
        Err(LcmsError::NotADeviceLink)
    );
    assert_eq!(
        apply_device_link_f64(&srgb_bytes, &[0.1, 0.2, 0.3]),
        Err(LcmsError::NotADeviceLink)
    );
    Ok(())
}

#[test]
fn invalid_profile_bytes_are_rejected() {
    let garbage = b"this is definitely not an ICC profile";
    assert_eq!(inspect_device_link(garbage), Err(LcmsError::InvalidProfile));
    assert_eq!(
        apply_device_link_f64(garbage, &[0.1, 0.2, 0.3]),
        Err(LcmsError::InvalidProfile)
    );
}
