//! Focused epoch-plan matrix: paired-lane state, initial-colour candidates,
//! q/Q closure, strict operator/consumer boundaries, selector/route/Default/
//! localization/ownership refusals, and repeated-occurrence consistency.
//!
//! Everything here drives the plan exactly as the converter does: one fresh
//! [`PaintProgram`] walk over one parsed logical page sequence, seeding the
//! pre-op snapshot from the page default, observing every op, then closing at
//! page end. No byte is ever changed; assertions read the private outcome.

use std::rc::Rc;

use presslint_paint::{GraphicsStateSnapshot, PaintProgram};
use presslint_pdf::{
    ColorSpaceFamily, DefaultColorSpaceKind, IndirectObjectEditDisposition, IndirectRef,
};
use presslint_selectors::{Predicate, Selector};
use presslint_types::{ColorSpace, ColorUsage, PageIndex, PdfName};

use crate::DeviceLinkInput;
use crate::alias_epoch_plan::{
    AliasEpochOutcome, AliasEpochPlan, AliasEpochReport, CandidateKind, EpochRefusalReason,
    EpochStatus, LaneSide,
};
use crate::content_color_convert::DeviceColorSpace;
use crate::link_routing::{LinkRouting, build_link_routing};
use crate::page_content_sequence::{OccurrenceInput, PageContentSequence};
use crate::page_device_space_policy::{PageColorFacts, PageDeviceSpacePolicy};
use crate::page_xobject_policy::PageXObjectPolicy;

use super::content_color_convert::{
    CMYK_TO_CMYK_LINK, GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, link_bytes,
};
use super::page_device_space_policy::{
    absent_defaults, default_fact, defaults_report, three_aliases,
};

const CAP: usize = 1 << 20;

fn object(number: u32) -> IndirectRef {
    IndirectRef {
        object_number: number,
        generation: 0,
    }
}

/// Parse one logical sequence from ordered `(bytes, object, disposition)`.
fn sequence_of(streams: &[(&[u8], u32, IndirectObjectEditDisposition)]) -> PageContentSequence {
    let inputs: Vec<OccurrenceInput<'_>> = streams
        .iter()
        .enumerate()
        .map(
            |(ordinal, (decoded, number, disposition))| OccurrenceInput {
                stream_ordinal: ordinal,
                content_object: object(*number),
                decoded,
                disposition: *disposition,
            },
        )
        .collect();
    PageContentSequence::new(&inputs, CAP).expect("sequence parses")
}

/// The three exact device aliases over provably absent defaults.
fn standard_policy() -> PageDeviceSpacePolicy {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    PageDeviceSpacePolicy::from_page_facts(&PageColorFacts {
        color_spaces: Some(&color_spaces),
        defaults: Some(&defaults),
    })
}

fn routing_of(links: &[&str]) -> LinkRouting {
    let inputs: Vec<DeviceLinkInput> = links
        .iter()
        .map(|hex| DeviceLinkInput {
            id: None,
            bytes: link_bytes(hex),
        })
        .collect();
    build_link_routing(&inputs).expect("routing builds")
}

/// Gray, RGB->CMYK, and CMYK links in request order 0/1/2.
fn all_links() -> LinkRouting {
    routing_of(&[GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, CMYK_TO_CMYK_LINK])
}

/// Drive the plan exactly as the converter's single walk does. `None` mirrors
/// the converter's whole-page walk-failure refusal. The `XObject` policy is the
/// unmatched (`None`-report) one, so every named `Do` stays unknown/refused;
/// admitted-image/stencil behaviour lives in the dedicated `XObject` matrix.
fn run_plan(
    policy: &PageDeviceSpacePolicy,
    routing: &LinkRouting,
    target: Option<&Selector>,
    sequence: &PageContentSequence,
) -> Option<AliasEpochOutcome> {
    let program = PaintProgram::new(
        sequence.bytes(),
        sequence.records(),
        policy.color_space_env(),
    );
    let xobjects = PageXObjectPolicy::new(None);
    let mut plan = AliasEpochPlan::new(policy, routing, &xobjects, target, PageIndex(0), sequence);
    let mut previous = Rc::new(GraphicsStateSnapshot::page_default());
    for op in program.ops() {
        let Ok(op) = op else {
            return None;
        };
        let state_before = std::mem::replace(&mut previous, Rc::clone(&op.state));
        let operator = &sequence.bytes()[op.operator_range.start()..op.operator_range.end()];
        plan.observe(&op, operator, &state_before, sequence);
    }
    Some(plan.finish())
}

