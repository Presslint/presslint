//! Request-scoped reached-Form closure clone-set PLAN, no bytes.
//!
//! For every leaf page whose `/Resources /XObject` binding witness qualifies —
//! decoded `/Subtype /Form`, an explicitly leaf-direct binding path, the exact
//! `Unproven { TargetConsumersNotExclusive }` verdict, and a globally complete
//! binding walk — this private plan computes the bounded reached-Form closure
//! (nested forms, fonts, colour spaces, profiles, `ExtGState`, shadings,
//! patterns, streams, resource dictionaries), assigns reserved fresh
//! identities to every non-refused set through ONE
//! [`crate::reserve_fresh_object_references`] call, and retains what the
//! future clone-body export and one-page retarget slices need.
//!
//! The plan itself is OBSERVE-ONLY: it constructs no fresh bodies,
//! copies/rewrites no bytes, retargets no page, mutates no Form, and never
//! authorizes any admission. Its execution half is the STAGED/VALIDATE-ONLY
//! export in [`export`]: every planned set is re-corroborated, materialized,
//! and fully validated as a [`crate::FreshObjectBytes`] batch, but the batch
//! is never handed to the writer by production callers — emitted product
//! bytes stay byte-identical to the plan-only behaviour, and only the
//! additive omit-when-empty [`FormCloneSetPlanCounts`] projection on
//! [`crate::ConvertedPage`] (plan counters plus staged-export counters) is
//! published.
//!
//! # Frontier policy
//!
//! Fail-closed with NO drop lane and NO null-out rewrite: exact known
//! catalog/page-tree/page identities, structure / `AcroForm` / optional-content
//! graph escapes, ancillary companion keys
//! (`/OC`, `/Ref`, external-file `/F`, `/OPI`, `/PieceInfo`,
//! `/StructParent(s)` — the latter are Table-95 INTEGER keys invisible to a
//! reference walker, so a decoded-name duplicate-sensitive dictionary
//! preflight catches them), malformed or ambiguous structural keys,
//! incomplete body scans, and every budget exhaustion refuse the WHOLE clone
//! set with a typed refusal class. Free/not-found targets are terminal
//! null-equivalent verdicts (ISO 32000-1 §7.3.9–7.3.10) that allocate
//! nothing and preserve the original token; an explicit `null` is an ordinary
//! value; an indirect `/Length` is an ordinary closure member.
//!
//! # Reservation
//!
//! Exactly ONE reservation per request, over the total member count, after
//! all non-refused sets are known: sets are ordered by (page ordinal, root
//! source reference) and members by source reference, so the partition is
//! deterministic. The reservation is all-or-nothing; a reservation failure
//! (including the whole-document floor-proof reference budget, which may
//! refuse a tiny closure) and the ISO 32000-1 Annex C Table C.1
//! indirect-object-limit check are typed reservation refusals folded into the
//! per-page counts — reported honestly as reservation budget, never as a
//! closure fact and never as a public [`crate::WriteError`]. The same source
//! object on different pages receives distinct fresh identities, and no
//! source-to-fresh map outlives its one clone set (pikepdf#271 class).

// Crate-private plan family inside a private module: `pub(crate)` here is the
// deliberate crate-wide seam the pipeline and tests consume.
#![allow(clippy::redundant_pub_crate)]

use std::collections::BTreeMap;

use presslint_pdf::{
    BindingContainerLocality, BindingResourcesSource, DictionaryEntryByteRange, IndirectRef,
    ObjectConsumerIndexInspection, ObjectConsumerReferrer, ObjectLookup,
    PageXObjectBindingUnprovenReason, PageXObjectBindingVerdict, PageXObjectBindingWitness,
    PageXObjectBindingsInspection, PdfName, XObjectBindingSubtype,
    inspect_document_page_xobject_bindings_with_lookup,
};
use serde::{Deserialize, Serialize};

use crate::writer::{FreshObjectBytes, WriteError};

use self::export::{CloneSetExportRefusal, StagedExportBatch, build_staged_export};
use self::walk::{CloneSetBudgetUsage, walk_reached_form_closure};

pub(crate) mod export;
pub(crate) mod walk;

/// Largest indirect object number a conforming reader must support
/// (ISO 32000-1 Annex C, Table C.1). A reservation whose highest fresh
/// identity exceeds this limit is refused before the plan is declared ready.
const ANNEX_C_MAX_INDIRECT_OBJECT_NUMBER: u32 = 8_388_607;

