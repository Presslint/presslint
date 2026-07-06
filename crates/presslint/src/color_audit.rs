//! Read-only document color-usage audit over the neutral PDF inventory.
//!
//! [`audit_color_usage`] builds the backend-neutral [`PdfInventory`] with
//! [`build_pdf_inventory`], then scans the merged, page-ordered inventory once
//! and reports what the engine observed: deterministic document-level and
//! per-page counts by [`ColorSpace`], [`ColorUsage`], and [`ObjectKind`],
//! deduplicated spot-colorant names, explicit `DeviceRGB` findings, per-page
//! declared graphics-state findings from the classified page `/ExtGState`
//! resources (see `graphics_state_findings`), and a list of coverage gaps where
//! the current engine could not fully classify color or graphics state.
//!
//! This is CHK2: a descriptive audit, not a print-safety verdict. It does NOT
//! compute ink limits / TAC, does NOT run ICC transforms, resolve alternate
//! color spaces, or infer an `Indexed` base, and does NOT plan actions or mutate
//! bytes. Its only claim is `Complete` (every observation was classified into a
//! modeled space) versus `Incomplete` (at least one coverage gap remains). It is
//! a companion to the fixed-policy `check_no_rgb_in_print` preflight, not a
//! replacement.
//!
//! A page that simply declares no `/Resources` or no `/XObject` dictionary is
//! not a coverage gap: there is no `XObject` color to miss. Only resource skips
//! that describe a present-but-unclassifiable `XObject` count as gaps.
//!
//! The audit lives in the umbrella crate, not `presslint-actions`: it plans
//! nothing, mutates nothing, and retains no PDF source bytes, decoded streams,
//! image samples, ICC/profile bytes, or color component vectors beyond the owned
//! [`PdfInventory`] moved into the report exactly once.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use presslint_inventory::InventoryEntry;
use presslint_types::{ColorObservation, ColorSpace, ColorUsage, ObjectId, ObjectKind, PageIndex};
use serde::{Deserialize, Serialize};

use crate::graphics_state_findings::{
    GraphicsStateFinding, GraphicsStateScan, scan_document_graphics_state,
};
use crate::pdf_inventory::{
    PdfInventory, PdfInventoryError, PdfInventoryPageResult, build_pdf_inventory,
};

/// Overall audit completeness.
///
/// This is deliberately not a pass/fail or print-safe verdict. It only states
/// whether every observed color was classified into a modeled space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorAuditStatus {
    /// Every color observation the engine saw was classified into a modeled
    /// device space and every page/resource was fully inspected: no coverage
    /// gaps. This does NOT claim print-safety.
    Complete,
    /// At least one coverage gap exists (skipped page/resource, undecoded image
    /// color, skipped form expansion, resource-inspection error, or an unmodeled
    /// or unresolved color space).
    Incomplete,
}

/// One color-space count, per [`ColorObservation`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorSpaceCount {
    /// Observed color space.
    pub color_space: ColorSpace,
    /// Number of color observations in this space.
    pub count: usize,
}

/// One color-usage count, per [`ColorObservation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorUsageCount {
    /// Observed paint use.
    pub usage: ColorUsage,
    /// Number of color observations with this usage.
    pub count: usize,
}

/// One object-kind count, per [`InventoryEntry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectKindCount {
    /// Inventory object class.
    pub kind: ObjectKind,
    /// Number of inventory entries with this class.
    pub count: usize,
}

/// Deterministic color-usage counts.
///
/// `color_space_counts` and `color_usage_counts` count each
/// [`ColorObservation`]; `object_kind_counts` counts each [`InventoryEntry`].
/// All three vectors are sorted by a stable variant order (color spaces break
/// `Resource` ties by raw name bytes), because the underlying key types do not
/// implement `Ord`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorUsageSummary {
    /// Color-space counts, one per observation, in stable variant order.
    pub color_space_counts: Vec<ColorSpaceCount>,
    /// Color-usage counts, one per observation, in stable variant order.
    pub color_usage_counts: Vec<ColorUsageCount>,
    /// Object-kind counts, one per inventory entry, in stable variant order.
    pub object_kind_counts: Vec<ObjectKindCount>,
}