/// Standard policy, all three links, no selector, one in-place stream.
fn run(stream: &[u8]) -> AliasEpochOutcome {
    run_streams(&[(stream, 4, IndirectObjectEditDisposition::InPlaceMutation)])
}

fn run_streams(streams: &[(&[u8], u32, IndirectObjectEditDisposition)]) -> AliasEpochOutcome {
    let policy = standard_policy();
    let routing = all_links();
    let sequence = sequence_of(streams);
    run_plan(&policy, &routing, None, &sequence).expect("walk succeeds")
}

fn refusal(epoch: &AliasEpochReport) -> Option<EpochRefusalReason> {
    match epoch.status {
        EpochStatus::Closed => None,
        EpochStatus::Refused { reason, .. } => Some(reason),
    }
}

fn is_closed(epoch: &AliasEpochReport) -> bool {
    epoch.status == EpochStatus::Closed
}

fn in_place(stream: &[u8], number: u32) -> (&[u8], u32, IndirectObjectEditDisposition) {
    (
        stream,
        number,
        IndirectObjectEditDisposition::InPlaceMutation,
    )
}

// --- Selection-initial candidates ----------------------------------------------

#[test]
fn every_family_and_side_selection_carries_the_exact_initial_candidate() {
    let stream = b"/GrayAlias cs f\n/GrayAlias CS S\n/RgbAlias cs f\n/RgbAlias CS S\n/CmykAlias cs f\n/CmykAlias CS S\n";
    let outcome = run(stream);
    let expected: [(&str, LaneSide, DeviceColorSpace, usize, &[f64]); 6] = [
        (
            "GrayAlias",
            LaneSide::Nonstroking,
            DeviceColorSpace::Gray,
            0,
            &[0.0],
        ),
        (
            "GrayAlias",
            LaneSide::Stroking,
            DeviceColorSpace::Gray,
            0,
            &[0.0],
        ),
        (
            "RgbAlias",
            LaneSide::Nonstroking,
            DeviceColorSpace::Rgb,
            1,
            &[0.0, 0.0, 0.0],
        ),
        (
            "RgbAlias",
            LaneSide::Stroking,
            DeviceColorSpace::Rgb,
            1,
            &[0.0, 0.0, 0.0],
        ),
        (
            "CmykAlias",
            LaneSide::Nonstroking,
            DeviceColorSpace::Cmyk,
            2,
            &[0.0, 0.0, 0.0, 1.0],
        ),
        (
            "CmykAlias",
            LaneSide::Stroking,
            DeviceColorSpace::Cmyk,
            2,
            &[0.0, 0.0, 0.0, 1.0],
        ),
    ];
    assert_eq!(outcome.epochs.len(), expected.len());
    for (epoch, (alias, side, source, link_index, components)) in
        outcome.epochs.iter().zip(expected)
    {
        assert_eq!(epoch.alias, PdfName(alias.as_bytes().to_vec()));
        assert_eq!(epoch.side, side);
        assert_eq!(epoch.source, source);
        let route = epoch.route.expect("route fixed at selection");
        assert_eq!(route.link_index, link_index);
        assert!(is_closed(epoch), "{alias} closes");
        assert!(epoch.has_consumer, "{alias} consumed");
        assert_eq!(epoch.candidates.len(), 1);
        let candidate = &epoch.candidates[0];
        assert_eq!(candidate.kind, CandidateKind::SelectionInitial);
        assert_eq!(candidate.components, components);
        assert!(candidate.selector_matched);
        assert_eq!(candidate.occurrence_index, 0);
    }
    // Fixed emitted families: gray->gray, rgb->cmyk, cmyk->cmyk.
    assert_eq!(
        outcome.epochs[0].route.expect("gray").destination,
        DeviceColorSpace::Gray
    );
    assert_eq!(
        outcome.epochs[2].route.expect("rgb").destination,
        DeviceColorSpace::Cmyk
    );
    assert_eq!(
        outcome.epochs[4].route.expect("cmyk").destination,
        DeviceColorSpace::Cmyk
    );
}

#[test]
fn selection_candidate_localizes_to_the_exact_record_bytes() {
    let stream = b"/GrayAlias cs f\n";
    let outcome = run(stream);
    let candidate = &outcome.epochs[0].candidates[0];
    assert_eq!(
        &stream[candidate.local_range.start..candidate.local_range.end],
        b"/GrayAlias cs"
    );
}

