//! Parse-once view over one decoded content stream.
//!
//! [`ParsedContent`] tokenizes a decoded content-stream buffer exactly once,
//! verifies that the unmodified token serialization reproduces the input
//! byte-for-byte, and assembles the operator records exactly once. The shared
//! content-edit pipeline builds it before invoking an operator-interpreting
//! edit callback, so such a callback never re-runs its own tokenize/assemble
//! pass over the same bytes. Raw byte callbacks are unaffected: their public
//! contract stays `&[u8]`, adapted at the pipeline boundary.
//!
//! The view is deliberately minimal: borrowed decoded bytes plus owned syntax
//! vectors, private fields, borrowed accessors only. It stores no walker, no
//! paint program, and no caches, and it lives only for the edit-callback
//! lifetime — the token and record vectors drop with it.

use presslint_syntax::{
    OperatorRecord, Token, assemble_operators, serialize_tokens_unmodified, tokenize,
};

/// Borrowed decoded bytes plus owned tokens and assembled operator records,
/// produced by one validated parse.
pub struct ParsedContent<'a> {
    /// The decoded content-stream bytes the tokens and records address.
    decoded: &'a [u8],
    /// Owned source-preserving tokens of `decoded`.
    tokens: Vec<Token>,
    /// Owned operator records assembled from `tokens`.
    records: Vec<OperatorRecord>,
}

impl<'a> ParsedContent<'a> {
    /// Parse `decoded` once: tokenize, verify the unmodified token
    /// serialization equals the input byte-for-byte, and assemble the
    /// operator records.
    ///
    /// Returns `None` when tokenization fails, the serialization round-trip is
    /// not byte-identical, or assembly fails; the pipeline maps that to its
    /// existing non-fatal round-trip mismatch skip.
    pub fn parse(decoded: &'a [u8]) -> Option<Self> {
        let tokens = tokenize(decoded).ok()?;
        let serialized = serialize_tokens_unmodified(decoded, &tokens).ok()?;
        if serialized != decoded {
            return None;
        }
        let assembled = assemble_operators(&tokens).ok()?;
        Some(Self {
            decoded,
            tokens,
            records: assembled.records,
        })
    }

    /// The decoded content-stream bytes this view was parsed from.
    #[must_use]
    pub const fn bytes(&self) -> &'a [u8] {
        self.decoded
    }

    /// The owned source-preserving tokens of [`bytes`](Self::bytes).
    ///
    /// The shipped converter consumes [`bytes`](Self::bytes) plus
    /// [`records`](Self::records); the token vector is retained so the parse
    /// happens once per stream, and this accessor is exercised by the
    /// parse-once seam tests.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tokens(&self) -> &[Token] {
        &self.tokens
    }

    /// The operator records assembled from [`tokens`](Self::tokens).
    #[must_use]
    pub fn records(&self) -> &[OperatorRecord] {
        &self.records
    }
}
