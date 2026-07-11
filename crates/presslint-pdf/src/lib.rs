//! Structural PDF access interfaces.
//!
//! This crate provides byte-preserving structural inspection for PDF sources:
//! source classification, classic xref parsing, single-section xref-stream
//! decoding, bounded `/Prev` same-type chaining, backend-neutral [`ObjectLookup`]
//! views, bounded object-stream member resolution for structural objects,
//! page-tree traversal, stream extent access, and small planning contracts used
//! by higher-level crates. Classic helpers remain thin wrappers over neutral
//! lookup-aware or resolved-object-aware variants, so classic-xref,
//! incrementally updated same-type chains, and xref-stream documents share the
//! same structural access spine where supported. The APIs carry structural
//! metadata and byte ranges rather than retaining source payloads.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

mod array_extent;
mod catalog_pages;
mod classic_xref;
mod classic_xref_chain;
mod content_stream_extent;
mod content_stream_filter;
mod content_stream_slice;
mod decode_parms;
mod default_color_spaces;
mod dictionary_entries;
mod dictionary_extent;
mod document_access;
mod document_page_content_extents;
mod extgstate_classify;
mod form_color_space_resources;
mod form_extgstate_resources;
mod form_transparency_group;
mod form_xobject_resources;
mod icc_profile;
mod image_xobject;
mod indirect_reference;
mod integer_object;
mod object_body;
mod object_body_references;
mod object_consumer_index;
mod object_dictionary;
mod object_header;
mod object_lookup;
mod object_resolver;
mod object_stream;
mod object_stream_objects;
mod output_intents;
mod page_boxes;
mod page_color_space_classify;
mod page_color_space_resources;
mod page_content_extents;
mod page_content_targets;
mod page_contents;
mod page_contents_resolved;
mod page_extgstate_resources;
mod page_resource_inheritance;
mod page_transparency_group;
mod page_tree_kid_targets;
mod page_tree_kids;
mod page_tree_leaves;
mod page_tree_node;
mod page_tree_node_type;
mod page_tree_reference;
mod page_xobject_resource_targets;
mod page_xobject_resources;
mod source;
mod source_utils;
mod startxref;
mod stream_decode;
mod stream_encode;
mod trailer;
mod trailer_prev;
mod trailer_root;
mod transparency_group_classify;
mod xref_chain;
mod xref_resolve;
mod xref_section;
mod xref_stream;
mod xref_stream_entries;
mod xref_stream_map;
mod xref_stream_trailer;

#[cfg(test)]
mod tests;

