//! Multi-content-stream DeviceLink conversion.
//!
//! A page's ordered `/Contents` occurrences are interpreted as one exact decoded
//! sequence while approved replacements remain local to one physical stream.
//! These synthetic cases cover global graphics state and operands, atomic
//! refusal, raw and Flate elements, ownership-vetoed participation, repeated
//! object reconciliation, and both classic and xref-stream backends.

// "DeviceLink" is the ICC domain term used as prose here, matching the module
// under test.
#![allow(clippy::doc_markdown)]

use presslint_pdf::{
    DocumentAccessBackend, FlateDecodeParameters, decode_flate_stream, encode_flate_stream,
};
use presslint_types::PageIndex;

use crate::{
    BlackPreservationPolicy, ConvertContentColorsRequest, ConvertPageSkipReason, PageSelection,
    convert_content_colors_incremental,
};

use super::content_color_convert::{
    RGB_TO_CMYK_LINK, assemble_classic, contains, convert, occurrence_count, one_link, page_body,
    page_encoded_stream_at, stream_body,
};
use super::{reopen, xref_record};

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
const FLATE_LIMIT: usize = 1 << 20;

/// Convert every page of `input` through one RGB->CMYK link, no black overlay.
fn convert_all(input: &[u8]) -> crate::ConvertContentColorsOutput {
    convert(input, RGB_TO_CMYK_LINK)
}

/// Object numbers of a page's analysed content-stream objects.
fn content_object_numbers(page: &crate::ConvertedPage) -> Vec<u32> {
    page.content_objects
        .iter()
        .map(|reference| reference.object_number)
        .collect()
}

/// One-page classic PDF whose page names two content-stream objects (4 and 5).
fn classic_two_stream_pdf(stream_a: &[u8], stream_b: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("[4 0 R 5 0 R]"),
        stream_body("", stream_a),
        stream_body("", stream_b),
    ])
}

/// One-page xref-stream PDF whose page names two content-stream objects (4, 5).
fn xref_stream_two_stream_pdf(stream_a: &[u8], stream_b: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");

    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
    let mut page = Vec::new();
    page.extend_from_slice(b"3 0 obj\n");
    page.extend_from_slice(&page_body("[4 0 R 5 0 R]"));
    page.extend_from_slice(b"\nendobj\n");
    let mut object4 = Vec::new();
    object4.extend_from_slice(b"4 0 obj\n");
    object4.extend_from_slice(&stream_body("", stream_a));
    object4.extend_from_slice(b"\nendobj\n");
    let mut object5 = Vec::new();
    object5.extend_from_slice(b"5 0 obj\n");
    object5.extend_from_slice(&stream_body("", stream_b));
    object5.extend_from_slice(b"\nendobj\n");

    let catalog_offset = buf.len();
    buf.extend_from_slice(catalog);
    let pages_offset = buf.len();
    buf.extend_from_slice(pages);
    let page_offset = buf.len();
    buf.extend_from_slice(&page);
    let object4_offset = buf.len();
    buf.extend_from_slice(&object4);
    let object5_offset = buf.len();
    buf.extend_from_slice(&object5);
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(1, page_offset, 0));
    body.extend_from_slice(&xref_record(1, object4_offset, 0));
    body.extend_from_slice(&xref_record(1, object5_offset, 0));
    body.extend_from_slice(&xref_record(1, xref_offset, 0));

    buf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /Index [0 7] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}

#[test]
fn both_streams_of_a_page_convert_independently() {
    let input = classic_two_stream_pdf(b"1 0 0 rg\n", b"0 0 1 RG\n");
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 2);
    assert_eq!(content_object_numbers(page), vec![4, 5]);

    // Each stream object was rewritten to its destination-space operator.
    let stream4 = page_encoded_stream_at(&output.bytes, 0, 0);
    let stream5 = page_encoded_stream_at(&output.bytes, 0, 1);
    assert!(!contains(&stream4, b"rg"));
    assert!(!contains(&stream5, b"RG"));
    assert!(contains(&stream4, b"k"));
    assert!(contains(&stream5, b"K"));

    // Both stream objects appear exactly once in the appended revision.
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 2);
    assert_eq!(reopen(&output.bytes).page_leaves.leaves.len(), 1);
}

#[test]
fn one_stream_edits_while_its_sibling_is_analysed_but_unchanged() {
    // Stream 4 has a convertible RGB operator; stream 5 has none, so it is
    // analysed but produces no splice and no dirty object.
    let input = classic_two_stream_pdf(b"1 0 0 rg\n", b"q 10 20 m S Q\n");
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 1);
    // Both streams were analysed (reported), in stream-ordinal order.
    assert_eq!(content_object_numbers(page), vec![4, 5]);

    // Only the edited object is re-appended; the unchanged sibling is not.
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    // The unchanged sibling still decodes to its original bytes.
    assert_eq!(
        page_encoded_stream_at(&output.bytes, 0, 1),
        b"q 10 20 m S Q\n"
    );
}

#[test]
fn mixed_raw_and_flate_occurrences_convert_and_reopen() {
    let compressed = encode_flate_stream(b"0 0 1 RG\n", FLATE_LIMIT).expect("encode");
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("[4 0 R 5 0 R]"),
        stream_body("", b"1 0 0 rg\n"),
        stream_body(" /Filter /FlateDecode", &compressed),
    ]);
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 2);
    assert_eq!(content_object_numbers(&output.converted[0]), vec![4, 5]);

    let raw = page_encoded_stream_at(&output.bytes, 0, 0);
    let flate = page_encoded_stream_at(&output.bytes, 0, 1);
    let decoded_flate = decode_flate_stream(&flate, FlateDecodeParameters::default(), FLATE_LIMIT)
        .expect("decode rewritten Flate stream");
    assert!(!contains(&raw, b"rg"));
    assert!(contains(&raw, b"k"));
    assert!(!contains(&decoded_flate, b"RG"));
    assert!(contains(&decoded_flate, b"K"));
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 2);
    assert_eq!(reopen(&output.bytes).page_leaves.leaves.len(), 1);
}

