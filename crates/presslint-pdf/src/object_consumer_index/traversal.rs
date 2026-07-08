//! Iterative per-user traversal engine and object-stream once-decode cache.
//!
//! Every user runs its own worklist with its own visited set; the only global
//! state is the accumulated report data (consumers, facts, counters) and the
//! object-stream cache. A global visited set would corrupt the index: the
//! second user of a shared subtree would register nothing and the subtree
//! would be misclassified as single-use.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

use crate::source_utils::is_pdf_whitespace;
use crate::xref_stream::parse_non_negative_integer;
use crate::{
    IndirectObjectBodyLeadingTokenKind, IndirectObjectDictionaryInspectionRejection, IndirectRef,
    MAX_OBJECT_BODY_REFERENCES, MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    ObjectBodyReferencesInspection, ObjectLookup, ObjectLookupLocation, ObjectResolutionError,
    ObjectResolutionRejection, ObjectStreamMemberExtractionRejection, ResolvedObjectData,
    inspect_dictionary_entries, inspect_indirect_object_body_token,
    inspect_indirect_object_dictionary, inspect_indirect_object_header,
    inspect_object_body_references, locate_xref_object, resolve_object, resolve_xref_object_offset,
    scan_indirect_references_in_span,
};

use super::{
    DictionaryEntrySpan, MAX_OBJECT_CONSUMER_CACHE_BYTES, MAX_OBJECT_CONSUMER_EXPANDED_NODES,
    MAX_OBJECT_CONSUMER_RECORDED_PAIRS, MAX_OBJECT_CONSUMER_TRAVERSAL_DEPTH,
    MAX_OBJECT_CONSUMER_VISITED_NODES, ObjectConsumerIndexLimit, ObjectConsumerIndexTruncation,
    ObjectConsumerReferrer, ObjectConsumerUnresolvedEdge, ObjectStreamCacheReport,
    SkippedObjectBodyReference, SkippedObjectConsumerScan,
};

/// Mutable report accumulators shared by every user traversal.
pub(super) struct IndexState {
    /// Registered `(target, referrer)` pairs, deduplicated and ordered.
    pub(super) consumers: BTreeMap<IndirectRef, BTreeSet<ObjectConsumerReferrer>>,
    /// Unresolved-edge facts in traversal order (sorted by the caller).
    pub(super) unresolved_edges: Vec<ObjectConsumerUnresolvedEdge>,
    /// Structured scan skips in traversal order.
    pub(super) skipped: Vec<SkippedObjectConsumerScan>,
    /// Bound-hit facts in traversal order.
    pub(super) truncations: Vec<ObjectConsumerIndexTruncation>,
    /// Nodes expanded across all users.
    pub(super) expanded_node_count: usize,
    /// Recorded `(target, referrer)` pairs.
    pub(super) recorded_pair_count: usize,
    /// Set when a global bound stopped the whole build.
    pub(super) stopped: bool,
}

impl IndexState {
    pub(super) const fn new() -> Self {
        Self {
            consumers: BTreeMap::new(),
            unresolved_edges: Vec::new(),
            skipped: Vec::new(),
            truncations: Vec::new(),
            expanded_node_count: 0,
            recorded_pair_count: 0,
            stopped: false,
        }
    }

    /// Record one `(target, referrer)` consumer pair, deduplicated, under the
    /// global recorded-pair bound.
    pub(super) fn register(&mut self, target: IndirectRef, referrer: &ObjectConsumerReferrer) {
        if self
            .consumers
            .get(&target)
            .is_some_and(|referrers| referrers.contains(referrer))
        {
            return;
        }
        if self.recorded_pair_count >= MAX_OBJECT_CONSUMER_RECORDED_PAIRS {
            if !self.stopped {
                self.truncate(
                    Some(referrer.clone()),
                    Some(target),
                    ObjectConsumerIndexLimit::MaxRecordedPairs {
                        max_recorded_pairs: MAX_OBJECT_CONSUMER_RECORDED_PAIRS,
                    },
                );
                self.stopped = true;
            }
            return;
        }
        self.consumers
            .entry(target)
            .or_default()
            .insert(referrer.clone());
        self.recorded_pair_count += 1;
    }

    /// Record one bound-hit fact.
    pub(super) fn truncate(
        &mut self,
        referrer: Option<ObjectConsumerReferrer>,
        target: Option<IndirectRef>,
        limit: ObjectConsumerIndexLimit,
    ) {
        self.truncations.push(ObjectConsumerIndexTruncation {
            referrer,
            target,
            limit,
        });
    }

