use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefTableInspection, ObjectLookup, PageTreeKidReference, PageTreeKidsInspection,
    PageTreeKidsInspectionRejection, PageTreeReferenceTargetInspection,
    PageTreeReferenceTargetInspectionError, ResolvedObjectData, inspect_page_tree_kids,
    inspect_page_tree_node_resolved, inspect_page_tree_reference_target_resolved,
    inspect_page_tree_reference_target_with_lookup,
};

/// Resolved and classified direct `/Kids` targets for one page-tree node.
///
/// This report stores only the caller-visible source length, the delegated
/// page-tree-kids report, and one source-ordered result per direct kid
/// reference. It does not retain or copy PDF bytes, object bodies, stream
/// bodies, page dictionaries, page-tree dictionaries, contents streams, decoded
/// streams, or source slices; owned data is limited to delegated reports and the
/// source-ordered child-result vector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeKidTargetsInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Delegated `/Kids` inspection, including direct references and skipped
    /// malformed or unsupported top-level entries.
    pub kids: PageTreeKidsInspection,
    /// Source-ordered per-kid target results, one per direct kid reference.
    pub entries: Vec<PageTreeKidTargetInspection>,
}

impl PageTreeKidTargetsInspection {
    /// Count of child targets that resolved and classified successfully.
    #[must_use]
    pub fn resolved_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry, PageTreeKidTargetInspection::Resolved { .. }))
            .count()
    }
}

/// Resolution and classification result for one direct page-tree kid reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PageTreeKidTargetInspection {
    /// The kid reference resolved and its target `/Type` was classified.
    Resolved {
        /// Original direct `/Kids` reference reported by kid inspection.
        kid: PageTreeKidReference,
        /// Delegated reference-target inspection report.
        target: PageTreeReferenceTargetInspection,
    },
    /// The kid reference failed to resolve or classify.
    Failed {
        /// Original direct `/Kids` reference reported by kid inspection.
        kid: PageTreeKidReference,
        /// Delegated reference-target inspection failure, preserved verbatim.
        error: PageTreeReferenceTargetInspectionError,
    },
}

/// Error returned when page-tree kid targets cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeKidTargetsInspectionError {
    /// Caller-supplied byte offset where page-tree-node inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the resolved node object header begins, when it was
    /// located.
    pub node_header_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: PageTreeKidTargetsInspectionRejection,
}

/// Structured page-tree kid target inspection rejection reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageTreeKidTargetsInspectionRejection {
    /// Delegated page-tree kid inspection failed.
    PageTreeKids {
        /// Underlying page-tree-kids rejection reason.
        kids_reason: PageTreeKidsInspectionRejection,
    },
}

/// Resolve and classify every direct `/Kids` reference of one page-tree node
/// through a classic xref table.
///
/// This is a thin classic wrapper over
/// [`inspect_page_tree_kid_targets_with_lookup`] via [`ObjectLookup::ClassicXref`],
/// so its report, per-child resolution, and error variants stay byte-identical
/// to the pre-`_with_lookup` behavior.
///
/// # Errors
///
/// Returns [`PageTreeKidTargetsInspectionError`] only when delegated
/// [`inspect_page_tree_kids`] fails. Malformed or unsupported top-level `/Kids`
/// entries remain in [`PageTreeKidsInspection::skipped`], while per-child target
/// failures are reported as [`PageTreeKidTargetInspection::Failed`].
pub fn inspect_page_tree_kid_targets(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    node_object_offset: usize,
) -> Result<PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError> {
    inspect_page_tree_kid_targets_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        node_object_offset,
    )
}

