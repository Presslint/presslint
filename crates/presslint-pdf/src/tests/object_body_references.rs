//! Object-body indirect-reference scanner coverage.
//!
//! The fail-safe direction is asserted from both sides: strings, comments, and
//! stream data never produce references (no false extraction from opaque
//! bytes), while nested values, duplicates, comment-separated references, and
//! reference-shaped scalar bodies are always captured (a missed reference
//! would later corrupt a consumer index). Serde round-trips pin the public
//! report, skip, truncation, and rejection shapes.

#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use crate::{
    IndirectObjectHeaderInspectionRejection, IndirectRef, MAX_OBJECT_BODY_REFERENCES,
    ObjectBodyReferencesInspection, ObjectBodyReferencesInspectionError,
    ObjectBodyReferencesInspectionRejection, ObjectBodyReferencesTruncation, ResolvedObject,
    ResolvedObjectData, SkippedObjectBodyReference, inspect_object_body_references,
    inspect_object_body_references_resolved, scan_indirect_references_in_span,
};

fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

fn scan_all(buffer: &[u8]) -> ObjectBodyReferencesInspection {
    scan_indirect_references_in_span(buffer, 0..buffer.len())
}

fn array_body_with_references(reference_count: usize) -> String {
    use std::fmt::Write as _;
    let mut body = String::from("[");
    for object_number in 1..=reference_count {
        write!(body, "{object_number} 0 R ").expect("writing to a String cannot fail");
    }
    body.push(']');
    body
}

#[test]
fn extracts_nested_dictionary_and_array_references_in_source_order() {
    let source = b"1 0 obj\n<< /A << /B [1 0 R << /C 2 0 R >>] >> >>\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(
        report.references,
        vec![indirect_ref(1, 0), indirect_ref(2, 0)]
    );
    assert!(report.skipped_references.is_empty());
    assert_eq!(report.truncation, None);
}

#[test]
fn array_body_preserves_duplicate_references() {
    let source = b"1 0 obj\n[3 0 R 3 0 R]\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(
        report.references,
        vec![indirect_ref(3, 0), indirect_ref(3, 0)]
    );
}

#[test]
fn strings_and_comments_never_produce_references() {
    let source = b"1 0 obj\n<< /S (1 0 R) /H <313020520A> % 5 0 R\n/N /X >>\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(report.references, Vec::new());
    assert!(report.skipped_references.is_empty());
}

#[test]
fn comment_between_reference_tokens_counts_as_whitespace() {
    // ISO 32000-1 7.2.4: a comment is equivalent to a single whitespace byte,
    // so it must not hide a real reference (a miss is the unsafe direction).
    let source = b"1 0 obj\n[6 0 % note\nR]\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(report.references, vec![indirect_ref(6, 0)]);
}

#[test]
fn stream_data_is_never_scanned() {
    let source = b"4 0 obj\n<< /Length 12 /Ref 6 0 R >>\nstream\nBT 7 0 R ET\nendstream\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(report.references, vec![indirect_ref(6, 0)]);
}

#[test]
fn keyword_boundary_rejects_robot_and_r0_tokens() {
    let robot = inspect_object_body_references(b"1 0 obj\n[1 0 Robot]\nendobj\n", 0)
        .expect("object should inspect");
    let r_zero = inspect_object_body_references(b"1 0 obj\n[1 0 R0]\nendobj\n", 0)
        .expect("object should inspect");

    assert_eq!(robot.references, Vec::new());
    assert_eq!(r_zero.references, Vec::new());
}

