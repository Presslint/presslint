use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefTableInspection, IndirectRef, PageTreeKidTargetInspection,
    PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError, PageTreeNodeType,
    PageTreeReferenceTargetInspectionError, inspect_page_tree_kid_targets,
};

/// Maximum page-tree recursion depth.
///
/// The root `/Pages` node is depth `0`; descent into a child `/Pages` node at a
/// depth beyond this bound is refused and reported as a structured truncation
/// marker rather than recursing further. This caps the recursion stack
/// independently of any cycle guard.
pub const MAX_PAGE_TREE_DEPTH: usize = 64;

/// Maximum number of `/Pages` nodes (root plus intermediates) expanded in one
/// walk.
///
/// Once this many nodes have been expanded, descent into any further child
/// `/Pages` node is refused and reported as a structured truncation marker. This
/// caps total work at `O(nodes)` for adversarial `/Kids` graphs.
pub const MAX_VISITED_PAGE_TREE_NODES: usize = 65_536;

/// Document-ordered leaf `/Page` enumeration of a page tree.
///
/// This report stores only the caller-visible source length, the document-order
/// leaf vector, the ordered skip diagnostics, the visited-node count, and an
/// optional truncation marker. It does not retain or copy PDF bytes, object
/// bodies, stream bodies, page dictionaries, page-tree dictionaries, content
/// streams, decoded streams, or source slices; owned data is limited to offsets,
/// the kid `IndirectRef` metadata already present in delegated reports, small
/// enums, the delegated reference-target/kid-target error metadata for skips, and
/// the result vectors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeLeavesInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Leaf `/Page` objects in left-to-right depth-first document order.
    pub leaves: Vec<PageTreeLeaf>,
    /// Ordered skip diagnostics for non-leaf, failed, or bound-stopped kids.
    pub skipped: Vec<SkippedPageTreeLeafEntry>,
    /// Number of `/Pages` nodes expanded during the walk (root plus
    /// intermediates).
    pub visited_node_count: usize,
    /// First bound that stopped a descent, when any bound was hit; `None` when
    /// the whole tree was enumerated within the bounds.
    pub truncated: Option<PageTreeLeavesTruncation>,
}

impl PageTreeLeavesInspection {
    /// Count of enumerated leaf `/Page` objects.
    #[must_use]
    pub const fn leaf_count(&self) -> usize {
        self.leaves.len()
    }
}

/// One enumerated leaf `/Page` object in document order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeLeaf {
    /// Indirect reference of the leaf `/Page` kid, as reported by `/Kids`.
    pub reference: IndirectRef,
    /// Resolved in-use object byte offset of the leaf `/Page` object.
    pub object_byte_offset: usize,
}

/// One kid skipped while enumerating leaf pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPageTreeLeafEntry {
    /// Indirect reference of the kid this skip concerns.
    pub kid: IndirectRef,
    /// Byte offset of the parent `/Pages` node whose `/Kids` held this kid.
    pub parent_node_byte_offset: usize,
    /// Structured skip reason.
    pub reason: SkippedPageTreeLeafReason,
}

/// Structured reason a kid was skipped during leaf enumeration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedPageTreeLeafReason {
    /// The kid resolved to a node whose `/Type` was neither `/Pages` nor
    /// `/Page`.
    OtherNodeType {
        /// Resolved in-use object byte offset of the skipped node.
        object_byte_offset: usize,
    },
    /// The kid reference failed to resolve or classify; the delegated failure is
    /// preserved verbatim and the rest of the walk continues.
    UnresolvedTarget {
        /// Delegated reference-target inspection failure.
        error: PageTreeReferenceTargetInspectionError,
    },
    /// The kid resolved to a `/Pages` node but expanding its own `/Kids` failed;
    /// the delegated failure is preserved verbatim and the rest of the walk
    /// continues.
    NodeExpansionFailed {
        /// Delegated kid-targets inspection failure for the child node.
        error: PageTreeKidTargetsInspectionError,
    },
    /// Descent into a `/Pages` child was refused because the maximum recursion
    /// depth would be exceeded.
    MaxDepthExceeded {
        /// Resolved in-use object byte offset of the un-expanded child node.
        object_byte_offset: usize,
        /// Depth the refused child would have occupied.
        attempted_depth: usize,
    },
    /// Descent into a `/Pages` child was refused because the maximum visited-node
    /// count had already been reached.
    MaxVisitedNodesExceeded {
        /// Resolved in-use object byte offset of the un-expanded child node.
        object_byte_offset: usize,
    },
    /// Descent into a `/Pages` child was refused because its object number was
    /// already visited in this walk (cycle guard).
    Cycle {
        /// Resolved in-use object byte offset of the un-expanded child node.
        object_byte_offset: usize,
    },
}

