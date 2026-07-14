//! Focused page-font-policy and ordinary-TextShow admission matrix.
//!
//! The policy-unit half drives [`PageFontPolicy`] directly over synthetic
//! `/Font` and `/ExtGState` reports: ordinary-subtype admission, Type3/CID/
//! direct/missing refusal, safe/unsafe tuple poisoning, semantic `ExtGState`
//! collision poisoning, and the borrowed environment coverage states. The
//! plan-level half drives one fresh [`PaintProgram`] all-environments walk over
//! one parsed logical page sequence exactly as the converter does — seeding the
//! pre-op snapshot from the page default, observing every op, closing at page
//! end — to lock the Tr 0-7 lane table across all four text-show operators,
//! cross-stream font/mode state, and the exact refusal boundaries. The
//! end-to-end half proves ordinary text conversion, Type3 refusal, and verbatim
//! text bytes through the whole append pipeline.

use std::rc::Rc;

use presslint_paint::{
    ExtGStateFontDirective, FontSelectionState, GraphicsStateSnapshot, PaintProgram,
};
use presslint_pdf::{
    ClassifiedExtGStateResource, ClassifiedFontResource, ExtGStateFontEffect, ExtGStateParamClass,
    FontDictionaryTypeFact, FontSubtypeClass, IndirectObjectEditDisposition, IndirectRef,
    PageExtGStateResourcesInspection, PageFontResourcesInspection, PdfName,
};
use presslint_selectors::Selector;
use presslint_types::PageIndex;

/// Paint-facing PDF name (distinct from the structural `presslint_pdf::PdfName`
/// used by the report builders).
fn paint_name(bytes: &[u8]) -> presslint_types::PdfName {
    presslint_types::PdfName(bytes.to_vec())
}

use crate::alias_epoch_plan::{
    AliasEpochOutcome, AliasEpochPlan, AliasEpochReport, EpochRefusalReason, EpochStatus, LaneSide,
};
use crate::link_routing::{LinkRouting, build_link_routing};
use crate::page_content_sequence::{OccurrenceInput, PageContentSequence};
use crate::page_device_space_policy::{PageColorFacts, PageDeviceSpacePolicy};
use crate::page_font_policy::PageFontPolicy;
use crate::page_xobject_policy::PageXObjectPolicy;
use crate::{
    BlackPreservationPolicy, ConvertContentColorsRequest, DeviceLinkInput, PageSelection,
    convert_content_colors_incremental,
};

use super::content_color_convert::{
    GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, assemble_classic, contains, convert, link_bytes,
    page_decoded_stream, stream_body,
};
use super::page_device_space_policy::{absent_defaults, three_aliases};

const CAP: usize = 1 << 20;

fn object(number: u32) -> IndirectRef {
    IndirectRef {
        object_number: number,
        generation: 0,
    }
}

// --- Report builders -----------------------------------------------------------

/// One classified indirect font resource with the given subtype and `/Type
/// /Font` fact.
fn indirect_font(
    name: &[u8],
    object_number: u32,
    object_byte_offset: usize,
    subtype: FontSubtypeClass,
) -> ClassifiedFontResource {
    ClassifiedFontResource {
        name: PdfName(name.to_vec()),
        dictionary_type: FontDictionaryTypeFact::Font,
        subtype,
        reference: Some(object(object_number)),
        object_byte_offset: Some(object_byte_offset),
    }
}

/// A direct (non-indirect) font dictionary: no reached tuple.
fn direct_font(name: &[u8], subtype: FontSubtypeClass) -> ClassifiedFontResource {
    ClassifiedFontResource {
        name: PdfName(name.to_vec()),
        dictionary_type: FontDictionaryTypeFact::Font,
        subtype,
        reference: None,
        object_byte_offset: None,
    }
}

fn font_report(fonts: Vec<ClassifiedFontResource>) -> PageFontResourcesInspection {
    PageFontResourcesInspection {
        ordinal: 0,
        page_reference: object(3),
        page_object_byte_offset: 30,
        fonts,
        skipped: Vec::new(),
    }
}

/// A classified `ExtGState` resource with every safety parameter unset and the
/// given `/Font` effect.
fn extgstate_resource(
    name: &[u8],
    font_effect: ExtGStateFontEffect,
) -> ClassifiedExtGStateResource {
    ClassifiedExtGStateResource {
        name: PdfName(name.to_vec()),
        op_stroking: ExtGStateParamClass::Unset,
        op_nonstroking: ExtGStateParamClass::Unset,
        overprint_mode: ExtGStateParamClass::Unset,
        stroking_alpha: ExtGStateParamClass::Unset,
        nonstroking_alpha: ExtGStateParamClass::Unset,
        blend_mode: ExtGStateParamClass::Unset,
        soft_mask: ExtGStateParamClass::Unset,
        has_unclassified_keys: false,
        font_effect,
    }
}

