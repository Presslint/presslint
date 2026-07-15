//! Bounded recursive nested ordinary Form colour-effect analysis (T190).
//!
//! These matrices exercise the depth-indexed lattice recursion the analyzer
//! gained: per-slot composition of child lane effects at the parent `Do`
//! (sentinel-liveness fold, state independence, stencil/image/local-colour
//! mixes), the `IndirectRef`-keyed active-path cycle refusal, DAG/alias
//! charge-once reuse, the exact eight-edge depth boundary with cache-order
//! independence and the nine-frame traversal horizon (deeper chains charge
//! exactly nine frames; horizon-cut lattices deepen purely on a shallower
//! re-query), root-verbatim child admission (including proven-absent
//! `/Resources`), tight first-seen-target and successful/failed-attempt byte
//! budget accounting across the descent, and the end-to-end page-only
//! transaction boundary. Analyzer UNIT tests call
//! [`FormXObjectEffectAnalyzer::analyze`] on real Forms reached through the
//! request `ObjectLookup`; END-TO-END tests drive
//! `convert_content_colors_incremental`.

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    DeviceLinkInput, PageSelection, convert_content_colors_incremental,
    form_xobject_effect::FormXObjectEffectAnalyzer,
};
use presslint_pdf::{
    DocumentAccessBackend, IndirectRef, ObjectLookup, ObjectLookupLocation, encode_flate_stream,
    inspect_document_access, locate_xref_object,
};

use super::content_color_convert::{
    GRAY_TO_GRAY_LINK, assemble_classic, contains, link_bytes, occurrence_count,
    page_decoded_stream, stream_body,
};
use super::reopen;

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
const FORM_BASE: &str = " /Type /XObject /Subtype /Form /BBox [0 0 100 100]";
const FLATE_LIMIT: usize = 1 << 20;

// --- Analyzer unit harness ---------------------------------------------------

fn backend_lookup(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
    match backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    }
}

fn object_offset(lookup: ObjectLookup<'_>, object_number: u32) -> usize {
    let key = usize::try_from(object_number).expect("object number fits usize");
    match locate_xref_object(lookup, key) {
        ObjectLookupLocation::ClassicInUse { byte_offset, .. }
        | ObjectLookupLocation::XrefStreamUncompressed { byte_offset, .. } => byte_offset,
        other => panic!("object {object_number} not addressable: {other:?}"),
    }
}

/// One-page classic PDF whose objects 5.. are the supplied bodies.
fn form_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
    let mut bodies = vec![
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream_body("", b""),
    ];
    bodies.extend_from_slice(objects);
    assemble_classic(&bodies)
}

/// A raw form stream with an extra dictionary fragment and a content body.
fn form(dict_extra: &str, content: &[u8]) -> Vec<u8> {
    stream_body(&format!("{FORM_BASE}{dict_extra}"), content)
}

/// A raw form declaring a Form-local `/Resources /XObject` sub-dictionary.
fn xform(xobjects: &str, content: &[u8]) -> Vec<u8> {
    form(
        &format!(" /Resources << /XObject << {xobjects} >> >>"),
        content,
    )
}

/// An ordinary gray image `XObject`.
fn image_object() -> Vec<u8> {
    stream_body(
        " /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray",
        b"\x00",
    )
}

/// A structurally valid stencil-mask `XObject`.
fn stencil_object() -> Vec<u8> {
    stream_body(
        " /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 1 /ImageMask true",
        b"\x00",
    )
}

/// Analyze object 5 (the first supplied body) in a fresh request analyzer.
fn analyze(objects: &[Vec<u8>]) -> Option<[bool; 2]> {
    let input = form_pdf(objects);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset = object_offset(lookup, 5);
    FormXObjectEffectAnalyzer::new().analyze(
        &input,
        lookup,
        IndirectRef {
            object_number: 5,
            generation: 0,
        },
        offset,
    )
}

