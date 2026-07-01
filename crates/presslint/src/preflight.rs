//! Read-only `no DeviceRGB in print` preflight over the neutral PDF inventory.
//!
//! [`check_no_rgb_in_print`] builds the backend-neutral [`PdfInventory`] with
//! [`build_pdf_inventory`], then scans the merged, page-ordered inventory once
//! and applies a fixed prepress color policy: `DeviceRGB` in marking content is
//! an error, `DeviceCMYK`/`DeviceGray` are pass-compatible, every other observed
//! color space needs human review, and three honesty signals record where the
//! current engine cannot yet see (skipped pages, undecoded image color, and
//! skipped Form `XObject` expansion diagnostics).
//!
//! This is a read-only check. It lives in the umbrella crate, not
//! `presslint-actions`: it plans nothing, mutates nothing, and retains no PDF
//! source bytes beyond the owned [`PdfInventory`] moved into the report.

use presslint_inventory::InventoryEntry;
use presslint_types::{ColorObservation, ColorSpace, ColorUsage, ObjectId, ObjectKind, PageIndex};
use serde::{Deserialize, Serialize};

use crate::pdf_inventory::{
    PdfInventory, PdfInventoryError, PdfInventoryPageResult, build_pdf_inventory,
};

/// Read-only preflight check discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightCheck {
    /// "No `DeviceRGB` in print content" preflight.
    NoRgbInPrint,
}

/// Overall preflight verdict.
///
/// See [`aggregate_status`] for the exact aggregation rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightStatus {
    /// No observed `DeviceRGB` in inventoried marking content and no review or
    /// coverage blocker. This does NOT claim "no RGB anywhere": it means nothing
    /// the engine inspected failed, subject to the recorded coverage limits.
    Pass,
    /// At least one `Error`-severity finding (observed `DeviceRGB`).
    Fail,
    /// No error, but at least one `Review`-severity finding or coverage gap.
    NeedsReview,
}

/// Severity of a single preflight finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightSeverity {
    /// A hard prepress failure that forces [`PreflightStatus::Fail`].
    Error,
    /// A finding a human must resolve; contributes to
    /// [`PreflightStatus::NeedsReview`].
    Review,
}

/// Why a preflight finding was emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreflightReason {
    /// A marking object observed a `DeviceRGB` color. Always an error.
    RgbDeviceColor,
    /// A marking object observed a color space this check neither passes nor
    /// fails on its own (`IccBased`, `CalGray`, `CalRgb`, `Lab`, `Indexed`,
    /// `Separation`, `DeviceN`, `Pattern`, `Resource(_)`, or `Unknown`). Always
    /// a review.
    UnmodeledOrUnresolvedColorSpace,
    /// The engine could not fully inspect some content: a skipped page, an image
    /// whose color is not yet decoded, or a real Form `XObject` expansion skip.
    /// Always a review.
    CoverageIncomplete,
}

/// One preflight finding.
///
/// The page is always present. The object-anchored fields (`object`,
/// `entry_index`, `kind`, `usage`, `color_space`) are populated only for
/// findings tied to a concrete inventory entry:
///
/// - per-object color findings ([`PreflightReason::RgbDeviceColor`],
///   [`PreflightReason::UnmodeledOrUnresolvedColorSpace`]) carry all of them;
/// - an image-`Unknown` coverage finding carries all of them
///   (`usage = Image`, `color_space = Unknown`);
/// - skipped-page and form-skip coverage findings carry only the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreflightFinding {
    /// Check that produced this finding.
    pub check: PreflightCheck,
    /// Finding severity.
    pub severity: PreflightSeverity,
    /// Structured reason.
    pub reason: PreflightReason,
    /// Page the finding is anchored to.
    pub page: PageIndex,
    /// Inventory object identity for entry-anchored findings.
    pub object: Option<ObjectId>,
    /// Stable index into `report.inventory.inventory.entries` for entry-anchored
    /// findings.
    pub entry_index: Option<usize>,
    /// Inventory object class for entry-anchored findings.
    pub kind: Option<ObjectKind>,
    /// Color usage for color-observation findings.
    pub usage: Option<ColorUsage>,
    /// Observed color space for color-observation findings.
    pub color_space: Option<ColorSpace>,
}

/// Read-only preflight report.
///
/// The full neutral [`PdfInventory`] is moved into `inventory` exactly once;
/// `findings` carry only small `Copy`/enum data plus a cloned per-object
/// [`ObjectId`] and color-space/usage/kind discriminants, never decoded streams,
/// color components, or PDF source bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreflightReport {
    /// Check that produced this report.
    pub check: PreflightCheck,
    /// Aggregated verdict.
    pub status: PreflightStatus,
    /// Findings in document/page/entry/observation order.
    pub findings: Vec<PreflightFinding>,
    /// Neutral inventory the check ran over, owned by the report.
    pub inventory: PdfInventory,
}

/// Run the read-only "no `DeviceRGB` in print content" preflight over PDF bytes.
///
/// The document/page path is [`build_pdf_inventory`] verbatim, so top-level
/// document-access/build failures propagate unchanged as [`PdfInventoryError`].
/// Per-page problems are already structured page skips inside the inventory and
/// surface only as [`PreflightReason::CoverageIncomplete`] findings, never as a
/// hard error.
///
/// # Errors
///
/// Returns the same [`PdfInventoryError`] as [`build_pdf_inventory`] when the
/// neutral document/page-content path cannot be established.
pub fn check_no_rgb_in_print(
    input: &[u8],
    max_decoded_stream_bytes: usize,
) -> Result<PreflightReport, PdfInventoryError> {
    let inventory = build_pdf_inventory(input, max_decoded_stream_bytes)?;
    Ok(build_no_rgb_report(inventory))
}