/// Resolve and classify every direct `/Kids` reference of one page-tree node
/// through any [`ObjectLookup`] backend.
///
/// The helper first delegates to [`inspect_page_tree_kids`] for bounded `/Kids`
/// array inspection, preserving its direct-reference order and skipped-entry
/// diagnostics unchanged. It then walks the delegated `kids.kids` in source
/// order and delegates each [`PageTreeKidReference::reference`] to
/// [`inspect_page_tree_reference_target_with_lookup`]. Per-child target failures
/// are reported in place and do not abort later kids.
///
/// It does not recursively traverse the page tree, enumerate leaf pages,
/// validate `/Count`, inspect page contents/resources/boxes/annotations, parse
/// `/Parent` links, follow `/Prev`, extract object streams, build caches or
/// indexes, or mutate source bytes.
///
/// # Errors
///
/// Returns [`PageTreeKidTargetsInspectionError`] only when delegated
/// [`inspect_page_tree_kids`] fails. Malformed or unsupported top-level `/Kids`
/// entries remain in [`PageTreeKidsInspection::skipped`], while per-child target
/// failures are reported as [`PageTreeKidTargetInspection::Failed`].
pub fn inspect_page_tree_kid_targets_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    node_object_offset: usize,
) -> Result<PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError> {
    let kids = inspect_page_tree_kids(input, node_object_offset).map_err(|error| {
        PageTreeKidTargetsInspectionError {
            byte_offset: node_object_offset,
            byte_len: input.len(),
            node_header_byte_offset: error.node_header_byte_offset,
            error_byte_offset: error.error_byte_offset,
            reason: PageTreeKidTargetsInspectionRejection::PageTreeKids {
                kids_reason: error.reason,
            },
        }
    })?;

    let entries = kids
        .kids
        .iter()
        .copied()
        .map(|kid| inspect_kid_target(input, lookup, kid))
        .collect();

    Ok(PageTreeKidTargetsInspection {
        byte_len: input.len(),
        kids,
        entries,
    })
}

/// Resolve and classify direct `/Kids` references from body-aware page-tree
/// node data.
///
/// # Errors
///
/// Returns [`PageTreeKidTargetsInspectionError`] only when the resolved node's
/// own `/Kids` inspection cannot be completed. Per-child target failures remain
/// non-fatal entries.
pub fn inspect_page_tree_kid_targets_resolved(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    resolved: &ResolvedObjectData,
    max_decoded_object_stream_bytes: usize,
) -> Result<PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError> {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => {
            let kids =
                inspect_page_tree_kids(input, resolved.object_byte_offset).map_err(|error| {
                    PageTreeKidTargetsInspectionError {
                        byte_offset: resolved.object_byte_offset,
                        byte_len: input.len(),
                        node_header_byte_offset: error.node_header_byte_offset,
                        error_byte_offset: error.error_byte_offset,
                        reason: PageTreeKidTargetsInspectionRejection::PageTreeKids {
                            kids_reason: error.reason,
                        },
                    }
                })?;

            let entries = kids
                .kids
                .iter()
                .copied()
                .map(|kid| {
                    inspect_kid_target_resolved(input, lookup, kid, max_decoded_object_stream_bytes)
                })
                .collect();

            Ok(PageTreeKidTargetsInspection {
                byte_len: input.len(),
                kids,
                entries,
            })
        }
        ResolvedObjectData::Compressed {
            decoded_object_stream,
            object_body_span,
            ..
        } => {
            let node = inspect_page_tree_node_resolved(input, resolved).map_err(|error| {
                PageTreeKidTargetsInspectionError {
                    byte_offset: 0,
                    byte_len: input.len(),
                    node_header_byte_offset: error.node_header_byte_offset,
                    error_byte_offset: error.error_byte_offset,
                    reason: PageTreeKidTargetsInspectionRejection::PageTreeKids {
                        kids_reason: PageTreeKidsInspectionRejection::PageTreeNode {
                            node_reason: error.reason,
                        },
                    },
                }
            })?;
            let body = decoded_object_stream
                .get(object_body_span.start..object_body_span.end)
                .unwrap_or(&[]);
            let kids = crate::page_tree_kids::inspect_page_tree_kids_from_node(body, node);
            let entries = kids
                .kids
                .iter()
                .copied()
                .map(|kid| {
                    inspect_kid_target_resolved(input, lookup, kid, max_decoded_object_stream_bytes)
                })
                .collect();

            Ok(PageTreeKidTargetsInspection {
                byte_len: input.len(),
                kids,
                entries,
            })
        }
    }
}

fn inspect_kid_target(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    kid: PageTreeKidReference,
) -> PageTreeKidTargetInspection {
    match inspect_page_tree_reference_target_with_lookup(input, lookup, kid.reference) {
        Ok(target) => PageTreeKidTargetInspection::Resolved { kid, target },
        Err(error) => PageTreeKidTargetInspection::Failed { kid, error },
    }
}

fn inspect_kid_target_resolved(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    kid: PageTreeKidReference,
    max_decoded_object_stream_bytes: usize,
) -> PageTreeKidTargetInspection {
    match inspect_page_tree_reference_target_resolved(
        input,
        lookup,
        kid.reference,
        max_decoded_object_stream_bytes,
    ) {
        Ok(target) => PageTreeKidTargetInspection::Resolved { kid, target },
        Err(error) => PageTreeKidTargetInspection::Failed { kid, error },
    }
}
