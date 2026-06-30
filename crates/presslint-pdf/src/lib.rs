//! Structural PDF access interfaces.
//!
//! This crate will own document opening, object lookup, stream access, and
//! deterministic write seams. The initial scaffold keeps only public data
//! contracts so higher-level crates can depend on a stable boundary.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod array_extent;
mod catalog_pages;
mod classic_xref;
mod content_stream_extent;
mod dictionary_entries;
mod dictionary_extent;
mod indirect_reference;
mod integer_object;
mod object_body;
mod object_dictionary;
mod object_header;
mod object_stream;
mod page_content_extents;
mod page_content_targets;
mod page_contents;
mod page_tree_kid_targets;
mod page_tree_kids;
mod page_tree_node;
mod page_tree_node_type;
mod page_tree_reference;
mod source;
mod source_utils;
mod startxref;
mod trailer;
mod trailer_root;
mod xref_resolve;
mod xref_section;

#[cfg(test)]
mod tests;

pub use array_extent::{
    ArrayExtentInspection, ArrayExtentInspectionError, ArrayExtentInspectionRejection,
    inspect_array_extent,
};
pub use catalog_pages::{
    CatalogPagesInspection, CatalogPagesInspectionError, CatalogPagesInspectionRejection,
    inspect_catalog_pages,
};
pub use classic_xref::inspect_classic_xref_table;
pub use content_stream_extent::{
    ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionError,
    ContentStreamDataExtentInspectionRejection, inspect_content_stream_data_extent,
};
pub use dictionary_entries::{
    DictionaryEntryByteRange, DictionaryEntryInspection, DictionaryEntryInspectionError,
    DictionaryEntryInspectionRejection, DictionaryEntrySpan, DictionaryValueKind,
    inspect_dictionary_entries,
};
pub use dictionary_extent::{
    DictionaryExtentInspection, DictionaryExtentInspectionError,
    DictionaryExtentInspectionRejection, inspect_dictionary_extent,
};
pub use indirect_reference::{
    IndirectReferenceByteRange, IndirectReferenceInspection, IndirectReferenceInspectionError,
    IndirectReferenceInspectionRejection, parse_indirect_reference,
};
pub use integer_object::{
    ClassicXrefIntegerObjectResolution, ClassicXrefIntegerObjectResolutionError,
    ClassicXrefIntegerObjectResolutionRejection, IntegerObjectValueByteRange,
    resolve_classic_xref_integer_object,
};
pub use object_body::{
    IndirectObjectBodyLeadingTokenKind, IndirectObjectBodyTokenInspection,
    IndirectObjectBodyTokenInspectionError, IndirectObjectBodyTokenInspectionRejection,
    inspect_indirect_object_body_token,
};
pub use object_dictionary::{
    IndirectObjectDictionaryInspection, IndirectObjectDictionaryInspectionError,
    IndirectObjectDictionaryInspectionRejection, inspect_indirect_object_dictionary,
};
pub use object_header::{
    IndirectObjectHeaderByteRange, IndirectObjectHeaderInspection,
    IndirectObjectHeaderInspectionError, IndirectObjectHeaderInspectionRejection,
    inspect_indirect_object_header,
};
pub use object_stream::{
    ContentStreamStartInspection, ContentStreamStartInspectionError,
    ContentStreamStartInspectionRejection, DirectLengthContentStreamDataExtentInspection,
    DirectLengthContentStreamDataExtentInspectionError,
    DirectLengthContentStreamDataExtentInspectionRejection,
    IndirectLengthContentStreamDataExtentInspection,
    IndirectLengthContentStreamDataExtentInspectionError,
    IndirectLengthContentStreamDataExtentInspectionRejection, StreamEolIssue, StreamKeywordEol,
    inspect_content_stream_start, inspect_direct_length_content_stream_data_extent,
    inspect_indirect_length_content_stream_data_extent,
};
pub use page_content_extents::{
    PageContentExtentInspection, PageContentExtentsInspection, inspect_page_content_extents,
};
pub use page_content_targets::{
    PageContentTargetInspection, PageContentTargetsInspection, SkippedPageContentTargetReason,
    inspect_page_content_targets,
};
pub use page_contents::{
    PageContentReference, PageContentsInspection, PageContentsInspectionError,
    PageContentsInspectionRejection, PageContentsValueShape, SkippedPageContentEntry,
    SkippedPageContentEntryKind, inspect_page_contents,
};
pub use page_tree_kid_targets::{
    PageTreeKidTargetInspection, PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError,
    PageTreeKidTargetsInspectionRejection, inspect_page_tree_kid_targets,
};
pub use page_tree_kids::{
    PageTreeKidReference, PageTreeKidsInspection, PageTreeKidsInspectionError,
    PageTreeKidsInspectionRejection, SkippedPageTreeKid, SkippedPageTreeKidKind,
    inspect_page_tree_kids,
};
pub use page_tree_node::{
    PageTreeNodeInspection, PageTreeNodeInspectionError, PageTreeNodeInspectionRejection,
    inspect_page_tree_node,
};
pub use page_tree_node_type::{
    PageTreeNodeType, PageTreeNodeTypeInspection, PageTreeNodeTypeInspectionError,
    PageTreeNodeTypeInspectionRejection, inspect_page_tree_node_type,
};
pub use page_tree_reference::{
    PageTreeReferenceTargetInspection, PageTreeReferenceTargetInspectionError,
    PageTreeReferenceTargetInspectionRejection, inspect_page_tree_reference_target,
};
pub use source::{
    PDF_HEADER_SCAN_LIMIT, PdfHeader, PdfSourceDiagnostic, PdfSourceInspection,
    PdfSourceInspectionError, PdfSourceRejection, PdfStartXref, PdfStartXrefIssue, PdfVersion,
    PdfXrefSectionIssue, STARTXREF_SCAN_LIMIT, XREF_SECTION_SCAN_LIMIT, XrefSection,
    inspect_pdf_source,
};
pub use trailer::{
    ClassicXrefTrailerDictionaryInspection, ClassicXrefTrailerDictionaryInspectionError,
    ClassicXrefTrailerDictionaryInspectionRejection, inspect_classic_xref_trailer_dictionary,
};
pub use trailer_root::{
    ClassicXrefTrailerRootInspection, ClassicXrefTrailerRootInspectionError,
    ClassicXrefTrailerRootInspectionRejection, inspect_classic_xref_trailer_root,
};
pub use xref_resolve::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefObjectLocation, resolve_classic_xref_object,
};

