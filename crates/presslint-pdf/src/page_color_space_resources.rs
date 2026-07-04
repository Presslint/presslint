use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::page_color_space_classify::classify_color_space_entry;
use crate::page_resource_inheritance::{ResourceContext, unique_entry};
use crate::{
    ClassicXrefTableInspection, DictionaryEntryByteRange, DictionaryEntryInspectionError,
    DictionaryEntrySpan, DictionaryValueKind, IndirectRef, ObjectLookup, ObjectLookupLocation,
    PageTreeKidTargetInspection, PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError,
    PageTreeLeavesTruncation, PageTreeNodeType, PdfName, SkippedPageTreeLeafEntry,
    SkippedPageTreeLeafReason, SkippedPageXObjectResourceReason,
    inspect_indirect_object_dictionary,
};

/// Structural colour-space family classified from a `/Resources /ColorSpace`
/// entry.
///
/// This mirrors the small subset of PDF colour-space families the inventory
/// walker needs to interpret `cs`/`scn`. It is a SHAPE fact only: no paint
/// semantics, colourimetry, tint-transform evaluation, or profile parsing beyond
/// the shallow `/N` component count is implied. It is intentionally free of
/// `presslint-types` so `presslint-pdf` keeps no dependency on the inventory
/// type layer; the umbrella crate maps this into the inventory colour model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpaceFamily {
    /// `/DeviceGray`.
    DeviceGray,
    /// `/DeviceRGB`.
    DeviceRgb,
    /// `/DeviceCMYK`.
    DeviceCmyk,
    /// `[/ICCBased stream]`.
    IccBased,
    /// `[/Separation …]`.
    Separation,
    /// `[/DeviceN …]`.
    DeviceN,
}

/// One classified `/Resources /ColorSpace` entry.
///
/// Stores only the resource name, the structural family, an optional shallow
/// component count (from `/ICCBased /N`, or the colorant arity of
/// `Separation`/`DeviceN`), the spot colorant names, and the optional alternate
/// family recorded as a FACT (never substituted as the painted source). No PDF
/// bytes, dictionaries, stream bodies, decoded data, or source slices are kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedColorSpaceResource {
    /// Resource name (without the leading slash) that selects this space.
    pub name: PdfName,
    /// Structural colour-space family.
    pub family: ColorSpaceFamily,
    /// Shallow component count when known.
    pub component_count: Option<usize>,
    /// Spot colorant names for `Separation` (one) or `DeviceN` (many).
    pub spot_names: Vec<PdfName>,
    /// Alternate colour space recorded as a fact for `Separation`/`DeviceN`.
    pub alternate_space: Option<ColorSpaceFamily>,
}

/// Per-document page `/Resources /ColorSpace` classification report.
///
/// Stores only structural metadata, document-order per-page classified entries,
/// and small skip records. It retains no PDF bytes, object bodies, resource
/// dictionaries, stream bodies, decoded data, or source slices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageColorSpaceResourcesInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Document-ordered per-page colour-space resource reports.
    pub pages: Vec<PageColorSpaceResourcesInspection>,
    /// Ordered page-tree traversal skips for children that were not leaf pages.
    pub page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    /// Number of `/Pages` nodes expanded during the walk.
    pub visited_node_count: usize,
    /// First traversal bound that stopped a descent, when any.
    pub truncated: Option<PageTreeLeavesTruncation>,
}

impl DocumentPageColorSpaceResourcesInspection {
    /// Count of inspected leaf pages.
    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// Per-page classified page-scope colour-space resources.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageColorSpaceResourcesInspection {
    /// Zero-based document-order page ordinal.
    pub ordinal: usize,
    /// Indirect reference of the leaf `/Page`.
    pub page_reference: IndirectRef,
    /// Resolved page object byte offset.
    pub page_object_byte_offset: usize,
    /// Classified colour-space resources, sorted/deduplicated by name.
    pub color_spaces: Vec<ClassifiedColorSpaceResource>,
    /// Page-local structural colour-space diagnostics.
    pub skipped: Vec<SkippedColorSpaceResource>,
}

/// One page-local colour-space resource diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedColorSpaceResource {
    /// Resolved leaf page object byte offset.
    pub page_object_byte_offset: usize,
    /// Resource name when the diagnostic concerns one `/ColorSpace` entry.
    pub resource_name: Option<PdfName>,
    /// Structured skip reason.
    pub reason: SkippedColorSpaceResourceReason,
}