fn extgstate_report(
    extgstates: Vec<ClassifiedExtGStateResource>,
) -> PageExtGStateResourcesInspection {
    PageExtGStateResourcesInspection {
        ordinal: 0,
        page_reference: object(3),
        page_object_byte_offset: 30,
        extgstates,
        skipped: Vec::new(),
    }
}

fn structurally_valid(
    object_number: u32,
    object_byte_offset: usize,
    size: f64,
    subtype: FontSubtypeClass,
) -> ExtGStateFontEffect {
    ExtGStateFontEffect::StructurallyValid {
        reference: object(object_number),
        object_byte_offset,
        size_bits: size.to_bits(),
        dictionary_type: FontDictionaryTypeFact::Font,
        subtype,
    }
}

fn resolved(object_number: u32, generation: u16, object_byte_offset: usize) -> FontSelectionState {
    FontSelectionState::ResolvedIndirect {
        object_number,
        generation,
        object_byte_offset,
        size: 12.0,
    }
}

// --- Policy-unit: admission ----------------------------------------------------

#[test]
fn ordinary_indirect_subtypes_admit_and_others_refuse() {
    let ordinary = [
        FontSubtypeClass::Type1,
        FontSubtypeClass::MmType1,
        FontSubtypeClass::TrueType,
        FontSubtypeClass::Type0,
    ];
    for (index, subtype) in ordinary.into_iter().enumerate() {
        let object_number = 10 + u32::try_from(index).unwrap();
        let report = font_report(vec![indirect_font(
            b"F1",
            object_number,
            100,
            subtype.clone(),
        )]);
        let policy = PageFontPolicy::new(Some(&report), None);
        assert!(
            policy.admits(&resolved(object_number, 0, 100)),
            "{subtype:?} must admit"
        );
    }

    let refused = [
        FontSubtypeClass::Type3,
        FontSubtypeClass::CidFontType0,
        FontSubtypeClass::CidFontType2,
        FontSubtypeClass::OtherName {
            name: PdfName(b"Type1C".to_vec()),
        },
        FontSubtypeClass::Missing,
    ];
    for subtype in refused {
        let report = font_report(vec![indirect_font(b"F1", 7, 100, subtype.clone())]);
        let policy = PageFontPolicy::new(Some(&report), None);
        assert!(
            !policy.admits(&resolved(7, 0, 100)),
            "{subtype:?} must refuse"
        );
    }
}

#[test]
fn bad_type_missing_ref_and_direct_dictionary_never_admit() {
    // `/Subtype /Type1` but `/Type` not `/Font`.
    let bad_type = ClassifiedFontResource {
        name: PdfName(b"F1".to_vec()),
        dictionary_type: FontDictionaryTypeFact::Missing,
        subtype: FontSubtypeClass::Type1,
        reference: Some(object(7)),
        object_byte_offset: Some(100),
    };
    assert!(
        !PageFontPolicy::new(Some(&font_report(vec![bad_type])), None).admits(&resolved(7, 0, 100))
    );

    // Direct dictionary: no reached tuple, so nothing admits.
    let direct = font_report(vec![direct_font(b"F1", FontSubtypeClass::Type1)]);
    let policy = PageFontPolicy::new(Some(&direct), None);
    assert!(!policy.admits(&resolved(0, 0, 0)));
    assert!(!policy.admits(&resolved(7, 0, 100)));
}

#[test]
fn admits_only_resolved_indirect_with_exact_tuple() {
    let report = font_report(vec![indirect_font(b"F1", 7, 100, FontSubtypeClass::Type1)]);
    let policy = PageFontPolicy::new(Some(&report), None);

    assert!(policy.admits(&resolved(7, 0, 100)));
    // A newer reached offset for the same reference diverges and is not admitted.
    assert!(!policy.admits(&resolved(7, 0, 101)));
    assert!(!policy.admits(&resolved(7, 1, 100)));
    assert!(!policy.admits(&resolved(8, 0, 100)));
    // Non-resolved selections never admit.
    assert!(!policy.admits(&FontSelectionState::Unset));
    assert!(!policy.admits(&FontSelectionState::Indeterminate));
    assert!(!policy.admits(&FontSelectionState::Selected {
        name: paint_name(b"F1"),
        size: 12.0,
    }));
}