/// Analyze one object number of an already-open document in `analyzer`.
fn analyze_object(
    analyzer: &mut FormXObjectEffectAnalyzer,
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_number: u32,
) -> Option<[bool; 2]> {
    analyzer.analyze(
        input,
        lookup,
        IndirectRef {
            object_number,
            generation: 0,
        },
        object_offset(lookup, object_number),
    )
}

/// A linear form chain of `edges` nested `Do` edges rooted at object 5: every
/// form invokes the next object, and the final form fills with the inherited
/// nonstroking lane.
fn chain(edges: usize) -> Vec<Vec<u8>> {
    let mut objects = Vec::new();
    for index in 0..edges {
        let next = 6 + index;
        objects.push(xform(&format!("/N {next} 0 R"), b"/N Do"));
    }
    objects.push(form("", b"0 0 m 1 1 l f"));
    objects
}

// --- Composition and state independence ------------------------------------

#[test]
fn a_nested_chain_composes_exact_per_lane_bits_across_three_levels() {
    // Root -> child -> grandchild; the grandchild strokes AND fills with the
    // inherited sentinels, and both bits survive two sentinel-live folds.
    let grandchild = form("", b"0 0 m 1 1 l B");
    assert_eq!(
        analyze(&[
            xform("/C 6 0 R", b"/C Do"),
            xform("/G 7 0 R", b"/G Do"),
            grandchild.clone(),
        ]),
        Some([true, true])
    );
    // A local root colour before the descent absorbs that lane locally: only
    // the still-live stroking lane reaches the caller.
    assert_eq!(
        analyze(&[
            xform("/C 6 0 R", b"0 0 1 rg /C Do"),
            xform("/G 7 0 R", b"/G Do"),
            grandchild,
        ]),
        Some([true, false])
    );
}

#[test]
fn one_cached_child_folds_independently_at_two_parent_sites() {
    // The same child is invoked twice under different parent lane states: the
    // first site folds both lanes, the second (after a local `rg`) only the
    // stroking lane. One target and one decode pay for the child; the tight
    // bounds prove the second site reuses the cached lattice.
    let root = b"/Fm Do 0 0 1 rg /Fm Do";
    let child = b"0 0 m 1 1 l B";
    let input = form_pdf(&[xform("/Fm 6 0 R", root), form("", child)]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(2, root.len() + child.len());
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 5),
        Some([true, true])
    );
    // The second-site fold respected the live local nonstroking colour: the
    // same child under a killed nonstroking lane consumes only stroking.
    assert_eq!(
        analyze(&[xform("/Fm 6 0 R", b"0 0 1 rg /Fm Do"), form("", child)]),
        Some([true, false])
    );
}

#[test]
fn child_colour_writes_do_not_escape_the_invocation() {
    // The child kills its nonstroking lane and fills locally (neutral), yet
    // the parent's OWN later fill still consumes the parent sentinel: the
    // implicit save/restore of ISO 32000-1 §8.10.1 keeps child writes inside
    // the `Do`.
    assert_eq!(
        analyze(&[
            xform("/Fm 6 0 R", b"/Fm Do 0 0 m 1 1 l f"),
            form("", b"0.5 g 0 0 m 1 1 l f"),
        ]),
        Some([false, true])
    );
    // Without the parent's own paint the neutral child leaves both lanes
    // untouched.
    assert_eq!(
        analyze(&[
            xform("/Fm 6 0 R", b"/Fm Do"),
            form("", b"0.5 g 0 0 m 1 1 l f"),
        ]),
        Some([false, false])
    );
}

#[test]
fn a_nested_stencil_propagates_only_while_the_parent_lane_is_live() {
    let child = xform("/St 7 0 R", b"/St Do");
    // The child's stencil consumption reaches the caller through the fold
    // while the parent's nonstroking lane still equals its sentinel...
    assert_eq!(
        analyze(&[xform("/C 6 0 R", b"/C Do"), child.clone(), stencil_object(),]),
        Some([false, true])
    );
    // ...and is absorbed by a live local parent colour at the `Do`.
    assert_eq!(
        analyze(&[xform("/C 6 0 R", b"0.5 g /C Do"), child, stencil_object(),]),
        Some([false, false])
    );
}

