//! The [`ColorEngine`] adapter over the free DeviceLink functions.
//!
//! [`LcmsColorEngine`] is a zero-state implementation of the
//! `presslint-color` LINK-APPLICATION contract. It delegates verbatim to the
//! existing free functions [`inspect_device_link`](crate::inspect_device_link)
//! and the same validation/build/apply helpers as
//! [`apply_device_link_f64`](crate::apply_device_link_f64), so results stay
//! bit-identical to — and fail identically to — the free function.

use std::cell::RefCell;
use std::fmt;

use presslint_color::{ColorEngine, DeviceLinkShape};
use presslint_types::ColorSpace;

use crate::{
    DeviceLinkInfo, DeviceLinkSpace, LcmsError, NativeDeviceLink, inspect_device_link,
    validate_device_link_input,
};

/// A zero-state [`ColorEngine`] backed by Little CMS.
///
/// The engine holds no state: every method delegates to the crate's free
/// DeviceLink functions. It is cheap to create and copy.
#[derive(Debug, Clone, Copy, Default)]
pub struct LcmsColorEngine;

/// A prepared DeviceLink: its ICC bytes, inspected shape, and lazy native pair.
///
/// This owns a copy of the DeviceLink bytes so first application can open the
/// profile and build the transform request-locally. The native pair is retained
/// after that first successful `apply`, so later applies through the same
/// prepared link reuse the transform instead of reparsing/rebuilding it.
pub struct PreparedDeviceLink {
    /// The DeviceLink ICC bytes, opened lazily on first reachable `apply`.
    bytes: Vec<u8>,
    /// The inspected source/destination spaces and channel counts.
    info: DeviceLinkInfo,
    /// Lazy `OpenProfile` + `OwnedTransform`; private to this crate.
    native: RefCell<Option<NativeDeviceLink>>,
}

impl PreparedDeviceLink {
    /// The raw inspected DeviceLink metadata.
    ///
    /// This preserves unsupported ICC signatures for callers that need exact
    /// report/error fields. It exposes no native Little CMS handles.
    #[must_use]
    pub const fn info(&self) -> DeviceLinkInfo {
        self.info
    }
}

impl fmt::Debug for PreparedDeviceLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedDeviceLink")
            .field("bytes_len", &self.bytes.len())
            .field("info", &self.info)
            .field("native_ready", &self.native.borrow().is_some())
            .finish()
    }
}

impl ColorEngine for LcmsColorEngine {
    type DeviceLink = PreparedDeviceLink;
    type Error = LcmsError;

    fn prepare_device_link(&self, bytes: &[u8]) -> Result<Self::DeviceLink, Self::Error> {
        // The same single `inspect_device_link` the routing layer already
        // performs. Validates the profile is a DeviceLink and captures its
        // shape; the bytes are retained for the delegated `apply`.
        let info = inspect_device_link(bytes)?;
        Ok(PreparedDeviceLink {
            bytes: bytes.to_vec(),
            info,
            native: RefCell::new(None),
        })
    }

    fn device_link_shape(&self, link: &Self::DeviceLink) -> DeviceLinkShape {
        DeviceLinkShape {
            source: map_color_space(link.info.source_space),
            destination: map_color_space(link.info.destination_space),
            input_channels: link.info.input_channels,
            output_channels: link.info.output_channels,
        }
    }

    fn apply_device_link(
        &self,
        link: &Self::DeviceLink,
        input: &[f64],
    ) -> Result<Vec<f64>, Self::Error> {
        let formats = validate_device_link_input(link.info, input)?;
        let mut native = link.native.borrow_mut();
        if native.is_none() {
            *native = Some(NativeDeviceLink::open(&link.bytes, link.info, formats)?);
        }
        let native = native.as_ref().ok_or(LcmsError::TransformFailed)?;
        Ok(native.apply(input))
    }
}

/// Map a [`DeviceLinkSpace`] to the shared [`ColorSpace`] vocabulary.
///
/// Gray→`DeviceGray`, Rgb→`DeviceRgb`, Cmyk→`DeviceCmyk`, Lab→`Lab`, and any
/// `Unsupported(_)` to `Unknown`.
///
/// NOTE: the raw 32-bit ICC signature carried by `DeviceLinkSpace::Unsupported`
/// is LOST here — `ColorSpace::Unknown` is signature-free. A caller that needs
/// the raw signature must read the `DeviceLinkInfo` directly rather than the
/// mapped `DeviceLinkShape`.
const fn map_color_space(space: DeviceLinkSpace) -> ColorSpace {
    match space {
        DeviceLinkSpace::Gray => ColorSpace::DeviceGray,
        DeviceLinkSpace::Rgb => ColorSpace::DeviceRgb,
        DeviceLinkSpace::Cmyk => ColorSpace::DeviceCmyk,
        DeviceLinkSpace::Lab => ColorSpace::Lab,
        DeviceLinkSpace::Unsupported(_) => ColorSpace::Unknown,
    }
}