#[test]
fn aliases_to_one_safe_tuple_converge_and_a_conflicting_fact_poisons() {
    // Two names to the same safe tuple converge; the tuple stays admitted.
    let converge = font_report(vec![
        indirect_font(b"F1", 7, 100, FontSubtypeClass::Type1),
        indirect_font(b"F2", 7, 100, FontSubtypeClass::Type1),
    ]);
    assert!(PageFontPolicy::new(Some(&converge), None).admits(&resolved(7, 0, 100)));

    // A Type3 fact for the same exact tuple poisons it fail-closed.
    let conflict = font_report(vec![
        indirect_font(b"F1", 7, 100, FontSubtypeClass::Type1),
        indirect_font(b"F2", 7, 100, FontSubtypeClass::Type3),
    ]);
    assert!(!PageFontPolicy::new(Some(&conflict), None).admits(&resolved(7, 0, 100)));
}

#[test]
fn direct_extgstate_font_effects_feed_and_poison_the_admitted_set() {
    // An exact ordinary structurally-valid gs /Font effect admits.
    let ordinary = extgstate_report(vec![extgstate_resource(
        b"GS0",
        structurally_valid(9, 200, 12.0, FontSubtypeClass::Type1),
    )]);
    assert!(PageFontPolicy::new(None, Some(&ordinary)).admits(&resolved(9, 0, 200)));

    // A Type3 structurally-valid gs /Font effect never admits.
    let type3 = extgstate_report(vec![extgstate_resource(
        b"GS0",
        structurally_valid(9, 200, 12.0, FontSubtypeClass::Type3),
    )]);
    assert!(!PageFontPolicy::new(None, Some(&type3)).admits(&resolved(9, 0, 200)));

    // Same tuple reached safely through Tf and unsafely through gs poisons it.
    let report_fonts = font_report(vec![indirect_font(b"F1", 9, 200, FontSubtypeClass::Type1)]);
    assert!(!PageFontPolicy::new(Some(&report_fonts), Some(&type3)).admits(&resolved(9, 0, 200)));
}

#[test]
fn font_env_coverage_states_are_distinct() {
    // Unknown coverage: a None report.
    let unknown = PageFontPolicy::new(None, None);
    assert!(unknown.font_env().resolve(&paint_name(b"F1")).is_none());

    // Known-empty: a report with no fonts still resolves an absent name to a
    // definite "not present" — distinct from disabled/unknown behaviour.
    let empty = PageFontPolicy::new(Some(&font_report(Vec::new())), None);
    assert!(empty.font_env().resolve(&paint_name(b"F1")).is_none());

    // Known-valid: /F1 resolves to the exact ordinary indirect target.
    let report = font_report(vec![indirect_font(b"F1", 7, 100, FontSubtypeClass::Type1)]);
    let policy = PageFontPolicy::new(Some(&report), None);
    assert!(policy.font_env().resolve(&paint_name(b"F1")).is_some());
}

#[test]
fn semantic_extgstate_collision_poisons_the_font_directive() {
    // /GS1 and /GS#31 decode to the same semantic name; both directives poison
    // so a raw-name gs cannot first-win one over the other.
    let report = extgstate_report(vec![
        extgstate_resource(
            b"GS1",
            structurally_valid(9, 200, 12.0, FontSubtypeClass::Type1),
        ),
        extgstate_resource(
            b"GS#31",
            structurally_valid(11, 300, 12.0, FontSubtypeClass::Type1),
        ),
    ]);
    let policy = PageFontPolicy::new(None, Some(&report));
    let env = policy.extgstate_env();
    for raw in [b"GS1".as_slice(), b"GS#31".as_slice()] {
        let resource = env.resolve(&paint_name(raw)).expect("resource present");
        assert_eq!(
            resource.font,
            ExtGStateFontDirective::Unknown,
            "{} directive must poison",
            String::from_utf8_lossy(raw)
        );
    }
    // A lone escaped spelling still maps its directive normally.
    let lone = extgstate_report(vec![extgstate_resource(
        b"GS#31",
        structurally_valid(11, 300, 12.0, FontSubtypeClass::Type1),
    )]);
    let policy = PageFontPolicy::new(None, Some(&lone));
    assert!(matches!(
        policy
            .extgstate_env()
            .resolve(&paint_name(b"GS#31"))
            .expect("present")
            .font,
        ExtGStateFontDirective::Select { .. }
    ));
}

