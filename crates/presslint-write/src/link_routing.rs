//! Multi-link routing for DeviceLink content-colour conversion (F4-5).
//!
//! `convert_content_colors_incremental` carries a SET of DeviceLinks. This module
//! inspects each supplied link ONCE up front and builds a deterministic routing
//! table from a direct device colour space to the one link whose **source** space
//! equals it. Per operator, the converter then routes the operator's declared
//! space to its matching link (or leaves it verbatim when no link's source
//! matches). Routing is unambiguous by construction: two links sharing a source
//! space is a whole-operation error, never a silent guess.
//!
//! The `id` on a [`DeviceLinkInput`] is an OPAQUE caller label echoed into the
//! per-link report; this slice does not resolve names to files or profiles (that
//! is a later CLI concern).
//!
//! # Copy budget
//!
//! Each link's raw ICC bytes are BORROWED from the request (`&'a [u8]`) — no
//! profile payload is copied into the routing table. The only owned copies are
//! the small optional `id` label strings (one per link, bounded by the number of
//! links), materialised so the routing outlives per-page borrows cleanly.

// "DeviceLink" is the ICC profile-class domain term used throughout these docs as
// prose, not always as a code identifier; mirror the `presslint-color-lcms` crate
// and do not force backticks on it.
#![allow(clippy::doc_markdown)]

use std::collections::BTreeMap;

use presslint_color_lcms::{DeviceLinkSpace, inspect_device_link};
use serde::{Deserialize, Serialize};

use crate::content_color_convert::{ConvertContentColorsError, DeviceColorSpace};

/// One caller-supplied DeviceLink: an opaque `id` label plus raw ICC bytes.
///
/// A single-link caller passes a one-element vec; multi-source jobs pass one
/// input per source space (e.g. an RGB->CMYK link and a CMYK->CMYK link).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLinkInput {
    /// Opaque caller label carried through to the per-link report only. It is
    /// NOT resolved to a file/profile here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Raw ICC DeviceLink profile bytes, inspected once up front.
    pub bytes: Vec<u8>,
}

/// Per-page, per-link conversion tally in the page report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkConversionCounts {
    /// Zero-based index of this link in the request `device_links` vec.
    pub link_index: usize,
    /// The link's opaque caller label, echoed for reporting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link_id: Option<String>,
    /// The link's inspected source (A-side) space.
    pub source: DeviceLinkSpace,
    /// The link's inspected destination (B-side) space.
    pub destination: DeviceLinkSpace,
    /// Operators this page routed to and converted through this link.
    pub operators_converted: usize,
}

/// One validated, source-routable DeviceLink.
///
/// Bytes are BORROWED from the request, so building the routing copies no profile
/// payload. Both `DeviceColorSpace` fields drive per-operator mechanics (operand
/// rewrite, black preservation); the `DeviceLinkSpace` fields are the inspected
/// spaces echoed verbatim into [`LinkConversionCounts`].
pub struct RoutedLink<'a> {
    /// Index of this link in the request `device_links` vec.
    pub index: usize,
    /// The link's opaque caller label.
    pub id: Option<String>,
    /// Borrowed raw ICC bytes for `apply_device_link_f64`.
    pub bytes: &'a [u8],
    /// Narrowed destination device space (drives the operator rewrite).
    pub destination: DeviceColorSpace,
    /// Inspected source space, echoed into the per-link report.
    pub source_link_space: DeviceLinkSpace,
    /// Inspected destination space, echoed into the per-link report.
    pub destination_link_space: DeviceLinkSpace,
}

/// Deterministic source-space -> link routing table, built once per call.
pub struct LinkRouting<'a> {
    /// Validated links in request (`link_index`) order; position == `index`.
    links: Vec<RoutedLink<'a>>,
    /// Source device space -> position in `links`. `BTreeMap` keeps duplicate
    /// detection and iteration deterministic.
    by_source: BTreeMap<DeviceColorSpace, usize>,
}

impl<'a> LinkRouting<'a> {
    /// Every validated link, in request (`link_index`) order.
    pub fn links(&self) -> &[RoutedLink<'a>] {
        &self.links
    }

    /// The link whose SOURCE space equals `space`, if any.
    pub fn route(&self, space: DeviceColorSpace) -> Option<&RoutedLink<'a>> {
        self.by_source.get(&space).map(|&index| &self.links[index])
    }
}

/// Inspect every supplied link ONCE and build the deterministic routing table.
///
/// # Errors
///
/// - [`ConvertContentColorsError::NoDeviceLinks`] when `inputs` is empty.
/// - [`ConvertContentColorsError::DeviceLinkInspectFailed`] when a link's bytes
///   cannot be inspected as a DeviceLink.
/// - [`ConvertContentColorsError::UnsupportedLinkSpace`] when a link's source or
///   destination space is Lab / unsupported (no direct device operator).
/// - [`ConvertContentColorsError::AmbiguousLinkSource`] when two links declare the
///   same source space (routing would be a silent guess).
pub fn build_link_routing(
    inputs: &[DeviceLinkInput],
) -> Result<LinkRouting<'_>, ConvertContentColorsError> {
    if inputs.is_empty() {
        return Err(ConvertContentColorsError::NoDeviceLinks);
    }

    let mut links: Vec<RoutedLink<'_>> = Vec::with_capacity(inputs.len());
    let mut by_source: BTreeMap<DeviceColorSpace, usize> = BTreeMap::new();

    for (index, input) in inputs.iter().enumerate() {
        let info = inspect_device_link(&input.bytes).map_err(|error| {
            ConvertContentColorsError::DeviceLinkInspectFailed {
                index,
                id: input.id.clone(),
                error,
            }
        })?;
        let (Some(source), Some(destination)) = (
            DeviceColorSpace::from_link(info.source_space),
            DeviceColorSpace::from_link(info.destination_space),
        ) else {
            return Err(ConvertContentColorsError::UnsupportedLinkSpace {
                index,
                id: input.id.clone(),
                source: info.source_space,
                destination: info.destination_space,
            });
        };
        if let Some(&first_index) = by_source.get(&source) {
            return Err(ConvertContentColorsError::AmbiguousLinkSource {
                space: info.source_space,
                first_index,
                second_index: index,
            });
        }
        by_source.insert(source, index);
        links.push(RoutedLink {
            index,
            id: input.id.clone(),
            bytes: input.bytes.as_slice(),
            destination,
            source_link_space: info.source_space,
            destination_link_space: info.destination_space,
        });
    }

    Ok(LinkRouting { links, by_source })
}