/// Per-page color-usage counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageColorUsage {
    /// Zero-based document-order page ordinal.
    pub page: PageIndex,
    /// Counts over this page's contiguous inventory entry run. Empty for a
    /// skipped page.
    pub summary: ColorUsageSummary,
}

/// One explicit `DeviceRGB` observation.
///
/// The observed space is always `DeviceRGB`, so it is not repeated; the finding
/// carries object identity, entry index, object kind, and usage so a caller can
/// locate the marking object without rescanning the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RgbFinding {
    /// Page the finding is anchored to.
    pub page: PageIndex,
    /// Inventory object identity.
    pub object: ObjectId,
    /// Stable index into `audit.inventory.inventory.entries`.
    pub entry_index: usize,
    /// Inventory object class.
    pub kind: ObjectKind,
    /// Color usage of the `DeviceRGB` observation.
    pub usage: ColorUsage,
}

/// Why a coverage gap was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGapKind {
    /// A page was intentionally skipped by the inventory bridge, so its content
    /// color was never observed.
    SkippedPage,
    /// An image observation is still modeled as `Unknown`: image color is not
    /// decoded yet.
    ImageColorUndecoded,
    /// A Form `XObject` expansion was skipped, so color inside that form is not
    /// observed.
    FormExpansionSkipped,
    /// A page-scope `XObject` resource could not be classified.
    PageResourceSkipped,
    /// A page-scope `/Resources /ColorSpace` resource could not be classified
    /// (present but unresolvable: pattern/indexed/Lab/CalGray/CalRGB, an
    /// unresolved reference, or a malformed operand).
    ColorSpaceResourceSkipped,
    /// The document-level page `XObject` resource inspection could not begin.
    ResourceInspectionError,
    /// The document-level `/Resources /ColorSpace` resource inspection could not
    /// begin.
    ColorSpaceResourceInspectionError,
    /// A marking object observed a color space this audit neither models nor
    /// resolves (`CalGray`, `CalRgb`, `Lab`, `Indexed`, `Pattern`,
    /// `Resource(_)`, or non-image `Unknown`).
    ///
    /// Resource colours resolved to `IccBased`/`Separation`/`DeviceN` are NOT
    /// gaps: they are honest observations of the real source family.
    UnmodeledColorSpace,
    /// The document-level page `/Resources /ExtGState` inspection could not
    /// begin, so whether any page declares overprint/transparency-relevant
    /// graphics state is unknown.
    ExtGStateResourceInspectionError,
    /// A page `/ExtGState` resource was skipped in a way the graphics-state
    /// finding derivation cannot see (a duplicate or non-dictionary
    /// `/ExtGState`, an unscannable dictionary, a `/Resources` resolution
    /// failure, or a named entry shadowed by an earlier classified one).
    ///
    /// A named entry that could not be classified is NOT a gap: it surfaces as
    /// the [`GraphicsStateFinding`] `unresolved` flag instead.
    ExtGStateResourceSkipped,
}

/// One coverage gap: a place the audit could not fully classify color.
///
/// `page` is present for every gap except the document-level
/// [`CoverageGapKind::ResourceInspectionError`]. The object-anchored fields
/// (`object`, `entry_index`, `kind`, `usage`, `color_space`) are populated only
/// for entry-anchored gaps ([`CoverageGapKind::ImageColorUndecoded`] and
/// [`CoverageGapKind::UnmodeledColorSpace`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverageGap {
    /// Structured gap kind.
    pub kind: CoverageGapKind,
    /// Page the gap is anchored to, when page-scoped.
    pub page: Option<PageIndex>,
    /// Inventory object identity for entry-anchored gaps.
    pub object: Option<ObjectId>,
    /// Stable index into `audit.inventory.inventory.entries` for entry-anchored
    /// gaps.
    pub entry_index: Option<usize>,
    /// Inventory object class for entry-anchored gaps.
    pub kind_of_object: Option<ObjectKind>,
    /// Color usage for observation-anchored gaps.
    pub usage: Option<ColorUsage>,
    /// Observed color space for observation-anchored gaps.
    pub color_space: Option<ColorSpace>,
}