#[test]
fn initial_colour_consumed_before_an_explicit_setter_stays_a_valid_candidate() {
    let outcome = run(b"/GrayAlias cs f 0.5 sc f\n");
    assert_eq!(outcome.epochs.len(), 1);
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(epoch.has_consumer);
    let kinds: Vec<CandidateKind> = epoch.candidates.iter().map(|c| c.kind).collect();
    assert_eq!(
        kinds,
        vec![
            CandidateKind::SelectionInitial,
            CandidateKind::ExplicitSetter
        ]
    );
    assert_eq!(epoch.candidates[1].components, vec![0.5]);
    assert_eq!(outcome.eligible_setters, vec![1]);
}

#[test]
fn epoch_without_a_paint_consumer_closes_as_a_noop_candidate() {
    let outcome = run(b"/GrayAlias cs 0.5 sc\n");
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(
        !epoch.has_consumer,
        "no consumer: authorizes no byte change"
    );
}

// --- Structural setter tallies (unchanged T177 meaning) ------------------------

#[test]
fn setter_without_a_prior_selection_fabricates_no_epoch_and_stays_uncounted() {
    let outcome = run(b"0.5 sc\n");
    assert!(outcome.epochs.is_empty());
    assert_eq!(outcome.eligible_setters, vec![0]);
    assert_eq!(outcome.ineligible_setters, vec![0]);
}

#[test]
fn unresolved_selection_name_fabricates_no_epoch() {
    let outcome = run(b"/Nope cs 0.5 sc f\n");
    assert!(outcome.epochs.is_empty());
    assert_eq!(outcome.eligible_setters, vec![0]);
    assert_eq!(outcome.ineligible_setters, vec![0]);
}

#[test]
fn ineligible_setter_shapes_refuse_the_root_and_count_ineligible() {
    let cases: [(&[u8], &str); 3] = [
        (b"/GrayAlias cs 0.5 0.5 sc f\n", "wrong component count"),
        (b"/GrayAlias cs 1.5 sc f\n", "operand above 1"),
        (
            b"/GrayAlias cs 0.5 /PatternPaint scn f\n",
            "trailing Pattern name",
        ),
    ];
    for (stream, label) in cases {
        let outcome = run(stream);
        assert_eq!(
            refusal(&outcome.epochs[0]),
            Some(EpochRefusalReason::IneligibleSetter),
            "{label}"
        );
        assert_eq!(outcome.eligible_setters, vec![0], "{label}");
        assert_eq!(outcome.ineligible_setters, vec![1], "{label}");
    }
}

// --- Lane independence and termination ------------------------------------------

#[test]
fn lanes_carry_independent_epochs_with_independent_candidates() {
    let outcome = run(b"/GrayAlias cs /RgbAlias CS 0.5 sc 0 0.5 1 SC f S\n");
    assert_eq!(outcome.epochs.len(), 2);
    let fill = &outcome.epochs[0];
    assert_eq!(fill.side, LaneSide::Nonstroking);
    assert_eq!(fill.candidates[1].components, vec![0.5]);
    assert!(is_closed(fill) && fill.has_consumer);
    let stroke = &outcome.epochs[1];
    assert_eq!(stroke.side, LaneSide::Stroking);
    assert_eq!(stroke.candidates[1].components, vec![0.0, 0.5, 1.0]);
    assert!(is_closed(stroke) && stroke.has_consumer);
    assert_eq!(outcome.eligible_setters, vec![2]);
}

#[test]
fn other_space_selection_terminates_the_branch_without_refusing_the_epoch() {
    let outcome = run(b"/GrayAlias cs /Nope cs 0.5 sc f\n");
    assert_eq!(outcome.epochs.len(), 1);
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(!epoch.has_consumer, "the f consumed the unresolved space");
    assert_eq!(outcome.eligible_setters, vec![0]);
}

#[test]
fn direct_shortcut_terminates_the_branch_without_refusing_the_epoch() {
    let outcome = run(b"/GrayAlias cs 0.5 g f\n");
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(!epoch.has_consumer);
    assert_eq!(epoch.candidates.len(), 1);
}

#[test]
fn reselecting_the_same_alias_starts_an_independent_epoch() {
    let outcome = run(b"/GrayAlias cs 0.5 sc f /GrayAlias cs f\n");
    assert_eq!(outcome.epochs.len(), 2);
    assert_eq!(outcome.epochs[0].candidates.len(), 2);
    assert_eq!(outcome.epochs[1].candidates.len(), 1);
    assert!(outcome.epochs.iter().all(is_closed));
    assert!(outcome.epochs.iter().all(|epoch| epoch.has_consumer));
}

// --- q/Q closure -----------------------------------------------------------------

