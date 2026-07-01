use presslint_pdf::{IndirectRef, PageBoxKind, PageRectangle, inspect_document_page_boxes};

use crate::{
    DictionaryEntryWrite, PageBoxEdit, SetPageBoxSkipReason, SetPageBoxesError,
    SetPageBoxesRequest, set_page_boxes_incremental,
};

/// Build a valid single-revision classic-xref PDF from object bodies 1..=n. The
/// trailer names `/Root 1 0 R`, so object 1 is the catalog by convention.
fn classic_pdf(bodies: &[&[u8]]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");

    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(buf.len());
        let number = index + 1;
        buf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }

    let size = bodies.len() + 1;
    let xref_offset = buf.len();
    buf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF")
            .as_bytes(),
    );
    buf
}

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";

fn rect(llx: f64, lly: f64, urx: f64, ury: f64) -> PageRectangle {
    PageRectangle { llx, lly, urx, ury }
}

fn media_only(page_index: usize, rectangle: PageRectangle) -> SetPageBoxesRequest {
    SetPageBoxesRequest {
        pages: vec![PageBoxEdit {
            page_index,
            media_box: Some(rectangle),
            crop_box: None,
        }],
    }
}

/// Effective `/MediaBox` for a reopened page.
fn media_of(bytes: &[u8], page_index: usize) -> PageRectangle {
    inspect_document_page_boxes(bytes)
        .expect("page boxes reinspect")
        .pages
        .iter()
        .find(|page| page.page_index == page_index)
        .expect("page present")
        .media_box
        .effective
}

/// Effective `/CropBox` for a reopened page.
fn crop_of(bytes: &[u8], page_index: usize) -> PageRectangle {
    inspect_document_page_boxes(bytes)
        .expect("page boxes reinspect")
        .pages
        .iter()
        .find(|page| page.page_index == page_index)
        .expect("page present")
        .crop_box
        .effective
}

#[test]
fn replaces_direct_media_box_and_preserves_prefix() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 200.0, 400.0)))
        .expect("edit");

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.skipped, Vec::new());
    assert_eq!(output.edited.len(), 1);
    let edited = output.edited[0];
    assert_eq!(edited.page_index, 0);
    assert_eq!(
        edited.leaf_reference,
        IndirectRef {
            object_number: 3,
            generation: 0,
        }
    );
    let applied = edited.media_box.expect("media applied");
    assert_eq!(applied.kind, PageBoxKind::MediaBox);
    assert_eq!(applied.op, DictionaryEntryWrite::Replace);

    assert_eq!(media_of(&output.bytes, 0), rect(0.0, 0.0, 200.0, 400.0));
}

#[test]
fn inherited_media_box_becomes_direct_leaf_entry() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
        b"<< /Type /Page /Parent 2 0 R >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 300.0, 300.0)))
        .expect("edit");

    let applied = output.edited[0].media_box.expect("media applied");
    assert_eq!(applied.op, DictionaryEntryWrite::Insert);

    // The inherited ancestor /Pages dictionary body is not rewritten: the only
    // appended object header is the leaf (object 3).
    let appended = &output.bytes[input.len()..];
    assert!(contains(appended, b"3 0 obj"));
    assert!(!contains(appended, b"2 0 obj"));

    assert_eq!(media_of(&output.bytes, 0), rect(0.0, 0.0, 300.0, 300.0));
}

#[test]
fn replace_media_and_insert_crop_together() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
    ]);

    let request = SetPageBoxesRequest {
        pages: vec![PageBoxEdit {
            page_index: 0,
            media_box: Some(rect(0.0, 0.0, 500.0, 500.0)),
            crop_box: Some(rect(10.0, 10.0, 400.0, 400.0)),
        }],
    };
    let output = set_page_boxes_incremental(&input, &request).expect("edit");

    let edited = output.edited[0];
    assert_eq!(
        edited.media_box.expect("media").op,
        DictionaryEntryWrite::Replace
    );
    assert_eq!(
        edited.crop_box.expect("crop").op,
        DictionaryEntryWrite::Insert
    );

    assert_eq!(media_of(&output.bytes, 0), rect(0.0, 0.0, 500.0, 500.0));
    assert_eq!(crop_of(&output.bytes, 0), rect(10.0, 10.0, 400.0, 400.0));
}

