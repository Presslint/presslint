//! Request-scoped exact-identity analysis of demanded Form `XObject`
//! inherited-colour effects.
//!
//! [`FormXObjectEffectAnalyzer`] is the ONE new abstraction this slice adds. It
//! answers exactly one read-only question about a demanded Form — root-page or
//! nested: does painting inside the Form consume the CALLER's inherited
//! stroking and/or nonstroking colour? It never mutates, clones, or stages a
//! Form; each compute attempt reads one admitted stream, proves a narrow
//! structural shape, summarizes a two-bit lane effect, and descends only into
//! invoked, retained nested ordinary Forms through the bounded recursion below
//! (T190).
//!
//! The analyzer is request-scoped and shared across every selected page. For
//! each exact tuple `(object number, generation, reached object byte offset)`
//! it caches ONE depth-indexed [`recurse::BoundedFormLaneEffects`] lattice:
//! slot `d` is the two-bit/Unknown effect with `d` nested-Form edges remaining,
//! slot 0 refuses every invoked nested Form, and the public result is the
//! maximum-depth slot. Every computed slot is a pure function of the form's
//! OWN subtree, so the same Form reached under multiple resource names,
//! repeated `Do` invocations, several nesting parents, or across selected
//! pages serves one cached lattice in any analysis order. A complete cache hit
//! never decodes or walks again; only a horizon-cut partial entry may deepen
//! through one bounded re-decode per shallower compute attempt. Map presence
//! distinguishes a cached Unknown from an unseen target. A fixed first-seen
//! target cap charges once per unique identity, while one aggregate decoded-
//! byte budget charges every successfully read body and every budget-dependent
//! failed attempt across the whole request. An over-limit raw body or Flate
//! inflation exhausts the residual byte budget, so it cannot be retried;
//! intrinsic decode failures retain their deterministic refusal behavior.
//!
//! Admission is intentionally conservative and two-layered. A demanded Form is
//! summarized only after:
//!
//! 1. exact identity corroboration — the reference re-resolves through the
//!    current [`ObjectLookup`] to a source-addressable in-use object whose
//!    generation and byte offset match the reached target;
//! 2. one decoded-name Form-dictionary preflight that refuses external stream
//!    data, reference `XObjects`, optional content, OPI substitution, malformed
//!    names, and any noncanonical/duplicate safety key delegated to a raw-key
//!    inspector;
//! 3. a raw or single-`/FlateDecode`/default-predictor bounded decode (any
//!    filter chain, unsupported filter, non-default predictor, or decode failure
//!    is Unknown);
//! 4. proven absence of any transparency `/Group` through the existing public
//!    Form group inspector (`group.is_none()` AND `skipped.is_empty()`);
//! 5. a strict CLOSED raw-record preflight allowlist with a path/`q`-stack
//!    grammar (any other operator, arity, or context is Unknown);
//! 6. exactly ONE seeded [`PaintProgram`] walk over those records, whose
//!    inherited lanes carry distinct source-less sentinels.
//!
//! Neither layer may be omitted. The raw pass is a syntax/refusal grammar only —
//! it never models colour state, and because the walker collapses unsupported
//! raw operators to a no-op it is the ONLY thing that can refuse them. The paint
//! walk owns `q`/`Q`, direct setter lane kills, and the exact colour compared at
//! each path paint. A lane is consumed only when the current colour still equals
//! its inherited sentinel; a valid direct setter stamps a concrete source range,
//! so even a numerically identical setter can never recreate a source-less
//! sentinel.
//!
//! A narrow set of explicit Form-LOCAL device colour operations is also admitted
//! (T188), implemented in the private [`color`] submodule. When a `CS`/`cs`
//! resource colour operator appears, one bounded decoded-name authority is
//! projected from the Form's OWN `/Resources /ColorSpace` (never page fallback)
//! with collision, malformed-name, `/Default*` and skip poisoning. Raw authority
//! keys must first be canonical and unique; reported facts and distinct used
//! operand spellings each have a separate 256-entry cap before the writer
//! map/environment can grow. Only canonical `/DeviceGray`/`/DeviceRGB`/
//! `/DeviceCMYK` and unique Form-local aliases resolving directly to one of
//! those families are supported, and only while the matching `/Default*` binding
//! is proven absent. Each raw operand spelling actually used is resolved through
//! that authority before an ephemeral [`ColorSpaceEnv`] resource is handed to
//! the single walk, whose raw-name environment is never the semantic authority.
//! `CS`/`cs` applies the ISO initial colour and kills only its selected
//! inherited lane; `SC`/`SCN`/`sc`/`scn` is admitted only over a proven local
//! Device lane with exact 1/3/4 arity and no Pattern operand.
//!
//! Invoked Form-LOCAL Image `XObject` effects are admitted next (T189),
//! implemented in the private [`xobjects`] submodule. When a syntactically valid
//! `Do` is present, ONE bounded decoded-name authority is built from the Form's
//! OWN `/Resources /XObject` (never page fallback) with the same canonical-key,
//! collision, named-skip, literal-poison and nameless-skip semantics as the
//! colour authority, plus a 256-fact cap. Each classified Image target is
//! re-corroborated — reference, generation, reached offset, exact reinspected
//! dictionary — and proven canonical `/Subtype /Image` with unambiguous
//! `/ImageMask` authority and no substitution/optional-content/external escape
//! (`/Alternates`, `/OPI`, `/OC`, `/F`, `/Ref`) before the shipped page
//! ordinary/stencil classifier is trusted. An admitted ordinary Image consumes
//! neither lane; a proven stencil uses the CURRENT nonstroking colour for
//! marking (ISO 32000-1 §8.9.6.2), so it consumes the inherited lane only while
//! that lane still equals its sentinel. `/Subtype /Form` targets
//! pass the same exact-identity corroboration plus a canonical semantically
//! unique `/Subtype /Form` proof before their tuple is retained; every
//! uncertain name or target refuses the whole Form.
//!
//! Proven-neutral `gs` activations are admitted next (T191), implemented in
//! the private [`extgstate`] submodule. When a syntactically valid `gs` is
//! present, ONE bounded decoded-name authority is built from the Form's OWN
//! `/Resources /ExtGState` (never page fallback) with the same canonical-key,
//! collision, named-skip, literal-poison, nameless-skip and 256-fact/spelling
//! cap semantics as the other authorities. Every executed `gs` must resolve to
//! exactly one classified entry proven colour-lane neutral and font-inert
//! through the shipped classifier facts: no active overprint (ANY set `/OPM`,
//! including `0`), no active transparency (alpha, blend mode, soft mask), no
//! unresolved or unclassified safety parameter, `font_effect` exactly `Unset`,
//! and no unclassified keys at all. The gate runs BEFORE the walk and is not a
//! state machine — neutrality of every activated entry makes activation order
//! irrelevant, so the single walk keeps its empty `ExtGState` environment and
//! a proven-neutral `gs` is a no-op for the two-bit lane question. Every other
//! `gs` refuses the whole Form.
//!
//! Invoked, retained nested ordinary Form targets are analyzed by bounded
//! recursion (T190), implemented over the private [`recurse`] lattice. A child
//! enters through the complete root path above — target/byte budgets, exact
//! identity corroboration, the semantic dictionary preflight, filter/extent
//! classification, proven Group absence, the raw grammar, and its OWN
//! demand-built colour and `XObject` authorities (`Ok(None)` proven-absent
//! `/Resources` stays admissible exactly as for a root) — and is ALWAYS walked
//! sentinel-seeded exactly like a root, so its cached lattice is a pure
//! function of its own bytes and resources; no caller or page state ever
//! reaches a child analysis. At the parent's `Do`, each parent slot `d >= 1`
//! folds the child's slot `d - 1`: a child bit propagates only while the
//! parent's matching lane still equals its inherited sentinel (a live local
//! colour absorbs the consumption), an Unknown child slot refuses the parent
//! slot, and parent slot 0 refuses every nested invocation. Depth is the
//! recursion path length itself, and the TRAVERSAL is bounded to the same
//! horizon as the lattice: an unseen frame is entered only while the path
//! stays within [`recurse::MAX_NESTED_FORM_DEPTH`] nested edges (nine form
//! streams, nine native compute frames), so a deeper acyclic chain never
//! decodes, charges, or stacks past the horizon — its cut frames cache
//! PARTIAL lattices whose computed slots are pure, and a later shallower
//! query deepens them with a recompute that recharges only the real
//! re-decode, never the first-seen target. An `IndirectRef`-keyed active-path
//! set refuses every direct or mutual cycle before any charge, while
//! legitimate DAG reuse spends one first-seen target and serves every complete
//! entry without further byte charge.
//! Publication stays unconditional post-compute, and recursion happens
//! internally on the analyzer under the caller's single borrow — never back
//! through the page policy.
//!
//! This makes only a colour-lane-dependency claim. `/Matrix` and `/BBox` are
//! unmodelled: a non-identity `/Matrix` cannot change which colour lane a paint
//! reads, and `/BBox` clipping can only suppress paint, so ignoring both
//! may at worst over-report consumption (a conservative positive) and can never
//! fabricate an unsafe false Neutral; the argument extends transitively to
//! every admitted nested child. No Form byte is read as mutation authority,
//! no CTM/bounds/visibility is published, and every group, uncertain resource
//! colour, unproven `gs`, text, shading, inline image, and unknown construct
//! refuses.

