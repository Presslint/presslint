//! Structural PDF access interfaces.
//!
//! This crate will own document opening, object lookup, stream access, and
//! deterministic write seams. The initial scaffold keeps only public data
//! contracts so higher-level crates can depend on a stable boundary.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod source;

#[cfg(test)]
mod tests;

pub use source::{
    PDF_HEADER_SCAN_LIMIT, PdfHeader, PdfSourceDiagnostic, PdfSourceInspection,
    PdfSourceInspectionError, PdfSourceRejection, PdfStartXref, PdfStartXrefIssue, PdfVersion,
    PdfXrefSectionIssue, STARTXREF_SCAN_LIMIT, XREF_SECTION_SCAN_LIMIT, XrefSection,
    inspect_pdf_source,
};

/// PDF indirect reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct IndirectRef {
    /// Object number.
    pub object_number: u32,
    /// Generation number.
    pub generation: u16,
}

/// Proven ownership state for a planned indirect-object edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum IndirectObjectOwnership {
    /// The target object is proven to be owned by exactly one consumer.
    ProvenSingleUse {
        /// The only proven owning consumer.
        owner: IndirectRef,
    },
    /// The target object is proven to be consumed by multiple owners.
    Shared {
        /// Proven owning consumers in deterministic indirect-reference order.
        consumers: Vec<IndirectRef>,
    },
    /// Ownership was not proven.
    Unproven,
}

/// Disposition for a planned indirect-object edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndirectObjectEditDisposition {
    /// The target object may be mutated in place.
    InPlaceMutation,
    /// The edit must be represented as a private copy for the consumer.
    PrivateCopy,
}

/// Pure decision result for an indirect-object edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectObjectEditDecision {
    /// Indirect object considered for editing.
    pub target: IndirectRef,
    /// Proven ownership state used for the decision.
    pub ownership: IndirectObjectOwnership,
    /// Required edit disposition.
    pub disposition: IndirectObjectEditDisposition,
}

/// Decide whether a future edit to an indirect object may mutate in place.
///
/// Only exactly one unique proven owning consumer permits in-place mutation.
/// Empty, shared, duplicate-insensitive, or otherwise unproven ownership
/// requires a private copy.
#[must_use]
pub fn decide_indirect_object_edit<I>(
    target: IndirectRef,
    proven_consumers: I,
) -> IndirectObjectEditDecision
where
    I: IntoIterator<Item = IndirectRef>,
{
    let consumers: Vec<_> = proven_consumers
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let ownership = match consumers.as_slice() {
        [] => IndirectObjectOwnership::Unproven,
        [owner] => IndirectObjectOwnership::ProvenSingleUse { owner: *owner },
        _ => IndirectObjectOwnership::Shared { consumers },
    };

    let disposition = match &ownership {
        IndirectObjectOwnership::ProvenSingleUse { .. } => {
            IndirectObjectEditDisposition::InPlaceMutation
        }
        IndirectObjectOwnership::Shared { .. } | IndirectObjectOwnership::Unproven => {
            IndirectObjectEditDisposition::PrivateCopy
        }
    };

    IndirectObjectEditDecision {
        target,
        ownership,
        disposition,
    }
}

/// Document identity returned by an opener.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentInfo {
    /// Number of pages.
    pub page_count: usize,
    /// PDF header version when known.
    pub pdf_version: Option<(u8, u8)>,
}
