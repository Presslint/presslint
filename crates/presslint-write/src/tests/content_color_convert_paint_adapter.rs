//! Paint-driven candidate discovery: parse-once seam, exact shortcut-byte
//! eligibility, record-range splices, and resource-operator exclusion.
//!
//! The converter discovers candidates exclusively through the shared paint
//! interpreter and splices at each event's record range. These tests pin the
//! `ParsedContent` seam invariants (byte-identical token serialization, single
//! assembly, refusal shapes) and that ONLY the six exact direct device shortcut
//! operators (`g`/`G`, `rg`/`RG`, `k`/`K`) are eligible — resource colour
//! operators, lookalike operator bytes, and payload text resembling a shortcut
//! all stay byte-verbatim.

use presslint_syntax::serialize_tokens_unmodified;

use crate::OperatorSkipCounts;
use crate::parsed_content::ParsedContent;

use super::content_color_convert::{
    GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, classic_raw_pdf, contains, convert, occurrence_count,
    operands_of, page_decoded_stream,
};

// --- ParsedContent: the parse-once seam ------------------------------------

#[test]
fn parsed_content_round_trips_tokens_and_assembles_records_once() {
    let decoded = b"q % note\n1 0 0 rg\nQ\n";
    let parsed = ParsedContent::parse(decoded).expect("valid stream parses");

    assert_eq!(parsed.bytes(), decoded);
    // The owned tokens re-serialize byte-identically (comments included).
    let serialized =
        serialize_tokens_unmodified(decoded, parsed.tokens()).expect("tokens serialize");
    assert_eq!(serialized, decoded);
    // Exactly one assembly: q, rg, Q — with exact record ranges.
    let records = parsed.records();
    assert_eq!(records.len(), 3);
    assert_eq!(&decoded[records[0].range.start..records[0].range.end], b"q");
    assert_eq!(
        &decoded[records[1].range.start..records[1].range.end],
        b"1 0 0 rg"
    );
    assert_eq!(&decoded[records[2].range.start..records[2].range.end], b"Q");
}

#[test]
fn parsed_content_accepts_unusual_whitespace() {
    let decoded = b"1\t0\r0 rg";
    let parsed = ParsedContent::parse(decoded).expect("unusual whitespace parses");
    assert_eq!(parsed.records().len(), 1);
    let record = &parsed.records()[0];
    assert_eq!(&decoded[record.range.start..record.range.end], decoded);
}

#[test]
fn parsed_content_accepts_the_empty_stream() {
    let parsed = ParsedContent::parse(b"").expect("empty stream parses");
    assert!(parsed.bytes().is_empty());
    assert!(parsed.tokens().is_empty());
    assert!(parsed.records().is_empty());
}

#[test]
fn parsed_content_refuses_a_malformed_token() {
    // Unterminated literal string: tokenization fails.
    assert!(ParsedContent::parse(b"(never closed").is_none());
}

#[test]
fn parsed_content_refuses_a_failed_assembly() {
    // Trailing operands with no operator: assembly fails.
    assert!(ParsedContent::parse(b"1 0 0\n").is_none());
}

// --- Exact shortcut-byte eligibility ----------------------------------------

#[test]
fn sc_at_default_device_gray_state_is_not_eligible() {
    // `0.5 sc` sets the default DeviceGray nonstroking colour, so the paint
    // walk emits a DeviceGray colour event — but the operator bytes are `sc`,
    // not `g`, so it is ineligible and UNCOUNTED even with a Gray link routed.
    let input = classic_raw_pdf(b"0.5 sc\n");
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        output.converted[0].operator_skips,
        OperatorSkipCounts::default()
    );
    assert!(output.skipped.is_empty());
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(page_decoded_stream(&output.bytes, false), b"0.5 sc\n");
}

#[test]
fn resource_colour_space_operators_stay_verbatim_next_to_a_converted_shortcut() {
    // Every cs/CS/sc/scn/SC/SCN form stays byte-verbatim while the explicit rg
    // shortcut in the same stream converts.
    let stream = b"/CS0 cs 0.1 0.2 0.3 scn\n/CS0 CS 0.4 SCN\n0.5 0.5 sc\n0.6 SC\n1 0 0 rg\n";
    let input = classic_raw_pdf(stream);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        output.converted[0].operator_skips,
        OperatorSkipCounts::default()
    );
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/CS0 cs 0.1 0.2 0.3 scn"));
    assert!(contains(&decoded, b"/CS0 CS 0.4 SCN"));
    assert!(contains(&decoded, b"0.5 0.5 sc"));
    assert!(contains(&decoded, b"0.6 SC"));
    assert!(!contains(&decoded, b"rg"));
    assert_eq!(operands_of(&decoded, b"k").len(), 4);
}

#[test]
fn lookalike_operator_bytes_are_not_eligible() {
    // `rgx` is a NoOp operator whose bytes merely contain `rg`.
    let input = classic_raw_pdf(b"1 0 0 rgx\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        output.converted[0].operator_skips,
        OperatorSkipCounts::default()
    );
    assert_eq!(page_decoded_stream(&output.bytes, false), b"1 0 0 rgx\n");
}

#[test]
fn payload_text_resembling_rg_stays_untouched() {
    // An inline-image-like token run and a string payload both contain `rg` as
    // TEXT, never as an exact operator token, so nothing is eligible.
    let stream = b"BI /W 1 /H 1 ID 00rg00 EI\n(1 0 0 rg) Tj\n";
    let input = classic_raw_pdf(stream);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(page_decoded_stream(&output.bytes, false), stream.as_slice());
}

// --- Record-range splices ----------------------------------------------------

#[test]
fn comments_survive_length_changing_splices_on_both_sides() {
    // Both shortcuts convert with length-changing replacements; comment and
    // whitespace bytes OUTSIDE the two record ranges survive verbatim.
    let input = classic_raw_pdf(b"% head\n1 0 0 rg % tail\n0 0 1 RG\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 2);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(decoded.starts_with(b"% head\n"));
    assert!(contains(&decoded, b" % tail\n"));
    assert!(!contains(&decoded, b"rg"));
    assert!(!contains(&decoded, b"RG"));
    assert_eq!(operands_of(&decoded, b"k").len(), 4);
    assert_eq!(operands_of(&decoded, b"K").len(), 4);
}

#[test]
fn record_range_bounds_the_splice_exactly() {
    // The record range spans the first operand token through the operator, so
    // the splice canonicalizes whitespace INSIDE the record while the trivia
    // before and after it survives byte-verbatim.
    let input = classic_raw_pdf(b"q\t1  0\t0 rg\tQ\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(decoded.starts_with(b"q\t"));
    assert!(decoded.ends_with(b"\tQ\n"));
    assert!(!contains(&decoded, b"1  0"));
    assert_eq!(operands_of(&decoded, b"k").len(), 4);
}

#[test]
fn shortcuts_convert_regardless_of_save_restore_state() {
    // Eligibility is per exact operator bytes; the interpreter's q/Q state
    // never gates an explicit shortcut.
    let input = classic_raw_pdf(b"q 1 0 0 rg Q 0 1 0 rg\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 2);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    assert!(decoded.starts_with(b"q "));
    assert!(contains(&decoded, b" Q "));
}