mod color;
mod extgstate;
mod recurse;
mod refusal;
mod xobjects;

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use presslint_paint::{
    ColorSpaceEnv, GraphicsColor, GraphicsStateSnapshot, PaintOpKind, PaintProgram,
};
use presslint_pdf::{
    DictionaryEntrySpan, DictionaryValueKind, FlateDecodeParameters, FlateDecodeStreamRejection,
    IndirectRef, ObjectLookup, ObjectLookupLocation, content_stream_data_slice,
    decode_flate_stream, inspect_content_stream_data_extent_with_lookup,
    inspect_dictionary_entries, inspect_form_transparency_group,
    inspect_indirect_object_dictionary, locate_xref_object, parse_indirect_reference,
};
use presslint_syntax::{
    OperandRecord, OperatorRecord, Token, TokenKind, assemble_operators, tokenize,
};
use presslint_types::ColorSpace;

use crate::{
    content_edit_pipeline::{MAX_CONTENT_STREAM_BYTES, PipelineFilterKind, classify_filter},
    page_xobject_policy::{PageXObjectEffect, decode_pdf_name},
};

use recurse::BoundedFormLaneEffects;
pub use refusal::{FormXObjectRefusalClass, FormXObjectRefusalCounts};
use xobjects::FormLocalXObjectAuthority;

#[cfg(test)]
pub use xobjects::xobject_target_identity_corroborates_for_test;

/// Fixed per-request cap on first-seen exact Form targets. Further unseen
/// identities after this many attempts are deterministically Unknown.
const MAX_FORM_TARGETS: usize = 256;

/// Inherited stroking/nonstroking lane effect of one analyzed Form:
/// `[consumes_inherited_stroking, consumes_inherited_nonstroking]`, in the same
/// stroking-first lane order the alias planner uses.
pub type FormLaneEffect = [bool; 2];

/// One exact identity's cached pure lattice and observe-only refusal
/// attribution. Keeping both in one value avoids a second keyed allocation;
/// the lattice remains the sole input to every analysis decision.
#[derive(Clone, Copy)]
struct CachedFormAnalysis {
    lattice: BoundedFormLaneEffects,
    refusal_class: Option<FormXObjectRefusalClass>,
}

/// Request-scoped exact-identity cache and bounds for Form colour-effect
/// analysis, root and nested. Conceptually owns a
/// `(reference, reached_offset) -> CachedFormAnalysis` map (map presence =
/// seen; an all-Unknown lattice = cached refusal), a remaining first-seen
/// computation-charge count, and one aggregate remaining decoded-byte budget
/// shared across the whole request, nested frames included.
pub struct FormXObjectEffectAnalyzer {
    cache: BTreeMap<(IndirectRef, usize), CachedFormAnalysis>,
    remaining_targets: usize,
    remaining_bytes: usize,
    /// Exact demanded `(reference, reached_offset)` identities already counted
    /// for the CURRENT page's tally. Reset at each page boundary so a
    /// cache-hit re-count on a new page still counts once there.
    page_seen: BTreeSet<(IndirectRef, usize)>,
    /// The current page's tallied refusal-class counts.
    page_counts: FormXObjectRefusalCounts,
}

