//! Page-anchored `/Resources /XObject` binding/locality witness, read-only.
//!
//! For every enumerated leaf page and every entry of the page's EFFECTIVE
//! `/Resources /XObject` subdictionary, this inspector proves exactly WHICH
//! indirect object the entry name binds, and characterizes the locality of
//! every node on the binding path — page → `/Resources` → `/XObject`
//! subdictionary → entry — as leaf-direct, indirect-shared, or inherited from
//! an ancestor `/Pages` node (ISO 32000-1 Table 30 whole-value replacement,
//! §7.7.3.4). Each witness carries a conservative page-local verdict that
//! additionally passes the borrowed document consumer-index exclusivity veto.
//!
//! A witness proves RESOLUTION and LOCALITY only. It never authorizes
//! mutation, plans clones, retargets consumers, or touches bytes.
//!
//! # Name discipline
//!
//! Binding uses decoded-PDF-name matching (ISO 32000-1 §7.3.5): container
//! keys are matched semantically, decoded-name collisions poison EVERY
//! colliding entry, and malformed names refuse fail-closed. Raw spellings are
//! retained for reporting. There is no first- or last-wins recovery.
//!
//! # No obsolete fallback
//!
//! The witness binds the OUTER page `Do` namespace only. Names that exist
//! only inside a Form `XObject`'s own `/Resources` are never resolved here, and
//! a missing effective container is a classified refusal — never resolved
//! through the obsolete content-stream resource fallback (ISO 32000-1 §7.8.3).
//!
//! # Retention
//!
//! Reports retain identities, classifications, and byte ranges only: raw name
//! bytes, [`IndirectRef`] identities, offsets, and structured refusals. No
//! dictionary bodies, resource dictionaries, stream bytes, or decoded data
//! are retained, and no stream payload is ever decoded.

use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefTableInspection, DictionaryEntryByteRange, DictionaryEntryInspectionError,
    DictionaryValueKind, IndirectObjectDictionaryInspectionError, IndirectObjectOwnership,
    IndirectRef, IndirectReferenceInspectionRejection, ObjectConsumerIndexInspection, ObjectLookup,
    ObjectLookupLocation, PageTreeKidTargetsInspectionError, PageTreeLeavesTruncation, PdfName,
    SkippedPageTreeLeafEntry,
};

mod walk;

/// Document-wide page `/Resources /XObject` binding witness report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageXObjectBindingsInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Document-ordered per-page binding reports.
    pub pages: Vec<PageXObjectBindingsInspection>,
    /// Ordered page-tree traversal skips for children that were not leaf pages.
    pub page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    /// Number of `/Pages` nodes expanded during the walk.
    pub visited_node_count: usize,
    /// First traversal bound that stopped a descent, when any.
    pub truncated: Option<PageTreeLeavesTruncation>,
}

impl DocumentPageXObjectBindingsInspection {
    /// Count of inspected leaf pages.
    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// Per-page binding witnesses and structured refusals.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageXObjectBindingsInspection {
    /// Zero-based document-order page ordinal.
    pub ordinal: usize,
    /// Indirect reference of the leaf `/Page`.
    pub page_reference: IndirectRef,
    /// Resolved page object byte offset.
    pub page_object_byte_offset: usize,
    /// Binding witnesses sorted by raw entry name.
    pub witnesses: Vec<PageXObjectBindingWitness>,
    /// Fail-closed refusals: container-level first, then entry-level in
    /// source order.
    pub refused: Vec<RefusedPageXObjectBinding>,
}

/// One proven name-to-object binding of a page's effective `/XObject`
/// subdictionary.
///
/// The witness is self-contained for a future retarget point: it carries the
/// full binding-path provenance/locality, the exact entry byte ranges, the
/// corroborated target identity, the conservative target ownership under the
/// public decision contract, and the composed page-local verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageXObjectBindingWitness {
    /// Raw entry name bytes without the leading slash. Binding compared
    /// decoded values; the raw spelling is retained for reporting.
    pub name: PdfName,
    /// Byte range of the entry key inside the effective `/XObject`
    /// subdictionary.
    pub key_range: DictionaryEntryByteRange,
    /// Byte range of the entry value (the indirect-reference tokens).
    pub value_range: DictionaryEntryByteRange,
    /// Provenance and locality of every node on the binding path.
    pub path: XObjectBindingPath,
    /// Exact bound target reference, generation included.
    pub target: IndirectRef,
    /// Reached target object byte offset, corroborated against the object
    /// header identity at that offset.
    pub target_object_byte_offset: usize,
    /// Exact `/Subtype` classification of the bound target dictionary.
    pub subtype: XObjectBindingSubtype,
    /// Conservative target ownership under the public decision contract,
    /// vetoed by the borrowed consumer index. Independent of path locality.
    pub target_ownership: IndirectObjectOwnership,
    /// Composed conservative page-local verdict.
    pub verdict: PageXObjectBindingVerdict,
}

