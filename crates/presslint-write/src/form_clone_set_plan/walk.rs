//! Iterative bounded reached-Form closure walker.
//!
//! Mechanics only: the FIFO worklist, the explicit budgets, and the
//! fail-closed frontier preflight over every dictionary-bodied closure
//! member. The seed qualification, set grouping, reservation, and counts
//! projection live in the parent module.
//!
//! The walk is ITERATIVE only (`VecDeque`, no recursion — the
//! PDFBOX-2574/-5232 and pypdf #2098/#2839 recursive-cloner failure class),
//! with a visited map keyed by exact [`IndirectRef`] identity registered
//! BEFORE enqueue (PDFBOX-4477: identity keys, never value equality), edges
//! examined in body source order, and every budget explicit. A
//! self-referential indirect `/Length` (pypdf #3112) is an ordinary cycle the
//! visited map terminates, not a frontier cut.

// Crate-private walker mechanics inside a private module: `pub(crate)` is the
// deliberate crate-wide seam the plan and tests consume.
#![allow(clippy::redundant_pub_crate)]

use std::collections::{BTreeSet, VecDeque};

use presslint_pdf::{
    DictionaryEntrySpan, DictionaryValueKind, FlateDecodeStreamRejection,
    IndirectObjectDictionaryInspectionRejection, IndirectRef, ObjectLookup, ObjectLookupLocation,
    ObjectResolutionError, ObjectResolutionRejection, ObjectStreamMemberExtractionRejection,
    ResolvedObjectData, ResolvedObjectDictionaryInspection,
    ResolvedObjectDictionaryInspectionRejection, inspect_object_body_references_resolved,
    inspect_object_dictionary, locate_xref_object, resolve_object,
};

use super::{
    AncillaryKeyClass, CloneMemberLocator, CloneSetMember, CloneSetRefusal, GraphEscapeClass,
    NullEquivalentVerdict, StructuralFrontier,
};
use crate::page_xobject_policy::decode_pdf_name;

/// Deepest reference chain admitted from the root Form (edges, root at 0).
pub(crate) const MAX_CLONE_SET_REFERENCE_DEPTH: usize = 64;

/// Unique resolved closure members per clone set. Terminal null-equivalent
/// identities do not consume this allocation budget; their discovery is
/// independently bounded by the cumulative reference-occurrence budget.
pub(crate) const MAX_CLONE_SET_MEMBERS: usize = 4096;

/// Cumulative outgoing-reference occurrences accepted per clone set. Like
/// the fresh-floor proof this is an accepted-accumulation budget, not a hard
/// cap on scanner work: each delegated body scan may first discover up to
/// [`presslint_pdf::MAX_OBJECT_BODY_REFERENCES`] (65,536) references before
/// this budget refuses the set.
pub(crate) const MAX_CLONE_SET_REFERENCE_OCCURRENCES: usize = 4096;

/// Cumulative object-stream decode work, in decoded bytes, per clone set.
/// The RESIDUAL budget is passed into every [`resolve_object`] call, so a
/// later container decode is cut off the instant it would exceed the
/// cumulative cap instead of paying its full cost first.
pub(crate) const MAX_CLONE_SET_DECODE_WORK_BYTES: usize = 1_048_576;

/// Budget usage retained per clone set. On a budget refusal the failing
/// budget's residual is exhausted before refusing, so the recorded usage
/// honestly reads as fully spent and nothing looks retryable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CloneSetBudgetUsage {
    /// Deepest examined reference depth (root is 0).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) max_depth_reached: usize,
    /// Unique resolved closure members accepted.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) unique_members: usize,
    /// Accepted outgoing-reference occurrences.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) reference_occurrences: usize,
    /// Charged object-stream decode work in decoded bytes.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) decode_work_bytes: usize,
}

/// The completed closure of one clone set.
#[derive(Debug)]
pub(crate) struct ClosureMembers {
    /// Members ordered deterministically by source reference.
    pub(crate) members: Vec<CloneSetMember>,
    /// Terminal null-equivalent verdicts in FIFO discovery order.
    pub(crate) null_equivalents: Vec<NullEquivalentVerdict>,
}