/// Structured reason a page-scope colour-space resource was not classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedColorSpaceResourceReason {
    /// A `/Resources` inheritance-level diagnostic (delegated vocabulary).
    Resources {
        /// Delegated `/Resources` resolution/inheritance failure.
        resources_reason: SkippedPageXObjectResourceReason,
    },
    /// No effective `/Resources` dictionary was available for this page.
    MissingColorSpaceResources,
    /// No `/ColorSpace` sub-dictionary was present in the effective resources.
    MissingColorSpace,
    /// A `/ColorSpace` key occurred more than once.
    DuplicateColorSpace {
        /// First `/ColorSpace` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/ColorSpace` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/ColorSpace` value was not a direct dictionary.
    NonDictionaryColorSpace {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The direct `/ColorSpace` dictionary could not be scanned.
    ColorSpaceDictionaryFailed {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// A `/ColorSpace` dictionary repeated the same resource name.
    DuplicateColorSpaceName {
        /// First matching resource-name key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching resource-name key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// A colour-space name was not a modeled device / special family.
    UnknownColorSpaceName {
        /// Raw name bytes including the leading slash.
        name: Vec<u8>,
    },
    /// A colour-space array operand was malformed or the wrong shape.
    MalformedColorSpaceOperand,
    /// A colour-space array declared an unexpected component count.
    WrongComponentCount {
        /// Expected count for the family, when known.
        expected: Option<usize>,
        /// Observed count.
        got: usize,
    },
    /// `[/Pattern …]` — pattern colour is a counted skip in this slice.
    UnsupportedPatternColor,
    /// `[/Indexed …]` — indexed colour expansion is a counted skip in this slice.
    UnsupportedIndexedColor,
    /// `[/Lab …]`, `[/CalGray …]`, or `[/CalRGB …]` — CIE colourimetry is a
    /// counted skip in this slice.
    UnsupportedLabOrCalSpace,
    /// An indirect colour-space or profile reference did not resolve.
    UnresolvedResourceReference {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number, when available.
        location: Option<ObjectLookupLocation>,
    },
    /// A `Separation`/`DeviceN` tint-transform operand was malformed.
    UnsupportedTintTransform,
}

/// Error returned when page colour-space resource inspection cannot begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageColorSpaceResourcesInspectionError {
    /// Caller-supplied root `/Pages` object offset.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root-node expansion failure.
    pub error: PageTreeKidTargetsInspectionError,
}

/// Inspect page-scope `/Resources /ColorSpace` entries through a classic xref
/// table.
///
/// Thin wrapper over [`inspect_document_page_color_space_resources_with_lookup`]
/// via [`ObjectLookup::ClassicXref`].
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-page resource
/// failures are recorded as structured page diagnostics.
pub fn inspect_document_page_color_space_resources(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
) -> Result<DocumentPageColorSpaceResourcesInspection, DocumentPageColorSpaceResourcesInspectionError>
{
    inspect_document_page_color_space_resources_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        root_node_object_offset,
    )
}

/// Inspect page-scope `/Resources /ColorSpace` entries through any object lookup
/// backend.
///
/// The walk follows the page tree root-down in document order, carrying the
/// effective inheritable `/Resources` dictionary exactly like the `/XObject`
/// resource pass. Each entry's colour-space definition is classified into a
/// small structural family model; unsupported or unresolvable shapes become
/// structured page-local skips. The returned entry vectors are sorted and
/// deduplicated by name for deterministic downstream inventory.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-child
/// page-tree failures and per-page resource failures are diagnostics in a
/// successful report.
pub fn inspect_document_page_color_space_resources_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
) -> Result<DocumentPageColorSpaceResourcesInspection, DocumentPageColorSpaceResourcesInspectionError>
{
    let root_targets =
        crate::inspect_page_tree_kid_targets_with_lookup(input, lookup, root_node_object_offset)
            .map_err(|error| DocumentPageColorSpaceResourcesInspectionError {
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
    let mut walk = ColorSpaceResourceWalk::new();
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

    Ok(DocumentPageColorSpaceResourcesInspection {
        byte_len: input.len(),
        pages: walk.pages,
        page_tree_skipped: walk.page_tree_skipped,
        visited_node_count: walk.visited_node_count,
        truncated: walk.truncated,
    })
}

struct ColorSpaceResourceWalk {
    pages: Vec<PageColorSpaceResourcesInspection>,
    page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    visited: BTreeSet<u32>,
    visited_node_count: usize,
    truncated: Option<PageTreeLeavesTruncation>,
}

impl ColorSpaceResourceWalk {
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

        let mut page =
            inspect_effective_color_spaces(input, lookup, page_object_byte_offset, &context);
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

fn inspect_effective_color_spaces(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page_object_byte_offset: usize,
    context: &ResourceContext,
) -> PageColorSpaceResourcesInspection {
    let mut skipped = context
        .skips
        .iter()
        .cloned()
        .map(|reason| skipped_entry(page_object_byte_offset, None, resources_skip(reason)))
        .collect::<Vec<_>>();
    let Some(resources) = &context.resources else {
        skipped.push(skipped_entry(
            page_object_byte_offset,
            None,
            SkippedColorSpaceResourceReason::MissingColorSpaceResources,
        ));
        return empty_page_report(page_object_byte_offset, skipped);
    };

    let Some(cs_entry) = (match unique_entry(input, &resources.entries, b"/ColorSpace") {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(skipped_entry(
                page_object_byte_offset,
                None,
                SkippedColorSpaceResourceReason::DuplicateColorSpace {
                    first_key_range,
                    duplicate_key_range,
                },
            ));
            return empty_page_report(page_object_byte_offset, skipped);
        }
    }) else {
        skipped.push(skipped_entry(
            page_object_byte_offset,
            None,
            SkippedColorSpaceResourceReason::MissingColorSpace,
        ));
        return empty_page_report(page_object_byte_offset, skipped);
    };

    if cs_entry.value_kind != DictionaryValueKind::Dictionary {
        skipped.push(skipped_entry(
            page_object_byte_offset,
            None,
            SkippedColorSpaceResourceReason::NonDictionaryColorSpace {
                value_kind: cs_entry.value_kind,
            },
        ));
        return empty_page_report(page_object_byte_offset, skipped);
    }

    let entries = match crate::inspect_dictionary_entries(input, cs_entry.value_range.start) {
        Ok(entries) => entries,
        Err(error) => {
            skipped.push(skipped_entry(
                page_object_byte_offset,
                None,
                SkippedColorSpaceResourceReason::ColorSpaceDictionaryFailed { error },
            ));
            return empty_page_report(page_object_byte_offset, skipped);
        }
    };

    let color_spaces = classify_color_space_entries(
        input,
        lookup,
        page_object_byte_offset,
        entries.entries,
        &mut skipped,
    );
    page_report(page_object_byte_offset, skipped, color_spaces)
}

fn classify_color_space_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page_object_byte_offset: usize,
    entries: Vec<DictionaryEntrySpan>,
    skipped: &mut Vec<SkippedColorSpaceResource>,
) -> Vec<ClassifiedColorSpaceResource> {
    let mut classified = Vec::new();
    let mut seen_names = BTreeMap::new();
    for entry in entries {
        let name = PdfName(input[entry.key_range.start + 1..entry.key_range.end].to_vec());
        if let Some(first_key_range) = seen_names.get(&name) {
            skipped.push(skipped_entry(
                page_object_byte_offset,
                Some(name),
                SkippedColorSpaceResourceReason::DuplicateColorSpaceName {
                    first_key_range: *first_key_range,
                    duplicate_key_range: entry.key_range,
                },
            ));
            continue;
        }
        seen_names.insert(name.clone(), entry.key_range);
        match classify_color_space_entry(input, lookup, &name, entry) {
            Ok(resource) => classified.push(resource),
            Err(reason) => {
                skipped.push(skipped_entry(page_object_byte_offset, Some(name), reason));
            }
        }
    }
    classified
}

fn page_report(
    page_object_byte_offset: usize,
    skipped: Vec<SkippedColorSpaceResource>,
    mut color_spaces: Vec<ClassifiedColorSpaceResource>,
) -> PageColorSpaceResourcesInspection {
    color_spaces.sort_by(|left, right| left.name.cmp(&right.name));
    PageColorSpaceResourcesInspection {
        ordinal: 0,
        page_reference: IndirectRef {
            object_number: 0,
            generation: 0,
        },
        page_object_byte_offset,
        color_spaces,
        skipped,
    }
}

fn empty_page_report(
    page_object_byte_offset: usize,
    skipped: Vec<SkippedColorSpaceResource>,
) -> PageColorSpaceResourcesInspection {
    page_report(page_object_byte_offset, skipped, Vec::new())
}

const fn skipped_entry(
    page_object_byte_offset: usize,
    resource_name: Option<PdfName>,
    reason: SkippedColorSpaceResourceReason,
) -> SkippedColorSpaceResource {
    SkippedColorSpaceResource {
        page_object_byte_offset,
        resource_name,
        reason,
    }
}

const fn resources_skip(
    reason: SkippedPageXObjectResourceReason,
) -> SkippedColorSpaceResourceReason {
    SkippedColorSpaceResourceReason::Resources {
        resources_reason: reason,
    }
}
