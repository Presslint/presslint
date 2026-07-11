use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::font_classify::{
    ClassifiedFontResource, SkippedFontResource, inspect_effective_font_resource_entries,
};
use crate::page_resource_inheritance::ResourceContext;
use crate::{
    ClassicXrefTableInspection, IndirectRef, ObjectLookup, PageTreeKidTargetInspection,
    PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError, PageTreeLeavesTruncation,
    PageTreeNodeType, SkippedPageTreeLeafEntry, SkippedPageTreeLeafReason,
    SkippedPageXObjectResourceReason, inspect_indirect_object_dictionary,
};

/// Per-document page `/Resources /Font` classification report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageFontResourcesInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Document-ordered per-page `Font` resource reports.
    pub pages: Vec<PageFontResourcesInspection>,
    /// Ordered page-tree traversal skips for children that were not leaf pages.
    pub page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    /// Number of `/Pages` nodes expanded during the walk.
    pub visited_node_count: usize,
    /// First traversal bound that stopped a descent, when any.
    pub truncated: Option<PageTreeLeavesTruncation>,
}

impl DocumentPageFontResourcesInspection {
    /// Count of inspected leaf pages.
    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// Per-page classified page-scope `Font` resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageFontResourcesInspection {
    /// Zero-based document-order page ordinal.
    pub ordinal: usize,
    /// Indirect reference of the leaf `/Page`.
    pub page_reference: IndirectRef,
    /// Resolved page object byte offset.
    pub page_object_byte_offset: usize,
    /// Classified `Font` resources, sorted/deduplicated by raw name.
    pub fonts: Vec<ClassifiedFontResource>,
    /// Page-local structural `Font` diagnostics.
    pub skipped: Vec<SkippedFontResource>,
}

/// Error returned when page `Font` resource inspection cannot begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageFontResourcesInspectionError {
    /// Caller-supplied root `/Pages` object offset.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root-node expansion failure.
    pub error: PageTreeKidTargetsInspectionError,
}

/// Inspect page-scope `/Resources /Font` entries through a classic xref table.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-page resource
/// failures are recorded as structured page diagnostics.
pub fn inspect_document_page_font_resources(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
) -> Result<DocumentPageFontResourcesInspection, DocumentPageFontResourcesInspectionError> {
    inspect_document_page_font_resources_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        root_node_object_offset,
    )
}

/// Inspect page-scope `/Resources /Font` entries through any object lookup
/// backend.
///
/// The walk follows the page tree root-down in document order, carrying the
/// effective inheritable `/Resources` dictionary as whole-dictionary
/// nearest-ancestor replacement, never per-key merge (ISO 32000-1 Table 30,
/// §7.7.3.4). Each `/Font` entry is classified shallowly into exact `/Type`
/// and `/Subtype` structural facts; unsupported or unresolved resource shapes
/// become structured page-local skips.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-child
/// page-tree failures and per-page resource failures are diagnostics in a
/// successful report.
pub fn inspect_document_page_font_resources_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
) -> Result<DocumentPageFontResourcesInspection, DocumentPageFontResourcesInspectionError> {
    let root_targets =
        crate::inspect_page_tree_kid_targets_with_lookup(input, lookup, root_node_object_offset)
            .map_err(|error| DocumentPageFontResourcesInspectionError {
                root_node_byte_offset: root_node_object_offset,
                byte_len: input.len(),
                error,
            })?;

    let root_context = ResourceContext::from_dictionary(
        input,
        lookup,
        &root_targets.kids.node.node_dictionary,
        None,
    );
    let mut walk = FontResourceWalk::new();
    walk.visited.insert(
        root_targets
            .kids
            .node
            .node_dictionary
            .reference
            .object_number,
    );
    walk.visited_node_count = 1;
    walk.process_node(input, lookup, &root_targets, &root_context, 0);

    Ok(DocumentPageFontResourcesInspection {
        byte_len: input.len(),
        pages: walk.pages,
        page_tree_skipped: walk.page_tree_skipped,
        visited_node_count: walk.visited_node_count,
        truncated: walk.truncated,
    })
}

struct FontResourceWalk {
    pages: Vec<PageFontResourcesInspection>,
    page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    visited: BTreeSet<u32>,
    visited_node_count: usize,
    truncated: Option<PageTreeLeavesTruncation>,
}

impl FontResourceWalk {
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
        context: &ResourceContext,
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
                            context,
                        ),
                        PageTreeNodeType::Pages => self.descend_into_child(
                            input,
                            lookup,
                            ChildPagesNode {
                                reference: kid.reference,
                                object_byte_offset: target.object_byte_offset,
                                parent_node_byte_offset: node_byte_offset,
                            },
                            context,
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
        inherited: &ResourceContext,
    ) {
        let context = match inspect_indirect_object_dictionary(input, page_object_byte_offset) {
            Ok(dictionary) => {
                ResourceContext::from_dictionary(input, lookup, &dictionary, Some(inherited))
            }
            Err(error) => {
                let mut skips = inherited.skips.clone();
                skips.push(SkippedPageXObjectResourceReason::PageDictionaryFailed { error });
                ResourceContext {
                    resources: inherited.resources.clone(),
                    skips,
                }
            }
        };

        let mut page = inspect_effective_fonts(input, lookup, page_object_byte_offset, &context);
        page.ordinal = self.pages.len();
        page.page_reference = page_reference;
        self.pages.push(page);
    }

    fn descend_into_child(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        child: ChildPagesNode,
        inherited: &ResourceContext,
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
            Ok(child_targets) => {
                let context = ResourceContext::from_dictionary(
                    input,
                    lookup,
                    &child_targets.kids.node.node_dictionary,
                    Some(inherited),
                );
                self.process_node(input, lookup, &child_targets, &context, child_depth);
            }
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

fn inspect_effective_fonts(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page_object_byte_offset: usize,
    context: &ResourceContext,
) -> PageFontResourcesInspection {
    let effective =
        inspect_effective_font_resource_entries(input, lookup, page_object_byte_offset, context);
    page_report(page_object_byte_offset, effective.skipped, effective.fonts)
}

const fn page_report(
    page_object_byte_offset: usize,
    skipped: Vec<SkippedFontResource>,
    fonts: Vec<ClassifiedFontResource>,
) -> PageFontResourcesInspection {
    PageFontResourcesInspection {
        ordinal: 0,
        page_reference: IndirectRef {
            object_number: 0,
            generation: 0,
        },
        page_object_byte_offset,
        fonts,
        skipped,
    }
}
