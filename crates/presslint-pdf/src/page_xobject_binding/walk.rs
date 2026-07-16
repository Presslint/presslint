//! Private descent, container resolution, and consumer-veto internals for the
//! page `/XObject` binding witness.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::source_utils::{consume_keyword, decode_pdf_name, skip_whitespace_and_comments};
use crate::{
    DictionaryEntryByteRange, DictionaryEntrySpan, DictionaryValueKind,
    IndirectObjectDictionaryInspection, IndirectObjectDictionaryInspectionError,
    IndirectObjectOwnership, IndirectRef, ObjectConsumerIndexInspection, ObjectConsumerReferrer,
    ObjectConsumersEntry, ObjectLookup, ObjectLookupLocation, PageTreeKidTargetInspection,
    PageTreeKidTargetsInspection, PageTreeLeavesTruncation, PageTreeNodeType, PdfName,
    SkippedObjectConsumerScan, SkippedPageTreeLeafEntry, SkippedPageTreeLeafReason,
    decide_indirect_object_edit, inspect_dictionary_entries, inspect_indirect_object_dictionary,
    inspect_indirect_object_header, locate_xref_object, parse_indirect_reference,
};

use super::{
    BindingContainer, BindingContainerLocality, BindingResourcesSource,
    DocumentPageXObjectBindingsInspection, DocumentPageXObjectBindingsInspectionError,
    PageXObjectBindingRefusal, PageXObjectBindingUnprovenReason, PageXObjectBindingVerdict,
    PageXObjectBindingWitness, PageXObjectBindingsInspection, RefusedPageXObjectBinding,
    XObjectBindingPath, XObjectBindingSubtype,
};

pub(super) fn run(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
    consumers: &ObjectConsumerIndexInspection,
) -> Result<DocumentPageXObjectBindingsInspection, DocumentPageXObjectBindingsInspectionError> {
    let root_targets =
        crate::inspect_page_tree_kid_targets_with_lookup(input, lookup, root_node_object_offset)
            .map_err(|error| DocumentPageXObjectBindingsInspectionError {
                root_node_byte_offset: root_node_object_offset,
                byte_len: input.len(),
                error,
            })?;

    let env = WalkEnv {
        input,
        lookup,
        veto: ConsumerVeto::new(consumers),
    };
    let root_state = resources_state(
        &env,
        &root_targets.kids.node.node_dictionary,
        &BindingResourcesState::Absent,
    );
    let mut walk = BindingWalk::new();
    walk.visited.insert(
        root_targets
            .kids
            .node
            .node_dictionary
            .reference
            .object_number,
    );
    walk.visited_node_count = 1;
    walk.process_node(&env, &root_targets, &root_state, 0);

    Ok(DocumentPageXObjectBindingsInspection {
        byte_len: input.len(),
        pages: walk.pages,
        page_tree_skipped: walk.page_tree_skipped,
        visited_node_count: walk.visited_node_count,
        truncated: walk.truncated,
    })
}

/// Borrowed walk environment shared by every node and entry step.
#[derive(Clone, Copy)]
struct WalkEnv<'a, 'c> {
    input: &'a [u8],
    lookup: ObjectLookup<'a>,
    veto: ConsumerVeto<'c>,
}

/// Lookup-only exclusivity veto over the borrowed consumer index.
///
/// Completeness is read from the inspection's own fact vectors; lookups are
/// binary searches over its sorted entries. No re-traversal, no new index.
#[derive(Clone, Copy)]
struct ConsumerVeto<'c> {
    entries: &'c [ObjectConsumersEntry],
    complete: bool,
}

impl<'c> ConsumerVeto<'c> {
    fn new(consumers: &'c ObjectConsumerIndexInspection) -> Self {
        let complete = consumers.truncations.is_empty()
            && consumers.unresolved_edges.is_empty()
            && consumers.skipped.iter().all(|skip| {
                matches!(
                    skip,
                    SkippedObjectConsumerScan::UnreferencedEntryUnresolvable { .. }
                )
            });
        Self {
            entries: &consumers.entries,
            complete,
        }
    }