#[test]
fn q_derived_branch_belongs_to_the_root_and_the_root_resumes_after_restore() {
    let outcome = run(b"/GrayAlias cs q 0.5 sc f Q f\n");
    assert_eq!(outcome.epochs.len(), 1);
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(epoch.has_consumer);
    assert_eq!(epoch.candidates.len(), 2);
}

#[test]
fn alias_selected_inside_a_saved_frame_is_an_independent_epoch_closed_at_restore() {
    let outcome = run(b"q /GrayAlias cs 0.5 sc f Q f\n");
    assert_eq!(outcome.epochs.len(), 1);
    assert!(is_closed(&outcome.epochs[0]));
    assert!(outcome.epochs[0].has_consumer);
}

#[test]
fn direct_termination_inside_a_frame_restores_the_outer_pair_after_restore() {
    let outcome = run(b"/GrayAlias cs q 0 g Q f\n");
    assert_eq!(outcome.epochs.len(), 1);
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(epoch.has_consumer, "the restored alias lane fed the f");
    assert_eq!(epoch.candidates.len(), 1);
}

#[test]
fn restore_returns_the_saved_lane_tuple_and_the_later_consumer_proves_it() {
    // The saved pair carries the branch-local source tuple: the paint after Q
    // is proved against the RESTORED 0.2, not just alias name and family.
    let outcome = run(b"/GrayAlias cs 0.2 sc q 0.8 sc f Q f\n");
    assert_eq!(outcome.epochs.len(), 1);
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(epoch.has_consumer);
    let components: Vec<&[f64]> = epoch
        .candidates
        .iter()
        .map(|candidate| candidate.components.as_slice())
        .collect();
    assert_eq!(components, vec![&[0.0][..], &[0.2], &[0.8]]);
    assert_eq!(outcome.eligible_setters, vec![2]);
}

#[test]
fn nested_restores_prove_each_saved_tuple_in_turn() {
    // Consumers fire at 0.6, at the restored 0.8, and at the restored 0.2;
    // each is cross-checked against its own frame's saved pending tuple.
    let outcome = run(b"/GrayAlias cs 0.2 sc q 0.8 sc f q 0.6 sc f Q f Q f\n");
    assert_eq!(outcome.epochs.len(), 1);
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(epoch.has_consumer);
    let components: Vec<&[f64]> = epoch
        .candidates
        .iter()
        .map(|candidate| candidate.components.as_slice())
        .collect();
    assert_eq!(components, vec![&[0.0][..], &[0.2], &[0.8], &[0.6]]);
    assert_eq!(outcome.eligible_setters, vec![3]);
}

#[test]
fn one_failing_branch_refuses_the_whole_root_and_a_later_epoch_still_proves() {
    let outcome = run(b"/GrayAlias cs q 1.5 sc Q f /GrayAlias cs 0.5 sc f\n");
    assert_eq!(outcome.epochs.len(), 2);
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::IneligibleSetter)
    );
    assert!(is_closed(&outcome.epochs[1]));
    assert_eq!(outcome.eligible_setters, vec![1]);
    assert_eq!(outcome.ineligible_setters, vec![1]);
}

#[test]
fn boundary_refuses_a_root_suspended_in_a_saved_frame() {
    // The current lane was terminated inside the frame, but the root epoch
    // resumes after Q, so the unknown boundary still refuses it fail-closed.
    let outcome = run(b"/GrayAlias cs q 0 g (x) Tj Q f\n");
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::TextShow)
    );
}

#[test]
fn restore_underflow_stays_a_whole_page_walk_failure() {
    let policy = standard_policy();
    let routing = all_links();
    let sequence = sequence_of(&[in_place(b"/GrayAlias cs Q f\n", 4)]);
    assert!(run_plan(&policy, &routing, None, &sequence).is_none());
}

#[test]
fn trailing_unmatched_save_refuses_every_alias_plan_on_the_page() {
    // Even an epoch terminated BEFORE the trailing q refuses: ISO 32000-1
    // §8.4.2 requires balance across the logical page.
    let outcome = run(b"/GrayAlias cs 0.5 sc f 0 g q\n");
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::UnbalancedSaveAtPageEnd)
    );
    // Structural counts keep their exact per-setter meaning regardless.
    assert_eq!(outcome.eligible_setters, vec![1]);
}

#[test]
fn save_restore_crossing_physical_occurrences_closes_normally() {
    let outcome = run_streams(&[
        in_place(b"/GrayAlias cs q 0.5 sc\n", 4),
        in_place(b"f Q f\n", 5),
    ]);
    assert_eq!(outcome.epochs.len(), 1);
    assert!(is_closed(&outcome.epochs[0]));
    assert!(outcome.epochs[0].has_consumer);
    assert_eq!(outcome.eligible_setters, vec![1, 0]);
}

