use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::transparency_group_classify::{
    ClassifiedTransparencyGroup, SkippedTransparencyGroup, SkippedTransparencyGroupReason,
    classify_transparency_group_entry, skipped_group,
};
use crate::{
    ClassicXrefTableInspection, IndirectRef, ObjectLookup, PageTreeKidTargetInspection,
    PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError, PageTreeLeavesTruncation,
    PageTreeNodeType, SkippedPageTreeLeafEntry, SkippedPageTreeLeafReason,
    inspect_indirect_object_dictionary,
};

/// Per-document page `/Group` classification report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageTransparencyGroupsInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Document-ordered per-page `/Group` reports.
    pub pages: Vec<PageTransparencyGroupInspection>,
    /// Ordered page-tree traversal skips for children that were not leaf pages.
    pub page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    /// Number of `/Pages` nodes expanded during the walk.
    pub visited_node_count: usize,
    /// First traversal bound that stopped a descent, when any.
    pub truncated: Option<PageTreeLeavesTruncation>,
}

impl DocumentPageTransparencyGroupsInspection {
    /// Count of inspected leaf pages.
    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// Per-page top-level `/Group` classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTransparencyGroupInspection {
    /// Zero-based document-order page ordinal.
    pub ordinal: usize,
    /// Indirect reference of the leaf `/Page`.
    pub page_reference: IndirectRef,
    /// Resolved page object byte offset.
    pub page_object_byte_offset: usize,
    /// Classified transparency group when the page has a valid
    /// `/Group << /S /Transparency ... >>`.
    pub group: Option<ClassifiedTransparencyGroup>,
    /// Page-local structural `/Group` diagnostics.
    pub skipped: Vec<SkippedTransparencyGroup>,
}

/// Error returned when page `/Group` inspection cannot begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageTransparencyGroupsInspectionError {
    /// Caller-supplied root `/Pages` object offset.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root-node expansion failure.
    pub error: PageTreeKidTargetsInspectionError,
}

/// Inspect page top-level `/Group` entries through a classic xref table.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails.
pub fn inspect_document_page_transparency_groups(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
) -> Result<DocumentPageTransparencyGroupsInspection, DocumentPageTransparencyGroupsInspectionError>
{
    inspect_document_page_transparency_groups_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        root_node_object_offset,
    )
}

/// Inspect page top-level `/Group` entries through any object lookup backend.
///
/// The walk follows the page tree in document order. `/Group` is read only from
/// each leaf page dictionary; it is not a resource and is not inherited.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-page group
/// failures are diagnostics in a successful report.
pub fn inspect_document_page_transparency_groups_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
) -> Result<DocumentPageTransparencyGroupsInspection, DocumentPageTransparencyGroupsInspectionError>
{
    let root_targets =
        crate::inspect_page_tree_kid_targets_with_lookup(input, lookup, root_node_object_offset)
            .map_err(|error| DocumentPageTransparencyGroupsInspectionError {
                root_node_byte_offset: root_node_object_offset,
                byte_len: input.len(),
                error,
            })?;

    let mut walk = TransparencyGroupWalk::new();
    walk.visited.insert(
        root_targets
            .kids
            .node
            .node_dictionary
            .reference
            .object_number,
    );
    walk.visited_node_count = 1;
    walk.process_node(input, lookup, &root_targets, 0);

    Ok(DocumentPageTransparencyGroupsInspection {
        byte_len: input.len(),
        pages: walk.pages,
        page_tree_skipped: walk.page_tree_skipped,
        visited_node_count: walk.visited_node_count,
        truncated: walk.truncated,
    })
}

struct TransparencyGroupWalk {
    pages: Vec<PageTransparencyGroupInspection>,
    page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    visited: BTreeSet<u32>,
    visited_node_count: usize,
    truncated: Option<PageTreeLeavesTruncation>,
}

impl TransparencyGroupWalk {
    const fn new() -> Self {
        Self {
            pages: Vec::new(),
            page_tree_skipped: Vec::new(),
            visited: BTreeSet::new(),
            visited_node_count: 0,
            truncated: None,
        }
    }