/// Per-page clone-set plan counts (public projection, additive counters).
///
/// Mirrors the `FormXObjectRefusalCounts` house pattern: named `usize`
/// fields, `is_empty`, and `#[serde(default, skip_serializing_if = "..")]` on
/// every field plus the owning `ConvertedPage` field, so existing JSON
/// without clone-set plans stays byte-identical. The counters are
/// observe-only telemetry; they never authorize any clone, export, or
/// retarget.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormCloneSetPlanCounts {
    /// Qualifying clone-set candidates on this page, one per bound
    /// `(page, target)` pair (several names binding one target share a set).
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub candidate_sets: usize,
    /// Candidate sets whose bounded closure walk completed and whose fresh
    /// identities were reserved.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub planned_sets: usize,
    /// Candidate sets refused fail-closed (frontier escape, budget
    /// exhaustion, or the all-or-nothing reservation refusal).
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub refused_sets: usize,
    /// Total closure member objects across this page's planned sets.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub planned_objects: usize,
    /// Terminal null-equivalent reference verdicts across this page's planned
    /// sets (free or not-found targets; nothing is allocated for them).
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub null_equivalents: usize,
    /// Planned sets whose clone bodies were materialized and fully validated
    /// by the staged export. Staged bodies are NOT emitted: product bytes
    /// stay byte-identical to the plan-only behaviour.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub staged_sets: usize,
    /// Total materialized member bodies across this page's staged sets.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub staged_objects: usize,
    /// Total materialized body bytes across this page's staged sets.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub staged_body_bytes: usize,
    /// Planned sets suppressed because the all-or-nothing staged export
    /// refused (any export failure discards the whole request batch).
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub export_refused_sets: usize,
}

/// Serde helper: omit an additive scalar count while it is zero, so existing
/// zero-count JSON shapes stay byte-compatible.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn count_is_zero(count: &usize) -> bool {
    *count == 0
}

impl FormCloneSetPlanCounts {
    /// The all-zero counts (const constructor, used where `Default::default`
    /// is not `const`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            candidate_sets: 0,
            planned_sets: 0,
            refused_sets: 0,
            planned_objects: 0,
            null_equivalents: 0,
            staged_sets: 0,
            staged_objects: 0,
            staged_body_bytes: 0,
            export_refused_sets: 0,
        }
    }

    /// Whether every count is zero (the omit-when-empty predicate).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.candidate_sets == 0
            && self.planned_sets == 0
            && self.refused_sets == 0
            && self.planned_objects == 0
            && self.null_equivalents == 0
            && self.staged_sets == 0
            && self.staged_objects == 0
            && self.staged_body_bytes == 0
            && self.export_refused_sets == 0
    }
}

/// Exact leaf page identity a clone set is anchored to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CloneSetPageIdentity {
    /// Zero-based document-order page ordinal.
    pub(crate) ordinal: usize,
    /// Leaf `/Page` indirect reference.
    pub(crate) reference: IndirectRef,
    /// Resolved leaf page object byte offset.
    pub(crate) object_byte_offset: usize,
}

/// One page `/XObject` entry the future retarget slice must rewrite to the
/// root's fresh identity: the exact key/value byte ranges inside the
/// leaf-direct `/XObject` subdictionary plus the expected old target.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct CloneSetRetargetSite {
    /// Raw entry name bytes without the leading slash (reporting spelling).
    pub(crate) name: PdfName,
    /// Byte range of the entry key inside the `/XObject` subdictionary.
    pub(crate) key_range: DictionaryEntryByteRange,
    /// Byte range of the entry value (the indirect-reference tokens).
    pub(crate) value_range: DictionaryEntryByteRange,
    /// Exact target the value must still name when the retarget executes.
    pub(crate) expected_target: IndirectRef,
}

/// Where one closure member's source bytes live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CloneMemberLocator {
    /// An ordinary uncompressed indirect object at a source byte offset.
    Uncompressed {
        /// Corroborated object byte offset.
        object_byte_offset: usize,
    },
    /// A type-2 compressed object-stream member (container coordinates only;
    /// no source byte offset exists and none is fabricated).
    Compressed {
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Index of the member inside the object stream.
        index_within_object_stream: usize,
    },
}