// --- Consumer side mapping --------------------------------------------------------

#[test]
fn stroke_paints_consume_the_stroking_lane_only() {
    for op in ["S", "s"] {
        let stream = format!("/GrayAlias CS /RgbAlias cs {op}\n");
        let outcome = run(stream.as_bytes());
        assert!(outcome.epochs[0].has_consumer, "{op} consumes stroke");
        assert!(!outcome.epochs[1].has_consumer, "{op} leaves fill alone");
    }
}

#[test]
fn fill_paints_consume_the_nonstroking_lane_only() {
    for op in ["f", "F", "f*"] {
        let stream = format!("/GrayAlias cs /RgbAlias CS {op}\n");
        let outcome = run(stream.as_bytes());
        assert!(outcome.epochs[0].has_consumer, "{op} consumes fill");
        assert!(!outcome.epochs[1].has_consumer, "{op} leaves stroke alone");
    }
}

#[test]
fn fill_and_stroke_paints_consume_both_lanes() {
    for op in ["B", "B*", "b", "b*"] {
        let stream = format!("/GrayAlias cs /RgbAlias CS {op}\n");
        let outcome = run(stream.as_bytes());
        assert!(
            outcome.epochs.iter().all(|epoch| epoch.has_consumer),
            "{op}"
        );
        assert!(outcome.epochs.iter().all(is_closed), "{op}");
    }
}

#[test]
fn end_path_consumes_neither_lane_and_sh_is_colour_neutral() {
    let outcome = run(b"/GrayAlias cs /RgbAlias CS n\n");
    assert!(outcome.epochs.iter().all(is_closed));
    assert!(outcome.epochs.iter().all(|epoch| !epoch.has_consumer));

    // A shading owns its colour space: byte-verbatim, neutral, no consumer.
    let outcome = run(b"/GrayAlias cs /Sh0 sh f\n");
    assert!(is_closed(&outcome.epochs[0]));
    assert!(outcome.epochs[0].has_consumer, "the later f still consumes");
}

// --- Strict operator boundaries ----------------------------------------------------

#[test]
fn colour_neutral_allowlist_operators_keep_the_epoch_provable() {
    let stream = b"/GrayAlias cs 0.5 sc 1 0 0 1 5 5 cm 0.4 w 2 J 1 j 4 M [2 1] 0 d \
/Perceptual ri 1 i /GS0 gs 1 Tr BT /F1 12 Tf 1 0 0 1 0 0 Tm 12 TL 1 Tc 1 Tw 100 Tz \
1 Ts T* 0 0 Td 0 0 TD ET /Tag MP /Tag /P DP BMC /Tag EMC 0 0 m 1 1 l 2 2 3 3 4 4 c \
1 1 2 2 v 1 1 2 2 y h W n 0 0 5 5 re f\n";
    let outcome = run(stream);
    assert_eq!(outcome.epochs.len(), 1);
    assert!(is_closed(&outcome.epochs[0]));
    assert!(outcome.epochs[0].has_consumer);
    assert_eq!(outcome.eligible_setters, vec![1]);
}

#[test]
fn set_font_is_colour_neutral_context_checked_and_does_not_relax_text_show() {
    let outcome = run(b"/GrayAlias cs 0.5 sc BT /F1 12 Tf ET f\n");
    assert!(is_closed(&outcome.epochs[0]));
    assert!(outcome.epochs[0].has_consumer);

    let outcome = run(b"/GrayAlias cs 0.5 sc 0 0 m /F1 12 Tf 1 1 l S\n");
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::InvalidGraphicsObjectContext)
    );

    let outcome = run(b"/GrayAlias cs 0.5 sc BT /F1 12 Tf (x) Tj ET f\n");
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::TextShow)
    );
}

#[test]
fn conservative_boundaries_refuse_a_live_root_each_with_its_reason() {
    let cases: [(&[u8], EpochRefusalReason); 13] = [
        (b"(x) Tj", EpochRefusalReason::TextShow),
        (b"[(x)] TJ", EpochRefusalReason::TextShow),
        (b"(x) '", EpochRefusalReason::TextShow),
        (b"1 2 (x) \"", EpochRefusalReason::TextShow),
        (b"/Fm Do", EpochRefusalReason::XObjectInvoke),
        (b"BI", EpochRefusalReason::InlineImage),
        (b"ID", EpochRefusalReason::InlineImage),
        (b"EI", EpochRefusalReason::InlineImage),
        (b"1 0 d0", EpochRefusalReason::Type3Operator),
        (b"1 0 0 0 0 0 d1", EpochRefusalReason::Type3Operator),
        (b"BX", EpochRefusalReason::CompatibilitySection),
        (b"EX", EpochRefusalReason::CompatibilitySection),
        (b"XY", EpochRefusalReason::UnknownOperator),
    ];
    for (boundary, reason) in cases {
        let mut stream = b"/GrayAlias cs 0.5 sc ".to_vec();
        stream.extend_from_slice(boundary);
        stream.extend_from_slice(b" f\n");
        let outcome = run(&stream);
        assert_eq!(
            refusal(&outcome.epochs[0]),
            Some(reason),
            "{}",
            String::from_utf8_lossy(boundary)
        );
        // The structural tallies never change with epoch refusal.
        assert_eq!(outcome.eligible_setters, vec![1]);
    }
}