#[test]
fn a_mixed_child_composes_exact_bits_across_three_levels_with_raw_flate_parity() {
    // The child selects a local alias nonstroking colour (killing that lane),
    // invokes a neutral image, a stencil (consuming the LOCAL colour, not the
    // caller's), and a deeper form whose stroke consumes the still-inherited
    // stroking lane. Exactly [stroking, not nonstroking] reaches the root.
    let resources = " /Resources << /ColorSpace << /L /DeviceGray >> \
                     /XObject << /Im 7 0 R /St 8 0 R /F2 9 0 R >> >>";
    let content = b"/L cs 0.5 sc /Im Do /St Do /F2 Do";
    let siblings = [image_object(), stencil_object(), form("", b"0 0 m 1 1 l S")];
    let raw = [
        xform("/C 6 0 R", b"/C Do"),
        form(resources, content),
        siblings[0].clone(),
        siblings[1].clone(),
        siblings[2].clone(),
    ];
    assert_eq!(analyze(&raw), Some([true, false]));
    let compressed = encode_flate_stream(content, FLATE_LIMIT).expect("encode");
    let flate = [
        xform("/C 6 0 R", b"/C Do"),
        stream_body(
            &format!("{FORM_BASE}{resources} /Filter /FlateDecode"),
            &compressed,
        ),
        siblings[0].clone(),
        siblings[1].clone(),
        siblings[2].clone(),
    ];
    assert_eq!(analyze(&flate), Some([true, false]));
}

#[test]
fn a_positive_descendant_prefix_never_survives_an_unsupported_suffix() {
    // The grandchild consumes a lane, then hits unsupported grammar: every
    // affected parent slot is Unknown and the root refuses.
    assert_eq!(
        analyze(&[
            xform("/C 6 0 R", b"/C Do"),
            xform("/G 7 0 R", b"/G Do"),
            form("", b"0 0 m 1 1 l f BT ET"),
        ]),
        None
    );
    // The same unsupported suffix one level up refuses identically.
    assert_eq!(
        analyze(&[
            xform("/C 6 0 R", b"/C Do"),
            xform("/G 7 0 R", b"/G Do 0 0 m 1 1 l f BT ET"),
            form("", b"0 0 m 1 1 l f"),
        ]),
        None
    );
}

// --- Cycles, DAG reuse and depth ---------------------------------------------

#[test]
fn a_self_referential_form_refuses_without_recharging_the_re_encounter() {
    let cycle = b"/Me Do";
    let leaf = b"0 0 m 1 1 l f";
    let input = form_pdf(&[xform("/Me 5 0 R", cycle), form("", leaf)]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // Exactly one target and one decode afford the cycle form plus the leaf:
    // if the active-path re-encounter recharged either budget, the leaf below
    // could no longer prove.
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(2, cycle.len() + leaf.len());
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    // Deterministic when re-queried, served from the cache.
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 6),
        Some([false, true])
    );
}

#[test]
fn a_two_form_mutual_cycle_refuses_both_deterministically() {
    let a = b"/B Do";
    let b = b"/A Do";
    let leaf = b"0 0 m 1 1 l f";
    let input = form_pdf(&[xform("/B 6 0 R", a), xform("/A 5 0 R", b), form("", leaf)]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // Two targets and two decodes afford both cycle members; the third pair
    // affords the leaf only if the mutual re-encounter charged nothing.
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(3, a.len() + b.len() + leaf.len());
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 6), None);
    // Deterministic on re-query, in both orders.
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 6), None);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 7),
        Some([false, true])
    );
}

