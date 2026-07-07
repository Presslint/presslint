use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::page_color_space_classify::classify_color_space_definition_entry;
use crate::page_resource_inheritance::{ResourceContext, unique_entry};
use crate::{
    ClassicXrefTableInspection, ClassifiedColorSpaceDefinition, DictionaryEntryByteRange,
    DictionaryEntrySpan, DictionaryValueKind, IndirectRef, ObjectLookup,
    PageTreeKidTargetInspection, PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError,
    PageTreeLeavesTruncation, PageTreeNodeType, SkippedColorSpaceResourceReason,
    SkippedPageTreeLeafEntry, SkippedPageTreeLeafReason, SkippedPageXObjectResourceReason,
    inspect_indirect_object_dictionary,
};

/// Device colour-space default key observed in a resource dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultColorSpaceKind {
    /// `/DefaultGray`.
    DefaultGray,
    /// `/DefaultRGB`.
    DefaultRgb,
    /// `/DefaultCMYK`.
    DefaultCmyk,
}

impl DefaultColorSpaceKind {
    const ALL: [Self; 3] = [Self::DefaultGray, Self::DefaultRgb, Self::DefaultCmyk];

    const fn key(self) -> &'static [u8] {
        match self {
            Self::DefaultGray => b"/DefaultGray",
            Self::DefaultRgb => b"/DefaultRGB",
            Self::DefaultCmyk => b"/DefaultCMYK",
        }
    }
}

/// One classified default device colour-space environment fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultColorSpaceFact {
    /// Default-space resource key that supplied this fact.
    pub kind: DefaultColorSpaceKind,
    /// Shallow classified colour-space definition.
    pub color_space: ClassifiedColorSpaceDefinition,
}

/// Per-document page default colour-space environment report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageDefaultColorSpacesInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Document-ordered per-page default colour-space facts.
    pub pages: Vec<PageDefaultColorSpacesInspection>,
    /// Ordered page-tree traversal skips for children that were not leaf pages.
    pub page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    /// Number of `/Pages` nodes expanded during the walk.
    pub visited_node_count: usize,
    /// First traversal bound that stopped a descent, when any.
    pub truncated: Option<PageTreeLeavesTruncation>,
}

/// Per-page effective default colour-space facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageDefaultColorSpacesInspection {
    /// Zero-based document-order page ordinal.
    pub ordinal: usize,
    /// Indirect reference of the leaf `/Page`.
    pub page_reference: IndirectRef,
    /// Resolved page object byte offset.
    pub page_object_byte_offset: usize,
    /// Classified default colour-space facts in canonical key order.
    pub defaults: Vec<DefaultColorSpaceFact>,
    /// Page-local default colour-space diagnostics.
    pub skipped: Vec<SkippedDefaultColorSpace>,
}

/// Classified own-scope default colour spaces for one Form `XObject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormDefaultColorSpacesInspection {
    /// Resolved form stream object byte offset the resources were read from.
    pub object_byte_offset: usize,
    /// Classified form-local default colour-space facts in canonical key order.
    pub defaults: Vec<DefaultColorSpaceFact>,
    /// Form-local default colour-space diagnostics.
    pub skipped: Vec<SkippedDefaultColorSpace>,
}

/// One default colour-space diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedDefaultColorSpace {
    /// Page or form object byte offset whose resource environment was inspected.
    pub object_byte_offset: usize,
    /// Default key when the diagnostic concerns one default entry.
    pub kind: Option<DefaultColorSpaceKind>,
    /// Structured skip reason.
    pub reason: SkippedDefaultColorSpaceReason,
}

/// Structured reason a default colour-space fact was not classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedDefaultColorSpaceReason {
    /// A `/Resources` inheritance or resolution diagnostic.
    Resources {
        /// Delegated `/Resources` resolution/inheritance failure.
        resources_reason: SkippedPageXObjectResourceReason,
    },
    /// No effective `/Resources` dictionary was available.
    MissingResources,
    /// A default key occurred more than once in the effective resource
    /// dictionary.
    DuplicateDefault {
        /// First default key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate default key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The default value did not classify as a supported shallow colour-space
    /// definition.
    ColorSpace {
        /// Delegated colour-space classifier reason.
        color_space_reason: SkippedColorSpaceResourceReason,
    },
}

/// Error returned when page default colour-space inspection cannot begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageDefaultColorSpacesInspectionError {
    /// Caller-supplied root `/Pages` object offset.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root-node expansion failure.
    pub error: PageTreeKidTargetsInspectionError,
}

/// Inspect page-scope default colour spaces through a classic xref table.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-page
/// resource/default failures are recorded as structured diagnostics.
pub fn inspect_document_page_default_color_spaces(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
) -> Result<DocumentPageDefaultColorSpacesInspection, DocumentPageDefaultColorSpacesInspectionError>
{
    inspect_document_page_default_color_spaces_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        root_node_object_offset,
    )
}

