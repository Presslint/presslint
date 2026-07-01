use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::page_resource_inheritance::{
    ResolveReferenceError, ResourceContext, resolve_reference, unique_entry,
};
use crate::page_xobject_resource_targets::{
    ClassifiedPageXObjectResource, PageXObjectResourceSubtype,
};
use crate::{
    ClassicXrefTableInspection, DictionaryEntryByteRange, DictionaryEntryInspectionError,
    DictionaryEntrySpan, DictionaryValueKind, IndirectObjectDictionaryInspectionError, IndirectRef,
    IndirectReferenceInspectionRejection, ObjectLookup, ObjectLookupLocation,
    PageTreeKidTargetInspection, PageTreeKidTargetsInspection, PageTreeKidTargetsInspectionError,
    PageTreeLeavesTruncation, PageTreeNodeType, PageXObjectResourceTarget,
    SkippedPageTreeLeafEntry, SkippedPageTreeLeafReason, parse_indirect_reference,
};

/// PDF resource name represented as raw bytes without the leading slash.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PdfName(pub Vec<u8>);

/// Per-document page `XObject` resource classification.
///
/// This report stores only structural metadata, document-order per-page
/// `PdfName` lists, and small skip records. It does not retain PDF bytes, object
/// bodies, resource dictionaries, stream dictionaries, stream bodies, decoded
/// data, or source slices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageXObjectResourcesInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Document-ordered page resource reports.
    pub pages: Vec<PageXObjectResourcesInspection>,
    /// Ordered page-tree traversal skips for children that were not leaf pages.
    pub page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    /// Number of `/Pages` nodes expanded during the walk.
    pub visited_node_count: usize,
    /// First traversal bound that stopped a descent, when any.
    pub truncated: Option<PageTreeLeavesTruncation>,
}

impl DocumentPageXObjectResourcesInspection {
    /// Count of inspected leaf pages.
    #[must_use]
    pub const fn page_count(&self) -> usize {
        self.pages.len()
    }
}

/// Per-page classified page-scope `XObject` resource targets and legacy names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageXObjectResourcesInspection {
    /// Zero-based document-order page ordinal.
    pub ordinal: usize,
    /// Indirect reference of the leaf `/Page`.
    pub page_reference: IndirectRef,
    /// Resolved page object byte offset.
    pub page_object_byte_offset: usize,
    /// Sorted/deduplicated targets whose resolved `XObject` subtype is `/Image`.
    pub image_xobjects: Vec<PageXObjectResourceTarget>,
    /// Sorted/deduplicated targets whose resolved `XObject` subtype is `/Form`.
    pub form_xobjects: Vec<PageXObjectResourceTarget>,
    /// Sorted/deduplicated names whose resolved `XObject` subtype is `/Image`.
    pub image_xobject_names: Vec<PdfName>,
    /// Sorted/deduplicated names whose resolved `XObject` subtype is `/Form`.
    pub form_xobject_names: Vec<PdfName>,
    /// Page-local structural resource diagnostics.
    pub skipped: Vec<SkippedPageXObjectResource>,
}

/// One page-local `XObject` resource diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPageXObjectResource {
    /// Resolved leaf page object byte offset.
    pub page_object_byte_offset: usize,
    /// Resource name when the diagnostic concerns one `/XObject` entry.
    pub resource_name: Option<PdfName>,
    /// Structured skip reason.
    pub reason: SkippedPageXObjectResourceReason,
}

