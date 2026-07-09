//! The [`ColorEngine`] adapter over the free DeviceLink functions.
//!
//! [`LcmsColorEngine`] is a zero-state implementation of the
//! `presslint-color` LINK-APPLICATION contract. It delegates verbatim to the
//! existing free functions [`inspect_device_link`](crate::inspect_device_link)
//! and [`apply_device_link_f64`](crate::apply_device_link_f64), so its results
//! are bit-identical to — and fail identically to — the current converter's
//! API. The differential integration test pins that equivalence.
//!
//! Native profile/transform handle retention is EXPLICITLY the next slice; a
//! [`PreparedDeviceLink`] currently owns the link bytes plus the inspected
//! [`DeviceLinkInfo`](crate::DeviceLinkInfo) only, and `apply` re-parses the
//! bytes through `apply_device_link_f64` exactly as the shipped converter does
//! today. Retaining the opened profile + built transform behind this type will
//! be a pure implementation detail with no contract change.

use presslint_color::{ColorEngine, DeviceLinkShape};
use presslint_types::ColorSpace;

use crate::{
    DeviceLinkInfo, DeviceLinkSpace, LcmsError, apply_device_link_f64, inspect_device_link,
};

/// A zero-state [`ColorEngine`] backed by Little CMS.
///
/// The engine holds no state: every method delegates to the crate's free
/// DeviceLink functions. It is cheap to create and copy.
#[derive(Debug, Clone, Copy, Default)]
pub struct LcmsColorEngine;

/// A prepared DeviceLink: its ICC bytes and the inspected two-sided shape.
///
/// This owns a copy of the DeviceLink bytes so `apply` can re-open the profile,
/// matching the shipped converter's per-operator parse. That byte copy is
/// intentional and bounded (one small profile per routed link, off the
/// hot-path this slice does not touch); the retained-native-handle optimisation
/// that ends the re-parse is the next slice.
#[derive(Debug, Clone)]
pub struct PreparedDeviceLink {
    /// The DeviceLink ICC bytes, re-opened on every `apply`.
    bytes: Vec<u8>,
    /// The inspected source/destination spaces and channel counts.
    info: DeviceLinkInfo,
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
        // Delegate verbatim: identical bytes, identical validation order
        // (channel-count → format/Lab-reject → finiteness → range), identical
        // output. The differential test asserts bit-identity with this call.
        apply_device_link_f64(&link.bytes, input)
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
