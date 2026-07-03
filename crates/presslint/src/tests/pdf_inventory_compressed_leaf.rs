//! End-to-end inventory/audit over COMPRESSED leaf `/Page` objects whose
//! `/Contents` is read from the resolved member body (T134).
//!
//! These cases exercise the resolved-body content path: a compressed leaf whose
//! `/Contents` points at an uncompressed content stream now produces REAL colour
//! inventory, while a compressed leaf whose `/Contents` target is itself
//! compressed stays an honest per-page skip.

use super::{object_stream, xref_record};
use crate::{
    ColorAuditStatus, CoverageGapKind, ObjectKind, PdfInventoryPageResult, PdfInventorySkip,
    audit_color_usage, build_pdf_inventory,
};

/// RGB vector content, mirroring the shared `vector_content` fixture.
const VECTOR_CONTENT: &[u8] = b"q\n0 0 1 rg\n12 12 80 80 re\nf\nQ";

fn content_object(number: u32, data: &[u8]) -> Vec<u8> {
    let mut object = format!("{number} 0 obj\n<< /Length {} >>\nstream\n", data.len()).into_bytes();
    object.extend_from_slice(data);
    object.extend_from_slice(b"\nendstream\nendobj\n");
    object
}

/// A cross-reference-stream PDF whose catalog (`1`), root `/Pages` (`2`), and both
/// leaf `/Page` objects (`3`, `4`) are type-2 compressed members of `/ObjStm` `5`.
/// Both leaves reference UNCOMPRESSED content objects (`6`, `7`) carrying real RGB
/// vector content, so the resolved bridge inventories both compressed leaves.
fn compressed_leaves_with_uncompressed_content_pdf() -> Result<Vec<u8>, String> {
    let catalog: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
    let pages: &[u8] = b"<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>";
    let leaf3: &[u8] = b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>";
    let leaf4: &[u8] = b"<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>";
    let objstm = object_stream(5, &[(1, catalog), (2, pages), (3, leaf3), (4, leaf4)]);

    let mut source = b"%PDF-1.5\n".to_vec();
    let objstm_offset = source.len();
    source.extend_from_slice(&objstm);
    let content6_offset = source.len();
    source.extend_from_slice(&content_object(6, VECTOR_CONTENT));
    let content7_offset = source.len();
    source.extend_from_slice(&content_object(7, VECTOR_CONTENT));
    let xref_offset = source.len();
    let size = 9;

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0)?); // 0 free
    for index in 0..4u8 {
        records.extend_from_slice(&xref_record(2, 5, index)?); // 1..=4 compressed
    }
    records.extend_from_slice(&xref_record(1, objstm_offset, 0)?); // 5 objstm
    records.extend_from_slice(&xref_record(1, content6_offset, 0)?); // 6 content
    records.extend_from_slice(&xref_record(1, content7_offset, 0)?); // 7 content
    records.extend_from_slice(&xref_record(1, xref_offset, 0)?); // 8 xref stream

    source.extend_from_slice(
        format!(
            "8 0 obj\n<< /Type /XRef /Size {size} /W [ 1 2 1 ] /Index [ 0 {size} ] /Root 1 0 R /Length {} >>\nstream\n",
            records.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&records);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    Ok(source)
}

