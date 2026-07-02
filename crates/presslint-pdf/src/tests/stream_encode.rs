use crate::{
    FLATE_ENCODE_LEVEL, FlateDecodeParameters, FlateEncodeStreamError, FlateEncodeStreamRejection,
    decode_flate_stream, encode_flate_stream,
};

fn round_trip(input: &[u8]) {
    let encoded = encode_flate_stream(input, input.len()).expect("input should encode");
    let decoded = decode_flate_stream(&encoded, FlateDecodeParameters::default(), input.len())
        .expect("encoded bytes should decode");

    assert_eq!(decoded, input);
}

#[test]
fn round_trips_empty_input() {
    round_trip(b"");
}

#[test]
fn round_trips_small_content_stream_body() {
    round_trip(b"q\n0 0 1 rg\n36 144 200 80 re\nf\nQ\n");
}

#[test]
fn round_trips_large_repetitive_input() {
    let mut input = Vec::new();
    for _ in 0..4096 {
        input.extend_from_slice(b"BT /F1 12 Tf 72 720 Td (PressLint) Tj ET\n");
    }

    round_trip(&input);
}

#[test]
fn round_trips_high_entropy_input() {
    let mut state = 0x1234_5678u32;
    let input: Vec<u8> = (0..8192)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect();

    round_trip(&input);
}

#[test]
fn round_trips_already_compressed_input_bytes() {
    let first =
        encode_flate_stream(b"already compressed bytes", 64).expect("fixture bytes should encode");

    round_trip(&first);
}

#[test]
fn output_is_deterministic_for_same_input() {
    let input = b"q\n/GS1 gs\n0 0 0 rg\n(black text) Tj\nQ\n";

    let first = encode_flate_stream(input, 1024).expect("first encode should succeed");
    let second = encode_flate_stream(input, 1024).expect("second encode should succeed");

    assert_eq!(first, second);
    assert_eq!(FLATE_ENCODE_LEVEL, 6);
}

#[test]
fn rejects_input_over_limit_without_encoding() {
    let error = encode_flate_stream(b"0123456789", 4).expect_err("over-limit input should reject");

    assert_eq!(
        error,
        FlateEncodeStreamError {
            input_len: 10,
            input_limit: 4,
            level: FLATE_ENCODE_LEVEL,
            reason: FlateEncodeStreamRejection::InputLimitExceeded,
        }
    );
}

#[test]
fn real_decoded_content_stream_body_round_trips_with_default_parameters() {
    let decoded = b"q\n0.12 0.34 0.56 rg\n72 144 240 120 re\nf\nBT\n/F1 11 Tf\n72 700 Td\n(Hello) Tj\nET\nQ\n";
    let encoded = encode_flate_stream(decoded, 1024).expect("content stream body should encode");

    let round_tripped =
        decode_flate_stream(&encoded, FlateDecodeParameters::default(), decoded.len())
            .expect("default FlateDecode parameters should round-trip");

    assert_eq!(round_tripped, decoded);
}

#[test]
fn error_shape_reports_bound_and_fixed_level() {
    let error = FlateEncodeStreamError {
        input_len: 10,
        input_limit: 4,
        level: FLATE_ENCODE_LEVEL,
        reason: FlateEncodeStreamRejection::InputLimitExceeded,
    };

    assert_eq!(error.input_len, 10);
    assert_eq!(error.input_limit, 4);
    assert_eq!(error.level, 6);
    assert_eq!(error.reason, FlateEncodeStreamRejection::InputLimitExceeded);
}
