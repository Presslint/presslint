//! Append-only incremental-update PDF writing.
//!
//! This crate holds the first byte-writing slice of the presslint F3 patch
//! executor: a deterministic classic-xref *incremental append* writer. Its only
//! current capability is a semantic **no-op**: it copies the caller's input
//! verbatim as the output prefix, then appends one classic incremental revision
//! that rewrites selected existing uncompressed indirect objects with
//! caller-supplied body bytes, followed by a classic cross-reference table and a
//! trailer carrying `/Prev`.
//!
//! It proves the append mechanics the future semantic writer needs — verbatim
//! prefix preservation, appended-object offset accounting, classic xref-entry
//! width, whole-`/Prev`-chain `/Size` computation, and newest-wins resolution
//! through the existing [`presslint_pdf`] access spine — without performing any
//! semantic edit. It deliberately does not encode dictionaries, rewrite content
//! operands, re-encode streams, clone shared objects, delete objects, repair
//! free lists, preserve encryption, or write cross-reference streams. It also
//! rejects hybrid-reference classic trailers carrying `/XRefStm`, because this
//! slice follows only the classic `/Prev` chain and does not merge supplemental
//! xref-stream entries.
//!
//! Structural facts about the input (the final `startxref`, the classic
//! cross-reference `/Prev` chain, `/Root`, and object currency) are read through
//! [`presslint_pdf`] rather than reparsed here, so the writer stays a thin byte
//! assembler over already-validated structural metadata.

#![forbid(unsafe_code)]

mod writer;

pub use writer::{ActiveTrailerError, DirtyObjectBytes, WriteError, write_incremental_revision};

#[cfg(test)]
mod tests;
