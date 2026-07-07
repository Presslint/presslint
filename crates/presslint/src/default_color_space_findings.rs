//! Page-scope default device colour-space findings for the colour-usage audit.
//!
//! This module derives the audit's `default_color_space_findings` surface from
//! the classified page `/Resources /ColorSpace` `/DefaultGray`, `/DefaultRGB`,
//! and `/DefaultCMYK` inspection. It is report-only: default spaces are not
//! applied to [`ColorObservation`](presslint_types::ColorObservation) records
//! and no conversion, profile parsing, tint transform evaluation, or byte
//! mutation happens here.

use presslint_pdf::{
    ColorSpaceFamily, DefaultColorSpaceFact, DefaultColorSpaceKind, DocumentAccessBackend,
    ObjectLookup, SkippedDefaultColorSpace, SkippedDefaultColorSpaceReason,
    SkippedPageXObjectResourceReason, inspect_document_access,
    inspect_document_page_default_color_spaces_with_lookup,
};
use presslint_types::{ColorSpace, PageIndex, PdfName};
use serde::{Deserialize, Serialize};

use crate::color_audit::{CoverageGap, CoverageGapKind, Scan, page_gap};

/// Which resource scope a default colour-space finding was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultColorSpaceFindingSource {
    /// The page's effective `/Resources /ColorSpace` default entry (including
    /// page-tree inheritance).
    PageDefaultColorSpace,
}

/// One page-scope declared default device colour-space implication.
///
/// A finding is emitted only when the page declares a classified non-trivial
/// default for one device family and the inventory observed at least one
/// matching device-family colour on the same page. The observation remains in
/// its original `Device*` family; this record reports only that the declared
/// page environment may govern that source meaning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultColorSpaceFinding {
    /// Page the finding is anchored to.
    pub page: PageIndex,
    /// Resource scope the finding was derived from.
    pub source: DefaultColorSpaceFindingSource,
    /// Default-space key that supplied this finding.
    pub default: DefaultColorSpaceKind,
    /// Device family whose direct colour operands are affected.
    pub device_space: ColorSpace,
    /// Classified replacement colour-space family.
    pub replacement_space: ColorSpace,
    /// Shallow replacement component count when known.
    pub replacement_component_count: Option<usize>,
    /// Spot colorant names for `Separation`/`DeviceN` replacements.
    pub replacement_spot_names: Vec<PdfName>,
    /// Number of same-page observations in `device_space`.
    pub matching_device_observation_count: usize,
}

/// Everything the default colour-space pass contributes to one audit run.
#[derive(Debug, Default)]
pub struct DefaultColorSpaceScan {
    /// Findings in document page order, then `/DefaultGray`, `/DefaultRGB`,
    /// `/DefaultCMYK` key order.
    pub findings: Vec<DefaultColorSpaceFinding>,
    /// Gaps for skipped present defaults or a pass that could not begin.
    pub coverage_gaps: Vec<CoverageGap>,
}

/// Run the page-scope default colour-space pass and correlate it with the
/// already-scanned inventory observations.
#[must_use]
pub fn scan_document_default_color_spaces(
    input: &[u8],
    inventory_scan: &Scan,
) -> DefaultColorSpaceScan {
    let has_device_observations = inventory_scan.has_device_observations();
    let Ok(access) = inspect_document_access(input) else {
        if !has_device_observations {
            return DefaultColorSpaceScan::default();
        }
        return inspection_error_scan();
    };
    let lookup = backend_lookup(&access.backend);
    let Ok(report) = inspect_document_page_default_color_spaces_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    ) else {
        if !has_device_observations {
            return DefaultColorSpaceScan::default();
        }
        return inspection_error_scan();
    };

    let mut scan = DefaultColorSpaceScan::default();
    for page in &report.pages {
        let Ok(ordinal) = u32::try_from(page.ordinal) else {
            scan.coverage_gaps
                .push(pageless_gap(CoverageGapKind::DefaultColorSpaceSkipped));
            continue;
        };
        let page_index = PageIndex(ordinal);
        for default in &page.defaults {
            if let Some(finding) = derive_finding(page_index, default, inventory_scan) {
                scan.findings.push(finding);
            }
        }
        for skip in &page.skipped {
            if is_present_default_skip(skip) {
                scan.coverage_gaps.push(page_gap(
                    CoverageGapKind::DefaultColorSpaceSkipped,
                    page_index,
                ));
            }
        }
    }
    scan
}

fn derive_finding(
    page: PageIndex,
    fact: &DefaultColorSpaceFact,
    inventory_scan: &Scan,
) -> Option<DefaultColorSpaceFinding> {
    let device_space = default_device_space(fact.kind);
    let replacement_space = family_space(fact.color_space.family);
    if replacement_space == device_space {
        return None;
    }
    let matching_device_observation_count =
        inventory_scan.count_page_observations(page, &device_space);
    if matching_device_observation_count == 0 {
        return None;
    }
    Some(DefaultColorSpaceFinding {
        page,
        source: DefaultColorSpaceFindingSource::PageDefaultColorSpace,
        default: fact.kind,
        device_space,
        replacement_space,
        replacement_component_count: fact.color_space.component_count,
        replacement_spot_names: fact
            .color_space
            .spot_names
            .iter()
            .map(|name| PdfName(name.0.clone()))
            .collect(),
        matching_device_observation_count,
    })
}

const fn default_device_space(kind: DefaultColorSpaceKind) -> ColorSpace {
    match kind {
        DefaultColorSpaceKind::DefaultGray => ColorSpace::DeviceGray,
        DefaultColorSpaceKind::DefaultRgb => ColorSpace::DeviceRgb,
        DefaultColorSpaceKind::DefaultCmyk => ColorSpace::DeviceCmyk,
    }
}

const fn family_space(family: ColorSpaceFamily) -> ColorSpace {
    match family {
        ColorSpaceFamily::DeviceGray => ColorSpace::DeviceGray,
        ColorSpaceFamily::DeviceRgb => ColorSpace::DeviceRgb,
        ColorSpaceFamily::DeviceCmyk => ColorSpace::DeviceCmyk,
        ColorSpaceFamily::IccBased => ColorSpace::IccBased,
        ColorSpaceFamily::Separation => ColorSpace::Separation,
        ColorSpaceFamily::DeviceN => ColorSpace::DeviceN,
    }
}

/// Present default entries skipped by the classifier are gaps. Mere absence of
/// resources or `/ColorSpace`, and inherited-resource diagnostics that only say
/// a resource dictionary was absent, do not hide a present default key.
const fn is_present_default_skip(skip: &SkippedDefaultColorSpace) -> bool {
    if skip.kind.is_some() {
        return true;
    }
    match &skip.reason {
        SkippedDefaultColorSpaceReason::Resources { resources_reason } => !matches!(
            resources_reason,
            SkippedPageXObjectResourceReason::MissingResources
                | SkippedPageXObjectResourceReason::MissingXObject
        ),
        SkippedDefaultColorSpaceReason::MissingResources
        | SkippedDefaultColorSpaceReason::DuplicateDefault { .. }
        | SkippedDefaultColorSpaceReason::ColorSpace { .. } => false,
    }
}

fn inspection_error_scan() -> DefaultColorSpaceScan {
    DefaultColorSpaceScan {
        findings: Vec::new(),
        coverage_gaps: vec![pageless_gap(
            CoverageGapKind::DefaultColorSpaceInspectionError,
        )],
    }
}

const fn pageless_gap(kind: CoverageGapKind) -> CoverageGap {
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

/// Select the unified object-lookup backend for a resolved document access.
const fn backend_lookup(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
    match backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    }
}