/// Read-only document color-usage audit report.
///
/// The full neutral [`PdfInventory`] is moved into `inventory` exactly once. The
/// summaries, findings, and gaps carry only small `Copy`/enum data plus cloned
/// [`ObjectId`], [`ColorSpace`], and spot [`PdfName`](presslint_types::PdfName)
/// values, never decoded streams, image samples, color components, or PDF source
/// bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorUsageAudit {
    /// Completeness verdict (`Complete` iff no coverage gap).
    pub status: ColorAuditStatus,
    /// Document-level counts over every inventoried entry/observation.
    pub document: ColorUsageSummary,
    /// One entry per enumerated page, in document order.
    pub pages: Vec<PageColorUsage>,
    /// Deduplicated spot-colorant names from `Separation`/`DeviceN`
    /// observations, sorted by raw PDF-name bytes.
    pub spot_names: Vec<presslint_types::PdfName>,
    /// Explicit `DeviceRGB` observations in document/page/entry/observation
    /// order.
    pub rgb_findings: Vec<RgbFinding>,
    /// Per-page DECLARED graphics-state findings in document page order, at
    /// most one per page per source; this slice emits page-scope findings only.
    ///
    /// These are declared-in-resources facts: a page's effective `/ExtGState`
    /// resource that sets overprint/transparency-relevant state counts even if
    /// no `gs` ever selects it (see [`GraphicsStateFinding`]). The field is
    /// additive: an empty vec is omitted from serialization and absent in older
    /// reports, so existing audit JSON shapes are unchanged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub graphics_state_findings: Vec<GraphicsStateFinding>,
    /// Coverage gaps in document order: the inventory-scan gaps first, then any
    /// graphics-state inspection gaps (page order, document-level last).
    pub coverage_gaps: Vec<CoverageGap>,
    /// Neutral inventory the audit ran over, owned by the report.
    pub inventory: PdfInventory,
}

/// Run the read-only document color-usage audit over PDF bytes.
///
/// The document/page path is [`build_pdf_inventory`] verbatim, so top-level
/// document-access/build failures propagate unchanged as [`PdfInventoryError`].
/// Per-page problems are already structured page skips inside the inventory and
/// surface only as [`CoverageGap`] records, never as a hard error.
///
/// # Errors
///
/// Returns the same [`PdfInventoryError`] as [`build_pdf_inventory`] when the
/// neutral document/page-content path cannot be established.
pub fn audit_color_usage(
    input: &[u8],
    max_decoded_stream_bytes: usize,
) -> Result<ColorUsageAudit, PdfInventoryError> {
    let inventory = build_pdf_inventory(input, max_decoded_stream_bytes)?;
    // Graphics-state findings need document access the owned inventory does not
    // carry, so they are derived here in the outer composing function (the same
    // dictionary-only inspection the bridges already ran) and folded into the
    // pure build. `build_color_usage_audit` itself stays pure over its input.
    let graphics_state = scan_document_graphics_state(input);
    Ok(build_audit(inventory, graphics_state))
}

/// Analyze an owned neutral inventory and assemble the read-only audit.
///
/// Split from [`audit_color_usage`] so the pure inventory-to-report scan can be
/// exercised over synthetic inventories without building a PDF. The inventory is
/// scanned by borrow and then moved into the returned report; it is never
/// cloned. Graphics-state findings need document access, so this pure path
/// carries none: `graphics_state_findings` is empty and no `ExtGState` gap is
/// recorded, exactly the pre-finding behaviour.
///
/// Crate-internal and now exercised only by the synthetic-inventory tests
/// (`audit_color_usage` composes [`build_audit`] with the graphics-state pass
/// directly), so it is compiled for tests only.
#[cfg(test)]
#[must_use]
pub fn build_color_usage_audit(inventory: PdfInventory) -> ColorUsageAudit {
    build_audit(inventory, GraphicsStateScan::default())
}

/// Fold the inventory scan and the graphics-state pass into one report.
///
/// `ExtGState` coverage gaps append after the inventory-scan gaps (they are
/// produced by a separate document pass), and the status verdict is computed
/// over the combined gap list.
fn build_audit(inventory: PdfInventory, graphics_state: GraphicsStateScan) -> ColorUsageAudit {
    let scan = scan_inventory(&inventory);
    let mut coverage_gaps = scan.coverage_gaps;
    coverage_gaps.extend(graphics_state.coverage_gaps);
    let status = if coverage_gaps.is_empty() {
        ColorAuditStatus::Complete
    } else {
        ColorAuditStatus::Incomplete
    };
    ColorUsageAudit {
        status,
        document: scan.document.finish(),
        pages: scan.pages,
        spot_names: scan.spot_names.into_iter().collect(),
        rgb_findings: scan.rgb_findings,
        graphics_state_findings: graphics_state.findings,
        coverage_gaps,
        inventory,
    }
}