#[test]
fn signed_and_real_numbers_never_join_the_window() {
    let signed_plus = inspect_object_body_references(b"1 0 obj\n[+1 0 R]\nendobj\n", 0)
        .expect("object should inspect");
    let signed_minus = inspect_object_body_references(b"1 0 obj\n[-1 0 R]\nendobj\n", 0)
        .expect("object should inspect");
    let real = inspect_object_body_references(b"1 0 obj\n[1.5 0 R]\nendobj\n", 0)
        .expect("object should inspect");
    let unsigned = inspect_object_body_references(b"1 0 obj\n[1 0 R]\nendobj\n", 0)
        .expect("object should inspect");

    assert_eq!(signed_plus.references, Vec::new());
    assert_eq!(signed_minus.references, Vec::new());
    assert_eq!(real.references, Vec::new());
    assert_eq!(unsigned.references, vec![indirect_ref(1, 0)]);
}

#[test]
fn reference_shaped_scalar_body_reports_exactly_one_reference() {
    let source = b"5 0 obj\n2 0 R\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
    assert!(report.skipped_references.is_empty());
    assert_eq!(report.truncation, None);
}

#[test]
fn non_reference_scalar_bodies_yield_empty_reports() {
    for source in [
        &b"5 0 obj\n42\nendobj\n"[..],
        &b"5 0 obj\n3.14\nendobj\n"[..],
        &b"5 0 obj\n/Name\nendobj\n"[..],
        &b"5 0 obj\n(1 0 R)\nendobj\n"[..],
        &b"5 0 obj\n<31302052>\nendobj\n"[..],
        &b"5 0 obj\ntrue\nendobj\n"[..],
        &b"5 0 obj\nnull\nendobj\n"[..],
    ] {
        let report = inspect_object_body_references(source, 0).expect("object should inspect");
        assert_eq!(report.references, Vec::new());
        assert!(report.skipped_references.is_empty());
    }
}

#[test]
fn out_of_range_numbers_become_structured_skips_in_source_order() {
    let source = b"1 0 obj\n[4294967296 0 R 1 65536 R 2 0 R]\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
    assert_eq!(
        report.skipped_references,
        vec![
            SkippedObjectBodyReference::ObjectNumberOutOfRange,
            SkippedObjectBodyReference::GenerationOutOfRange,
        ]
    );
}

#[test]
fn out_of_range_scalar_body_becomes_a_structured_skip() {
    let source = b"9 0 obj\n4294967296 0 R\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    assert_eq!(report.references, Vec::new());
    assert_eq!(
        report.skipped_references,
        vec![SkippedObjectBodyReference::ObjectNumberOutOfRange]
    );
}

#[test]
fn resolved_uncompressed_delegates_to_the_offset_path() {
    let source = b"3 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let resolved = ResolvedObjectData::Uncompressed {
        resolved: ResolvedObject {
            reference: indirect_ref(3, 0),
            object_byte_offset: 0,
            xref_generation: 0,
        },
    };

    let report = inspect_object_body_references_resolved(source, &resolved)
        .expect("uncompressed object should inspect");

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
}

#[test]
fn resolved_compressed_member_scans_the_decoded_buffer_span() {
    let body: &[u8] = b"<< /Parent 4 0 R /Kids [7 0 R 8 0 R] >>";
    let mut decoded = b"9 0 R sibling-prefix ".to_vec();
    let start = decoded.len();
    decoded.extend_from_slice(body);
    let span = start..decoded.len();
    decoded.extend_from_slice(b" 11 0 R sibling-suffix");
    let resolved = ResolvedObjectData::Compressed {
        reference: indirect_ref(10, 0),
        object_stream_number: 5,
        index_within_object_stream: 1,
        decoded_object_stream: decoded,
        object_body_span: span,
    };

    let report = inspect_object_body_references_resolved(&[], &resolved)
        .expect("compressed member should scan");

    // Only the member's own body span is scanned, not sibling member bytes.
    assert_eq!(
        report.references,
        vec![indirect_ref(4, 0), indirect_ref(7, 0), indirect_ref(8, 0)]
    );
}

#[test]
fn resolved_compressed_reference_shaped_member_body_is_captured() {
    let body: &[u8] = b"2 0 R";
    let resolved = ResolvedObjectData::Compressed {
        reference: indirect_ref(10, 0),
        object_stream_number: 5,
        index_within_object_stream: 0,
        decoded_object_stream: body.to_vec(),
        object_body_span: 0..body.len(),
    };

    let report = inspect_object_body_references_resolved(&[], &resolved)
        .expect("compressed member should scan");

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
}

