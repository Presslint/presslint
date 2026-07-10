//! Fail-closed refusal of unwalkable physical content streams.
//!
//! Candidate discovery runs through the shared paint interpreter, so ANY
//! graphics-walk error — a malformed direct device operand, a malformed
//! operand of a supported NON-colour operator, or a graphics-state stack
//! underflow — refuses the ENTIRE physical stream. The stream is reported
//! through the existing `ContentRoundTripMismatch` skip, no splice is applied
//! (even for a valid candidate discovered before the error), no tally is
//! recorded, and the stream bytes stay verbatim. This is a deliberate
//! tightening over the earlier per-operator malformed-record skip.

use crate::ConvertPageSkipReason;

use super::content_color_convert::{RGB_TO_CMYK_LINK, classic_raw_pdf, convert, occurrence_count};

/// Assert `stream` refuses whole: not analysed, skipped as a round-trip
/// mismatch, zero appended revision copies of the content object, verbatim
/// input prefix.
fn assert_stream_refused(stream: &[u8]) {
    let input = classic_raw_pdf(stream);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    // No page report at all: a refused stream records no tally, so a valid
    // candidate before the error can never surface as a false converted count.
    assert!(output.converted.is_empty());
    assert_eq!(output.skipped.len(), 1);
    let skip = &output.skipped[0];
    assert_eq!(skip.reason, ConvertPageSkipReason::ContentRoundTripMismatch);
    assert_eq!(skip.content_object.map(|r| r.object_number), Some(4));
    // No revision object was appended for the content stream: the original
    // "4 0 obj" is the only copy, so the stream bytes are untouched.
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
}

#[test]
fn wrong_device_operand_count_refuses_the_whole_stream() {
    assert_stream_refused(b"0 0 rg\n");
}

#[test]
fn non_number_device_operand_refuses_the_whole_stream() {
    assert_stream_refused(b"1 0 /X rg\n");
}

#[test]
fn non_finite_device_operand_refuses_the_whole_stream() {
    // A 400-digit integer is a valid PDF number lexeme but overflows `f64` to
    // +inf, which the walker rejects as a non-finite numeric operand.
    let mut stream = b"1 0 ".to_vec();
    stream.extend(std::iter::repeat_n(b'9', 400));
    stream.extend_from_slice(b" rg\n");
    assert_stream_refused(&stream);
}

#[test]
fn valid_candidate_before_the_error_produces_zero_splice_and_no_tally() {
    // The first rg is a perfectly valid candidate; the later malformed rg
    // discards it with the whole stream.
    assert_stream_refused(b"1 0 0 rg\n0 0 rg\n");
}

#[test]
fn malformed_supported_non_colour_operator_refuses_the_whole_stream() {
    // `cm` takes six numeric operands; three is a walk error even though `cm`
    // is not a colour operator and the rg candidate itself is valid.
    assert_stream_refused(b"1 0 0 rg\n1 0 0 cm\n");
}

#[test]
fn graphics_state_stack_underflow_refuses_the_whole_stream() {
    // `Q` with no matching `q` underflows the interpreter's stack.
    assert_stream_refused(b"Q\n1 0 0 rg\n");
}
