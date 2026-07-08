//! Document-wide object consumer index, read-only snapshot.
//!
//! For every indirect object reachable from a typed user, the index reports
//! WHICH users reference it. Users are the newest trailer keys (except
//! `/Root`), the catalog object itself, the catalog top-level keys (including
//! `/Pages`), and the enumerated page leaves. The report is the correctness
//! prerequisite for proving shared-object ownership before in-place mutation.
//!
//! # Snapshot contract
//!
//! The index is a SNAPSHOT computed on the current source bytes. Any byte
//! mutation invalidates it; no invalidation machinery exists in this slice and
//! callers must recompute after writing.
//!
//! # Safety pins
//!
//! - A truncated index must never feed ownership decisions: when any
//!   [`ObjectConsumerIndexTruncation`] fact is present, every affected target
//!   is `Unproven`, never proven single-use.
//! - Page dictionaries and page-tree intermediates are INHERENTLY multi-user:
//!   a page is registered by the `RootKey(/Pages)` descent AND by its own
//!   `Page` user. That double registration is the safety property that keeps
//!   page objects out of single-owner in-place mutation; do not "fix" it.
//!
//! # Retention
//!
//! The report retains key NAME bytes only (the [`PdfName`] payloads of
//! [`ObjectConsumerReferrer::TrailerKey`] and
//! [`ObjectConsumerReferrer::RootKey`]). It retains no other PDF bytes: no
//! object bodies, stream bodies, value bytes, or decoded object-stream bytes.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    DictionaryEntrySpan, DocumentAccess, IndirectRef, MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    ObjectBodyReferencesInspection, ObjectLookup, ObjectLookupLocation, ObjectResolutionRejection,
    PdfName, ResolvedObjectData, ResolvedObjectDictionaryInspection, SkippedObjectBodyReference,
    inspect_classic_xref_table, inspect_classic_xref_trailer_dictionary,
    inspect_dictionary_entries, inspect_indirect_object_dictionary, inspect_object_dictionary,
    locate_xref_object, resolve_object, scan_indirect_references_in_span,
};

mod traversal;

use traversal::{ConsumerTraversal, IndexState, ObjectStreamCache};

/// Maximum traversal depth per user; a seed is depth `0`.
pub const MAX_OBJECT_CONSUMER_TRAVERSAL_DEPTH: usize = 64;

/// Maximum visited object numbers per user (the per-user visited set size).
pub const MAX_OBJECT_CONSUMER_VISITED_NODES: usize = 65_536;

/// Maximum expanded nodes across ALL users (global work bound).
pub const MAX_OBJECT_CONSUMER_EXPANDED_NODES: usize = 1_048_576;

/// Maximum recorded `(target, referrer)` pairs across the whole index.
pub const MAX_OBJECT_CONSUMER_RECORDED_PAIRS: usize = 1_000_000;

/// Total decoded-byte budget of the per-container object-stream cache.
///
/// On overflow the cache is dropped and members re-decode per resolution
/// (slower but correct); the drop is recorded in
/// [`ObjectStreamCacheReport::dropped_over_budget`].
pub const MAX_OBJECT_CONSUMER_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// One typed user that references indirect objects.
///
/// Variants sort in seeding order: trailer keys, the catalog itself, catalog
/// keys, then pages. The `key` payloads retain PDF name bytes (without the
/// leading `/`); no other PDF bytes are retained.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "referrer", rename_all = "snake_case")]
pub enum ObjectConsumerReferrer {
    /// One key of the NEWEST trailer (or newest xref-stream dictionary) except
    /// `/Root`, e.g. `/Info`, `/Encrypt`, `/ID`.
    TrailerKey {
        /// Trailer key name bytes without the leading `/`.
        key: PdfName,
    },
    /// The document catalog object itself (the trailer `/Root` target).
    Root,
    /// One top-level key of the catalog dictionary, including `/Pages`.
    RootKey {
        /// Catalog key name bytes without the leading `/`.
        key: PdfName,
    },
    /// One enumerated leaf `/Page`.
    Page {
        /// Zero-based document-order page index.
        page_index: usize,
        /// Indirect reference of the leaf `/Page`.
        page: IndirectRef,
    },
}

