//! `ICCBased` colour-space descriptor findings for the colour-usage audit.
//!
//! This module derives read-only findings from already classified page-scope
//! `/Resources /ColorSpace` entries and page-scope default colour-space facts.
//! It does not decode ICC streams, inspect profile headers, apply alternate
//! spaces, emit coverage gaps, or change audit status.

use presslint_pdf::{
    ColorSpaceFamily, DefaultColorSpaceKind, DocumentAccessBackend, IndirectRef, ObjectLookup,
    inspect_document_access, inspect_document_page_color_space_resources_with_lookup,
    inspect_document_page_default_color_spaces_with_lookup,
};
use presslint_types::{ColorSpace, PageIndex, PdfName};
use serde::{Deserialize, Serialize};

/// Which page-scope resource family an `ICCBased` finding was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IccBasedFindingSource {
    /// A named page `/Resources /ColorSpace` entry.
    PageColorSpaceResource,
    /// A page `/Resources /ColorSpace` default entry.
    DefaultColorSpace,
}

/// One `ICCBased` dictionary descriptor anomaly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IccBasedFinding {
    /// Page the finding is anchored to.
    pub page: PageIndex,
    /// Resource scope the finding was derived from.
    pub source: IccBasedFindingSource,
    /// Page colour-space resource name or default colour-space key.
    pub resource_name: PdfName,
    /// Referenced ICC profile stream from `[/ICCBased ref]`.
    pub profile_stream: Option<IndirectRef>,
    /// `/N` component count when it was parsed as a direct non-negative integer.
    pub n: Option<usize>,
    /// Classified `/Alternate` family when present and shallowly modeled.
    pub alternate_space: Option<ColorSpace>,
    /// Descriptor anomaly kind.
    pub kind: IccBasedFindingKind,
}

/// `ICCBased` descriptor anomaly category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IccBasedFindingKind {
    /// `/N` is absent, malformed, or outside the PDF `ICCBased` component set.
    MissingOrMalformedN,
    /// Direct `/Range` arity does not equal `2 x N`.
    RangeArityMismatch {
        /// Expected number of direct `/Range` array entries.
        expected: usize,
        /// Observed number of direct `/Range` array entries.
        got: usize,
    },
    /// A device-family `/Alternate` has a component count different from `/N`.
    AlternateComponentMismatch {
        /// Parsed `/N` value.
        n: usize,
        /// Component count implied by the device-family alternate.
        alternate_implied: usize,
    },
    /// `/Alternate` is present but not shallowly classifiable.
    AlternateUnclassified,
}

/// Run the page-scope `ICCBased` descriptor pass.
#[must_use]
pub fn scan_document_icc_based_findings(input: &[u8]) -> Vec<IccBasedFinding> {
    let Ok(access) = inspect_document_access(input) else {
        return Vec::new();
    };
    let lookup = backend_lookup(&access.backend);
    let Ok(color_spaces) = inspect_document_page_color_space_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    ) else {
        return Vec::new();
    };
    let defaults = inspect_document_page_default_color_spaces_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .ok();

    let mut findings = Vec::new();
    for page in &color_spaces.pages {
        let Ok(ordinal) = u32::try_from(page.ordinal) else {
            continue;
        };
        let page_index = PageIndex(ordinal);
        for resource in &page.color_spaces {
            if is_default_color_space_name(&resource.name.0) {
                continue;
            }
            collect_definition_findings(
                page_index,
                IccBasedFindingSource::PageColorSpaceResource,
                &PdfName(resource.name.0.clone()),
                IccDescriptorFacts {
                    family: resource.family,
                    component_count: resource.component_count,
                    alternate_space: resource.alternate_space,
                    icc_profile_stream: resource.icc_profile_stream,
                    icc_range_entry_count: resource.icc_range_entry_count,
                    icc_alternate_present: resource.icc_alternate_present,
                },
                &mut findings,
            );
        }
    }
    if let Some(defaults) = defaults {
        for page in &defaults.pages {
            let Ok(ordinal) = u32::try_from(page.ordinal) else {
                continue;
            };
            let page_index = PageIndex(ordinal);
            for fact in &page.defaults {
                collect_definition_findings(
                    page_index,
                    IccBasedFindingSource::DefaultColorSpace,
                    &default_resource_name(fact.kind),
                    IccDescriptorFacts {
                        family: fact.color_space.family,
                        component_count: fact.color_space.component_count,
                        alternate_space: fact.color_space.alternate_space,
                        icc_profile_stream: fact.color_space.icc_profile_stream,
                        icc_range_entry_count: fact.color_space.icc_range_entry_count,
                        icc_alternate_present: fact.color_space.icc_alternate_present,
                    },
                    &mut findings,
                );
            }
        }
    }
    findings
}

