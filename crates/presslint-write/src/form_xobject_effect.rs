//! Request-scoped exact-identity analysis of root-page Form `XObject`
//! inherited-colour effects.
//!
//! [`FormXObjectEffectAnalyzer`] is the ONE new abstraction this slice adds. It
//! answers exactly one read-only question about a root-page Form demanded by a
//! `Do`: does painting inside the Form consume the CALLER's inherited stroking
//! and/or nonstroking colour? It never mutates, clones, stages, or descends into
//! a Form; it reads the exact demanded stream once, proves a narrow structural
//! shape, and summarizes a two-bit lane effect.
//!
//! The analyzer is request-scoped and shared across every selected page. It
//! caches both positive and refused (Unknown) results keyed by the exact tuple
//! `(object number, generation, reached object byte offset)`, so the same Form
//! reached under multiple resource names, repeated `Do` invocations, or across
//! selected pages is analyzed at most once. Map presence distinguishes a cached
//! Unknown from an unseen target. A fixed first-seen target cap and one
//! aggregate decoded-byte budget bound the whole request; exhaustion
//! deterministically caches Unknown for further unseen targets.
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
//! unique `/Subtype /Form` proof before their tuple is retained for a later
//! recursion slice, and still refuse when invoked; every uncertain name or
//! target refuses the whole Form.
//!
//! This makes only a colour-lane-dependency claim. `/Matrix` and `/BBox` are
//! unmodelled: a non-identity `/Matrix` cannot change which colour lane a paint
//! reads, and `/BBox` clipping can only suppress paint, so ignoring both
//! may at worst over-report consumption (a conservative positive) and can never
//! fabricate an unsafe false Neutral. No Form byte is read as mutation authority,
//! no CTM/bounds/visibility is published, and every group, uncertain resource
//! colour, text, nested Form invocation, shading, `gs`, inline image, and
//! unknown construct refuses.

mod color;
mod xobjects;

use std::collections::BTreeMap;
use std::rc::Rc;

use presslint_paint::{
    ColorSpaceEnv, GraphicsColor, GraphicsStateSnapshot, PaintOpKind, PaintProgram,
};
use presslint_pdf::{
    DictionaryEntrySpan, DictionaryValueKind, FlateDecodeParameters, IndirectRef, ObjectLookup,
    ObjectLookupLocation, content_stream_data_slice, decode_flate_stream,
    inspect_content_stream_data_extent_with_lookup, inspect_dictionary_entries,
    inspect_form_transparency_group, inspect_indirect_object_dictionary, locate_xref_object,
    parse_indirect_reference,
};
use presslint_syntax::{
    OperandRecord, OperatorRecord, Token, TokenKind, assemble_operators, tokenize,
};
use presslint_types::ColorSpace;

use crate::{
    content_edit_pipeline::{MAX_CONTENT_STREAM_BYTES, PipelineFilterKind, classify_filter},
    page_xobject_policy::{PageXObjectEffect, decode_pdf_name},
};

use xobjects::FormLocalXObjectAuthority;

#[cfg(test)]
pub use xobjects::xobject_target_identity_corroborates_for_test;

/// Fixed per-request cap on first-seen exact Form targets. Further unseen
/// identities after this many attempts are deterministically Unknown.
const MAX_FORM_TARGETS: usize = 256;

/// Inherited stroking/nonstroking lane effect of one analyzed root Form:
/// `[consumes_inherited_stroking, consumes_inherited_nonstroking]`, in the same
/// stroking-first lane order the alias planner uses.
pub type FormLaneEffect = [bool; 2];