#[test]
fn boundaries_without_any_live_alias_refuse_nothing() {
    let outcome = run(b"(x) Tj XY BI 1 0 d0\n");
    assert!(outcome.epochs.is_empty());

    // An epoch whose every branch already terminated is not "live" anymore.
    let outcome = run(b"/GrayAlias cs 0.5 sc f 0 g (x) Tj\n");
    assert!(is_closed(&outcome.epochs[0]));
}

#[test]
fn invalid_graphics_object_context_refuses_a_live_root() {
    let cases: [(&[u8], &str); 10] = [
        (b"/GrayAlias cs 0.5 sc ET f\n", "ET without BT"),
        (b"/GrayAlias cs 0.5 sc BT BT ET ET f\n", "nested BT"),
        (b"/GrayAlias cs 0.5 sc EMC f\n", "EMC underflow"),
        (
            b"/GrayAlias cs 0.5 sc BT 0 0 1 1 re ET f\n",
            "path construction inside a text object",
        ),
        (
            b"/GrayAlias cs 0.5 sc BT q Q ET f\n",
            "q/Q inside a text object",
        ),
        (
            b"/GrayAlias cs 0.5 sc BT 1 0 0 1 0 0 cm ET f\n",
            "cm inside a text object",
        ),
        (
            b"/GrayAlias cs 0.5 sc 0 0 m 1 w 1 1 l S\n",
            "line width inside an open path object",
        ),
        (
            b"/GrayAlias cs 0.5 sc 0 0 m q 1 1 l Q S\n",
            "q/Q inside an open path object",
        ),
        (
            b"/GrayAlias cs 0.5 sc 1 1 l f\n",
            "path continuation without an open path",
        ),
        (b"/GrayAlias cs 0.5 sc W n f\n", "clipping without a path"),
    ];
    for (stream, label) in cases {
        let outcome = run(stream);
        assert_eq!(
            refusal(&outcome.epochs[0]),
            Some(EpochRefusalReason::InvalidGraphicsObjectContext),
            "{label}"
        );
        assert_eq!(outcome.eligible_setters, vec![1], "{label}");
    }
}

#[test]
fn colour_operators_inside_an_open_path_refuse_but_keep_structural_tallies() {
    let outcome = run(b"/GrayAlias cs 0 0 m 0.5 sc 1 1 l S\n");
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::InvalidGraphicsObjectContext)
    );
    // The per-setter structural classification is placement-independent.
    assert_eq!(outcome.eligible_setters, vec![1]);
}

#[test]
fn alias_selected_inside_an_open_path_refuses_its_own_epoch() {
    let outcome = run(b"0 0 m /GrayAlias cs 1 1 l S\n");
    assert_eq!(outcome.epochs.len(), 1);
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::InvalidGraphicsObjectContext)
    );
}

#[test]
fn unterminated_text_object_at_page_end_refuses_the_open_root() {
    let outcome = run(b"/GrayAlias cs 0.5 sc f BT\n");
    assert_eq!(
        outcome.epochs[0].status,
        EpochStatus::Refused {
            record_index: None,
            reason: EpochRefusalReason::InvalidGraphicsObjectContext,
        }
    );
}

#[test]
fn unterminated_path_object_at_page_end_refuses_the_open_root() {
    let outcome = run(b"/GrayAlias cs 0.5 sc f 0 0 m\n");
    assert_eq!(
        outcome.epochs[0].status,
        EpochStatus::Refused {
            record_index: None,
            reason: EpochRefusalReason::InvalidGraphicsObjectContext,
        }
    );
}

// --- Selector, route, and Default refusals -----------------------------------------