#[test]
fn diamond_reuse_analyzes_and_charges_the_shared_child_once() {
    let root = b"/A Do /B Do";
    let branch = b"/C Do";
    let shared = b"0 0 m 1 1 l f";
    let objects = [
        xform("/A 6 0 R /B 7 0 R", root),
        xform("/C 8 0 R", branch),
        xform("/C 8 0 R", branch),
        form("", shared),
    ];
    let input = form_pdf(&objects);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // Root, both branches, and ONE shared-child analysis: exactly four targets
    // and the exact byte sum prove C is charged once.
    let exact_bytes = root.len() + 2 * branch.len() + shared.len();
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(4, exact_bytes);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 5),
        Some([false, true])
    );
    // One byte short starves the LAST unique frame; the earlier branch stays
    // cached and proven while the root refuses.
    let mut starved = FormXObjectEffectAnalyzer::with_bounds(4, exact_bytes - 1);
    assert_eq!(analyze_object(&mut starved, &input, lookup, 5), None);
    assert_eq!(
        analyze_object(&mut starved, &input, lookup, 6),
        Some([false, true])
    );
}

#[test]
fn aliases_and_repeated_do_spend_one_child_target_and_decode() {
    let root = b"/A Do /B Do /A Do";
    let child = b"0 0 m 1 1 l f";
    let input = form_pdf(&[xform("/A 6 0 R /B 6 0 R", root), form("", child)]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // Two aliases and a repeated `Do` of one child: two targets and the exact
    // two-stream byte sum suffice.
    let exact_bytes = root.len() + child.len();
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(2, exact_bytes);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 5),
        Some([false, true])
    );
    // One target or one byte short refuses fail-closed.
    let mut short_targets = FormXObjectEffectAnalyzer::with_bounds(1, exact_bytes);
    assert_eq!(analyze_object(&mut short_targets, &input, lookup, 5), None);
    let mut short_bytes = FormXObjectEffectAnalyzer::with_bounds(2, exact_bytes - 1);
    assert_eq!(analyze_object(&mut short_bytes, &input, lookup, 5), None);
}

#[test]
fn the_depth_boundary_admits_eight_nested_edges_and_refuses_nine() {
    assert_eq!(analyze(&chain(8)), Some([false, true]));
    assert_eq!(analyze(&chain(9)), None);
}

#[test]
fn depth_refusals_are_cache_order_independent() {
    // In a nine-edge chain rooted at object 5, object 6 heads an eight-edge
    // chain: it proves as a root while the nine-edge parent refuses,
    // regardless of which is analyzed first.
    let input = form_pdf(&chain(9));
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);

    let mut descendant_first = FormXObjectEffectAnalyzer::new();
    assert_eq!(
        analyze_object(&mut descendant_first, &input, lookup, 6),
        Some([false, true])
    );
    assert_eq!(
        analyze_object(&mut descendant_first, &input, lookup, 5),
        None
    );

    let mut parent_first = FormXObjectEffectAnalyzer::new();
    assert_eq!(analyze_object(&mut parent_first, &input, lookup, 5), None);
    assert_eq!(
        analyze_object(&mut parent_first, &input, lookup, 6),
        Some([false, true])
    );
    // A mid-chain form queried directly as a root after being part of the
    // deeper refusal still proves from its own cached lattice.
    assert_eq!(
        analyze_object(&mut parent_first, &input, lookup, 10),
        Some([false, true])
    );
}

#[test]
fn a_deeper_than_nine_chain_charges_exactly_the_nine_frame_horizon() {
    // Twelve nested edges rooted at object 5: the traversal enters exactly
    // nine form frames (objects 5..=13) and never decodes past the horizon.
    let input = form_pdf(&chain(12));
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let frame = b"/N Do".len();
    let leaf = b"0 0 m 1 1 l f".len();
    // The deep root refuses after charging EXACTLY nine targets and nine
    // frame decodes: the never-visited leaf's own analysis still fits in the
    // one remaining target and its exact byte length.
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(10, 9 * frame + leaf);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 17),
        Some([false, true])
    );
    // One target or one byte short of that sum starves the leaf: the deep
    // root charged nine frames, not one more and not one less.
    let mut short_targets = FormXObjectEffectAnalyzer::with_bounds(9, 9 * frame + leaf);
    assert_eq!(analyze_object(&mut short_targets, &input, lookup, 5), None);
    assert_eq!(analyze_object(&mut short_targets, &input, lookup, 17), None);
    let mut short_bytes = FormXObjectEffectAnalyzer::with_bounds(10, 9 * frame + leaf - 1);
    assert_eq!(analyze_object(&mut short_bytes, &input, lookup, 5), None);
    assert_eq!(analyze_object(&mut short_bytes, &input, lookup, 17), None);
}