/// Parsed metadata for a classic cross-reference table.
///
/// This report stores only structural table metadata. It does not retain or
/// copy PDF bytes, object bodies, stream bodies, or trailer dictionary bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicXrefTableInspection {
    /// Byte offset where the `xref` keyword begins.
    pub table_byte_offset: usize,
    /// Parsed table subsections in source order.
    pub subsections: Vec<ClassicXrefSubsection>,
    /// Byte offset where the following `trailer` keyword begins.
    pub trailer_byte_offset: usize,
}

/// Parsed metadata for one classic cross-reference table subsection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicXrefSubsection {
    /// First object number covered by this subsection.
    pub first_object_number: u32,
    /// Number of entries declared by the subsection header.
    pub entry_count: u32,
    /// Fixed-width entries, ordered by object number within this subsection.
    pub entries: Vec<ClassicXrefEntry>,
}

/// Parsed metadata for one classic cross-reference table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicXrefEntry {
    /// Object number assigned by the enclosing subsection and entry position.
    pub object_number: u32,
    /// Generation number from the fixed-width xref entry.
    pub generation: u16,
    /// Byte offset field from the fixed-width xref entry.
    pub byte_offset: usize,
    /// Free or in-use entry state.
    pub state: ClassicXrefEntryState,
}

/// State marker from a classic cross-reference table entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassicXrefEntryState {
    /// Free entry (`f`).
    Free,
    /// In-use entry (`n`).
    InUse,
}

/// Error returned when a classic cross-reference table cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicXrefTableInspectionError {
    /// Caller-supplied byte offset where inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed construct was found, when available.
    pub error_byte_offset: Option<usize>,
    /// Object number associated with an entry-level error, when available.
    pub object_number: Option<u32>,
    /// Structured failure reason.
    pub reason: ClassicXrefTableInspectionRejection,
}

/// Structured classic cross-reference table inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ClassicXrefTableInspectionRejection {
    /// The caller-supplied offset lies beyond the source length.
    OffsetOutOfBounds,
    /// The offset does not point at a classic `xref` table.
    NotXrefTable,
    /// A subsection header was present but not shaped as `first count`.
    MalformedSubsectionHeader,
    /// A subsection header object number does not fit `u32`.
    SubsectionObjectNumberOutOfRange,
    /// A subsection header entry count does not fit `u32`.
    SubsectionEntryCountOutOfRange,
    /// The subsection range cannot be represented as `u32` object numbers.
    SubsectionObjectRangeOutOfRange,
    /// An entry line is missing or malformed.
    MalformedEntry,
    /// An entry generation number does not fit `u16`.
    EntryGenerationOutOfRange,
    /// An entry byte offset does not fit `usize`.
    EntryByteOffsetOutOfRange,
    /// No following `trailer` keyword was found.
    MissingTrailer,
}

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
