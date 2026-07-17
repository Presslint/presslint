//! Staged clone-body export: corroborate, materialize, and fully validate
//! every planned clone set as a [`FreshObjectBytes`] batch — held one step
//! short of the writer.
//!
//! STAGED/VALIDATE-ONLY: the batch this module builds is never handed to the
//! writer by production callers, so emitted product bytes stay byte-identical
//! to the plan-only behaviour. The build is ALL-OR-NOTHING: any failure
//! discards the entire request batch with one typed
//! [`CloneSetExportRefusal`]; no per-set salvage and no second reservation
//! ever happens after a failure (pikepdf#271 class — a source-to-fresh map
//! never outlives its one build).
//!
//! # Corroboration (all must hold, fail-closed)
//!
//! Input length; per-member locator re-resolution (exact uncompressed offset,
//! or object-stream container + index); root uniqueness at its retained
//! offset; re-scan census EXACTLY equal to the retained duplicate-preserving
//! outgoing list (count, order, values, generations; zero skips or
//! truncation); the injective aligned generation-zero source-to-fresh
//! mapping with contiguous reservation coverage in plan order; and, after
//! rewriting, re-scan equality against the translated census. The re-scan
//! uses the range-bearing sibling of the plan's census scanner — one shared
//! scanner core, so plan-scan and export-scan are token-identical by
//! construction. Every uncompressed member must additionally prove its
//! indirect object holds exactly ONE supported value followed only by
//! whitespace/comments and one delimiter-bounded `endobj` token (§7.3.10),
//! token-scanned from the classified value end — never a raw `endobj` byte
//! search; compressed-member admission is unchanged (§7.5.7 has no
//! `endobj`).
//!
//! # Materialization
//!
//! Bodies are source bytes with ONLY the object-number and generation numeric
//! tokens of in-set references spliced to their fresh identities — `R`
//! keywords, whitespace, interior comments, and every other byte preserved
//! exactly (reverse-order splices, the house pattern). Stream members keep
//! their original framing byte-for-byte: dictionary bytes and trivia, the
//! bytes between the dictionary close and the `stream` keyword, the stream
//! keyword EOL as found (LF or CRLF; a lone CR refuses per §7.3.8.1), stream
//! data untouched, and the EOL before `endstream`. References are scanned
//! only in the dictionary, never in stream data. `/Length` verdicts: in-set
//! indirect rewrites the reference and copies the integer object verbatim;
//! self-referential refuses explicitly (pypdf#3112 — the plan admits it as an
//! ordinary cycle); out-of-set resolvable is a plan-consistency refusal;
//! null-equivalent or extent-mismatched refuses — never repaired, never
//! recomputed. Object-stream members materialize as plain generation-zero
//! bodies from the decoded container span (ranges rebased against
//! `object_body_span`), after a private admission check for one-value
//! consumption and the §7.5.7 "not solely a reference" rule that member
//! extraction does not prove; the per-set decode budget is re-applied during
//! export re-resolution.
//!
//! # Budget
//!
//! One request-level cumulative [`MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES`]
//! bound on final materialized body lengths, charged fail-closed:
//! corroborated extents are pre-charged BEFORE any copy work, and the exact
//! final length (after reference-width changes) is settled with checked
//! arithmetic BEFORE the body allocation. Duplicate bodies across
//! page-specific sets count each time. Its typed refusal is distinct from
//! the closure and reservation refusal classes.

// Crate-private export mechanics inside a private module: `pub(crate)` is the
// deliberate crate-wide seam the plan and tests consume.
#![allow(clippy::redundant_pub_crate)]

use std::collections::BTreeMap;
use std::ops::Range;

use presslint_pdf::{
    ContentStreamStartInspection, ContentStreamStartInspectionRejection, DictionaryEntrySpan,
    DictionaryValueKind, FlateDecodeStreamRejection, IndirectObjectBodyLeadingTokenKind,
    IndirectRef, ObjectBodyReferenceRangesInspection, ObjectLookup, ObjectLookupLocation,
    ObjectResolutionError, ObjectResolutionRejection, ObjectStreamMemberExtractionRejection,
    ResolvedObjectData, inspect_array_extent, inspect_content_stream_data_extent_with_lookup,
    inspect_content_stream_start, inspect_dictionary_extent, inspect_indirect_object_body_token,
    inspect_indirect_object_header, inspect_object_body_reference_ranges, locate_xref_object,
    parse_indirect_reference, resolve_object, scan_indirect_reference_ranges_in_span,
    scan_indirect_references_in_span,
};

use super::walk::MAX_CLONE_SET_DECODE_WORK_BYTES;
use super::{CloneMemberLocator, CloneSetMember, CloneSetOutcome, FormCloneSet, FormCloneSetPlan};
use crate::page_xobject_policy::decode_pdf_name;
use crate::writer::FreshObjectBytes;

/// Request-level cumulative bound on final materialized clone-body lengths
/// (qpdf#219 bloat class). Duplicate bodies across page-specific sets count
/// each time; the typed refusal is distinct from every closure and
/// reservation class.
pub(crate) const MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES: usize = 64 * 1024 * 1024;

const LENGTH_KEY: &[u8] = b"/Length";
const ENDSTREAM_KEYWORD_LEN: usize = b"endstream".len();
const ENDOBJ_KEYWORD: &[u8] = b"endobj";

