use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefTableInspection, ObjectLookup, PageContentExtentsInspection,
    PageContentTargetsInspection, PageContentsInspection, PageContentsInspectionError,
    PageTreeLeaf, PageTreeLeavesInspection, PageTreeLeavesInspectionError, ResolvedObjectData,
    ResolvedObjectPosition, inspect_page_content_extents_with_lookup,
    inspect_page_content_targets_with_lookup, inspect_page_contents,
    inspect_page_contents_resolved, inspect_page_tree_leaves_resolved,
    inspect_page_tree_leaves_with_lookup, page_contents_inspection_from_resolved, resolve_object,
};

/// Document-order locate-only report for page content-stream data extents.
///
/// This report stores only caller-visible source length, the delegated page-tree
/// leaf enumeration report, and one document-ordered per-page result for each
/// enumerated leaf. It retains or copies no PDF bytes, object bodies, page
/// dictionaries, stream dictionaries, stream bytes, decoded bytes, concatenated
/// content buffers, or source slices; owned data is limited to delegated
/// reports, offsets, ordinals, small enums, and source-ordered result vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageContentExtentsInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Delegated document-order leaf enumeration report.
    pub leaves: PageTreeLeavesInspection,
    /// Document-ordered per-page content extent results.
    pub pages: Vec<DocumentPageContentExtentInspection>,
}

impl DocumentPageContentExtentsInspection {
    /// Count of enumerated leaf pages represented in `pages`.
    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Count of leaf pages whose content path was fully located.
    ///
    /// A page is counted when its `/Contents` inspection succeeded and every
    /// delegated content target has a located extent.
    #[must_use]
    pub fn located_page_count(&self) -> usize {
        self.pages.iter().filter(|page| page.is_located()).count()
    }
}

/// Document-order content extent result for one enumerated leaf page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageContentExtentInspection {
    /// Zero-based document-order ordinal assigned from the leaf vector.
    pub ordinal: usize,
    /// Original page-tree leaf metadata from the delegated leaf report.
    pub leaf: PageTreeLeaf,
    /// Per-page content inspection result.
    pub result: DocumentPageContentExtentResult,
}

impl DocumentPageContentExtentInspection {
    /// Whether this page's content path was fully located.
    #[must_use]
    pub fn is_located(&self) -> bool {
        match &self.result {
            DocumentPageContentExtentResult::Inspected { extents, .. }
            | DocumentPageContentExtentResult::CompressedLeafInspected { extents, .. } => {
                extents.located_count() == extents.entries.len()
            }
            DocumentPageContentExtentResult::ContentsFailed { .. }
            | DocumentPageContentExtentResult::CompressedLeaf { .. } => false,
        }
    }
}

/// Per-page result for the document page-content-extents aggregate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DocumentPageContentExtentResult {
    /// The leaf page's `/Contents` inspection succeeded, followed by delegated
    /// target resolution and content-stream extent location.
    Inspected {
        /// Delegated `/Contents` inspection report.
        contents: PageContentsInspection,
        /// Delegated content target resolution report.
        targets: PageContentTargetsInspection,
        /// Delegated content-stream data extent report.
        extents: PageContentExtentsInspection,
    },
    /// The leaf page's `/Contents` inspection failed; later leaf pages still
    /// continue through the aggregate pipeline.
    ContentsFailed {
        /// Delegated `/Contents` inspection failure.
        error: PageContentsInspectionError,
    },
    /// The leaf `/Page` object is a type-2 compressed object-stream member. Its
    /// `/Contents` cannot be inspected through the offset-only page-content path
    /// (there is no source byte offset to read a `/Contents` value from), so it
    /// is reported honestly as a compressed leaf rather than fabricating an
    /// offset-0 parse failure. Compressed-leaf CONTENT inventory is a follow-up.
    CompressedLeaf {
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Index of this object inside the object stream.
        index_within_object_stream: usize,
    },
    /// The leaf `/Page` object is a type-2 compressed object-stream member whose
    /// `/Contents` was READ from its resolved (decoded) member body, and whose
    /// referenced content-stream objects were resolved to source-valid extents.
    ///
    /// RESOLVED-BODY PROVENANCE BOUNDARY: the leaf dictionary itself lives in
    /// decoded `/ObjStm` bytes with no stable source offset, so this variant NEVER
    /// carries the leaf-dict-side `/Contents` byte spans. Only `targets` and
    /// `extents` are reported, and they carry ONLY the source-valid offsets of the
    /// referenced content-stream objects (ordinary uncompressed objects). A content
    /// target that is itself compressed remains an honest located-skip inside
    /// `extents`. This variant is additive to [`Self::CompressedLeaf`], which is
    /// retained for the un-inspectable case (dict unresolvable, or missing /
    /// duplicate / malformed `/Contents`).
    CompressedLeafInspected {
        /// Delegated content target resolution report for the resolved references.
        targets: PageContentTargetsInspection,
        /// Delegated content-stream data extent report (source-valid offsets).
        extents: PageContentExtentsInspection,
    },
}