/// One resolved closure member, retained by locator and outgoing references
/// only — never by body bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CloneSetMember {
    /// Exact source identity (generation included).
    pub(crate) source: IndirectRef,
    /// Uncompressed offset or compressed container/member locator.
    pub(crate) locator: CloneMemberLocator,
    /// Duplicate-preserving outgoing `N G R` references in body source order.
    pub(crate) outgoing: Vec<IndirectRef>,
}

/// Terminal null-equivalent verdict for one reached identity: a free or
/// absent xref entry is equivalent to `null` (ISO 32000-1 §7.3.9–7.3.10), so
/// the original reference token is preserved and nothing is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NullEquivalentVerdict {
    /// The reached reference whose identity is free or absent.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) reference: IndirectRef,
    /// True for an explicit free xref entry, false for an absent identity.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) free_entry: bool,
}

/// The `/Type` value that escaped the clonable object graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GraphEscapeClass {
    /// `/Type /Catalog`.
    Catalog,
    /// `/Type /Pages`.
    Pages,
    /// `/Type /Page`.
    Page,
    /// `/Type /StructTreeRoot`.
    StructTreeRoot,
    /// `/Type /StructElem`.
    StructElem,
    /// `/Type /OCG` (optional-content group).
    OptionalContentGroup,
    /// `/Type /OCMD` (optional-content membership dictionary).
    OptionalContentMembership,
    /// An identity reached from the catalog's `/AcroForm` graph.
    AcroFormGraph,
    /// An identity reached from the catalog's `/StructTreeRoot` graph.
    StructureGraph,
    /// An identity reached from the catalog's `/OCProperties` graph.
    OptionalContentPropertiesGraph,
}

/// Exact structural identities that must never become clone members even if
/// their dictionaries omit or spoof `/Type`.
///
/// Catalog, page-tree-root, and known leaf identities come from the already-
/// open document spine/binding walk. `AcroForm`, structure, and optional-
/// content graph membership comes from the SAME borrowed complete consumer
/// index used for seed qualification; no second index or graph walk is built.
#[derive(Default)]
pub(crate) struct StructuralFrontier {
    identities: BTreeMap<IndirectRef, GraphEscapeClass>,
}

impl StructuralFrontier {
    pub(crate) fn new(
        catalog: IndirectRef,
        page_tree_root: IndirectRef,
        pages: impl IntoIterator<Item = IndirectRef>,
        consumers: &ObjectConsumerIndexInspection,
    ) -> Self {
        let mut identities = BTreeMap::new();
        identities.insert(catalog, GraphEscapeClass::Catalog);
        identities.insert(page_tree_root, GraphEscapeClass::Pages);
        for page in pages {
            identities.entry(page).or_insert(GraphEscapeClass::Page);
        }

        for entry in &consumers.entries {
            for referrer in &entry.referrers {
                let ObjectConsumerReferrer::RootKey { key } = referrer else {
                    continue;
                };
                let Some(decoded) = crate::page_xobject_policy::decode_pdf_name(&key.0) else {
                    continue;
                };
                let escape = match decoded.as_ref() {
                    b"AcroForm" => GraphEscapeClass::AcroFormGraph,
                    b"StructTreeRoot" => GraphEscapeClass::StructureGraph,
                    b"OCProperties" => GraphEscapeClass::OptionalContentPropertiesGraph,
                    _ => continue,
                };
                identities.entry(entry.target).or_insert(escape);
            }
        }
        Self { identities }
    }

    pub(crate) fn classify(&self, reference: IndirectRef) -> Option<GraphEscapeClass> {
        self.identities.get(&reference).copied()
    }
}

/// The ancillary companion key whose presence refused a clone set. Cloning a
/// dictionary carrying one of these would silently duplicate an object wired
/// into a document-level companion structure this slice plans no repair for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AncillaryKeyClass {
    /// `/OC` optional-content membership on the member dictionary.
    OptionalContent,
    /// `/Ref` reference `XObject` (imported-page proxy).
    ReferenceXObject,
    /// `/F` external-file stream data (or any other `/F` use, fail-closed).
    ExternalFile,
    /// `/OPI` proxy/version dictionary.
    Opi,
    /// `/PieceInfo` page-piece private application data.
    PieceInfo,
    /// `/StructParent` integer key into the structure parent tree.
    StructParent,
    /// `/StructParents` integer key into the structure parent tree.
    StructParents,
}