/// Borrowed per-`analyze` descent state threaded through the bounded nested-
/// Form recursion: the request bytes and lookup every frame shares, plus the
/// `IndirectRef`-keyed active-path set that refuses every direct or mutual
/// cycle before any target or byte charge. Depth needs no second counter: the
/// path length is the recursion itself, and the depth-indexed lattice slots
/// bound what any path can prove.
struct FormDescent<'request> {
    /// The whole request input the recursion re-reaches child objects in.
    input: &'request [u8],
    /// The request's already-open exact object lookup.
    lookup: ObjectLookup<'request>,
    /// Out-of-band object identities of every frame currently on the descent
    /// path. Keyed by `IndirectRef` only: exact corroboration admits at most
    /// one reached offset per reference under the current lookup, so ref-only
    /// keying is strictly conservative against offset variation fragmenting
    /// cycle detection.
    active: BTreeSet<IndirectRef>,
}

impl FormXObjectEffectAnalyzer {
    /// Create one analyzer for a whole conversion request, budgeted by the
    /// existing aggregate decoded-byte cap and the fixed first-seen target cap.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
            remaining_targets: MAX_FORM_TARGETS,
            remaining_bytes: MAX_CONTENT_STREAM_BYTES,
            page_seen: BTreeSet::new(),
            page_counts: FormXObjectRefusalCounts::new(),
        }
    }

    /// Build an analyzer with explicit bounds. Test-only: production always uses
    /// [`Self::new`] with the fixed first-seen target cap and the aggregate
    /// decoded-byte budget.
    #[cfg(test)]
    pub(crate) const fn with_bounds(remaining_targets: usize, remaining_bytes: usize) -> Self {
        Self {
            cache: BTreeMap::new(),
            remaining_targets,
            remaining_bytes,
            page_seen: BTreeSet::new(),
            page_counts: FormXObjectRefusalCounts::new(),
        }
    }

    /// Reset per-page refusal tallying before a fresh page's Form demands
    /// begin: clears the per-page identity-seen dedup set and per-page counts
    /// so a prior (possibly aborted) page's partial tally never bleeds into
    /// the next page.
    pub(crate) fn begin_page_refusal_tally(&mut self) {
        self.page_seen.clear();
        self.page_counts = FormXObjectRefusalCounts::new();
    }

    /// Take this page's tallied refusal-class counts, ending the tally.
    pub(crate) fn take_page_refusal_counts(&mut self) -> FormXObjectRefusalCounts {
        std::mem::take(&mut self.page_counts)
    }

    /// Remaining aggregate decoded-byte budget. Test-only evidence that a
    /// budget-dependent failed read cannot be retried.
    #[cfg(test)]
    pub(crate) const fn remaining_bytes_for_test(&self) -> usize {
        self.remaining_bytes
    }

    /// Highest computed slot retained for one cached identity. Test-only
    /// evidence that an unaffordable deepening never overwrites a partial
    /// lattice's already-computed pure slots.
    #[cfg(test)]
    pub(crate) fn cached_computed_through_for_test(
        &self,
        reference: IndirectRef,
        reached_offset: usize,
    ) -> Option<usize> {
        self.cache
            .get(&(reference, reached_offset))
            .map(|entry| entry.lattice.computed_through())
    }

    /// Summarize the inherited-colour lane effect of one demanded Form target
    /// reached at `reached_offset` under `reference`.
    ///
    /// Returns `Some([stroking, nonstroking])` for a proven effect (a neutral
    /// Form is `Some([false, false])`) or `None` for Unknown (unaddressable,
    /// grouped, malformed, out-of-budget, cyclic, deeper than the nested-Form
    /// depth bound, or outside the raw allowlist). The result is the cached
    /// lattice's maximum-depth slot. A complete cache hit is O(log F) and
    /// recharges neither the target count nor the byte budget; only a lattice
    /// left partial by the traversal horizon deepens with a recompute. A
    /// successful re-decode charges bytes alone; a budget-dependent failed
    /// attempt exhausts the residual bytes and preserves the partial entry.
    pub fn analyze(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        reference: IndirectRef,
        reached_offset: usize,
    ) -> Option<FormLaneEffect> {
        let mut descent = FormDescent {
            input,
            lookup,
            active: BTreeSet::new(),
        };
        // A root call always sits within the horizon (the active path is
        // empty), so the frame is never unavailable; a lattice left partial
        // by exhaustion reads its uncomputed top slot as Unknown.
        let analyzed = self.analyze_nested(&mut descent, reference, reached_offset);
        let effect = analyzed.and_then(|entry| entry.lattice.max_depth_effect());
        // Observe-only per-page refusal tally: this never influences `effect`.
        // Count once per exact demanded identity per page; a cache hit still
        // counts here (zero decode/walk/budget charge) since the class comes
        // from the same already-cached entry as the lattice.
        if effect.is_none() && self.page_seen.insert((reference, reached_offset)) {
            if let Some(class) = analyzed.and_then(|entry| entry.refusal_class) {
                self.page_counts.add(class);
            }
        }
        effect
    }

    /// The single recursive entry every root analysis and nested descent uses.
    /// `None` is an UNAVAILABLE frame: the target sits past the traversal
    /// horizon with nothing cached, so nothing is charged, published, or
    /// descended into and the caller's affected slots stay uncomputed.
    ///
    /// In this order: (1) a reference already on the active path is a cycle —
    /// the frame is all-Unknown for THIS caller and nothing is charged or
    /// published (the cycle still closes from every member's own compute, so
    /// each member caches all-Unknown order-independently); (2) a cache entry
    /// serves whenever a compute from this depth could not reach past what it
    /// already holds — every complete entry, and a partial entry at or past
    /// the horizon; (3) within the horizon, an unseen identity runs the full
    /// root-identical compute and a partial entry re-entered with more
    /// remaining edges than it has computed runs a DEEPENING recompute (bytes
    /// recharge for the real re-decode, the first-seen target does not), each
    /// with its reference held in the active set; (4) the computed lattice is
    /// published unconditionally post-compute — extending, never shrinking, a
    /// prior partial entry — so no in-progress entry is ever visible. A
    /// deepening recompute that cannot be afforded keeps the previously
    /// published pure entry serving, exactly like every other cached form
    /// under exhaustion.
    fn analyze_nested(
        &mut self,
        descent: &mut FormDescent<'_>,
        reference: IndirectRef,
        reached_offset: usize,
    ) -> Option<CachedFormAnalysis> {
        if descent.active.contains(&reference) {
            return Some(CachedFormAnalysis {
                lattice: BoundedFormLaneEffects::all_unknown(),
                refusal_class: Some(FormXObjectRefusalClass::RecursionCycle),
            });
        }
        let key = (reference, reached_offset);
        // Edges already used equal the active path length; entering this
        // frame keeps the path within the horizon only while `remaining`
        // exists, and the frame can then compute slots up to `remaining`.
        let remaining = recurse::MAX_NESTED_FORM_DEPTH.checked_sub(descent.active.len());
        let cached = self.cache.get(&key).copied();
        if let Some(entry) = cached {
            if remaining.is_none_or(|edges| edges <= entry.lattice.computed_through()) {
                return Some(entry);
            }
        }
        remaining?;
        descent.active.insert(reference);
        let computed = self.compute(descent, reference, reached_offset, cached.is_none());
        descent.active.remove(&reference);
        let entry = match computed {
            Ok(mut entry) => {
                if entry.lattice.max_depth_effect().is_some() {
                    entry.refusal_class = None;
                }
                entry
            }
            // A failed FIRST compute is the intrinsic/unaffordable refusal of
            // every slot; a failed deepening recompute (only ever budget
            // exhaustion — every intrinsic gate is deterministic over the
            // same bytes) keeps the prior pure entry instead of destroying
            // its computed slots.
            Err(class) => {
                // A deepening failure keeps the lattice unchanged, but THIS
                // query's fresh class must replace the cached attribution.
                let mut entry = cached.unwrap_or_else(|| CachedFormAnalysis {
                    lattice: BoundedFormLaneEffects::all_unknown(),
                    refusal_class: None,
                });
                entry.refusal_class = Some(class);
                entry
            }
        };
        self.cache.insert(key, entry);
        Some(entry)
    }

    /// Compute one exact identity's lattice, charging one first-seen target
    /// attempt (`charge_target`, never for a deepening recompute of a cached
    /// partial entry) and — for every successfully sliced/decoded body, first
    /// compute or recompute alike — its bounded byte cost, identically for a
    /// root and a nested child. A budget-dependent failed raw read or Flate
    /// inflation exhausts the residual byte budget so later deepening stops at
    /// the zero-budget guard; intrinsic decoder failures leave it unchanged.
    /// `Err` on a first compute is a refusal of every slot and is cached as the
    /// all-Unknown lattice.
    fn compute(
        &mut self,
        descent: &mut FormDescent<'_>,
        reference: IndirectRef,
        reached_offset: usize,
        charge_target: bool,
    ) -> Result<CachedFormAnalysis, FormXObjectRefusalClass> {
        if charge_target {
            if self.remaining_targets == 0 {
                return Err(FormXObjectRefusalClass::TargetBudget);
            }
            self.remaining_targets -= 1;
        }
        // Aggregate budget exhaustion makes every remaining unseen target
        // Unknown, including a zero-byte Form; cached identities remain usable.
        if self.remaining_bytes == 0 {
            return Err(FormXObjectRefusalClass::DecodedByteBudget);
        }
        if !corroborates(descent.lookup, reference, reached_offset) {
            return Err(FormXObjectRefusalClass::StructuralPreflight);
        }
        // The raw-key extent/filter/group helpers are safe only after every
        // top-level key has been decoded and their delegated spellings are
        // proved canonical and unique. This also refuses stream substitution,
        // reference XObjects, optional content and OPI before any body access.
        if !semantic_dictionary_preflight(descent.input, reached_offset) {
            return Err(FormXObjectRefusalClass::StructuralPreflight);
        }
        // Accept only raw/uncompressed or a single default-predictor
        // `/FlateDecode`; every other filter/chain/predictor is Unknown.
        let filter = classify_filter(descent.input, reached_offset)
            .map_err(|_| FormXObjectRefusalClass::StreamFilterOrExtent)?;
        let extent = inspect_content_stream_data_extent_with_lookup(
            descent.input,
            Some(descent.lookup),
            reached_offset,
        )
        .map_err(|_| FormXObjectRefusalClass::StreamFilterOrExtent)?;
        // Prove the Form declares no transparency group before any body walk;
        // any valid, malformed, indirect-unresolved, or uncertain group is
        // Unknown.
        let group = inspect_form_transparency_group(descent.input, descent.lookup, reached_offset);
        if group.group.is_some() || !group.skipped.is_empty() {
            return Err(FormXObjectRefusalClass::TransparencyGroup);
        }
        // Borrow raw bytes directly; bound a Flate decode by the remaining
        // aggregate budget and drop the owned buffer once the lattice is read.
        match filter {
            PipelineFilterKind::Raw => {
                if extent.length() > self.remaining_bytes {
                    // The whole residual allowance was insufficient for this
                    // attempt. Exhaust it so a horizon-cut partial entry cannot
                    // trigger the same over-budget body read indefinitely.
                    self.remaining_bytes = 0;
                    return Err(FormXObjectRefusalClass::DecodedByteBudget);
                }
                let slice = content_stream_data_slice(descent.input, &extent)
                    .map_err(|_| FormXObjectRefusalClass::StreamFilterOrExtent)?;
                self.remaining_bytes -= slice.len();
                self.analyze_bytes(descent, reached_offset, slice)
            }
            PipelineFilterKind::Flate => {
                let slice = content_stream_data_slice(descent.input, &extent)
                    .map_err(|_| FormXObjectRefusalClass::StreamFilterOrExtent)?;
                let decoded = match decode_flate_stream(
                    slice,
                    FlateDecodeParameters::default(),
                    self.remaining_bytes,
                ) {
                    Ok(decoded) => decoded,
                    Err(error) => {
                        if error.reason == FlateDecodeStreamRejection::OutputLimitExceeded {
                            // Inflation consumed the attempt's entire output
                            // allowance. Exhaust the shared residual budget so
                            // an unchanged partial cache entry cannot amplify
                            // the same bounded work on later queries.
                            self.remaining_bytes = 0;
                            return Err(FormXObjectRefusalClass::DecodedByteBudget);
                        }
                        // Every other decode failure is intrinsic to these
                        // bytes (invalid zlib), not a budget effect.
                        return Err(FormXObjectRefusalClass::StreamFilterOrExtent);
                    }
                };
                self.remaining_bytes -= decoded.len();
                self.analyze_bytes(descent, reached_offset, &decoded)
            }
        }
    }

    /// Tokenize/assemble the decoded Form once, run the closed raw preflight,
    /// then the ONE seeded paint walk. Any parse, preflight, or walk failure is
    /// an intrinsic refusal of every slot. The decoded bytes, tokens, records,
    /// authority, and walk state are all dropped on return.
    ///
    /// The Form-local device colour projection is built only when a `CS`/`cs`
    /// resource colour operator is present, the Form-local `ExtGState` gate
    /// runs only when a syntactically valid `gs` is present, and the
    /// Form-local `XObject` authority is built only when a syntactically valid
    /// `Do` is present; otherwise the walk runs with an empty [`ColorSpaceEnv`]
    /// and no authority, byte-for-byte reproducing the resource-independent
    /// T187 result. The projection borrows `resources`, which must outlive the
    /// walk.
    fn analyze_bytes(
        &mut self,
        descent: &mut FormDescent<'_>,
        reached_offset: usize,
        decoded: &[u8],
    ) -> Result<CachedFormAnalysis, FormXObjectRefusalClass> {
        let tokens = tokenize(decoded).map_err(|_| FormXObjectRefusalClass::RawGrammar)?;
        let assembled =
            assemble_operators(&tokens).map_err(|_| FormXObjectRefusalClass::RawGrammar)?;
        let records = assembled.records;
        if !raw_preflight_ok(&tokens, &records, decoded) {
            return Err(FormXObjectRefusalClass::RawGrammar);
        }
        // Demand for the ExtGState gate is a syntactically valid `gs`; a Form
        // without one never inspects its own `/Resources /ExtGState`, even a
        // malformed one. Every activated entry is proven colour-lane neutral
        // and font-inert BEFORE the walk, whose empty `ExtGState` environment
        // keeps a proven-neutral `gs` inert for the two-bit lane question.
        if !extgstate::proven_neutral_gs_activations(
            descent.input,
            descent.lookup,
            reached_offset,
            &records,
            decoded,
        ) {
            return Err(FormXObjectRefusalClass::ExtGStateAuthority);
        }
        let resources = if records
            .iter()
            .any(|record| color::is_color_space_operator(record, decoded))
        {
            if !has_canonical_form_resource_dictionary(
                descent.input,
                descent.lookup,
                reached_offset,
                b"ColorSpace",
            ) {
                return Err(FormXObjectRefusalClass::ColorAuthority);
            }
            color::build_device_projection(
                descent.input,
                descent.lookup,
                reached_offset,
                &records,
                decoded,
            )
            .ok_or(FormXObjectRefusalClass::ColorAuthority)?
        } else {
            Vec::new()
        };
        // Demand for the XObject authority is a syntactically valid `Do`; a
        // Form without one never inspects its own `/Resources /XObject`.
        let xobjects = records
            .iter()
            .any(|record| is_xobject_invoke_operator(record, decoded))
            .then(|| {
                FormLocalXObjectAuthority::from_form(descent.input, descent.lookup, reached_offset)
            });
        self.walk_lane_effect(
            descent,
            decoded,
            &records,
            ColorSpaceEnv::new(&resources),
            xobjects.as_ref(),
        )
    }

    /// Run the ONE seeded paint walk and accumulate the depth-indexed lattice
    /// the inherited lanes each operation consumes. The walker owns `q`/`Q`,
    /// direct setter lane kills, and finiteness; this only compares the live
    /// colour to the inherited sentinel. A walk error (the iterator fuses on
    /// first error) is an intrinsic refusal of every slot.
    ///
    /// `CS`/`cs` and `SC`/`SCN`/`sc`/`scn` are admitted only when the paint walk
    /// proves them safe over the Form-local device colour projection. `env` is
    /// that projection: a `CS`/`cs` naming a supported Device family selects it
    /// (ISO 32000-1 Table 74 sets the space AND its initial colour, so the
    /// selected inherited lane is killed with concrete local provenance). A
    /// named setter is admitted only over a lane already proven a supported
    /// local Device lane — never the inherited sentinel — with exact 1/3/4
    /// arity. Any other selection or setter shape refuses every slot.
    ///
    /// A `Do` is admitted only through `xobjects`, the Form-local `XObject`
    /// authority, and folded into the lattice by [`Self::fold_xobject_invoke`].
    fn walk_lane_effect(
        &mut self,
        descent: &mut FormDescent<'_>,
        decoded: &[u8],
        records: &[OperatorRecord],
        env: ColorSpaceEnv<'_>,
        xobjects: Option<&FormLocalXObjectAuthority>,
    ) -> Result<CachedFormAnalysis, FormXObjectRefusalClass> {
        let stroking_sentinel = inherited_sentinel(&[0.5, 0.25, 0.125, 0.0625]);
        let nonstroking_sentinel = inherited_sentinel(&[0.0625, 0.125, 0.25, 0.5]);
        let mut seed = GraphicsStateSnapshot::page_default();
        seed.stroking_color = stroking_sentinel.clone();
        seed.nonstroking_color = nonstroking_sentinel.clone();
        let seed = Rc::new(seed);
        let program = PaintProgram::new(decoded, records, env);
        let mut lattice = BoundedFormLaneEffects::neutral();
        // First-wins: the actionable cause of the FIRST nested-Form fold that
        // damages this walk's lattice (a genuine cycle, a horizon cutoff, or a
        // bubbled child class), in walk order. `None` while every fold so far
        // left the lattice fully proven.
        let mut fold_refusal: Option<FormXObjectRefusalClass> = None;
        let mut previous = Rc::clone(&seed);
        for op in program.ops_with_initial_state(seed) {
            let Ok(op) = op else {
                return Err(FormXObjectRefusalClass::RawGrammar);
            };
            let Some(record) = records.get(op.index) else {
                return Err(FormXObjectRefusalClass::RawGrammar);
            };
            let operator = decoded.get(record.operator.range.start..record.operator.range.end);
            // `PaintOp.state` is post-operator. A `CS`/`cs` selecting a
            // supported Device family stamps a concrete local colour, so the
            // post-state lane becomes a supported local Device lane; an
            // unresolved/unsupported name leaves a `Resource(name)` lane and
            // refuses the whole Form. A named setter's PRIOR lane must already
            // be a supported local Device lane (never the CMYK-shaped inherited
            // sentinel), and its arity must match that family exactly. Each
            // guard fires only on the refusal case.
            match operator {
                Some(b"CS")
                    if !color::valid_color_space_selection(
                        &op.state.stroking_color,
                        env,
                        record,
                        decoded,
                    ) =>
                {
                    return Err(FormXObjectRefusalClass::ColorAuthority);
                }
                Some(b"cs")
                    if !color::valid_color_space_selection(
                        &op.state.nonstroking_color,
                        env,
                        record,
                        decoded,
                    ) =>
                {
                    return Err(FormXObjectRefusalClass::ColorAuthority);
                }
                Some(b"SC" | b"SCN")
                    if !color::valid_named_setter(
                        &previous.stroking_color,
                        &op.state.stroking_color,
                        &stroking_sentinel,
                        record,
                    ) =>
                {
                    return Err(FormXObjectRefusalClass::ColorAuthority);
                }
                Some(b"sc" | b"scn")
                    if !color::valid_named_setter(
                        &previous.nonstroking_color,
                        &op.state.nonstroking_color,
                        &nonstroking_sentinel,
                        record,
                    ) =>
                {
                    return Err(FormXObjectRefusalClass::ColorAuthority);
                }
                _ => {}
            }
            if let PaintOpKind::XObjectInvoke { name } = &op.kind {
                // The walker is state-neutral for `XObjectInvoke` (per ISO
                // 32000-1 §8.10.1 an implicit save/restore brackets every
                // `Do`), so the post-operator `op.state` equals the
                // pre-invocation state the shipped stencil arm reads: each
                // lane's sentinel liveness at the `Do` is exact and shared by
                // every lattice slot.
                let lane_live = [
                    op.state.stroking_color == stroking_sentinel,
                    op.state.nonstroking_color == nonstroking_sentinel,
                ];
                let damage =
                    self.fold_xobject_invoke(descent, xobjects, &name.0, lane_live, &mut lattice)?;
                fold_refusal = fold_refusal.or(damage);
            }
            if let PaintOpKind::PathPaint { paint } = op.kind {
                if paint.uses_stroke() && op.state.stroking_color == stroking_sentinel {
                    lattice.mark_consumed(0);
                }
                if paint.uses_fill() && op.state.nonstroking_color == nonstroking_sentinel {
                    lattice.mark_consumed(1);
                }
            }
            previous = Rc::clone(&op.state);
        }
        Ok(CachedFormAnalysis {
            lattice,
            refusal_class: fold_refusal,
        })
    }

    /// Apply one raw `Do` operand to the lattice through the Form-local
    /// `XObject` authority: an ordinary Image consumes neither lane, a proven
    /// stencil consumes the inherited nonstroking lane exactly while it is
    /// still live at the invocation (a live local colour is consumed locally
    /// and never reaches the caller), and a retained nested ordinary Form is
    /// analyzed through the bounded recursion — sentinel-seeded like a root,
    /// cycle-refused, horizon-bounded, and charged once per first-seen target —
    /// then folded per slot at this site. Every other resolution is an
    /// intrinsic refusal (`Err`) of the whole Form.
    ///
    /// `Ok(None)` is a fold that left this walk's lattice fully proven so far
    /// (an ordinary Image, a stencil, or a clean nested-Form fold). `Ok(Some(_))`
    /// is a fold that damaged the lattice's maximum-depth slot: a genuine
    /// active-path cycle re-encounter, a traversal-horizon depth cutoff, or —
    /// bubbled from the child's own cached class — the child's actionable
    /// cause. The caller keeps only the FIRST such class in walk order.
    fn fold_xobject_invoke(
        &mut self,
        descent: &mut FormDescent<'_>,
        xobjects: Option<&FormLocalXObjectAuthority>,
        raw_name: &[u8],
        lane_live: [bool; 2],
        lattice: &mut BoundedFormLaneEffects,
    ) -> Result<Option<FormXObjectRefusalClass>, FormXObjectRefusalClass> {
        let Some((target, effect)) = xobjects.and_then(|authority| authority.resolve(raw_name))
        else {
            return Err(FormXObjectRefusalClass::XObjectAuthority);
        };
        match effect {
            PageXObjectEffect::OrdinaryImage => Ok(None),
            PageXObjectEffect::Stencil => {
                if lane_live[1] {
                    lattice.mark_consumed(1);
                }
                Ok(None)
            }
            PageXObjectEffect::Form => {
                let reference = target.reference;
                let reached_offset = target.object_byte_offset;
                if let Some(child) = self.analyze_nested(descent, reference, reached_offset) {
                    lattice.fold_nested_form(child.lattice, lane_live);
                    let damage = if lattice.max_depth_effect().is_some() {
                        None
                    } else {
                        child.refusal_class
                    };
                    Ok(damage)
                } else {
                    // Past the horizon with nothing cached: slot 0 is still
                    // computed (an invoked nested Form with no edge left is
                    // Unknown) and every deeper slot stays uncomputed for a
                    // shallower re-entry to fill in.
                    lattice.refuse_nested_descent();
                    Ok(Some(FormXObjectRefusalClass::RecursionDepth))
                }
            }
            _ => Err(FormXObjectRefusalClass::RawGrammar),
        }
    }
}