#[test]
fn both_inserts_order_media_before_crop() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 100 100] >>",
        b"<< /Type /Page /Parent 2 0 R >>",
    ]);

    let request = SetPageBoxesRequest {
        pages: vec![PageBoxEdit {
            page_index: 0,
            media_box: Some(rect(0.0, 0.0, 300.0, 300.0)),
            crop_box: Some(rect(0.0, 0.0, 150.0, 150.0)),
        }],
    };
    let output = set_page_boxes_incremental(&input, &request).expect("edit");

    // Both are inserts after `<<`; the serialized order is /MediaBox then /CropBox.
    let appended = &output.bytes[input.len()..];
    let media_at = find(appended, b"/MediaBox").expect("media present");
    let crop_at = find(appended, b"/CropBox").expect("crop present");
    assert!(media_at < crop_at);

    assert_eq!(media_of(&output.bytes, 0), rect(0.0, 0.0, 300.0, 300.0));
    assert_eq!(crop_of(&output.bytes, 0), rect(0.0, 0.0, 150.0, 150.0));
}

#[test]
fn edits_only_selected_pages() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 111.0, 111.0)))
        .expect("edit");

    // Only the selected leaf (object 3) is appended; object 4 is never rewritten.
    let appended = &output.bytes[input.len()..];
    assert!(contains(appended, b"3 0 obj"));
    assert!(!contains(appended, b"4 0 obj"));

    assert_eq!(media_of(&output.bytes, 0), rect(0.0, 0.0, 111.0, 111.0));
    // The unselected page keeps its original media box.
    assert_eq!(media_of(&output.bytes, 1), rect(0.0, 0.0, 200.0, 200.0));
}

#[test]
fn edited_reports_are_in_document_order_for_reversed_request() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] >>",
    ]);

    // Request the higher page index first; the reports must still come back in
    // ascending document-page order.
    let request = SetPageBoxesRequest {
        pages: vec![
            PageBoxEdit {
                page_index: 1,
                media_box: Some(rect(0.0, 0.0, 222.0, 222.0)),
                crop_box: None,
            },
            PageBoxEdit {
                page_index: 0,
                media_box: Some(rect(0.0, 0.0, 111.0, 111.0)),
                crop_box: None,
            },
        ],
    };
    let output = set_page_boxes_incremental(&input, &request).expect("edit");

    assert_eq!(output.skipped, Vec::new());
    let indexes: Vec<usize> = output.edited.iter().map(|page| page.page_index).collect();
    assert_eq!(indexes, vec![0, 1]);
    assert_eq!(
        output.edited[0].leaf_reference,
        IndirectRef {
            object_number: 3,
            generation: 0,
        }
    );
    assert_eq!(
        output.edited[1].leaf_reference,
        IndirectRef {
            object_number: 4,
            generation: 0,
        }
    );

    // Both selected pages carry their requested media box after reopening.
    assert_eq!(media_of(&output.bytes, 0), rect(0.0, 0.0, 111.0, 111.0));
    assert_eq!(media_of(&output.bytes, 1), rect(0.0, 0.0, 222.0, 222.0));
}

#[test]
fn skips_shared_leaf_referenced_by_two_kids() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R 3 0 R] /Count 2 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 300.0, 300.0)))
        .expect("plan");

    assert!(output.edited.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert!(matches!(
        output.skipped[0].reason,
        SetPageBoxSkipReason::OwnershipNotProven { occurrences: 2, .. }
    ));
    // No leaf object was appended.
    assert!(!contains(&output.bytes[input.len()..], b"3 0 obj"));
}

#[test]
fn skips_leaf_without_provable_parent() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /MediaBox [0 0 100 100] >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 300.0, 300.0)))
        .expect("plan");

    assert!(output.edited.is_empty());
    assert!(matches!(
        output.skipped[0].reason,
        SetPageBoxSkipReason::OwnershipNotProven { occurrences: 1, .. }
    ));
}

#[test]
fn skips_indirect_box_value() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox 9 0 R >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 300.0, 300.0)))
        .expect("plan");

    assert!(output.edited.is_empty());
    assert!(matches!(
        output.skipped[0].reason,
        SetPageBoxSkipReason::UnsupportedBoxValue {
            kind: PageBoxKind::MediaBox,
            ..
        }
    ));
}