/// Typed reason one whole clone set was refused, fail-closed. Exactly the
/// FIRST refusal encountered in deterministic walk order is retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneSetRefusal {
    /// A member's decoded `/Type` names a page-tree / structure /
    /// optional-content graph node (including type-spoofed objects reached
    /// through ordinary references).
    GraphEscape {
        /// Member whose dictionary carried the escaping type.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
        /// The escaping decoded type name.
        escape: GraphEscapeClass,
    },
    /// A member dictionary carries an ancillary companion key (decoded-name
    /// checked, so `/O#43`-style escapes cannot bypass).
    AncillaryKey {
        /// Member whose dictionary carried the key.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
        /// The refusing decoded key.
        key: AncillaryKeyClass,
    },
    /// A member's top-level dictionary keys were malformed or ambiguous: an
    /// undecodable name, a duplicate decoded key, a non-name `/Type` value,
    /// or a dictionary whose entries could not be scanned.
    MalformedStructuralKeys {
        /// Member whose dictionary refused.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// The xref lookup answered with an ambiguous, reserved, or out-of-range
    /// entry for a reached identity.
    AmbiguousLookup {
        /// The reached reference whose lookup was ambiguous.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// [`presslint_pdf::resolve_object`] refused a reached in-use identity
    /// (generation mismatch, header mismatch, malformed member extraction).
    ResolutionFailed {
        /// The reference that failed to resolve.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A body reference scan could not prove completeness: a per-body
    /// truncation (the retained 65,536 scanner cap) or a skipped
    /// out-of-range reference shape.
    BodyScanIncomplete {
        /// The member whose body scan was incomplete.
        #[cfg_attr(not(test), allow(dead_code))]
        member: IndirectRef,
    },
    /// A reached identity would sit past the reference-depth budget.
    DepthBudgetExhausted {
        /// Configured depth budget.
        #[cfg_attr(not(test), allow(dead_code))]
        max_depth: usize,
    },
    /// The unique-member budget was spent.
    MemberBudgetExhausted {
        /// Configured unique-member budget.
        #[cfg_attr(not(test), allow(dead_code))]
        max_members: usize,
    },
    /// The cumulative reference-occurrence budget was spent.
    ReferenceBudgetExhausted {
        /// Configured cumulative occurrence budget.
        #[cfg_attr(not(test), allow(dead_code))]
        max_occurrences: usize,
    },
    /// The cumulative object-stream decode-work budget was spent (the
    /// residual budget is exhausted before refusing; nothing is retryable).
    DecodeWorkBudgetExhausted {
        /// Configured cumulative decode-work budget in decoded bytes.
        #[cfg_attr(not(test), allow(dead_code))]
        max_decoded_bytes: usize,
    },
    /// The single all-or-nothing request reservation refused: the
    /// whole-document floor proof failed (its own budgets included) or the
    /// Annex C indirect-object limit was exceeded. Reported as reservation
    /// budget, never as a closure fact and never as a public `WriteError`.
    ReservationRefused {
        /// Delegated reservation failure, retained for telemetry only.
        #[cfg_attr(not(test), allow(dead_code))]
        reason: ReservationRefusal,
    },
}

/// Why the single request reservation refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReservationRefusal {
    /// [`crate::reserve_fresh_object_references`] returned an error; the
    /// boxed failure is retained crate-privately for telemetry only.
    FloorProof {
        /// Delegated writer failure.
        #[cfg_attr(not(test), allow(dead_code))]
        error: Box<WriteError>,
    },
    /// The highest reserved identity exceeds the Annex C Table C.1
    /// indirect-object limit.
    AnnexCObjectLimitExceeded {
        /// Highest reserved fresh object number.
        #[cfg_attr(not(test), allow(dead_code))]
        highest_reserved: u32,
        /// The Annex C limit the reservation must fit.
        #[cfg_attr(not(test), allow(dead_code))]
        limit: u32,
    },
    /// The reservation returned a different identity count than requested
    /// (defensive fail-closed guard on the writer contract; never expected).
    ContractMismatch {
        /// Identity count the plan requested.
        #[cfg_attr(not(test), allow(dead_code))]
        requested: usize,
        /// Identity count the reservation returned.
        #[cfg_attr(not(test), allow(dead_code))]
        reserved: usize,
    },
}

/// Outcome of one clone set's closure walk plus reservation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CloneSetOutcome {
    /// The bounded closure completed and fresh identities were reserved.
    Planned {
        /// Ordered members (by source reference, deterministic).
        members: Vec<CloneSetMember>,
        /// Terminal null-equivalent verdicts in discovery order.
        null_equivalents: Vec<NullEquivalentVerdict>,
        /// Source-to-fresh identity pairs, aligned with `members`. This map
        /// never outlives its one clone set and is never shared across
        /// pages or separate clone operations.
        source_to_fresh: Vec<(IndirectRef, IndirectRef)>,
    },
    /// The whole set was refused with its exact first refusal.
    Refused {
        /// First refusal in deterministic walk/reservation order.
        refusal: CloneSetRefusal,
    },
}

