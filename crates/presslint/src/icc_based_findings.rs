//! `ICCBased` colour-space descriptor findings for the colour-usage audit.
//!
//! This module derives read-only findings from two passes over already
//! classified page-scope `/Resources /ColorSpace` entries and page-scope default
//! colour-space facts:
//!
//! - a dictionary-level pass over `/N`, `/Range`, and `/Alternate` shape; and
//! - a bounded profile-header descriptor pass over the referenced
//!   `icc_profile_stream`, deduplicated per [`IndirectRef`] and bounded by a
//!   caller-chosen decoded-byte cap.
//!
//! The descriptor pass decodes each unique profile stream at most once, extracts
//! byte-level header facts through [`inspect_icc_profile_stream_with_lookup`],
//! and emits additive findings per use-site anchor. It never applies alternate
//! spaces, never runs a CMM, never parses the ICC tag table, and never changes
//! audit status: a malformed or uninspectable profile is a report fact or gap.

use std::collections::BTreeMap;

use presslint_pdf::{
    ColorSpaceFamily, DefaultColorSpaceKind, DocumentAccessBackend, IccProfileInspectionGap,
    IccProfileStreamInspection, IndirectRef, ObjectLookup, inspect_document_access,
    inspect_document_page_color_space_resources_with_lookup,
    inspect_document_page_default_color_spaces_with_lookup, inspect_icc_profile_stream_with_lookup,
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
    /// The decoded profile payload was shorter than the 128-byte ICC header.
    ProfileHeaderTruncated {
        /// Observed decoded byte length.
        decoded_len: usize,
    },
    /// The decoded profile header is missing its `acsp` file signature.
    ProfileAcspMissing,
    /// The ICC header's declared profile size differs from the decoded length.
    ProfileDeclaredSizeMismatch {
        /// Declared profile size from the ICC header (big-endian bytes `0..4`).
        declared: u32,
        /// Decoded profile byte length.
        decoded_len: usize,
    },
    /// `/N` disagrees with the component count implied by a recognized ICC data
    /// colour-space signature.
    ProfileComponentCountMismatch {
        /// Parsed `/N` value.
        n: usize,
        /// Raw ICC data colour-space signature, header bytes `16..20`.
        data_color_space_signature: [u8; 4],
    },
    /// The ICC profile/device class is not one allowed for a PDF `ICCBased`
    /// source profile (`scnr`, `mntr`, `prtr`, `spac`); `link` and unknown
    /// classes are reported.
    ProfileClassDisallowed {
        /// Raw ICC profile/device class signature, header bytes `12..16`.
        profile_class_signature: [u8; 4],
    },
    /// The profile stream could not be decoded and inspected; the reason is a
    /// structured coverage gap, not a profile anomaly.
    ProfileInspectionGap {
        /// Why bounded header inspection could not reach decoded bytes.
        reason: IccProfileInspectionGap,
    },
}

/// Hard cap on decoded ICC profile bytes per unique profile stream.
///
/// The declared-size comparison needs the full decoded length, so the profile is
/// fully inflated under this bound; real embedded profiles (including
/// `DeviceLink` profiles with large CLUTs) sit well under it, and anything
/// larger becomes a
/// bounded [`IccProfileInspectionGap::DecodeOutputLimitExceeded`] gap rather than
/// an unbounded allocation.
const MAX_DECODED_ICC_PROFILE_BYTES: usize = 8 * 1024 * 1024;

/// Run the page-scope `ICCBased` dictionary and profile-header descriptor pass.
///
/// Each unique profile stream is inflated at most once, bounded by
/// [`MAX_DECODED_ICC_PROFILE_BYTES`]; the decoded buffer is dropped after header
/// facts are extracted. Descriptor inspection is deduplicated per [`IndirectRef`]
/// so a profile shared by several resources or defaults is decoded once, but
/// findings are still emitted per use-site anchor in deterministic order.
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

    let mut descriptors = DescriptorScan::new(input, lookup, MAX_DECODED_ICC_PROFILE_BYTES);
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
            collect_use_site_findings(
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
                &mut descriptors,
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
                collect_use_site_findings(
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
                    &mut descriptors,
                    &mut findings,
                );
            }
        }
    }
    findings
}

