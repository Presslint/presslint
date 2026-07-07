//! Default device colour-space findings for the colour-usage audit.
//!
//! This module derives the audit's `default_color_space_findings` surface from
//! classified page and root Form `XObject` `/Resources /ColorSpace`
//! `/DefaultGray`, `/DefaultRGB`, and `/DefaultCMYK` inspection. It is
//! report-only: default spaces are not applied to
//! [`ColorObservation`](presslint_types::ColorObservation) records and no
//! conversion, profile parsing, tint transform evaluation, or byte mutation
//! happens here.

use presslint_pdf::{
    ColorSpaceFamily, DefaultColorSpaceFact, DefaultColorSpaceKind, DocumentAccessBackend,
    ObjectLookup, PageXObjectResourceTarget, SkippedDefaultColorSpace,
    SkippedDefaultColorSpaceReason, SkippedPageXObjectResourceReason, inspect_document_access,
    inspect_document_page_default_color_spaces_with_lookup,
    inspect_document_page_xobject_resources_with_lookup, inspect_form_default_color_spaces,
    inspect_page_content_extents_with_lookup, inspect_page_content_targets_with_lookup,
    inspect_page_contents,
};
use presslint_syntax::{OperatorRecord, assemble_operators, tokenize};
use presslint_types::{ByteRange, ColorSpace, InvocationFrame, InvocationPath, PageIndex, PdfName};
use serde::{Deserialize, Serialize};

use crate::color_audit::{CoverageGap, CoverageGapKind, Scan, page_gap};
use crate::page_content::page_content_bytes;

/// Which resource scope a default colour-space finding was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultColorSpaceFindingSource {
    /// The page's effective `/Resources /ColorSpace` default entry (including
    /// page-tree inheritance).
    PageDefaultColorSpace,
    /// A page-level Form `XObject` invocation's own `/Resources /ColorSpace`
    /// default entry.
    FormDefaultColorSpace,
}

/// One declared default device colour-space implication.
///
/// A finding is emitted only when the resource scope declares a classified
/// non-trivial default for one device family and the inventory observed at
/// least one matching device-family colour in the same scope. The observation
/// remains in its original `Device*` family; this record reports only that the
/// declared resource environment may govern that source meaning.
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
    /// Root form resource name for form-scope findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub form_name: Option<PdfName>,
    /// Exact one-frame invocation path for form-scope findings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation: Option<InvocationPath>,
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
    max_decoded_stream_bytes: usize,
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
    let Ok(default_report) = inspect_document_page_default_color_spaces_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    ) else {
        if !has_device_observations {
            return DefaultColorSpaceScan::default();
        }
        return inspection_error_scan();
    };
    let xobject_report = inspect_document_page_xobject_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .ok();

    let mut scan = DefaultColorSpaceScan::default();
    for page in &default_report.pages {
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
        if !inventory_scan.has_form_invocation_on_page(page_index) {
            continue;
        }
        if let Some(xobject_report) = &xobject_report {
            if let Some(xobject_page) = xobject_report.pages.iter().find(|page| {
                u32::try_from(page.ordinal).is_ok_and(|ordinal| PageIndex(ordinal) == page_index)
            }) {
                scan_page_form_defaults(
                    input,
                    lookup,
                    max_decoded_stream_bytes,
                    page_index,
                    xobject_page,
                    inventory_scan,
                    &mut scan,
                );
            }
        } else {
            scan.coverage_gaps.push(pageless_gap(
                CoverageGapKind::DefaultColorSpaceInspectionError,
            ));
        }
    }
    scan
}

fn scan_page_form_defaults(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    max_decoded_stream_bytes: usize,
    page: PageIndex,
    xobject_page: &presslint_pdf::PageXObjectResourcesInspection,
    inventory_scan: &Scan,
    scan: &mut DefaultColorSpaceScan,
) {
    for invocation in page_root_invocations(
        input,
        lookup,
        max_decoded_stream_bytes,
        page,
        xobject_page,
        inventory_scan,
    ) {
        let report =
            inspect_form_default_color_spaces(input, lookup, invocation.target.object_byte_offset);
        for default in &report.defaults {
            if let Some(finding) = derive_form_finding(
                page,
                &invocation.form_name,
                &invocation.invocation,
                default,
                inventory_scan,
            ) {
                scan.findings.push(finding);
            }
        }
        for skip in &report.skipped {
            if is_present_default_skip(skip) {
                scan.coverage_gaps
                    .push(page_gap(CoverageGapKind::DefaultColorSpaceSkipped, page));
            }
        }
    }
}

struct RootFormInvocationTarget<'a> {
    form_name: PdfName,
    invocation: InvocationPath,
    target: &'a PageXObjectResourceTarget,
}

fn page_root_invocations<'a>(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    max_decoded_stream_bytes: usize,
    page: PageIndex,
    xobject_page: &'a presslint_pdf::PageXObjectResourcesInspection,
    inventory_scan: &Scan,
) -> Vec<RootFormInvocationTarget<'a>> {
    let mut invocations = if inventory_scan.has_page_form_invocation_on_page(page) {
        page_root_invocations_from_content(
            input,
            lookup,
            max_decoded_stream_bytes,
            page,
            xobject_page,
            inventory_scan,
        )
    } else {
        Vec::new()
    };
    for counts in inventory_scan
        .root_form_invocations()
        .iter()
        .filter(|counts| counts.page == page)
    {
        let frame = &counts.invocation.frames[0];
        if invocations
            .iter()
            .any(|invocation| invocation.invocation == counts.invocation)
        {
            continue;
        }
        if let Some(target) = find_form_target(&xobject_page.form_xobjects, &frame.name) {
            invocations.push(RootFormInvocationTarget {
                form_name: frame.name.clone(),
                invocation: counts.invocation.clone(),
                target,
            });
        }
    }
    invocations
}