/// One `(page, target)` clone set: the request-scoped unit of planning,
/// export, and retargeting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FormCloneSet {
    /// Anchoring leaf page identity.
    pub(crate) page: CloneSetPageIdentity,
    /// Root Form source reference (the witnessed bound target).
    pub(crate) root: IndirectRef,
    /// Corroborated root object byte offset from the binding witness.
    pub(crate) root_object_byte_offset: usize,
    /// Every qualifying witnessed page `/XObject` entry binding this root, in
    /// witness order.
    pub(crate) retarget_sites: Vec<CloneSetRetargetSite>,
    /// Budget usage of the closure walk (residuals exhausted on budget
    /// refusals, so a budget refusal always reads as fully spent).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) budget: CloneSetBudgetUsage,
    /// Planned members + reservation, or the exact first refusal.
    pub(crate) outcome: CloneSetOutcome,
}

/// Identity-poisoned per-page counts slot (house `PageReportIndex` shape).
struct PageCountsSlot {
    object_byte_offset: usize,
    ordinal: usize,
    counts: FormCloneSetPlanCounts,
}

/// The request-scoped clone-set plan. Crate-private except for the
/// [`FormCloneSetPlanCounts`] projection consumed per page.
pub(crate) struct FormCloneSetPlan {
    /// Total source length the plan was computed over, re-asserted by the
    /// staged export before any member re-resolution.
    pub(crate) input_byte_len: usize,
    /// Clone sets in deterministic plan order: (page ordinal, root source
    /// reference).
    pub(crate) sets: Vec<FormCloneSet>,
    /// Witnesses counted but never walked: `ProvenPageLocal` in-place
    /// candidates (no clone is needed).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) in_place_candidates: usize,
    /// Witnesses counted but never walked: wrong subtype, inherited/indirect
    /// binding paths, incomplete consumer index, or a non-qualifying verdict.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) disqualified_witnesses: usize,
    /// True when the binding walk itself was globally complete; false leaves
    /// zero candidates fail-closed.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) globally_complete: bool,
    /// Per-page counts keyed by exact leaf reference, duplicate-poisoned.
    page_slots: BTreeMap<IndirectRef, Option<PageCountsSlot>>,
}

impl FormCloneSetPlan {
    /// Build the request-scoped plan: one binding inspection borrowing the
    /// caller's consumer index, one bounded closure walk per qualifying
    /// selected `(page, target)` seed, and exactly one reservation over the
    /// total member count of all non-refused sets (including a zero-count
    /// call for an empty plan).
    ///
    /// A failed binding inspection or an incomplete binding walk yields an
    /// empty plan (zero candidates) — fail-closed observation, never an
    /// error, because the plan is observe-only.
    pub(crate) fn build(
        input: &[u8],
        lookup: ObjectLookup<'_>,
        catalog_reference: IndirectRef,
        page_tree_root_reference: IndirectRef,
        root_node_object_offset: usize,
        selected_page_ordinals: &[usize],
        consumers: &ObjectConsumerIndexInspection,
    ) -> Self {
        let Ok(bindings) = inspect_document_page_xobject_bindings_with_lookup(
            input,
            lookup,
            root_node_object_offset,
            consumers,
        ) else {
            let mut plan = Self::empty(input.len(), false);
            // The request contract still makes exactly one zero-count
            // reservation call; the public reservation seam short-circuits
            // without a floor scan for count zero.
            plan.reserve_all_or_nothing(input);
            return plan;
        };
        // Fail-closed global completeness: any traversal truncation or
        // page-tree skip could hide a page, so no clone set is seeded at all.
        let globally_complete =
            bindings.truncated.is_none() && bindings.page_tree_skipped.is_empty();

        let mut plan = Self::empty(input.len(), globally_complete);
        // No closure can run after an incomplete binding walk, so avoid
        // projecting the structural frontier in that fail-closed case.
        let frontier = globally_complete.then(|| {
            StructuralFrontier::new(
                catalog_reference,
                page_tree_root_reference,
                bindings.pages.iter().map(|page| page.page_reference),
                consumers,
            )
        });
        let mut page_identities = Vec::new();
        for page in &bindings.pages {
            // `select_indices` supplies sorted, deduplicated ordinals. The
            // planner observes requested pages only; unselected witnesses do
            // not consume closure or reservation budgets.
            if selected_page_ordinals.binary_search(&page.ordinal).is_err() {
                continue;
            }
            let identity = CloneSetPageIdentity {
                ordinal: page.ordinal,
                reference: page.page_reference,
                object_byte_offset: page.page_object_byte_offset,
            };
            if let Some(frontier) = frontier.as_ref() {
                plan.seed_page_sets(input, lookup, frontier, identity, page);
            } else {
                plan.count_page_without_walking(page);
            }
            page_identities.push(identity);
        }

        plan.sets.sort_by_key(|set| (set.page.ordinal, set.root));
        plan.reserve_all_or_nothing(input);
        plan.build_page_slots(&page_identities);
        plan
    }

