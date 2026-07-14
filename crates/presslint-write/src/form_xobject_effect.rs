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
//! This makes only a colour-lane-dependency claim. `/Matrix` and `/BBox` are
//! unmodelled: a non-identity `/Matrix` cannot change which colour lane a path
//! paint reads, and `/BBox` clipping can only suppress paint, so ignoring both
//! may at worst over-report consumption (a conservative positive) and can never
//! fabricate an unsafe false Neutral. No Form byte is read as mutation authority,
//! no CTM/bounds/visibility is published, and every group, resource colour,
//! text, nested `Do`, image, shading, `gs`, and unknown construct refuses.

use std::collections::BTreeMap;
use std::rc::Rc;

use presslint_paint::{
    ColorSpaceEnv, GraphicsColor, GraphicsStateSnapshot, PaintOpKind, PaintProgram,
};
use presslint_pdf::{
    FlateDecodeParameters, IndirectRef, ObjectLookup, ObjectLookupLocation,
    content_stream_data_slice, decode_flate_stream, inspect_content_stream_data_extent_with_lookup,
    inspect_form_transparency_group, inspect_indirect_object_dictionary, locate_xref_object,
};
use presslint_syntax::{
    OperandRecord, OperatorRecord, Token, TokenKind, assemble_operators, tokenize,
};
use presslint_types::ColorSpace;

use crate::{
    content_edit_pipeline::{MAX_CONTENT_STREAM_BYTES, PipelineFilterKind, classify_filter},
    page_xobject_policy::decode_pdf_name,
};

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
                analyze_bytes(slice)
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
                analyze_bytes(&decoded)
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
/// The decoded bytes, tokens, records, and walk state are all dropped on return.
fn analyze_bytes(decoded: &[u8]) -> Option<FormLaneEffect> {
    let tokens = tokenize(decoded).ok()?;
    let records = assemble_operators(&tokens).ok()?.records;
    if !raw_preflight_ok(&tokens, &records, decoded) {
        return None;
    }
    walk_lane_effect(decoded, &records)
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
            // Every other operator — resource colours, line/text state, `gs`,
            // `Do`, `sh`, inline images, `BX/EX`, marked content, `d0/d1`, and
            // unknown extensions — refuses. A positive prefix never survives.
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

/// Run the ONE seeded paint walk and read the inherited lanes each path paint
/// consumes. The walker owns `q`/`Q`, direct setter lane kills, and finiteness;
/// this only compares the live colour to the inherited sentinel. A walk error
/// (the iterator fuses on first error) is Unknown.
fn walk_lane_effect(decoded: &[u8], records: &[OperatorRecord]) -> Option<FormLaneEffect> {
    let stroking_sentinel = inherited_sentinel(&[0.5, 0.25, 0.125, 0.0625]);
    let nonstroking_sentinel = inherited_sentinel(&[0.0625, 0.125, 0.25, 0.5]);
    let mut seed = GraphicsStateSnapshot::page_default();
    seed.stroking_color = stroking_sentinel.clone();
    seed.nonstroking_color = nonstroking_sentinel.clone();
    let program = PaintProgram::new(decoded, records, ColorSpaceEnv::empty());
    let mut effect: FormLaneEffect = [false, false];
    for op in program.ops_with_initial_state(Rc::new(seed)) {
        let op = op.ok()?;
        if let PaintOpKind::PathPaint { paint } = op.kind {
            if paint.uses_stroke() && op.state.stroking_color == stroking_sentinel {
                effect[0] = true;
            }
            if paint.uses_fill() && op.state.nonstroking_color == nonstroking_sentinel {
                effect[1] = true;
            }
        }
    }
    Some(effect)
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