/// A cross-reference-stream PDF with a single compressed leaf `/Page` (`3`) whose
/// `/Contents 6 0 R` target is ITSELF a compressed member of `/ObjStm` `5`, so the
/// content target cannot be located and the page stays an honest skip.
fn compressed_leaf_with_compressed_content_target_pdf() -> Result<Vec<u8>, String> {
    let catalog: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
    let pages: &[u8] = b"<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>";
    let leaf3: &[u8] = b"<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>";
    // Object `6` is a compressed member: its xref entry is type-2, so the content
    // target resolves to a compressed location and is skipped.
    let compressed_content: &[u8] = b"<< /Note /compressed-content-target >>";
    let objstm = object_stream(
        5,
        &[
            (1, catalog),
            (2, pages),
            (3, leaf3),
            (6, compressed_content),
        ],
    );

    let mut source = b"%PDF-1.5\n".to_vec();
    let objstm_offset = source.len();
    source.extend_from_slice(&objstm);
    let xref_offset = source.len();
    let size = 7;

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0)?); // 0 free
    records.extend_from_slice(&xref_record(2, 5, 0)?); // 1 catalog
    records.extend_from_slice(&xref_record(2, 5, 1)?); // 2 pages
    records.extend_from_slice(&xref_record(2, 5, 2)?); // 3 leaf3
    records.extend_from_slice(&xref_record(1, xref_offset, 0)?); // 4 xref stream
    records.extend_from_slice(&xref_record(1, objstm_offset, 0)?); // 5 objstm
    records.extend_from_slice(&xref_record(2, 5, 3)?); // 6 compressed content target

    source.extend_from_slice(
        format!(
            "4 0 obj\n<< /Type /XRef /Size {size} /W [ 1 2 1 ] /Index [ 0 {size} ] /Root 1 0 R /Length {} >>\nstream\n",
            records.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&records);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    Ok(source)
}

#[test]
fn compressed_leaves_with_uncompressed_content_inventory_real_colour() -> Result<(), String> {
    let source = compressed_leaves_with_uncompressed_content_pdf()?;

    // Before T134 both compressed leaves were blanket `CompressedLeaf` skips with
    // zero colour; now each leaf's `/Contents` is read from its resolved body and
    // its uncompressed content stream is inventoried for real.
    let report = build_pdf_inventory(&source, 4096).map_err(|error| format!("{error:?}"))?;

    assert_eq!(report.pages.len(), 2);
    for page in &report.pages {
        assert_eq!(
            page.result,
            PdfInventoryPageResult::Inventoried {
                entry_count: 1,
                form_skipped: Vec::new()
            }
        );
    }
    assert_eq!(report.inventory.len(), 2);
    assert!(
        report
            .inventory
            .entries
            .iter()
            .all(|entry| entry.kind == ObjectKind::Vector)
    );
    Ok(())
}

#[test]
fn audit_over_inventoried_compressed_leaves_reports_real_colour() -> Result<(), String> {
    let source = compressed_leaves_with_uncompressed_content_pdf()?;

    let audit = audit_color_usage(&source, 4096).map_err(|error| format!("{error:?}"))?;

    assert_eq!(audit.pages.len(), 2);
    // The compressed leaves now carry REAL colour observations, so their pages are
    // no longer `SkippedPage` content gaps.
    let skipped_page_gaps = audit
        .coverage_gaps
        .iter()
        .filter(|gap| gap.kind == CoverageGapKind::SkippedPage)
        .count();
    assert_eq!(skipped_page_gaps, 0);
    // Real DeviceRGB fill colour is observed on both pages.
    assert!(!audit.rgb_findings.is_empty());
    assert!(!audit.document.color_space_counts.is_empty());
    for page in &audit.pages {
        assert!(!page.summary.color_space_counts.is_empty());
    }
    Ok(())
}

#[test]
fn compressed_leaf_with_compressed_content_target_is_skipped() -> Result<(), String> {
    let source = compressed_leaf_with_compressed_content_target_pdf()?;

    let report = build_pdf_inventory(&source, 4096).map_err(|error| format!("{error:?}"))?;

    // The leaf is reached and its `/Contents` read, but the content target is itself
    // compressed, so the page is an honest `TargetSkipped` skip with zero colour.
    assert_eq!(report.pages.len(), 1);
    assert!(matches!(
        report.pages[0].result,
        PdfInventoryPageResult::Skipped {
            reason: PdfInventorySkip::TargetSkipped { .. }
        }
    ));
    assert_eq!(report.inventory.len(), 0);

    // The audit surfaces this as a coverage gap rather than real colour.
    let audit = audit_color_usage(&source, 4096).map_err(|error| format!("{error:?}"))?;
    assert_eq!(audit.status, ColorAuditStatus::Incomplete);
    assert!(audit.rgb_findings.is_empty());
    Ok(())
}