impl Default for FormXObjectEffectAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Re-resolve the reference through the current lookup and require an in-use,
/// source-addressable object whose generation and byte offset match the reached
/// target. Compressed, free, missing, ambiguous, or out-of-range targets fail.
fn corroborates(lookup: ObjectLookup<'_>, reference: IndirectRef, reached_offset: usize) -> bool {
    let Ok(object_number) = usize::try_from(reference.object_number) else {
        return false;
    };
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
        } => generation == reference.generation && byte_offset == reached_offset,
        _ => false,
    }
}

/// Decoded-name preflight for the exact Form's top-level stream dictionary.
///
/// `/F` substitutes external bytes for the local stream; `/Ref` may substitute
/// imported page content for the local proxy; `/OC` can suppress the entire
/// invocation; and `/OPI` supplies prepress proxy/substitute semantics. None is
/// inside this slice's admitted execution envelope, under any valid `#xx`
/// spelling. The safety keys whose value semantics remain delegated to existing
/// raw-key inspectors (`/Group`, `/Length`, `/Filter`, `/DecodeParms`) must be
/// canonical and semantically unique, otherwise an alias could evade the
/// delegated inspection. Any undecodable key also makes the dictionary Unknown.
fn semantic_dictionary_preflight(input: &[u8], reached_offset: usize) -> bool {
    let Ok(dictionary) = inspect_indirect_object_dictionary(input, reached_offset) else {
        return false;
    };
    let mut delegated_seen = [false; 4];
    for entry in &dictionary.entries {
        let Some(raw_key) = input.get(entry.key_range.start..entry.key_range.end) else {
            return false;
        };
        let Some(raw_name) = raw_key.strip_prefix(b"/") else {
            return false;
        };
        let Some(decoded) = decode_pdf_name(raw_name) else {
            return false;
        };
        let delegated_key = match decoded.as_ref() {
            b"F" | b"Ref" | b"OC" | b"OPI" => return false,
            b"Group" => Some((&b"/Group"[..], 0)),
            b"Length" => Some((&b"/Length"[..], 1)),
            b"Filter" => Some((&b"/Filter"[..], 2)),
            b"DecodeParms" => Some((&b"/DecodeParms"[..], 3)),
            _ => None,
        };
        if let Some((canonical, index)) = delegated_key {
            if raw_key != canonical || delegated_seen[index] {
                return false;
            }
            delegated_seen[index] = true;
        }
    }
    true
}