#[test]
fn span_scan_clamps_out_of_bounds_ranges() {
    let buffer = b"[1 0 R]";

    let report = scan_indirect_references_in_span(buffer, 0..buffer.len() + 100);

    assert_eq!(report.references, vec![indirect_ref(1, 0)]);
}

#[test]
fn span_scan_stops_at_an_unterminated_string() {
    // Everything after an unterminated `(` is string interior, never tokens.
    let report = scan_all(b"[2 0 R (unterminated 3 0 R");

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
}

#[test]
fn comment_reaching_the_span_end_never_leaks_suffix_references() {
    // The comment opens inside the span and its terminating newline lies past
    // `span.end`: the skipper must stop at the span end, so the suffix bytes
    // (sibling object-stream members in the compressed case) yield nothing.
    let in_span: &[u8] = b"[1 0 R] % trailing comment";
    let mut buffer = in_span.to_vec();
    buffer.extend_from_slice(b" still comment\n9 0 R");

    let report = scan_indirect_references_in_span(&buffer, 0..in_span.len());

    assert_eq!(report.references, vec![indirect_ref(1, 0)]);
}

#[test]
fn literal_string_closing_past_the_span_end_never_leaks_suffix_references() {
    // The `(` opens inside the span but the matching `)` lies past `span.end`:
    // within the span the string is unterminated, so the scan ends and the
    // suffix bytes are never read.
    let in_span: &[u8] = b"2 0 R (crosses the span end 3 0 R";
    let mut buffer = in_span.to_vec();
    buffer.extend_from_slice(b") 9 0 R");

    let report = scan_indirect_references_in_span(&buffer, 0..in_span.len());

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
}

#[test]
fn hex_string_closing_past_the_span_end_never_leaks_suffix_references() {
    // The `<` opens inside the span but the closing `>` lies past `span.end`:
    // within the span the hex string is unterminated, so the scan ends and
    // the suffix bytes are never read.
    let in_span: &[u8] = b"2 0 R <4142";
    let mut buffer = in_span.to_vec();
    buffer.extend_from_slice(b"43> 9 0 R");

    let report = scan_indirect_references_in_span(&buffer, 0..in_span.len());

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
}

#[test]
fn per_body_reference_cap_reports_structured_truncation() {
    let body = array_body_with_references(MAX_OBJECT_BODY_REFERENCES + 1);

    let report = scan_all(body.as_bytes());

    assert_eq!(report.references.len(), MAX_OBJECT_BODY_REFERENCES);
    assert_eq!(report.references[0], indirect_ref(1, 0));
    assert_eq!(
        report.truncation,
        Some(ObjectBodyReferencesTruncation::MaxReferences {
            max_references: MAX_OBJECT_BODY_REFERENCES,
        })
    );
}

#[test]
fn body_of_exactly_cap_references_is_not_truncated() {
    let body = array_body_with_references(MAX_OBJECT_BODY_REFERENCES);

    let report = scan_all(body.as_bytes());

    assert_eq!(report.references.len(), MAX_OBJECT_BODY_REFERENCES);
    assert_eq!(report.truncation, None);
}

#[test]
fn window_resets_on_names_delimiters_and_other_tokens() {
    // `/A 0 R`: a name is not an integer, so `R` sees a partial window.
    let name_gap = scan_all(b"<< /A 0 R >>");
    // `1 0 (x) R`: a string token resets the window.
    let string_gap = scan_all(b"[1 0 (x) R]");
    // `1 ] 0 R`: the `]` delimiter resets between the two integers.
    let delimiter_gap = scan_all(b"[[1] 0 R]");
    // `1 0 true R`: a keyword token resets the window.
    let keyword_gap = scan_all(b"[1 0 true R]");

    assert_eq!(name_gap.references, Vec::new());
    assert_eq!(string_gap.references, Vec::new());
    assert_eq!(delimiter_gap.references, Vec::new());
    assert_eq!(keyword_gap.references, Vec::new());
}