/// First bound that stopped a descent during leaf enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "limit", rename_all = "snake_case")]
pub enum PageTreeLeavesTruncation {
    /// The recursion depth bound stopped a descent.
    MaxDepth {
        /// Configured maximum recursion depth.
        max_depth: usize,
    },
    /// The visited-node count bound stopped a descent.
    MaxVisitedNodes {
        /// Configured maximum visited-node count.
        max_visited_nodes: usize,
    },
    /// The cycle guard stopped a descent into an already-visited `/Pages` node.
    Cycle {
        /// Object number of the already-visited `/Pages` node.
        object_number: u32,
    },
}

/// Error returned when leaf enumeration cannot begin.
///
/// This is returned only when the delegated root-node
/// [`inspect_page_tree_kid_targets`] fails at the supplied root offset.
/// Per-child resolution failures, `Other` targets, child-node expansion
/// failures, and bound-stopped descents are non-fatal skips inside a successful
/// report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeLeavesInspectionError {
    /// Caller-supplied byte offset of the root `/Pages` node.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root-node kid-targets inspection failure.
    pub error: PageTreeKidTargetsInspectionError,
}

/// Enumerate the leaf `/Page` objects of a page tree in document order.
///
/// Starting from a caller-supplied root `/Pages` node byte offset, the walk
/// delegates one node expansion to [`inspect_page_tree_kid_targets`] and
/// classifies each [`PageTreeKidTargetInspection::Resolved`] entry by its
/// delegated `node_type.node_type`: a `Page` becomes an ordered leaf entry
/// carrying the kid [`IndirectRef`] and resolved `object_byte_offset`, a `Pages`
/// is recursed into at its resolved `object_byte_offset`, and an `Other` is
/// recorded as a structured skip. [`PageTreeKidTargetInspection::Failed`] kids
/// are recorded as structured skips and never abort the rest of the walk.
///
/// Recursion is depth-first and visits each node's kids in source order, so the
/// produced leaf order equals a left-to-right depth-first traversal of `/Kids`.
/// It is bounded by [`MAX_PAGE_TREE_DEPTH`] and [`MAX_VISITED_PAGE_TREE_NODES`]
/// and cycle-guarded by a visited object-number set over intermediate `/Pages`
/// nodes; hitting any bound records a structured truncation marker plus a skip
/// entry identifying the bound and the kid, and never silently drops the
/// remainder.
///
/// It does not validate `/Count`, reconcile inherited attributes, inspect leaf
/// `/Contents`/`/Resources`/boxes/`/Annots`/`/Parent`, follow `/Prev`, parse
/// xref or object streams, build caches or object maps, or mutate source bytes.
///
/// # Errors
///
/// Returns [`PageTreeLeavesInspectionError`] only when the delegated root-node
/// [`inspect_page_tree_kid_targets`] fails at `root_node_object_offset`.
pub fn inspect_page_tree_leaves(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
) -> Result<PageTreeLeavesInspection, PageTreeLeavesInspectionError> {
    let root_targets = inspect_page_tree_kid_targets(input, xref, root_node_object_offset)
        .map_err(|error| PageTreeLeavesInspectionError {
            root_node_byte_offset: root_node_object_offset,
            byte_len: input.len(),
            error,
        })?;

    let mut walk = LeafWalk::new();
    walk.visited.insert(
        root_targets
            .kids
            .node
            .node_dictionary
            .reference
            .object_number,
    );
    walk.visited_node_count = 1;
    walk.process_node(input, xref, &root_targets, 0);

    Ok(PageTreeLeavesInspection {
        byte_len: input.len(),
        leaves: walk.leaves,
        skipped: walk.skipped,
        visited_node_count: walk.visited_node_count,
        truncated: walk.truncated,
    })
}

/// Mutable accumulators and cycle/limit state for one bounded leaf walk.
struct LeafWalk {
    leaves: Vec<PageTreeLeaf>,
    skipped: Vec<SkippedPageTreeLeafEntry>,
    visited: BTreeSet<u32>,
    visited_node_count: usize,
    truncated: Option<PageTreeLeavesTruncation>,
}