/// Strict CLOSED raw-record preflight: the allowlisted operator grammar plus the
/// path/`q`-stack context machine (ISO 32000-1 §8.2). Every other operator,
/// arity, non-number operand, or invalid ordering refuses. The walker validates
/// finiteness of the operators it models; this pass parses EVERY numeric operand
/// as a finite `f64` and is the ONLY validator for the path-construction/
/// clipping operators the walker treats as no-ops.
fn raw_preflight_ok(tokens: &[Token], records: &[OperatorRecord], source: &[u8]) -> bool {
    let mut path_open = false;
    let mut q_depth: usize = 0;
    for record in records {
        let Some(operator) = source.get(record.operator.range.start..record.operator.range.end)
        else {
            return false;
        };
        match operator {
            // Graphics stack: page-description level only (no open path), zero
            // operands, no underflow.
            b"q" => {
                if path_open || !record.operands.is_empty() {
                    return false;
                }
                q_depth += 1;
            }
            b"Q" => {
                if path_open || !record.operands.is_empty() {
                    return false;
                }
                let Some(depth) = q_depth.checked_sub(1) else {
                    return false;
                };
                q_depth = depth;
            }
            // Geometry-only CTM: six numerics validated by the walker.
            b"cm" => {
                if path_open || !numeric_arity(tokens, record, source, 6) {
                    return false;
                }
            }
            // Direct device lane overwrite: no open path, exact numeric arity.
            b"G" | b"g" => {
                if path_open || !numeric_arity(tokens, record, source, 1) {
                    return false;
                }
            }
            b"RG" | b"rg" => {
                if path_open || !numeric_arity(tokens, record, source, 3) {
                    return false;
                }
            }
            b"K" | b"k" => {
                if path_open || !numeric_arity(tokens, record, source, 4) {
                    return false;
                }
            }
            // Path construction that opens or continues a path.
            b"m" => {
                if !numeric_arity(tokens, record, source, 2) {
                    return false;
                }
                path_open = true;
            }
            b"re" => {
                if !numeric_arity(tokens, record, source, 4) {
                    return false;
                }
                path_open = true;
            }
            // Path continuation and clipping require an open path.
            b"l" => {
                if !path_open || !numeric_arity(tokens, record, source, 2) {
                    return false;
                }
            }
            b"c" => {
                if !path_open || !numeric_arity(tokens, record, source, 6) {
                    return false;
                }
            }
            b"v" | b"y" => {
                if !path_open || !numeric_arity(tokens, record, source, 4) {
                    return false;
                }
            }
            b"h" | b"W" | b"W*" => {
                if !path_open || !record.operands.is_empty() {
                    return false;
                }
            }
            // Path paint/end: require an open path, close it, zero operands.
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                if !path_open || !record.operands.is_empty() {
                    return false;
                }
                path_open = false;
            }
            // Form-local resource colour-space selection (`CS`/`cs`),
            // external-object invocation (`Do`), and extended-graphics-state
            // activation (`gs`): no open path, exactly one syntactically valid
            // name operand. Whether the name resolves to a supported Device
            // family (selection), an admissible Form-local ordinary
            // Image/stencil/Form (invocation), or a proven-neutral `ExtGState`
            // entry (activation) is proven later by the decoded-name
            // projection/authority/gate; the raw pass validates syntax only.
            b"CS" | b"cs" | b"Do" | b"gs" => {
                if path_open || !single_name_operand(tokens, record) {
                    return false;
                }
            }
            // Named colour setter: no open path, at least one finite numeric
            // operand and no trailing Pattern name. Exact 1/3/4 arity over a
            // proven local Device lane is validated by the seeded walk.
            b"SC" | b"SCN" | b"sc" | b"scn" => {
                if path_open || !finite_numeric_setter(tokens, record, source) {
                    return false;
                }
            }
            // Every other operator — line/text state, `sh`, inline images,
            // `BX/EX`, marked content, `d0/d1`, and unknown extensions —
            // refuses. A positive prefix never survives.
            _ => return false,
        }
    }
    !path_open && q_depth == 0
}