/// Structured reason a page-scope `XObject` resource was not classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedPageXObjectResourceReason {
    /// No effective `/Resources` dictionary was available for this page.
    MissingResources,
    /// A `/Resources` key occurred more than once.
    DuplicateResources {
        /// First `/Resources` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Resources` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// A `/Resources` value was neither a direct dictionary nor an indirect reference.
    UnsupportedResourcesValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// An indirect `/Resources` value was shaped like a reference but malformed.
    MalformedResourcesReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// An indirect `/Resources` reference did not resolve to an in-use object.
    UnresolvedResourcesReference {
        /// Requested indirect resource dictionary reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number.
        location: ObjectLookupLocation,
    },
    /// An indirect `/Resources` reference resolved with a mismatched generation.
    ResourcesGenerationMismatch {
        /// Requested indirect resource dictionary reference.
        reference: IndirectRef,
        /// Generation number from the matching in-use xref entry.
        xref_generation: u16,
    },
    /// A direct `/Resources` dictionary could not be scanned.
    DirectResourcesDictionaryFailed {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// An indirect `/Resources` dictionary could not be scanned.
    IndirectResourcesDictionaryFailed {
        /// Requested indirect resource dictionary reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// The page dictionary itself could not be scanned for `/Resources`.
    PageDictionaryFailed {
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// No effective `/XObject` dictionary was present in the effective resources.
    MissingXObject,
    /// An effective `/XObject` key occurred more than once.
    DuplicateXObject {
        /// First `/XObject` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/XObject` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The effective `/XObject` value was not a direct dictionary.
    NonDictionaryXObject {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The direct `/XObject` dictionary could not be scanned.
    XObjectDictionaryFailed {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// A direct `/XObject` dictionary repeated the same resource name.
    DuplicateXObjectName {
        /// First matching resource-name key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching resource-name key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// An `/XObject` entry value was not an indirect reference.
    NonReferenceXObject {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// An `/XObject` entry was shaped like a reference but malformed.
    MalformedXObjectReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// An `/XObject` entry reference did not resolve to an in-use object.
    UnresolvedXObjectReference {
        /// Requested `XObject` target reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number.
        location: ObjectLookupLocation,
    },
    /// An `/XObject` entry resolved with a mismatched generation.
    XObjectGenerationMismatch {
        /// Requested `XObject` target reference.
        reference: IndirectRef,
        /// Generation number from the matching in-use xref entry.
        xref_generation: u16,
    },
    /// The resolved `XObject` target was not a dictionary-bodied object.
    XObjectTargetDictionaryFailed {
        /// Requested `XObject` target reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// The target dictionary had no `/Subtype`.
    MissingSubtype {
        /// Resolved `XObject` target object byte offset.
        object_byte_offset: usize,
    },
    /// The target dictionary had duplicate `/Subtype` keys.
    DuplicateSubtype {
        /// Resolved `XObject` target object byte offset.
        object_byte_offset: usize,
        /// First `/Subtype` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Subtype` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The target `/Subtype` was not a name.
    NonNameSubtype {
        /// Resolved `XObject` target object byte offset.
        object_byte_offset: usize,
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The target `/Subtype` was a name other than `/Image` or `/Form`.
    UnknownSubtype {
        /// Resolved `XObject` target object byte offset.
        object_byte_offset: usize,
        /// Raw subtype name bytes including the leading slash.
        subtype: Vec<u8>,
    },
}

/// Error returned when page `XObject` resource inspection cannot begin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentPageXObjectResourcesInspectionError {
    /// Caller-supplied root `/Pages` object offset.
    pub root_node_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Delegated root-node expansion failure.
    pub error: PageTreeKidTargetsInspectionError,
}

/// Inspect page-scope `XObject` resource targets through a classic xref table.
///
/// This is a thin wrapper over
/// [`inspect_document_page_xobject_resources_with_lookup`] via
/// [`ObjectLookup::ClassicXref`].
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-page resource
/// failures are recorded as structured page diagnostics.
pub fn inspect_document_page_xobject_resources(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    root_node_object_offset: usize,
) -> Result<DocumentPageXObjectResourcesInspection, DocumentPageXObjectResourcesInspectionError> {
    inspect_document_page_xobject_resources_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        root_node_object_offset,
    )
}

/// Inspect page-scope `XObject` resource targets through any object lookup backend.
///
/// The walk follows the page tree root-down in document order, carrying the
/// effective inheritable `/Resources` dictionary. A child page or page-tree node
/// `/Resources` entry replaces the inherited dictionary for this slice.
///
/// Only direct `/XObject` dictionaries are scanned. Each direct `/XObject`
/// entry must be an indirect reference whose resolved target dictionary has
/// `/Subtype /Image` or `/Subtype /Form`; every other shape becomes a
/// structured page-local skip. The returned target and name vectors are sorted
/// and deduplicated for deterministic downstream inventory.
///
/// # Errors
///
/// Returns an error only when root page-tree expansion fails. Per-child
/// page-tree failures and per-page resource failures are diagnostics in a
/// successful report.
pub fn inspect_document_page_xobject_resources_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
) -> Result<DocumentPageXObjectResourcesInspection, DocumentPageXObjectResourcesInspectionError> {
    let root_targets =
        crate::inspect_page_tree_kid_targets_with_lookup(input, lookup, root_node_object_offset)
            .map_err(|error| DocumentPageXObjectResourcesInspectionError {
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
    let mut walk = XObjectResourceWalk::new();
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

    Ok(DocumentPageXObjectResourcesInspection {
        byte_len: input.len(),
        pages: walk.pages,
        page_tree_skipped: walk.page_tree_skipped,
        visited_node_count: walk.visited_node_count,
        truncated: walk.truncated,
    })
}

struct XObjectResourceWalk {
    pages: Vec<PageXObjectResourcesInspection>,
    page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    visited: BTreeSet<u32>,
    visited_node_count: usize,
    truncated: Option<PageTreeLeavesTruncation>,
}

impl XObjectResourceWalk {
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
        let context =
            match crate::inspect_indirect_object_dictionary(input, page_object_byte_offset) {
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

        let mut page = inspect_effective_xobjects(input, lookup, page_object_byte_offset, &context);
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

fn inspect_effective_xobjects(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page_object_byte_offset: usize,
    context: &ResourceContext,
) -> PageXObjectResourcesInspection {
    let mut skipped = context
        .skips
        .iter()
        .cloned()
        .map(|reason| skipped_page(page_object_byte_offset, None, reason))
        .collect::<Vec<_>>();
    let Some(resources) = &context.resources else {
        skipped.push(skipped_page(
            page_object_byte_offset,
            None,
            SkippedPageXObjectResourceReason::MissingResources,
        ));
        return empty_page_report(page_object_byte_offset, skipped);
    };

    let Some(xobject_entry) = (match unique_entry(input, &resources.entries, b"/XObject") {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(skipped_page(
                page_object_byte_offset,
                None,
                SkippedPageXObjectResourceReason::DuplicateXObject {
                    first_key_range,
                    duplicate_key_range,
                },
            ));
            return empty_page_report(page_object_byte_offset, skipped);
        }
    }) else {
        skipped.push(skipped_page(
            page_object_byte_offset,
            None,
            SkippedPageXObjectResourceReason::MissingXObject,
        ));
        return empty_page_report(page_object_byte_offset, skipped);
    };

    if xobject_entry.value_kind != DictionaryValueKind::Dictionary {
        skipped.push(skipped_page(
            page_object_byte_offset,
            None,
            SkippedPageXObjectResourceReason::NonDictionaryXObject {
                value_kind: xobject_entry.value_kind,
            },
        ));
        return empty_page_report(page_object_byte_offset, skipped);
    }

    let xobjects = match crate::inspect_dictionary_entries(input, xobject_entry.value_range.start) {
        Ok(xobjects) => xobjects,
        Err(error) => {
            skipped.push(skipped_page(
                page_object_byte_offset,
                None,
                SkippedPageXObjectResourceReason::XObjectDictionaryFailed { error },
            ));
            return empty_page_report(page_object_byte_offset, skipped);
        }
    };

    let (images, forms) = classify_xobject_entries(
        input,
        lookup,
        page_object_byte_offset,
        xobjects.entries,
        &mut skipped,
    );

    page_report(page_object_byte_offset, skipped, images, forms)
}

fn classify_xobject_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page_object_byte_offset: usize,
    entries: Vec<DictionaryEntrySpan>,
    skipped: &mut Vec<SkippedPageXObjectResource>,
) -> (
    Vec<PageXObjectResourceTarget>,
    Vec<PageXObjectResourceTarget>,
) {
    let mut images = Vec::new();
    let mut forms = Vec::new();
    let mut seen_names = BTreeMap::new();
    for entry in entries {
        let name = PdfName(input[entry.key_range.start + 1..entry.key_range.end].to_vec());
        if let Some(first_key_range) = seen_names.get(&name) {
            skipped.push(skipped_page(
                page_object_byte_offset,
                Some(name),
                SkippedPageXObjectResourceReason::DuplicateXObjectName {
                    first_key_range: *first_key_range,
                    duplicate_key_range: entry.key_range,
                },
            ));
            continue;
        }
        seen_names.insert(name.clone(), entry.key_range);
        match classify_xobject_entry(input, lookup, entry) {
            Ok(classification) => {
                let target = PageXObjectResourceTarget {
                    name,
                    reference: classification.reference,
                    object_byte_offset: classification.object_byte_offset,
                    image_metadata: classification.image_metadata,
                };
                match classification.subtype {
                    PageXObjectResourceSubtype::Image => {
                        images.push(target);
                    }
                    PageXObjectResourceSubtype::Form => {
                        forms.push(target);
                    }
                }
            }
            Err(reason) => skipped.push(skipped_page(page_object_byte_offset, Some(name), reason)),
        }
    }
    (images, forms)
}

fn page_report(
    page_object_byte_offset: usize,
    skipped: Vec<SkippedPageXObjectResource>,
    mut images: Vec<PageXObjectResourceTarget>,
    mut forms: Vec<PageXObjectResourceTarget>,
) -> PageXObjectResourcesInspection {
    images.sort_by(|left, right| left.name.cmp(&right.name));
    forms.sort_by(|left, right| left.name.cmp(&right.name));
    let image_xobject_names = images.iter().map(|target| target.name.clone()).collect();
    let form_xobject_names = forms.iter().map(|target| target.name.clone()).collect();
    PageXObjectResourcesInspection {
        ordinal: 0,
        page_reference: IndirectRef {
            object_number: 0,
            generation: 0,
        },
        page_object_byte_offset,
        image_xobjects: images,
        form_xobjects: forms,
        image_xobject_names,
        form_xobject_names,
        skipped,
    }
}

fn empty_page_report(
    page_object_byte_offset: usize,
    skipped: Vec<SkippedPageXObjectResource>,
) -> PageXObjectResourcesInspection {
    page_report(page_object_byte_offset, skipped, Vec::new(), Vec::new())
}

#[derive(Debug, Clone, Copy)]
struct ChildPagesNode {
    reference: IndirectRef,
    object_byte_offset: usize,
    parent_node_byte_offset: usize,
}

const fn skipped_page(
    page_object_byte_offset: usize,
    resource_name: Option<PdfName>,
    reason: SkippedPageXObjectResourceReason,
) -> SkippedPageXObjectResource {
    SkippedPageXObjectResource {
        page_object_byte_offset,
        resource_name,
        reason,
    }
}

fn classify_xobject_entry(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    entry: DictionaryEntrySpan,
) -> Result<ClassifiedPageXObjectResource, SkippedPageXObjectResourceReason> {
    if entry.value_kind != DictionaryValueKind::IndirectReferenceLike {
        return Err(SkippedPageXObjectResourceReason::NonReferenceXObject {
            value_kind: entry.value_kind,
        });
    }
    let reference = parse_indirect_reference(input, entry.value_range.start).map_err(|error| {
        SkippedPageXObjectResourceReason::MalformedXObjectReference {
            reference_reason: error.reason,
        }
    })?;
    let (_, object_byte_offset) =
        resolve_reference(lookup, reference.reference).map_err(|reason| match reason {
            ResolveReferenceError::Unresolved {
                reference,
                location,
            } => SkippedPageXObjectResourceReason::UnresolvedXObjectReference {
                reference,
                location,
            },
            ResolveReferenceError::GenerationMismatch {
                reference,
                xref_generation,
            } => SkippedPageXObjectResourceReason::XObjectGenerationMismatch {
                reference,
                xref_generation,
            },
        })?;

    let target =
        crate::inspect_indirect_object_dictionary(input, object_byte_offset).map_err(|error| {
            SkippedPageXObjectResourceReason::XObjectTargetDictionaryFailed {
                reference: reference.reference,
                object_byte_offset,
                error,
            }
        })?;
    let subtype = classify_subtype(input, object_byte_offset, &target.entries)?;
    let image_metadata = subtype.image_metadata(input, &target.entries);
    Ok(ClassifiedPageXObjectResource {
        subtype,
        reference: reference.reference,
        object_byte_offset,
        image_metadata,
    })
}

fn classify_subtype(
    input: &[u8],
    object_byte_offset: usize,
    entries: &[DictionaryEntrySpan],
) -> Result<PageXObjectResourceSubtype, SkippedPageXObjectResourceReason> {
    let Some(subtype) = unique_entry(input, entries, b"/Subtype").map_err(
        |(first_key_range, duplicate_key_range)| {
            SkippedPageXObjectResourceReason::DuplicateSubtype {
                object_byte_offset,
                first_key_range,
                duplicate_key_range,
            }
        },
    )?
    else {
        return Err(SkippedPageXObjectResourceReason::MissingSubtype { object_byte_offset });
    };

    if subtype.value_kind != DictionaryValueKind::Name {
        return Err(SkippedPageXObjectResourceReason::NonNameSubtype {
            object_byte_offset,
            value_kind: subtype.value_kind,
        });
    }

    match &input[subtype.value_range.start..subtype.value_range.end] {
        b"/Image" => Ok(PageXObjectResourceSubtype::Image),
        b"/Form" => Ok(PageXObjectResourceSubtype::Form),
        other => Err(SkippedPageXObjectResourceReason::UnknownSubtype {
            object_byte_offset,
            subtype: other.to_vec(),
        }),
    }
}
