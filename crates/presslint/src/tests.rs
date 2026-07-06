use miniz_oxide::deflate::compress_to_vec_zlib;

use crate::{
    ClassicPdfInventoryError, ClassicPdfInventoryPageResult, ClassicPdfInventorySkip, ObjectKind,
    build_classic_pdf_inventory,
};

mod color_audit;
mod extgstate_wiring;
mod form_inventory;
mod form_inventory_golden;
mod pdf_inventory;
mod pdf_query;
mod preflight;

fn single_page_pdf(content_dict_suffix: &[u8], content_data: &[u8]) -> Vec<u8> {
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n";

    let mut content = Vec::new();
    content.extend_from_slice(b"4 0 obj\n<< /Length ");
    content.extend_from_slice(content_data.len().to_string().as_bytes());
    content.extend_from_slice(content_dict_suffix);
    content.extend_from_slice(b" >>\nstream\n");
    content.extend_from_slice(content_data);
    content.extend_from_slice(b"\nendstream\nendobj\n");

    classic_pdf(&[catalog, pages, page, &content])
}

fn multi_stream_page_pdf() -> Vec<u8> {
    multi_stream_page_pdf_with_streams(b"", b"q\n0 0 1 rg\n12 12 80 80 re\n", b"", b"f\nQ")
}

fn multi_stream_page_pdf_with_streams(
    first_dict_suffix: &[u8],
    first_data: &[u8],
    second_dict_suffix: &[u8],
    second_data: &[u8],
) -> Vec<u8> {
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [ 4 0 R 5 0 R ] >>\nendobj\n";

    let mut first = Vec::new();
    first.extend_from_slice(b"4 0 obj\n<< /Length ");
    first.extend_from_slice(first_data.len().to_string().as_bytes());
    first.extend_from_slice(first_dict_suffix);
    first.extend_from_slice(b" >>\nstream\n");
    first.extend_from_slice(first_data);
    first.extend_from_slice(b"\nendstream\nendobj\n");

    let mut second = Vec::new();
    second.extend_from_slice(b"5 0 obj\n<< /Length ");
    second.extend_from_slice(second_data.len().to_string().as_bytes());
    second.extend_from_slice(second_dict_suffix);
    second.extend_from_slice(b" >>\nstream\n");
    second.extend_from_slice(second_data);
    second.extend_from_slice(b"\nendstream\nendobj\n");

    classic_pdf(&[catalog, pages, page, &first, &second])
}

fn classic_pdf(objects: &[&[u8]]) -> Vec<u8> {
    let mut source = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for object in objects {
        offsets.push(source.len());
        source.extend_from_slice(object);
    }

    let xref_offset = source.len();
    let object_count = objects.len() + 1;
    source.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
    source.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    source.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );
    source
}

fn vector_content() -> &'static [u8] {
    b"q\n0 0 1 rg\n12 12 80 80 re\nf\nQ"
}

#[test]
fn builds_inventory_from_raw_single_stream_page() -> Result<(), ClassicPdfInventoryError> {
    let source = single_page_pdf(b"", vector_content());

    let report = build_classic_pdf_inventory(&source, 1024)?;

    assert_eq!(report.byte_len, source.len());
    assert_eq!(report.pages.len(), 1);
    assert_eq!(report.pages[0].page_index.0, 0);
    assert_eq!(
        report.pages[0].result,
        ClassicPdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    assert_eq!(report.inventory.entries[0].id.page.0, 0);
    Ok(())
}

#[test]
fn builds_inventory_from_flate_single_stream_page() -> Result<(), ClassicPdfInventoryError> {
    let compressed = compress_to_vec_zlib(vector_content(), 6);
    let source = single_page_pdf(b" /Filter /FlateDecode", &compressed);

    let report = build_classic_pdf_inventory(&source, 1024)?;

    assert_eq!(
        report.pages[0].result,
        ClassicPdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn skips_unsupported_filter() -> Result<(), ClassicPdfInventoryError> {
    let source = single_page_pdf(b" /Filter /ASCIIHexDecode", vector_content());

    let report = build_classic_pdf_inventory(&source, 1024)?;

    assert!(matches!(
        report.pages[0].result,
        ClassicPdfInventoryPageResult::Skipped {
            reason: ClassicPdfInventorySkip::UnsupportedFilter { .. }
        }
    ));
    assert!(report.inventory.is_empty());
    Ok(())
}

#[test]
fn skips_malformed_content_stream() -> Result<(), ClassicPdfInventoryError> {
    let source = single_page_pdf(b"", b"(unterminated literal string");

    let report = build_classic_pdf_inventory(&source, 1024)?;

    assert!(matches!(
        report.pages[0].result,
        ClassicPdfInventoryPageResult::Skipped {
            reason: ClassicPdfInventorySkip::TokenizeFailed { .. }
        }
    ));
    assert!(report.inventory.is_empty());
    Ok(())
}

#[test]
fn builds_inventory_from_raw_multi_stream_page() -> Result<(), ClassicPdfInventoryError> {
    let source = multi_stream_page_pdf();

    let report = build_classic_pdf_inventory(&source, 1024)?;

    assert_eq!(report.pages.len(), 1);
    assert_eq!(
        report.pages[0].result,
        ClassicPdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    assert_eq!(report.inventory.len(), 1);
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Vector);
    Ok(())
}
