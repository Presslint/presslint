//! Differential integration tests: the [`LcmsColorEngine`] adapter versus the
//! free DeviceLink functions.
//!
//! For every synthetic in-memory DeviceLink the executor suite builds, this
//! proves that `engine.prepare + apply` is BIT-IDENTICAL to the free
//! `apply_device_link_f64` (via `f64::to_bits`) and ERROR-FOR-ERROR identical
//! on every failure fixture, in the same contractual validation order
//! (channel-count → format/Lab-reject → finiteness → range). It also proves
//! `device_link_shape` agrees with `inspect_device_link`.
//!
//! The synthetic link builders are duplicated from `tests/device_link.rs`
//! rather than shared through a `tests/common` module, because this task's
//! write scope covers only the two integration-test files; a shared module
//! would be a third, out-of-scope file. Each fixture is built through the same
//! `lcms2-sys` FFI (built-in / synthetic profiles, `cmsTransform2DeviceLink`,
//! `cmsSaveProfileToMem`). No ECI/FOGRA profiles are used; nothing is vendored.
#![allow(unsafe_code)]
#![allow(
    clippy::doc_markdown,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::float_cmp,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::panic
)]

use core::ptr;

use lcms2_sys::ffi::{
    CIExyY, HPROFILE, HTRANSFORM, Intent, PixelFormat, cmsBuildGamma, cmsCloseProfile,
    cmsCreate_sRGBProfile, cmsCreateGrayProfile, cmsCreateLab4Profile, cmsCreateTransform,
    cmsDeleteTransform, cmsFreeToneCurve, cmsSaveProfileToMem, cmsTransform2DeviceLink,
};
use presslint_color_lcms::{
    ColorEngine, LcmsColorEngine, LcmsError, apply_device_link_f64, inspect_device_link,
};
use presslint_types::ColorSpace;

// --- Synthetic link builders (duplicated from `tests/device_link.rs`) --------

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

// --- Differential helpers ----------------------------------------------------

/// Assert the adapter's `prepare + apply` matches the free `apply_device_link_f64`
/// for the same bytes and input: bit-identical on success, equal error on
/// failure, and never one-Ok-one-Err.
fn assert_adapter_matches_free_function(bytes: &[u8], input: &[f64]) {
    let engine = LcmsColorEngine;
    let free = apply_device_link_f64(bytes, input);
    let adapted = match engine.prepare_device_link(bytes) {
        Ok(link) => engine.apply_device_link(&link, input),
        // A prepare failure (invalid profile / not a DeviceLink) is the same
        // error the free function raises at open time.
        Err(error) => Err(error),
    };

    match (free, adapted) {
        (Ok(free_output), Ok(adapted_output)) => {
            let free_bits: Vec<u64> = free_output.iter().map(|value| value.to_bits()).collect();
            let adapted_bits: Vec<u64> =
                adapted_output.iter().map(|value| value.to_bits()).collect();
            assert_eq!(free_bits, adapted_bits, "adapter output not bit-identical");
        }
        (Err(free_error), Err(adapted_error)) => {
            assert_eq!(free_error, adapted_error, "adapter error diverged");
        }
        (free, adapted) => {
            panic!("adapter/free divergence: free={free:?} adapted={adapted:?}");
        }
    }
}

/// Assert `device_link_shape` agrees with `inspect_device_link` on channels and
/// maps the two sides to the expected shared `ColorSpace`.
fn assert_shape_agrees_with_inspect(
    bytes: &[u8],
    expected_source: &ColorSpace,
    expected_destination: &ColorSpace,
) {
    let engine = LcmsColorEngine;
    let info = inspect_device_link(bytes).expect("inspect a valid DeviceLink");
    let link = engine
        .prepare_device_link(bytes)
        .expect("prepare a valid DeviceLink");
    let shape = engine.device_link_shape(&link);

    assert_eq!(shape.input_channels, info.input_channels);
    assert_eq!(shape.output_channels, info.output_channels);
    assert_eq!(&shape.source, expected_source);
    assert_eq!(&shape.destination, expected_destination);
}

// --- Bit-identity on every applicable fixture --------------------------------

#[test]
fn adapter_apply_is_bit_identical_on_rgb_link() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    for input in [
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
        [0.25, 0.5, 0.75],
        [0.13, 0.87, 0.42],
    ] {
        assert_adapter_matches_free_function(&bytes, &input);
    }
    Ok(())
}

#[test]
fn adapter_apply_is_bit_identical_on_gray_link() -> Result<(), String> {
    let bytes = gray_to_rgb_link()?;
    for input in [[0.0], [1.0], [0.5], [0.375]] {
        assert_adapter_matches_free_function(&bytes, &input);
    }
    Ok(())
}

#[test]
fn prepared_link_reuses_native_transform_across_repeated_applies() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    let engine = LcmsColorEngine;
    let link = engine
        .prepare_device_link(&bytes)
        .expect("prepare a valid DeviceLink");

    assert!(
        format!("{link:?}").contains("native_ready: false"),
        "prepare must not build native state"
    );

    let first = engine
        .apply_device_link(&link, &[0.25, 0.5, 0.75])
        .map_err(|error| format!("{error:?}"))?;
    assert!(
        format!("{link:?}").contains("native_ready: true"),
        "first apply must retain native state"
    );

    let second = engine
        .apply_device_link(&link, &[0.25, 0.5, 0.75])
        .map_err(|error| format!("{error:?}"))?;
    let free =
        apply_device_link_f64(&bytes, &[0.25, 0.5, 0.75]).map_err(|error| format!("{error:?}"))?;

    let first_bits: Vec<u64> = first.iter().map(|value| value.to_bits()).collect();
    let second_bits: Vec<u64> = second.iter().map(|value| value.to_bits()).collect();
    let free_bits: Vec<u64> = free.iter().map(|value| value.to_bits()).collect();
    assert_eq!(first_bits, second_bits);
    assert_eq!(first_bits, free_bits);
    Ok(())
}

