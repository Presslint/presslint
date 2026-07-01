use miniz_oxide::deflate::compress_to_vec_zlib;

use super::super::{multi_stream_page_pdf_with_streams, single_page_pdf, vector_content};
use crate::{
    ClassicPdfInventoryPageResult, ClassicPdfInventorySkip, ObjectKind, PdfInventoryPageResult,
    PdfInventorySkip, build_classic_pdf_inventory, build_pdf_inventory,
};

#[test]
fn adjacent_streams_are_separated_before_tokenization() -> Result<(), String> {
    let source =
        multi_stream_page_pdf_with_streams(b"", b"q\n(Hello) T", b"", b"j\n12 12 80 80 re\nf\nQ");

    let report = build_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        report.pages[0].result,
        PdfInventoryPageResult::Inventoried {
            entry_count: 1,
            form_skipped: Vec::new()
        }
    );
    let entry = report
        .inventory
        .entries
        .first()
        .ok_or("missing vector entry")?;
    assert_eq!(entry.kind, ObjectKind::Vector);
    Ok(())
}

#[test]
fn total_joined_size_limit_is_enforced_for_multi_stream_pages() -> Result<(), String> {
    let first = b"q\n";
    let second = b"0 0 1 rg\n12 12 80 80 re\nf\nQ";
    let source = multi_stream_page_pdf_with_streams(
        b" /Filter /FlateDecode",
        &compress_to_vec_zlib(first, 6),
        b" /Filter /FlateDecode",
        &compress_to_vec_zlib(second, 6),
    );
    let max_joined = first.len() + 1 + second.len() - 1;

    let report = build_pdf_inventory(&source, max_joined).map_err(|error| format!("{error:?}"))?;

    assert!(matches!(
        report.pages[0].result,
        PdfInventoryPageResult::Skipped {
            reason: PdfInventorySkip::DecodeFailed { .. }
        }
    ));
    assert!(report.inventory.is_empty());
    Ok(())
}

#[test]
fn multi_stream_page_with_unsupported_stream_is_skipped() -> Result<(), String> {
    let source = multi_stream_page_pdf_with_streams(
        b"",
        b"q\n0 0 1 rg\n",
        b" /Filter /ASCIIHexDecode",
        b"12 12 80 80 re\nf\nQ",
    );

    let report =
        build_classic_pdf_inventory(&source, 1024).map_err(|error| format!("{error:?}"))?;

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
fn single_raw_stream_page_content_stays_borrowed() -> Result<(), String> {
    let source = single_page_pdf(b"", vector_content());
    let access = crate::pdf::inspect_classic_document_access(&source)
        .map_err(|error| format!("{error:?}"))?;
    let extents = crate::pdf::inspect_document_page_content_extents(
        &source,
        &access.xref_table,
        access.page_tree_root.object_byte_offset,
    )
    .map_err(|error| format!("{error:?}"))?;

    let page = &extents.pages[0];
    let crate::pdf::DocumentPageContentExtentResult::Inspected { extents, .. } = &page.result
    else {
        return Err("single-page fixture should inspect contents".to_string());
    };
    let content = crate::page_content::page_content_bytes(&source, &extents.entries, 1024)
        .map_err(|error| format!("{error:?}"))?;

    assert!(matches!(
        content,
        crate::page_content::PageContentBytes::Borrowed(_)
    ));
    Ok(())
}