    /// Record one span scan's shape-skip and per-body cap facts.
    ///
    /// `target` is the scanned object, or `None` for a trailer or catalog
    /// seed span (the trailer is not an object, and catalog seed spans belong
    /// to the catalog's own `Root` registration, not to an edge target).
    pub(super) fn record_scan_facts(
        &mut self,
        referrer: &ObjectConsumerReferrer,
        target: Option<IndirectRef>,
        skipped_references: Vec<SkippedObjectBodyReference>,
        truncated: bool,
    ) {
        if !skipped_references.is_empty() {
            self.skipped
                .push(SkippedObjectConsumerScan::ReferenceShapes {
                    target,
                    referrer: referrer.clone(),
                    skipped_references,
                });
        }
        if truncated {
            self.truncate(
                Some(referrer.clone()),
                target,
                ObjectConsumerIndexLimit::MaxBodyReferences {
                    max_references: MAX_OBJECT_BODY_REFERENCES,
                },
            );
        }
    }

    pub(super) fn skip_body_scan(
        &mut self,
        target: IndirectRef,
        referrer: &ObjectConsumerReferrer,
    ) {
        self.skipped.push(SkippedObjectConsumerScan::BodyScan {
            target,
            referrer: referrer.clone(),
        });
    }
}

/// Per-user traversal state: the user's identity, page exclusions, visited
/// set, tree-node membership, and explicit worklist.
struct UserWalk<'referrer> {
    referrer: &'referrer ObjectConsumerReferrer,
    own_page: Option<u32>,
    /// Keyed by FULL reference: a generation-mismatched edge must never be
    /// suppressed by a visit of the xref's real generation — it has to reach
    /// resolution and surface as an unresolved-edge fact. Newest-wins keeps
    /// one live generation per number, so valid edges are unaffected.
    visited: BTreeSet<IndirectRef>,
    tree_nodes: BTreeSet<u32>,
    worklist: Vec<(IndirectRef, usize)>,
}

/// Borrowed traversal context shared by every user run.
pub(super) struct ConsumerTraversal<'input> {
    /// Whole PDF source.
    pub(super) input: &'input [u8],
    /// Backend lookup view from the document-access spine.
    pub(super) lookup: ObjectLookup<'input>,
    /// Enumerated page-leaf references (page membership, never `/Type`).
    ///
    /// Membership compares the FULL reference: a generation-mismatched
    /// reference to a page's object number is not a page edge and goes
    /// through normal resolution, which records the mismatch as an
    /// unresolved-edge fact instead of a consumer edge.
    pub(super) known_pages: &'input BTreeSet<IndirectRef>,
}