    fn process_node(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        targets: &PageTreeKidTargetsInspection,
        depth: usize,
    ) {
        let node_byte_offset = targets.kids.node.node_dictionary.header_range.start;
        for entry in &targets.entries {
            match entry {
                PageTreeKidTargetInspection::Resolved { kid, target } => {
                    match target.node_type.node_type {
                        PageTreeNodeType::Page => self.inspect_page(
                            input,
                            lookup,
                            kid.reference,
                            target.object_byte_offset,
                        ),
                        PageTreeNodeType::Pages => self.descend_into_child(
                            input,
                            lookup,
                            ChildPagesNode {
                                reference: kid.reference,
                                object_byte_offset: target.object_byte_offset,
                                parent_node_byte_offset: node_byte_offset,
                            },
                            depth,
                        ),
                        PageTreeNodeType::Other => self.push_page_tree_skip(
                            kid.reference,
                            node_byte_offset,
                            SkippedPageTreeLeafReason::OtherNodeType {
                                object_byte_offset: target.object_byte_offset,
                            },
                        ),
                    }
                }
                PageTreeKidTargetInspection::Failed { kid, error } => self.push_page_tree_skip(
                    kid.reference,
                    node_byte_offset,
                    SkippedPageTreeLeafReason::UnresolvedTarget {
                        error: error.clone(),
                    },
                ),
            }
        }
    }

    fn inspect_page(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        page_reference: IndirectRef,
        page_object_byte_offset: usize,
    ) {
        let (group, skipped) =
            match inspect_indirect_object_dictionary(input, page_object_byte_offset) {
                Ok(dictionary) => match classify_transparency_group_entry(
                    input,
                    lookup,
                    page_object_byte_offset,
                    &dictionary.entries,
                ) {
                    Ok(group) => (group, Vec::new()),
                    Err(skip) => (None, vec![skip]),
                },
                Err(error) => (
                    None,
                    vec![skipped_group(
                        page_object_byte_offset,
                        SkippedTransparencyGroupReason::ObjectDictionaryFailed { error },
                    )],
                ),
            };

        self.pages.push(PageTransparencyGroupInspection {
            ordinal: self.pages.len(),
            page_reference,
            page_object_byte_offset,
            group,
            skipped,
        });
    }

    fn descend_into_child(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        child: ChildPagesNode,
        depth: usize,
    ) {
        if self.visited.contains(&child.reference.object_number) {
            self.stop_descent(
                child.reference,
                child.parent_node_byte_offset,
                PageTreeLeavesTruncation::Cycle {
                    object_number: child.reference.object_number,
                },
                SkippedPageTreeLeafReason::Cycle {
                    object_byte_offset: child.object_byte_offset,
                },
            );
            return;
        }
        let child_depth = depth + 1;
        if child_depth > crate::MAX_PAGE_TREE_DEPTH {
            self.stop_descent(
                child.reference,
                child.parent_node_byte_offset,
                PageTreeLeavesTruncation::MaxDepth {
                    max_depth: crate::MAX_PAGE_TREE_DEPTH,
                },
                SkippedPageTreeLeafReason::MaxDepthExceeded {
                    object_byte_offset: child.object_byte_offset,
                    attempted_depth: child_depth,
                },
            );
            return;
        }
        if self.visited_node_count >= crate::MAX_VISITED_PAGE_TREE_NODES {
            self.stop_descent(
                child.reference,
                child.parent_node_byte_offset,
                PageTreeLeavesTruncation::MaxVisitedNodes {
                    max_visited_nodes: crate::MAX_VISITED_PAGE_TREE_NODES,
                },
                SkippedPageTreeLeafReason::MaxVisitedNodesExceeded {
                    object_byte_offset: child.object_byte_offset,
                },
            );
            return;
        }

        self.visited.insert(child.reference.object_number);
        self.visited_node_count += 1;
        match crate::inspect_page_tree_kid_targets_with_lookup(
            input,
            lookup,
            child.object_byte_offset,
        ) {
            Ok(child_targets) => self.process_node(input, lookup, &child_targets, child_depth),
            Err(error) => self.push_page_tree_skip(
                child.reference,
                child.parent_node_byte_offset,
                SkippedPageTreeLeafReason::NodeExpansionFailed { error },
            ),
        }
    }

    fn stop_descent(
        &mut self,
        kid: IndirectRef,
        parent_node_byte_offset: usize,
        truncation: PageTreeLeavesTruncation,
        reason: SkippedPageTreeLeafReason,
    ) {
        if self.truncated.is_none() {
            self.truncated = Some(truncation);
        }
        self.push_page_tree_skip(kid, parent_node_byte_offset, reason);
    }

    fn push_page_tree_skip(
        &mut self,
        kid: IndirectRef,
        parent_node_byte_offset: usize,
        reason: SkippedPageTreeLeafReason,
    ) {
        self.page_tree_skipped.push(SkippedPageTreeLeafEntry {
            kid,
            parent_node_byte_offset,
            reason,
        });
    }
}

#[derive(Debug, Clone, Copy)]
struct ChildPagesNode {
    reference: IndirectRef,
    object_byte_offset: usize,
    parent_node_byte_offset: usize,
}