#[test]
fn shared_stream_object_is_ownership_skipped_while_private_sibling_converts() {
    // Page A (obj 3) names [4 0 R 5 0 R]; page B (obj 6) names 5 0 R, so object 5
    // is shared (two owners) and object 4 is private to page A.
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_vec(),
        page_body("[4 0 R 5 0 R]"),
        stream_body("", b"1 0 0 rg\n"),
        stream_body("", b"0 0 1 RG\n"),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_vec(),
    ]);

    // Convert only page A; object 5 is still shared across the whole document.
    let output = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::Indices(vec![PageIndex(0)]),
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds");

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    // Page A converts its private stream 4 only.
    assert_eq!(output.converted.len(), 1);
    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 1);
    assert_eq!(content_object_numbers(page), vec![4]);

    // The shared object 5 is reported as an ownership skip on page A.
    assert_eq!(output.skipped.len(), 1);
    let skip = &output.skipped[0];
    assert_eq!(skip.content_object.map(|r| r.object_number), Some(5));
    assert!(matches!(
        skip.reason,
        ConvertPageSkipReason::OwnershipNotInPlace { occurrences: 2, .. }
    ));
    // Only object 4 is re-appended; the shared object 5 is untouched.
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
}

#[test]
fn same_stream_object_referenced_twice_merges_to_one_dirty_object() {
    // A page whose /Contents names object 4 twice. Its edits merge into ONE dirty
    // object, so the plan is never handed two dirty objects with the same number.
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("[4 0 R 4 0 R]"),
        stream_body("", b"1 0 0 rg\n"),
    ]);
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    let page = &output.converted[0];
    // Object 4 is edited and counted exactly once.
    assert_eq!(page.operators_converted, 1);
    assert_eq!(content_object_numbers(page), vec![4]);
    // Exactly one appended copy of object 4 (original + one rewrite).
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(reopen(&output.bytes).page_leaves.leaves.len(), 1);
}

#[test]
fn cross_stream_save_restore_walks_as_one_logical_sequence() {
    let input = classic_two_stream_pdf(b"q 1 0 0 rg\n", b"Q 0 0 1 rg\n");
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted.len(), 1);
    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 2);
    assert_eq!(content_object_numbers(page), vec![4, 5]);
    assert!(output.skipped.is_empty());
    assert!(!contains(
        &page_encoded_stream_at(&output.bytes, 0, 0),
        b"rg"
    ));
    assert!(!contains(
        &page_encoded_stream_at(&output.bytes, 0, 1),
        b"rg"
    ));
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 2);
}

#[test]
fn composite_operand_and_operator_may_span_occurrences() {
    let input = classic_two_stream_pdf(b"[1 2", b"] 0 d\n1 0 0 rg\n");
    let output = convert_all(&input);

    assert!(output.skipped.is_empty());
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(content_object_numbers(&output.converted[0]), vec![4, 5]);
    assert_eq!(page_encoded_stream_at(&output.bytes, 0, 0), b"[1 2");
    assert!(!contains(
        &page_encoded_stream_at(&output.bytes, 0, 1),
        b"rg"
    ));
}

#[test]
fn selected_cross_occurrence_replacement_refuses_the_page_atomically() {
    let input = classic_two_stream_pdf(b"1 0 ", b"0 rg\n");
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.converted.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert_eq!(
        output.skipped[0].reason,
        ConvertPageSkipReason::ContentRoundTripMismatch
    );
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
}

#[test]
fn later_unwalkable_occurrence_rolls_back_an_early_candidate() {
    let input = classic_two_stream_pdf(b"1 0 0 rg\n", b"Q\n");
    let output = convert_all(&input);

    assert!(output.converted.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert_eq!(
        output.skipped[0].reason,
        ConvertPageSkipReason::ContentRoundTripMismatch
    );
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
}

#[test]
fn ownership_vetoed_occurrence_still_carries_graphics_state() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_vec(),
        page_body("[5 0 R 4 0 R]"),
        stream_body("", b"Q\n1 0 0 rg\n"),
        stream_body("", b"q\n"),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_vec(),
    ]);
    let output = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::Indices(vec![PageIndex(0)]),
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds");

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(content_object_numbers(&output.converted[0]), vec![4]);
    assert_eq!(output.skipped.len(), 1);
    assert!(matches!(
        output.skipped[0].reason,
        ConvertPageSkipReason::OwnershipNotInPlace { .. }
    ));
}

#[test]
fn xref_stream_multi_stream_page_converts_both_and_reopens_as_chain() {
    let input = xref_stream_two_stream_pdf(b"1 0 0 rg\n", b"0 0 1 RG\n");
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 2);
    assert_eq!(content_object_numbers(&output.converted[0]), vec![4, 5]);
    assert!(matches!(
        reopen(&output.bytes).backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    assert!(!contains(
        &page_encoded_stream_at(&output.bytes, 0, 0),
        b"rg"
    ));
    assert!(!contains(
        &page_encoded_stream_at(&output.bytes, 0, 1),
        b"RG"
    ));
}