/// All deduplicated users of one target object, referrers in sorted order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectConsumersEntry {
    /// Referenced indirect object.
    pub target: IndirectRef,
    /// Deduplicated referrers in [`ObjectConsumerReferrer`] order.
    pub referrers: Vec<ObjectConsumerReferrer>,
}

/// One reference edge whose target did not resolve to an object body.
///
/// Dangling references, free entries, generation mismatches, reserved
/// cross-reference entries, and classic-ambiguous lookups all land here per
/// the ISO 32000-1 7.3.10 null semantics: they are facts, never consumer
/// edges, and never fatal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectConsumerUnresolvedEdge {
    /// Referenced indirect object that did not resolve.
    pub target: IndirectRef,
    /// User whose traversal followed the edge.
    pub referrer: ObjectConsumerReferrer,
    /// Delegated structured resolution failure.
    pub resolution_reason: ObjectResolutionRejection,
}

/// Structured reason part of the index could not be computed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedObjectConsumerScan {
    /// The newest section's trailer dictionary could not be re-inspected, so
    /// no `TrailerKey` users were seeded.
    NewestTrailerDictionary {
        /// Byte offset of the newest cross-reference section.
        section_byte_offset: usize,
    },
    /// The catalog dictionary could not be inspected, so no `RootKey` users
    /// were seeded.
    CatalogDictionary,
    /// One resolved object's body could not be scanned for references; its
    /// outgoing edges are unknown.
    BodyScan {
        /// Object whose body scan failed.
        target: IndirectRef,
        /// User whose traversal reached the object.
        referrer: ObjectConsumerReferrer,
    },
    /// Reference-shaped constructs were skipped for out-of-range numbers
    /// during one span scan.
    ReferenceShapes {
        /// Object whose body carried the constructs, or `None` for a trailer
        /// or catalog seed span.
        target: Option<IndirectRef>,
        /// User whose traversal scanned the span.
        referrer: ObjectConsumerReferrer,
        /// Delegated structured skip markers.
        skipped_references: Vec<SkippedObjectBodyReference>,
    },
    /// An in-use cross-reference entry could not be normalized for the
    /// unreferenced diff (out-of-range numbers or classic-ambiguous
    /// duplicates).
    UnreferencedEntryUnresolvable {
        /// Raw entry object number.
        object_number: usize,
    },
}

/// One bound hit during index construction. Never silent: every hit is a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectConsumerIndexTruncation {
    /// User whose traversal hit the bound, when the bound is per-user.
    pub referrer: Option<ObjectConsumerReferrer>,
    /// Edge target refused by the bound, when one edge was refused.
    pub target: Option<IndirectRef>,
    /// Bound that was hit.
    pub limit: ObjectConsumerIndexLimit,
}

/// Bound that stopped part of the traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "limit", rename_all = "snake_case")]
pub enum ObjectConsumerIndexLimit {
    /// The per-user traversal depth bound refused an edge.
    MaxTraversalDepth {
        /// Configured maximum depth.
        max_depth: usize,
    },
    /// The per-user visited-node bound refused an edge.
    MaxVisitedNodes {
        /// Configured maximum visited-node count.
        max_visited_nodes: usize,
    },
    /// The global expanded-node work bound stopped the whole build.
    MaxExpandedNodes {
        /// Configured maximum expanded-node count.
        max_expanded_nodes: usize,
    },
    /// The recorded-pair bound stopped the whole build.
    MaxRecordedPairs {
        /// Configured maximum recorded `(target, referrer)` pairs.
        max_recorded_pairs: usize,
    },
    /// One body scan stopped at the per-body reference cap.
    MaxBodyReferences {
        /// Configured per-body reference cap.
        max_references: usize,
    },
    /// A compressed member's object-stream container exceeded the per-container
    /// decoded-byte cap, so the member's outgoing references were not scanned.
    MaxDecodedObjectStreamBytes {
        /// Unfiltered object-stream body length, when the resolver could derive it.
        decoded_length: usize,
        /// Configured per-container decoded-byte cap.
        max_decoded_object_stream_bytes: usize,
    },
}

