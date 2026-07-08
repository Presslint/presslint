//! Per-observation classification into RGB findings or coverage gaps, plus the
//! coverage-gap constructors shared with the sibling finding passes.

use presslint_inventory::InventoryEntry;
use presslint_types::{ColorObservation, ColorSpace, ColorUsage, PageIndex};

use super::report::{CoverageGap, CoverageGapKind, RgbFinding};

/// Classify a single color observation into at most one RGB finding or one
/// coverage gap. Modeled process spaces (`DeviceCMYK`/`DeviceGray`) produce
/// neither.
pub(super) fn classify_observation(
    entry_index: usize,
    entry: &InventoryEntry,
    observation: &ColorObservation,
    rgb_findings: &mut Vec<RgbFinding>,
    coverage_gaps: &mut Vec<CoverageGap>,
) {
    // Image color is not decoded yet: an image observation modeled as `Unknown`
    // is a coverage gap, not an unmodeled-space gap.
    if observation.usage == ColorUsage::Image && observation.space == ColorSpace::Unknown {
        coverage_gaps.push(entry_gap(
            CoverageGapKind::ImageColorUndecoded,
            entry_index,
            entry,
            observation,
        ));
        return;
    }
    match observation.space {
        ColorSpace::DeviceRgb => rgb_findings.push(RgbFinding {
            page: entry.id.page,
            object: entry.id.clone(),
            entry_index,
            kind: entry.kind,
            usage: observation.usage,
        }),
        // Modeled device process spaces, and resource colours resolved to their
        // real source family (`IccBased`/`Separation`/`DeviceN`), are honest
        // observations, not coverage gaps. Only still-unresolvable spaces remain.
        ColorSpace::DeviceCmyk
        | ColorSpace::DeviceGray
        | ColorSpace::IccBased
        | ColorSpace::Separation
        | ColorSpace::DeviceN => {}
        _ => coverage_gaps.push(entry_gap(
            CoverageGapKind::UnmodeledColorSpace,
            entry_index,
            entry,
            observation,
        )),
    }
}

/// Build a coverage gap anchored to an inventory entry's observation.
fn entry_gap(
    kind: CoverageGapKind,
    entry_index: usize,
    entry: &InventoryEntry,
    observation: &ColorObservation,
) -> CoverageGap {
    CoverageGap {
        kind,
        page: Some(entry.id.page),
        object: Some(entry.id.clone()),
        entry_index: Some(entry_index),
        kind_of_object: Some(entry.kind),
        usage: Some(observation.usage),
        color_space: Some(observation.space.clone()),
    }
}

/// Build a page-anchored coverage gap with no object detail.
///
/// Shared with the graphics-state pass in `graphics_state_findings`, which
/// anchors its `ExtGState` gaps the same way.
pub const fn page_gap(kind: CoverageGapKind, page: PageIndex) -> CoverageGap {
    CoverageGap {
        kind,
        page: Some(page),
        object: None,
        entry_index: None,
        kind_of_object: None,
        usage: None,
        color_space: None,
    }
}

/// Build a document-anchored coverage gap with no page or object detail, for
/// document-level inspection errors.
pub(super) const fn pageless_gap(kind: CoverageGapKind) -> CoverageGap {
    CoverageGap {
        kind,
        page: None,
        object: None,
        entry_index: None,
        kind_of_object: None,
        usage: None,
        color_space: None,
    }
}