#[test]
fn extgstate_skips_poison_the_mapped_font_directive() {
    use presslint_pdf::{
        DictionaryValueKind, SkippedExtGStateResource, SkippedExtGStateResourceReason,
    };

    let selecting = || {
        extgstate_resource(
            b"GS0",
            structurally_valid(9, 200, 12.0, FontSubtypeClass::Type1),
        )
    };
    let directive = |report: &PageExtGStateResourcesInspection| {
        PageFontPolicy::new(None, Some(report))
            .extgstate_env()
            .resolve(&paint_name(b"GS0"))
            .expect("present")
            .font
    };

    // A named skip decoding to the same semantic name (`/GS#30` -> `GS0`) as a
    // surviving classified resource poisons that resource's `/Font` directive, so
    // a `gs` on it clears font certainty rather than keeping a positive binding.
    let named = PageExtGStateResourcesInspection {
        ordinal: 0,
        page_reference: object(3),
        page_object_byte_offset: 30,
        extgstates: vec![selecting()],
        skipped: vec![SkippedExtGStateResource {
            object_byte_offset: 30,
            resource_name: Some(PdfName(b"GS#30".to_vec())),
            reason: SkippedExtGStateResourceReason::NonDictionaryEntry {
                value_kind: DictionaryValueKind::Boolean,
            },
        }],
    };
    assert_eq!(directive(&named), ExtGStateFontDirective::Unknown);

    // A nameless skip is namespace-level incompleteness: every mapped directive
    // poisons fail-closed.
    let nameless = PageExtGStateResourcesInspection {
        ordinal: 0,
        page_reference: object(3),
        page_object_byte_offset: 30,
        extgstates: vec![selecting()],
        skipped: vec![SkippedExtGStateResource {
            object_byte_offset: 30,
            resource_name: None,
            reason: SkippedExtGStateResourceReason::MissingExtGState,
        }],
    };
    assert_eq!(directive(&nameless), ExtGStateFontDirective::Unknown);
}

// --- Plan-level: run one all-environments walk exactly as the converter does ---

fn standard_policy() -> PageDeviceSpacePolicy {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    PageDeviceSpacePolicy::from_page_facts(&PageColorFacts {
        color_spaces: Some(&color_spaces),
        defaults: Some(&defaults),
    })
}

fn gray_routing() -> LinkRouting {
    let inputs = vec![DeviceLinkInput {
        id: None,
        bytes: link_bytes(GRAY_TO_GRAY_LINK),
    }];
    build_link_routing(&inputs).expect("routing builds")
}

/// A font policy admitting `/F1` -> the ordinary indirect Type1 tuple
/// `(7, 0, 100)`, exactly the tuple `resolved(7, 0, 100)` carries.
fn f1_policy() -> PageFontPolicy {
    PageFontPolicy::new(
        Some(&font_report(vec![indirect_font(
            b"F1",
            7,
            100,
            FontSubtypeClass::Type1,
        )])),
        None,
    )
}

fn sequence_of(streams: &[&[u8]]) -> PageContentSequence {
    let inputs: Vec<OccurrenceInput<'_>> = streams
        .iter()
        .enumerate()
        .map(|(ordinal, decoded)| OccurrenceInput {
            stream_ordinal: ordinal,
            // Each physical content stream is a distinct indirect object, exactly
            // as a `/Contents [4 0 R 5 0 R …]` array holds, so cross-stream state
            // is not modelled as repeated references to one object.
            content_object: object(4 + u32::try_from(ordinal).unwrap()),
            decoded,
            disposition: IndirectObjectEditDisposition::InPlaceMutation,
        })
        .collect();
    PageContentSequence::new(&inputs, CAP).expect("sequence parses")
}

/// The closed/refused epoch that lives on `side` (the plan-level lane tests set
/// up one alias per lane, so at most one epoch exists per side).
fn epoch_on(outcome: &AliasEpochOutcome, side: LaneSide) -> &AliasEpochReport {
    outcome
        .epochs
        .iter()
        .find(|epoch| epoch.side == side)
        .expect("epoch for side")
}

fn run_with_fonts(fonts: &PageFontPolicy, streams: &[&[u8]]) -> Option<AliasEpochOutcome> {
    let policy = standard_policy();
    let routing = gray_routing();
    let sequence = sequence_of(streams);
    let program = PaintProgram::with_all_envs(
        sequence.bytes(),
        sequence.records(),
        policy.color_space_env(),
        fonts.extgstate_env(),
        fonts.font_env(),
    );
    let xobjects = PageXObjectPolicy::new(None);
    let target: Option<&Selector> = None;
    let mut plan = AliasEpochPlan::new(
        &policy,
        &routing,
        &xobjects,
        fonts,
        target,
        PageIndex(0),
        &sequence,
    );
    let mut previous = Rc::new(GraphicsStateSnapshot::page_default());
    for op in program.ops() {
        let op = op.ok()?;
        let state_before = std::mem::replace(&mut previous, Rc::clone(&op.state));
        let operator = &sequence.bytes()[op.operator_range.start()..op.operator_range.end()];
        plan.observe(&op, operator, &state_before, &sequence);
    }
    Some(plan.finish())
}