/// Whether an operator record carries exactly `expected` operands, each a single
/// numeric lexeme.
fn numeric_arity(
    tokens: &[Token],
    record: &OperatorRecord,
    source: &[u8],
    expected: usize,
) -> bool {
    record.operands.len() == expected
        && record
            .operands
            .iter()
            .all(|operand| is_finite_number_operand(tokens, operand, source))
}

/// Whether one operand is a single numeric token whose exact decimal lexeme
/// parses to a finite `f64`. In particular, overflowing decimal lexemes refuse
/// even for path operators the paint walker otherwise treats as no-ops.
fn is_finite_number_operand(tokens: &[Token], operand: &OperandRecord, source: &[u8]) -> bool {
    let [token_ref] = operand.tokens.as_slice() else {
        return false;
    };
    let Some(token) = tokens.get(token_ref.token_index) else {
        return false;
    };
    if !matches!(token.kind, TokenKind::Number(_)) {
        return false;
    }
    let Some(bytes) = token.source_bytes(source) else {
        return false;
    };
    let Ok(text) = core::str::from_utf8(bytes) else {
        return false;
    };
    text.parse::<f64>().is_ok_and(f64::is_finite)
}

/// Whether a `CS`/`cs`/`Do`/`gs` record carries exactly one operand that is a
/// single PDF name lexeme. Semantic resolution of that name happens later,
/// never here.
fn single_name_operand(tokens: &[Token], record: &OperatorRecord) -> bool {
    let [operand] = record.operands.as_slice() else {
        return false;
    };
    let [token_ref] = operand.tokens.as_slice() else {
        return false;
    };
    tokens
        .get(token_ref.token_index)
        .is_some_and(|token| matches!(token.kind, TokenKind::Name))
}