    /// Run the staged clone-body export over every planned set and commit the
    /// staged counters atomically.
    ///
    /// Must run immediately after [`FormCloneSetPlan::build`] and BEFORE any
    /// [`FormCloneSetPlan::page_counts`] consumption, so the staged counters
    /// ride the same per-page projection. The batch build is all-or-nothing:
    /// counters are written only after the whole batch has succeeded or
    /// failed — never partially. On success every planned set folds its
    /// staged member/byte totals into its page slot; on failure every planned
    /// set is marked export-suppressed with the distinct export-refused
    /// counter, never masquerading as a closure or reservation refusal.
    ///
    /// STAGED/VALIDATE-ONLY: the returned batch is never handed to the writer
    /// by production callers, so emitted product bytes stay byte-identical to
    /// the plan-only behaviour. Tests drive the public fresh-object writer
    /// with the returned batch directly to prove end-to-end viability.
    pub(crate) fn stage_export(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
    ) -> Result<Vec<FreshObjectBytes>, CloneSetExportRefusal> {
        match build_staged_export(input, lookup, self) {
            Ok(batch) => {
                self.commit_staged_counts(&batch);
                Ok(batch.fresh_objects)
            }
            Err(refusal) => {
                self.commit_export_refused_counts();
                Err(refusal)
            }
        }
    }

    /// Fold one successful batch's staged totals into the per-page slots,
    /// through the same identity-corroborated join as the plan counts.
    fn commit_staged_counts(&mut self, batch: &StagedExportBatch) {
        for set in &batch.staged_sets {
            let page = self.sets[set.set_index].page;
            let Some(counts) = self.page_slot_counts(page) else {
                continue;
            };
            counts.staged_sets += 1;
            counts.staged_objects += set.member_count;
            counts.staged_body_bytes += set.body_bytes_total;
        }
    }

    /// Mark every otherwise-ready set export-suppressed after a batch
    /// refusal (all-or-nothing: no partial staging survives).
    fn commit_export_refused_counts(&mut self) {
        for set_index in 0..self.sets.len() {
            if !matches!(
                self.sets[set_index].outcome,
                CloneSetOutcome::Planned { .. }
            ) {
                continue;
            }
            let page = self.sets[set_index].page;
            if let Some(counts) = self.page_slot_counts(page) {
                counts.export_refused_sets += 1;
            }
        }
    }

    /// Mutable identity-corroborated access to one page's counts slot; a
    /// missing, poisoned, or inconsistent slot is `None`, fail-closed.
    fn page_slot_counts(
        &mut self,
        page: CloneSetPageIdentity,
    ) -> Option<&mut FormCloneSetPlanCounts> {
        let slot = self.page_slots.get_mut(&page.reference)?.as_mut()?;
        (slot.object_byte_offset == page.object_byte_offset && slot.ordinal == page.ordinal)
            .then_some(&mut slot.counts)
    }

    /// Resolve one page's identity-corroborated counts; any missing,
    /// duplicate, or inconsistent match is the empty counts, fail-closed.
    pub(crate) fn page_counts(
        &self,
        reference: IndirectRef,
        object_byte_offset: usize,
        ordinal: usize,
    ) -> FormCloneSetPlanCounts {
        let Some(Some(slot)) = self.page_slots.get(&reference) else {
            return FormCloneSetPlanCounts::new();
        };
        if slot.object_byte_offset == object_byte_offset && slot.ordinal == ordinal {
            slot.counts
        } else {
            FormCloneSetPlanCounts::new()
        }
    }