/// Error returned when document page content extents cannot be inspected.
///
/// This top-level error is used only when the delegated root page-tree leaf
/// enumeration fails. Per-leaf `/Contents` failures are reported inside the
/// successful aggregate report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageContentExtentsInspectionError {
    /// Caller-supplied byte offset of the root `/Pages` node.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root leaf-enumeration failure.
    pub error: PageTreeLeavesInspectionError,
}

/// Inspect document-ordered page content-stream data extents from a page-tree
/// root through a classic xref inspection.
///
/// This is a thin classic wrapper over
/// [`inspect_document_page_content_extents_with_lookup`]: it delegates through
/// [`ObjectLookup::ClassicXref`] and therefore keeps the leaf enumeration, the
/// per-page `/Contents`/target/extent results, and every error byte-identical to
/// the pre-`_with_lookup` behavior.
///
/// # Errors
///
/// Returns [`DocumentPageContentExtentsInspectionError`] only when delegated
/// root leaf enumeration fails at `root_node_object_offset`.
pub fn inspect_document_page_content_extents(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
) -> Result<DocumentPageContentExtentsInspection, DocumentPageContentExtentsInspectionError> {
    inspect_document_page_content_extents_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        root_node_object_offset,
    )
}

/// Inspect document-ordered page content-stream data extents from a page-tree
/// root through any [`ObjectLookup`] backend.
///
/// The helper first delegates to [`inspect_page_tree_leaves_with_lookup`]. If
/// root leaf enumeration fails, that delegated failure is returned as the only
/// top-level error. For each enumerated [`PageTreeLeaf`] in document order, it
/// delegates to [`inspect_page_contents`],
/// [`inspect_page_content_targets_with_lookup`], and
/// [`inspect_page_content_extents_with_lookup`] in that order, threading the same
/// backend through target resolution and extent location. A `/Contents`
/// inspection failure is recorded as a structured per-page result and does not
/// stop later leaves from being processed.
///
/// Skipped leaf-tree diagnostics and truncation markers remain in the delegated
/// [`PageTreeLeavesInspection`] carried by the report. This helper does not
/// reinterpret, promote, or drop them, and it reimplements no page-tree
/// traversal, `/Contents` parsing, xref target resolution, `/Length` parsing, or
/// `endstream` validation.
///
/// It performs no filesystem I/O, stream decoding, stream concatenation,
/// content tokenization, resource inspection, object stream parsing, `/Prev`
/// traversal, cache construction, object-map construction, or mutation.
///
/// # Errors
///
/// Returns [`DocumentPageContentExtentsInspectionError`] only when delegated
/// root leaf enumeration fails at `root_node_object_offset`.
pub fn inspect_document_page_content_extents_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
) -> Result<DocumentPageContentExtentsInspection, DocumentPageContentExtentsInspectionError> {
    let leaves = inspect_page_tree_leaves_with_lookup(input, lookup, root_node_object_offset)
        .map_err(|error| DocumentPageContentExtentsInspectionError {
            root_node_byte_offset: root_node_object_offset,
            byte_len: input.len(),
            error,
        })?;

    Ok(assemble_document_page_extents(input, leaves, |leaf| {
        content_extent_result_at_offset(input, lookup, leaf.object_byte_offset)
    }))
}

