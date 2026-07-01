//! Document-level selector query over the neutral PDF inventory.
//!
//! This is the first end-to-end "query a real PDF" path: it builds the
//! backend-neutral flat [`PdfInventory`] with [`build_pdf_inventory`], then
//! evaluates a caller-supplied [`presslint_selectors::Selector`] over the
//! merged, page-ordered inventory. It returns stable indices into
//! `report.inventory.entries` rather than cloning matched entries, so the
//! full report is moved into the result once and matches carry only ids.

use presslint_selectors::Selector;
use presslint_types::PageIndex;
use serde::{Deserialize, Serialize};

use crate::pdf_inventory::{PdfInventory, PdfInventoryError, build_pdf_inventory};

/// Result of evaluating a selector over a neutral PDF inventory.
///
/// The full neutral inventory is moved into `report` exactly once; `matches`
/// carries only stable indices and page ordinals, never cloned entries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfInventoryQuery {
    /// Full neutral inventory the selector was evaluated over.
    pub report: PdfInventory,
    /// Selector matches in ascending `entry_index` order.
    pub matches: Vec<PdfInventoryMatch>,
}

/// One selector match into [`PdfInventoryQuery::report`].
///
/// `entry_index` is a stable index into `report.inventory.entries`;
/// `page_index` is the matched entry's own zero-based document-order page
/// ordinal (`entry.id.page`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfInventoryMatch {
    /// Stable index into `report.inventory.entries`.
    pub entry_index: usize,
    /// The matched entry's zero-based document-order page ordinal.
    pub page_index: PageIndex,
}

/// Build the neutral PDF inventory and evaluate a selector over it.
///
/// The document/page path is the existing [`build_pdf_inventory`] verbatim, so
/// the query is a strict superset (build, then select): top-level failures
/// surface as the same [`PdfInventoryError`] unchanged. On success the merged,
/// page-ordered `report.inventory.entries` are scanned once in order and
/// [`presslint_selectors::matches`] decides each entry; hits are pushed as
/// [`PdfInventoryMatch`] in ascending `entry_index` order.
///
/// Matched entries are never cloned into the result: `matches` holds only
/// `usize` + [`PageIndex`] pairs and the full report is moved into
/// [`PdfInventoryQuery::report`] once.
///
/// # Errors
///
/// Returns the same [`PdfInventoryError`] as [`build_pdf_inventory`] when the
/// neutral document/page-content path cannot be established. Unsupported page
/// and stream shapes remain structured page skips inside the returned report.
pub fn query_pdf_inventory(
    input: &[u8],
    selector: &Selector,
    max_decoded_stream_bytes: usize,
) -> Result<PdfInventoryQuery, PdfInventoryError> {
    let report = build_pdf_inventory(input, max_decoded_stream_bytes)?;

    let matches = report
        .inventory
        .entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| presslint_selectors::matches(selector, entry))
        .map(|(entry_index, entry)| PdfInventoryMatch {
            entry_index,
            page_index: entry.id.page,
        })
        .collect();

    Ok(PdfInventoryQuery { report, matches })
}