/// Whether an `SC`/`SCN`/`sc`/`scn` record carries at least one operand, every
/// one a single finite numeric lexeme with no trailing Pattern name. The exact
/// 1/3/4 arity is proven against the prior lane by the seeded walk.
fn finite_numeric_setter(tokens: &[Token], record: &OperatorRecord, source: &[u8]) -> bool {
    !record.operands.is_empty()
        && record
            .operands
            .iter()
            .all(|operand| is_finite_number_operand(tokens, operand, source))
}

/// Whether one record's operator token is `Do`.
fn is_xobject_invoke_operator(record: &OperatorRecord, source: &[u8]) -> bool {
    matches!(
        source.get(record.operator.range.start..record.operator.range.end),
        Some(b"Do")
    )
}

/// Resolve the analyzed Form's own `/Resources` dictionary entries under exact
/// canonical authority.
///
/// `Ok(None)` is proven absence of any `/Resources` key. `Ok(Some(entries))`
/// is the single canonical dictionary's shallow entries, resolved through one
/// exact source-addressable in-use indirect target when referenced. `Err(())`
/// is escaped, duplicated, malformed, unresolved, compressed, or otherwise
/// ambiguous authority and must fail closed. Page/caller fallback is never
/// consulted and scopes are never merged.
fn canonical_form_resources_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reached_offset: usize,
) -> Result<Option<Vec<DictionaryEntrySpan>>, ()> {
    let Ok(form) = inspect_indirect_object_dictionary(input, reached_offset) else {
        return Err(());
    };
    let Some(resources) = canonical_unique_authority_entry(input, &form.entries, b"Resources")?
    else {
        return Ok(None);
    };
    match resources.value_kind {
        DictionaryValueKind::Dictionary => {
            inspect_dictionary_entries(input, resources.value_range.start)
                .map(|dictionary| Some(dictionary.entries))
                .map_err(|_| ())
        }
        DictionaryValueKind::IndirectReferenceLike => {
            let reference = parse_indirect_reference(input, resources.value_range.start)
                .map_err(|_| ())?
                .reference;
            let object_number = usize::try_from(reference.object_number).map_err(|_| ())?;
            let object_offset = match locate_xref_object(lookup, object_number) {
                ObjectLookupLocation::ClassicInUse {
                    generation,
                    byte_offset,
                    ..
                }
                | ObjectLookupLocation::XrefStreamUncompressed {
                    generation,
                    byte_offset,
                    ..
                } if generation == reference.generation => byte_offset,
                _ => return Err(()),
            };
            let dictionary =
                inspect_indirect_object_dictionary(input, object_offset).map_err(|_| ())?;
            if dictionary.reference == reference {
                Ok(Some(dictionary.entries))
            } else {
                Err(())
            }
        }
        _ => Err(()),
    }
}