#[test]
fn forms_past_the_horizon_stay_unvisited_and_later_analyzable() {
    // After the twelve-edge root refusal, every later query still resolves
    // purely from each form's own subtree: the eleven-edge mid form deepens
    // its horizon-cut lattice and refuses, the three-edge form past the
    // horizon proves, and the exactly-eight-edge mid form deepens and proves.
    let input = form_pdf(&chain(12));
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::new();
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 6), None);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 14),
        Some([false, true])
    );
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 9),
        Some([false, true])
    );
}

#[test]
fn deepening_a_horizon_cut_lattice_recharges_bytes_but_never_targets() {
    // Analyzing the nine-edge root first cuts objects 6..=13 at the horizon;
    // deepening object 6 to its own eight-edge proof re-decodes those eight
    // frames and freshly analyzes only the leaf: exactly ONE further target
    // and the re-decoded byte sum suffice.
    let input = form_pdf(&chain(9));
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let frame = b"/N Do".len();
    let leaf = b"0 0 m 1 1 l f".len();
    let exact_bytes = 9 * frame + 8 * frame + leaf;
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(10, exact_bytes);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 6),
        Some([false, true])
    );
    // With no target left for the leaf the deepening refuses fail-closed;
    // one byte short of the re-decoded sum refuses identically.
    let mut short_targets = FormXObjectEffectAnalyzer::with_bounds(9, exact_bytes);
    assert_eq!(analyze_object(&mut short_targets, &input, lookup, 5), None);
    assert_eq!(analyze_object(&mut short_targets, &input, lookup, 6), None);
    let mut short_bytes = FormXObjectEffectAnalyzer::with_bounds(10, exact_bytes - 1);
    assert_eq!(analyze_object(&mut short_bytes, &input, lookup, 5), None);
    assert_eq!(analyze_object(&mut short_bytes, &input, lookup, 6), None);
}

#[test]
fn an_unaffordable_horizon_cut_flate_deepening_exhausts_once_and_preserves_partial() {
    // The nine-edge root enters objects 5..=13 and leaves object 6 with slots
    // 0..=7 computed. Make object 6 Flate so its later root query must
    // re-inflate that partial frame before it can demand slot 8.
    let mut objects = chain(9);
    let mut flate_plain = vec![b' '; 64];
    flate_plain.extend_from_slice(b"/N Do");
    let compressed = encode_flate_stream(&flate_plain, FLATE_LIMIT).expect("encode");
    objects[1] = stream_body(
        &format!("{FORM_BASE} /Resources << /XObject << /N 7 0 R >> >> /Filter /FlateDecode"),
        &compressed,
    );
    let stable_object = 5 + u32::try_from(objects.len()).expect("object count fits u32");
    let stable = b"q Q";
    objects.push(form("", stable));

    let input = form_pdf(&objects);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let frame = b"/N Do".len();
    // The first root walk charges object 6's decoded length plus the eight
    // other visited raw frames. Leave a positive residual that is one byte
    // too small to inflate object 6 again during deepening.
    let initial_root_bytes = flate_plain.len() + 8 * frame;
    let residual = flate_plain.len() - 1;
    let mut analyzer =
        FormXObjectEffectAnalyzer::with_bounds(10, stable.len() + initial_root_bytes + residual);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, stable_object),
        Some([false, false])
    );
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(analyzer.remaining_bytes_for_test(), residual);

    let partial = IndirectRef {
        object_number: 6,
        generation: 0,
    };
    let partial_offset = object_offset(lookup, 6);
    assert_eq!(
        analyzer.cached_computed_through_for_test(partial, partial_offset),
        Some(7)
    );

    // The first unaffordable deepening gets structured
    // `OutputLimitExceeded`, exhausts the residual allowance, and leaves the
    // partial entry intact. Later identical queries stop at the zero-budget
    // guard: no further inflation, no cache erosion, deterministic Unknown.
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 6), None);
    assert_eq!(analyzer.remaining_bytes_for_test(), 0);
    assert_eq!(
        analyzer.cached_computed_through_for_test(partial, partial_offset),
        Some(7)
    );
    for _ in 0..3 {
        assert_eq!(analyze_object(&mut analyzer, &input, lookup, 6), None);
        assert_eq!(analyzer.remaining_bytes_for_test(), 0);
        assert_eq!(
            analyzer.cached_computed_through_for_test(partial, partial_offset),
            Some(7)
        );
    }
    // A complete entry cached before exhaustion remains available even with
    // no target or byte allowance left.
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, stable_object),
        Some([false, false])
    );
}

