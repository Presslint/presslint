//! Interim ExtGState (`gs`) guard on the device-colour converter (T140).
//!
//! The shipped converter is overprint/transparency BLIND, so a page whose content
//! sets an ExtGState via `gs` must be left byte-verbatim rather than risk a silent
//! under-overprint colour change. These SYNTHETIC cases cover: the cross-stream
//! acceptance case (stream 0 sets `/GS1 gs`, stream 1 paints a convertible `k` —
//! the WHOLE page converts nothing, ISO 32000 §7.8.2 shared graphics state); a
//! single-stream `gs` page guarded; a no-`gs` page converting unaffected; a run
//! where every page is guarded appending only the no-op revision; and the pure
//! operator-precise scanner. Every guarded case asserts
//! `output.bytes[..input.len()] == input`.

// "ExtGState" and "DeviceLink" are PDF/ICC domain terms used as prose here.
#![allow(clippy::doc_markdown)]

use presslint_types::PageIndex;

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    ConvertPageSkipReason, PageSelection, convert_content_colors_incremental,
};

use super::content_color_convert::{
    CMYK_TO_CMYK_LINK, assemble_classic, classic_raw_pdf, occurrence_count, one_link, page_body,
    page_encoded_stream_at, stream_body,
};

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";

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

/// Convert every page of `input` through one CMYK->CMYK link, no black overlay.
/// A CMYK link makes a `k`/`K` operator a convertible source, so a guarded page
/// leaves a real conversion on the table (proving the guard, not mere coverage).
fn convert_cmyk(input: &[u8]) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(CMYK_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds")
}

#[test]
fn cross_stream_gs_guards_the_whole_page() {
    // Stream 0 sets an ExtGState (`/GS1 gs`); stream 1 paints a convertible CMYK
    // black (`0 0 0 1 k`). The streams share graphics state, so the whole page is
    // poisoned and stream 1 converts NOTHING.
    let input = classic_two_stream_pdf(b"/GS1 gs\n", b"0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    // No page analysed/converted; exactly one whole-page ExtGState skip.
    assert!(output.converted.is_empty());
    assert_eq!(output.skipped.len(), 1);
    let skip = &output.skipped[0];
    assert_eq!(skip.page_index, PageIndex(0));
    assert_eq!(skip.content_object, None);
    assert!(matches!(
        skip.reason,
        ConvertPageSkipReason::ExtGStatePresent
    ));

    // Byte-verbatim prefix; neither stream object is re-appended, and the `k`
    // stream still decodes to its original bytes (nothing converted cross-stream).
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(page_encoded_stream_at(&output.bytes, 0, 1), b"0 0 0 1 k\n");
}

#[test]
fn single_stream_gs_page_is_guarded() {
    let input = classic_raw_pdf(b"/GS1 gs\n0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    assert!(output.converted.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert_eq!(output.skipped[0].content_object, None);
    assert!(matches!(
        output.skipped[0].reason,
        ConvertPageSkipReason::ExtGStatePresent
    ));
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    // The content object is left byte-verbatim (not re-appended).
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
}

#[test]
fn no_gs_page_converts_unaffected_by_guard() {
    // Identical shape to the guarded single-stream case but WITHOUT `gs`: the
    // convertible `k` operator is still converted, so the guard never fires.
    let input = classic_raw_pdf(b"0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    // The converted stream object is re-appended (original + one rewrite).
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
}

#[test]
fn all_pages_guarded_appends_only_the_no_op_revision() {
    // When every selected page is guarded, no dirty object is emitted, so the
    // output is exactly the foundational empty/no-op incremental revision.
    let input = classic_raw_pdf(b"/GS1 gs\n0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    let no_op = crate::write_incremental_revision(&input, &[]).expect("no-op revision");
    assert_eq!(output.bytes, no_op);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
}

#[test]
fn scanner_matches_the_operator_not_a_string_or_name() {
    use crate::extgstate_guard::has_extgstate;

    // A `gs` OPERATOR is detected.
    assert!(has_extgstate(b"/GS1 gs\n"));
    assert!(has_extgstate(b"q /GS1 gs 0 0 0 1 k Q\n"));
    // The bytes "gs" inside a literal string are NOT the operator.
    assert!(!has_extgstate(b"(gs) Tj\n"));
    // The bytes "gs" inside a name operand are NOT the operator.
    assert!(!has_extgstate(b"/gs 1 Tf\n"));
    // A plain painted stream has no `gs`.
    assert!(!has_extgstate(b"0 0 0 1 k\n"));
    // Undecodable content is conservatively treated as no-`gs` (the pipeline's
    // per-stream round-trip gate already refuses to convert it).
    assert!(!has_extgstate(b"(unterminated"));
}