/// Intermediate scan output, before the owned inventory is folded back in.
struct Scan {
    document: SummaryAccumulator,
    pages: Vec<PageColorUsage>,
    spot_names: BTreeSet<presslint_types::PdfName>,
    rgb_findings: Vec<RgbFinding>,
    coverage_gaps: Vec<CoverageGap>,
}

/// Scan pages and entries in lockstep, matching the `preflight.rs` cursor
/// discipline: each `Inventoried { entry_count }` bounds a page's contiguous
/// entry run, so per-page and document counts, spot names, RGB findings, and
/// coverage gaps are all produced deterministically in document order.
fn scan_inventory(inventory: &PdfInventory) -> Scan {
    let entries = &inventory.inventory.entries;
    let mut document = SummaryAccumulator::default();
    let mut pages = Vec::with_capacity(inventory.pages.len());
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
            if is_unclassified_color_space_skip(&skip.reason) {
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
                for (offset, entry) in entries[cursor..end].iter().enumerate() {
                    scan_entry(
                        cursor + offset,
                        entry,
                        &mut page_summary,
                        &mut document,
                        &mut spot_names,
                        &mut rgb_findings,
                        &mut coverage_gaps,
                    );
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
        coverage_gaps.push(CoverageGap {
            kind: CoverageGapKind::ResourceInspectionError,
            page: None,
            object: None,
            entry_index: None,
            kind_of_object: None,
            usage: None,
            color_space: None,
        });
    }
    if inventory.color_space_resource_error.is_some() {
        coverage_gaps.push(CoverageGap {
            kind: CoverageGapKind::ColorSpaceResourceInspectionError,
            page: None,
            object: None,
            entry_index: None,
            kind_of_object: None,
            usage: None,
            color_space: None,
        });
    }

    Scan {
        document,
        pages,
        spot_names,
        rgb_findings,
        coverage_gaps,
    }
}

/// Scan one inventory entry: count its kind once, then classify each color
/// observation in observation order.
fn scan_entry(
    entry_index: usize,
    entry: &InventoryEntry,
    page_summary: &mut SummaryAccumulator,
    document: &mut SummaryAccumulator,
    spot_names: &mut BTreeSet<presslint_types::PdfName>,
    rgb_findings: &mut Vec<RgbFinding>,
    coverage_gaps: &mut Vec<CoverageGap>,
) {
    page_summary.add_entry_kind(entry.kind);
    document.add_entry_kind(entry.kind);
    for observation in &entry.colors {
        page_summary.add_observation(observation);
        document.add_observation(observation);
        collect_spot_name(observation, spot_names);
        classify_observation(entry_index, entry, observation, rgb_findings, coverage_gaps);
    }
}

/// Collect a spot-colorant name conservatively: only when the observation space
/// is `Separation` or `DeviceN` and a name is present. The `BTreeSet` both
/// deduplicates and orders by raw `PdfName` bytes.
fn collect_spot_name(
    observation: &ColorObservation,
    spot_names: &mut BTreeSet<presslint_types::PdfName>,
) {
    if matches!(
        observation.space,
        ColorSpace::Separation | ColorSpace::DeviceN
    ) {
        if let Some(name) = &observation.spot_name {
            spot_names.insert(name.clone());
        }
    }
}

/// Classify a single color observation into at most one RGB finding or one
/// coverage gap. Modeled process spaces (`DeviceCMYK`/`DeviceGray`) produce
/// neither.
fn classify_observation(
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
const fn is_unclassified_color_space_skip(
    reason: &presslint_pdf::SkippedColorSpaceResourceReason,
) -> bool {
    use presslint_pdf::SkippedColorSpaceResourceReason as Reason;
    use presslint_pdf::SkippedPageXObjectResourceReason as ResourcesReason;
    match reason {
        Reason::MissingColorSpaceResources | Reason::MissingColorSpace => false,
        Reason::Resources { resources_reason } => !matches!(
            resources_reason,
            ResourcesReason::MissingResources | ResourcesReason::MissingXObject
        ),
        _ => true,
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

/// Mutable count accumulator finalized into sorted [`ColorUsageSummary`] vectors.
///
/// Color spaces, usages, and kinds are all counted with the shared [`bump`]
/// linear probe and sorted into a fixed variant order by [`finish`].
///
/// [`finish`]: SummaryAccumulator::finish
#[derive(Default)]
struct SummaryAccumulator {
    color_spaces: Vec<(ColorSpace, usize)>,
    usages: Vec<(ColorUsage, usize)>,
    kinds: Vec<(ObjectKind, usize)>,
}

impl SummaryAccumulator {
    fn add_observation(&mut self, observation: &ColorObservation) {
        bump(&mut self.color_spaces, &observation.space);
        bump(&mut self.usages, &observation.usage);
    }

    fn add_entry_kind(&mut self, kind: ObjectKind) {
        bump(&mut self.kinds, &kind);
    }

    fn finish(mut self) -> ColorUsageSummary {
        self.color_spaces
            .sort_by(|a, b| color_space_cmp(&a.0, &b.0));
        self.usages
            .sort_by_key(|(usage, _)| color_usage_rank(*usage));
        self.kinds.sort_by_key(|(kind, _)| object_kind_rank(*kind));
        ColorUsageSummary {
            color_space_counts: self
                .color_spaces
                .into_iter()
                .map(|(color_space, count)| ColorSpaceCount { color_space, count })
                .collect(),
            color_usage_counts: self
                .usages
                .into_iter()
                .map(|(usage, count)| ColorUsageCount { usage, count })
                .collect(),
            object_kind_counts: self
                .kinds
                .into_iter()
                .map(|(kind, count)| ObjectKindCount { kind, count })
                .collect(),
        }
    }
}

/// Increment the count for `value`, appending a fresh slot the first time it is
/// seen. One helper for every count kind (color space, usage, object kind):
/// distinct keys are few, so this linear probe is cheaper than a hash map and
/// needs no `Hash`/`Ord` bound on the key type.
fn bump<T: Clone + PartialEq>(counts: &mut Vec<(T, usize)>, value: &T) {
    if let Some(slot) = counts.iter_mut().find(|(existing, _)| existing == value) {
        slot.1 += 1;
    } else {
        counts.push((value.clone(), 1));
    }
}

/// Total order over color spaces: by a fixed variant rank, breaking `Resource`
/// ties by raw name bytes. The key type does not implement `Ord`, so this makes
/// the summary deterministic.
fn color_space_cmp(a: &ColorSpace, b: &ColorSpace) -> Ordering {
    color_space_rank(a)
        .cmp(&color_space_rank(b))
        .then_with(|| match (a, b) {
            (ColorSpace::Resource(x), ColorSpace::Resource(y)) => x.cmp(y),
            _ => Ordering::Equal,
        })
}

const fn color_space_rank(space: &ColorSpace) -> u8 {
    match space {
        ColorSpace::DeviceGray => 0,
        ColorSpace::DeviceRgb => 1,
        ColorSpace::DeviceCmyk => 2,
        ColorSpace::IccBased => 3,
        ColorSpace::Lab => 4,
        ColorSpace::CalGray => 5,
        ColorSpace::CalRgb => 6,
        ColorSpace::Indexed => 7,
        ColorSpace::Separation => 8,
        ColorSpace::DeviceN => 9,
        ColorSpace::Pattern => 10,
        ColorSpace::Resource(_) => 11,
        ColorSpace::Unknown => 12,
    }
}

const fn color_usage_rank(usage: ColorUsage) -> u8 {
    match usage {
        ColorUsage::Fill => 0,
        ColorUsage::Stroke => 1,
        ColorUsage::Image => 2,
        ColorUsage::Shading => 3,
    }
}

const fn object_kind_rank(kind: ObjectKind) -> u8 {
    match kind {
        ObjectKind::Text => 0,
        ObjectKind::Vector => 1,
        ObjectKind::Image => 2,
        ObjectKind::FormXObject => 3,
        ObjectKind::Shading => 4,
        ObjectKind::Pattern => 5,
        ObjectKind::Annotation => 6,
    }
}
