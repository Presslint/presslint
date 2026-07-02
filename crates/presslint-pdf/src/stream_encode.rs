use miniz_oxide::deflate::compress_to_vec_zlib;
use serde::{Deserialize, Serialize};

/// Fixed zlib compression level used by [`encode_flate_stream`].
///
/// The value is pinned so identical inputs produce byte-identical output for a
/// given `miniz_oxide` version. The encoder emits plain zlib-wrapped Deflate
/// bytes and does not apply PNG or TIFF predictors.
pub const FLATE_ENCODE_LEVEL: u8 = 6;

/// Error returned when a bounded `/FlateDecode` payload cannot be encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlateEncodeStreamError {
    /// Caller-supplied uncompressed byte count.
    pub input_len: usize,
    /// Caller-supplied maximum uncompressed byte count accepted for encoding.
    pub input_limit: usize,
    /// Fixed compression level used by this encoder.
    pub level: u8,
    /// Structured failure reason.
    pub reason: FlateEncodeStreamRejection,
}

/// Structured `/FlateDecode` stream encode rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FlateEncodeStreamRejection {
    /// The uncompressed input exceeds the caller-supplied input limit.
    InputLimitExceeded,
}

/// Encode bytes as a deterministic zlib-wrapped `/FlateDecode` payload.
///
/// The input stays borrowed. The returned encoded byte stream is owned because
/// compression materializes bytes for a future stream replacement. `input_limit`
/// bounds the source byte count before compression is attempted.
///
/// # Errors
///
/// Returns [`FlateEncodeStreamError`] when `input.len()` exceeds `input_limit`.
pub fn encode_flate_stream(
    input: &[u8],
    input_limit: usize,
) -> Result<Vec<u8>, FlateEncodeStreamError> {
    if input.len() > input_limit {
        return Err(FlateEncodeStreamError {
            input_len: input.len(),
            input_limit,
            level: FLATE_ENCODE_LEVEL,
            reason: FlateEncodeStreamRejection::InputLimitExceeded,
        });
    }

    Ok(compress_to_vec_zlib(input, FLATE_ENCODE_LEVEL))
}