/// Analyze an owned neutral inventory and assemble the read-only report.
///
/// Split from [`check_no_rgb_in_print`] so the pure inventory-to-report policy
/// can be exercised over synthetic inventories without building a PDF. The
/// inventory is scanned by borrow and then moved into the returned report; it is
/// never cloned.
pub fn build_no_rgb_report(inventory: PdfInventory) -> PreflightReport {
    let findings = collect_findings(&inventory);
    let status = aggregate_status(&findings);
    PreflightReport {
        check: PreflightCheck::NoRgbInPrint,
        status,
        findings,
        inventory,
    }
}

/// Collect findings in strict document/page/entry/observation order.
///
/// Pages are visited in document order. Each page is either skipped (one
/// coverage finding) or inventoried, in which case its entries — a contiguous
/// run tracked by `entry_count` — are scanned in content order. Walking pages
/// and entries in lockstep keeps skipped-page and per-object findings correctly
/// interleaved in document order in a single pass.
fn collect_findings(inventory: &PdfInventory) -> Vec<PreflightFinding> {
    let entries = &inventory.inventory.entries;
    let mut findings = Vec::new();
    let mut cursor = 0;
    for page in &inventory.pages {
        match &page.result {
            PdfInventoryPageResult::Skipped { .. } => {
                findings.push(page_coverage_finding(page.page_index));
            }
            PdfInventoryPageResult::Inventoried {
                entry_count,
                form_skipped,
            } => {
                let end = (cursor + entry_count).min(entries.len());
                for (offset, entry) in entries[cursor..end].iter().enumerate() {
                    entry_findings(cursor + offset, entry, &mut findings);
                }
                for _ in form_skipped {
                    findings.push(page_coverage_finding(page.page_index));
                }
                cursor = end;
            }
        }
    }
    findings
}

/// Emit the findings for one inventory entry in observation order.
fn entry_findings(entry_index: usize, entry: &InventoryEntry, out: &mut Vec<PreflightFinding>) {
    for observation in &entry.colors {
        if let Some(finding) = observation_finding(entry_index, entry, observation) {
            out.push(finding);
        }
    }
}

/// Classify a single color observation on an entry into at most one finding.
fn observation_finding(
    entry_index: usize,
    entry: &InventoryEntry,
    observation: &ColorObservation,
) -> Option<PreflightFinding> {
    // Image color is not decoded yet: an image observation modeled as `Unknown`
    // is a coverage gap, not an unmodeled-space review.
    if observation.usage == ColorUsage::Image && observation.space == ColorSpace::Unknown {
        return Some(entry_finding(
            PreflightSeverity::Review,
            PreflightReason::CoverageIncomplete,
            entry_index,
            entry,
            Some(observation),
        ));
    }
    match observation.space {
        ColorSpace::DeviceRgb => Some(entry_finding(
            PreflightSeverity::Error,
            PreflightReason::RgbDeviceColor,
            entry_index,
            entry,
            Some(observation),
        )),
        ColorSpace::DeviceCmyk | ColorSpace::DeviceGray => None,
        _ => Some(entry_finding(
            PreflightSeverity::Review,
            PreflightReason::UnmodeledOrUnresolvedColorSpace,
            entry_index,
            entry,
            Some(observation),
        )),
    }
}

/// Build a finding anchored to an inventory entry, optionally with a color
/// observation's usage/space.
fn entry_finding(
    severity: PreflightSeverity,
    reason: PreflightReason,
    entry_index: usize,
    entry: &InventoryEntry,
    observation: Option<&ColorObservation>,
) -> PreflightFinding {
    PreflightFinding {
        check: PreflightCheck::NoRgbInPrint,
        severity,
        reason,
        page: entry.id.page,
        object: Some(entry.id.clone()),
        entry_index: Some(entry_index),
        kind: Some(entry.kind),
        usage: observation.map(|observation| observation.usage),
        color_space: observation.map(|observation| observation.space.clone()),
    }
}

/// Build a page-anchored coverage finding for skipped page/form content.
const fn page_coverage_finding(page: PageIndex) -> PreflightFinding {
    PreflightFinding {
        check: PreflightCheck::NoRgbInPrint,
        severity: PreflightSeverity::Review,
        reason: PreflightReason::CoverageIncomplete,
        page,
        object: None,
        entry_index: None,
        kind: None,
        usage: None,
        color_space: None,
    }
}

/// Aggregate findings into a status.
///
/// Exactly: `Fail` if any `Error` finding; else `NeedsReview` if any `Review`
/// finding (unmodeled space or any coverage gap); else `Pass`.
fn aggregate_status(findings: &[PreflightFinding]) -> PreflightStatus {
    if findings
        .iter()
        .any(|finding| finding.severity == PreflightSeverity::Error)
    {
        PreflightStatus::Fail
    } else if findings
        .iter()
        .any(|finding| finding.severity == PreflightSeverity::Review)
    {
        PreflightStatus::NeedsReview
    } else {
        PreflightStatus::Pass
    }
}