fn run(streams: &[&[u8]]) -> AliasEpochOutcome {
    run_with_fonts(&f1_policy(), streams).expect("walk succeeds")
}

fn first(outcome: &AliasEpochOutcome) -> &AliasEpochReport {
    &outcome.epochs[0]
}

fn refusal(epoch: &AliasEpochReport) -> Option<EpochRefusalReason> {
    match epoch.status {
        EpochStatus::Closed => None,
        EpochStatus::Refused { reason, .. } => Some(reason),
    }
}

#[test]
fn nonstroking_lanes_consume_under_modes_zero_and_four() {
    for mode in ["0", "4"] {
        let stream =
            format!("/GrayAlias cs 0.5 sc BT /F1 12 Tf {mode} Tr (x) Tj ET\n").into_bytes();
        let outcome = run(&[&stream]);
        let epoch = first(&outcome);
        assert_eq!(epoch.status, EpochStatus::Closed, "mode {mode}");
        assert!(epoch.has_consumer, "mode {mode} nonstroking consumer");
        assert_eq!(epoch.side, LaneSide::Nonstroking);
    }
}

#[test]
fn stroking_lanes_consume_under_modes_one_and_five() {
    for mode in ["1", "5"] {
        let stream =
            format!("/GrayAlias CS 0.5 SC BT /F1 12 Tf {mode} Tr (x) Tj ET\n").into_bytes();
        let outcome = run(&[&stream]);
        let epoch = first(&outcome);
        assert_eq!(epoch.status, EpochStatus::Closed, "mode {mode}");
        assert!(epoch.has_consumer, "mode {mode} stroking consumer");
        assert_eq!(epoch.side, LaneSide::Stroking);
    }
}

#[test]
fn both_lanes_consume_under_modes_two_and_six() {
    for mode in ["2", "6"] {
        let stream =
            format!("/GrayAlias cs 0.5 sc /GrayAlias CS 0.6 SC BT /F1 12 Tf {mode} Tr (x) Tj ET\n")
                .into_bytes();
        let outcome = run(&[&stream]);
        assert_eq!(outcome.epochs.len(), 2);
        for epoch in &outcome.epochs {
            assert_eq!(epoch.status, EpochStatus::Closed, "mode {mode}");
            assert!(epoch.has_consumer, "mode {mode} both consumers");
        }
    }
}

#[test]
fn invisible_and_clip_modes_three_and_seven_consume_no_lane() {
    for mode in ["3", "7"] {
        let stream =
            format!("/GrayAlias cs 0.5 sc BT /F1 12 Tf {mode} Tr (x) Tj ET\n").into_bytes();
        let outcome = run(&[&stream]);
        let epoch = first(&outcome);
        assert_eq!(epoch.status, EpochStatus::Closed, "mode {mode} closes");
        assert!(!epoch.has_consumer, "mode {mode} authorizes no splice");
    }
}

#[test]
fn a_later_supported_consumer_consumes_a_still_live_no_consumer_root() {
    // Mode 3 shows nothing; a following 0 Tr text show consumes the root.
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf 3 Tr (x) Tj 0 Tr (y) Tj ET\n";
    let outcome = run(&[stream]);
    let epoch = first(&outcome);
    assert_eq!(epoch.status, EpochStatus::Closed);
    assert!(epoch.has_consumer);
}

#[test]
fn unsupported_render_mode_value_refuses_text_show() {
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf 8 Tr (x) Tj ET\n";
    let outcome = run(&[stream]);
    assert_eq!(refusal(first(&outcome)), Some(EpochRefusalReason::TextShow));
}

#[test]
fn all_four_text_show_operators_consume_an_admitted_font() {
    for show in ["(x) Tj", "[(x)] TJ", "(x) '", "1 2 (x) \""] {
        let stream = format!("/GrayAlias cs 0.5 sc BT /F1 12 Tf 0 Tr {show} ET\n").into_bytes();
        let outcome = run(&[&stream]);
        let epoch = first(&outcome);
        assert_eq!(epoch.status, EpochStatus::Closed, "{show}");
        assert!(epoch.has_consumer, "{show} consumes");
    }
}

