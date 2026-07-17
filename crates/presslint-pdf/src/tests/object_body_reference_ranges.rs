//! Range-bearing object-body reference scanner coverage.
//!
//! The load-bearing property is CENSUS PARITY: for the same buffer, span, or
//! object offset, the range-bearing sibling must report exactly the same
//! references, skips, and truncation as the census scanner — both run one
//! shared scanner core, and these tests assert the equivalence from the
//! outside over every body shape family. Range exactness is asserted by
//! slicing the scanned buffer with the reported ranges: each range must cover
//! exactly one token, so a splice of only the two numeric ranges preserves
//! the `R` keyword and every trivia byte (including interior comments).

#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use crate::{
    IndirectRef, MAX_OBJECT_BODY_REFERENCES, ObjectBodyReferenceRangesInspection,
    ObjectBodyReferenceTokenRanges, ObjectBodyReferencesTruncation, SkippedObjectBodyReference,
    inspect_object_body_reference_ranges, inspect_object_body_references,
    scan_indirect_reference_ranges_in_span, scan_indirect_references_in_span,
};

fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

/// Assert the two span scanners are token-identical over the whole buffer and
/// return the range-bearing report.
fn scan_all_parity(buffer: &[u8]) -> ObjectBodyReferenceRangesInspection {
    let census = scan_indirect_references_in_span(buffer, 0..buffer.len());
    let ranges = scan_indirect_reference_ranges_in_span(buffer, 0..buffer.len());
    assert_eq!(ranges.references, census.references);
    assert_eq!(ranges.skipped_references, census.skipped_references);
    assert_eq!(ranges.truncation, census.truncation);
    assert_eq!(ranges.token_ranges.len(), ranges.references.len());
    ranges
}

/// Assert the two inspect siblings are token-identical at `offset` and return
/// the range-bearing report.
fn inspect_parity(source: &[u8], offset: usize) -> ObjectBodyReferenceRangesInspection {
    let census = inspect_object_body_references(source, offset).expect("census inspects");
    let ranges = inspect_object_body_reference_ranges(source, offset).expect("ranges inspect");
    assert_eq!(ranges.references, census.references);
    assert_eq!(ranges.skipped_references, census.skipped_references);
    assert_eq!(ranges.truncation, census.truncation);
    assert_eq!(ranges.token_ranges.len(), ranges.references.len());
    ranges
}

