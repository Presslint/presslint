use crate::{
    DictionaryValueKind, IndirectRef, PageContentsValueShape, ResolvedObject, ResolvedObjectData,
    ResolvedPageContentsError, inspect_page_contents_resolved,
    page_contents_inspection_from_resolved,
};

fn iref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

/// Wrap a leaf `/Page` dictionary as an UNCOMPRESSED indirect object located at a
/// real source byte offset, returning both the source and the resolved handle.
fn uncompressed_leaf(dictionary: &[u8]) -> (Vec<u8>, ResolvedObjectData) {
    let mut source = b"%PDF-1.7\n".to_vec();
    let object_byte_offset = source.len();
    source.extend_from_slice(b"3 0 obj\n");
    source.extend_from_slice(dictionary);
    source.extend_from_slice(b"\nendobj\n");
    let resolved = ResolvedObjectData::Uncompressed {
        resolved: ResolvedObject {
            reference: iref(3, 0),
            object_byte_offset,
            xref_generation: 0,
        },
    };
    (source, resolved)
}

/// Wrap a leaf `/Page` dictionary as a type-2 COMPRESSED member body. The dict
/// lives only in the decoded `/ObjStm` buffer — it has no original-PDF source
/// offset — so `input` is deliberately unrelated to the member body.
fn compressed_leaf(dictionary: &[u8]) -> ResolvedObjectData {
    ResolvedObjectData::Compressed {
        reference: iref(3, 0),
        object_stream_number: 5,
        index_within_object_stream: 2,
        decoded_object_stream: dictionary.to_vec(),
        object_body_span: 0..dictionary.len(),
    }
}

#[test]
fn uncompressed_single_reference_reports_one_content_object() {
    let (source, resolved) = uncompressed_leaf(b"<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>");
    let report =
        inspect_page_contents_resolved(&source, &resolved).expect("single /Contents should read");
    assert_eq!(report.value_shape, PageContentsValueShape::SingleReference);
    assert_eq!(report.contents, vec![iref(8, 0)]);
    assert_eq!(report.skipped_non_reference_count, 0);
}

#[test]
fn compressed_single_reference_reports_one_content_object() {
    // The compressed leaf's dict lives only in the decoded member body; `input`
    // (here empty) is never consulted for a compressed leaf.
    let resolved = compressed_leaf(b"<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>");
    let report =
        inspect_page_contents_resolved(&[], &resolved).expect("compressed /Contents should read");
    assert_eq!(report.value_shape, PageContentsValueShape::SingleReference);
    assert_eq!(report.contents, vec![iref(8, 0)]);
    assert_eq!(report.skipped_non_reference_count, 0);
}

#[test]
fn compressed_array_reports_each_content_object_in_order() {
    let resolved = compressed_leaf(b"<< /Type /Page /Contents [ 8 0 R 9 0 R 10 0 R ] >>");
    let report =
        inspect_page_contents_resolved(&[], &resolved).expect("array /Contents should read");
    assert_eq!(report.value_shape, PageContentsValueShape::Array);
    assert_eq!(report.contents, vec![iref(8, 0), iref(9, 0), iref(10, 0)]);
    assert_eq!(report.skipped_non_reference_count, 0);
}

#[test]
fn compressed_array_counts_non_reference_entries_as_skips() {
    // A nested array, a literal string, and a name are honest non-reference skips;
    // the two real references are still reported in source order.
    let resolved =
        compressed_leaf(b"<< /Type /Page /Contents [ 8 0 R [ 1 2 ] (junk) /Name 9 0 R ] >>");
    let report = inspect_page_contents_resolved(&[], &resolved).expect("mixed array should read");
    assert_eq!(report.value_shape, PageContentsValueShape::Array);
    assert_eq!(report.contents, vec![iref(8, 0), iref(9, 0)]);
    assert_eq!(report.skipped_non_reference_count, 3);
}

#[test]
fn missing_contents_is_reported_honestly() {
    let resolved = compressed_leaf(b"<< /Type /Page /Parent 2 0 R >>");
    let error = inspect_page_contents_resolved(&[], &resolved)
        .expect_err("a leaf without /Contents should not inspect");
    assert_eq!(error, ResolvedPageContentsError::MissingContents);
}

#[test]
fn duplicate_contents_is_reported_honestly() {
    let resolved = compressed_leaf(b"<< /Type /Page /Contents 8 0 R /Contents 9 0 R >>");
    let error = inspect_page_contents_resolved(&[], &resolved)
        .expect_err("a leaf with duplicate /Contents should not inspect");
    assert_eq!(error, ResolvedPageContentsError::DuplicateContents);
}

#[test]
fn non_reference_non_array_value_is_reported_with_its_value_kind() {
    let resolved = compressed_leaf(b"<< /Type /Page /Contents /NotARef >>");
    let error = inspect_page_contents_resolved(&[], &resolved)
        .expect_err("a name-valued /Contents should not inspect");
    assert_eq!(
        error,
        ResolvedPageContentsError::NonReferenceOrArrayContentsValue {
            value_kind: DictionaryValueKind::Name,
        }
    );
}

#[test]
fn malformed_single_reference_is_reported_honestly() {
    // `8 0 obj` is shaped like a reference candidate but is not a complete `N G R`.
    let resolved = compressed_leaf(b"<< /Type /Page /Contents 8 0 obj >>");
    let error = inspect_page_contents_resolved(&[], &resolved)
        .expect_err("a malformed single /Contents reference should not inspect");
    assert!(matches!(
        error,
        ResolvedPageContentsError::MalformedContentsReference { .. }
    ));
}

#[test]
fn compressed_body_that_is_not_a_dictionary_surfaces_a_page_dictionary_error() {
    // A compressed member body that is not a `<<` dictionary cannot yield a leaf
    // dict, so the resolved reader reports a structured page-dictionary failure.
    let resolved = compressed_leaf(b"[ 1 2 3 ]");
    let error = inspect_page_contents_resolved(&[], &resolved)
        .expect_err("a non-dictionary body should not inspect");
    assert!(matches!(
        error,
        ResolvedPageContentsError::PageDictionary { .. }
    ));
}

#[test]
fn adapter_is_provenance_neutral_and_preserves_references() {
    // PROVENANCE: adapting resolved references into the shared PageContentsInspection
    // shape must never surface a buffer-relative span. Every span field is the zero
    // sentinel, yet the position-independent object references round-trip intact.
    let resolved = compressed_leaf(b"<< /Type /Page /Contents [ 8 0 R 9 0 R ] >>");
    let report = inspect_page_contents_resolved(&[], &resolved).expect("array should read");

    let inspection = page_contents_inspection_from_resolved(&report);
    assert_eq!(inspection.value_shape, PageContentsValueShape::Array);
    assert_eq!(inspection.contents_key_range.start, 0);
    assert_eq!(inspection.contents_key_range.end, 0);
    assert_eq!(inspection.contents_value_range.start, 0);
    assert_eq!(inspection.contents_value_range.end, 0);
    assert_eq!(inspection.page_dictionary.header_range.start, 0);
    assert_eq!(inspection.page_dictionary.header_range.end, 0);
    assert!(inspection.skipped.is_empty());

    let references: Vec<IndirectRef> = inspection
        .contents
        .iter()
        .map(|content| content.reference)
        .collect();
    assert_eq!(references, vec![iref(8, 0), iref(9, 0)]);
    // Each synthetic reference carries the zero sentinel span, NOT a member-body
    // offset masquerading as a source range.
    for content in &inspection.contents {
        assert_eq!(content.reference_range.start, 0);
        assert_eq!(content.reference_range.end, 0);
    }
}