/// Inspect document-ordered page content-stream data extents from a body-aware
/// resolved page-tree root through any [`ObjectLookup`] backend.
///
/// This is the resolved-object-aware sibling of
/// [`inspect_document_page_content_extents_with_lookup`]: it enumerates leaves
/// through [`inspect_page_tree_leaves_resolved`], so a page tree whose
/// INTERMEDIATE `/Pages` nodes are type-2 compressed object-stream members is
/// navigated instead of hard-failing when the offset-only walk reads an
/// indirect-object header at the fabricated offset `0`. Leaf order is preserved
/// exactly (`leaves.leaves.iter().copied().enumerate()`).
///
/// Per enumerated leaf, the result branches on the resolved
/// [`PageTreeLeaf::position`]:
///
/// - an [`ResolvedObjectPosition::Uncompressed`] leaf is inspected through the
///   same offset-only `/Contents`/target/extent path as the legacy bridge, so
///   uncompressed leaves stay byte-identical;
/// - a [`ResolvedObjectPosition::Compressed`] leaf has its body resolved once and
///   its `/Contents` READ from the decoded member body (never offset `0` fed into
///   [`inspect_page_contents`]); the referenced content-stream objects are resolved
///   to source-valid extents and reported as
///   [`DocumentPageContentExtentResult::CompressedLeafInspected`]. When the body
///   cannot be resolved, or `/Contents` is missing / duplicate / malformed /
///   non-reference, the honest [`DocumentPageContentExtentResult::CompressedLeaf`]
///   skip is retained. A `/Contents` target that is itself compressed stays a
///   located-skip inside the reported `extents`.
///
/// The report retains or copies no PDF bytes, object bodies, or decoded
/// object-stream buffers: it carries only offsets, ordinals, small enums, and
/// the delegated per-leaf reports, exactly like the offset-based bridge. The
/// resolved leaf enumeration decodes object streams bounded by
/// `max_decoded_object_stream_bytes`, and each compressed leaf is resolved exactly
/// once (same bound) to read its `/Contents`; the decoded buffer is dropped after
/// the references are extracted, so no `/ObjStm` buffer is retained in the report.
///
/// # Errors
///
/// Returns [`DocumentPageContentExtentsInspectionError`] only when the delegated
/// resolved root leaf enumeration fails.
pub fn inspect_document_page_content_extents_resolved(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    resolved_root: &ResolvedObjectData,
    max_decoded_object_stream_bytes: usize,
) -> Result<DocumentPageContentExtentsInspection, DocumentPageContentExtentsInspectionError> {
    let leaves = inspect_page_tree_leaves_resolved(
        input,
        lookup,
        resolved_root,
        max_decoded_object_stream_bytes,
    )
    .map_err(|error| DocumentPageContentExtentsInspectionError {
        root_node_byte_offset: error.root_node_byte_offset,
        byte_len: input.len(),
        error,
    })?;

    Ok(assemble_document_page_extents(
        input,
        leaves,
        |leaf| match leaf.position {
            ResolvedObjectPosition::Uncompressed {
                object_byte_offset, ..
            } => content_extent_result_at_offset(input, lookup, object_byte_offset),
            ResolvedObjectPosition::Compressed {
                object_stream_number,
                index_within_object_stream,
            } => compressed_leaf_content_result(
                input,
                lookup,
                leaf,
                max_decoded_object_stream_bytes,
                object_stream_number,
                index_within_object_stream,
            ),
        },
    ))
}

