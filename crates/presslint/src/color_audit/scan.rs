//! Observation collection: one deterministic pass over the neutral inventory
//! producing the intermediate [`Scan`], plus the resource-skip predicates that
//! decide which structured skips hide color.

use std::collections::BTreeSet;

use presslint_inventory::InventoryEntry;
use presslint_types::{
    ByteRange, ColorObservation, ColorSpace, ContentScope, InvocationPath, ObjectKind, PageIndex,
};

use crate::pdf_inventory::{PdfInventory, PdfInventoryPageResult};

use super::classify::{classify_observation, page_gap, pageless_gap};
use super::report::{CoverageGap, CoverageGapKind, PageColorUsage, RgbFinding};
use super::summary::{SummaryAccumulator, bump};

/// Intermediate scan output, before the owned inventory is folded back in.
pub struct Scan {
    pub(super) document: SummaryAccumulator,
    pub(super) pages: Vec<PageColorUsage>,
    pub(super) page_form_invocations: Vec<PageFormInvocation>,
    pub(super) root_form_invocations: Vec<RootFormInvocationColorCounts>,
    pub(super) spot_names: BTreeSet<presslint_types::PdfName>,
    pub(super) rgb_findings: Vec<RgbFinding>,
    pub(super) coverage_gaps: Vec<CoverageGap>,
}

impl Scan {
    pub fn has_device_observations(&self) -> bool {
        self.pages.iter().any(|usage| {
            usage
                .summary
                .color_space_counts
                .iter()
                .any(|count| is_device_space(&count.color_space) && count.count > 0)
        })
    }

    pub fn count_page_observations(&self, page: PageIndex, space: &ColorSpace) -> usize {
        self.pages
            .iter()
            .find(|usage| usage.page == page)
            .and_then(|usage| {
                usage
                    .summary
                    .color_space_counts
                    .iter()
                    .find(|count| &count.color_space == space)
            })
            .map_or(0, |count| count.count)
    }

    pub fn root_form_invocations(&self) -> &[RootFormInvocationColorCounts] {
        &self.root_form_invocations
    }

    pub fn page_form_invocations(&self) -> &[PageFormInvocation] {
        &self.page_form_invocations
    }

    pub fn has_form_invocation_on_page(&self, page: PageIndex) -> bool {
        self.has_page_form_invocation_on_page(page)
            || self
                .root_form_invocations
                .iter()
                .any(|counts| counts.page == page)
    }

    pub fn has_page_form_invocation_on_page(&self, page: PageIndex) -> bool {
        self.page_form_invocations
            .iter()
            .any(|invocation| invocation.page == page)
    }

    pub fn count_root_form_observations(
        &self,
        page: PageIndex,
        invocation: &InvocationPath,
        space: &ColorSpace,
    ) -> usize {
        self.root_form_invocations
            .iter()
            .find(|counts| counts.page == page && &counts.invocation == invocation)
            .and_then(|counts| {
                counts
                    .color_space_counts
                    .iter()
                    .find(|(color_space, _)| color_space == space)
            })
            .map_or(0, |(_, count)| *count)
    }
}

/// Page-level Form `XObject` invocation entries, used to recover invoked root
/// forms even when the form itself paints nothing.
pub struct PageFormInvocation {
    pub page: PageIndex,
    pub range: Option<ByteRange>,
}

/// Device-colour counts for one page-level form invocation, in first-observed
/// inventory order. Only exact one-frame invocation paths are represented.
pub struct RootFormInvocationColorCounts {
    pub page: PageIndex,
    pub invocation: InvocationPath,
    color_space_counts: Vec<(ColorSpace, usize)>,
}

const fn is_device_space(space: &ColorSpace) -> bool {
    matches!(
        space,
        ColorSpace::DeviceGray | ColorSpace::DeviceRgb | ColorSpace::DeviceCmyk
    )
}

