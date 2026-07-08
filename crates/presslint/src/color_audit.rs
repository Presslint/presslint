//! Read-only document color-usage audit over the neutral PDF inventory.
//!
//! [`audit_color_usage`] builds the backend-neutral [`PdfInventory`] with
//! [`build_pdf_inventory`], then scans the merged, page-ordered inventory once
//! and reports what the engine observed: deterministic document-level and
//! per-page counts by [`ColorSpace`](presslint_types::ColorSpace),
//! [`ColorUsage`](presslint_types::ColorUsage), and
//! [`ObjectKind`](presslint_types::ObjectKind),
//! deduplicated spot-colorant names, explicit `DeviceRGB` findings, per-page
//! declared default colour-space findings from classified page `/ColorSpace`
//! defaults, per-page declared graphics-state findings from the classified page
//! `/ExtGState` resources (see `graphics_state_findings`), and a list of
//! coverage gaps where the current engine could not fully classify color or
//! graphics state.
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
//!
//! This module root keeps only the audit entry points and assembly; the report
//! DTOs live in `report`, the inventory pass in `scan`, per-observation
//! classification in `classify`, and count aggregation in `summary`.

mod classify;
mod report;
mod scan;
mod summary;

pub use classify::page_gap;
pub use report::{
    ColorAuditStatus, ColorSpaceCount, ColorUsageAudit, ColorUsageAuditWithPolicyError,
    ColorUsageCount, ColorUsageSummary, CoverageGap, CoverageGapKind, ObjectKindCount,
    PageColorUsage, RgbFinding,
};
pub use scan::Scan;

use scan::scan_inventory;

use crate::color_environment::evaluate_pdf_output_intent_eligibility;
use crate::default_color_space_findings::{
    DefaultColorSpaceScan, scan_document_default_color_spaces,
};
use crate::graphics_state_findings::{GraphicsStateScan, scan_document_graphics_state};
use crate::icc_based_findings::scan_document_icc_based_findings;
use crate::pdf_inventory::{PdfInventory, PdfInventoryError, build_pdf_inventory};

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
    // Resource findings need document access the owned inventory does not
    // carry, so they are derived here in the outer composing function (the same
    // dictionary-only inspection family the bridges already run) and folded
    // into the pure build. `build_color_usage_audit` itself stays pure over its
    // input.
    let scan = scan_inventory(&inventory);
    let graphics_state = scan_document_graphics_state(input);
    let default_color_spaces =
        scan_document_default_color_spaces(input, max_decoded_stream_bytes, &scan);
    let icc_based_findings = scan_document_icc_based_findings(input);
    Ok(build_audit(
        inventory,
        scan,
        graphics_state,
        default_color_spaces,
        icc_based_findings,
    ))
}

/// Run color-usage audit and attach output-intent eligibility for `policy`.
///
/// This is a report-only composition of the normal audit with catalog
/// output-intent observation. It performs no conversion and writes no PDF bytes.
///
/// # Errors
///
/// Returns the normal color-audit failure or the catalog output-intent
/// inspection failure, depending on which read-only pass failed.
pub fn audit_color_usage_with_output_intent_policy(
    input: &[u8],
    max_decoded_stream_bytes: usize,
    policy: &presslint_color::OutputIntentPolicy,
) -> Result<ColorUsageAudit, ColorUsageAuditWithPolicyError> {
    let mut audit = audit_color_usage(input, max_decoded_stream_bytes)
        .map_err(|error| ColorUsageAuditWithPolicyError::ColorUsage { error })?;
    audit.output_intent_eligibility = Some(
        evaluate_pdf_output_intent_eligibility(input, policy)
            .map_err(|error| ColorUsageAuditWithPolicyError::OutputIntent { error })?,
    );
    Ok(audit)
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
    let scan = scan_inventory(&inventory);
    build_audit(
        inventory,
        scan,
        GraphicsStateScan::default(),
        DefaultColorSpaceScan::default(),
        Vec::new(),
    )
}

/// Fold the inventory scan and the graphics-state pass into one report.
///
/// `ExtGState` coverage gaps append after the inventory-scan gaps (they are
/// produced by a separate document pass), and the status verdict is computed
/// over the combined gap list.
fn build_audit(
    inventory: PdfInventory,
    scan: Scan,
    graphics_state: GraphicsStateScan,
    default_color_spaces: DefaultColorSpaceScan,
    icc_based_findings: Vec<crate::icc_based_findings::IccBasedFinding>,
) -> ColorUsageAudit {
    let mut coverage_gaps = scan.coverage_gaps;
    coverage_gaps.extend(default_color_spaces.coverage_gaps);
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
        default_color_space_findings: default_color_spaces.findings,
        icc_based_findings,
        output_intent_eligibility: None,
        coverage_gaps,
        inventory,
    }
}