/// One walk's budget usage plus its members-or-first-refusal result.
pub(crate) struct ClosureWalkOutcome {
    /// Budget usage, retained on success AND refusal.
    pub(crate) budget: CloneSetBudgetUsage,
    /// The closure, or the exact first refusal in deterministic order.
    pub(crate) result: Result<ClosureMembers, CloneSetRefusal>,
}

/// Walk the bounded reached-Form closure from `root`.
///
/// FIFO worklist; visited-map insertion before enqueue; exact `IndirectRef`
/// identity keys; source-order edge examination; deterministic member
/// ordering by source reference. Free/not-found identities are terminal
/// null-equivalent verdicts; every other irregularity refuses the whole set.
pub(crate) fn walk_reached_form_closure(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    frontier: &StructuralFrontier,
    root: IndirectRef,
) -> ClosureWalkOutcome {
    let mut budgets = WalkBudgets::new();
    let result = run_walk(input, lookup, frontier, root, &mut budgets);
    ClosureWalkOutcome {
        budget: budgets.usage(),
        result,
    }
}

fn run_walk(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    frontier: &StructuralFrontier,
    root: IndirectRef,
    budgets: &mut WalkBudgets,
) -> Result<ClosureMembers, CloneSetRefusal> {
    let mut visited: BTreeSet<IndirectRef> = BTreeSet::new();
    let mut queue: VecDeque<(IndirectRef, usize)> = VecDeque::new();
    visited.insert(root);
    queue.push_back((root, 0));
    let mut members = Vec::new();
    let mut null_equivalents = Vec::new();

    while let Some((reference, depth)) = queue.pop_front() {
        budgets.max_depth_reached = budgets.max_depth_reached.max(depth);
        if let Some(escape) = frontier.classify(reference) {
            return Err(CloneSetRefusal::GraphEscape {
                member: reference,
                escape,
            });
        }
        match classify_location(lookup, reference) {
            LocationClass::NullEquivalent { free_entry } => {
                null_equivalents.push(NullEquivalentVerdict {
                    reference,
                    free_entry,
                });
                continue;
            }
            LocationClass::Ambiguous => {
                return Err(CloneSetRefusal::AmbiguousLookup { member: reference });
            }
            LocationClass::Resolvable => {}
        }
        budgets.record_member()?;

        let resolved = resolve_member(input, lookup, reference, budgets)?;
        if let ResolvedObjectData::Compressed {
            decoded_object_stream,
            ..
        } = &resolved
        {
            budgets.record_decode_work(decoded_object_stream.len())?;
        }
        preflight_member(input, &resolved, reference)?;

        let scan = inspect_object_body_references_resolved(input, &resolved)
            .map_err(|_| CloneSetRefusal::BodyScanIncomplete { member: reference })?;
        if scan.truncation.is_some() || !scan.skipped_references.is_empty() {
            return Err(CloneSetRefusal::BodyScanIncomplete { member: reference });
        }
        budgets.record_references(scan.references.len())?;

        for &child in &scan.references {
            if !visited.insert(child) {
                continue;
            }
            if depth + 1 > MAX_CLONE_SET_REFERENCE_DEPTH {
                budgets.max_depth_reached = MAX_CLONE_SET_REFERENCE_DEPTH;
                return Err(CloneSetRefusal::DepthBudgetExhausted {
                    max_depth: MAX_CLONE_SET_REFERENCE_DEPTH,
                });
            }
            queue.push_back((child, depth + 1));
        }

        members.push(CloneSetMember {
            source: reference,
            locator: member_locator(&resolved),
            outgoing: scan.references,
        });
    }

    members.sort_by_key(|member| member.source);
    Ok(ClosureMembers {
        members,
        null_equivalents,
    })
}

/// How one located identity is handled by the walk.
enum LocationClass {
    /// Free or absent identity: terminal null-equivalent (ISO 32000-1
    /// §7.3.9–7.3.10), token preserved, nothing allocated.
    NullEquivalent {
        /// True for an explicit free entry, false for an absent identity.
        free_entry: bool,
    },
    /// Ambiguous, reserved, or out-of-range entry: refuse.
    Ambiguous,
    /// An in-use identity to resolve.
    Resolvable,
}