// --- Shape agreement ---------------------------------------------------------

#[test]
fn shape_agrees_with_inspect_for_rgb_link() -> Result<(), String> {
    assert_shape_agrees_with_inspect(
        &rgb_to_rgb_link()?,
        &ColorSpace::DeviceRgb,
        &ColorSpace::DeviceRgb,
    );
    Ok(())
}

#[test]
fn shape_agrees_with_inspect_for_gray_link() -> Result<(), String> {
    assert_shape_agrees_with_inspect(
        &gray_to_rgb_link()?,
        &ColorSpace::DeviceGray,
        &ColorSpace::DeviceRgb,
    );
    Ok(())
}

#[test]
fn shape_maps_lab_source_side() -> Result<(), String> {
    // Lab-sided DeviceLink is inspectable (and `apply`-rejected); its source
    // side maps to `ColorSpace::Lab`.
    assert_shape_agrees_with_inspect(
        &lab_to_rgb_link()?,
        &ColorSpace::Lab,
        &ColorSpace::DeviceRgb,
    );
    Ok(())
}

// --- Error identity on every failure fixture ---------------------------------

#[test]
fn error_identity_on_invalid_profile_bytes() {
    let garbage = b"this is definitely not an ICC profile";
    // Prepare fails identically to the free function's open-time error.
    let engine = LcmsColorEngine;
    assert!(matches!(
        engine.prepare_device_link(garbage),
        Err(LcmsError::InvalidProfile)
    ));
    assert_adapter_matches_free_function(garbage, &[0.1, 0.2, 0.3]);
}

#[test]
fn error_identity_on_non_device_link_profile() -> Result<(), String> {
    let bytes = srgb_display_profile_bytes()?;
    let engine = LcmsColorEngine;
    assert!(matches!(
        engine.prepare_device_link(&bytes),
        Err(LcmsError::NotADeviceLink)
    ));
    assert_adapter_matches_free_function(&bytes, &[0.1, 0.2, 0.3]);
    Ok(())
}

#[test]
fn error_identity_on_lab_sided_device_link() -> Result<(), String> {
    // In-range components prove the rejection is the unsupported Lab encoding,
    // not range validation — and the adapter reports it identically.
    assert_adapter_matches_free_function(&lab_to_rgb_link()?, &[0.5, 0.5, 0.5]);
    Ok(())
}

#[test]
fn error_identity_on_channel_count_mismatch() -> Result<(), String> {
    assert_adapter_matches_free_function(&rgb_to_rgb_link()?, &[0.1, 0.2]);
    Ok(())
}

#[test]
fn error_identity_on_non_finite_components() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    assert_adapter_matches_free_function(&bytes, &[0.1, f64::NAN, 0.3]);
    assert_adapter_matches_free_function(&bytes, &[0.1, f64::INFINITY, 0.3]);
    Ok(())
}

#[test]
fn error_identity_on_out_of_range_components() -> Result<(), String> {
    let bytes = rgb_to_rgb_link()?;
    assert_adapter_matches_free_function(&bytes, &[0.1, 1.5, 0.3]);
    assert_adapter_matches_free_function(&bytes, &[0.1, -0.5, 0.3]);
    Ok(())
}

#[test]
fn validation_order_is_preserved_on_prepared_and_free_paths() -> Result<(), String> {
    let engine = LcmsColorEngine;

    let rgb = rgb_to_rgb_link()?;
    let rgb_prepared = engine
        .prepare_device_link(&rgb)
        .expect("prepare RGB DeviceLink");
    let channel_mismatch = Err(LcmsError::ChannelCountMismatch {
        expected: 3,
        got: 2,
    });
    assert_eq!(
        apply_device_link_f64(&rgb, &[f64::NAN, 0.2]),
        channel_mismatch
    );
    assert_eq!(
        engine.apply_device_link(&rgb_prepared, &[f64::NAN, 0.2]),
        channel_mismatch
    );

    let lab = lab_to_rgb_link()?;
    let lab_prepared = engine
        .prepare_device_link(&lab)
        .expect("prepare Lab-sided DeviceLink");
    assert_eq!(
        apply_device_link_f64(&lab, &[0.5, f64::NAN, 0.5]),
        Err(LcmsError::UnsupportedColorSpace)
    );
    assert_eq!(
        engine.apply_device_link(&lab_prepared, &[0.5, f64::NAN, 0.5]),
        Err(LcmsError::UnsupportedColorSpace)
    );
    assert_eq!(
        apply_device_link_f64(&rgb, &[0.1, f64::NAN, 0.3]),
        Err(LcmsError::NonFiniteComponent)
    );
    assert_eq!(
        engine.apply_device_link(&rgb_prepared, &[0.1, f64::NAN, 0.3]),
        Err(LcmsError::NonFiniteComponent)
    );
    assert_eq!(
        apply_device_link_f64(&rgb, &[0.1, 1.5, 0.3]),
        Err(LcmsError::ComponentOutOfRange)
    );
    assert_eq!(
        engine.apply_device_link(&rgb_prepared, &[0.1, 1.5, 0.3]),
        Err(LcmsError::ComponentOutOfRange)
    );
    Ok(())
}
