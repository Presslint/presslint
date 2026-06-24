//! Byte-preserving content-stream syntax.

#![forbid(unsafe_code)]

use presslint_core::ByteRange;
use serde::{Deserialize, Serialize};

/// Lexical token with source byte range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    /// Token bytes exactly as observed.
    pub bytes: Vec<u8>,
    /// Source range in the content stream.
    pub range: ByteRange,
}

/// Return the input bytes unchanged.
///
/// This placeholder makes the round-trip contract explicit before the real
/// tokenizer lands.
#[must_use]
pub fn serialize_unmodified(input: &[u8]) -> Vec<u8> {
    input.to_vec()
}

#[cfg(test)]
mod tests {
    use super::serialize_unmodified;

    #[test]
    fn unmodified_serializer_is_byte_identical() {
        let input = b"0 0 0 rg\n10 20 m\n";
        assert_eq!(serialize_unmodified(input), input);
    }
}