pub use array_extent::{
    ArrayExtentInspection, ArrayExtentInspectionError, ArrayExtentInspectionRejection,
    inspect_array_extent,
};
pub use catalog_pages::{
    CatalogPagesInspection, CatalogPagesInspectionError, CatalogPagesInspectionRejection,
    inspect_catalog_pages, inspect_catalog_pages_resolved,
};
pub use classic_xref::inspect_classic_xref_table;
pub use classic_xref_chain::{
    ClassicXrefChain, ClassicXrefChainError, ClassicXrefChainRejection,
    MAX_CLASSIC_XREF_CHAIN_ENTRIES, MAX_CLASSIC_XREF_CHAIN_SECTIONS, build_classic_xref_chain,
    resolve_classic_xref_chain_object,
};
pub use content_stream_extent::{
    ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionError,
    ContentStreamDataExtentInspectionRejection, LookupIndirectLengthRejection,
    inspect_content_stream_data_extent, inspect_content_stream_data_extent_with_lookup,
};
pub use content_stream_filter::{
    ContentStreamFilterClassification, ContentStreamFilterClassificationError,
    ContentStreamFilterClassificationRejection, classify_content_stream_filter,
};
pub use content_stream_slice::{
    ContentStreamDataSliceError, ContentStreamDataSliceRejection, content_stream_data_slice,
};
pub use decode_parms::{
    DecodeParmsParameter, FlateDecodeParametersResolution, FlateDecodeParametersResolutionError,
    FlateDecodeParametersResolutionRejection, resolve_flate_decode_parameters,
};
pub use default_color_spaces::{
    DefaultColorSpaceFact, DefaultColorSpaceKind, DocumentPageDefaultColorSpacesInspection,
    DocumentPageDefaultColorSpacesInspectionError, FormDefaultColorSpacesInspection,
    PageDefaultColorSpacesInspection, SkippedDefaultColorSpace, SkippedDefaultColorSpaceReason,
    inspect_document_page_default_color_spaces,
    inspect_document_page_default_color_spaces_with_lookup, inspect_form_default_color_spaces,
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
pub use document_access::{
    ClassicDocumentAccess, ClassicDocumentAccessError, ClassicDocumentAccessRejection,
    DocumentAccess, DocumentAccessBackend, DocumentAccessError, DocumentAccessRejection,
    MAX_XREF_STREAM_SECTION_DECODED_BYTES, ResolvedObjectPosition, ResolvedStructuralObject,
    inspect_classic_document_access, inspect_document_access,
};
pub use document_page_content_extents::{
    DocumentPageContentExtentInspection, DocumentPageContentExtentResult,
    DocumentPageContentExtentsInspection, DocumentPageContentExtentsInspectionError,
    inspect_document_page_content_extents, inspect_document_page_content_extents_resolved,
    inspect_document_page_content_extents_with_lookup,
};
pub use extgstate_classify::{
    ClassifiedExtGStateResource, ExtGStateAlpha, ExtGStateBlendMode, ExtGStateOverprintMode,
    ExtGStateParamClass, ExtGStateSoftMask, SkippedExtGStateResource,
    SkippedExtGStateResourceReason, classify_extgstate_entry,
};
pub use form_color_space_resources::{
    FormColorSpaceResourcesInspection, inspect_form_color_space_resources,
};
pub use form_extgstate_resources::{
    FormExtGStateResourcesInspection, inspect_form_extgstate_resources,
};
pub use form_transparency_group::{
    FormTransparencyGroupInspection, inspect_form_transparency_group,
};
pub use form_xobject_resources::{FormXObjectResourcesInspection, inspect_form_xobject_resources};
pub use icc_profile::{
    ICC_PROFILE_HEADER_LEN, IccProfileHeaderDescriptor, IccProfileHeaderParse,
    IccProfileInspectionGap, IccProfileStreamInspection, inspect_icc_profile_stream_with_lookup,
    parse_icc_profile_header,
};
pub use image_xobject::{
    ImageColorSpaceMetadata, ImageIntegerMetadata, ImageMaskMetadata, ImageXObjectMetadata,
    inspect_image_xobject_metadata,
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
pub use object_body_references::{
    MAX_OBJECT_BODY_REFERENCES, ObjectBodyReferencesInspection,
    ObjectBodyReferencesInspectionError, ObjectBodyReferencesInspectionRejection,
    ObjectBodyReferencesTruncation, SkippedObjectBodyReference, inspect_object_body_references,
    inspect_object_body_references_resolved, scan_indirect_references_in_span,
};
pub use object_consumer_index::{
    MAX_OBJECT_CONSUMER_CACHE_BYTES, MAX_OBJECT_CONSUMER_EXPANDED_NODES,
    MAX_OBJECT_CONSUMER_RECORDED_PAIRS, MAX_OBJECT_CONSUMER_TRAVERSAL_DEPTH,
    MAX_OBJECT_CONSUMER_VISITED_NODES, ObjectConsumerIndexInspection, ObjectConsumerIndexLimit,
    ObjectConsumerIndexTruncation, ObjectConsumerReferrer, ObjectConsumerUnresolvedEdge,
    ObjectConsumersEntry, ObjectStreamCacheReport, SkippedObjectConsumerScan,
    inspect_object_consumer_index,
};
pub use object_dictionary::{
    CompressedObjectDictionaryInspection, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionError, IndirectObjectDictionaryInspectionRejection,
    ResolvedObjectDictionaryInspection, ResolvedObjectDictionaryInspectionError,
    ResolvedObjectDictionaryInspectionRejection, inspect_indirect_object_dictionary,
    inspect_object_dictionary,
};
pub use object_header::{
    IndirectObjectHeaderByteRange, IndirectObjectHeaderInspection,
    IndirectObjectHeaderInspectionError, IndirectObjectHeaderInspectionRejection,
    inspect_indirect_object_header,
};
pub use object_lookup::{ObjectLookup, ObjectLookupLocation, locate_xref_object};
pub use object_resolver::{
    ObjectResolutionError, ObjectResolutionRejection, ResolvedObject, ResolvedObjectData,
    resolve_classic_xref_object_offset, resolve_object, resolve_xref_object_offset,
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
pub use object_stream_objects::{
    ExtractedObjectStreamMember, ObjectStreamMemberExtractionError,
    ObjectStreamMemberExtractionRejection, extract_object_stream_member,
};
pub use output_intents::{
    DestOutputProfileFact, OutputIntentArrayEntryKind, OutputIntentsInspection,
    OutputIntentsInspectionError, PdfOutputIntentFact, PdfOutputIntentSubtype, SkippedOutputIntent,
    SkippedOutputIntentReason, inspect_catalog_output_intents, inspect_document_output_intents,
};
pub use page_boxes::{
    DocumentPageBoxesInspection, PageBoxInspectionError, PageBoxKind, PageBoxSource,
    PageBoxesInspection, PageRectangle, ResolvedPageBox, SkippedPageBox, SkippedPageBoxReason,
    inspect_document_page_boxes,
};
pub use page_color_space_resources::{
    ClassifiedColorSpaceDefinition, ClassifiedColorSpaceResource, ColorSpaceFamily,
    DocumentPageColorSpaceResourcesInspection, DocumentPageColorSpaceResourcesInspectionError,
    IndexedLookupDescriptor, PageColorSpaceResourcesInspection, SkippedColorSpaceResource,
    SkippedColorSpaceResourceReason, inspect_document_page_color_space_resources,
    inspect_document_page_color_space_resources_with_lookup,
};
pub use page_content_extents::{
    PageContentExtentInspection, PageContentExtentsInspection, inspect_page_content_extents,
    inspect_page_content_extents_with_lookup,
};
pub use page_content_targets::{
    PageContentTargetInspection, PageContentTargetsInspection, SkippedPageContentTargetReason,
    inspect_page_content_targets, inspect_page_content_targets_with_lookup,
};
pub use page_contents::{
    PageContentReference, PageContentsInspection, PageContentsInspectionError,
    PageContentsInspectionRejection, PageContentsValueShape, SkippedPageContentEntry,
    SkippedPageContentEntryKind, inspect_page_contents,
};
pub use page_contents_resolved::{
    ResolvedPageContents, ResolvedPageContentsError, inspect_page_contents_resolved,
    page_contents_inspection_from_resolved,
};
pub use page_extgstate_resources::{
    DocumentPageExtGStateResourcesInspection, DocumentPageExtGStateResourcesInspectionError,
    PageExtGStateResourcesInspection, inspect_document_page_extgstate_resources,
    inspect_document_page_extgstate_resources_with_lookup,
};
pub use page_transparency_group::{
    DocumentPageTransparencyGroupsInspection, DocumentPageTransparencyGroupsInspectionError,
    PageTransparencyGroupInspection, inspect_document_page_transparency_groups,
    inspect_document_page_transparency_groups_with_lookup,
};
pub use page_tree_kid_targets::{
    PageTreeKidTargetInspection, PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError,
    PageTreeKidTargetsInspectionRejection, inspect_page_tree_kid_targets,
    inspect_page_tree_kid_targets_resolved, inspect_page_tree_kid_targets_with_lookup,
};
pub use page_tree_kids::{
    PageTreeKidReference, PageTreeKidsInspection, PageTreeKidsInspectionError,
    PageTreeKidsInspectionRejection, SkippedPageTreeKid, SkippedPageTreeKidKind,
    inspect_page_tree_kids,
};
pub use page_tree_leaves::{
    MAX_PAGE_TREE_DEPTH, MAX_VISITED_PAGE_TREE_NODES, PageTreeLeaf, PageTreeLeavesInspection,
    PageTreeLeavesInspectionError, PageTreeLeavesTruncation, SkippedPageTreeLeafEntry,
    SkippedPageTreeLeafReason, inspect_page_tree_leaves, inspect_page_tree_leaves_resolved,
    inspect_page_tree_leaves_with_lookup,
};
pub use page_tree_node::{
    PageTreeNodeInspection, PageTreeNodeInspectionError, PageTreeNodeInspectionRejection,
    inspect_page_tree_node, inspect_page_tree_node_resolved,
};
pub use page_tree_node_type::{
    PageTreeNodeType, PageTreeNodeTypeInspection, PageTreeNodeTypeInspectionError,
    PageTreeNodeTypeInspectionRejection, inspect_page_tree_node_type,
    inspect_page_tree_node_type_resolved,
};
pub use page_tree_reference::{
    PageTreeReferenceTargetInspection, PageTreeReferenceTargetInspectionError,
    PageTreeReferenceTargetInspectionRejection, inspect_page_tree_reference_target,
    inspect_page_tree_reference_target_resolved, inspect_page_tree_reference_target_with_lookup,
};
pub use page_xobject_resource_targets::PageXObjectResourceTarget;
pub use page_xobject_resources::{
    DocumentPageXObjectResourcesInspection, DocumentPageXObjectResourcesInspectionError,
    PageXObjectResourcesInspection, PdfName, SkippedPageXObjectResource,
    SkippedPageXObjectResourceReason, inspect_document_page_xobject_resources,
    inspect_document_page_xobject_resources_with_lookup,
};
pub use source::{
    PDF_HEADER_SCAN_LIMIT, PdfHeader, PdfSourceDiagnostic, PdfSourceInspection,
    PdfSourceInspectionError, PdfSourceRejection, PdfStartXref, PdfStartXrefIssue, PdfVersion,
    PdfXrefSectionIssue, STARTXREF_SCAN_LIMIT, XREF_SECTION_SCAN_LIMIT, XrefSection,
    inspect_pdf_source,
};
pub use stream_decode::{
    FlateDecodeParameters, FlateDecodeStreamError, FlateDecodeStreamRejection, decode_flate_stream,
};
pub use stream_encode::{
    FLATE_ENCODE_LEVEL, FlateEncodeStreamError, FlateEncodeStreamRejection, encode_flate_stream,
};
pub use trailer::{
    ClassicXrefTrailerDictionaryInspection, ClassicXrefTrailerDictionaryInspectionError,
    ClassicXrefTrailerDictionaryInspectionRejection, inspect_classic_xref_trailer_dictionary,
};
pub use trailer_prev::{
    ClassicXrefTrailerPrevInspection, ClassicXrefTrailerPrevInspectionError,
    ClassicXrefTrailerPrevInspectionRejection, inspect_classic_xref_trailer_prev,
};
pub use trailer_root::{
    ClassicXrefTrailerRootInspection, ClassicXrefTrailerRootInspectionError,
    ClassicXrefTrailerRootInspectionRejection, inspect_classic_xref_trailer_root,
};
pub use transparency_group_classify::{
    ClassifiedTransparencyGroup, SkippedTransparencyGroup, SkippedTransparencyGroupReason,
    TransparencyGroupColorSpace, TransparencyGroupParamClass, classify_transparency_group_entry,
};
pub use xref_chain::{
    MAX_XREF_STREAM_CHAIN_ENTRIES, MAX_XREF_STREAM_CHAIN_SECTIONS, XrefStreamChain,
    XrefStreamChainError, XrefStreamChainRejection, build_xref_stream_chain,
};
pub use xref_resolve::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefObjectLocation, resolve_classic_xref_object,
};
pub use xref_stream::{
    XrefStreamDictionaryInspection, XrefStreamDictionaryInspectionError,
    XrefStreamDictionaryInspectionRejection, XrefStreamSubsection, inspect_xref_stream_dictionary,
};
pub use xref_stream_entries::{
    XrefStreamEntriesError, XrefStreamEntriesRejection, XrefStreamEntry, XrefStreamEntryRecord,
    parse_xref_stream_entries,
};
pub use xref_stream_map::{
    XrefStreamSection, XrefStreamSectionError, XrefStreamSectionRejection,
    decode_xref_stream_section,
};
pub use xref_stream_trailer::{
    XrefStreamTrailerInspection, XrefStreamTrailerInspectionError,
    XrefStreamTrailerInspectionRejection, inspect_xref_stream_trailer,
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