fn page_root_invocations_from_content<'a>(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    max_decoded_stream_bytes: usize,
    page: PageIndex,
    xobject_page: &'a presslint_pdf::PageXObjectResourcesInspection,
    inventory_scan: &Scan,
) -> Vec<RootFormInvocationTarget<'a>> {
    let Ok(contents) = inspect_page_contents(input, xobject_page.page_object_byte_offset) else {
        return Vec::new();
    };
    let targets = inspect_page_content_targets_with_lookup(input, lookup, &contents);
    let extents = inspect_page_content_extents_with_lookup(input, lookup, &targets);
    let Ok(content) = page_content_bytes(input, &extents.entries, max_decoded_stream_bytes) else {
        return Vec::new();
    };
    let Ok(tokens) = tokenize(content.as_slice()) else {
        return Vec::new();
    };
    let Ok(assembled) = assemble_operators(&tokens) else {
        return Vec::new();
    };

    let mut invocations = Vec::new();
    let mut ordinal = 0u32;
    for record in &assembled.records {
        if content
            .as_slice()
            .get(record.operator.range.start..record.operator.range.end)
            != Some(b"Do")
        {
            continue;
        }
        let Some(name) = do_name(content.as_slice(), record) else {
            continue;
        };
        let Some(target) = find_form_target(&xobject_page.form_xobjects, &name) else {
            continue;
        };
        if inventory_scan
            .page_form_invocations()
            .iter()
            .any(|invocation| invocation.page == page && invocation.range == Some(record.range))
        {
            invocations.push(RootFormInvocationTarget {
                form_name: name.clone(),
                invocation: InvocationPath {
                    frames: vec![InvocationFrame { ordinal, name }],
                },
                target,
            });
        }
        ordinal = ordinal.saturating_add(1);
    }
    invocations
}

fn do_name(content: &[u8], record: &OperatorRecord) -> Option<PdfName> {
    let [operand] = record.operands.as_slice() else {
        return None;
    };
    let ByteRange { start, end } = operand.range;
    let bytes = content.get(start..end)?;
    bytes
        .strip_prefix(b"/")
        .map(|name| PdfName(name.to_vec()))
        .filter(|name| !name.0.is_empty())
}

fn find_form_target<'a>(
    targets: &'a [PageXObjectResourceTarget],
    name: &PdfName,
) -> Option<&'a PageXObjectResourceTarget> {
    targets
        .iter()
        .find(|target| target.name.0.as_slice() == name.0.as_slice())
}

fn derive_finding(
    page: PageIndex,
    fact: &DefaultColorSpaceFact,
    inventory_scan: &Scan,
) -> Option<DefaultColorSpaceFinding> {
    let (device_space, replacement_space) = non_trivial_spaces(fact)?;
    let matching_device_observation_count =
        inventory_scan.count_page_observations(page, &device_space);
    if matching_device_observation_count == 0 {
        return None;
    }
    Some(default_finding(
        page,
        fact,
        device_space,
        replacement_space,
        matching_device_observation_count,
        FindingAttribution {
            source: DefaultColorSpaceFindingSource::PageDefaultColorSpace,
            form_name: None,
            invocation: None,
        },
    ))
}

fn derive_form_finding(
    page: PageIndex,
    form_name: &PdfName,
    invocation: &InvocationPath,
    fact: &DefaultColorSpaceFact,
    inventory_scan: &Scan,
) -> Option<DefaultColorSpaceFinding> {
    let (device_space, replacement_space) = non_trivial_spaces(fact)?;
    let matching_device_observation_count =
        inventory_scan.count_root_form_observations(page, invocation, &device_space);
    if matching_device_observation_count == 0 {
        return None;
    }
    Some(default_finding(
        page,
        fact,
        device_space,
        replacement_space,
        matching_device_observation_count,
        FindingAttribution {
            source: DefaultColorSpaceFindingSource::FormDefaultColorSpace,
            form_name: Some(form_name.clone()),
            invocation: Some(invocation.clone()),
        },
    ))
}

fn non_trivial_spaces(fact: &DefaultColorSpaceFact) -> Option<(ColorSpace, ColorSpace)> {
    let device_space = default_device_space(fact.kind);
    let replacement_space = family_space(fact.color_space.family);
    (replacement_space != device_space).then_some((device_space, replacement_space))
}

fn default_finding(
    page: PageIndex,
    fact: &DefaultColorSpaceFact,
    device_space: ColorSpace,
    replacement_space: ColorSpace,
    matching_device_observation_count: usize,
    attribution: FindingAttribution,
) -> DefaultColorSpaceFinding {
    DefaultColorSpaceFinding {
        page,
        source: attribution.source,
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
        form_name: attribution.form_name,
        invocation: attribution.invocation,
    }
}

struct FindingAttribution {
    source: DefaultColorSpaceFindingSource,
    form_name: Option<PdfName>,
    invocation: Option<InvocationPath>,
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