#[test]
fn skips_malformed_box_value() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612] >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 300.0, 300.0)))
        .expect("plan");

    assert!(output.edited.is_empty());
    assert!(matches!(
        output.skipped[0].reason,
        SetPageBoxSkipReason::MalformedBoxValue {
            kind: PageBoxKind::MediaBox
        }
    ));
}

#[test]
fn skips_duplicate_box_key() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /MediaBox [0 0 2 2] >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 300.0, 300.0)))
        .expect("plan");

    assert!(output.edited.is_empty());
    assert!(matches!(
        output.skipped[0].reason,
        SetPageBoxSkipReason::DuplicateBoxKey {
            kind: PageBoxKind::MediaBox
        }
    ));
}

#[test]
fn skips_missing_page_index() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
    ]);

    let output = set_page_boxes_incremental(&input, &media_only(5, rect(0.0, 0.0, 300.0, 300.0)))
        .expect("plan");

    assert!(output.edited.is_empty());
    assert_eq!(output.skipped[0].page_index, 5);
    assert!(matches!(
        output.skipped[0].reason,
        SetPageBoxSkipReason::PageNotFound
    ));
}

#[test]
fn rejects_crop_outside_media() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
    ]);

    let request = SetPageBoxesRequest {
        pages: vec![PageBoxEdit {
            page_index: 0,
            media_box: None,
            crop_box: Some(rect(0.0, 0.0, 200.0, 200.0)),
        }],
    };
    let error = set_page_boxes_incremental(&input, &request).unwrap_err();
    assert!(matches!(
        error,
        SetPageBoxesError::CropOutsideMedia { page_index: 0, .. }
    ));
}

#[test]
fn rejects_zero_area_and_non_finite_and_bad_request() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
    ]);

    let zero_area =
        set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, 0.0, 100.0))).unwrap_err();
    assert!(matches!(
        zero_area,
        SetPageBoxesError::ZeroAreaRectangle { page_index: 0, .. }
    ));

    let non_finite =
        set_page_boxes_incremental(&input, &media_only(0, rect(0.0, 0.0, f64::NAN, 100.0)))
            .unwrap_err();
    assert!(matches!(
        non_finite,
        SetPageBoxesError::NonFiniteRectangle { page_index: 0, .. }
    ));

    let empty = SetPageBoxesRequest {
        pages: vec![PageBoxEdit {
            page_index: 0,
            media_box: None,
            crop_box: None,
        }],
    };
    assert!(matches!(
        set_page_boxes_incremental(&input, &empty).unwrap_err(),
        SetPageBoxesError::NoBoxesRequested { page_index: 0 }
    ));

    let duplicate = SetPageBoxesRequest {
        pages: vec![
            media_only(0, rect(0.0, 0.0, 10.0, 10.0)).pages[0],
            media_only(0, rect(0.0, 0.0, 20.0, 20.0)).pages[0],
        ],
    };
    assert!(matches!(
        set_page_boxes_incremental(&input, &duplicate).unwrap_err(),
        SetPageBoxesError::DuplicatePageIndex { page_index: 0 }
    ));
}

#[test]
fn normalizes_flipped_rectangle() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>",
    ]);

    // Upper-right and lower-left supplied in swapped order.
    let output = set_page_boxes_incremental(&input, &media_only(0, rect(400.0, 300.0, 0.0, 0.0)))
        .expect("edit");

    assert_eq!(media_of(&output.bytes, 0), rect(0.0, 0.0, 400.0, 300.0));
}

#[test]
fn second_append_is_idempotent() {
    let input = classic_pdf(&[
        CATALOG,
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
    ]);

    let requested = rect(0.0, 0.0, 200.0, 400.0);
    let first = set_page_boxes_incremental(&input, &media_only(0, requested)).expect("first");
    let second =
        set_page_boxes_incremental(&first.bytes, &media_only(0, requested)).expect("second");

    // The second output keeps the first as a verbatim prefix and reports a
    // second in-place replace (the leaf now carries a direct /MediaBox).
    assert_eq!(&second.bytes[..first.bytes.len()], first.bytes.as_slice());
    assert_eq!(
        second.edited[0].media_box.expect("media").op,
        DictionaryEntryWrite::Replace
    );
    assert_eq!(media_of(&second.bytes, 0), requested);
}

/// True when `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

/// First offset of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