/// The full Tr 0-7 lane truth table crossed with all four PDF text-show
/// operators. Each stream selects one alias per lane, so the mode's consumption
/// is read independently on each side. Every combination closes both epochs;
/// only the mode decides which side records a consumer.
#[test]
fn every_show_operator_consumes_exactly_the_lanes_its_mode_selects() {
    // (mode, consumes nonstroking, consumes stroking).
    let modes = [
        ("0", true, false),
        ("1", false, true),
        ("2", true, true),
        ("3", false, false),
        ("4", true, false),
        ("5", false, true),
        ("6", true, true),
        ("7", false, false),
    ];
    for show in ["(x) Tj", "[(x)] TJ", "(x) '", "1 2 (x) \""] {
        for (mode, fill, stroke) in modes {
            let stream = format!(
                "/GrayAlias cs 0.5 sc /GrayAlias CS 0.6 SC BT /F1 12 Tf {mode} Tr {show} ET\n"
            )
            .into_bytes();
            let outcome = run(&[&stream]);
            assert_eq!(outcome.epochs.len(), 2, "{show} mode {mode}");
            let nonstroking = epoch_on(&outcome, LaneSide::Nonstroking);
            let stroking = epoch_on(&outcome, LaneSide::Stroking);
            assert_eq!(
                nonstroking.status,
                EpochStatus::Closed,
                "{show} mode {mode} nonstroking closes"
            );
            assert_eq!(
                stroking.status,
                EpochStatus::Closed,
                "{show} mode {mode} stroking closes"
            );
            assert_eq!(
                nonstroking.has_consumer, fill,
                "{show} mode {mode} nonstroking consumer"
            );
            assert_eq!(
                stroking.has_consumer, stroke,
                "{show} mode {mode} stroking consumer"
            );
        }
    }
}

/// A `gs` whose `/ExtGState` proves no `/Font` effect (`Unset`, no unclassified
/// keys) maps to `LeaveUnchanged`: the prior `Tf` selection persists, so the
/// following show still consumes.
#[test]
fn gs_leaveunchanged_font_effect_preserves_the_prior_tf_selection() {
    let report = extgstate_report(vec![extgstate_resource(b"GS0", ExtGStateFontEffect::Unset)]);
    let fonts = PageFontPolicy::new(
        Some(&font_report(vec![indirect_font(
            b"F1",
            7,
            100,
            FontSubtypeClass::Type1,
        )])),
        Some(&report),
    );
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf /GS0 gs 0 Tr (x) Tj ET\n";
    let outcome = run_with_fonts(&fonts, &[stream]).expect("walk succeeds");
    let epoch = first(&outcome);
    assert_eq!(epoch.status, EpochStatus::Closed);
    assert!(epoch.has_consumer);
}

/// An uncertain `gs` (an `Unset` `/Font` effect carrying an unclassified key)
/// maps to `Unknown`: it clears font certainty, leaving the selection
/// indeterminate, so the following show refuses fail-closed.
#[test]
fn uncertain_gs_font_effect_clears_certainty_and_refuses_the_show() {
    let mut resource = extgstate_resource(b"GS0", ExtGStateFontEffect::Unset);
    resource.has_unclassified_keys = true;
    let fonts = PageFontPolicy::new(
        Some(&font_report(vec![indirect_font(
            b"F1",
            7,
            100,
            FontSubtypeClass::Type1,
        )])),
        Some(&extgstate_report(vec![resource])),
    );
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf /GS0 gs 0 Tr (x) Tj ET\n";
    let outcome = run_with_fonts(&fonts, &[stream]).expect("walk succeeds");
    assert_eq!(refusal(first(&outcome)), Some(EpochRefusalReason::TextShow));
}

/// `Tf`/`gs` selections are last-writer-wins: whichever selected the effective
/// font last decides admission. An admitted `Tf` overwritten by an unadmitted
/// Type3 `gs` refuses; the reverse order admits.
#[test]
fn tf_and_gs_font_selection_is_last_writer_wins() {
    // GS0 selects a Type3 target (never admitted); F1 is an admitted Type1.
    let report = extgstate_report(vec![extgstate_resource(
        b"GS0",
        structurally_valid(9, 200, 12.0, FontSubtypeClass::Type3),
    )]);
    let fonts = PageFontPolicy::new(
        Some(&font_report(vec![indirect_font(
            b"F1",
            7,
            100,
            FontSubtypeClass::Type1,
        )])),
        Some(&report),
    );

    // Tf admits, then gs overwrites with the Type3 target: refuse.
    let tf_then_gs = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf /GS0 gs 0 Tr (x) Tj ET\n";
    let outcome = run_with_fonts(&fonts, &[tf_then_gs]).expect("walk succeeds");
    assert_eq!(refusal(first(&outcome)), Some(EpochRefusalReason::TextShow));

    // gs selects the Type3 target, then Tf overwrites with the admitted F1: close.
    let gs_then_tf = b"/GrayAlias cs 0.5 sc BT /GS0 gs /F1 12 Tf 0 Tr (x) Tj ET\n";
    let outcome = run_with_fonts(&fonts, &[gs_then_tf]).expect("walk succeeds");
    let epoch = first(&outcome);
    assert_eq!(epoch.status, EpochStatus::Closed);
    assert!(epoch.has_consumer);
}