#[test]
fn selector_excluding_the_initial_candidate_refuses_the_complete_root() {
    let policy = standard_policy();
    let routing = all_links();
    let target = Selector::Predicate {
        predicate: Predicate::ColorUsage {
            usage: ColorUsage::Stroke,
        },
    };
    let sequence = sequence_of(&[in_place(b"/GrayAlias cs 0.5 sc f /GrayAlias CS S\n", 4)]);
    let outcome = run_plan(&policy, &routing, Some(&target), &sequence).expect("walk");
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::SelectorExcluded)
    );
    assert!(!outcome.epochs[0].candidates[0].selector_matched);
    assert!(is_closed(&outcome.epochs[1]), "the stroke epoch matches");
    // Structural counts stay selector-independent.
    assert_eq!(outcome.eligible_setters, vec![1]);
}

#[test]
fn selector_excluding_one_later_setter_refuses_the_root_never_a_prefix() {
    let policy = standard_policy();
    let routing = all_links();
    // Matches exactly the initial colour [0], not the later 0.5 setter.
    let target = Selector::Predicate {
        predicate: Predicate::ColorComponents {
            space: ColorSpace::DeviceGray,
            usage: None,
            components: vec![0.0],
            tolerance: None,
        },
    };
    let sequence = sequence_of(&[in_place(b"/GrayAlias cs f 0.5 sc f\n", 4)]);
    let outcome = run_plan(&policy, &routing, Some(&target), &sequence).expect("walk");
    let epoch = &outcome.epochs[0];
    assert_eq!(refusal(epoch), Some(EpochRefusalReason::SelectorExcluded));
    assert!(epoch.candidates[0].selector_matched);
    assert!(!epoch.candidates[1].selector_matched);
}

#[test]
fn missing_prepared_route_refuses_the_epoch_at_selection() {
    let policy = standard_policy();
    let routing = routing_of(&[CMYK_TO_CMYK_LINK]);
    let sequence = sequence_of(&[in_place(b"/GrayAlias cs 0.5 sc f\n", 4)]);
    let outcome = run_plan(&policy, &routing, None, &sequence).expect("walk");
    let epoch = &outcome.epochs[0];
    assert_eq!(refusal(epoch), Some(EpochRefusalReason::NoPreparedRoute));
    assert!(epoch.route.is_none());
    assert_eq!(outcome.eligible_setters, vec![1]);
}

#[test]
fn replaced_source_default_never_starts_an_epoch_and_counts_ineligible() {
    let color_spaces = three_aliases();
    let defaults = defaults_report(
        vec![default_fact(
            DefaultColorSpaceKind::DefaultGray,
            ColorSpaceFamily::DeviceCmyk,
        )],
        Vec::new(),
    );
    let policy = PageDeviceSpacePolicy::from_page_facts(&PageColorFacts {
        color_spaces: Some(&color_spaces),
        defaults: Some(&defaults),
    });
    let routing = all_links();
    let sequence = sequence_of(&[in_place(b"/GrayAlias cs 0.5 sc f\n", 4)]);
    let outcome = run_plan(&policy, &routing, None, &sequence).expect("walk");
    assert!(outcome.epochs.is_empty());
    assert_eq!(outcome.ineligible_setters, vec![1]);
}

#[test]
fn replaced_destination_default_refuses_the_routed_epoch() {
    let color_spaces = three_aliases();
    // RGB source stays raw-safe; the emitted CMYK family is replaced.
    let defaults = defaults_report(
        vec![default_fact(
            DefaultColorSpaceKind::DefaultCmyk,
            ColorSpaceFamily::DeviceRgb,
        )],
        Vec::new(),
    );
    let policy = PageDeviceSpacePolicy::from_page_facts(&PageColorFacts {
        color_spaces: Some(&color_spaces),
        defaults: Some(&defaults),
    });
    let routing = all_links();
    let sequence = sequence_of(&[in_place(b"/RgbAlias cs f\n", 4)]);
    let outcome = run_plan(&policy, &routing, None, &sequence).expect("walk");
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::RouteDefaultUnsafe)
    );
}

// --- Localization, ownership, and repeated references -------------------------------

#[test]
fn selection_record_crossing_a_physical_boundary_refuses_the_epoch() {
    let outcome = run_streams(&[in_place(b"/GrayAlias", 4), in_place(b" cs f\n", 5)]);
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::CandidateNotLocalized)
    );
    assert!(outcome.epochs[0].candidates.is_empty());
}

#[test]
fn ownership_vetoed_occurrence_refuses_the_candidate_but_keeps_the_tally() {
    let outcome = run_streams(&[(
        b"/GrayAlias cs 0.5 sc f\n",
        4,
        IndirectObjectEditDisposition::PrivateCopy,
    )]);
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::CandidateOwnershipVetoed)
    );
    // Per-occurrence structural counts are unchanged; the converter's
    // aggregation still suppresses vetoed occurrences exactly as before.
    assert_eq!(outcome.eligible_setters, vec![1]);
}