fn is_default_color_space_name(name: &[u8]) -> bool {
    matches!(name, b"DefaultGray" | b"DefaultRGB" | b"DefaultCMYK")
}

#[derive(Clone, Copy)]
struct IccDescriptorFacts {
    family: ColorSpaceFamily,
    component_count: Option<usize>,
    alternate_space: Option<ColorSpaceFamily>,
    icc_profile_stream: Option<IndirectRef>,
    icc_range_entry_count: Option<usize>,
    icc_alternate_present: Option<bool>,
}

fn collect_definition_findings(
    page: PageIndex,
    source: IccBasedFindingSource,
    resource_name: &PdfName,
    facts: IccDescriptorFacts,
    findings: &mut Vec<IccBasedFinding>,
) {
    if facts.family != ColorSpaceFamily::IccBased {
        return;
    }
    let Some(n) = facts.component_count.filter(|n| valid_icc_n(*n)) else {
        findings.push(finding(
            page,
            source,
            resource_name,
            facts,
            IccBasedFindingKind::MissingOrMalformedN,
        ));
        return;
    };
    if let Some(got) = facts.icc_range_entry_count {
        let expected = n.saturating_mul(2);
        if got != expected {
            findings.push(finding(
                page,
                source,
                resource_name,
                facts,
                IccBasedFindingKind::RangeArityMismatch { expected, got },
            ));
        }
    }
    if facts.icc_alternate_present == Some(true) {
        match facts.alternate_space {
            Some(alternate) => {
                if let Some(alternate_implied) = device_component_count(alternate) {
                    if alternate_implied != n {
                        findings.push(finding(
                            page,
                            source,
                            resource_name,
                            facts,
                            IccBasedFindingKind::AlternateComponentMismatch {
                                n,
                                alternate_implied,
                            },
                        ));
                    }
                }
            }
            None => findings.push(finding(
                page,
                source,
                resource_name,
                facts,
                IccBasedFindingKind::AlternateUnclassified,
            )),
        }
    }
}

fn finding(
    page: PageIndex,
    source: IccBasedFindingSource,
    resource_name: &PdfName,
    facts: IccDescriptorFacts,
    kind: IccBasedFindingKind,
) -> IccBasedFinding {
    IccBasedFinding {
        page,
        source,
        resource_name: resource_name.clone(),
        profile_stream: facts.icc_profile_stream,
        n: facts.component_count,
        alternate_space: facts.alternate_space.map(family_space),
        kind,
    }
}

const fn valid_icc_n(n: usize) -> bool {
    matches!(n, 1 | 3 | 4)
}

const fn device_component_count(family: ColorSpaceFamily) -> Option<usize> {
    match family {
        ColorSpaceFamily::DeviceGray => Some(1),
        ColorSpaceFamily::DeviceRgb => Some(3),
        ColorSpaceFamily::DeviceCmyk => Some(4),
        ColorSpaceFamily::IccBased
        | ColorSpaceFamily::Separation
        | ColorSpaceFamily::DeviceN
        | ColorSpaceFamily::Indexed => None,
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
        ColorSpaceFamily::Indexed => ColorSpace::Indexed,
    }
}

fn default_resource_name(kind: DefaultColorSpaceKind) -> PdfName {
    let name = match kind {
        DefaultColorSpaceKind::DefaultGray => b"DefaultGray".as_slice(),
        DefaultColorSpaceKind::DefaultRgb => b"DefaultRGB".as_slice(),
        DefaultColorSpaceKind::DefaultCmyk => b"DefaultCMYK".as_slice(),
    };
    PdfName(name.to_vec())
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