/// Provenance and locality of the binding-path container nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XObjectBindingPath {
    /// Defining page-tree node of the effective `/Resources` value.
    pub resources_source: BindingResourcesSource,
    /// Direct/indirect classification of the `/Resources` container.
    pub resources_locality: BindingContainerLocality,
    /// Byte range of the `/XObject` key inside the effective resources.
    pub xobject_key_range: DictionaryEntryByteRange,
    /// Byte range of the `/XObject` value inside the effective resources.
    pub xobject_value_range: DictionaryEntryByteRange,
    /// Direct/indirect classification of the `/XObject` subdictionary.
    pub xobject_locality: BindingContainerLocality,
}

/// Defining page-tree node of the effective `/Resources` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum BindingResourcesSource {
    /// The leaf `/Page` dictionary supplied the effective `/Resources`.
    Direct {
        /// Leaf page object that supplied the value.
        target: IndirectRef,
        /// Byte range of the `/Resources` key.
        key_range: DictionaryEntryByteRange,
        /// Byte range of the `/Resources` value.
        value_range: DictionaryEntryByteRange,
    },
    /// An ancestor `/Pages` dictionary supplied the inherited value
    /// (ISO 32000-1 Table 30 whole-value replacement).
    Inherited {
        /// Ancestor page-tree node that supplied the value.
        ancestor: IndirectRef,
        /// Byte range of the `/Resources` key.
        key_range: DictionaryEntryByteRange,
        /// Byte range of the `/Resources` value.
        value_range: DictionaryEntryByteRange,
    },
}

/// Direct/indirect classification of one binding container node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "locality", rename_all = "snake_case")]
pub enum BindingContainerLocality {
    /// The container value is a direct dictionary inside its defining node.
    DirectDictionary,
    /// The container value is an indirect reference resolved to an
    /// uncompressed dictionary whose object header corroborated the identity.
    IndirectResolved {
        /// Resolved container object reference.
        reference: IndirectRef,
        /// Corroborated container object byte offset.
        object_byte_offset: usize,
    },
}

/// Binding container whose resolution a refusal concerns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingContainer {
    /// The effective `/Resources` dictionary value.
    Resources,
    /// The effective `/XObject` subdictionary value.
    XObjectDictionary,
}

/// Exact `/Subtype` classification of one bound target dictionary.
///
/// A reached target with a wrong, absent, or malformed `/Subtype` stays a
/// WITNESS with an explicit fail-closed class: wrong-subtype is deliberately
/// distinct from the unresolved refusal vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum XObjectBindingSubtype {
    /// Decoded `/Subtype` name `Form`.
    Form,
    /// Decoded `/Subtype` name `Image`.
    Image,
    /// Another direct name value, raw bytes retained without decoding.
    OtherName {
        /// Raw name bytes without the leading slash.
        name: PdfName,
    },
    /// The `/Subtype` key is absent.
    Missing,
    /// The `/Subtype` key occurred more than once.
    Duplicate {
        /// First `/Subtype` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Subtype` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Subtype` value was not a direct name.
    NonName {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// Composed conservative page-local verdict for one binding witness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum PageXObjectBindingVerdict {
    /// Every binding-path node is leaf-direct AND the complete consumer index
    /// proves this page is the only typed consumer of the bound target.
    ProvenPageLocal,
    /// Fail-closed default carrying the first blocking check in deterministic
    /// order.
    Unproven {
        /// First blocking check.
        reason: PageXObjectBindingUnprovenReason,
    },
}

/// First blocking check that kept a witness verdict unproven.
///
/// Checks run in a fixed deterministic order: inherited resources, indirect
/// resources, indirect `/XObject` subdictionary, consumer-index
/// incompleteness, target exclusivity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageXObjectBindingUnprovenReason {
    /// The effective `/Resources` was inherited from an ancestor node, so the
    /// binding path is shared with sibling pages by construction.
    ResourcesInherited {
        /// Ancestor page-tree node that supplied the value.
        ancestor: IndirectRef,
    },
    /// The effective `/Resources` container is a separate indirect object.
    ResourcesIndirect {
        /// Resolved container object reference.
        reference: IndirectRef,
    },
    /// The effective `/XObject` subdictionary is a separate indirect object.
    XObjectDictionaryIndirect {
        /// Resolved container object reference.
        reference: IndirectRef,
    },
    /// The borrowed consumer index carries truncations, unresolved edges, or
    /// scan skips, so exclusivity cannot be proven.
    ConsumerIndexIncomplete,
    /// The consumer index does not show exactly this page as the single typed
    /// consumer of the bound target.
    TargetConsumersNotExclusive {
        /// Deduplicated typed referrers recorded for the target; `0` means
        /// the target was not indexed at all.
        referrer_count: usize,
    },
}