#[test]
fn epoch_spanning_physical_streams_localizes_each_candidate_to_its_occurrence() {
    let outcome = run_streams(&[in_place(b"/GrayAlias cs\n", 4), in_place(b"0.5 sc f\n", 5)]);
    let epoch = &outcome.epochs[0];
    assert!(is_closed(epoch));
    assert!(epoch.has_consumer);
    assert_eq!(epoch.candidates[0].occurrence_index, 0);
    assert_eq!(epoch.candidates[1].occurrence_index, 1);
    assert_eq!(outcome.eligible_setters, vec![0, 1]);
}

#[test]
fn repeated_reference_with_identical_candidate_facts_proves_every_epoch() {
    let stream: &[u8] = b"/GrayAlias cs 0.5 sc f\n";
    let outcome = run_streams(&[in_place(stream, 4), in_place(stream, 4)]);
    assert_eq!(outcome.epochs.len(), 2);
    assert!(outcome.epochs.iter().all(is_closed));
    assert_eq!(outcome.eligible_setters, vec![1, 1]);
}

#[test]
fn repeated_reference_losing_its_candidate_refuses_the_recorded_root() {
    // Occurrence 2 of object 4 sees the alias; occurrence 4 runs after a
    // direct g terminated the lane, so the same physical record stops being a
    // candidate: the recorded root refuses instead of authorizing a rewrite.
    let outcome = run_streams(&[
        in_place(b"/GrayAlias cs\n", 5),
        in_place(b"0.5 sc f\n", 4),
        in_place(b"0 g\n", 6),
        in_place(b"0.5 sc f\n", 4),
    ]);
    assert_eq!(outcome.epochs.len(), 1);
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::RepeatedReferenceMismatch)
    );
    assert_eq!(outcome.eligible_setters, vec![0, 1, 0, 0]);
}

#[test]
fn repeated_reference_gaining_a_candidate_refuses_the_current_root() {
    let outcome = run_streams(&[
        in_place(b"0.5 sc f\n", 4),
        in_place(b"/GrayAlias cs\n", 5),
        in_place(b"0.5 sc f\n", 4),
    ]);
    assert_eq!(outcome.epochs.len(), 1);
    assert_eq!(
        refusal(&outcome.epochs[0]),
        Some(EpochRefusalReason::RepeatedReferenceMismatch)
    );
    assert_eq!(outcome.eligible_setters, vec![0, 0, 1]);
}

#[test]
fn repeated_reference_mismatch_refuses_every_root_that_relied_on_the_record() {
    // Three occurrences of object 4: identical candidates under root 0 and
    // root 1, then a no-candidate occurrence. EVERY relying root refuses —
    // a later identical root never stays closed over a conflicting record.
    let outcome = run_streams(&[
        in_place(b"/GrayAlias cs\n", 5),
        in_place(b"0.5 sc f\n", 4),
        in_place(b"/GrayAlias cs\n", 5),
        in_place(b"0.5 sc f\n", 4),
        in_place(b"0 g\n", 6),
        in_place(b"0.5 sc f\n", 4),
    ]);
    assert_eq!(outcome.epochs.len(), 2);
    for epoch in &outcome.epochs {
        assert_eq!(
            refusal(epoch),
            Some(EpochRefusalReason::RepeatedReferenceMismatch)
        );
    }
    assert_eq!(outcome.eligible_setters, vec![0, 1, 0, 1, 0, 0]);
}

#[test]
fn poisoned_repeated_record_refuses_a_later_root_even_with_matching_facts() {
    // The divergence poisons the physical record permanently: the last
    // occurrence reproduces the ORIGINAL candidate facts exactly, yet the
    // root relying on it still refuses.
    let outcome = run_streams(&[
        in_place(b"/GrayAlias cs\n", 5),
        in_place(b"0.5 sc f\n", 4),
        in_place(b"0 g\n", 6),
        in_place(b"0.5 sc f\n", 4),
        in_place(b"/GrayAlias cs\n", 5),
        in_place(b"0.5 sc f\n", 4),
    ]);
    assert_eq!(outcome.epochs.len(), 2);
    for epoch in &outcome.epochs {
        assert_eq!(
            refusal(epoch),
            Some(EpochRefusalReason::RepeatedReferenceMismatch)
        );
    }
    assert_eq!(outcome.eligible_setters, vec![0, 1, 0, 0, 0, 1]);
}