    fn referrers(&self, target: IndirectRef) -> &'c [ObjectConsumerReferrer] {
        self.entries
            .binary_search_by(|entry| entry.target.cmp(&target))
            .map_or(&[][..], |index| self.entries[index].referrers.as_slice())
    }

    /// Whether the index shows exactly this page as the single typed consumer.
    fn exclusive_to_page(&self, target: IndirectRef, ordinal: usize, page: IndirectRef) -> bool {
        matches!(
            self.referrers(target),
            [ObjectConsumerReferrer::Page { page_index, page: referrer_page }]
                if *referrer_page == page && *page_index == ordinal
        )
    }
}

/// Effective `/Resources` state carried root-down (Table 30 whole-value
/// replacement; a node-level refusal poisons descent fail-closed until a
/// deeper node replaces the value).
#[derive(Clone)]
enum BindingResourcesState {
    Absent,
    Poisoned(Box<PageXObjectBindingRefusal>),
    Resolved(Rc<ResolvedBindingResources>),
}

/// One resolved effective `/Resources` value with provenance and locality.
struct ResolvedBindingResources {
    defining_node: IndirectRef,
    key_range: DictionaryEntryByteRange,
    value_range: DictionaryEntryByteRange,
    locality: BindingContainerLocality,
    entries: Vec<DictionaryEntrySpan>,
}

struct BindingWalk {
    pages: Vec<PageXObjectBindingsInspection>,
    page_tree_skipped: Vec<SkippedPageTreeLeafEntry>,
    visited: BTreeSet<u32>,
    visited_node_count: usize,
    truncated: Option<PageTreeLeavesTruncation>,
}