/// Resolve one compressed leaf `/Page` object's body, read its `/Contents`
/// references, and locate the referenced content-stream objects' source-valid
/// extents.
///
/// The leaf is resolved exactly once through [`resolve_object`] (bounded by
/// `max_decoded_object_stream_bytes`); its `/Contents` references are read from the
/// resolved body through [`inspect_page_contents_resolved`], then adapted into the
/// shared [`inspect_page_content_targets_with_lookup`] →
/// [`inspect_page_content_extents_with_lookup`] path via
/// [`page_contents_inspection_from_resolved`], whose synthetic references carry
/// provenance-neutral sentinel spans so no member-body offset is ever presented as
/// a source byte range. When the leaf body cannot be resolved, or its `/Contents`
/// is missing / duplicate / malformed / non-reference, the honest
/// [`DocumentPageContentExtentResult::CompressedLeaf`] skip is retained instead of
/// fabricating extents. The decoded object-stream buffer is dropped after the
/// references are extracted; the returned result keeps only offsets, refs, and the
/// small delegated reports.
fn compressed_leaf_content_result(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    leaf: PageTreeLeaf,
    max_decoded_object_stream_bytes: usize,
    object_stream_number: usize,
    index_within_object_stream: usize,
) -> DocumentPageContentExtentResult {
    let compressed_leaf_skip = DocumentPageContentExtentResult::CompressedLeaf {
        object_stream_number,
        index_within_object_stream,
    };

    let Ok(resolved) = resolve_object(
        input,
        lookup,
        leaf.reference,
        max_decoded_object_stream_bytes,
    ) else {
        return compressed_leaf_skip;
    };
    let Ok(resolved_contents) = inspect_page_contents_resolved(input, &resolved) else {
        return compressed_leaf_skip;
    };

    let contents = page_contents_inspection_from_resolved(&resolved_contents);
    let targets = inspect_page_content_targets_with_lookup(input, lookup, &contents);
    let extents = inspect_page_content_extents_with_lookup(input, lookup, &targets);
    DocumentPageContentExtentResult::CompressedLeafInspected { targets, extents }
}

/// Assemble the document-ordered report from an enumerated leaf report, deriving
/// each per-page result through `result_for`.
///
/// Leaf order is preserved exactly (`leaves.leaves.iter().copied().enumerate()`),
/// and the report keeps only offsets, ordinals, small enums, and the delegated
/// per-leaf results — no PDF bytes, object bodies, or decoded object-stream
/// buffers. Both the offset-based and resolved bridges share this assembly; they
/// differ only in how a leaf maps to a [`DocumentPageContentExtentResult`].
fn assemble_document_page_extents(
    input: &[u8],
    leaves: PageTreeLeavesInspection,
    mut result_for: impl FnMut(PageTreeLeaf) -> DocumentPageContentExtentResult,
) -> DocumentPageContentExtentsInspection {
    let pages = leaves
        .leaves
        .iter()
        .copied()
        .enumerate()
        .map(|(ordinal, leaf)| DocumentPageContentExtentInspection {
            ordinal,
            leaf,
            result: result_for(leaf),
        })
        .collect();

    DocumentPageContentExtentsInspection {
        byte_len: input.len(),
        leaves,
        pages,
    }
}

/// Run the shared offset-only `/Contents` → target → extent path for one
/// uncompressed leaf `/Page` object located at `object_byte_offset`.
fn content_extent_result_at_offset(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> DocumentPageContentExtentResult {
    match inspect_page_contents(input, object_byte_offset) {
        Ok(contents) => {
            let targets = inspect_page_content_targets_with_lookup(input, lookup, &contents);
            let extents = inspect_page_content_extents_with_lookup(input, lookup, &targets);
            DocumentPageContentExtentResult::Inspected {
                contents,
                targets,
                extents,
            }
        }
        Err(error) => DocumentPageContentExtentResult::ContentsFailed { error },
    }
}