/// Slice the three token ranges of one reference out of the scanned buffer.
fn token_bytes<'a>(
    buffer: &'a [u8],
    tokens: &ObjectBodyReferenceTokenRanges,
) -> (&'a [u8], &'a [u8], &'a [u8]) {
    (
        &buffer[tokens.object_number_range.clone()],
        &buffer[tokens.generation_range.clone()],
        &buffer[tokens.r_keyword_range.clone()],
    )
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
fn per_token_ranges_slice_exact_tokens() {
    let buffer = b"<< /A 12 0 R /B [7 65535 R] >>";

    let report = scan_all_parity(buffer);

    assert_eq!(
        report.references,
        vec![indirect_ref(12, 0), indirect_ref(7, 65535)]
    );
    let (object, generation, keyword) = token_bytes(buffer, &report.token_ranges[0]);
    assert_eq!(
        (object, generation, keyword),
        (&b"12"[..], &b"0"[..], &b"R"[..])
    );
    let (object, generation, keyword) = token_bytes(buffer, &report.token_ranges[1]);
    assert_eq!(
        (object, generation, keyword),
        (&b"7"[..], &b"65535"[..], &b"R"[..])
    );
}

#[test]
fn interior_comment_between_tokens_is_outside_every_range() {
    // ISO 32000-1 7.2.3: the comment is whitespace between tokens; splicing
    // only the numeric ranges must leave it byte-identical in place.
    let buffer = b"[7 % note\n 0 R]";

    let report = scan_all_parity(buffer);

    assert_eq!(report.references, vec![indirect_ref(7, 0)]);
    let tokens = &report.token_ranges[0];
    let (object, generation, keyword) = token_bytes(buffer, tokens);
    assert_eq!(
        (object, generation, keyword),
        (&b"7"[..], &b"0"[..], &b"R"[..])
    );
    // The comment sits strictly between the object-number and generation
    // token ranges.
    let between = &buffer[tokens.object_number_range.end..tokens.generation_range.start];
    assert_eq!(between, b" % note\n ");
}

#[test]
fn splicing_only_numeric_ranges_preserves_all_other_bytes() {
    let buffer = b"<< /X 5 0 R /Note (keep 5 0 R) % 5 0 R\n /Y 5 0 R >>";
    let report = scan_all_parity(buffer);
    assert_eq!(report.references.len(), 2);

    let mut rewritten = buffer.to_vec();
    for tokens in report.token_ranges.iter().rev() {
        rewritten.splice(tokens.generation_range.clone(), b"0".iter().copied());
        rewritten.splice(tokens.object_number_range.clone(), b"900".iter().copied());
    }

    assert_eq!(
        rewritten,
        b"<< /X 900 0 R /Note (keep 5 0 R) % 5 0 R\n /Y 900 0 R >>".to_vec()
    );
}

#[test]
fn strings_comments_and_stream_data_stay_opaque_in_both_siblings() {
    let opaque = scan_all_parity(b"<< /S (1 0 R) /H <313020520A> % 5 0 R\n/N /X >>");
    assert_eq!(opaque.references, Vec::new());
    assert!(opaque.token_ranges.is_empty());

    let stream_source =
        b"4 0 obj\n<< /Length 12 /Ref 6 0 R >>\nstream\nBT 7 0 R ET\nendstream\nendobj\n";
    let report = inspect_parity(stream_source, 0);
    assert_eq!(report.references, vec![indirect_ref(6, 0)]);
    // The one range triple sits inside the dictionary extent, before the
    // stream keyword: stream data is never scanned.
    let stream_keyword = stream_source
        .windows(b"stream".len())
        .position(|window| window == b"stream")
        .expect("stream keyword present");
    assert!(report.token_ranges[0].r_keyword_range.end <= stream_keyword);
}

#[test]
fn inspect_parity_across_body_shape_families() {
    for source in [
        &b"1 0 obj\n<< /A << /B [1 0 R << /C 2 0 R >>] >> >>\nendobj\n"[..],
        &b"1 0 obj\n[3 0 R 3 0 R]\nendobj\n"[..],
        &b"5 0 obj\n2 0 R\nendobj\n"[..],
        &b"5 0 obj\n42\nendobj\n"[..],
        &b"5 0 obj\n/Name\nendobj\n"[..],
        &b"5 0 obj\n(1 0 R)\nendobj\n"[..],
        &b"5 0 obj\ntrue\nendobj\n"[..],
        &b"5 0 obj\nnull\nendobj\n"[..],
        &b"1 0 obj\n[4294967296 0 R 1 65536 R 2 0 R]\nendobj\n"[..],
        &b"9 0 obj\n4294967296 0 R\nendobj\n"[..],
    ] {
        inspect_parity(source, 0);
    }
}

#[test]
fn reference_shaped_scalar_body_reports_aligned_ranges() {
    let source = b"5 0 obj\n2 0 R\nendobj\n";

    let report = inspect_parity(source, 0);

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
    let (object, generation, keyword) = token_bytes(source, &report.token_ranges[0]);
    assert_eq!(
        (object, generation, keyword),
        (&b"2"[..], &b"0"[..], &b"R"[..])
    );
}

#[test]
fn out_of_range_skips_report_parity_with_no_ranges() {
    let report = scan_all_parity(b"[4294967296 0 R 1 65536 R 2 0 R]");

    assert_eq!(report.references, vec![indirect_ref(2, 0)]);
    assert_eq!(report.token_ranges.len(), 1);
    assert_eq!(
        report.skipped_references,
        vec![
            SkippedObjectBodyReference::ObjectNumberOutOfRange,
            SkippedObjectBodyReference::GenerationOutOfRange,
        ]
    );
}

#[test]
fn truncation_parity_at_the_shared_cap() {
    let body = array_body_with_references(MAX_OBJECT_BODY_REFERENCES + 1);

    let report = scan_all_parity(body.as_bytes());

    assert_eq!(report.references.len(), MAX_OBJECT_BODY_REFERENCES);
    assert_eq!(report.token_ranges.len(), MAX_OBJECT_BODY_REFERENCES);
    assert_eq!(
        report.truncation,
        Some(ObjectBodyReferencesTruncation::MaxReferences {
            max_references: MAX_OBJECT_BODY_REFERENCES,
        })
    );
}

#[test]
fn unterminated_string_and_span_clamping_parity() {
    scan_all_parity(b"[2 0 R (unterminated 3 0 R");

    let buffer = b"[1 0 R]";
    let census = scan_indirect_references_in_span(buffer, 0..buffer.len() + 100);
    let ranges = scan_indirect_reference_ranges_in_span(buffer, 0..buffer.len() + 100);
    assert_eq!(ranges.references, census.references);
    assert_eq!(ranges.token_ranges.len(), 1);
}

#[test]
fn span_bounded_ranges_address_the_scanned_buffer() {
    // A decoded-buffer style scan: ranges are absolute in the scanned buffer
    // (here offset by the sibling prefix), never in any other frame.
    let mut buffer = b"9 0 R prefix ".to_vec();
    let start = buffer.len();
    buffer.extend_from_slice(b"<< /Parent 4 0 R >>");
    let span = start..buffer.len();
    buffer.extend_from_slice(b" 11 0 R suffix");

    let report = scan_indirect_reference_ranges_in_span(&buffer, span.clone());

    assert_eq!(report.references, vec![indirect_ref(4, 0)]);
    let tokens = &report.token_ranges[0];
    assert!(tokens.object_number_range.start >= span.start);
    assert!(tokens.r_keyword_range.end <= span.end);
    let (object, generation, keyword) = token_bytes(&buffer, tokens);
    assert_eq!(
        (object, generation, keyword),
        (&b"4"[..], &b"0"[..], &b"R"[..])
    );
}

#[test]
fn inspect_errors_are_the_shared_classifier_rejections() {
    for source in [
        &b"xref\n0 1\n"[..],
        &b"1 0 obj\n<< /A 1 0 R"[..],
        &b"1 0 obj\n[1 0 R"[..],
        &b"1 0 obj"[..],
    ] {
        let census = inspect_object_body_references(source, 0).expect_err("census rejects");
        let ranges = inspect_object_body_reference_ranges(source, 0).expect_err("ranges reject");
        assert_eq!(ranges, census, "both siblings share one rejection surface");
    }
}

#[test]
fn serde_round_trips_the_ranges_report_shape() {
    let report = ObjectBodyReferenceRangesInspection {
        references: vec![indirect_ref(1, 0)],
        token_ranges: vec![ObjectBodyReferenceTokenRanges {
            object_number_range: 3..4,
            generation_range: 5..6,
            r_keyword_range: 7..8,
        }],
        skipped_references: vec![SkippedObjectBodyReference::GenerationOutOfRange],
        truncation: Some(ObjectBodyReferencesTruncation::MaxReferences {
            max_references: MAX_OBJECT_BODY_REFERENCES,
        }),
    };

    let value = serde_value(&report).expect("report should serialize");
    let restored: ObjectBodyReferenceRangesInspection =
        from_serde_value(value).expect("report should deserialize");
    assert_eq!(restored, report);
}