/// One fail-closed binding refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedPageXObjectBinding {
    /// Resolved leaf page object byte offset.
    pub page_object_byte_offset: usize,
    /// Raw entry name bytes when the refusal concerns one `/XObject` entry.
    pub resource_name: Option<PdfName>,
    /// Structured refusal classification.
    pub reason: PageXObjectBindingRefusal,
}

/// Structured reason a binding could not be witnessed.
///
/// This vocabulary is deliberately DISTINCT from the legacy
/// [`crate::SkippedPageXObjectResourceReason`] skip semantics: indirect
/// containers resolve here instead of skipping, compressed containers refuse
/// with their own classification, and offsets are never fabricated for
/// compressed objects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageXObjectBindingRefusal {
    /// The leaf page dictionary itself could not be scanned; no binding on
    /// this page can be proven, including inherited ones.
    PageDictionaryFailed {
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// No `/Resources` key exists on the leaf or any ancestor: Table 30
    /// inheritance is exhausted and the absence is classified, never resolved
    /// through the obsolete content-stream fallback.
    MissingResources,
    /// The effective resources carry no `/XObject` key (or carry it with a
    /// `null` value, equivalent to an absent entry per ISO 32000-1 §7.3.9):
    /// classified absence, never resolved through the obsolete
    /// content-stream fallback.
    MissingXObject {
        /// Dictionary owning the effective resources entries: the defining
        /// page-tree node for a direct `/Resources`, the resolved container
        /// object when `/Resources` is indirect.
        defining_node: IndirectRef,
    },
    /// A container key occurred more than once by decoded-name comparison.
    DuplicateContainerKey {
        /// Container whose key collided.
        container: BindingContainer,
        /// Dictionary-owning node whose entries carried the collision.
        defining_node: IndirectRef,
        /// First matching key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// A container value was neither a direct dictionary nor an indirect
    /// reference.
    UnsupportedContainerValue {
        /// Container whose value was unsupported.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the value.
        defining_node: IndirectRef,
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// A container value was shaped like a reference but malformed.
    MalformedContainerReference {
        /// Container whose reference was malformed.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the reference.
        defining_node: IndirectRef,
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// A container reference did not resolve to an in-use uncompressed
    /// object.
    UnresolvedContainer {
        /// Container whose reference did not resolve.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the reference.
        defining_node: IndirectRef,
        /// Requested container reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number.
        location: ObjectLookupLocation,
    },
    /// A container reference resolved with a mismatched generation.
    ContainerGenerationMismatch {
        /// Container whose reference mismatched.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the reference.
        defining_node: IndirectRef,
        /// Requested container reference.
        reference: IndirectRef,
        /// Generation number from the matching in-use xref entry.
        xref_generation: u16,
    },
    /// A container reference resolved to a compressed object-stream member.
    /// Compressed containers have no source byte offsets; refusing is the
    /// only classification that never fabricates one.
    CompressedContainer {
        /// Container whose target is compressed.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the reference.
        defining_node: IndirectRef,
        /// Requested container reference.
        reference: IndirectRef,
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Index of the member inside the object stream.
        index_within_object_stream: usize,
    },
    /// The object header at the reached container offset did not corroborate
    /// the requested identity. The header is checked BEFORE body validation,
    /// so a mismatched offset classifies here even when the reached body is
    /// non-dictionary or malformed.
    ContainerIdentityMismatch {
        /// Container whose identity failed corroboration.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the reference.
        defining_node: IndirectRef,
        /// Requested container reference.
        reference: IndirectRef,
        /// Reached object byte offset.
        object_byte_offset: usize,
        /// Identity parsed from the object header at the reached offset.
        header_reference: IndirectRef,
    },
    /// A container reference resolved to a stream object. `/Resources` and
    /// the `/XObject` subdictionary require dictionary objects (ISO 32000-1
    /// §7.8.3), so the dictionary portion of a stream is never admitted as a
    /// binding container.
    StreamContainer {
        /// Container whose target is a stream object.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the reference.
        defining_node: IndirectRef,
        /// Requested container reference.
        reference: IndirectRef,
        /// Reached object byte offset.
        object_byte_offset: usize,
    },
    /// A resolved indirect container was not a scannable dictionary object.
    IndirectContainerDictionaryFailed {
        /// Container whose dictionary failed to scan.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the reference.
        defining_node: IndirectRef,
        /// Requested container reference.
        reference: IndirectRef,
        /// Reached object byte offset.
        object_byte_offset: usize,
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// A direct container dictionary could not be scanned.
    DirectContainerDictionaryFailed {
        /// Container whose dictionary failed to scan.
        container: BindingContainer,
        /// Dictionary-owning node whose entry carried the value.
        defining_node: IndirectRef,
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// An entry name failed PDF-name decoding (ISO 32000-1 §7.3.5).
    MalformedEntryName {
        /// Byte range of the malformed key.
        key_range: DictionaryEntryByteRange,
    },
    /// Two or more entry names collided by decoded-name comparison; EVERY
    /// colliding entry is poisoned with the full colliding key-range set.
    EntryNameCollision {
        /// Key ranges of every entry in the colliding group, source order.
        colliding_key_ranges: Vec<DictionaryEntryByteRange>,
    },
    /// An entry value was not an indirect reference.
    NonReferenceEntry {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// An entry value was shaped like a reference but malformed.
    MalformedEntryReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// An entry target reference did not resolve to an in-use uncompressed
    /// object.
    UnresolvedEntryTarget {
        /// Requested target reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number.
        location: ObjectLookupLocation,
    },
    /// An entry target resolved with a mismatched generation.
    EntryTargetGenerationMismatch {
        /// Requested target reference.
        reference: IndirectRef,
        /// Generation number from the matching in-use xref entry.
        xref_generation: u16,
    },
    /// An entry target resolved to a compressed object-stream member; no
    /// source offset exists and none is fabricated.
    CompressedEntryTarget {
        /// Requested target reference.
        reference: IndirectRef,
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Index of the member inside the object stream.
        index_within_object_stream: usize,
    },
    /// The object header at the reached target offset did not corroborate the
    /// requested identity. The header is checked BEFORE body validation, so a
    /// mismatched offset classifies here even when the reached body is
    /// non-dictionary or malformed.
    EntryTargetIdentityMismatch {
        /// Requested target reference.
        reference: IndirectRef,
        /// Reached object byte offset.
        object_byte_offset: usize,
        /// Identity parsed from the object header at the reached offset.
        header_reference: IndirectRef,
    },
    /// The reached entry target was not a dictionary-bodied object.
    EntryTargetDictionaryFailed {
        /// Requested target reference.
        reference: IndirectRef,
        /// Reached object byte offset.
        object_byte_offset: usize,
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
}

/// Error returned when document binding inspection cannot begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageXObjectBindingsInspectionError {
    /// Caller-supplied root `/Pages` object offset.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root-node expansion failure.
    pub error: PageTreeKidTargetsInspectionError,
}

/// Inspect page `/Resources /XObject` binding witnesses through a classic
/// xref table.
///
/// This is a thin wrapper over
/// [`inspect_document_page_xobject_bindings_with_lookup`] via
/// [`ObjectLookup::ClassicXref`].
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-page
/// failures are fail-closed structured refusals in a successful report.
pub fn inspect_document_page_xobject_bindings(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
    consumers: &ObjectConsumerIndexInspection,
) -> Result<DocumentPageXObjectBindingsInspection, DocumentPageXObjectBindingsInspectionError> {
    inspect_document_page_xobject_bindings_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        root_node_object_offset,
        consumers,
    )
}

/// Inspect page `/Resources /XObject` binding witnesses through any object
/// lookup backend.
///
/// The walk descends the page tree exactly once, root-down in document order,
/// bounded by [`crate::MAX_PAGE_TREE_DEPTH`],
/// [`crate::MAX_VISITED_PAGE_TREE_NODES`], and a visited-set cycle guard. The
/// effective `/Resources` value follows ISO 32000-1 Table 30 whole-value
/// nearest-ancestor replacement, never per-key merge; a `null` value is
/// equivalent to an absent entry (§7.3.9), so it never replaces an ancestor
/// value. Both the `/Resources` value and the `/XObject` subdictionary may
/// independently be direct dictionaries or indirect references; an
/// uncompressed indirect container resolves with its identity recorded and
/// its object header corroborated BEFORE body validation, while compressed,
/// unresolvable, or stream-object containers refuse with distinct
/// classifications.
///
/// `consumers` is a borrowed, already-computed document consumer index used
/// ONLY as a conservative exclusivity veto: lookups are binary searches over
/// its sorted entries, its completeness is read from its own fact vectors,
/// and any incompleteness yields unproven verdicts fail-closed. The veto
/// never re-traverses the document and never builds a new index.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-child
/// page-tree failures and per-page binding failures are structured facts in a
/// successful report.
pub fn inspect_document_page_xobject_bindings_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
    consumers: &ObjectConsumerIndexInspection,
) -> Result<DocumentPageXObjectBindingsInspection, DocumentPageXObjectBindingsInspectionError> {
    walk::run(input, lookup, root_node_object_offset, consumers)
}