    /// Test-only constructor: drive the staged export directly with
    /// hand-built sets (page-slot counter folding is exercised through real
    /// plans; this plan has no slots).
    #[cfg(test)]
    pub(crate) fn from_sets_for_tests(input_byte_len: usize, sets: Vec<FormCloneSet>) -> Self {
        Self {
            sets,
            ..Self::empty(input_byte_len, true)
        }
    }

    const fn empty(input_byte_len: usize, globally_complete: bool) -> Self {
        Self {
            input_byte_len,
            sets: Vec::new(),
            in_place_candidates: 0,
            disqualified_witnesses: 0,
            globally_complete,
            page_slots: BTreeMap::new(),
        }
    }

    /// Group one page's qualifying witnesses into `(page, target)` seeds
    /// (deterministic by target reference), walk each seed's closure, and
    /// append the sets in plan order.
    fn seed_page_sets(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        frontier: &StructuralFrontier,
        identity: CloneSetPageIdentity,
        page: &PageXObjectBindingsInspection,
    ) {
        let mut seeds: BTreeMap<IndirectRef, (usize, Vec<CloneSetRetargetSite>)> = BTreeMap::new();
        for witness in &page.witnesses {
            match classify_seed(witness) {
                SeedClass::CloneCandidate => {
                    let seed = seeds
                        .entry(witness.target)
                        .or_insert_with(|| (witness.target_object_byte_offset, Vec::new()));
                    seed.1.push(CloneSetRetargetSite {
                        name: witness.name.clone(),
                        key_range: witness.key_range,
                        value_range: witness.value_range,
                        expected_target: witness.target,
                    });
                }
                SeedClass::InPlace => self.in_place_candidates += 1,
                SeedClass::Disqualified => self.disqualified_witnesses += 1,
            }
        }

        for (root, (root_object_byte_offset, retarget_sites)) in seeds {
            let walk = walk_reached_form_closure(input, lookup, frontier, root);
            let outcome = match walk.result {
                Ok(closure) => CloneSetOutcome::Planned {
                    members: closure.members,
                    null_equivalents: closure.null_equivalents,
                    source_to_fresh: Vec::new(),
                },
                Err(refusal) => CloneSetOutcome::Refused { refusal },
            };
            self.sets.push(FormCloneSet {
                page: identity,
                root,
                root_object_byte_offset,
                retarget_sites,
                budget: walk.budget,
                outcome,
            });
        }
    }

    /// Count one page's witnesses without walking anything (the binding walk
    /// was not globally complete, so no seed may qualify).
    fn count_page_without_walking(&mut self, page: &PageXObjectBindingsInspection) {
        for witness in &page.witnesses {
            match classify_seed(witness) {
                SeedClass::InPlace => self.in_place_candidates += 1,
                SeedClass::CloneCandidate | SeedClass::Disqualified => {
                    self.disqualified_witnesses += 1;
                }
            }
        }
    }

    /// The single all-or-nothing request reservation: exactly one
    /// [`crate::reserve_fresh_object_references`] call over the total member
    /// count of every non-refused set, partitioned deterministically in plan
    /// order (sets already sorted by page ordinal then root reference,
    /// members already sorted by source reference). A failure — including
    /// the Annex C limit check that runs before the plan is declared ready —
    /// refuses EVERY would-be-planned set with the typed reservation class.
    fn reserve_all_or_nothing(&mut self, input: &[u8]) {
        let total: usize = self
            .sets
            .iter()
            .map(|set| match &set.outcome {
                CloneSetOutcome::Planned { members, .. } => members.len(),
                CloneSetOutcome::Refused { .. } => 0,
            })
            .sum();
        let refusal = match crate::reserve_fresh_object_references(input, total) {
            Err(error) => Some(ReservationRefusal::FloorProof {
                error: Box::new(error),
            }),
            Ok(fresh) if fresh.len() != total => Some(ReservationRefusal::ContractMismatch {
                requested: total,
                reserved: fresh.len(),
            }),
            Ok(fresh) => {
                let highest = fresh.last().map_or(0, |reference| reference.object_number);
                if highest > ANNEX_C_MAX_INDIRECT_OBJECT_NUMBER {
                    Some(ReservationRefusal::AnnexCObjectLimitExceeded {
                        highest_reserved: highest,
                        limit: ANNEX_C_MAX_INDIRECT_OBJECT_NUMBER,
                    })
                } else {
                    self.partition_reservation(&fresh);
                    None
                }
            }
        };
        if let Some(refusal) = refusal {
            for set in &mut self.sets {
                if matches!(set.outcome, CloneSetOutcome::Planned { .. }) {
                    set.outcome = CloneSetOutcome::Refused {
                        refusal: CloneSetRefusal::ReservationRefused {
                            reason: refusal.clone(),
                        },
                    };
                }
            }
        }
    }

