use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefTableInspection, PageContentExtentsInspection, PageContentTargetsInspection,
    PageContentsInspection, PageContentsInspectionError, PageTreeLeaf, PageTreeLeavesInspection,
    PageTreeLeavesInspectionError, inspect_page_content_extents, inspect_page_content_targets,
    inspect_page_contents, inspect_page_tree_leaves,
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
            DocumentPageContentExtentResult::Inspected { extents, .. } => {
                extents.located_count() == extents.entries.len()
            }
            DocumentPageContentExtentResult::ContentsFailed { .. } => false,
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
/// root.
///
/// The helper first delegates to [`inspect_page_tree_leaves`]. If root leaf
/// enumeration fails, that delegated failure is returned as the only top-level
/// error. For each enumerated [`PageTreeLeaf`] in document order, it delegates
/// to [`inspect_page_contents`], [`inspect_page_content_targets`], and
/// [`inspect_page_content_extents`] in that order. A `/Contents` inspection
/// failure is recorded as a structured per-page result and does not stop later
/// leaves from being processed.
///
/// Skipped leaf-tree diagnostics and truncation markers remain in the delegated
/// [`PageTreeLeavesInspection`] carried by the report. This helper does not
/// reinterpret, promote, or drop them, and it reimplements no page-tree
/// traversal, `/Contents` parsing, xref target resolution, `/Length` parsing, or
/// `endstream` validation.
///
/// It performs no filesystem I/O, stream decoding, stream concatenation,
/// content tokenization, resource inspection, xref stream parsing, object stream
/// parsing, `/Prev` traversal, cache construction, object-map construction, or
/// mutation.
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
    let leaves =
        inspect_page_tree_leaves(input, xref, root_node_object_offset).map_err(|error| {
            DocumentPageContentExtentsInspectionError {
                root_node_byte_offset: root_node_object_offset,
                byte_len: input.len(),
                error,
            }
        })?;

    let pages = leaves
        .leaves
        .iter()
        .copied()
        .enumerate()
        .map(|(ordinal, leaf)| inspect_leaf_page(input, xref, ordinal, leaf))
        .collect();

    Ok(DocumentPageContentExtentsInspection {
        byte_len: input.len(),
        leaves,
        pages,
    })
}

fn inspect_leaf_page(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    ordinal: usize,
    leaf: PageTreeLeaf,
) -> DocumentPageContentExtentInspection {
    let result = match inspect_page_contents(input, leaf.object_byte_offset) {
        Ok(contents) => {
            let targets = inspect_page_content_targets(input, xref, &contents);
            let extents = inspect_page_content_extents(input, xref, &targets);
            DocumentPageContentExtentResult::Inspected {
                contents,
                targets,
                extents,
            }
        }
        Err(error) => DocumentPageContentExtentResult::ContentsFailed { error },
    };

    DocumentPageContentExtentInspection {
        ordinal,
        leaf,
        result,
    }
}