/// Facts about the per-container object-stream once-decode cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectStreamCacheReport {
    /// Configured total decoded-byte budget.
    pub budget_bytes: usize,
    /// Object-stream containers held by the cache when the build finished.
    pub cached_container_count: usize,
    /// Total decoded bytes held by the cache when the build finished.
    pub cached_byte_count: usize,
    /// Whether the budget overflowed: the cache was dropped and members
    /// re-decoded per resolution from that point on (slower but correct).
    pub dropped_over_budget: bool,
}

/// Deterministic document-wide object consumer index snapshot.
///
/// Everything is sorted: entries ascend by target, referrers by
/// [`ObjectConsumerReferrer`] order, unresolved edges by `(target, referrer)`,
/// and the unreferenced diff by object number. Skips and truncations are in
/// deterministic traversal order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectConsumerIndexInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Consumer entries sorted ascending by target reference.
    pub entries: Vec<ObjectConsumersEntry>,
    /// Reference edges whose targets did not resolve, sorted by target then
    /// referrer.
    pub unresolved_edges: Vec<ObjectConsumerUnresolvedEdge>,
    /// Structured scan skips in traversal order.
    pub skipped: Vec<SkippedObjectConsumerScan>,
    /// Bound-hit facts in traversal order; empty means no bound was hit.
    pub truncations: Vec<ObjectConsumerIndexTruncation>,
    /// Nodes expanded across all users (global work counter).
    pub expanded_node_count: usize,
    /// Recorded `(target, referrer)` pairs.
    pub recorded_pair_count: usize,
    /// Object-stream once-decode cache facts.
    pub object_stream_cache: ObjectStreamCacheReport,
    /// In-use cross-reference entries never reached by any user, sorted
    /// ascending by object number.
    pub unreferenced: Vec<IndirectRef>,
}