#[test]
fn trailing_integer_pair_before_reference_wins_the_window() {
    // The window keeps only the LAST two integers: `[1 2 3 0 R]` is the
    // integers 1 and 2 followed by the reference `3 0 R`.
    let report = scan_all(b"[1 2 3 0 R]");

    assert_eq!(report.references, vec![indirect_ref(3, 0)]);
}

#[test]
fn header_and_extent_failures_are_structured_rejections() {
    let no_header = inspect_object_body_references(b"xref\n0 1\n", 0)
        .expect_err("non-object bytes should reject");
    assert_eq!(
        no_header.reason,
        ObjectBodyReferencesInspectionRejection::Header {
            header_reason: IndirectObjectHeaderInspectionRejection::MalformedHeader,
        }
    );
    assert_eq!(no_header.byte_offset, 0);

    let unterminated_dictionary = inspect_object_body_references(b"1 0 obj\n<< /A 1 0 R", 0)
        .expect_err("unterminated dictionary should reject");
    assert!(matches!(
        unterminated_dictionary.reason,
        ObjectBodyReferencesInspectionRejection::DictionaryExtent { .. }
    ));

    let unterminated_array = inspect_object_body_references(b"1 0 obj\n[1 0 R", 0)
        .expect_err("unterminated array should reject");
    assert!(matches!(
        unterminated_array.reason,
        ObjectBodyReferencesInspectionRejection::ArrayExtent { .. }
    ));

    let empty_body =
        inspect_object_body_references(b"1 0 obj", 0).expect_err("missing body should reject");
    assert!(matches!(
        empty_body.reason,
        ObjectBodyReferencesInspectionRejection::BodyToken { .. }
    ));
}

#[test]
fn report_does_not_retain_source_bytes() {
    let source = b"1 0 obj\n<< /Ref 6 0 R /Secret (corpus-detail-not-copied) >>\nendobj\n";

    let report = inspect_object_body_references(source, 0).expect("object should inspect");

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("corpus-detail-not-copied"));
    assert!(!debug_report.contains("Secret"));
}

#[test]
fn serde_round_trips_report_skip_and_truncation_shapes() {
    for report in [
        ObjectBodyReferencesInspection {
            references: vec![indirect_ref(1, 0), indirect_ref(2, 3)],
            skipped_references: Vec::new(),
            truncation: None,
        },
        ObjectBodyReferencesInspection {
            references: Vec::new(),
            skipped_references: vec![
                SkippedObjectBodyReference::ObjectNumberOutOfRange,
                SkippedObjectBodyReference::GenerationOutOfRange,
            ],
            truncation: Some(ObjectBodyReferencesTruncation::MaxReferences {
                max_references: MAX_OBJECT_BODY_REFERENCES,
            }),
        },
    ] {
        let value = serde_value(&report).expect("report should serialize");
        let restored: ObjectBodyReferencesInspection =
            from_serde_value(value).expect("report should deserialize");
        assert_eq!(restored, report);
    }
}

#[test]
fn serde_round_trips_rejection_shapes() {
    let source = b"1 0 obj\n[1 0 R";
    let errors = [
        inspect_object_body_references(b"not-an-object", 0)
            .expect_err("header failure should reject"),
        inspect_object_body_references(source, 0).expect_err("array failure should reject"),
        inspect_object_body_references(b"1 0 obj\n<< /A 1", 0)
            .expect_err("dictionary failure should reject"),
        inspect_object_body_references(b"1 0 obj", 0).expect_err("body failure should reject"),
    ];

    for error in errors {
        let value = serde_value(&error).expect("error should serialize");
        let restored: ObjectBodyReferencesInspectionError =
            from_serde_value(value).expect("error should deserialize");
        assert_eq!(restored, error);
    }
}