/// Request-scoped exact-identity cache and bounds for root Form colour-effect
/// analysis. Conceptually owns a `(reference, reached_offset) -> Option<effect>`
/// map (map presence = seen; `None` value = cached Unknown), a remaining
/// first-seen target count, and one aggregate remaining decoded-byte budget.
pub struct FormXObjectEffectAnalyzer {
    cache: BTreeMap<(IndirectRef, usize), Option<FormLaneEffect>>,
    remaining_targets: usize,
    remaining_bytes: usize,
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
        }
    }

    /// Summarize the inherited-colour lane effect of one demanded root Form
    /// target reached at `reached_offset` under `reference`.
    ///
    /// Returns `Some([stroking, nonstroking])` for a proven effect (a neutral
    /// Form is `Some([false, false])`) or `None` for Unknown (unaddressable,
    /// grouped, malformed, out-of-budget, or outside the raw allowlist). A cache
    /// hit is O(log F) and recharges neither the target count nor the byte
    /// budget.
    pub fn analyze(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        reference: IndirectRef,
        reached_offset: usize,
    ) -> Option<FormLaneEffect> {
        let key = (reference, reached_offset);
        if let Some(cached) = self.cache.get(&key) {
            return *cached;
        }
        let result = self.compute(input, lookup, reference, reached_offset);
        self.cache.insert(key, result);
        result
    }

    /// Compute one unseen exact identity's effect, charging one target attempt
    /// and (for a successfully sliced/decoded body) its bounded byte cost.
    fn compute(
        &mut self,
        input: &[u8],
        lookup: ObjectLookup<'_>,
        reference: IndirectRef,
        reached_offset: usize,
    ) -> Option<FormLaneEffect> {
        if self.remaining_targets == 0 {
            return None;
        }
        self.remaining_targets -= 1;
        // Aggregate budget exhaustion makes every remaining unseen target
        // Unknown, including a zero-byte Form; cached identities remain usable.
        if self.remaining_bytes == 0 {
            return None;
        }
        if !corroborates(lookup, reference, reached_offset) {
            return None;
        }
        // The raw-key extent/filter/group helpers are safe only after every
        // top-level key has been decoded and their delegated spellings are
        // proved canonical and unique. This also refuses stream substitution,
        // reference XObjects, optional content and OPI before any body access.
        if !semantic_dictionary_preflight(input, reached_offset) {
            return None;
        }
        // Accept only raw/uncompressed or a single default-predictor
        // `/FlateDecode`; every other filter/chain/predictor is Unknown.
        let filter = classify_filter(input, reached_offset).ok()?;
        let extent =
            inspect_content_stream_data_extent_with_lookup(input, Some(lookup), reached_offset)
                .ok()?;
        // Prove the Form declares no transparency group before any body walk;
        // any valid, malformed, indirect-unresolved, or uncertain group is
        // Unknown.
        let group = inspect_form_transparency_group(input, lookup, reached_offset);
        if group.group.is_some() || !group.skipped.is_empty() {
            return None;
        }
        // Borrow raw bytes directly; bound a Flate decode by the remaining
        // aggregate budget and drop the owned buffer once the two-bit effect is
        // read.
        match filter {
            PipelineFilterKind::Raw => {
                if extent.length() > self.remaining_bytes {
                    return None;
                }
                let slice = content_stream_data_slice(input, &extent).ok()?;
                self.remaining_bytes -= slice.len();
                analyze_bytes(input, lookup, reached_offset, slice)
            }
            PipelineFilterKind::Flate => {
                let slice = content_stream_data_slice(input, &extent).ok()?;
                let decoded = decode_flate_stream(
                    slice,
                    FlateDecodeParameters::default(),
                    self.remaining_bytes,
                )
                .ok()?;
                self.remaining_bytes -= decoded.len();
                analyze_bytes(input, lookup, reached_offset, &decoded)
            }
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

/// Tokenize/assemble the decoded Form once, run the closed raw preflight, then
/// the ONE seeded paint walk. Any parse, preflight, or walk failure is Unknown.
/// The decoded bytes, tokens, records, authority, and walk state are all
/// dropped on return.
///
/// The Form-local device colour projection is built only when a `CS`/`cs`
/// resource colour operator is present, and the Form-local `XObject` authority
/// is built only when a syntactically valid `Do` is present; otherwise the walk
/// runs with an empty [`ColorSpaceEnv`] and no authority, byte-for-byte
/// reproducing the resource-independent T187 result. The projection borrows
/// `resources`, which must outlive the walk.
fn analyze_bytes(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reached_offset: usize,
    decoded: &[u8],
) -> Option<FormLaneEffect> {
    let tokens = tokenize(decoded).ok()?;
    let records = assemble_operators(&tokens).ok()?.records;
    if !raw_preflight_ok(&tokens, &records, decoded) {
        return None;
    }
    let resources = if records
        .iter()
        .any(|record| color::is_color_space_operator(record, decoded))
    {
        if !has_canonical_form_resource_dictionary(input, lookup, reached_offset, b"ColorSpace") {
            return None;
        }
        color::build_device_projection(input, lookup, reached_offset, &records, decoded)?
    } else {
        Vec::new()
    };
    // Demand for the XObject authority is a syntactically valid `Do`; a Form
    // without one never inspects its own `/Resources /XObject`.
    let xobjects = records
        .iter()
        .any(|record| is_xobject_invoke_operator(record, decoded))
        .then(|| FormLocalXObjectAuthority::from_form(input, lookup, reached_offset));
    walk_lane_effect(
        decoded,
        &records,
        ColorSpaceEnv::new(&resources),
        xobjects.as_ref(),
    )
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
            // Form-local resource colour-space selection (`CS`/`cs`) and
            // external-object invocation (`Do`): no open path, exactly one
            // syntactically valid name operand. Whether the name resolves to a
            // supported Device family (selection) or an admissible Form-local
            // ordinary Image/stencil (invocation) is proven later by the
            // decoded-name projection/authority at the seeded walk; the raw
            // pass validates syntax only.
            b"CS" | b"cs" | b"Do" => {
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
            // Every other operator — line/text state, `gs`, `sh`, inline
            // images, `BX/EX`, marked content, `d0/d1`, and unknown extensions —
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

/// Whether a `CS`/`cs`/`Do` record carries exactly one operand that is a single
/// PDF name lexeme. Semantic resolution of that name happens later, never here.
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

/// Run the ONE seeded paint walk and read the inherited lanes each path paint
/// consumes. The walker owns `q`/`Q`, direct setter lane kills, and finiteness;
/// this only compares the live colour to the inherited sentinel. A walk error
/// (the iterator fuses on first error) is Unknown.
///
/// `CS`/`cs` and `SC`/`SCN`/`sc`/`scn` are admitted only when the paint walk
/// proves them safe over the Form-local device colour projection. `env` is that
/// projection: a `CS`/`cs` naming a supported Device family selects it (ISO
/// 32000-1 Table 74 sets the space AND its initial colour, so the selected
/// inherited lane is killed with concrete local provenance). A named setter is
/// admitted only over a lane already proven a supported local Device lane —
/// never the inherited sentinel — with exact 1/3/4 arity. Any other selection
/// or setter shape is Unknown.
///
/// A `Do` is admitted only through `xobjects`, the Form-local `XObject`
/// authority: an ordinary Image consumes neither lane, and a proven stencil
/// consumes the inherited nonstroking lane exactly when the live lane still
/// equals its sentinel (a live local colour is consumed locally and never
/// reaches the caller). Every other resolution refuses the whole Form.
fn walk_lane_effect(
    decoded: &[u8],
    records: &[OperatorRecord],
    env: ColorSpaceEnv<'_>,
    xobjects: Option<&FormLocalXObjectAuthority>,
) -> Option<FormLaneEffect> {
    let stroking_sentinel = inherited_sentinel(&[0.5, 0.25, 0.125, 0.0625]);
    let nonstroking_sentinel = inherited_sentinel(&[0.0625, 0.125, 0.25, 0.5]);
    let mut seed = GraphicsStateSnapshot::page_default();
    seed.stroking_color = stroking_sentinel.clone();
    seed.nonstroking_color = nonstroking_sentinel.clone();
    let seed = Rc::new(seed);
    let program = PaintProgram::new(decoded, records, env);
    let mut effect: FormLaneEffect = [false, false];
    let mut previous = Rc::clone(&seed);
    for op in program.ops_with_initial_state(seed) {
        let op = op.ok()?;
        let record = records.get(op.index)?;
        let operator = decoded.get(record.operator.range.start..record.operator.range.end);
        // `PaintOp.state` is post-operator. A `CS`/`cs` selecting a supported
        // Device family stamps a concrete local colour, so the post-state lane
        // becomes a supported local Device lane; an unresolved/unsupported name
        // leaves a `Resource(name)` lane and refuses the whole Form. A named
        // setter's PRIOR lane must already be a supported local Device lane
        // (never the CMYK-shaped inherited sentinel), and its arity must match
        // that family exactly. Each guard fires only on the refusal case.
        match operator {
            Some(b"CS")
                if !color::valid_color_space_selection(
                    &op.state.stroking_color,
                    env,
                    record,
                    decoded,
                ) =>
            {
                return None;
            }
            Some(b"cs")
                if !color::valid_color_space_selection(
                    &op.state.nonstroking_color,
                    env,
                    record,
                    decoded,
                ) =>
            {
                return None;
            }
            Some(b"SC" | b"SCN")
                if !color::valid_named_setter(
                    &previous.stroking_color,
                    &op.state.stroking_color,
                    &stroking_sentinel,
                    record,
                ) =>
            {
                return None;
            }
            Some(b"sc" | b"scn")
                if !color::valid_named_setter(
                    &previous.nonstroking_color,
                    &op.state.nonstroking_color,
                    &nonstroking_sentinel,
                    record,
                ) =>
            {
                return None;
            }
            _ => {}
        }
        // A `Do` reaching this point passed the raw grammar; its lane effect is
        // decided solely by the Form-local `XObject` authority. Painting a
        // stencil reads the CURRENT nonstroking colour (§8.9.6.2): under the
        // still-live sentinel it consumes the inherited lane, while under a
        // proven local colour it consumes that local colour and reaches no
        // caller lane. Anything else — nested Form, poisoned, skipped, or
        // unresolved name — refuses.
        if let PaintOpKind::XObjectInvoke { name } = &op.kind {
            match xobjects.and_then(|authority| authority.resolve(&name.0)) {
                Some(PageXObjectEffect::OrdinaryImage) => {}
                Some(PageXObjectEffect::Stencil) => {
                    if op.state.nonstroking_color == nonstroking_sentinel {
                        effect[1] = true;
                    }
                }
                _ => return None,
            }
        }
        if let PaintOpKind::PathPaint { paint } = op.kind {
            if paint.uses_stroke() && op.state.stroking_color == stroking_sentinel {
                effect[0] = true;
            }
            if paint.uses_fill() && op.state.nonstroking_color == nonstroking_sentinel {
                effect[1] = true;
            }
        }
        previous = Rc::clone(&op.state);
    }
    Some(effect)
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