/// Build the document-wide object consumer index over an already-inspected
/// [`DocumentAccess`] spine.
///
/// Users are seeded in deterministic order: one [`TrailerKey`] user per key of
/// the NEWEST trailer except `/Root`; the [`Root`] user registering the
/// catalog object itself (no descent); one [`RootKey`] user per catalog
/// top-level key (including `/Pages`); one [`Page`] user per enumerated page
/// leaf. Each user runs an ITERATIVE worklist with a PER-USER visited set
/// (`BTreeSet<u32>` on object number) — never a global visited set, so a
/// shared subtree registers under every user that reaches it.
///
/// Page rules use membership checks, never `/Type` sniffing: page membership
/// compares the FULL enumerated page-leaf reference (object number AND
/// generation), so a generation-mismatched reference to a page's object
/// number is not a page edge and resolves — and fails — like any other edge;
/// the top-level `/Parent` edge of page and page-tree-node dictionaries is
/// skipped; an edge whose target is a known page other than the current
/// user's own page is registered but not
/// expanded. Reference extraction reuses the object-body reference scanner,
/// so stream-parameter edges (indirect `/Length`, `/Filter`, `/DecodeParms`)
/// are real consumer edges inside the dictionary extent, and stream data is
/// never scanned. Object streams are TRANSPARENT: references inside
/// compressed members are edges of the MEMBER; the container is never a
/// consumer and appears only via its own direct referrers or the
/// unreferenced diff.
///
/// Unresolvable edges (dangling, free, generation mismatch, reserved,
/// classic-ambiguous, object-stream extraction failures including
/// self-referential or nested object streams) are structured facts, never
/// fatal. Every bound hit is a structured truncation fact.
///
/// Cost is `O(sum of per-user reachable subtrees)` by design (per-user
/// visited sets); shared-subtree closure memoization is a recorded future
/// optimization, not part of this slice.
///
/// [`TrailerKey`]: ObjectConsumerReferrer::TrailerKey
/// [`Root`]: ObjectConsumerReferrer::Root
/// [`RootKey`]: ObjectConsumerReferrer::RootKey
/// [`Page`]: ObjectConsumerReferrer::Page
#[must_use]
pub fn inspect_object_consumer_index(
    input: &[u8],
    access: &DocumentAccess,
) -> ObjectConsumerIndexInspection {
    let lookup = access.backend.object_lookup();
    let known_pages: BTreeSet<IndirectRef> = access
        .page_leaves
        .leaves
        .iter()
        .map(|leaf| leaf.reference)
        .collect();

    let traversal = ConsumerTraversal {
        input,
        lookup,
        known_pages: &known_pages,
    };
    let mut state = IndexState::new();
    let mut cache = ObjectStreamCache::new();

    seed_trailer_key_users(input, &traversal, &mut state, &mut cache);

    state.register(access.root_reference, &ObjectConsumerReferrer::Root);

    seed_root_key_users(
        input,
        access.root_reference,
        &traversal,
        &mut state,
        &mut cache,
    );

    for (page_index, leaf) in access.page_leaves.leaves.iter().enumerate() {
        let referrer = ObjectConsumerReferrer::Page {
            page_index,
            page: leaf.reference,
        };
        traversal.run_user(
            &mut state,
            &mut cache,
            &referrer,
            &[leaf.reference],
            Some(leaf.reference.object_number),
            false,
        );
    }

    let reached: BTreeSet<u32> = state
        .consumers
        .keys()
        .map(|target| target.object_number)
        .collect();
    let unreferenced = unreferenced_entries(lookup, &reached, &mut state);

    let mut unresolved_edges = state.unresolved_edges;
    unresolved_edges
        .sort_by(|left, right| (left.target, &left.referrer).cmp(&(right.target, &right.referrer)));

    ObjectConsumerIndexInspection {
        byte_len: input.len(),
        entries: state
            .consumers
            .into_iter()
            .map(|(target, referrers)| ObjectConsumersEntry {
                target,
                referrers: referrers.into_iter().collect(),
            })
            .collect(),
        unresolved_edges,
        skipped: state.skipped,
        truncations: state.truncations,
        expanded_node_count: state.expanded_node_count,
        recorded_pair_count: state.recorded_pair_count,
        object_stream_cache: cache.report(),
        unreferenced,
    }
}

/// Seed and run one `TrailerKey` user per newest-trailer key except `/Root`.
fn seed_trailer_key_users(
    input: &[u8],
    traversal: &ConsumerTraversal<'_>,
    state: &mut IndexState,
    cache: &mut ObjectStreamCache,
) {
    let entries = match newest_trailer_entries(input, traversal.lookup) {
        Ok(entries) => entries,
        Err(section_byte_offset) => {
            state
                .skipped
                .push(SkippedObjectConsumerScan::NewestTrailerDictionary {
                    section_byte_offset,
                });
            return;
        }
    };

    for entry in &entries {
        let key = key_bytes(input, entry);
        if key == b"/Root" {
            continue;
        }
        let referrer = ObjectConsumerReferrer::TrailerKey {
            key: referrer_key(key),
        };
        run_seed_span(input, traversal, state, cache, &referrer, entry, false);
    }
}