impl LeafWalk {
    const fn new() -> Self {
        Self {
            leaves: Vec::new(),
            skipped: Vec::new(),
            visited: BTreeSet::new(),
            visited_node_count: 0,
            truncated: None,
        }
    }

    /// Classify each already-expanded kid target of one `/Pages` node in source
    /// order, emitting leaves, recursing into child `/Pages` nodes, and recording
    /// skips.
    fn process_node(
        &mut self,
        input: &[u8],
        xref: &ClassicXrefTableInspection,
        targets: &PageTreeKidTargetsInspection,
        depth: usize,
    ) {
        let node_byte_offset = targets.kids.node.node_dictionary.header_range.start;
        for entry in &targets.entries {
            match entry {
                PageTreeKidTargetInspection::Resolved { kid, target } => {
                    match target.node_type.node_type {
                        PageTreeNodeType::Page => self.leaves.push(PageTreeLeaf {
                            reference: kid.reference,
                            object_byte_offset: target.object_byte_offset,
                        }),
                        PageTreeNodeType::Pages => self.descend_into_child(
                            input,
                            xref,
                            kid.reference,
                            target.object_byte_offset,
                            node_byte_offset,
                            depth,
                        ),
                        PageTreeNodeType::Other => self.push_skip(
                            kid.reference,
                            node_byte_offset,
                            SkippedPageTreeLeafReason::OtherNodeType {
                                object_byte_offset: target.object_byte_offset,
                            },
                        ),
                    }
                }
                PageTreeKidTargetInspection::Failed { kid, error } => self.push_skip(
                    kid.reference,
                    node_byte_offset,
                    SkippedPageTreeLeafReason::UnresolvedTarget {
                        error: error.clone(),
                    },
                ),
            }
        }
    }

    /// Apply the cycle/depth/node-count bounds, then expand the child `/Pages`
    /// node when all bounds allow it.
    fn descend_into_child(
        &mut self,
        input: &[u8],
        xref: &ClassicXrefTableInspection,
        kid: IndirectRef,
        child_byte_offset: usize,
        parent_node_byte_offset: usize,
        depth: usize,
    ) {
        if self.visited.contains(&kid.object_number) {
            self.stop_descent(
                kid,
                parent_node_byte_offset,
                PageTreeLeavesTruncation::Cycle {
                    object_number: kid.object_number,
                },
                SkippedPageTreeLeafReason::Cycle {
                    object_byte_offset: child_byte_offset,
                },
            );
            return;
        }

        let child_depth = depth + 1;
        if child_depth > MAX_PAGE_TREE_DEPTH {
            self.stop_descent(
                kid,
                parent_node_byte_offset,
                PageTreeLeavesTruncation::MaxDepth {
                    max_depth: MAX_PAGE_TREE_DEPTH,
                },
                SkippedPageTreeLeafReason::MaxDepthExceeded {
                    object_byte_offset: child_byte_offset,
                    attempted_depth: child_depth,
                },
            );
            return;
        }

        if self.visited_node_count >= MAX_VISITED_PAGE_TREE_NODES {
            self.stop_descent(
                kid,
                parent_node_byte_offset,
                PageTreeLeavesTruncation::MaxVisitedNodes {
                    max_visited_nodes: MAX_VISITED_PAGE_TREE_NODES,
                },
                SkippedPageTreeLeafReason::MaxVisitedNodesExceeded {
                    object_byte_offset: child_byte_offset,
                },
            );
            return;
        }

        self.visited.insert(kid.object_number);
        self.visited_node_count += 1;

        match inspect_page_tree_kid_targets(input, xref, child_byte_offset) {
            Ok(child_targets) => self.process_node(input, xref, &child_targets, child_depth),
            Err(error) => self.push_skip(
                kid,
                parent_node_byte_offset,
                SkippedPageTreeLeafReason::NodeExpansionFailed { error },
            ),
        }
    }

    /// Stop descending into a child `/Pages` node because a bound was hit,
    /// recording the truncation marker (first bound wins) and the matching skip
    /// entry together so a bound is never a silent drop.
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
        self.push_skip(kid, parent_node_byte_offset, reason);
    }

    fn push_skip(
        &mut self,
        kid: IndirectRef,
        parent_node_byte_offset: usize,
        reason: SkippedPageTreeLeafReason,
    ) {
        self.skipped.push(SkippedPageTreeLeafEntry {
            kid,
            parent_node_byte_offset,
            reason,
        });
    }
}