impl ConsumerTraversal<'_> {
    /// Run one user's bounded iterative traversal from its seeds.
    ///
    /// The visited set is PER USER and seeded with the user's start objects,
    /// so shared subtrees re-register under every user that reaches them.
    pub(super) fn run_user(
        &self,
        state: &mut IndexState,
        cache: &mut ObjectStreamCache,
        referrer: &ObjectConsumerReferrer,
        seeds: &[IndirectRef],
        own_page: Option<u32>,
        seeds_are_tree_nodes: bool,
    ) {
        if state.stopped {
            return;
        }
        let mut walk = UserWalk {
            referrer,
            own_page,
            visited: BTreeSet::new(),
            tree_nodes: BTreeSet::new(),
            worklist: Vec::new(),
        };
        for seed in seeds {
            if seeds_are_tree_nodes && !self.known_pages.contains(seed) {
                walk.tree_nodes.insert(seed.object_number);
            }
            self.enqueue(state, &mut walk, *seed, 0);
        }

        while let Some((target, depth)) = walk.worklist.pop() {
            if state.stopped {
                return;
            }
            if state.expanded_node_count >= MAX_OBJECT_CONSUMER_EXPANDED_NODES {
                state.truncate(
                    Some(walk.referrer.clone()),
                    Some(target),
                    ObjectConsumerIndexLimit::MaxExpandedNodes {
                        max_expanded_nodes: MAX_OBJECT_CONSUMER_EXPANDED_NODES,
                    },
                );
                state.stopped = true;
                return;
            }
            state.expanded_node_count += 1;

            let resolved = match resolve_with_cache(self.input, self.lookup, cache, target) {
                Ok(resolved) => resolved,
                Err(error) => {
                    record_resolution_truncation(state, walk.referrer, target, &error.reason);
                    state.unresolved_edges.push(ObjectConsumerUnresolvedEdge {
                        target,
                        referrer: walk.referrer.clone(),
                        resolution_reason: error.reason,
                    });
                    continue;
                }
            };
            state.register(target, walk.referrer);

            match resolved {
                ResolvedConsumerBody::Uncompressed { object_byte_offset } => {
                    self.expand_uncompressed(state, &mut walk, target, depth, object_byte_offset);
                }
                ResolvedConsumerBody::CompressedOwned { decoded, span } => {
                    let body = decoded.get(span).unwrap_or(&[]);
                    self.expand_member_body(state, &mut walk, target, depth, body);
                }
                ResolvedConsumerBody::CompressedCached { container, span } => {
                    let body = cache.member_body(container, &span);
                    self.expand_member_body(state, &mut walk, target, depth, body);
                }
            }
        }
    }

    /// Apply the edge rules to one discovered reference.
    ///
    /// A known page other than the user's own page is registered but never
    /// expanded; the page match compares the FULL reference, so a
    /// generation-mismatched reference to a page's object number falls
    /// through to normal resolution and surfaces as an unresolved-edge fact.
    /// Every other target passes the per-user visited, depth, and
    /// visited-node bounds before joining the worklist.
    fn enqueue(
        &self,
        state: &mut IndexState,
        walk: &mut UserWalk<'_>,
        target: IndirectRef,
        depth: usize,
    ) {
        if self.known_pages.contains(&target) && walk.own_page != Some(target.object_number) {
            state.register(target, walk.referrer);
            return;
        }
        if walk.visited.contains(&target) {
            return;
        }
        if depth > MAX_OBJECT_CONSUMER_TRAVERSAL_DEPTH {
            state.truncate(
                Some(walk.referrer.clone()),
                Some(target),
                ObjectConsumerIndexLimit::MaxTraversalDepth {
                    max_depth: MAX_OBJECT_CONSUMER_TRAVERSAL_DEPTH,
                },
            );
            return;
        }
        if walk.visited.len() >= MAX_OBJECT_CONSUMER_VISITED_NODES {
            state.truncate(
                Some(walk.referrer.clone()),
                Some(target),
                ObjectConsumerIndexLimit::MaxVisitedNodes {
                    max_visited_nodes: MAX_OBJECT_CONSUMER_VISITED_NODES,
                },
            );
            return;
        }
        walk.visited.insert(target);
        walk.worklist.push((target, depth));
    }

    /// Expand an uncompressed object body at its source byte offset.
    fn expand_uncompressed(
        &self,
        state: &mut IndexState,
        walk: &mut UserWalk<'_>,
        target: IndirectRef,
        depth: usize,
        object_byte_offset: usize,
    ) {
        match inspect_indirect_object_dictionary(self.input, object_byte_offset) {
            Ok(dictionary) => {
                let extent = dictionary.dictionary_open_byte_offset
                    ..dictionary.after_dictionary_close_byte_offset;
                self.expand_dictionary(
                    state,
                    walk,
                    target,
                    depth,
                    self.input,
                    &dictionary.entries,
                    extent,
                );
            }
            Err(error) => match error.reason {
                IndirectObjectDictionaryInspectionRejection::NonDictionaryBody { .. } => {
                    match inspect_object_body_references(self.input, object_byte_offset) {
                        Ok(report) => self.emit_report(state, walk, target, depth, report),
                        Err(_) => state.skip_body_scan(target, walk.referrer),
                    }
                }
                _ => state.skip_body_scan(target, walk.referrer),
            },
        }
    }

    /// Expand a compressed member's bare body inside a decoded buffer.
    ///
    /// Compressed members are never streams, so a non-dictionary body scans
    /// whole (matching the resolved reference scanner); a dictionary body gets
    /// the same page rules as an uncompressed dictionary.
    fn expand_member_body(
        &self,
        state: &mut IndexState,
        walk: &mut UserWalk<'_>,
        target: IndirectRef,
        depth: usize,
        body: &[u8],
    ) {
        match inspect_indirect_object_body_token(body, 0) {
            Ok(token) if token.token_kind == IndirectObjectBodyLeadingTokenKind::DictionaryOpen => {
                match inspect_dictionary_entries(body, token.first_token_byte_offset) {
                    Ok(inspection) => {
                        let extent = inspection.dictionary.open_byte_offset
                            ..inspection.dictionary.after_close_byte_offset;
                        self.expand_dictionary(
                            state,
                            walk,
                            target,
                            depth,
                            body,
                            &inspection.entries,
                            extent,
                        );
                    }
                    Err(_) => state.skip_body_scan(target, walk.referrer),
                }
            }
            _ => {
                let report = scan_indirect_references_in_span(body, 0..body.len());
                self.emit_report(state, walk, target, depth, report);
            }
        }
    }

    /// Expand a dictionary body, applying the page rules when the object is
    /// the user's own page or a page-tree node reached by a `/Pages` descent.
    ///
    /// Ordinary dictionaries scan their whole balanced extent in one pass
    /// (stream parameters such as an indirect `/Length` are inside it by
    /// construction). Page and tree-node dictionaries scan per top-level
    /// entry so the `/Parent` edge is skipped, and `/Kids` targets of tree
    /// nodes that are not known pages become tree nodes themselves.
    #[allow(clippy::too_many_arguments)]
    fn expand_dictionary(
        &self,
        state: &mut IndexState,
        walk: &mut UserWalk<'_>,
        target: IndirectRef,
        depth: usize,
        buffer: &[u8],
        entries: &[DictionaryEntrySpan],
        extent: Range<usize>,
    ) {
        let is_tree_node = walk.tree_nodes.contains(&target.object_number);
        let page_rules = is_tree_node || walk.own_page == Some(target.object_number);
        if !page_rules {
            let report = scan_indirect_references_in_span(buffer, extent);
            self.emit_report(state, walk, target, depth, report);
            return;
        }

        for entry in entries {
            let key = buffer
                .get(entry.key_range.start..entry.key_range.end)
                .unwrap_or(&[]);
            if key == b"/Parent" {
                continue;
            }
            let report = scan_indirect_references_in_span(
                buffer,
                entry.value_range.start..entry.value_range.end,
            );
            if is_tree_node && key == b"/Kids" {
                for reference in &report.references {
                    if !self.known_pages.contains(reference) {
                        walk.tree_nodes.insert(reference.object_number);
                    }
                }
            }
            self.emit_report(state, walk, target, depth, report);
        }
    }

    /// Emit one scan report's references as edges plus its structured
    /// shape-skip and cap facts.
    fn emit_report(
        &self,
        state: &mut IndexState,
        walk: &mut UserWalk<'_>,
        target: IndirectRef,
        depth: usize,
        report: ObjectBodyReferencesInspection,
    ) {
        let ObjectBodyReferencesInspection {
            references,
            skipped_references,
            truncation,
        } = report;
        for reference in references {
            self.enqueue(state, walk, reference, depth + 1);
        }
        state.record_scan_facts(
            walk.referrer,
            Some(target),
            skipped_references,
            truncation.is_some(),
        );
    }
}