/// Seed and run one `RootKey` user per catalog top-level key.
///
/// The `RootKey(/Pages)` user's seeds are marked as page-tree nodes, so the
/// descent applies the `/Parent` skip to tree intermediates and stops at every
/// known page dictionary while still owning the intermediates.
fn seed_root_key_users(
    input: &[u8],
    root_reference: IndirectRef,
    traversal: &ConsumerTraversal<'_>,
    state: &mut IndexState,
    cache: &mut ObjectStreamCache,
) {
    let Ok(resolved) = resolve_object(
        input,
        traversal.lookup,
        root_reference,
        MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    ) else {
        state
            .skipped
            .push(SkippedObjectConsumerScan::CatalogDictionary);
        return;
    };
    let Ok(dictionary) = inspect_object_dictionary(input, &resolved) else {
        state
            .skipped
            .push(SkippedObjectConsumerScan::CatalogDictionary);
        return;
    };
    let (buffer, entries): (&[u8], &[DictionaryEntrySpan]) = match (&dictionary, &resolved) {
        (ResolvedObjectDictionaryInspection::Uncompressed(inspection), _) => {
            (input, &inspection.entries)
        }
        (
            ResolvedObjectDictionaryInspection::Compressed(inspection),
            ResolvedObjectData::Compressed {
                decoded_object_stream,
                object_body_span,
                ..
            },
        ) => (
            decoded_object_stream
                .get(object_body_span.clone())
                .unwrap_or(&[]),
            &inspection.entries,
        ),
        // Unreachable by construction: `inspect_object_dictionary` returns a
        // compressed inspection only for compressed resolved data.
        _ => {
            state
                .skipped
                .push(SkippedObjectConsumerScan::CatalogDictionary);
            return;
        }
    };

    for entry in entries {
        let key = key_bytes(buffer, entry);
        let referrer = ObjectConsumerReferrer::RootKey {
            key: referrer_key(key),
        };
        let seeds_are_tree_nodes = key == b"/Pages";
        run_seed_span(
            buffer,
            traversal,
            state,
            cache,
            &referrer,
            entry,
            seeds_are_tree_nodes,
        );
    }
}

/// Scan one seed value span and run the user over the references it holds.
///
/// Out-of-range reference shapes and a per-span cap hit are recorded with no
/// target object: the trailer is not an object, and catalog seed spans belong
/// to the catalog's own `Root` registration, not to an edge target.
fn run_seed_span(
    buffer: &[u8],
    traversal: &ConsumerTraversal<'_>,
    state: &mut IndexState,
    cache: &mut ObjectStreamCache,
    referrer: &ObjectConsumerReferrer,
    entry: &DictionaryEntrySpan,
    seeds_are_tree_nodes: bool,
) {
    let ObjectBodyReferencesInspection {
        references,
        skipped_references,
        truncation,
    } = scan_indirect_references_in_span(buffer, entry.value_range.start..entry.value_range.end);
    state.record_scan_facts(referrer, None, skipped_references, truncation.is_some());
    traversal.run_user(
        state,
        cache,
        referrer,
        &references,
        None,
        seeds_are_tree_nodes,
    );
}

/// Top-level entry spans of the NEWEST section's trailer dictionary.
///
/// Classic single tables reuse the parsed trailer offset; classic and
/// xref-stream `/Prev` chains re-inspect the newest section because chains
/// retain only `/Root` plus section offsets; xref-stream sections read the
/// xref-stream object's own dictionary (its structural keys such as `/Size`
/// or `/W` become reference-free `TrailerKey` users and register nothing).
///
/// # Errors
///
/// Returns the newest section byte offset when the trailer dictionary cannot
/// be re-inspected.
fn newest_trailer_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
) -> Result<Vec<DictionaryEntrySpan>, usize> {
    match lookup {
        ObjectLookup::ClassicXref(xref_table) => classic_trailer_entries(
            input,
            xref_table.table_byte_offset,
            xref_table.trailer_byte_offset,
        ),
        ObjectLookup::ClassicXrefChain(chain) => {
            let section_byte_offset = chain
                .section_byte_offsets
                .first()
                .copied()
                .unwrap_or(chain.startxref_byte_offset);
            let table = inspect_classic_xref_table(input, section_byte_offset)
                .map_err(|_| section_byte_offset)?;
            classic_trailer_entries(input, section_byte_offset, table.trailer_byte_offset)
        }
        ObjectLookup::XrefStreamSection(section) => {
            xref_stream_trailer_entries(input, section.object_byte_offset)
        }
        ObjectLookup::XrefStreamChain(chain) => {
            let section_byte_offset = chain
                .section_byte_offsets
                .first()
                .copied()
                .unwrap_or(chain.startxref_byte_offset);
            xref_stream_trailer_entries(input, section_byte_offset)
        }
    }
}