/// Scan pages and entries in lockstep, matching the `preflight.rs` cursor
/// discipline: each `Inventoried { entry_count }` bounds a page's contiguous
/// entry run, so per-page and document counts, spot names, RGB findings, and
/// coverage gaps are all produced deterministically in document order.
pub(super) fn scan_inventory(inventory: &PdfInventory) -> Scan {
    let entries = &inventory.inventory.entries;
    let mut document = SummaryAccumulator::default();
    let mut pages = Vec::with_capacity(inventory.pages.len());
    let mut page_form_invocations = Vec::new();
    let mut root_form_invocations = Vec::new();
    let mut spot_names = BTreeSet::new();
    let mut rgb_findings = Vec::new();
    let mut coverage_gaps = Vec::new();
    let mut cursor = 0;

    for page in &inventory.pages {
        // Page-scope resource-classification skips are coverage gaps regardless
        // of whether the page content itself was inventoried, but only when they
        // describe a resource the engine could not classify. A page that simply
        // declares no `/Resources` or no `/XObject` dictionary has no XObject
        // color to miss and must not be reported as a gap (see
        // `is_unclassified_resource_skip`).
        for skip in &page.xobject_resource_skipped {
            if is_unclassified_resource_skip(&skip.reason) {
                coverage_gaps.push(page_gap(
                    CoverageGapKind::PageResourceSkipped,
                    page.page_index,
                ));
            }
        }
        // Colour-space resource skips that describe a present-but-unresolvable
        // space (pattern/indexed/Lab/CalGray/CalRGB, unresolved reference,
        // malformed operand) hide colour and are honest coverage gaps. A page
        // that simply declares no `/Resources` or no `/ColorSpace` dictionary
        // has no colour to miss and is not a gap.
        for skip in &page.color_space_resource_skipped {
            if is_unclassified_color_space_skip(skip) {
                coverage_gaps.push(page_gap(
                    CoverageGapKind::ColorSpaceResourceSkipped,
                    page.page_index,
                ));
            }
        }

        let mut page_summary = SummaryAccumulator::default();
        match &page.result {
            PdfInventoryPageResult::Skipped { .. } => {
                coverage_gaps.push(page_gap(CoverageGapKind::SkippedPage, page.page_index));
            }
            PdfInventoryPageResult::Inventoried {
                entry_count,
                form_skipped,
            } => {
                let end = (cursor + entry_count).min(entries.len());
                {
                    let mut sinks = EntryScanSinks {
                        page_summary: &mut page_summary,
                        document: &mut document,
                        page_form_invocations: &mut page_form_invocations,
                        root_form_invocations: &mut root_form_invocations,
                        spot_names: &mut spot_names,
                        rgb_findings: &mut rgb_findings,
                        coverage_gaps: &mut coverage_gaps,
                    };
                    for (offset, entry) in entries[cursor..end].iter().enumerate() {
                        scan_entry(cursor + offset, entry, &mut sinks);
                    }
                }
                for _ in form_skipped {
                    coverage_gaps.push(page_gap(
                        CoverageGapKind::FormExpansionSkipped,
                        page.page_index,
                    ));
                }
                cursor = end;
            }
        }
        pages.push(PageColorUsage {
            page: page.page_index,
            summary: page_summary.finish(),
        });
    }

    // The document-level resource inspection could not begin at all: one
    // document-anchored gap after every page-scoped gap.
    if inventory.xobject_resource_error.is_some() {
        coverage_gaps.push(pageless_gap(CoverageGapKind::ResourceInspectionError));
    }
    if inventory.color_space_resource_error.is_some() {
        coverage_gaps.push(pageless_gap(
            CoverageGapKind::ColorSpaceResourceInspectionError,
        ));
    }

    Scan {
        document,
        pages,
        page_form_invocations,
        root_form_invocations,
        spot_names,
        rgb_findings,
        coverage_gaps,
    }
}

/// Scan one inventory entry: count its kind once, then classify each color
/// observation in observation order.
fn scan_entry(entry_index: usize, entry: &InventoryEntry, sinks: &mut EntryScanSinks<'_>) {
    sinks.page_summary.add_entry_kind(entry.kind);
    sinks.document.add_entry_kind(entry.kind);
    collect_page_form_invocation(entry, sinks.page_form_invocations);
    let root_invocation_index = root_invocation_index(sinks.root_form_invocations, entry);
    for observation in &entry.colors {
        sinks.page_summary.add_observation(observation);
        sinks.document.add_observation(observation);
        if let Some(index) = root_invocation_index {
            bump(
                &mut sinks.root_form_invocations[index].color_space_counts,
                &observation.space,
            );
        }
        collect_spot_names(observation, sinks.spot_names);
        classify_observation(
            entry_index,
            entry,
            observation,
            sinks.rgb_findings,
            sinks.coverage_gaps,
        );
    }
}