fn classify_location(lookup: ObjectLookup<'_>, reference: IndirectRef) -> LocationClass {
    let object_number = usize::try_from(reference.object_number).unwrap_or(usize::MAX);
    match locate_xref_object(lookup, object_number) {
        ObjectLookupLocation::ClassicFree { .. } | ObjectLookupLocation::XrefStreamFree { .. } => {
            LocationClass::NullEquivalent { free_entry: true }
        }
        ObjectLookupLocation::ClassicNotFound { .. }
        | ObjectLookupLocation::XrefStreamNotFound { .. } => {
            LocationClass::NullEquivalent { free_entry: false }
        }
        ObjectLookupLocation::ClassicInUse { .. }
        | ObjectLookupLocation::XrefStreamUncompressed { .. }
        | ObjectLookupLocation::XrefStreamCompressed { .. } => LocationClass::Resolvable,
        ObjectLookupLocation::ClassicAmbiguous { .. }
        | ObjectLookupLocation::ClassicObjectNumberOutOfRange { .. }
        | ObjectLookupLocation::XrefStreamUncompressedGenerationOutOfRange { .. }
        | ObjectLookupLocation::XrefStreamFreeGenerationOutOfRange { .. }
        | ObjectLookupLocation::XrefStreamReserved { .. }
        | ObjectLookupLocation::XrefStreamObjectNumberOutOfRange { .. } => LocationClass::Ambiguous,
    }
}

/// Resolve one in-use member, bounding any compressed-container decode by the
/// RESIDUAL cumulative decode budget. A rejection caused by that reduced
/// bound exhausts the budget and refuses as decode work spent — matching what
/// the eventual cumulative charge would have reported — while every other
/// failure (generation mismatch, header mismatch, malformed extraction) is a
/// resolution refusal.
fn resolve_member(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reference: IndirectRef,
    budgets: &mut WalkBudgets,
) -> Result<ResolvedObjectData, CloneSetRefusal> {
    resolve_object(input, lookup, reference, budgets.decode_budget).map_err(|error| {
        if is_decode_work_limit_rejection(&error) {
            budgets.exhaust_decode_budget();
            CloneSetRefusal::DecodeWorkBudgetExhausted {
                max_decoded_bytes: MAX_CLONE_SET_DECODE_WORK_BYTES,
            }
        } else {
            CloneSetRefusal::ResolutionFailed { member: reference }
        }
    })
}

/// True when `error` is exactly the compressed-member decode rejection caused
/// by the caller-supplied decode-byte bound, not a genuinely malformed
/// object stream (same classification as the fresh-floor proof).
const fn is_decode_work_limit_rejection(error: &ObjectResolutionError) -> bool {
    matches!(
        error.reason,
        ObjectResolutionRejection::ObjectStreamMemberExtraction {
            extraction_reason: ObjectStreamMemberExtractionRejection::DecodedObjectStreamTooLarge { .. }
                | ObjectStreamMemberExtractionRejection::FlateDecode {
                    flate_reason: FlateDecodeStreamRejection::OutputLimitExceeded,
                },
        }
    )
}

const fn member_locator(resolved: &ResolvedObjectData) -> CloneMemberLocator {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => CloneMemberLocator::Uncompressed {
            object_byte_offset: resolved.object_byte_offset,
        },
        ResolvedObjectData::Compressed {
            object_stream_number,
            index_within_object_stream,
            ..
        } => CloneMemberLocator::Compressed {
            object_stream_number: *object_stream_number,
            index_within_object_stream: *index_within_object_stream,
        },
    }
}