/// One staged set's fold-ready totals, indexed back into the plan's set list.
#[derive(Debug)]
pub(crate) struct StagedCloneSet {
    /// Index into `FormCloneSetPlan::sets` (plan order).
    pub(crate) set_index: usize,
    /// Materialized member bodies in this set.
    pub(crate) member_count: usize,
    /// Total materialized body bytes in this set.
    pub(crate) body_bytes_total: usize,
}

/// The fully validated request batch, in plan order (set order, then member
/// source-reference order — the reservation partition order).
#[derive(Debug)]
pub(crate) struct StagedExportBatch {
    /// Validated fresh bodies covering the reservation contiguously.
    pub(crate) fresh_objects: Vec<FreshObjectBytes>,
    /// Per-set staged totals for the atomic counter commit.
    pub(crate) staged_sets: Vec<StagedCloneSet>,
}

/// Typed reason the whole staged-export batch was discarded, fail-closed.
/// Exactly the FIRST refusal in deterministic plan/member order is retained.
/// This class is deliberately distinct from the closure-walk and reservation
/// refusal classes: an export failure never masquerades as either.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneSetExportRefusal {
    /// The borrowed input length no longer matches the plan's retained
    /// length (defensive: the plan and export share one borrowed frame).
    InputLengthMismatch {
        /// Length the plan was computed over.
        #[cfg_attr(not(test), allow(dead_code))]
        expected: usize,
        /// Length supplied to the export.
        #[cfg_attr(not(test), allow(dead_code))]
        found: usize,
    },
    /// A set's source-to-fresh pairs are missing, misaligned with its
    /// members, or not injective.
    MappingMisaligned {
        /// Root of the inconsistent set.
        #[cfg_attr(not(test), allow(dead_code))]
        root: IndirectRef,
    },
    /// A set repeats one source identity; collecting it into the source map
    /// would silently collapse the duplicate.
    MappingDuplicateSource {
        /// The repeated source identity.
        #[cfg_attr(not(test), allow(dead_code))]
        source: IndirectRef,
    },
    /// A fresh identity carries a nonzero generation.
    MappingNotGenerationZero {
        /// The offending fresh identity.
        #[cfg_attr(not(test), allow(dead_code))]
        fresh: IndirectRef,
    },
    /// The fresh identities do not cover the reservation contiguously in
    /// plan order.
    ReservationNotContiguous {
        /// Expected next fresh object number.
        #[cfg_attr(not(test), allow(dead_code))]
        expected: u32,
        /// Fresh identity actually found.
        #[cfg_attr(not(test), allow(dead_code))]
        found: IndirectRef,
    },
    /// The set's root is not exactly one of its members.
    RootNotUnique {
        /// The set root.
        #[cfg_attr(not(test), allow(dead_code))]
        root: IndirectRef,
    },
    /// The root member's locator does not match the retained root offset.
    RootOffsetMismatch {
        /// The set root.
        #[cfg_attr(not(test), allow(dead_code))]
        root: IndirectRef,
    },
    /// A member no longer resolves (generation, header, or extraction).
    MemberResolutionFailed {
        /// The unresolved member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A member re-resolved to a different locator than the plan retained.
    LocatorMismatch {
        /// The relocated member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// The per-set object-stream decode budget was spent during export
    /// re-resolution (the residual is exhausted before refusing).
    DecodeWorkBudgetExhausted {
        /// Configured per-set decode budget in decoded bytes.
        #[cfg_attr(not(test), allow(dead_code))]
        max_decoded_bytes: usize,
    },
    /// A member's re-scan census does not equal the retained outgoing list
    /// exactly (count, order, values, generations, zero skips/truncation),
    /// or the re-scan itself failed where the plan scan succeeded.
    CensusMismatch {
        /// The member whose census failed to corroborate.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A member body shape this slice does not materialize (string-led
    /// bodies, or a body whose extent cannot be located).
    UnsupportedBodyShape {
        /// The member with the unsupported body.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// An uncompressed member's indirect object does not hold exactly one
    /// supported value followed only by PDF whitespace/comments and one
    /// delimiter-bounded `endobj` token (§7.3.10): a second value, trailing
    /// non-trivia, a malformed digit-led scalar (`12foo`), a misspelled
    /// keyword (`endobjX`), or a missing `endobj` all refuse. Token-scanned
    /// from the classified value end — never a raw byte search, never a
    /// repair.
    UncompressedMemberNotSingleValue {
        /// The member whose object framing failed the admission.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member's framing violates §7.3.8.1 (a lone CR or other
    /// invalid EOL after the `stream` keyword).
    StreamFraming {
        /// The stream member with invalid framing.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member has no semantic top-level `/Length` key.
    LengthMissing {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member has more than one semantic top-level `/Length` key.
    LengthDuplicate {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member has one semantic `/Length` key, but its source spelling
    /// is escaped. The raw-only extent helper cannot safely consume it.
    LengthNonCanonical {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member's `/Length` value is neither a direct integer nor an
    /// indirect reference (an explicit `null` included).
    LengthUnsupportedShape {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member's `/Length` reference names the member itself
    /// (pypdf#3112): the plan admits the cycle, the export refuses it.
    LengthSelfReferential {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member's `/Length` reference resolves outside the clone set:
    /// the closure walk would have admitted it, so the plan is inconsistent.
    LengthOutOfSet {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A stream member's `/Length` reference is free or absent
    /// (null-equivalent): the extent cannot corroborate — refuse, never
    /// repair or recompute.
    LengthNullEquivalent {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// The declared `/Length` does not corroborate the actual framing
    /// (data end, EOL, `endstream`) — refuse, never repair or recompute.
    LengthExtentMismatch {
        /// The stream member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A compressed member's decoded span does not consume exactly one value
    /// (§7.5.7 admission the member extraction does not prove).
    ObjectStreamMemberNotSingleValue {
        /// The compressed member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A compressed member's body consists solely of an indirect reference,
    /// which §7.5.7 forbids inside an object stream.
    ObjectStreamMemberSolelyReference {
        /// The compressed member.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// The request-level cumulative materialized-body budget was exceeded
    /// (checked BEFORE decode/copy work and again before each allocation).
    MaterializedBodyBudgetExceeded {
        /// Configured cumulative budget in bytes.
        #[cfg_attr(not(test), allow(dead_code))]
        max_body_bytes: usize,
    },
    /// A splice range fell outside the materialized body span (defensive
    /// fail-closed guard; never expected).
    SpliceOutOfBounds {
        /// The member whose splice was out of bounds.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// The post-rewrite re-scan of a materialized body does not equal the
    /// translated census.
    PostRewriteCensusMismatch {
        /// The member whose rewritten body failed verification.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
}

/// Build the fully validated request batch over every planned set, in plan
/// order, or discard everything with one typed refusal.
pub(crate) fn build_staged_export(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    plan: &FormCloneSetPlan,
) -> Result<StagedExportBatch, CloneSetExportRefusal> {
    if input.len() != plan.input_byte_len {
        return Err(CloneSetExportRefusal::InputLengthMismatch {
            expected: plan.input_byte_len,
            found: input.len(),
        });
    }

    let mut batch = StagedExportBatch {
        fresh_objects: Vec::new(),
        staged_sets: Vec::new(),
    };
    let mut budget = MaterializedBodyBudget { charged: 0 };
    let mut expected_next_fresh: Option<u32> = None;

    for (set_index, set) in plan.sets.iter().enumerate() {
        let CloneSetOutcome::Planned {
            members,
            source_to_fresh,
            ..
        } = &set.outcome
        else {
            continue;
        };
        // The validator builds the source map once while proving duplicates
        // cannot collapse; the map never outlives this set's build.
        let fresh_by_source =
            validate_set_mapping(set, members, source_to_fresh, &mut expected_next_fresh)?;

        let mut decode_budget = MAX_CLONE_SET_DECODE_WORK_BYTES;
        let mut body_bytes_total = 0usize;
        for (member, (_, fresh)) in members.iter().zip(source_to_fresh) {
            let body = materialize_member(
                input,
                lookup,
                member,
                &fresh_by_source,
                &mut decode_budget,
                &mut budget,
            )?;
            body_bytes_total += body.len();
            batch.fresh_objects.push(FreshObjectBytes {
                reference: *fresh,
                body_bytes: body,
            });
        }
        batch.staged_sets.push(StagedCloneSet {
            set_index,
            member_count: members.len(),
            body_bytes_total,
        });
    }

    Ok(batch)
}

/// Validate one set's aligned generation-zero mapping, batch-wide contiguous
/// reservation coverage, and root uniqueness at the retained offset, while
/// building the source map used by materialization.
fn validate_set_mapping(
    set: &FormCloneSet,
    members: &[CloneSetMember],
    source_to_fresh: &[(IndirectRef, IndirectRef)],
    expected_next_fresh: &mut Option<u32>,
) -> Result<BTreeMap<IndirectRef, IndirectRef>, CloneSetExportRefusal> {
    if source_to_fresh.len() != members.len() {
        return Err(CloneSetExportRefusal::MappingMisaligned { root: set.root });
    }
    let mut fresh_by_source = BTreeMap::new();
    for (member, (source, fresh)) in members.iter().zip(source_to_fresh) {
        if *source != member.source {
            return Err(CloneSetExportRefusal::MappingMisaligned { root: set.root });
        }
        if fresh_by_source.insert(*source, *fresh).is_some() {
            return Err(CloneSetExportRefusal::MappingDuplicateSource { source: *source });
        }
        if fresh.generation != 0 {
            return Err(CloneSetExportRefusal::MappingNotGenerationZero { fresh: *fresh });
        }
        if let Some(expected) = *expected_next_fresh {
            if fresh.object_number != expected {
                return Err(CloneSetExportRefusal::ReservationNotContiguous {
                    expected,
                    found: *fresh,
                });
            }
        }
        // Contiguous ascending coverage also proves the whole batch mapping
        // is injective: no fresh identity can repeat.
        *expected_next_fresh = Some(fresh.object_number.wrapping_add(1));
    }

    let mut root_members = members.iter().filter(|member| member.source == set.root);
    let Some(root_member) = root_members.next() else {
        return Err(CloneSetExportRefusal::RootNotUnique { root: set.root });
    };
    if root_members.next().is_some() {
        return Err(CloneSetExportRefusal::RootNotUnique { root: set.root });
    }
    let CloneMemberLocator::Uncompressed { object_byte_offset } = root_member.locator else {
        return Err(CloneSetExportRefusal::RootOffsetMismatch { root: set.root });
    };
    if object_byte_offset != set.root_object_byte_offset {
        return Err(CloneSetExportRefusal::RootOffsetMismatch { root: set.root });
    }
    Ok(fresh_by_source)
}

/// Re-resolve, corroborate, and materialize one member body.
fn materialize_member(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    member: &CloneSetMember,
    fresh_by_source: &BTreeMap<IndirectRef, IndirectRef>,
    decode_budget: &mut usize,
    budget: &mut MaterializedBodyBudget,
) -> Result<Vec<u8>, CloneSetExportRefusal> {
    let resolved = resolve_member(input, lookup, member, decode_budget)?;
    match &resolved {
        ResolvedObjectData::Uncompressed { resolved } => materialize_uncompressed(
            input,
            lookup,
            member,
            resolved.object_byte_offset,
            fresh_by_source,
            budget,
        ),
        ResolvedObjectData::Compressed {
            decoded_object_stream,
            object_body_span,
            ..
        } => materialize_compressed(
            decoded_object_stream,
            object_body_span.clone(),
            member,
            fresh_by_source,
            budget,
        ),
    }
}

/// Re-resolve one member at its retained locator, re-applying the per-set
/// decode budget as the residual bound (the walk's charging pattern), and
/// corroborate the resolved locator against the plan.
fn resolve_member(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    member: &CloneSetMember,
    decode_budget: &mut usize,
) -> Result<ResolvedObjectData, CloneSetExportRefusal> {
    let resolved =
        resolve_object(input, lookup, member.source, *decode_budget).map_err(|error| {
            if is_decode_work_limit_rejection(&error) {
                *decode_budget = 0;
                CloneSetExportRefusal::DecodeWorkBudgetExhausted {
                    max_decoded_bytes: MAX_CLONE_SET_DECODE_WORK_BYTES,
                }
            } else {
                CloneSetExportRefusal::MemberResolutionFailed {
                    member: member.source,
                }
            }
        })?;

    // Charge the ACTUAL decoded container length against the per-set budget
    // (the residual was already the per-call bound; an overdraft exhausts the
    // residual before refusing, mirroring the walk).
    if let ResolvedObjectData::Compressed {
        decoded_object_stream,
        ..
    } = &resolved
    {
        if let Some(residual) = decode_budget.checked_sub(decoded_object_stream.len()) {
            *decode_budget = residual;
        } else {
            *decode_budget = 0;
            return Err(CloneSetExportRefusal::DecodeWorkBudgetExhausted {
                max_decoded_bytes: MAX_CLONE_SET_DECODE_WORK_BYTES,
            });
        }
    }

    let locator_matches = match &resolved {
        ResolvedObjectData::Uncompressed { resolved } => matches!(
            member.locator,
            CloneMemberLocator::Uncompressed { object_byte_offset }
                if object_byte_offset == resolved.object_byte_offset
        ),
        ResolvedObjectData::Compressed {
            object_stream_number,
            index_within_object_stream,
            ..
        } => matches!(
            member.locator,
            CloneMemberLocator::Compressed {
                object_stream_number: retained_stream,
                index_within_object_stream: retained_index,
            } if retained_stream == *object_stream_number
                && retained_index == *index_within_object_stream
        ),
    };
    if !locator_matches {
        return Err(CloneSetExportRefusal::LocatorMismatch {
            member: member.source,
        });
    }
    Ok(resolved)
}

/// Materialize one uncompressed member: census corroboration through the
/// range-bearing sibling of the plan's census scanner, body-shape/extent
/// classification (stream framing preserved verbatim), splice, and verify.
fn materialize_uncompressed(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    member: &CloneSetMember,
    object_byte_offset: usize,
    fresh_by_source: &BTreeMap<IndirectRef, IndirectRef>,
    budget: &mut MaterializedBodyBudget,
) -> Result<Vec<u8>, CloneSetExportRefusal> {
    let reference_scan =
        inspect_object_body_reference_ranges(input, object_byte_offset).map_err(|_| {
            CloneSetExportRefusal::CensusMismatch {
                member: member.source,
            }
        })?;
    corroborate_census(&reference_scan, member)?;

    let shape =
        classify_uncompressed_body(input, lookup, member, object_byte_offset, fresh_by_source)?;
    admit_uncompressed_single_value(input, shape.body_span.end, member)?;
    splice_and_verify(
        input,
        shape.body_span,
        shape.dictionary_window_len,
        &reference_scan,
        member,
        fresh_by_source,
        budget,
    )
}

/// One uncompressed member's materialization extent.
struct UncompressedBodyShape {
    /// Byte span of the body inside `input` (for a stream member: dictionary
    /// open through the end of the `endstream` keyword).
    body_span: Range<usize>,
    /// For stream members, the pre-splice dictionary length: the post-rewrite
    /// verification re-scan is bounded to the rewritten dictionary window so
    /// stream data is never scanned.
    dictionary_window_len: Option<usize>,
}

/// Classify one uncompressed member body and locate its exact extent.
fn classify_uncompressed_body(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    member: &CloneSetMember,
    object_byte_offset: usize,
    fresh_by_source: &BTreeMap<IndirectRef, IndirectRef>,
) -> Result<UncompressedBodyShape, CloneSetExportRefusal> {
    let unsupported = || CloneSetExportRefusal::UnsupportedBodyShape {
        member: member.source,
    };
    let header =
        inspect_indirect_object_header(input, object_byte_offset).map_err(|_| unsupported())?;
    let body_token = inspect_indirect_object_body_token(input, header.after_obj_keyword_offset)
        .map_err(|_| unsupported())?;
    let first_token = body_token.first_token_byte_offset;

    match body_token.token_kind {
        IndirectObjectBodyLeadingTokenKind::DictionaryOpen => {
            match inspect_content_stream_start(input, object_byte_offset) {
                Ok(stream_start) => classify_stream_body(
                    input,
                    lookup,
                    member,
                    object_byte_offset,
                    &stream_start,
                    fresh_by_source,
                ),
                Err(error) => match error.reason {
                    // No stream keyword follows the dictionary: an ordinary
                    // dictionary-bodied member.
                    ContentStreamStartInspectionRejection::MissingStreamKeyword
                    | ContentStreamStartInspectionRejection::OffsetOutOfBounds => {
                        let extent = inspect_dictionary_extent(input, first_token)
                            .map_err(|_| unsupported())?;
                        Ok(UncompressedBodyShape {
                            body_span: extent.open_byte_offset..extent.after_close_byte_offset,
                            dictionary_window_len: None,
                        })
                    }
                    // The stream keyword is present but its EOL violates
                    // §7.3.8.1 (a lone CR included): refuse, never normalize.
                    ContentStreamStartInspectionRejection::InvalidStreamEol { .. } => {
                        Err(CloneSetExportRefusal::StreamFraming {
                            member: member.source,
                        })
                    }
                    ContentStreamStartInspectionRejection::ObjectDictionary { .. }
                    | ContentStreamStartInspectionRejection::NonDictionaryBody { .. } => {
                        Err(unsupported())
                    }
                },
            }
        }
        IndirectObjectBodyLeadingTokenKind::ArrayOpen => {
            let extent = inspect_array_extent(input, first_token).map_err(|_| unsupported())?;
            Ok(UncompressedBodyShape {
                body_span: extent.open_byte_offset..extent.after_close_byte_offset,
                dictionary_window_len: None,
            })
        }
        IndirectObjectBodyLeadingTokenKind::NumberLike => {
            // A reference-shaped scalar body is the whole `N G R` span (and
            // may itself be rewritten); a plain scalar must satisfy the PDF
            // number grammar exactly — a malformed digit-led token such as
            // `12foo` is not one supported value (the same lexeme check the
            // compressed-member admission applies).
            let end = if let Ok(reference) = parse_indirect_reference(input, first_token) {
                reference.reference_range.end
            } else {
                let token_end = body_token_end(input, first_token, input.len());
                if !is_pdf_number_lexeme(&input[first_token..token_end]) {
                    return Err(CloneSetExportRefusal::UncompressedMemberNotSingleValue {
                        member: member.source,
                    });
                }
                token_end
            };
            Ok(UncompressedBodyShape {
                body_span: first_token..end,
                dictionary_window_len: None,
            })
        }
        IndirectObjectBodyLeadingTokenKind::Name
        | IndirectObjectBodyLeadingTokenKind::Boolean
        | IndirectObjectBodyLeadingTokenKind::Null => Ok(UncompressedBodyShape {
            body_span: first_token..body_token_end(input, first_token, input.len()),
            dictionary_window_len: None,
        }),
        // String-led bodies are not materialized by this slice: refuse
        // fail-closed rather than duplicating opaque-string extent parsing.
        IndirectObjectBodyLeadingTokenKind::HexStringOpen
        | IndirectObjectBodyLeadingTokenKind::LiteralString => Err(unsupported()),
    }
}

/// Classify one stream member: apply the `/Length` verdicts, corroborate the
/// declared extent against the actual framing, and report the exact
/// dictionary-through-`endstream` body span.
fn classify_stream_body(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    member: &CloneSetMember,
    object_byte_offset: usize,
    stream_start: &ContentStreamStartInspection,
    fresh_by_source: &BTreeMap<IndirectRef, IndirectRef>,
) -> Result<UncompressedBodyShape, CloneSetExportRefusal> {
    let length_entry = unique_length_entry(input, &stream_start.dictionary.entries, member)?;
    match length_entry.value_kind {
        DictionaryValueKind::NumberLike => {}
        DictionaryValueKind::IndirectReferenceLike => {
            classify_indirect_length(input, lookup, member, length_entry, fresh_by_source)?;
        }
        _ => {
            return Err(CloneSetExportRefusal::LengthUnsupportedShape {
                member: member.source,
            });
        }
    }

    // Corroborate the declared `/Length` against the actual framing: data
    // end, EOL (LF or CRLF; a lone CR was already refused), `endstream`.
    // Never repaired, never recomputed.
    let extent =
        inspect_content_stream_data_extent_with_lookup(input, Some(lookup), object_byte_offset)
            .map_err(|_| CloneSetExportRefusal::LengthExtentMismatch {
                member: member.source,
            })?;
    let data_end = extent.stream_data_end_byte_offset();
    let eol_len = if input.get(data_end) == Some(&b'\n') {
        1
    } else {
        2
    };
    let body_end = data_end + eol_len + ENDSTREAM_KEYWORD_LEN;
    let dictionary_open = stream_start.dictionary.dictionary_open_byte_offset;
    Ok(UncompressedBodyShape {
        body_span: dictionary_open..body_end,
        dictionary_window_len: Some(
            stream_start.dictionary.after_dictionary_close_byte_offset - dictionary_open,
        ),
    })
}

/// Locate exactly one semantic top-level `/Length` entry. Escaped spellings
/// refuse explicitly because the downstream extent helper recognizes only the
/// canonical raw key; semantic collisions refuse before that helper is called.
fn unique_length_entry<'a>(
    input: &[u8],
    entries: &'a [DictionaryEntrySpan],
    member: &CloneSetMember,
) -> Result<&'a DictionaryEntrySpan, CloneSetExportRefusal> {
    let mut found: Option<(&DictionaryEntrySpan, bool)> = None;
    for entry in entries {
        let Some(raw_key) = input.get(entry.key_range.start..entry.key_range.end) else {
            continue;
        };
        let Some(decoded_key) = raw_key.strip_prefix(b"/").and_then(decode_pdf_name) else {
            continue;
        };
        if decoded_key.as_ref() != &LENGTH_KEY[1..] {
            continue;
        }
        if found.is_some() {
            return Err(CloneSetExportRefusal::LengthDuplicate {
                member: member.source,
            });
        }
        found = Some((entry, raw_key == LENGTH_KEY));
    }
    match found {
        Some((entry, true)) => Ok(entry),
        Some((_, false)) => Err(CloneSetExportRefusal::LengthNonCanonical {
            member: member.source,
        }),
        None => Err(CloneSetExportRefusal::LengthMissing {
            member: member.source,
        }),
    }
}

/// Apply the indirect `/Length` verdicts: self-referential, out-of-set
/// resolvable, and null-equivalent targets all refuse; only an in-set target
/// (an ordinary member whose integer body is copied verbatim) is admitted.
fn classify_indirect_length(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    member: &CloneSetMember,
    length_entry: &DictionaryEntrySpan,
    fresh_by_source: &BTreeMap<IndirectRef, IndirectRef>,
) -> Result<(), CloneSetExportRefusal> {
    let reference = parse_indirect_reference(input, length_entry.value_range.start)
        .map_err(|_| CloneSetExportRefusal::LengthUnsupportedShape {
            member: member.source,
        })?
        .reference;
    if reference == member.source {
        return Err(CloneSetExportRefusal::LengthSelfReferential {
            member: member.source,
        });
    }
    if fresh_by_source.contains_key(&reference) {
        // In-set: an ordinary member — the reference is rewritten by the
        // splice pass and the integer object is copied verbatim as its own
        // clone member.
        return Ok(());
    }
    let object_number = usize::try_from(reference.object_number).unwrap_or(usize::MAX);
    match locate_xref_object(lookup, object_number) {
        ObjectLookupLocation::ClassicFree { .. }
        | ObjectLookupLocation::XrefStreamFree { .. }
        | ObjectLookupLocation::ClassicNotFound { .. }
        | ObjectLookupLocation::XrefStreamNotFound { .. } => {
            Err(CloneSetExportRefusal::LengthNullEquivalent {
                member: member.source,
            })
        }
        // A resolvable (or even ambiguous) out-of-set `/Length` target is a
        // plan-consistency anomaly: the closure walk would have admitted a
        // resolvable target as a member.
        _ => Err(CloneSetExportRefusal::LengthOutOfSet {
            member: member.source,
        }),
    }
}

/// Corroborate one member's re-scan census against the retained
/// duplicate-preserving outgoing list: count, order, values, generations —
/// zero skips, zero truncation.
fn corroborate_census(
    reference_scan: &ObjectBodyReferenceRangesInspection,
    member: &CloneSetMember,
) -> Result<(), CloneSetExportRefusal> {
    let aligned = reference_scan.token_ranges.len() == reference_scan.references.len();
    let complete =
        reference_scan.truncation.is_none() && reference_scan.skipped_references.is_empty();
    let census_matches = reference_scan.references == member.outgoing;
    if complete && aligned && census_matches {
        Ok(())
    } else {
        Err(CloneSetExportRefusal::CensusMismatch {
            member: member.source,
        })
    }
}

/// Materialize one compressed member as a plain generation-zero body from the
/// decoded container span, after the §7.5.7 admission checks.
fn materialize_compressed(
    decoded: &[u8],
    span: Range<usize>,
    member: &CloneSetMember,
    fresh_by_source: &BTreeMap<IndirectRef, IndirectRef>,
    budget: &mut MaterializedBodyBudget,
) -> Result<Vec<u8>, CloneSetExportRefusal> {
    let reference_scan = scan_indirect_reference_ranges_in_span(decoded, span.clone());
    corroborate_census(&reference_scan, member)?;
    admit_object_stream_member(decoded, &span, member)?;
    splice_and_verify(
        decoded,
        span,
        None,
        &reference_scan,
        member,
        fresh_by_source,
        budget,
    )
}

/// Private admission for one decoded object-stream member span:
/// exactly ONE value must consume the span (trailing whitespace/comments
/// aside), and that value must not be solely an indirect reference (§7.5.7).
/// Member extraction proves neither.
fn admit_object_stream_member(
    decoded: &[u8],
    span: &Range<usize>,
    member: &CloneSetMember,
) -> Result<(), CloneSetExportRefusal> {
    let unsupported = || CloneSetExportRefusal::UnsupportedBodyShape {
        member: member.source,
    };
    let not_single_value = || CloneSetExportRefusal::ObjectStreamMemberNotSingleValue {
        member: member.source,
    };
    // Every check runs over a slice truncated at the span end, so no
    // classifier or extent scan can read sibling-member bytes.
    let end = span.end.min(decoded.len());
    let bounded = &decoded[..end];

    let body_token =
        inspect_indirect_object_body_token(bounded, span.start).map_err(|_| not_single_value())?;
    let first_token = body_token.first_token_byte_offset;

    let value_end = match body_token.token_kind {
        IndirectObjectBodyLeadingTokenKind::DictionaryOpen => {
            inspect_dictionary_extent(bounded, first_token)
                .map_err(|_| unsupported())?
                .after_close_byte_offset
        }
        IndirectObjectBodyLeadingTokenKind::ArrayOpen => {
            inspect_array_extent(bounded, first_token)
                .map_err(|_| unsupported())?
                .after_close_byte_offset
        }
        IndirectObjectBodyLeadingTokenKind::NumberLike => {
            if let Ok(reference) = parse_indirect_reference(bounded, first_token) {
                return if is_trivia_only(bounded, reference.after_keyword_offset, end) {
                    Err(CloneSetExportRefusal::ObjectStreamMemberSolelyReference {
                        member: member.source,
                    })
                } else {
                    Err(not_single_value())
                };
            }
            let token_end = body_token_end(bounded, first_token, end);
            if !is_pdf_number_lexeme(&bounded[first_token..token_end]) {
                return Err(not_single_value());
            }
            token_end
        }
        IndirectObjectBodyLeadingTokenKind::Name
        | IndirectObjectBodyLeadingTokenKind::Boolean
        | IndirectObjectBodyLeadingTokenKind::Null => body_token_end(bounded, first_token, end),
        IndirectObjectBodyLeadingTokenKind::HexStringOpen => {
            hex_string_end(bounded, first_token, end).ok_or_else(not_single_value)?
        }
        IndirectObjectBodyLeadingTokenKind::LiteralString => {
            literal_string_end(bounded, first_token, end).ok_or_else(not_single_value)?
        }
    };

    if is_trivia_only(bounded, value_end, end) {
        Ok(())
    } else {
        Err(not_single_value())
    }
}

/// Splice ONLY the in-set numeric tokens of one body span to their fresh
/// identities and verify the rewritten body: fail-closed budget charging
/// (extent pre-charged before the copy; exact final length settled with
/// checked arithmetic before allocation), reverse-order splices, and a
/// post-rewrite re-scan that must equal the translated census.
fn splice_and_verify(
    source: &[u8],
    body_span: Range<usize>,
    dictionary_window_len: Option<usize>,
    reference_scan: &ObjectBodyReferenceRangesInspection,
    member: &CloneSetMember,
    fresh_by_source: &BTreeMap<IndirectRef, IndirectRef>,
    budget: &mut MaterializedBodyBudget,
) -> Result<Vec<u8>, CloneSetExportRefusal> {
    let extent_len = body_span.len();
    budget.precharge(extent_len)?;

    // Build body-relative splice ops for in-set references only; every other
    // byte (null-equivalent tokens included) is preserved exactly.
    let out_of_bounds = || CloneSetExportRefusal::SpliceOutOfBounds {
        member: member.source,
    };
    let splice_window = dictionary_window_len.unwrap_or(extent_len);
    let mut ops: Vec<(Range<usize>, Vec<u8>)> = Vec::new();
    let mut removed = 0usize;
    let mut added = 0usize;
    for (reference, tokens) in reference_scan
        .references
        .iter()
        .zip(&reference_scan.token_ranges)
    {
        let Some(fresh) = fresh_by_source.get(reference) else {
            continue;
        };
        for (range, replacement) in [
            (
                tokens.object_number_range.clone(),
                fresh.object_number.to_string().into_bytes(),
            ),
            (tokens.generation_range.clone(), b"0".to_vec()),
        ] {
            if range.start < body_span.start || range.end > body_span.end {
                return Err(out_of_bounds());
            }
            let relative = range.start - body_span.start..range.end - body_span.start;
            if relative.end > splice_window {
                return Err(out_of_bounds());
            }
            removed += relative.len();
            added += replacement.len();
            ops.push((relative, replacement));
        }
    }

    // Exact final length, checked BEFORE the body allocation.
    let final_len = extent_len
        .checked_sub(removed)
        .and_then(|kept| kept.checked_add(added))
        .ok_or_else(out_of_bounds)?;
    budget.settle(extent_len, final_len)?;

    let mut body = Vec::with_capacity(final_len);
    body.extend_from_slice(&source[body_span]);
    // Reverse-order splice application (house pattern): earlier ranges stay
    // valid because later ones are consumed first.
    for (range, replacement) in ops.iter().rev() {
        body.splice(range.clone(), replacement.iter().copied());
    }

    // Post-rewrite verification: re-scan the rewritten body (for stream
    // members only the rewritten dictionary window — stream data is never
    // scanned) and require exact equality with the translated census.
    let verify_end = dictionary_window_len.map_or(body.len(), |window| {
        (window + added).saturating_sub(removed)
    });
    let rescan = scan_indirect_references_in_span(&body, 0..verify_end);
    let translated_matches = rescan
        .references
        .iter()
        .copied()
        .eq(member.outgoing.iter().map(|reference| {
            fresh_by_source
                .get(reference)
                .copied()
                .unwrap_or(*reference)
        }));
    if rescan.truncation.is_some() || !rescan.skipped_references.is_empty() || !translated_matches {
        return Err(CloneSetExportRefusal::PostRewriteCensusMismatch {
            member: member.source,
        });
    }

    Ok(body)
}

/// Request-level cumulative materialized-body budget with fail-closed
/// charging: extents are pre-charged before copy work, then settled to the
/// exact final length before each allocation.
struct MaterializedBodyBudget {
    charged: usize,
}

impl MaterializedBodyBudget {
    fn precharge(&mut self, extent_len: usize) -> Result<(), CloneSetExportRefusal> {
        self.charged = self
            .charged
            .checked_add(extent_len)
            .filter(|charged| *charged <= MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES)
            .ok_or(CloneSetExportRefusal::MaterializedBodyBudgetExceeded {
                max_body_bytes: MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES,
            })?;
        Ok(())
    }

    fn settle(&mut self, extent_len: usize, final_len: usize) -> Result<(), CloneSetExportRefusal> {
        self.charged = self
            .charged
            .checked_sub(extent_len)
            .and_then(|base| base.checked_add(final_len))
            .filter(|charged| *charged <= MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES)
            .ok_or(CloneSetExportRefusal::MaterializedBodyBudgetExceeded {
                max_body_bytes: MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES,
            })?;
        Ok(())
    }
}

/// End of one non-delimited body token (number, name after its leading `/`,
/// boolean, null), bounded to `end`.
fn body_token_end(buffer: &[u8], start: usize, end: usize) -> usize {
    let mut cursor = if buffer.get(start) == Some(&b'/') {
        start + 1
    } else {
        start
    };
    while cursor < end && !is_pdf_whitespace(buffer[cursor]) && !is_pdf_delimiter(buffer[cursor]) {
        cursor += 1;
    }
    cursor
}

/// Prove one uncompressed member's indirect object holds exactly one
/// supported value: after the classified value (or `endstream`) only PDF
/// whitespace/comments may follow until one exact delimiter-bounded `endobj`
/// token (§7.3.10 — one value per indirect object). Anything else — a second
/// value, `endobjX`, trailing non-trivia, or a missing `endobj` — refuses.
fn admit_uncompressed_single_value(
    input: &[u8],
    value_end: usize,
    member: &CloneSetMember,
) -> Result<(), CloneSetExportRefusal> {
    let refusal = || CloneSetExportRefusal::UncompressedMemberNotSingleValue {
        member: member.source,
    };
    let keyword_start = skip_trivia(input, value_end, input.len());
    let keyword_end = keyword_start
        .checked_add(ENDOBJ_KEYWORD.len())
        .filter(|end| *end <= input.len())
        .ok_or_else(refusal)?;
    if &input[keyword_start..keyword_end] != ENDOBJ_KEYWORD {
        return Err(refusal());
    }
    match input.get(keyword_end) {
        None => Ok(()),
        Some(&byte) if is_pdf_whitespace(byte) || is_pdf_delimiter(byte) => Ok(()),
        Some(_) => Err(refusal()),
    }
}

/// First non-trivia offset at or after `cursor`, bounded to `end` (PDF
/// whitespace and `%` comments through their EOL are trivia).
fn skip_trivia(buffer: &[u8], mut cursor: usize, end: usize) -> usize {
    while cursor < end {
        let byte = buffer[cursor];
        if is_pdf_whitespace(byte) {
            cursor += 1;
        } else if byte == b'%' {
            while cursor < end && !matches!(buffer[cursor], b'\r' | b'\n') {
                cursor += 1;
            }
        } else {
            break;
        }
    }
    cursor
}

/// True when `buffer[cursor..end]` holds only PDF whitespace and comments.
fn is_trivia_only(buffer: &[u8], cursor: usize, end: usize) -> bool {
    skip_trivia(buffer, cursor, end) == end
}

/// Exclusive end of one literal-string token, bounded to this member span.
/// Escaped bytes never affect parenthesis depth; unbalanced/unterminated input
/// fails closed instead of reading into the next object-stream member.
fn literal_string_end(buffer: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut cursor = open.checked_add(1)?;
    let mut depth = 1usize;
    while cursor < end {
        match buffer[cursor] {
            b'\\' => cursor = cursor.checked_add(2)?,
            b'(' => {
                depth = depth.checked_add(1)?;
                cursor += 1;
            }
            b')' => {
                depth -= 1;
                cursor += 1;
                if depth == 0 {
                    return Some(cursor);
                }
            }
            _ => cursor += 1,
        }
    }
    None
}

/// Exclusive end of one hexadecimal-string token, bounded to this member
/// span. Only hexadecimal digits and PDF whitespace are valid before `>`;
/// odd digit counts remain valid per PDF string syntax.
fn hex_string_end(buffer: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut cursor = open.checked_add(1)?;
    while cursor < end {
        let byte = buffer[cursor];
        cursor += 1;
        if byte == b'>' {
            return Some(cursor);
        }
        if !byte.is_ascii_hexdigit() && !is_pdf_whitespace(byte) {
            return None;
        }
    }
    None
}

/// True when a scalar token satisfies the PDF number grammar: optional sign,
/// decimal digits with at most one period, at least one digit, and no exponent.
fn is_pdf_number_lexeme(token: &[u8]) -> bool {
    let digits = match token.first() {
        Some(b'+' | b'-') => &token[1..],
        _ => token,
    };
    let mut saw_digit = false;
    let mut saw_period = false;
    for byte in digits {
        match byte {
            b'0'..=b'9' => saw_digit = true,
            b'.' if !saw_period => saw_period = true,
            _ => return false,
        }
    }
    saw_digit
}

/// True when `error` is exactly the compressed-member decode rejection caused
/// by the caller-supplied decode-byte bound, not a genuinely malformed object
/// stream — the same classification the closure walk and the fresh-floor
/// proof apply to their own decode budgets.
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

const fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

const fn is_pdf_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}