struct EntryScanSinks<'a> {
    page_summary: &'a mut SummaryAccumulator,
    document: &'a mut SummaryAccumulator,
    page_form_invocations: &'a mut Vec<PageFormInvocation>,
    root_form_invocations: &'a mut Vec<RootFormInvocationColorCounts>,
    spot_names: &'a mut BTreeSet<presslint_types::PdfName>,
    rgb_findings: &'a mut Vec<RgbFinding>,
    coverage_gaps: &'a mut Vec<CoverageGap>,
}

fn collect_page_form_invocation(
    entry: &InventoryEntry,
    page_form_invocations: &mut Vec<PageFormInvocation>,
) {
    if entry.kind == ObjectKind::FormXObject
        && entry.provenance.scope == ContentScope::Page
        && entry.provenance.invocation.is_none()
    {
        page_form_invocations.push(PageFormInvocation {
            page: entry.id.page,
            range: entry.provenance.range,
        });
    }
}

fn root_invocation_index(
    root_form_invocations: &mut Vec<RootFormInvocationColorCounts>,
    entry: &InventoryEntry,
) -> Option<usize> {
    let invocation = root_invocation(entry)?;
    if let Some(index) = root_form_invocations
        .iter()
        .position(|counts| counts.page == entry.id.page && counts.invocation == *invocation)
    {
        return Some(index);
    }
    root_form_invocations.push(RootFormInvocationColorCounts {
        page: entry.id.page,
        invocation: invocation.clone(),
        color_space_counts: Vec::new(),
    });
    Some(root_form_invocations.len() - 1)
}

fn root_invocation(entry: &InventoryEntry) -> Option<&InvocationPath> {
    let invocation = entry.provenance.invocation.as_ref()?;
    (invocation.frames.len() == 1).then_some(invocation)
}

/// Collect spot-colorant names conservatively: only when the observation space
/// is `Separation` or `DeviceN` and names are present. The `BTreeSet` both
/// deduplicates and orders by raw `PdfName` bytes.
fn collect_spot_names(
    observation: &ColorObservation,
    spot_names: &mut BTreeSet<presslint_types::PdfName>,
) {
    if !matches!(
        observation.space,
        ColorSpace::Separation | ColorSpace::DeviceN
    ) {
        return;
    }

    if observation.spot_names.is_empty() {
        if let Some(name) = &observation.spot_name {
            spot_names.insert(name.clone());
        }
    } else {
        spot_names.extend(observation.spot_names.iter().cloned());
    }
}

/// Decide whether a page `XObject` resource skip represents a resource the
/// engine could not classify (a genuine coverage gap) rather than the mere
/// absence of any resources.
///
/// `MissingResources`/`MissingXObject` mean the page declares no `/Resources`
/// or no `/XObject` dictionary: there is no `XObject` color to miss, so these
/// are not gaps. Every other skip reason concerns a resource that is present but
/// could not be resolved, classified, or subtyped, which does hide color.
const fn is_unclassified_resource_skip(
    reason: &presslint_pdf::SkippedPageXObjectResourceReason,
) -> bool {
    use presslint_pdf::SkippedPageXObjectResourceReason as Reason;
    !matches!(reason, Reason::MissingResources | Reason::MissingXObject)
}

/// Decide whether a colour-space resource skip hides colour (a genuine coverage
/// gap) rather than merely recording the absence of any colour-space resources.
///
/// `MissingColorSpaceResources`/`MissingColorSpace` and the delegated
/// `Resources` `MissingResources`/`MissingXObject` mean the page declares no
/// resources to classify. Every other skip concerns a colour space that is
/// present but could not be resolved or is deferred to a later slice, which does
/// hide colour.
fn is_unclassified_color_space_skip(skip: &presslint_pdf::SkippedColorSpaceResource) -> bool {
    use presslint_pdf::SkippedColorSpaceResourceReason as Reason;
    use presslint_pdf::SkippedPageXObjectResourceReason as ResourcesReason;
    if is_default_color_space_resource_name(skip.resource_name.as_ref()) {
        return false;
    }
    match &skip.reason {
        Reason::MissingColorSpaceResources | Reason::MissingColorSpace => false,
        Reason::Resources { resources_reason } => !matches!(
            resources_reason,
            ResourcesReason::MissingResources | ResourcesReason::MissingXObject
        ),
        _ => true,
    }
}

fn is_default_color_space_resource_name(name: Option<&presslint_pdf::PdfName>) -> bool {
    matches!(
        name.map(|name| name.0.as_slice()),
        Some(b"DefaultGray" | b"DefaultRGB" | b"DefaultCMYK")
    )
}