/// Deduplicating bounded profile-header inspection over a document.
///
/// Each unique profile reference is decoded and header-parsed at most once; the
/// small `Copy` [`IccProfileStreamInspection`] result is memoized by
/// [`IndirectRef`] so repeated use-sites reuse it.
struct DescriptorScan<'a> {
    input: &'a [u8],
    lookup: ObjectLookup<'a>,
    cap: usize,
    memo: BTreeMap<IndirectRef, IccProfileStreamInspection>,
}

impl<'a> DescriptorScan<'a> {
    const fn new(input: &'a [u8], lookup: ObjectLookup<'a>, cap: usize) -> Self {
        Self {
            input,
            lookup,
            cap,
            memo: BTreeMap::new(),
        }
    }

    fn inspect(&mut self, reference: IndirectRef) -> IccProfileStreamInspection {
        if let Some(inspection) = self.memo.get(&reference) {
            return *inspection;
        }
        let inspection =
            inspect_icc_profile_stream_with_lookup(self.input, self.lookup, reference, self.cap);
        self.memo.insert(reference, inspection);
        inspection
    }
}

/// Emit the dictionary-level findings then the profile-header descriptor
/// findings for one `ICCBased` use-site, in deterministic order.
fn collect_use_site_findings(
    page: PageIndex,
    source: IccBasedFindingSource,
    resource_name: &PdfName,
    facts: IccDescriptorFacts,
    descriptors: &mut DescriptorScan<'_>,
    findings: &mut Vec<IccBasedFinding>,
) {
    collect_definition_findings(page, source, resource_name, facts, findings);
    collect_descriptor_findings(page, source, resource_name, facts, descriptors, findings);
}

/// Emit bounded profile-header descriptor findings for one use-site.
///
/// Non-`ICCBased` families and `ICCBased` definitions without a referenced
/// profile stream produce nothing. The referenced profile is inspected (once per
/// reference), and each header fact/gap becomes an additive finding; `/N` vs
/// data-space component mismatch is only checked when both `/N` and a recognized
/// data-space component count are available.
fn collect_descriptor_findings(
    page: PageIndex,
    source: IccBasedFindingSource,
    resource_name: &PdfName,
    facts: IccDescriptorFacts,
    descriptors: &mut DescriptorScan<'_>,
    findings: &mut Vec<IccBasedFinding>,
) {
    if facts.family != ColorSpaceFamily::IccBased {
        return;
    }
    let Some(reference) = facts.icc_profile_stream else {
        return;
    };

    match descriptors.inspect(reference) {
        IccProfileStreamInspection::Parsed { descriptor } => {
            let mut push = |kind| findings.push(finding(page, source, resource_name, facts, kind));
            if !descriptor.acsp_present {
                push(IccBasedFindingKind::ProfileAcspMissing);
            }
            if descriptor.declared_profile_size as usize != descriptor.decoded_len {
                push(IccBasedFindingKind::ProfileDeclaredSizeMismatch {
                    declared: descriptor.declared_profile_size,
                    decoded_len: descriptor.decoded_len,
                });
            }
            if let (Some(n), Some(components)) = (
                facts.component_count,
                descriptor.data_space_component_count(),
            ) {
                if n != components {
                    push(IccBasedFindingKind::ProfileComponentCountMismatch {
                        n,
                        data_color_space_signature: descriptor.data_color_space_signature,
                    });
                }
            }
            if !is_allowed_pdf_icc_class(descriptor.profile_class_signature) {
                push(IccBasedFindingKind::ProfileClassDisallowed {
                    profile_class_signature: descriptor.profile_class_signature,
                });
            }
        }
        IccProfileStreamInspection::Truncated { decoded_len } => findings.push(finding(
            page,
            source,
            resource_name,
            facts,
            IccBasedFindingKind::ProfileHeaderTruncated { decoded_len },
        )),
        IccProfileStreamInspection::Gap { reason } => findings.push(finding(
            page,
            source,
            resource_name,
            facts,
            IccBasedFindingKind::ProfileInspectionGap { reason },
        )),
    }
}

/// Whether an ICC profile/device class is allowed for a PDF `ICCBased` source
/// profile.
///
/// Conservative and documented: PDF `ICCBased` streams describe an input
/// (`scnr`), display (`mntr`), output (`prtr`), or colour-space (`spac`)
/// profile. Abstract, `link` (`DeviceLink`), named-colour, and unknown classes
/// are reported rather than accepted.
const fn is_allowed_pdf_icc_class(signature: [u8; 4]) -> bool {
    matches!(&signature, b"scnr" | b"mntr" | b"prtr" | b"spac")
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