/// Frontier preflight over one member's TOP-LEVEL dictionary keys.
///
/// Applies to every dictionary-bodied member (Form roots included): the
/// ancillary companion keys and `/StructParent(s)` integers are invisible to
/// a reference walker, so only a decoded-name, duplicate-sensitive key scan
/// can catch them (PDFBOX-5372 class). Non-dictionary bodies (arrays,
/// scalars, an indirect `/Length` integer) have no keys to preflight and
/// pass through; a malformed dictionary refuses.
fn preflight_member(
    input: &[u8],
    resolved: &ResolvedObjectData,
    member: IndirectRef,
) -> Result<(), CloneSetRefusal> {
    let inspection = match inspect_object_dictionary(input, resolved) {
        Ok(inspection) => inspection,
        Err(error) => {
            return if is_non_dictionary_body(error.reason) {
                Ok(())
            } else {
                Err(CloneSetRefusal::MalformedStructuralKeys { member })
            };
        }
    };
    let (buffer, entries): (&[u8], &[DictionaryEntrySpan]) = match (&inspection, resolved) {
        (ResolvedObjectDictionaryInspection::Uncompressed(uncompressed), _) => {
            (input, &uncompressed.entries)
        }
        (
            ResolvedObjectDictionaryInspection::Compressed(compressed),
            ResolvedObjectData::Compressed {
                decoded_object_stream,
                object_body_span,
                ..
            },
        ) => {
            // Compressed entry spans are relative to the extracted member
            // body slice, not to the source input.
            let body = decoded_object_stream
                .get(object_body_span.start..object_body_span.end)
                .ok_or(CloneSetRefusal::MalformedStructuralKeys { member })?;
            (body, &compressed.entries)
        }
        (
            ResolvedObjectDictionaryInspection::Compressed(_),
            ResolvedObjectData::Uncompressed { .. },
        ) => {
            return Err(CloneSetRefusal::MalformedStructuralKeys { member });
        }
    };
    preflight_dictionary_entries(buffer, entries, member)
}

const fn is_non_dictionary_body(reason: ResolvedObjectDictionaryInspectionRejection) -> bool {
    matches!(
        reason,
        ResolvedObjectDictionaryInspectionRejection::Uncompressed {
            object_dictionary_reason:
                IndirectObjectDictionaryInspectionRejection::NonDictionaryBody { .. },
        } | ResolvedObjectDictionaryInspectionRejection::CompressedNonDictionaryBody { .. }
    )
}

fn preflight_dictionary_entries(
    buffer: &[u8],
    entries: &[DictionaryEntrySpan],
    member: IndirectRef,
) -> Result<(), CloneSetRefusal> {
    let mut seen_keys: BTreeSet<Vec<u8>> = BTreeSet::new();
    for entry in entries {
        let raw_key = buffer
            .get(entry.key_range.start..entry.key_range.end)
            .filter(|raw| raw.first() == Some(&b'/'))
            .ok_or(CloneSetRefusal::MalformedStructuralKeys { member })?;
        let decoded = decode_pdf_name(&raw_key[1..])
            .ok_or(CloneSetRefusal::MalformedStructuralKeys { member })?;
        // Duplicate-sensitive: ANY duplicated decoded top-level key makes
        // the dictionary ambiguous, so raw-spelling escapes cannot bypass.
        if !seen_keys.insert(decoded.clone().into_owned()) {
            return Err(CloneSetRefusal::MalformedStructuralKeys { member });
        }
        if let Some(key) = ancillary_key_class(decoded.as_ref()) {
            return Err(CloneSetRefusal::AncillaryKey { member, key });
        }
        if decoded.as_ref() == b"Type" {
            check_type_value(buffer, entry, member)?;
        }
    }
    Ok(())
}

/// Classify one decoded `/Type` name value; a graph-node type refuses, a
/// non-name or undecodable value is an ambiguous structural key.
fn check_type_value(
    buffer: &[u8],
    entry: &DictionaryEntrySpan,
    member: IndirectRef,
) -> Result<(), CloneSetRefusal> {
    if entry.value_kind != DictionaryValueKind::Name {
        return Err(CloneSetRefusal::MalformedStructuralKeys { member });
    }
    let raw_value = buffer
        .get(entry.value_range.start..entry.value_range.end)
        .filter(|raw| raw.first() == Some(&b'/'))
        .ok_or(CloneSetRefusal::MalformedStructuralKeys { member })?;
    let decoded = decode_pdf_name(&raw_value[1..])
        .ok_or(CloneSetRefusal::MalformedStructuralKeys { member })?;
    if let Some(escape) = graph_escape_class(decoded.as_ref()) {
        return Err(CloneSetRefusal::GraphEscape { member, escape });
    }
    Ok(())
}