/// Surface resolution failures that are also traversal bounds as truncation
/// facts. The unresolved edge remains recorded as the PDF-semantic fact.
fn record_resolution_truncation(
    state: &mut IndexState,
    referrer: &ObjectConsumerReferrer,
    target: IndirectRef,
    reason: &ObjectResolutionRejection,
) {
    if let ObjectResolutionRejection::ObjectStreamMemberExtraction {
        extraction_reason:
            ObjectStreamMemberExtractionRejection::DecodedObjectStreamTooLarge { length, limit },
    } = reason
    {
        state.truncate(
            Some(referrer.clone()),
            Some(target),
            ObjectConsumerIndexLimit::MaxDecodedObjectStreamBytes {
                decoded_length: *length,
                max_decoded_object_stream_bytes: *limit,
            },
        );
    }
}

/// Body-aware resolution result threaded through the cache.
enum ResolvedConsumerBody {
    /// Ordinary uncompressed object located in the source bytes.
    Uncompressed {
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
    },
    /// Compressed member whose decoded container was NOT cached (budget
    /// dropped, container table unparseable, or already-cached mismatch).
    CompressedOwned {
        /// Owned decoded object-stream body.
        decoded: Vec<u8>,
        /// Member body span within `decoded`.
        span: Range<usize>,
    },
    /// Compressed member served from the once-decode cache.
    CompressedCached {
        /// Object number of the containing object stream.
        container: usize,
        /// Member body span within the cached decoded buffer.
        span: Range<usize>,
    },
}

