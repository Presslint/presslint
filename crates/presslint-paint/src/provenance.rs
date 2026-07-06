//! Typed byte-range provenance: which buffer a range addresses is a TYPE.
//!
//! A bare [`ByteRange`] does not say what its offsets are relative to. Paint
//! provenance always addresses the DECODED content-stream buffer the walker
//! consumed, while future rewriter/render provenance will address the original
//! PDF source file. These newtypes make that basis explicit so the compiler
//! refuses to mix them.
//!
//! Both types are paint-local for now; they are intended to move to
//! `presslint-types` once the action-planning and write layers adopt typed
//! range bases. Conversion is deliberately explicit and identity-only
//! (`new`/`into_byte_range`): the newtypes expose no deref coercion and no
//! blanket conversion traits in either direction, so a range never changes
//! basis silently.

use presslint_types::ByteRange;
use serde::{Deserialize, Serialize};

/// Byte range into a DECODED buffer — the content stream the walker consumed.
///
/// Offsets are relative to the decoded (post-filter) bytes handed to the
/// walker, NOT to the original PDF source file. `#[serde(transparent)]` keeps
/// the wire shape a bare [`ByteRange`] (`{"start":..,"end":..}`), so adopting
/// this type changes no public JSON. Paint-local; see the module docs for the
/// planned move to `presslint-types`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct DecodedRange(ByteRange);

impl DecodedRange {
    /// Wrap a decoded-buffer range. Identity-only: the offsets are unchanged.
    #[must_use]
    pub const fn new(range: ByteRange) -> Self {
        Self(range)
    }

    /// Unwrap to the bare [`ByteRange`]. Identity-only: the offsets are
    /// unchanged; use this at explicit seams where an untyped range is the
    /// public contract.
    #[must_use]
    pub const fn into_byte_range(self) -> ByteRange {
        self.0
    }

    /// Inclusive start offset into the decoded buffer.
    #[must_use]
    pub const fn start(self) -> usize {
        self.0.start
    }

    /// Exclusive end offset into the decoded buffer.
    #[must_use]
    pub const fn end(self) -> usize {
        self.0.end
    }
}

/// Byte range into the ORIGINAL PDF source file.
///
/// Reserved: no paint field carries it yet; it names the other range basis so
/// the two can never be confused once source-addressed provenance appears.
/// Paint-local; see the module docs for the planned move to `presslint-types`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(transparent)]
#[serde(transparent)]
pub struct SourceRange(ByteRange);

impl SourceRange {
    /// Wrap a source-file range. Identity-only: the offsets are unchanged.
    #[must_use]
    pub const fn new(range: ByteRange) -> Self {
        Self(range)
    }

    /// Unwrap to the bare [`ByteRange`]. Identity-only: the offsets are
    /// unchanged.
    #[must_use]
    pub const fn into_byte_range(self) -> ByteRange {
        self.0
    }
}