/// Require a canonical, semantically unique direct dictionary for one named
/// entry in the Form's own canonical `/Resources`. Missing `/Resources` or a
/// missing named entry is exact absence; every ambiguous or non-dictionary
/// shape fails closed.
fn has_canonical_form_resource_dictionary(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reached_offset: usize,
    semantic_name: &[u8],
) -> bool {
    let entries = match canonical_form_resources_entries(input, lookup, reached_offset) {
        Ok(None) => return true,
        Ok(Some(entries)) => entries,
        Err(()) => return false,
    };
    let Ok(resource) = canonical_unique_authority_entry(input, &entries, semantic_name) else {
        return false;
    };
    let Some(resource) = resource else {
        return true;
    };
    resource.value_kind == DictionaryValueKind::Dictionary
        && inspect_dictionary_entries(input, resource.value_range.start).is_ok()
}

/// Find one fixed authority key by decoded equality while requiring its raw
/// spelling to be canonical and unique. An undecodable peer key makes the
/// authority dictionary ambiguous; unrelated valid keys remain isolated.
fn canonical_unique_authority_entry(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
    semantic_name: &[u8],
) -> Result<Option<DictionaryEntrySpan>, ()> {
    let mut found = None;
    for entry in entries {
        let raw_key = input
            .get(entry.key_range.start..entry.key_range.end)
            .ok_or(())?;
        let raw_name = raw_key.strip_prefix(b"/").ok_or(())?;
        let decoded = match decode_pdf_name(raw_name) {
            Some(decoded) => decoded,
            None if malformed_name_may_hide(raw_name, semantic_name) => return Err(()),
            None => continue,
        };
        if decoded.as_ref() != semantic_name {
            continue;
        }
        if raw_name != semantic_name || found.is_some() {
            return Err(());
        }
        found = Some(*entry);
    }
    Ok(found)
}

/// Whether the valid decoded prefix before the first malformed escape matches a
/// prefix of `candidate`. The caller invokes this only after full decoding has
/// failed, so reaching the malformed escape establishes uncertainty.
fn malformed_name_may_hide(raw: &[u8], candidate: &[u8]) -> bool {
    let mut raw_index = 0;
    let mut decoded_index = 0;
    while raw_index < raw.len() {
        let byte = if raw[raw_index] == b'#' {
            let Some(high) = raw
                .get(raw_index + 1)
                .and_then(|byte| local_hex_digit(*byte))
            else {
                return true;
            };
            let Some(low) = raw
                .get(raw_index + 2)
                .and_then(|byte| local_hex_digit(*byte))
            else {
                return true;
            };
            let decoded = high * 16 + low;
            if decoded == 0 {
                return true;
            }
            raw_index += 3;
            decoded
        } else {
            let decoded = raw[raw_index];
            raw_index += 1;
            decoded
        };
        if candidate.get(decoded_index) != Some(&byte) {
            return false;
        }
        decoded_index += 1;
    }
    false
}

const fn local_hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// One distinct, source-less inherited-lane sentinel. `source: None` is the true
/// discriminator: every valid direct setter stamps a concrete source range, so
/// no setter can recreate a sentinel even with identical numeric components.
fn inherited_sentinel(components: &[f64]) -> GraphicsColor {
    GraphicsColor {
        space: ColorSpace::DeviceCmyk,
        components: components.to_vec(),
        resource_name: None,
        spot_name: None,
        spot_names: Vec::new(),
        source: None,
    }
}