/// Resolve one reference, serving compressed members from the cache when the
/// container has already been decoded and adopting the decoded container on
/// first touch.
///
/// Cache misses and every non-compressed shape go through the canonical
/// resolvers so failure taxonomy stays identical to [`resolve_object`]. A
/// cached-entry mismatch (index, object number, or member shape) falls back
/// to the canonical path, which re-derives the same structured error.
fn resolve_with_cache(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    cache: &mut ObjectStreamCache,
    reference: IndirectRef,
) -> Result<ResolvedConsumerBody, ObjectResolutionError> {
    let object_number = usize::try_from(reference.object_number).unwrap_or(usize::MAX);
    if let ObjectLookupLocation::XrefStreamCompressed {
        object_stream_number,
        index_within_object_stream,
        ..
    } = locate_xref_object(lookup, object_number)
    {
        if reference.generation == 0 {
            if let Some(span) = cache.cached_member_span(
                object_stream_number,
                index_within_object_stream,
                reference.object_number,
            ) {
                return Ok(ResolvedConsumerBody::CompressedCached {
                    container: object_stream_number,
                    span,
                });
            }
        }
        return match resolve_object(
            input,
            lookup,
            reference,
            MAX_XREF_STREAM_SECTION_DECODED_BYTES,
        )? {
            ResolvedObjectData::Compressed {
                decoded_object_stream,
                object_body_span,
                ..
            } => Ok(cache.adopt(
                input,
                lookup,
                object_stream_number,
                decoded_object_stream,
                object_body_span,
            )),
            ResolvedObjectData::Uncompressed { resolved } => {
                Ok(ResolvedConsumerBody::Uncompressed {
                    object_byte_offset: resolved.object_byte_offset,
                })
            }
        };
    }

    resolve_xref_object_offset(input, lookup, reference).map(|resolved| {
        ResolvedConsumerBody::Uncompressed {
            object_byte_offset: resolved.object_byte_offset,
        }
    })
}

/// One cached decoded object-stream container plus its member table.
struct CachedObjectStream {
    /// Parsed `/First` offset of the first member body.
    first_body_byte_offset: usize,
    /// Header-pair object numbers in member order.
    member_object_numbers: Vec<usize>,
    /// Header-pair relative offsets in member order (strictly increasing).
    member_offsets: Vec<usize>,
    /// Bounded decoded object-stream body.
    decoded: Vec<u8>,
}

/// Per-container once-decode cache keyed by object-stream number.
///
/// The cache turns member resolution from `O(members x container size)` into
/// `O(containers x container size)` within a fixed total byte budget. On
/// budget overflow the whole cache is dropped and members re-decode through
/// the canonical resolver (slower but correct); the drop is a report fact.
pub(super) struct ObjectStreamCache {
    containers: BTreeMap<usize, CachedObjectStream>,
    cached_byte_count: usize,
    dropped_over_budget: bool,
}

impl ObjectStreamCache {
    pub(super) const fn new() -> Self {
        Self {
            containers: BTreeMap::new(),
            cached_byte_count: 0,
            dropped_over_budget: false,
        }
    }

    /// Snapshot the cache facts for the public report.
    pub(super) fn report(&self) -> ObjectStreamCacheReport {
        ObjectStreamCacheReport {
            budget_bytes: MAX_OBJECT_CONSUMER_CACHE_BYTES,
            cached_container_count: self.containers.len(),
            cached_byte_count: self.cached_byte_count,
            dropped_over_budget: self.dropped_over_budget,
        }
    }

    /// Serve one member's body span from an already-cached container.
    ///
    /// Returns `None` (canonical-resolver fallback) for an uncached
    /// container, an out-of-range index, an object-number mismatch, or a
    /// member body that begins with an indirect-object header, mirroring the
    /// extraction validation.
    fn cached_member_span(
        &self,
        container: usize,
        index: usize,
        object_number: u32,
    ) -> Option<Range<usize>> {
        let cached = self.containers.get(&container)?;
        let expected = usize::try_from(object_number).ok()?;
        if cached.member_object_numbers.get(index).copied()? != expected {
            return None;
        }
        let start = cached
            .first_body_byte_offset
            .checked_add(*cached.member_offsets.get(index)?)?;
        let end = match cached.member_offsets.get(index + 1) {
            Some(next) => cached.first_body_byte_offset.checked_add(*next)?,
            None => cached.decoded.len(),
        };
        let body = cached.decoded.get(start..end)?;
        if inspect_indirect_object_header(body, 0).is_ok() {
            return None;
        }
        Some(start..end)
    }