    /// Zip the reserved identities across planned sets in plan order. Each
    /// set keeps its own private source-to-fresh pairs, so the same source
    /// object on different pages receives distinct fresh identities. The
    /// caller already proved `fresh` covers the summed member count, so the
    /// sliced partition is total by construction.
    fn partition_reservation(&mut self, fresh: &[IndirectRef]) {
        let mut offset = 0;
        for set in &mut self.sets {
            let CloneSetOutcome::Planned {
                members,
                source_to_fresh,
                ..
            } = &mut set.outcome
            else {
                continue;
            };
            let slice = fresh.get(offset..offset + members.len()).unwrap_or(&[]);
            offset += members.len();
            *source_to_fresh = members
                .iter()
                .zip(slice)
                .map(|(member, fresh_reference)| (member.source, *fresh_reference))
                .collect();
        }
    }

    /// Fold per-page counts into identity-poisoned slots: a duplicate leaf
    /// reference poisons its slot, so the later exact join fails closed.
    fn build_page_slots(&mut self, page_identities: &[CloneSetPageIdentity]) {
        for identity in page_identities {
            self.page_slots
                .entry(identity.reference)
                .and_modify(|slot| *slot = None)
                .or_insert(Some(PageCountsSlot {
                    object_byte_offset: identity.object_byte_offset,
                    ordinal: identity.ordinal,
                    counts: FormCloneSetPlanCounts::new(),
                }));
        }

        // Fold each set once through the exact-reference index instead of
        // rescanning every set for every selected page.
        for set in &self.sets {
            let Some(Some(slot)) = self.page_slots.get_mut(&set.page.reference) else {
                continue;
            };
            if slot.object_byte_offset != set.page.object_byte_offset
                || slot.ordinal != set.page.ordinal
            {
                continue;
            }
            slot.counts.candidate_sets += 1;
            match &set.outcome {
                CloneSetOutcome::Planned {
                    members,
                    null_equivalents,
                    ..
                } => {
                    slot.counts.planned_sets += 1;
                    slot.counts.planned_objects += members.len();
                    slot.counts.null_equivalents += null_equivalents.len();
                }
                CloneSetOutcome::Refused { .. } => slot.counts.refused_sets += 1,
            }
        }
    }
}

/// How one binding witness relates to clone-set seeding.
enum SeedClass {
    /// Qualifies as a clone-set seed.
    CloneCandidate,
    /// `ProvenPageLocal`: an in-place candidate, no clone is needed.
    InPlace,
    /// Counted, never walked: wrong subtype, non-leaf-direct path, or a
    /// non-qualifying verdict (including an incomplete consumer index).
    Disqualified,
}

/// Seed qualification: decoded `/Subtype /Form`, an explicitly leaf-direct
/// binding path (direct leaf `/Resources`, direct `/Resources` and
/// `/XObject` dictionaries), and the exact
/// `Unproven { TargetConsumersNotExclusive }` verdict. The path checks are
/// asserted explicitly even though the verdict's fixed check order implies
/// them, so a future verdict reordering cannot silently widen seeding.
fn classify_seed(witness: &PageXObjectBindingWitness) -> SeedClass {
    if witness.subtype != XObjectBindingSubtype::Form {
        return SeedClass::Disqualified;
    }
    match &witness.verdict {
        PageXObjectBindingVerdict::ProvenPageLocal => SeedClass::InPlace,
        PageXObjectBindingVerdict::Unproven {
            reason: PageXObjectBindingUnprovenReason::TargetConsumersNotExclusive { .. },
        } => {
            let leaf_direct = matches!(
                witness.path.resources_source,
                BindingResourcesSource::Direct { .. }
            ) && witness.path.resources_locality
                == BindingContainerLocality::DirectDictionary
                && witness.path.xobject_locality == BindingContainerLocality::DirectDictionary;
            if leaf_direct {
                SeedClass::CloneCandidate
            } else {
                SeedClass::Disqualified
            }
        }
        PageXObjectBindingVerdict::Unproven { .. } => SeedClass::Disqualified,
    }
}