/// A same-tuple font reached through `Tf` and through a direct `gs /Font` effect
/// converges on one admitted identity: selecting either way admits the show.
#[test]
fn same_tuple_through_tf_and_gs_converges_and_admits() {
    // F1 and GS0 both reach the ordinary Type1 tuple (7, 0, 100).
    let report = extgstate_report(vec![extgstate_resource(
        b"GS0",
        structurally_valid(7, 100, 12.0, FontSubtypeClass::Type1),
    )]);
    let fonts = PageFontPolicy::new(
        Some(&font_report(vec![indirect_font(
            b"F1",
            7,
            100,
            FontSubtypeClass::Type1,
        )])),
        Some(&report),
    );
    for select in ["/F1 12 Tf", "/GS0 gs"] {
        let stream = format!("/GrayAlias cs 0.5 sc BT {select} 0 Tr (x) Tj ET\n").into_bytes();
        let outcome = run_with_fonts(&fonts, &[&stream]).expect("walk succeeds");
        let epoch = first(&outcome);
        assert_eq!(epoch.status, EpochStatus::Closed, "{select}");
        assert!(epoch.has_consumer, "{select} consumes");
    }
}

/// `q`/`Q` restore the paired lane state exactly: a value set inside the frame is
/// discarded on `Q`, and a text-show consumer after `Q` is proved against the
/// restored pending colour, not the intra-frame one.
#[test]
fn q_restores_the_lane_for_a_later_text_consumer() {
    let stream = b"/GrayAlias cs 0.5 sc q /GrayAlias cs 0.8 sc Q BT /F1 12 Tf 0 Tr (x) Tj ET\n";
    let outcome = run(&[stream]);
    // The root epoch selected before `q` closes with the post-`Q` consumer.
    let epoch = epoch_on(&outcome, LaneSide::Nonstroking);
    assert_eq!(epoch.status, EpochStatus::Closed);
    assert!(epoch.has_consumer);
}

/// `BT`/`ET` do not reset the font: a font selected in one text object is still
/// admitted when a later text object shows without re-selecting it.
#[test]
fn font_selection_persists_across_bt_et_boundaries() {
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf 0 Tr (x) Tj ET BT 0 Tr (y) Tj ET\n";
    let outcome = run(&[stream]);
    let epoch = first(&outcome);
    assert_eq!(epoch.status, EpochStatus::Closed);
    assert!(epoch.has_consumer);
}

/// An admitted font shown inside an OPEN path object (after `m`, before the
/// paint) refuses on invalid graphics-object placement, not as a font refusal.
#[test]
fn text_show_inside_an_open_path_refuses_invalid_context() {
    let stream = b"/GrayAlias cs 0.5 sc /F1 12 Tf 0 0 m (x) Tj f\n";
    let outcome = run(&[stream]);
    assert_eq!(
        refusal(first(&outcome)),
        Some(EpochRefusalReason::InvalidGraphicsObjectContext)
    );
}

#[test]
fn unadmitted_and_type3_fonts_keep_the_text_show_refusal() {
    // Unknown font namespace (None report): Tf -> indeterminate -> refuse.
    let unknown = PageFontPolicy::new(None, None);
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf 0 Tr (x) Tj ET\n";
    let outcome = run_with_fonts(&unknown, &[stream]).expect("walk succeeds");
    assert_eq!(refusal(first(&outcome)), Some(EpochRefusalReason::TextShow));

    // Type3 font resolves in the environment but is never admitted.
    let type3 = PageFontPolicy::new(
        Some(&font_report(vec![indirect_font(
            b"F1",
            7,
            100,
            FontSubtypeClass::Type3,
        )])),
        None,
    );
    let outcome = run_with_fonts(&type3, &[stream]).expect("walk succeeds");
    assert_eq!(refusal(first(&outcome)), Some(EpochRefusalReason::TextShow));
}

#[test]
fn text_show_outside_a_text_object_refuses_the_invalid_context() {
    // Admitted font, but the show is outside BT/ET: invalid graphics-object
    // placement, refused the structural way rather than as a font refusal.
    let stream = b"/GrayAlias cs 0.5 sc /F1 12 Tf (x) Tj f\n";
    let outcome = run(&[stream]);
    assert_eq!(
        refusal(first(&outcome)),
        Some(EpochRefusalReason::InvalidGraphicsObjectContext)
    );
}