#[test]
fn a_raw_over_budget_attempt_exhausts_the_residual_allowance() {
    let mut large = vec![b' '; 64];
    large.extend_from_slice(b"0 0 m 1 1 l f");
    let stable = b"q Q";
    let input = form_pdf(&[form("", &large), form("", stable)]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(2, stable.len() + large.len() - 1);

    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 6),
        Some([false, false])
    );
    assert_eq!(analyzer.remaining_bytes_for_test(), large.len() - 1);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(analyzer.remaining_bytes_for_test(), 0);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(analyzer.remaining_bytes_for_test(), 0);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 6),
        Some([false, false])
    );
}

// --- Admission parity ---------------------------------------------------------

#[test]
fn a_child_outside_the_root_admission_rules_refuses_only_when_invoked() {
    for bad_child in [
        form(" /OC 8 0 R", b"0 0 m 1 1 l f"),
        form(" /Group << /S /Transparency >>", b"0 0 m 1 1 l f"),
        form("", b"q 0 0 m 1 1 l f"),
        form("", b"Q"),
        form(" /Filter /LZWDecode", b"0 0 m 1 1 l f"),
    ] {
        assert_eq!(
            analyze(&[xform("/C 6 0 R", b"/C Do"), bad_child.clone()]),
            None
        );
        // The identical bad sibling, declared but never invoked, poisons
        // nothing else: the invoked ordinary Image stays proven.
        assert_eq!(
            analyze(&[
                xform("/C 6 0 R /Im 7 0 R", b"/Im Do"),
                bad_child,
                image_object(),
            ]),
            Some([false, false])
        );
    }
}

#[test]
fn resource_less_children_admit_only_resource_independent_content() {
    // A proven-absent `/Resources` child with direct paint proves and folds.
    assert_eq!(
        analyze(&[xform("/C 6 0 R", b"/C Do"), form("", b"0 0 m 1 1 l f")]),
        Some([false, true])
    );
    // The same child invoking an alias selection or a `Do` has no namespace of
    // its own to resolve them in and refuses fail-closed.
    assert_eq!(
        analyze(&[
            xform("/C 6 0 R", b"/C Do"),
            form("", b"/Alias cs 0.5 sc 0 0 m 1 1 l f"),
        ]),
        None
    );
    assert_eq!(
        analyze(&[xform("/C 6 0 R", b"/C Do"), form("", b"/X Do")]),
        None
    );
    // A reserved direct selection stays resource-independent: `CS /DeviceRGB`
    // kills the stroking lane locally while the fill still consumes the
    // inherited nonstroking sentinel.
    assert_eq!(
        analyze(&[
            xform("/C 6 0 R", b"/C Do"),
            form("", b"/DeviceRGB CS 0 0 m 1 1 l S 0 0 m 1 1 l f"),
        ]),
        Some([false, true])
    );
}

#[test]
fn a_child_missing_resources_never_resolves_through_its_parent() {
    // The parent declares `/Im`; the resource-less child invoking `/Im` must
    // NOT see it: caller resources never reach a child analysis.
    let parent_resources = "/C 6 0 R /Im 7 0 R";
    assert_eq!(
        analyze(&[
            xform(parent_resources, b"/C Do"),
            form("", b"/Im Do"),
            image_object(),
        ]),
        None
    );
    // The parent's own invocation of the same name proves, isolating the
    // refusal to the child's missing namespace.
    assert_eq!(
        analyze(&[
            xform(parent_resources, b"/Im Do"),
            form("", b"/Im Do"),
            image_object(),
        ]),
        Some([false, false])
    );
}

