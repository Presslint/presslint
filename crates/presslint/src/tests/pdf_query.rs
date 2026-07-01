use std::fmt::Write as _;

use presslint_selectors::{PageMatcher, PageParity, Predicate, Selector};
use presslint_types::{ColorSpace, ColorUsage, PageIndex};

use crate::{PdfInventoryError, build_pdf_inventory, query_pdf_inventory};

/// Build a multi-page classic PDF where each page carries one raw content
/// stream from `contents`, reusing the shared `classic_pdf` fixture writer.
fn multi_page_pdf(contents: &[&[u8]]) -> Vec<u8> {
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec();

    let mut kids = String::new();
    for i in 0..contents.len() {
        let page_obj = 3 + 2 * i;
        let _ = write!(kids, "{page_obj} 0 R ");
    }
    let pages = format!(
        "2 0 obj\n<< /Type /Pages /Kids [ {kids}] /Count {} >>\nendobj\n",
        contents.len()
    )
    .into_bytes();

    let mut objects: Vec<Vec<u8>> = vec![catalog, pages];
    for (i, data) in contents.iter().enumerate() {
        let page_obj = 3 + 2 * i;
        let contents_obj = 4 + 2 * i;
        let page = format!(
            "{page_obj} 0 obj\n<< /Type /Page /Parent 2 0 R /Contents {contents_obj} 0 R >>\nendobj\n"
        )
        .into_bytes();

        let mut stream = Vec::new();
        stream.extend_from_slice(format!("{contents_obj} 0 obj\n<< /Length ").as_bytes());
        stream.extend_from_slice(data.len().to_string().as_bytes());
        stream.extend_from_slice(b" >>\nstream\n");
        stream.extend_from_slice(data);
        stream.extend_from_slice(b"\nendstream\nendobj\n");

        objects.push(page);
        objects.push(stream);
    }

    let refs: Vec<&[u8]> = objects.iter().map(Vec::as_slice).collect();
    super::classic_pdf(&refs)
}

/// Three single-vector pages: blue, red, blue. One inventory entry per page in
/// document order, so entry indices line up with page ordinals 0, 1, 2.
fn three_page_pdf() -> Vec<u8> {
    let blue: &[u8] = b"q\n0 0 1 rg\n12 12 80 80 re\nf\nQ";
    let red: &[u8] = b"q\n1 0 0 rg\n12 12 80 80 re\nf\nQ";
    multi_page_pdf(&[blue, red, blue])
}

#[test]
fn page_parity_selector_matches_odd_pages() -> Result<(), PdfInventoryError> {
    let source = three_page_pdf();
    let selector = Selector::Predicate {
        predicate: Predicate::PageMatch {
            matcher: PageMatcher::Parity {
                parity: PageParity::Odd,
            },
        },
    };

    let query = query_pdf_inventory(&source, &selector, 1024)?;

    // Three pages, one vector entry each, in document order.
    assert_eq!(query.report.inventory.len(), 3);
    // Odd one-based page numbers are indices 0 and 2.
    let hits: Vec<(usize, u32)> = query
        .matches
        .iter()
        .map(|m| (m.entry_index, m.page_index.0))
        .collect();
    assert_eq!(hits, vec![(0, 0), (2, 2)]);
    Ok(())
}

#[test]
fn color_component_selector_hits_vector_fill() -> Result<(), PdfInventoryError> {
    // Page 0 and page 2 are blue `0 0 1 rg`; page 1 is red.
    let source = three_page_pdf();
    let selector = Selector::Predicate {
        predicate: Predicate::ColorComponents {
            space: ColorSpace::DeviceRgb,
            usage: Some(ColorUsage::Fill),
            components: vec![0.0, 0.0, 1.0],
            tolerance: None,
        },
    };

    let query = query_pdf_inventory(&source, &selector, 1024)?;

    let hits: Vec<(usize, u32)> = query
        .matches
        .iter()
        .map(|m| (m.entry_index, m.page_index.0))
        .collect();
    assert_eq!(hits, vec![(0, 0), (2, 2)]);
    Ok(())
}

#[test]
fn selector_all_matches_every_entry_in_scan_order() -> Result<(), PdfInventoryError> {
    let source = three_page_pdf();

    let query = query_pdf_inventory(&source, &Selector::All, 1024)?;

    let entry_count = query.report.inventory.len();
    assert_eq!(entry_count, 3);
    assert_eq!(query.matches.len(), entry_count);
    for (position, hit) in query.matches.iter().enumerate() {
        // `Selector::All` matches every entry, so each match's index is exactly
        // the scan position and its page ordinal is the entry's own page.
        assert_eq!(hit.entry_index, position);
        assert_eq!(
            hit.page_index,
            query.report.inventory.entries[position].id.page
        );
    }
    Ok(())
}

#[test]
fn selector_none_matches_nothing_over_non_empty_report() -> Result<(), PdfInventoryError> {
    let source = three_page_pdf();

    let query = query_pdf_inventory(&source, &Selector::None, 1024)?;

    assert!(query.matches.is_empty());
    // The report is still fully built even when nothing is selected.
    assert_eq!(query.report.inventory.len(), 3);
    Ok(())
}

#[test]
fn top_level_error_propagates_from_build() -> Result<(), String> {
    let input = b"not a pdf at all";

    let Err(build_err) = build_pdf_inventory(input, 1024) else {
        return Err("malformed input should fail the build path".to_string());
    };
    let Err(query_err) = query_pdf_inventory(input, &Selector::All, 1024) else {
        return Err("malformed input should fail the query path".to_string());
    };
    // The query is a strict superset of the build, so the top-level failure is
    // the same `PdfInventoryError` unchanged.
    assert_eq!(query_err, build_err);
    Ok(())
}

#[test]
fn single_page_parity_and_range_agree() -> Result<(), PdfInventoryError> {
    // A single blue page: index 0, one-based page 1 (odd).
    let source = super::single_page_pdf(b"", super::vector_content());

    let even = query_pdf_inventory(
        &source,
        &Selector::Predicate {
            predicate: Predicate::PageMatch {
                matcher: PageMatcher::Parity {
                    parity: PageParity::Even,
                },
            },
        },
        1024,
    )?;
    assert!(even.matches.is_empty());

    let range = query_pdf_inventory(
        &source,
        &Selector::Predicate {
            predicate: Predicate::PageMatch {
                matcher: PageMatcher::Range {
                    start: PageIndex(0),
                    end: PageIndex(0),
                },
            },
        },
        1024,
    )?;
    assert_eq!(range.matches.len(), 1);
    assert_eq!(range.matches[0].entry_index, 0);
    assert_eq!(range.matches[0].page_index, PageIndex(0));
    Ok(())
}