fn classic_trailer_entries(
    input: &[u8],
    section_byte_offset: usize,
    trailer_byte_offset: usize,
) -> Result<Vec<DictionaryEntrySpan>, usize> {
    let dictionary = inspect_classic_xref_trailer_dictionary(input, trailer_byte_offset)
        .map_err(|_| section_byte_offset)?;
    let entries = inspect_dictionary_entries(input, dictionary.dictionary_open_byte_offset)
        .map_err(|_| section_byte_offset)?;
    Ok(entries.entries)
}

fn xref_stream_trailer_entries(
    input: &[u8],
    section_byte_offset: usize,
) -> Result<Vec<DictionaryEntrySpan>, usize> {
    let dictionary = inspect_indirect_object_dictionary(input, section_byte_offset)
        .map_err(|_| section_byte_offset)?;
    Ok(dictionary.entries)
}

/// Diff the in-use cross-reference entries against the reached target set.
///
/// The normalization is deliberately private: classic and xref-stream entry
/// records differ, so raw entry object numbers are re-located through
/// [`locate_xref_object`], reusing its overflow checks. Classic duplicate
/// entries are handled by the number set (no uniqueness assumption); a
/// classic-ambiguous location is recorded as a structured skip. Free,
/// not-found, and reserved locations are not in-use and never diffed.
fn unreferenced_entries(
    lookup: ObjectLookup<'_>,
    reached: &BTreeSet<u32>,
    state: &mut IndexState,
) -> Vec<IndirectRef> {
    let mut candidate_numbers = BTreeSet::new();
    match lookup {
        ObjectLookup::ClassicXref(xref_table) => {
            for subsection in &xref_table.subsections {
                for entry in &subsection.entries {
                    candidate_numbers.insert(entry.object_number as usize);
                }
            }
        }
        ObjectLookup::ClassicXrefChain(chain) => {
            for entry in &chain.entries {
                candidate_numbers.insert(entry.object_number as usize);
            }
        }
        ObjectLookup::XrefStreamSection(section) => {
            for entry in &section.entries {
                candidate_numbers.insert(entry.object_number);
            }
        }
        ObjectLookup::XrefStreamChain(chain) => {
            for entry in &chain.entries {
                candidate_numbers.insert(entry.object_number);
            }
        }
    }

    let mut unreferenced = Vec::new();
    for object_number in candidate_numbers {
        let in_use = match locate_xref_object(lookup, object_number) {
            ObjectLookupLocation::ClassicInUse { generation, .. }
            | ObjectLookupLocation::XrefStreamUncompressed { generation, .. } => {
                in_use_reference(object_number, generation)
            }
            ObjectLookupLocation::XrefStreamCompressed { .. } => in_use_reference(object_number, 0),
            ObjectLookupLocation::ClassicAmbiguous { .. }
            | ObjectLookupLocation::ClassicObjectNumberOutOfRange { .. }
            | ObjectLookupLocation::XrefStreamObjectNumberOutOfRange { .. }
            | ObjectLookupLocation::XrefStreamUncompressedGenerationOutOfRange { .. } => {
                state
                    .skipped
                    .push(SkippedObjectConsumerScan::UnreferencedEntryUnresolvable {
                        object_number,
                    });
                None
            }
            _ => None,
        };
        if let Some(reference) = in_use {
            if !reached.contains(&reference.object_number) {
                unreferenced.push(reference);
            }
        }
    }
    unreferenced
}

/// Build an in-use [`IndirectRef`] from a located entry, or `None` when the
/// object number does not fit the public contract.
fn in_use_reference(object_number: usize, generation: u16) -> Option<IndirectRef> {
    u32::try_from(object_number)
        .ok()
        .map(|object_number| IndirectRef {
            object_number,
            generation,
        })
}

/// Raw key bytes (including the leading `/`) of one dictionary entry span.
fn key_bytes<'buffer>(buffer: &'buffer [u8], entry: &DictionaryEntrySpan) -> &'buffer [u8] {
    buffer
        .get(entry.key_range.start..entry.key_range.end)
        .unwrap_or(&[])
}

/// Strip the leading `/` into the retained [`PdfName`] key payload.
fn referrer_key(key: &[u8]) -> PdfName {
    PdfName(key.get(1..).unwrap_or_default().to_vec())
}