/// Xref-stream PDF whose analyzed root Form is uncompressed object 5 while its
/// invoked nested Form target is the Type-2 member 6 of object stream 7.
fn xref_stream_pdf_with_compressed_nested_form() -> Vec<u8> {
    let mut buf = b"%PDF-1.5\n".to_vec();
    let uncompressed = [
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream_body("", b""),
        xform("/Fm 6 0 R", b"/Fm Do"),
    ];
    let mut offsets = Vec::new();
    for (index, body) in uncompressed.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }

    let member_header = b"6 0 ";
    let member = b"<< /Type /XObject /Subtype /Form /BBox [0 0 100 100] >>";
    let mut object_stream_data = member_header.to_vec();
    object_stream_data.extend_from_slice(member);
    let object_stream_offset = buf.len();
    buf.extend_from_slice(b"7 0 obj\n");
    buf.extend_from_slice(&stream_body(
        &format!(" /Type /ObjStm /N 1 /First {}", member_header.len()),
        &object_stream_data,
    ));
    buf.extend_from_slice(b"\nendobj\n");

    let xref_offset = buf.len();
    let mut xref_body = Vec::new();
    xref_body.extend_from_slice(&super::xref_record(0, 0, 0));
    for offset in offsets {
        xref_body.extend_from_slice(&super::xref_record(1, offset, 0));
    }
    xref_body.extend_from_slice(&super::xref_record(2, 7, 0));
    xref_body.extend_from_slice(&super::xref_record(1, object_stream_offset, 0));
    xref_body.extend_from_slice(&super::xref_record(1, xref_offset, 0));
    buf.extend_from_slice(
        format!(
            "8 0 obj\n<< /Type /XRef /Size 9 /Index [0 9] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            xref_body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&xref_body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}

#[test]
fn child_identity_mismatches_and_compressed_targets_refuse() {
    // A generation mismatch on the invoked nested Form target refuses through
    // the authority's exact corroboration before any descent.
    assert_eq!(
        analyze(&[xform("/Fm 6 1 R", b"/Fm Do"), form("", b"0 0 m 1 1 l f")]),
        None
    );
    // An unresolvable child reference refuses identically.
    assert_eq!(analyze(&[xform("/Fm 99 0 R", b"/Fm Do")]), None);
    // A compressed (non-source-addressable) nested Form target refuses without
    // descent.
    let input = xref_stream_pdf_with_compressed_nested_form();
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    assert!(matches!(
        locate_xref_object(lookup, 6),
        ObjectLookupLocation::XrefStreamCompressed { .. }
    ));
    let mut analyzer = FormXObjectEffectAnalyzer::new();
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
}

// --- Budgets across the descent ------------------------------------------------

#[test]
fn a_large_flate_child_charges_its_decode_once_across_two_parents() {
    let parent_a = b"/C Do";
    let parent_b = b"/C Do 0 0 m 1 1 l S";
    let child_plain: Vec<u8> = b"0 0 m 1 1 l f ".repeat(64);
    let compressed = encode_flate_stream(&child_plain, FLATE_LIMIT).expect("encode");
    let input = form_pdf(&[
        xform("/C 7 0 R", parent_a),
        xform("/C 7 0 R", parent_b),
        stream_body(&format!("{FORM_BASE} /Filter /FlateDecode"), &compressed),
    ]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // Both parents and ONE decoded-child charge fit the exact byte sum.
    let exact_bytes = parent_a.len() + parent_b.len() + child_plain.len();
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(3, exact_bytes);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 5),
        Some([false, true])
    );
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 6),
        Some([true, true])
    );
}

