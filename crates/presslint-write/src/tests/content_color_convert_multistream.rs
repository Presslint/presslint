//! Multi-content-stream DeviceLink conversion (T136).
//!
//! Since T136 a page whose `/Contents` names more than one content-stream object
//! is converted by editing each stream OBJECT independently and emitting N dirty
//! objects in one incremental revision, instead of being skipped whole as
//! `MultipleContentStreams`. These SYNTHETIC cases cover: both streams of a page
//! converting; one stream editing while its sibling is analysed-but-unchanged; a
//! shared stream object ownership-skipped while its private sibling converts; the
//! same stream object referenced twice merging into ONE dirty object (no
//! `DuplicateDirtyObject`); and the classic + xref-stream backends. Every case
//! asserts `output.bytes[..input.len()] == input` and that the output reopens.
//!
//! KNOWN LIMITATION (pinned below): each physical stream is still walked
//! independently, so graphics state does NOT propagate across a page's
//! `/Contents` sequence. Explicit device shortcuts stay independently
//! convertible, but a `q` in one stream with its `Q` in the next underflows the
//! second stream's fresh interpreter and conservatively refuses THAT stream. A
//! logical concatenated page-stream walk is a follow-up slice.

// "DeviceLink" is the ICC domain term used as prose here, matching the module
// under test.
#![allow(clippy::doc_markdown)]

use presslint_pdf::DocumentAccessBackend;
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
fn cross_stream_save_restore_conservatively_refuses_the_second_stream() {
    // The page's graphics state legally spans its /Contents sequence
    // (ISO 32000-1 §7.8.2), but each physical stream is walked independently:
    // the `q` stream converts (an unclosed save is not an error), while the
    // sibling opening with `Q` underflows its fresh interpreter and is refused
    // whole. Pinned as the documented conservative limitation of this slice.
    let input = classic_two_stream_pdf(b"q 1 0 0 rg\n", b"Q 0 0 1 rg\n");
    let output = convert_all(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted.len(), 1);
    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 1);
    assert_eq!(content_object_numbers(page), vec![4]);

    assert_eq!(output.skipped.len(), 1);
    let skip = &output.skipped[0];
    assert_eq!(skip.content_object.map(|r| r.object_number), Some(5));
    assert_eq!(skip.reason, ConvertPageSkipReason::ContentRoundTripMismatch);

    // Stream 4 was rewritten; the refused stream 5 keeps its original bytes.
    assert!(!contains(
        &page_encoded_stream_at(&output.bytes, 0, 0),
        b"rg"
    ));
    assert_eq!(page_encoded_stream_at(&output.bytes, 0, 1), b"Q 0 0 1 rg\n");
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
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