const fn graph_escape_class(decoded_type: &[u8]) -> Option<GraphEscapeClass> {
    match decoded_type {
        b"Catalog" => Some(GraphEscapeClass::Catalog),
        b"Pages" => Some(GraphEscapeClass::Pages),
        b"Page" => Some(GraphEscapeClass::Page),
        b"StructTreeRoot" => Some(GraphEscapeClass::StructTreeRoot),
        b"StructElem" => Some(GraphEscapeClass::StructElem),
        b"OCG" => Some(GraphEscapeClass::OptionalContentGroup),
        b"OCMD" => Some(GraphEscapeClass::OptionalContentMembership),
        _ => None,
    }
}

const fn ancillary_key_class(decoded_key: &[u8]) -> Option<AncillaryKeyClass> {
    match decoded_key {
        b"OC" => Some(AncillaryKeyClass::OptionalContent),
        b"Ref" => Some(AncillaryKeyClass::ReferenceXObject),
        b"F" => Some(AncillaryKeyClass::ExternalFile),
        b"OPI" => Some(AncillaryKeyClass::Opi),
        b"PieceInfo" => Some(AncillaryKeyClass::PieceInfo),
        b"StructParent" => Some(AncillaryKeyClass::StructParent),
        b"StructParents" => Some(AncillaryKeyClass::StructParents),
        _ => None,
    }
}

/// Residual budgets threaded through one closure walk, with the honest usage
/// projection retained per set.
struct WalkBudgets {
    max_depth_reached: usize,
    unique_members: usize,
    reference_budget: usize,
    decode_budget: usize,
}

impl WalkBudgets {
    const fn new() -> Self {
        Self {
            max_depth_reached: 0,
            unique_members: 0,
            reference_budget: MAX_CLONE_SET_REFERENCE_OCCURRENCES,
            decode_budget: MAX_CLONE_SET_DECODE_WORK_BYTES,
        }
    }

    /// Charge one unique resolved closure member. The failing member exhausts
    /// the residual before refusing; terminal null-equivalents never call
    /// this method because they allocate no clone identity.
    const fn record_member(&mut self) -> Result<(), CloneSetRefusal> {
        if self.unique_members < MAX_CLONE_SET_MEMBERS {
            self.unique_members += 1;
            Ok(())
        } else {
            self.unique_members = MAX_CLONE_SET_MEMBERS;
            Err(CloneSetRefusal::MemberBudgetExhausted {
                max_members: MAX_CLONE_SET_MEMBERS,
            })
        }
    }

    /// Charge accepted outgoing-reference occurrences; an overdraft exhausts
    /// the residual before refusing (nothing is retryable).
    const fn record_references(&mut self, count: usize) -> Result<(), CloneSetRefusal> {
        if let Some(residual) = self.reference_budget.checked_sub(count) {
            self.reference_budget = residual;
            Ok(())
        } else {
            self.reference_budget = 0;
            Err(CloneSetRefusal::ReferenceBudgetExhausted {
                max_occurrences: MAX_CLONE_SET_REFERENCE_OCCURRENCES,
            })
        }
    }

    /// Charge actual decoded object-stream bytes; an overdraft exhausts the
    /// residual before refusing.
    const fn record_decode_work(&mut self, bytes: usize) -> Result<(), CloneSetRefusal> {
        if let Some(residual) = self.decode_budget.checked_sub(bytes) {
            self.decode_budget = residual;
            Ok(())
        } else {
            self.decode_budget = 0;
            Err(CloneSetRefusal::DecodeWorkBudgetExhausted {
                max_decoded_bytes: MAX_CLONE_SET_DECODE_WORK_BYTES,
            })
        }
    }

    const fn exhaust_decode_budget(&mut self) {
        self.decode_budget = 0;
    }

    const fn usage(&self) -> CloneSetBudgetUsage {
        CloneSetBudgetUsage {
            max_depth_reached: self.max_depth_reached,
            unique_members: self.unique_members,
            reference_occurrences: MAX_CLONE_SET_REFERENCE_OCCURRENCES - self.reference_budget,
            decode_work_bytes: MAX_CLONE_SET_DECODE_WORK_BYTES - self.decode_budget,
        }
    }
}