#[test]
fn budget_exhaustion_mid_descent_refuses_the_root_while_cached_forms_serve() {
    let small = b"0 0 m 1 1 l f";
    let root = b"/S Do /L Do";
    let large_plain: Vec<u8> = b"0 0 m 1 1 l f ".repeat(64);
    let compressed = encode_flate_stream(&large_plain, FLATE_LIMIT).expect("encode");
    let input = form_pdf(&[
        xform("/S 6 0 R /L 7 0 R", root),
        form("", small),
        stream_body(&format!("{FORM_BASE} /Filter /FlateDecode"), &compressed),
    ]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // Afford the small child and the root, but not the large child's decode:
    // the descent exhausts mid-way and the root refuses fail-closed.
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(3, small.len() + root.len() + 8);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 6),
        Some([false, true])
    );
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    // The previously cached small form keeps serving hits after exhaustion.
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 6),
        Some([false, true])
    );
}

// --- End-to-end page-only mutation ----------------------------------------------

fn page_body(contents: &str, resources: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {contents} /Resources << {resources} >> >>"
    )
    .into_bytes()
}

fn resource_pdf(content: &[u8], resources: &str, objects: &[Vec<u8>]) -> Vec<u8> {
    let mut bodies = vec![
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("4 0 R", resources),
        stream_body("", content),
    ];
    bodies.extend_from_slice(objects);
    assemble_classic(&bodies)
}

fn convert_link(input: &[u8], link: &str) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: vec![DeviceLinkInput {
                id: None,
                bytes: link_bytes(link),
            }],
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds")
}

const PAGE_RESOURCES: &str = "/ColorSpace << /GrayAlias /DeviceGray >> /XObject << /Fm 5 0 R >>";

#[test]
fn a_proven_nested_consumption_closes_the_page_alias_and_converts_only_the_page_setter() {
    // Page -> form -> child -> grandchild: the grandchild's fill consumes the
    // page's inherited nonstroking lane through two folds, so the alias root
    // closes and the existing page setter converts. Every Form byte stays
    // identical and appears exactly once.
    let root_form = xform("/C 6 0 R", b"/C Do");
    let child = xform("/G 7 0 R", b"/G Do");
    let grandchild = form("", b"0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[root_form.clone(), child.clone(), grandchild.clone()],
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(!contains(
        &page_decoded_stream(&output.bytes, false),
        b"GrayAlias"
    ));
    assert!(contains(&output.bytes, &root_form));
    assert!(contains(&output.bytes, &child));
    assert!(contains(&output.bytes, &grandchild));
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"6 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"7 0 obj"), 1);
    assert_eq!(output.converted.len(), 1);
    reopen(&output.bytes);
}

#[test]
fn a_neutral_nested_tree_leaves_the_page_alias_live_for_a_later_consumer() {
    // The nested tree only paints under its own local colour, so the page
    // alias survives the `Do` and is consumed by the LATER page fill, which
    // converts the existing page setter.
    let root_form = xform("/C 6 0 R", b"/C Do");
    let neutral_child = form("", b"0.5 g 0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do 0 0 m 1 1 l f\n",
        PAGE_RESOURCES,
        &[root_form.clone(), neutral_child.clone()],
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(contains(&output.bytes, &root_form));
    assert!(contains(&output.bytes, &neutral_child));
    reopen(&output.bytes);

    // Without any later consumer the neutral tree leaves the alias setter
    // verbatim: nothing converts and nothing is refused.
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[root_form.clone(), neutral_child],
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
    assert!(contains(&output.bytes, &root_form));
    reopen(&output.bytes);
}

#[test]
fn a_mutual_cycle_refuses_end_to_end_without_reentrancy_or_byte_changes() {
    // Two mutually invoking forms behind the page `Do`: the analysis refuses
    // deterministically inside one analyzer borrow (no `RefCell` re-entry),
    // the alias setter survives verbatim, and both Form bodies stay identical
    // and unduplicated.
    let form_a = xform("/B 6 0 R", b"/B Do");
    let form_b = xform("/A 5 0 R", b"/A Do");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[form_a.clone(), form_b.clone()],
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
    assert!(contains(&output.bytes, &form_a));
    assert!(contains(&output.bytes, &form_b));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"6 0 obj"), 1);
    reopen(&output.bytes);
}