    /// Borrow one cached member body; empty on any span mismatch.
    fn member_body(&self, container: usize, span: &Range<usize>) -> &[u8] {
        self.containers
            .get(&container)
            .and_then(|cached| cached.decoded.get(span.clone()))
            .unwrap_or(&[])
    }

    /// Adopt a freshly decoded container into the cache, or keep the buffer
    /// owned when the cache is dropped, the budget would overflow, or the
    /// container table cannot be re-derived.
    fn adopt(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        container: usize,
        decoded: Vec<u8>,
        span: Range<usize>,
    ) -> ResolvedConsumerBody {
        if self.dropped_over_budget || self.containers.contains_key(&container) {
            return ResolvedConsumerBody::CompressedOwned { decoded, span };
        }
        let new_total = self.cached_byte_count.saturating_add(decoded.len());
        if new_total > MAX_OBJECT_CONSUMER_CACHE_BYTES {
            self.containers.clear();
            self.cached_byte_count = 0;
            self.dropped_over_budget = true;
            return ResolvedConsumerBody::CompressedOwned { decoded, span };
        }
        match parse_container_table(input, lookup, container, &decoded) {
            Some((first_body_byte_offset, member_object_numbers, member_offsets)) => {
                self.cached_byte_count = new_total;
                self.containers.insert(
                    container,
                    CachedObjectStream {
                        first_body_byte_offset,
                        member_object_numbers,
                        member_offsets,
                        decoded,
                    },
                );
                ResolvedConsumerBody::CompressedCached { container, span }
            }
            None => ResolvedConsumerBody::CompressedOwned { decoded, span },
        }
    }
}

/// Re-derive one container's `/First`/`/N` and header pairs for the cache.
///
/// The canonical resolver already validated the same dictionary and header
/// on the first member extraction, so this re-parse is defensive: any
/// discrepancy simply skips caching and later members use the canonical
/// (re-decoding) path.
fn parse_container_table(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    container: usize,
    decoded: &[u8],
) -> Option<(usize, Vec<usize>, Vec<usize>)> {
    let container_reference = IndirectRef {
        object_number: u32::try_from(container).ok()?,
        generation: 0,
    };
    let resolved = resolve_xref_object_offset(input, lookup, container_reference).ok()?;
    let dictionary = inspect_indirect_object_dictionary(input, resolved.object_byte_offset).ok()?;
    let object_count = unique_usize_entry(input, &dictionary.entries, b"/N")?;
    let first = unique_usize_entry(input, &dictionary.entries, b"/First")?;
    let header = decoded.get(..first)?;

    let integers = scan_header_integers(header)?;
    if integers.len() != object_count.checked_mul(2)? {
        return None;
    }
    let mut member_object_numbers = Vec::with_capacity(object_count);
    let mut member_offsets = Vec::with_capacity(object_count);
    let mut previous: Option<usize> = None;
    for pair in integers.chunks_exact(2) {
        let offset = pair[1];
        if first.checked_add(offset)? > decoded.len() {
            return None;
        }
        if previous.is_some_and(|prior| offset <= prior) {
            return None;
        }
        previous = Some(offset);
        member_object_numbers.push(pair[0]);
        member_offsets.push(offset);
    }
    Some((first, member_object_numbers, member_offsets))
}

/// Read one exact top-level key as a direct non-negative integer.
fn unique_usize_entry(input: &[u8], entries: &[DictionaryEntrySpan], key: &[u8]) -> Option<usize> {
    let entry = crate::xref_stream::unique_entry(input, entries, key).ok()??;
    parse_non_negative_integer(input.get(entry.value_range.start..entry.value_range.end)?).ok()
}

/// Scan whitespace-separated non-negative decimal integers, or `None` on any
/// malformed or overflowing token.
fn scan_header_integers(header: &[u8]) -> Option<Vec<usize>> {
    let mut values = Vec::new();
    let mut cursor = 0;
    while cursor < header.len() {
        while cursor < header.len() && is_pdf_whitespace(header[cursor]) {
            cursor += 1;
        }
        if cursor >= header.len() {
            break;
        }
        let token_start = cursor;
        while cursor < header.len() && !is_pdf_whitespace(header[cursor]) {
            cursor += 1;
        }
        values.push(parse_non_negative_integer(&header[token_start..cursor]).ok()?);
    }
    Some(values)
}
