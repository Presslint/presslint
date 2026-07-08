//! Serde report DTOs for the color-usage audit. No logic lives here: every
//! type is an owned, stable, JSON-friendly boundary contract assembled by the
//! module root.

use presslint_types::{ColorSpace, ColorUsage, ObjectId, ObjectKind, PageIndex};
use serde::{Deserialize, Serialize};

use crate::color_environment::OutputIntentEligibility;
use crate::default_color_space_findings::DefaultColorSpaceFinding;
use crate::graphics_state_findings::GraphicsStateFinding;
use crate::icc_based_findings::IccBasedFinding;
use crate::pdf_inventory::{PdfInventory, PdfInventoryError};

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

/// One color-space count, per [`ColorObservation`](presslint_types::ColorObservation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorSpaceCount {
    /// Observed color space.
    pub color_space: ColorSpace,
    /// Number of color observations in this space.
    pub count: usize,
}

/// One color-usage count, per [`ColorObservation`](presslint_types::ColorObservation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorUsageCount {
    /// Observed paint use.
    pub usage: ColorUsage,
    /// Number of color observations with this usage.
    pub count: usize,
}

/// One object-kind count, per [`InventoryEntry`](presslint_inventory::InventoryEntry).
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
/// [`ColorObservation`](presslint_types::ColorObservation);
/// `object_kind_counts` counts each
/// [`InventoryEntry`](presslint_inventory::InventoryEntry).
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
    /// The document-level page default colour-space inspection could not begin,
    /// so whether any page declares `/DefaultGray`, `/DefaultRGB`, or
    /// `/DefaultCMYK` is unknown.
    DefaultColorSpaceInspectionError,
    /// A present page-scope default colour-space entry could not be classified,
    /// so the audit cannot report whether that default is a non-trivial
    /// replacement.
    DefaultColorSpaceSkipped,
    /// A page `/ExtGState` resource was skipped in a way the graphics-state
    /// finding derivation cannot see (a duplicate or non-dictionary
    /// `/ExtGState`, an unscannable dictionary, a `/Resources` resolution
    /// failure, or a named entry shadowed by an earlier classified one).
    ///
    /// A named entry that could not be classified is NOT a gap: it surfaces as
    /// the [`GraphicsStateFinding`] `unresolved` flag instead.
    ExtGStateResourceSkipped,
    /// The document-level page `/Group` inspection could not begin, so whether
    /// any page establishes a transparency group is unknown.
    TransparencyGroupInspectionError,
    /// A page top-level `/Group` entry was present but could not be classified
    /// as a transparency group or another known safe absence.
    TransparencyGroupSkipped,
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
    /// Page-scope declared default colour-space findings in document page
    /// order, then `/DefaultGray`, `/DefaultRGB`, `/DefaultCMYK` key order.
    ///
    /// These report that direct device-family colour observations on a page may
    /// be governed by a declared default colour-space environment. They do not
    /// alter `ColorObservation.space`, run colour conversion, or retain ICC
    /// profile bytes. The field is additive: an empty vec is omitted from
    /// serialization and absent in older reports.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_color_space_findings: Vec<DefaultColorSpaceFinding>,
    /// Page-scope `ICCBased` dictionary descriptor findings, ordered by source:
    /// named colour-space resources in page order, then default colour-space
    /// facts in page order. These report shallow descriptor divergences only;
    /// they do not decode profile streams, read ICC headers, apply alternates,
    /// or affect coverage-gap status.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub icc_based_findings: Vec<IccBasedFinding>,
    /// Optional report-only output-intent eligibility result.
    ///
    /// This is populated only when a caller supplies an explicit
    /// `OutputIntentPolicy`. The default audit path leaves it absent, preserving
    /// existing JSON shapes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_intent_eligibility: Option<OutputIntentEligibility>,
    /// Coverage gaps in document order: the inventory-scan gaps first, then any
    /// graphics-state inspection gaps (page order, document-level last).
    pub coverage_gaps: Vec<CoverageGap>,
    /// Neutral inventory the audit ran over, owned by the report.
    pub inventory: PdfInventory,
}

/// Error returned by the policy-aware color audit entry point.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ColorUsageAuditWithPolicyError {
    /// The normal color inventory/audit path failed.
    ColorUsage {
        /// Delegated inventory/audit failure.
        error: PdfInventoryError,
    },
    /// Catalog output-intent inspection failed before policy resolution.
    OutputIntent {
        /// Delegated output-intent inspection failure.
        error: presslint_pdf::OutputIntentsInspectionError,
    },
}