/// Inspect page-scope default colour spaces through any object lookup backend.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-page
/// resource/default failures are recorded as structured diagnostics.
pub fn inspect_document_page_default_color_spaces_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
) -> Result<DocumentPageDefaultColorSpacesInspection, DocumentPageDefaultColorSpacesInspectionError>
{
    let root_targets =
        crate::inspect_page_tree_kid_targets_with_lookup(input, lookup, root_node_object_offset)
            .map_err(|error| DocumentPageDefaultColorSpacesInspectionError {
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
    let mut walk = DefaultColorSpaceWalk::new();
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

    Ok(DocumentPageDefaultColorSpacesInspection {
        byte_len: input.len(),
        pages: walk.pages,
        page_tree_skipped: walk.page_tree_skipped,
        visited_node_count: walk.visited_node_count,
        truncated: walk.truncated,
    })
}

/// Inspect one Form `XObject`'s own default colour spaces.
#[must_use]
pub fn inspect_form_default_color_spaces(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> FormDefaultColorSpacesInspection {
    let context = match inspect_indirect_object_dictionary(input, object_byte_offset) {
        Ok(dictionary) => ResourceContext::from_dictionary(input, lookup, &dictionary, None),
        Err(error) => {
            return FormDefaultColorSpacesInspection {
                object_byte_offset,
                defaults: Vec::new(),
                skipped: vec![skip_record(
                    object_byte_offset,
                    None,
                    SkippedDefaultColorSpaceReason::Resources {
                        resources_reason: SkippedPageXObjectResourceReason::PageDictionaryFailed {
                            error,
                        },
                    },
                )],
            };
        }
    };
    let (defaults, skipped) =
        inspect_effective_defaults(input, lookup, object_byte_offset, &context);
    FormDefaultColorSpacesInspection {
        object_byte_offset,
        defaults,
        skipped,
    }
}

struct DefaultColorSpaceWalk {
    pages: Vec<PageDefaultColorSpacesInspection>,
    page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    visited: BTreeSet<u32>,
    visited_node_count: usize,
    truncated: Option<PageTreeLeavesTruncation>,
}

impl DefaultColorSpaceWalk {
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

        let (defaults, skipped) =
            inspect_effective_defaults(input, lookup, page_object_byte_offset, &context);
        self.pages.push(PageDefaultColorSpacesInspection {
            ordinal: self.pages.len(),
            page_reference,
            page_object_byte_offset,
            defaults,
            skipped,
        });
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

fn inspect_effective_defaults(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    context: &ResourceContext,
) -> (Vec<DefaultColorSpaceFact>, Vec<SkippedDefaultColorSpace>) {
    let mut skipped = context
        .skips
        .iter()
        .cloned()
        .map(|resources_reason| {
            skip_record(
                object_byte_offset,
                None,
                SkippedDefaultColorSpaceReason::Resources { resources_reason },
            )
        })
        .collect::<Vec<_>>();
    let Some(resources) = &context.resources else {
        skipped.push(skip_record(
            object_byte_offset,
            None,
            SkippedDefaultColorSpaceReason::MissingResources,
        ));
        return (Vec::new(), skipped);
    };

    let Some(entries) = color_space_entries_from_resources(
        input,
        object_byte_offset,
        &resources.entries,
        &mut skipped,
    ) else {
        return (Vec::new(), skipped);
    };

    let mut defaults = Vec::new();
    for kind in DefaultColorSpaceKind::ALL {
        let Some(entry) = (match unique_entry(input, &entries, kind.key()) {
            Ok(entry) => entry,
            Err((first_key_range, duplicate_key_range)) => {
                skipped.push(skip_record(
                    object_byte_offset,
                    Some(kind),
                    SkippedDefaultColorSpaceReason::DuplicateDefault {
                        first_key_range,
                        duplicate_key_range,
                    },
                ));
                continue;
            }
        }) else {
            continue;
        };

        match classify_color_space_definition_entry(input, lookup, entry) {
            Ok(color_space) => defaults.push(DefaultColorSpaceFact { kind, color_space }),
            Err(color_space_reason) => skipped.push(skip_record(
                object_byte_offset,
                Some(kind),
                SkippedDefaultColorSpaceReason::ColorSpace { color_space_reason },
            )),
        }
    }
    (defaults, skipped)
}

fn color_space_entries_from_resources(
    input: &[u8],
    object_byte_offset: usize,
    resource_entries: &[DictionaryEntrySpan],
    skipped: &mut Vec<SkippedDefaultColorSpace>,
) -> Option<Vec<DictionaryEntrySpan>> {
    let cs_entry = match unique_entry(input, resource_entries, b"/ColorSpace") {
        Ok(entry) => entry?,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(skip_record(
                object_byte_offset,
                None,
                SkippedDefaultColorSpaceReason::ColorSpace {
                    color_space_reason: SkippedColorSpaceResourceReason::DuplicateColorSpace {
                        first_key_range,
                        duplicate_key_range,
                    },
                },
            ));
            return None;
        }
    };

    if cs_entry.value_kind != DictionaryValueKind::Dictionary {
        skipped.push(skip_record(
            object_byte_offset,
            None,
            SkippedDefaultColorSpaceReason::ColorSpace {
                color_space_reason: SkippedColorSpaceResourceReason::NonDictionaryColorSpace {
                    value_kind: cs_entry.value_kind,
                },
            },
        ));
        return None;
    }

    match crate::inspect_dictionary_entries(input, cs_entry.value_range.start) {
        Ok(entries) => Some(entries.entries),
        Err(error) => {
            skipped.push(skip_record(
                object_byte_offset,
                None,
                SkippedDefaultColorSpaceReason::ColorSpace {
                    color_space_reason:
                        SkippedColorSpaceResourceReason::ColorSpaceDictionaryFailed { error },
                },
            ));
            None
        }
    }
}

const fn skip_record(
    object_byte_offset: usize,
    kind: Option<DefaultColorSpaceKind>,
    reason: SkippedDefaultColorSpaceReason,
) -> SkippedDefaultColorSpace {
    SkippedDefaultColorSpace {
        object_byte_offset,
        kind,
        reason,
    }
}