#[test]
fn font_and_mode_state_cross_physical_content_stream_boundaries() {
    // The text object, font, and mode are established in stream 0 and the show
    // op lands in stream 1; the epoch still closes with a consumer.
    let outcome = run(&[b"/GrayAlias cs 0.5 sc BT /F1 12 Tf 0 Tr ", b"(x) Tj ET\n"]);
    let epoch = first(&outcome);
    assert_eq!(epoch.status, EpochStatus::Closed);
    assert!(epoch.has_consumer);
}

#[test]
fn gs_direct_font_effect_admits_text_show() {
    // A gs /Font effect selects the exact ordinary indirect font; the following
    // text show consumes the alias exactly like a Tf-selected font would.
    let report = extgstate_report(vec![extgstate_resource(
        b"GS0",
        structurally_valid(7, 100, 12.0, FontSubtypeClass::Type1),
    )]);
    let fonts = PageFontPolicy::new(None, Some(&report));
    let stream = b"/GrayAlias cs 0.5 sc BT /GS0 gs 0 Tr (x) Tj ET\n";
    let outcome = run_with_fonts(&fonts, &[stream]).expect("walk succeeds");
    let epoch = first(&outcome);
    assert_eq!(epoch.status, EpochStatus::Closed);
    assert!(epoch.has_consumer);
}

// --- End-to-end: ordinary text conversion through the append pipeline ----------

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>";

fn font_page_pdf(stream: &[u8], font_body: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /ColorSpace << /GrayAlias /DeviceGray >> /Font << /F1 5 0 R >> >> >>"
            .to_vec(),
        stream_body("", stream),
        font_body.to_vec(),
    ])
}

#[test]
fn ordinary_text_show_closes_and_converts_the_alias_while_text_bytes_stay_verbatim() {
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf (x) Tj ET\n";
    let input = font_page_pdf(
        stream,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    // The alias selection record and its setter both convert.
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());

    let decoded = page_decoded_stream(&output.bytes, false);
    // No alias resource name survives, but every text byte is verbatim.
    assert!(!contains(&decoded, b"Alias"));
    assert!(contains(&decoded, b"BT /F1 12 Tf (x) Tj ET"));
}

#[test]
fn type3_text_show_refuses_and_holds_the_alias_bytes_verbatim() {
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf (x) Tj ET\n";
    let input = font_page_pdf(
        stream,
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] >>",
    );
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    // The structural setter tally is unchanged, but the epoch refuses so no
    // alias candidate converts.
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());

    let decoded = page_decoded_stream(&output.bytes, false);
    // The alias selection and setter stay byte-verbatim.
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(contains(&decoded, b"BT /F1 12 Tf (x) Tj ET"));
}

#[test]
fn mode_four_text_show_converts_the_alias_and_keeps_text_bytes_verbatim() {
    // Mode 4 (fill + clip) consumes the nonstroking lane exactly like mode 0, so
    // the alias converts; the interpreted clip half changes no text or clip byte.
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf 4 Tr (x) Tj ET\n";
    let input = font_page_pdf(
        stream,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"Alias"));
    assert!(contains(&decoded, b"BT /F1 12 Tf 4 Tr (x) Tj ET"));
}

#[test]
fn mode_seven_clip_only_text_show_holds_every_byte_verbatim() {
    // Mode 7 (clip only) consumes no lane, so the root has no consumer and
    // authorizes no splice: the alias selection AND the text bytes stay verbatim.
    let stream = b"/GrayAlias cs 0.5 sc BT /F1 12 Tf 7 Tr (x) Tj ET\n";
    let input = font_page_pdf(
        stream,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(contains(&decoded, b"BT /F1 12 Tf 7 Tr (x) Tj ET"));
}

#[test]
fn missing_font_report_leaves_text_pages_convertible_without_a_skip() {
    // A page whose walked content has no text dependency still converts its
    // direct shortcut even though this page declares no /Font namespace.
    let stream = b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f\n1 0 0 rg\n";
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
          /Resources << /ColorSpace << /GrayAlias /DeviceGray >> >> >>"
            .to_vec(),
        stream_body("", stream),
    ]);
    let output = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: vec![
                DeviceLinkInput {
                    id: None,
                    bytes: link_bytes(GRAY_TO_GRAY_LINK),
                },
                DeviceLinkInput {
                    id: None,
                    bytes: link_bytes(RGB_TO_CMYK_LINK),
                },
            ],
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds");

    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    assert!(output.converted[0].operators_converted >= 1);
}