impl BindingWalk {
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
        env: &WalkEnv<'_, '_>,
        targets: &PageTreeKidTargetsInspection,
        state: &BindingResourcesState,
        depth: usize,
    ) {
        let node_byte_offset = targets.kids.node.node_dictionary.header_range.start;
        for entry in &targets.entries {
            match entry {
                PageTreeKidTargetInspection::Resolved { kid, target } => {
                    match target.node_type.node_type {
                        PageTreeNodeType::Page => {
                            self.inspect_page(
                                env,
                                kid.reference,
                                target.object_byte_offset,
                                &target.node_type.object_dictionary,
                                state,
                            );
                        }
                        PageTreeNodeType::Pages => self.descend_into_child(
                            env,
                            ChildPagesNode {
                                reference: kid.reference,
                                object_byte_offset: target.object_byte_offset,
                                parent_node_byte_offset: node_byte_offset,
                            },
                            state,
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

    fn descend_into_child(
        &mut self,
        env: &WalkEnv<'_, '_>,
        child: ChildPagesNode,
        inherited: &BindingResourcesState,
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
            env.input,
            env.lookup,
            child.object_byte_offset,
        ) {
            Ok(child_targets) => {
                let state =
                    resources_state(env, &child_targets.kids.node.node_dictionary, inherited);
                self.process_node(env, &child_targets, &state, child_depth);
            }
            Err(error) => self.push_page_tree_skip(
                child.reference,
                child.parent_node_byte_offset,
                SkippedPageTreeLeafReason::NodeExpansionFailed { error },
            ),
        }
    }

    fn inspect_page(
        &mut self,
        env: &WalkEnv<'_, '_>,
        page_reference: IndirectRef,
        page_object_byte_offset: usize,
        dictionary: &IndirectObjectDictionaryInspection,
        inherited: &BindingResourcesState,
    ) {
        let ordinal = self.pages.len();
        let mut page = PageXObjectBindingsInspection {
            ordinal,
            page_reference,
            page_object_byte_offset,
            witnesses: Vec::new(),
            refused: Vec::new(),
        };

        let state = resources_state(env, dictionary, inherited);
        match state {
            BindingResourcesState::Absent => page.refused.push(refusal(
                page_object_byte_offset,
                None,
                PageXObjectBindingRefusal::MissingResources,
            )),
            BindingResourcesState::Poisoned(reason) => {
                page.refused
                    .push(refusal(page_object_byte_offset, None, *reason));
            }
            BindingResourcesState::Resolved(resources) => {
                inspect_bindings(env, dictionary, &resources, &mut page);
            }
        }

        page.witnesses
            .sort_by(|left, right| left.name.cmp(&right.name));
        self.pages.push(page);
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

/// Compute one node's effective `/Resources` state from its dictionary and
/// the inherited state (Table 30 whole-value replacement).
fn resources_state(
    env: &WalkEnv<'_, '_>,
    dictionary: &IndirectObjectDictionaryInspection,
    inherited: &BindingResourcesState,
) -> BindingResourcesState {
    let entry = match unique_semantic_entry(env.input, &dictionary.entries, b"Resources") {
        // ISO 32000-1 §7.3.9: a dictionary entry whose value is `null` is
        // equivalent to an absent entry, so ancestor resources stay
        // effective.
        Ok(Some(entry)) if entry.value_kind == DictionaryValueKind::Null => {
            return inherited.clone();
        }
        Ok(Some(entry)) => entry,
        Ok(None) => return inherited.clone(),
        Err((first_key_range, duplicate_key_range)) => {
            return BindingResourcesState::Poisoned(Box::new(
                PageXObjectBindingRefusal::DuplicateContainerKey {
                    container: BindingContainer::Resources,
                    defining_node: dictionary.reference,
                    first_key_range,
                    duplicate_key_range,
                },
            ));
        }
    };

    match resolve_container(
        env,
        BindingContainer::Resources,
        dictionary.reference,
        entry,
    ) {
        Ok((locality, entries)) => {
            BindingResourcesState::Resolved(Rc::new(ResolvedBindingResources {
                defining_node: dictionary.reference,
                key_range: entry.key_range,
                value_range: entry.value_range,
                locality,
                entries,
            }))
        }
        Err(reason) => BindingResourcesState::Poisoned(Box::new(reason)),
    }
}

/// Resolve one container value (direct dictionary or uncompressed indirect
/// dictionary object with header-identity corroboration BEFORE body
/// validation) into its locality and top-level entry spans. Stream objects
/// refuse: binding containers require dictionary objects.
fn resolve_container(
    env: &WalkEnv<'_, '_>,
    container: BindingContainer,
    defining_node: IndirectRef,
    entry: DictionaryEntrySpan,
) -> Result<(BindingContainerLocality, Vec<DictionaryEntrySpan>), PageXObjectBindingRefusal> {
    match entry.value_kind {
        DictionaryValueKind::Dictionary => {
            match inspect_dictionary_entries(env.input, entry.value_range.start) {
                Ok(entries) => Ok((BindingContainerLocality::DirectDictionary, entries.entries)),
                Err(error) => Err(PageXObjectBindingRefusal::DirectContainerDictionaryFailed {
                    container,
                    defining_node,
                    error,
                }),
            }
        }
        DictionaryValueKind::IndirectReferenceLike => {
            let reference = parse_indirect_reference(env.input, entry.value_range.start)
                .map_err(
                    |error| PageXObjectBindingRefusal::MalformedContainerReference {
                        container,
                        defining_node,
                        reference_reason: error.reason,
                    },
                )?
                .reference;
            let object_byte_offset = match locate_uncompressed(env.lookup, reference) {
                LocateOutcome::Uncompressed { byte_offset } => byte_offset,
                LocateOutcome::GenerationMismatch { xref_generation } => {
                    return Err(PageXObjectBindingRefusal::ContainerGenerationMismatch {
                        container,
                        defining_node,
                        reference,
                        xref_generation,
                    });
                }
                LocateOutcome::Compressed {
                    object_stream_number,
                    index_within_object_stream,
                } => {
                    return Err(PageXObjectBindingRefusal::CompressedContainer {
                        container,
                        defining_node,
                        reference,
                        object_stream_number,
                        index_within_object_stream,
                    });
                }
                LocateOutcome::Unresolved { location } => {
                    return Err(PageXObjectBindingRefusal::UnresolvedContainer {
                        container,
                        defining_node,
                        reference,
                        location,
                    });
                }
            };
            let dictionary =
                match corroborated_object_dictionary(env.input, reference, object_byte_offset) {
                    Ok(dictionary) => dictionary,
                    Err(CorroborationFailure::IdentityMismatch { header_reference }) => {
                        return Err(PageXObjectBindingRefusal::ContainerIdentityMismatch {
                            container,
                            defining_node,
                            reference,
                            object_byte_offset,
                            header_reference,
                        });
                    }
                    Err(CorroborationFailure::DictionaryFailed { error }) => {
                        return Err(
                            PageXObjectBindingRefusal::IndirectContainerDictionaryFailed {
                                container,
                                defining_node,
                                reference,
                                object_byte_offset,
                                error,
                            },
                        );
                    }
                };
            // Binding containers require dictionary OBJECTS (ISO 32000-1
            // §7.8.3); the dictionary portion of a stream is never admitted.
            if stream_keyword_follows(env.input, dictionary.after_dictionary_close_byte_offset) {
                return Err(PageXObjectBindingRefusal::StreamContainer {
                    container,
                    defining_node,
                    reference,
                    object_byte_offset,
                });
            }
            Ok((
                BindingContainerLocality::IndirectResolved {
                    reference,
                    object_byte_offset,
                },
                dictionary.entries,
            ))
        }
        value_kind => Err(PageXObjectBindingRefusal::UnsupportedContainerValue {
            container,
            defining_node,
            value_kind,
        }),
    }
}

/// Locate-only resolution keeping compressed, mismatched, and unresolvable
/// outcomes distinct so no offset is ever fabricated.
enum LocateOutcome {
    Uncompressed {
        byte_offset: usize,
    },
    GenerationMismatch {
        xref_generation: u16,
    },
    Compressed {
        object_stream_number: usize,
        index_within_object_stream: usize,
    },
    Unresolved {
        location: ObjectLookupLocation,
    },
}

fn locate_uncompressed(lookup: ObjectLookup<'_>, reference: IndirectRef) -> LocateOutcome {
    let object_number = usize::try_from(reference.object_number).map_or(usize::MAX, |value| value);
    match locate_xref_object(lookup, object_number) {
        ObjectLookupLocation::ClassicInUse {
            generation,
            byte_offset,
            ..
        }
        | ObjectLookupLocation::XrefStreamUncompressed {
            generation,
            byte_offset,
            ..
        } => {
            if generation == reference.generation {
                LocateOutcome::Uncompressed { byte_offset }
            } else {
                LocateOutcome::GenerationMismatch {
                    xref_generation: generation,
                }
            }
        }
        ObjectLookupLocation::XrefStreamCompressed {
            object_stream_number,
            index_within_object_stream,
            ..
        } => {
            if reference.generation == 0 {
                LocateOutcome::Compressed {
                    object_stream_number,
                    index_within_object_stream,
                }
            } else {
                LocateOutcome::GenerationMismatch { xref_generation: 0 }
            }
        }
        location => LocateOutcome::Unresolved { location },
    }
}

/// Failure of header-corroborated dictionary inspection at a reached offset.
enum CorroborationFailure {
    IdentityMismatch {
        header_reference: IndirectRef,
    },
    DictionaryFailed {
        error: IndirectObjectDictionaryInspectionError,
    },
}

/// Inspect a reached object's dictionary with the header identity
/// corroborated BEFORE body validation, so a mismatched xref offset
/// classifies as an identity mismatch even when the reached body is
/// non-dictionary or malformed.
fn corroborated_object_dictionary(
    input: &[u8],
    reference: IndirectRef,
    object_byte_offset: usize,
) -> Result<IndirectObjectDictionaryInspection, CorroborationFailure> {
    // A header inspection failure falls through: the full dictionary
    // inspection re-derives it as its structured Header rejection.
    if let Ok(header) = inspect_indirect_object_header(input, object_byte_offset)
        && header.reference != reference
    {
        return Err(CorroborationFailure::IdentityMismatch {
            header_reference: header.reference,
        });
    }
    inspect_indirect_object_dictionary(input, object_byte_offset)
        .map_err(|error| CorroborationFailure::DictionaryFailed { error })
}

/// Whether the next token after a dictionary close is the `stream` keyword,
/// i.e. the reached object is a stream rather than a dictionary object.
fn stream_keyword_follows(input: &[u8], after_dictionary_close: usize) -> bool {
    let limit = input.len();
    let cursor = skip_whitespace_and_comments(input, after_dictionary_close.min(limit), limit);
    consume_keyword(&input[cursor..], b"stream").is_some()
}

/// Inspect the effective `/XObject` subdictionary of one leaf page.
fn inspect_bindings(
    env: &WalkEnv<'_, '_>,
    leaf_dictionary: &IndirectObjectDictionaryInspection,
    resources: &ResolvedBindingResources,
    page: &mut PageXObjectBindingsInspection,
) {
    let resources_source = if resources.defining_node == leaf_dictionary.reference {
        BindingResourcesSource::Direct {
            target: resources.defining_node,
            key_range: resources.key_range,
            value_range: resources.value_range,
        }
    } else {
        BindingResourcesSource::Inherited {
            ancestor: resources.defining_node,
            key_range: resources.key_range,
            value_range: resources.value_range,
        }
    };

    // The dictionary owning the `/XObject` entry: the page-tree node itself
    // for a direct `/Resources`, the resolved container object when indirect.
    let xobject_owner = match resources.locality {
        BindingContainerLocality::DirectDictionary => resources.defining_node,
        BindingContainerLocality::IndirectResolved { reference, .. } => reference,
    };

    let xobject_entry = match unique_semantic_entry(env.input, &resources.entries, b"XObject") {
        // ISO 32000-1 §7.3.9: a `null` value is equivalent to an absent
        // entry, so it classifies as the same missing-key absence.
        Ok(Some(entry)) if entry.value_kind != DictionaryValueKind::Null => entry,
        Ok(Some(_) | None) => {
            page.refused.push(refusal(
                page.page_object_byte_offset,
                None,
                PageXObjectBindingRefusal::MissingXObject {
                    defining_node: xobject_owner,
                },
            ));
            return;
        }
        Err((first_key_range, duplicate_key_range)) => {
            page.refused.push(refusal(
                page.page_object_byte_offset,
                None,
                PageXObjectBindingRefusal::DuplicateContainerKey {
                    container: BindingContainer::XObjectDictionary,
                    defining_node: xobject_owner,
                    first_key_range,
                    duplicate_key_range,
                },
            ));
            return;
        }
    };

    let (xobject_locality, entries) = match resolve_container(
        env,
        BindingContainer::XObjectDictionary,
        xobject_owner,
        xobject_entry,
    ) {
        Ok(resolved) => resolved,
        Err(reason) => {
            page.refused
                .push(refusal(page.page_object_byte_offset, None, reason));
            return;
        }
    };

    let path = XObjectBindingPath {
        resources_source,
        resources_locality: resources.locality,
        xobject_key_range: xobject_entry.key_range,
        xobject_value_range: xobject_entry.value_range,
        xobject_locality,
    };
    inspect_entries(env, &path, &entries, page);
}

/// Classify every entry of the effective `/XObject` subdictionary with
/// decoded-name collision poisoning of EVERY colliding entry.
fn inspect_entries(
    env: &WalkEnv<'_, '_>,
    path: &XObjectBindingPath,
    entries: &[DictionaryEntrySpan],
    page: &mut PageXObjectBindingsInspection,
) {
    let mut malformed = vec![false; entries.len()];
    let mut groups: BTreeMap<Cow<'_, [u8]>, Vec<usize>> = BTreeMap::new();
    for (index, entry) in entries.iter().enumerate() {
        let raw = &env.input[entry.key_range.start + 1..entry.key_range.end];
        match decode_pdf_name(raw) {
            Some(decoded) => groups.entry(decoded).or_default().push(index),
            None => malformed[index] = true,
        }
    }
    let mut collision_ranges: BTreeMap<usize, Vec<DictionaryEntryByteRange>> = BTreeMap::new();
    for indices in groups.values().filter(|indices| indices.len() > 1) {
        let ranges: Vec<_> = indices
            .iter()
            .map(|&index| entries[index].key_range)
            .collect();
        for &index in indices {
            collision_ranges.insert(index, ranges.clone());
        }
    }

    for (index, entry) in entries.iter().enumerate() {
        let name = PdfName(env.input[entry.key_range.start + 1..entry.key_range.end].to_vec());
        if malformed[index] {
            page.refused.push(refusal(
                page.page_object_byte_offset,
                Some(name),
                PageXObjectBindingRefusal::MalformedEntryName {
                    key_range: entry.key_range,
                },
            ));
            continue;
        }
        if let Some(colliding_key_ranges) = collision_ranges.get(&index) {
            page.refused.push(refusal(
                page.page_object_byte_offset,
                Some(name),
                PageXObjectBindingRefusal::EntryNameCollision {
                    colliding_key_ranges: colliding_key_ranges.clone(),
                },
            ));
            continue;
        }
        match witness_entry(
            env,
            path,
            page.ordinal,
            page.page_reference,
            name.clone(),
            *entry,
        ) {
            Ok(witness) => page.witnesses.push(witness),
            Err(reason) => {
                page.refused
                    .push(refusal(page.page_object_byte_offset, Some(name), reason));
            }
        }
    }
}

/// Resolve and corroborate one unique-named entry into a binding witness.
fn witness_entry(
    env: &WalkEnv<'_, '_>,
    path: &XObjectBindingPath,
    ordinal: usize,
    page_reference: IndirectRef,
    name: PdfName,
    entry: DictionaryEntrySpan,
) -> Result<PageXObjectBindingWitness, PageXObjectBindingRefusal> {
    if entry.value_kind != DictionaryValueKind::IndirectReferenceLike {
        return Err(PageXObjectBindingRefusal::NonReferenceEntry {
            value_kind: entry.value_kind,
        });
    }
    let target = parse_indirect_reference(env.input, entry.value_range.start)
        .map_err(|error| PageXObjectBindingRefusal::MalformedEntryReference {
            reference_reason: error.reason,
        })?
        .reference;
    let target_object_byte_offset = match locate_uncompressed(env.lookup, target) {
        LocateOutcome::Uncompressed { byte_offset } => byte_offset,
        LocateOutcome::GenerationMismatch { xref_generation } => {
            return Err(PageXObjectBindingRefusal::EntryTargetGenerationMismatch {
                reference: target,
                xref_generation,
            });
        }
        LocateOutcome::Compressed {
            object_stream_number,
            index_within_object_stream,
        } => {
            return Err(PageXObjectBindingRefusal::CompressedEntryTarget {
                reference: target,
                object_stream_number,
                index_within_object_stream,
            });
        }
        LocateOutcome::Unresolved { location } => {
            return Err(PageXObjectBindingRefusal::UnresolvedEntryTarget {
                reference: target,
                location,
            });
        }
    };
    let dictionary =
        match corroborated_object_dictionary(env.input, target, target_object_byte_offset) {
            Ok(dictionary) => dictionary,
            Err(CorroborationFailure::IdentityMismatch { header_reference }) => {
                return Err(PageXObjectBindingRefusal::EntryTargetIdentityMismatch {
                    reference: target,
                    object_byte_offset: target_object_byte_offset,
                    header_reference,
                });
            }
            Err(CorroborationFailure::DictionaryFailed { error }) => {
                return Err(PageXObjectBindingRefusal::EntryTargetDictionaryFailed {
                    reference: target,
                    object_byte_offset: target_object_byte_offset,
                    error,
                });
            }
        };

    let subtype = classify_subtype(env.input, &dictionary.entries);
    let (target_ownership, verdict) =
        judge_witness(env.veto, path, target, ordinal, page_reference);

    Ok(PageXObjectBindingWitness {
        name,
        key_range: entry.key_range,
        value_range: entry.value_range,
        path: path.clone(),
        target,
        target_object_byte_offset,
        subtype,
        target_ownership,
        verdict,
    })
}

/// Compose the conservative target ownership and page-local verdict.
///
/// The ownership uses the public decision contract over the single proven
/// direct binding edge, then applies the document-wide exclusivity veto: an
/// incomplete index or non-exclusive typed referrers force `Unproven`. The
/// verdict additionally requires every binding-path node to be leaf-direct.
fn judge_witness(
    veto: ConsumerVeto<'_>,
    path: &XObjectBindingPath,
    target: IndirectRef,
    ordinal: usize,
    page_reference: IndirectRef,
) -> (IndirectObjectOwnership, PageXObjectBindingVerdict) {
    let exclusive = veto.exclusive_to_page(target, ordinal, page_reference);
    let mut decision = decide_indirect_object_edit(target, [page_reference]);
    if !veto.complete || !exclusive {
        decision.ownership = IndirectObjectOwnership::Unproven;
    }

    let reason = first_unproven_reason(veto, path, target, exclusive);
    let verdict = reason.map_or(PageXObjectBindingVerdict::ProvenPageLocal, |reason| {
        PageXObjectBindingVerdict::Unproven { reason }
    });
    (decision.ownership, verdict)
}

fn first_unproven_reason(
    veto: ConsumerVeto<'_>,
    path: &XObjectBindingPath,
    target: IndirectRef,
    exclusive: bool,
) -> Option<PageXObjectBindingUnprovenReason> {
    if let BindingResourcesSource::Inherited { ancestor, .. } = path.resources_source {
        return Some(PageXObjectBindingUnprovenReason::ResourcesInherited { ancestor });
    }
    if let BindingContainerLocality::IndirectResolved { reference, .. } = path.resources_locality {
        return Some(PageXObjectBindingUnprovenReason::ResourcesIndirect { reference });
    }
    if let BindingContainerLocality::IndirectResolved { reference, .. } = path.xobject_locality {
        return Some(PageXObjectBindingUnprovenReason::XObjectDictionaryIndirect { reference });
    }
    if !veto.complete {
        return Some(PageXObjectBindingUnprovenReason::ConsumerIndexIncomplete);
    }
    if !exclusive {
        return Some(
            PageXObjectBindingUnprovenReason::TargetConsumersNotExclusive {
                referrer_count: veto.referrers(target).len(),
            },
        );
    }
    None
}

/// Exact `/Subtype` classification from already-inspected dictionary entries.
fn classify_subtype(input: &[u8], entries: &[DictionaryEntrySpan]) -> XObjectBindingSubtype {
    match unique_semantic_entry(input, entries, b"Subtype") {
        Err((first_key_range, duplicate_key_range)) => XObjectBindingSubtype::Duplicate {
            first_key_range,
            duplicate_key_range,
        },
        Ok(None) => XObjectBindingSubtype::Missing,
        Ok(Some(entry)) => match entry.value_kind {
            DictionaryValueKind::Name => {
                let raw = &input[entry.value_range.start + 1..entry.value_range.end];
                match decode_pdf_name(raw).as_deref() {
                    Some(b"Form") => XObjectBindingSubtype::Form,
                    Some(b"Image") => XObjectBindingSubtype::Image,
                    _ => XObjectBindingSubtype::OtherName {
                        name: PdfName(raw.to_vec()),
                    },
                }
            }
            value_kind => XObjectBindingSubtype::NonName { value_kind },
        },
    }
}

/// Find at most one entry whose DECODED key equals `expected`; a decoded
/// collision (including raw duplicate keys) is a hard error, never
/// first-wins. Undecodable keys can never match a well-formed expected name.
fn unique_semantic_entry(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
    expected: &[u8],
) -> Result<Option<DictionaryEntrySpan>, (DictionaryEntryByteRange, DictionaryEntryByteRange)> {
    let mut found: Option<DictionaryEntrySpan> = None;
    for entry in entries {
        let raw = &input[entry.key_range.start + 1..entry.key_range.end];
        if decode_pdf_name(raw).is_none_or(|name| name.as_ref() != expected) {
            continue;
        }
        if let Some(first) = found {
            return Err((first.key_range, entry.key_range));
        }
        found = Some(*entry);
    }
    Ok(found)
}

const fn refusal(
    page_object_byte_offset: usize,
    resource_name: Option<PdfName>,
    reason: PageXObjectBindingRefusal,
) -> RefusedPageXObjectBinding {
    RefusedPageXObjectBinding {
        page_object_byte_offset,
        resource_name,
        reason,
    }
}
