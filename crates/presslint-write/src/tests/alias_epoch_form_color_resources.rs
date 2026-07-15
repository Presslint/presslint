//! Root Form LOCAL-device colour effect analysis (T188).
//!
//! These matrices exercise the Form-local `CS`/`cs` + `SC`/`SCN`/`sc`/`scn`
//! admission the T187 analyzer gained: the decoded-name resource authority
//! (direct reserved names, unique aliases, collision/malformed/`Default*`/skip
//! poisoning, the fact cap), the ISO initial-colour and named-setter state
//! machine, `q`/`Q` restoration, and the retained refusal envelope. Analyzer UNIT
//! tests call [`FormXObjectEffectAnalyzer::analyze`] on a real Form reached
//! through the request `ObjectLookup`; the END-TO-END tests drive
//! `convert_content_colors_incremental` to lock the page-only mutation, the
//! unchanged Form/resource bytes, and the outer `Do` lane behaviour.

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
    GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, assemble_classic, contains, link_bytes, occurrence_count,
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

/// One-page classic PDF whose objects 5.. are the supplied form bodies.
fn form_pdf(forms: &[Vec<u8>]) -> Vec<u8> {
    let mut bodies = vec![
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream_body("", b""),
    ];
    bodies.extend_from_slice(forms);
    assemble_classic(&bodies)
}

/// A raw form stream with an extra dictionary fragment and a content body.
fn form(dict_extra: &str, content: &[u8]) -> Vec<u8> {
    stream_body(&format!("{FORM_BASE}{dict_extra}"), content)
}

/// A raw form carrying a Form-local `/Resources /ColorSpace` sub-dictionary.
fn cs_form(color_space: &str, content: &[u8]) -> Vec<u8> {
    form(
        &format!(" /Resources << /ColorSpace << {color_space} >> >>"),
        content,
    )
}

/// Analyze one form object (object 5) in a fresh request-scoped analyzer.
fn analyze(form_object: Vec<u8>) -> Option<[bool; 2]> {
    analyze_objects(&[form_object], 5)
}

/// Analyze one target from a PDF whose object bodies begin at object 5.
fn analyze_objects(objects: &[Vec<u8>], object_number: u32) -> Option<[bool; 2]> {
    let input = form_pdf(objects);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset = object_offset(lookup, object_number);
    FormXObjectEffectAnalyzer::new().analyze(
        &input,
        lookup,
        IndirectRef {
            object_number,
            generation: 0,
        },
        offset,
    )
}

// --- Semantic resource authority ---------------------------------------------

#[test]
fn direct_device_names_resolve_without_form_resources() {
    // A canonical direct selector needs no resource key: `/Default*` is provably
    // absent when there is no `/ColorSpace`, so each family selects and kills its
    // own lane, leaving a fill/stroke to read a local (non-consumed) colour.
    assert_eq!(
        analyze(form("", b"/DeviceGray cs 0 sc 0 0 m 1 1 l f")),
        Some([false, false])
    );
    assert_eq!(
        analyze(form("", b"/DeviceRGB CS 0 0 0 SC 0 0 m 1 1 l S")),
        Some([false, false])
    );
    assert_eq!(
        analyze(form("", b"/DeviceCMYK cs 0 0 0 1 scn 0 0 m 1 1 l f")),
        Some([false, false])
    );
}

#[test]
fn direct_device_names_cannot_be_shadowed_by_resource_keys() {
    for (resource, content) in [
        (
            "/DeviceGray /DeviceRGB",
            b"/DeviceGray cs 0 sc 0 0 m 1 1 l f".as_slice(),
        ),
        (
            "/DeviceRGB /DeviceGray",
            b"/DeviceRGB cs 0 0 0 sc 0 0 m 1 1 l f",
        ),
        (
            "/DeviceCMYK /DeviceGray",
            b"/DeviceCMYK cs 0 0 0 1 sc 0 0 m 1 1 l f",
        ),
    ] {
        assert_eq!(
            analyze(cs_form(resource, content)),
            Some([false, false]),
            "{}",
            String::from_utf8_lossy(content),
        );
    }
}

#[test]
fn unique_local_aliases_resolve_for_each_device_family() {
    let color_space = "/G /DeviceGray /R /DeviceRGB /C /DeviceCMYK";
    assert_eq!(
        analyze(cs_form(color_space, b"/G cs 0.5 sc 0 0 m 1 1 l f")),
        Some([false, false])
    );
    assert_eq!(
        analyze(cs_form(color_space, b"/R CS 0.1 0.2 0.3 SCN 0 0 m 1 1 l S")),
        Some([false, false])
    );
    assert_eq!(
        analyze(cs_form(color_space, b"/C cs 0 0 0 1 scn 0 0 m 1 1 l f")),
        Some([false, false])
    );
}

#[test]
fn two_distinct_aliases_to_one_family_are_independently_valid() {
    assert_eq!(
        analyze(cs_form(
            "/A /DeviceGray /B /DeviceGray",
            b"/A cs 0.2 sc /B cs 0.8 sc 0 0 m 1 1 l f"
        )),
        Some([false, false])
    );
}

#[test]
fn escaped_key_and_operand_spellings_resolve_through_decoded_equality() {
    // Escaped key, canonical operand (#41 == 'A').
    assert_eq!(
        analyze(cs_form(
            "/#41lias /DeviceGray",
            b"/Alias cs 0.5 sc 0 0 m 1 1 l f"
        )),
        Some([false, false])
    );
    // Canonical key, escaped operand.
    assert_eq!(
        analyze(cs_form(
            "/Alias /DeviceGray",
            b"/#41lias cs 0.5 sc 0 0 m 1 1 l f"
        )),
        Some([false, false])
    );
}

#[test]
fn semantic_duplicate_keys_poison_only_the_invoked_name() {
    let color_space = "/A /DeviceGray /#41 /DeviceGray /B /DeviceGray";
    // `/A` and `/#41` decode to the same name, poisoning it fail-closed.
    assert_eq!(
        analyze(cs_form(color_space, b"/A cs 0.5 sc 0 0 m 1 1 l f")),
        None
    );
    // The unrelated `/B` remains a proven Device alias.
    assert_eq!(
        analyze(cs_form(color_space, b"/B cs 0.5 sc 0 0 m 1 1 l f")),
        Some([false, false])
    );
}

#[test]
fn malformed_invoked_name_named_skip_and_nameless_skip_fail_closed() {
    // Malformed invoked-name escape: undecodable operand is never proven.
    assert_eq!(
        analyze(cs_form(
            "/G /DeviceGray",
            b"/Bad#GG cs 0.5 sc 0 0 m 1 1 l f"
        )),
        None
    );
    // A matching named skip poisons its name (an unsupported family classifies
    // as a per-entry skip carrying that name).
    assert_eq!(
        analyze(cs_form(
            "/Cal [/CalRGB << /WhitePoint [1 1 1] >>]",
            b"/Cal cs 0 0 0 sc 0 0 m 1 1 l f"
        )),
        None
    );
    // A nameless uncertain skip (a non-dictionary `/Resources`) poisons every
    // selection, including the reserved direct names.
    assert_eq!(
        analyze(form(
            " /Resources (bad)",
            b"/DeviceGray cs 0 sc 0 0 m 1 1 l f"
        )),
        None
    );
}

#[test]
fn escaped_duplicate_or_malformed_authority_cannot_hide_a_default() {
    let content = b"/DeviceGray cs 0 sc 0 0 m 1 1 l f";
    for dict in [
        // Escaped top-level authority key.
        " /Resour#63es << /ColorSpace << /DefaultGray /DeviceRGB >> >>",
        // Semantic duplicate top-level authority key.
        " /Resources << /ColorSpace << >> >> /Resour#63es << /ColorSpace << /DefaultGray /DeviceRGB >> >>",
        // Escaped nested authority key.
        " /Resources << /Color#53pace << /DefaultGray /DeviceRGB >> >>",
        // Semantic duplicate nested authority key.
        " /Resources << /ColorSpace << >> /Color#53pace << /DefaultGray /DeviceRGB >> >>",
        // A malformed nested authority peer makes absence ambiguous.
        " /Resources << /Color#GGSpace << /DefaultGray /DeviceRGB >> >>",
    ] {
        assert_eq!(analyze(form(dict, content)), None, "{dict}");
    }
}

#[test]
fn exact_indirect_resources_admit_but_ambiguous_indirect_authority_refuses() {
    let form_object = form(" /Resources 6 0 R", b"/Local cs 0.5 sc 0 0 m 1 1 l f");
    assert_eq!(
        analyze_objects(
            &[
                form_object.clone(),
                b"<< /ColorSpace << /Local /DeviceGray >> >>".to_vec(),
            ],
            5,
        ),
        Some([false, false])
    );
    assert_eq!(
        analyze_objects(
            &[
                form_object,
                b"<< /Color#53pace << /Local /DeviceGray /DefaultGray /DeviceRGB >> >>".to_vec(),
            ],
            5,
        ),
        None
    );
    assert_eq!(
        analyze(form(
            " /Resources 99 0 R",
            b"/DeviceGray cs 0 sc 0 0 m 1 1 l f",
        )),
        None
    );
}

#[test]
fn absent_resources_or_colorspace_refuses_an_alias_but_never_uses_page_fallback() {
    // No `/Resources`: an alias never resolves; a direct name still selects.
    assert_eq!(
        analyze(form("", b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f")),
        None
    );
    // `/Resources` without `/ColorSpace`: alias refused, direct name admitted.
    assert_eq!(
        analyze(form(
            " /Resources << /XObject << >> >>",
            b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f"
        )),
        None
    );
    assert_eq!(
        analyze(form(
            " /Resources << /XObject << >> >>",
            b"/DeviceGray cs 0 sc 0 0 m 1 1 l f"
        )),
        Some([false, false])
    );
}

#[test]
fn alias_to_alias_unsupported_family_and_pattern_refuse() {
    // Alias-to-alias: a bare-name value is not a direct Device classification.
    assert_eq!(
        analyze(cs_form("/A /R", b"/A cs 0.5 sc 0 0 m 1 1 l f")),
        None
    );
    // Unsupported special family.
    assert_eq!(
        analyze(cs_form(
            "/Icc [/CalGray << /WhitePoint [1 1 1] >>]",
            b"/Icc cs 0 sc 0 0 m 1 1 l f"
        )),
        None
    );
    // Pattern colour space.
    assert_eq!(
        analyze(cs_form("/Pat /Pattern", b"/Pat cs 0 0 m 1 1 l f")),
        None
    );
}

#[test]
fn relevant_default_facts_refuse_only_the_matching_family() {
    for (default_name, family, components) in [
        ("DefaultGray", "DeviceGray", "0"),
        ("DefaultRGB", "DeviceRGB", "0 0 0"),
        ("DefaultCMYK", "DeviceCMYK", "0 0 0 1"),
    ] {
        assert_eq!(
            analyze(cs_form(
                &format!("/{default_name} /DeviceGray"),
                format!("/{family} cs {components} sc 0 0 m 1 1 l f").as_bytes(),
            )),
            None,
            "{default_name}",
        );
    }
    // Canonical present `/DefaultGray` blocks the gray family; RGB stays live.
    assert_eq!(
        analyze(cs_form(
            "/DefaultGray /DeviceRGB",
            b"/DeviceGray cs 0 sc 0 0 m 1 1 l f"
        )),
        None
    );
    assert_eq!(
        analyze(cs_form(
            "/DefaultGray /DeviceRGB",
            b"/DeviceRGB cs 0 0 0 sc 0 0 m 1 1 l f"
        )),
        Some([false, false])
    );
    // Escaped `/Default#43MYK` (#43 == 'C') blocks the cmyk family.
    assert_eq!(
        analyze(cs_form(
            "/Default#43MYK /DeviceRGB",
            b"/DeviceCMYK cs 0 0 0 1 sc 0 0 m 1 1 l f"
        )),
        None
    );
    // A skipped (unclassifiable) `/DefaultRGB` is uncertainty, not absence.
    assert_eq!(
        analyze(cs_form(
            "/DefaultRGB [/CalRGB << >>]",
            b"/DeviceRGB cs 0 0 0 sc 0 0 m 1 1 l f"
        )),
        None
    );
    // A semantic duplicate cannot restore absence.
    assert_eq!(
        analyze(cs_form(
            "/DefaultRGB /DeviceGray /Default#52GB /DeviceGray",
            b"/DeviceRGB cs 0 0 0 sc 0 0 m 1 1 l f",
        )),
        None
    );
    // A malformed spelling whose decoded prefix may hide `/DefaultCMYK`
    // poisons only that family.
    assert_eq!(
        analyze(cs_form(
            "/DefaultCMYK#GG /DeviceGray",
            b"/DeviceCMYK cs 0 0 0 1 sc 0 0 m 1 1 l f",
        )),
        None
    );
    assert_eq!(
        analyze(cs_form(
            "/DefaultCMYK#GG /DeviceGray",
            b"/DeviceGray cs 0 sc 0 0 m 1 1 l f",
        )),
        Some([false, false])
    );
}

#[test]
fn unrelated_safe_or_malformed_names_do_not_poison_a_proven_name() {
    assert_eq!(
        analyze(cs_form(
            "/Good /DeviceGray /Other /DeviceRGB",
            b"/Good cs 0.5 sc 0 0 m 1 1 l f"
        )),
        Some([false, false])
    );
    assert_eq!(
        analyze(cs_form(
            "/Other#GG /DeviceRGB /Good /DeviceGray",
            b"/Good cs 0.5 sc 0 0 m 1 1 l f",
        )),
        Some([false, false])
    );
    assert_eq!(
        analyze(form(
            " /Resources << /Other#GG 1 /ColorSpace << /Good /DeviceGray >> >>",
            b"/Good cs 0.5 sc 0 0 m 1 1 l f",
        )),
        Some([false, false])
    );
}

#[test]
fn malformed_classified_and_skipped_names_retain_literal_poison() {
    // The valid operand decodes to the literal malformed resource spelling.
    let collided = b"/Bad#23GG cs 0.5 sc 0 0 m 1 1 l f";
    assert_eq!(
        analyze(cs_form("/Bad#GG /DeviceGray /Good /DeviceGray", collided,)),
        None
    );
    assert_eq!(
        analyze(cs_form(
            "/Bad#GG [/CalGray << >>] /Good /DeviceGray",
            collided,
        )),
        None
    );
    // The same malformed facts do not poison a distinct proven spelling.
    assert_eq!(
        analyze(cs_form(
            "/Bad#GG /DeviceGray /Good /DeviceGray",
            b"/Good cs 0.5 sc 0 0 m 1 1 l f",
        )),
        Some([false, false])
    );
}

#[test]
fn the_reported_fact_cap_exhaustion_returns_unknown() {
    // More than the 256-fact cap of Form-local colour spaces poisons the whole
    // projection to Unknown before any writer-local map is allocated.
    use std::fmt::Write as _;
    let mut color_space = String::new();
    for index in 0..300 {
        write!(color_space, "/K{index} /DeviceGray ").expect("write");
    }
    assert_eq!(
        analyze(cs_form(&color_space, b"/DeviceGray cs 0 sc 0 0 m 1 1 l f")),
        None
    );
}

#[test]
fn distinct_used_operand_spellings_have_their_own_256_entry_cap() {
    fn program(spellings: usize) -> Vec<u8> {
        use std::fmt::Write as _;
        let mut content = String::new();
        for mask in 0..spellings {
            content.push('/');
            for bit in 0..9 {
                if mask & (1 << bit) == 0 {
                    content.push('A');
                } else {
                    content.push_str("#41");
                }
            }
            content.push_str(" cs ");
        }
        write!(content, "0 0 m 1 1 l B").expect("write");
        content.into_bytes()
    }

    let color_space = "/AAAAAAAAA /DeviceGray";
    assert_eq!(
        analyze(cs_form(color_space, &program(256))),
        Some([true, false])
    );
    assert_eq!(analyze(cs_form(color_space, &program(257))), None);
}

// --- Colour-state machine ----------------------------------------------------

#[test]
fn cs_alone_kills_exactly_its_selected_lane() {
    // `cs` kills the nonstroking lane via its Device initial colour, so a
    // combined paint still consumes the untouched inherited stroking lane.
    assert_eq!(
        analyze(form("", b"/DeviceGray cs 0 0 m 1 1 l B")),
        Some([true, false])
    );
    // `CS` kills the stroking lane; the fill reads inherited nonstroking.
    assert_eq!(
        analyze(form("", b"/DeviceGray CS 0 0 m 1 1 l B")),
        Some([false, true])
    );
    for family in ["DeviceRGB", "DeviceCMYK"] {
        assert_eq!(
            analyze(form("", format!("/{family} cs 0 0 m 1 1 l B").as_bytes())),
            Some([true, false])
        );
        assert_eq!(
            analyze(form("", format!("/{family} CS 0 0 m 1 1 l B").as_bytes())),
            Some([false, true])
        );
    }
}

#[test]
fn named_setters_with_exact_arity_kill_their_lane() {
    for (family, components) in [
        ("DeviceGray", "0.5"),
        ("DeviceRGB", "0.1 0.2 0.3"),
        ("DeviceCMYK", "0 0 0 1"),
    ] {
        for (selector, setter, expected) in [
            ("cs", "sc", [true, false]),
            ("cs", "scn", [true, false]),
            ("CS", "SC", [false, true]),
            ("CS", "SCN", [false, true]),
        ] {
            let content = format!("/{family} {selector} {components} {setter} 0 0 m 1 1 l B");
            assert_eq!(
                analyze(form("", content.as_bytes())),
                Some(expected),
                "{content}",
            );
        }
    }
}

#[test]
fn a_named_setter_on_the_inherited_sentinel_refuses() {
    // No prior `cs`: the lane is the source-less inherited sentinel.
    assert_eq!(analyze(form("", b"0.5 sc 0 0 m 1 1 l f")), None);
    // Four-component `SC` over the CMYK-shaped inherited stroking sentinel must
    // never be admitted as a local DeviceCMYK lane.
    assert_eq!(analyze(form("", b"0 0 0 1 SC 0 0 m 1 1 l S")), None);
}

#[test]
fn wrong_arity_nonfinite_and_pattern_operands_refuse() {
    // Wrong arity for the prior lane.
    assert_eq!(
        analyze(form("", b"/DeviceGray cs 0.5 0.5 sc 0 0 m 1 1 l f")),
        None
    );
    assert_eq!(
        analyze(form("", b"/DeviceRGB cs 0.1 sc 0 0 m 1 1 l f")),
        None
    );
    // Non-finite numeric operand.
    let overflow = "9".repeat(400);
    assert_eq!(
        analyze(form(
            "",
            format!("/DeviceGray cs {overflow} sc 0 0 m 1 1 l f").as_bytes()
        )),
        None
    );
    // Trailing Pattern name operand.
    assert_eq!(
        analyze(form("", b"/DeviceGray cs 0.5 /P scn 0 0 m 1 1 l f")),
        None
    );
}

#[test]
fn q_cs_paint_qq_paint_consumes_the_restored_inherited_lane() {
    // The fill inside `q` reads a local nonstroking colour; after `Q` restores
    // the inherited sentinel, the later fill consumes it.
    assert_eq!(
        analyze(form("", b"q /DeviceGray cs 0 0 1 1 re f Q 0 0 1 1 re f")),
        Some([false, true])
    );
}

#[test]
fn nested_q_restores_exact_lanes_and_stack_errors_still_refuse() {
    assert_eq!(
        analyze(form("", b"q q /DeviceRGB cs Q 0 0 1 1 re f Q 0 0 1 1 re f",)),
        Some([false, true])
    );
    assert_eq!(analyze(form("", b"/DeviceGray cs Q")), None);
    assert_eq!(analyze(form("", b"q /DeviceGray cs")), None);
}

#[test]
fn colour_operators_inside_an_open_path_refuse() {
    assert_eq!(analyze(form("", b"0 0 m /DeviceGray cs 1 1 l f")), None);
    assert_eq!(
        analyze(form("", b"/DeviceGray cs 0 0 m 0.5 sc 1 1 l f")),
        None
    );
}

#[test]
fn an_unsupported_suffix_after_a_proven_local_effect_returns_unknown() {
    assert_eq!(
        analyze(form("", b"/DeviceGray cs 0.5 sc 0 0 m 1 1 l f")),
        Some([false, false])
    );
    assert_eq!(
        analyze(form("", b"/DeviceGray cs 0.5 sc 0 0 m 1 1 l f 1 w")),
        None
    );
}

// --- Cache, budgets and parity -----------------------------------------------

#[test]
fn positive_neutral_and_unknown_effects_cache_by_exact_target() {
    let input = form_pdf(&[
        // Positive: fill still consumes inherited nonstroking colour.
        cs_form("/FormGray /DeviceGray", b"/FormGray CS 0 SC 0 0 m 1 1 l f"),
        // Neutral: the selected local nonstroking lane is filled.
        cs_form("/FormGray /DeviceGray", b"/FormGray cs 0 sc 0 0 m 1 1 l f"),
        // Unknown: the alias is absent from the Form-local authority.
        cs_form("/FormGray /DeviceGray", b"/Missing cs 0 sc 0 0 m 1 1 l f"),
        // A fourth unseen target demonstrates the three-target cap is spent.
        form("", b"0 0 m 1 1 l f"),
    ]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(3, FLATE_LIMIT);
    let analyze_target = |analyzer: &mut FormXObjectEffectAnalyzer, object_number| {
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number,
                generation: 0,
            },
            object_offset(lookup, object_number),
        )
    };
    let expected = [Some([false, true]), Some([false, false]), None];
    for (object_number, effect) in (5..=7).zip(expected) {
        assert_eq!(analyze_target(&mut analyzer, object_number), effect);
    }
    // Hits for all three cache value shapes remain available after the target
    // cap is exhausted and do not recharge bytes or target attempts.
    for _ in 0..50 {
        for (object_number, effect) in (5..=7).zip(expected) {
            assert_eq!(analyze_target(&mut analyzer, object_number), effect);
        }
    }
    assert_eq!(analyze_target(&mut analyzer, 8), None);
}

#[test]
fn raw_and_flate_resource_colour_programs_share_the_same_summary() {
    let content = b"/FormGray CS 0 SC 0 0 m 1 1 l f";
    let raw = cs_form("/FormGray /DeviceGray", content);
    let compressed = encode_flate_stream(content, FLATE_LIMIT).expect("encode");
    let flate = form(
        " /Resources << /ColorSpace << /FormGray /DeviceGray >> >> /Filter /FlateDecode",
        &compressed,
    );
    assert_eq!(analyze(raw), Some([false, true]));
    assert_eq!(analyze(flate), Some([false, true]));
}

#[test]
fn a_resource_independent_form_is_admitted_without_resource_inspection() {
    // A Form with no resource-colour operator keeps its exact T187 summary.
    assert_eq!(analyze(form("", b"0 0 m 1 1 l f")), Some([false, true]));
    assert_eq!(analyze(form("", b"0 0 m 1 1 l S")), Some([true, false]));
}

// --- End-to-end page-only mutation -------------------------------------------

fn page_body(contents: &str, resources: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {contents} /Resources << {resources} >> >>"
    )
    .into_bytes()
}

fn resource_pdf(content: &[u8], resources: &str, form_object: Vec<u8>) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("4 0 R", resources),
        stream_body("", content),
        form_object,
    ])
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

#[test]
fn a_form_local_device_form_consuming_the_inherited_lane_converts_only_the_page_setter() {
    // The Form selects its OWN device space on the stroking lane but fills with
    // the caller's inherited nonstroking colour, so it consumes the page's
    // nonstroking alias root and the page setter converts. The Form and its own
    // `/Resources /ColorSpace` bytes stay byte-identical and unduplicated.
    let resources = "/ColorSpace << /GrayAlias /DeviceGray >> /XObject << /Fm 5 0 R >>";
    let form_object = cs_form("/FormGray /DeviceGray", b"/FormGray CS 0 SC 0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        resources,
        form_object.clone(),
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
    assert!(contains(&output.bytes, &form_object));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    reopen(&output.bytes);
}

#[test]
fn a_form_local_device_selection_that_kills_the_lane_leaves_the_root_live() {
    // The Form kills the nonstroking lane locally before filling, so it consumes
    // nothing and the page's nonstroking alias setter stays verbatim.
    let resources = "/ColorSpace << /GrayAlias /DeviceGray >> /XObject << /Fm 5 0 R >>";
    let form_object = cs_form("/FormGray /DeviceGray", b"/FormGray cs 0 sc 0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        resources,
        form_object.clone(),
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
    assert!(contains(&output.bytes, &form_object));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
}

#[test]
fn an_rgb_alias_consumed_through_a_form_local_device_form_routes_and_converts() {
    // A different family end-to-end: the page RGB alias is consumed by a Form
    // that only selects its own device space on the stroking lane.
    let resources = "/ColorSpace << /RgbAlias /DeviceRGB >> /XObject << /Fm 5 0 R >>";
    let form_object = cs_form(
        "/FormRgb /DeviceRGB",
        b"/FormRgb CS 0 0 0 SCN 0 0 m 1 1 l f",
    );
    let input = resource_pdf(
        b"/RgbAlias cs 0.1 0.2 0.3 scn /Fm Do\n",
        resources,
        form_object,
    );
    let output = convert_link(&input, RGB_TO_CMYK_LINK);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert!(!contains(
        &page_decoded_stream(&output.bytes, false),
        b"RgbAlias"
    ));
}

#[test]
fn a_same_spelled_page_alias_is_never_form_fallback() {
    let resources = "/ColorSpace << /Shared /DeviceGray >> /XObject << /Fm 5 0 R >>";
    let form_object = form("", b"/Shared CS 0 SC 0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/Shared cs 0.5 sc /Fm Do\n",
        resources,
        form_object.clone(),
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/Shared cs 0.5 sc",
    ));
    assert!(contains(&output.bytes, &form_object));

    // With a same-spelled Form-local binding, that binding is authoritative:
    // three-component `SC` proves local DeviceRGB. Page DeviceGray fallback
    // would make the Form Unknown on arity.
    let form_object = cs_form(
        "/Shared /DeviceRGB",
        b"/Shared CS 0.1 0.2 0.3 SC 0 0 m 1 1 l f",
    );
    let input = resource_pdf(
        b"/Shared cs 0.5 sc /Fm Do\n",
        resources,
        form_object.clone(),
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert_eq!(output.converted[0].resource_alias_candidates_refused, 0);
    assert!(contains(&output.bytes, &form_object));
}

#[test]
fn repeated_exact_form_across_aliases_and_pages_reuses_request_analysis() {
    let pages = b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>";
    let page_one_resources = "/ColorSpace << /GrayOne /DeviceGray >> /XObject << /One 7 0 R >>";
    let page_two_resources = "/ColorSpace << /GrayTwo /DeviceGray >> /XObject << /Two 7 0 R >>";
    let form_object = cs_form("/Local /DeviceGray", b"/Local CS 0 SC 0 0 m 1 1 l f");
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        pages.to_vec(),
        page_body("5 0 R", page_one_resources),
        page_body("6 0 R", page_two_resources),
        stream_body("", b"/GrayOne cs 0.2 sc /One Do\n"),
        stream_body("", b"/GrayTwo cs 0.8 sc /Two Do\n"),
        form_object.clone(),
    ]);
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted.len(), 2);
    assert!(
        output
            .converted
            .iter()
            .all(|page| page.resource_alias_candidates_converted == 2)
    );
    assert!(
        output
            .converted
            .iter()
            .all(|page| page.resource_alias_candidates_refused == 0)
    );
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(contains(&output.bytes, &form_object));
    assert_eq!(occurrence_count(&output.bytes, b"7 0 obj"), 1);
    reopen(&output.bytes);
}

#[test]
fn indirect_form_resources_and_form_body_remain_byte_identical() {
    let page_resources = "/ColorSpace << /GrayAlias /DeviceGray >> /XObject << /Fm 5 0 R >>";
    let form_object = form(" /Resources 6 0 R", b"/FormGray CS 0 SC 0 0 m 1 1 l f");
    let resource_object = b"<< /ColorSpace << /FormGray /DeviceGray >> >>".to_vec();
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("4 0 R", page_resources),
        stream_body("", b"/GrayAlias cs 0.5 sc /Fm Do\n"),
        form_object.clone(),
        resource_object.clone(),
    ]);
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(contains(&output.bytes, &form_object));
    assert!(contains(&output.bytes, &resource_object));
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"6 0 obj"), 1);
    reopen(&output.bytes);
}

#[test]
fn a_form_local_unsupported_resource_colour_retains_the_fail_closed_refusal() {
    // The Form reaches an unsupported (ICC-alternate/Cal) local colour space, so
    // its exact analysis stays Unknown: the outer `Do` keeps the fail-closed
    // refusal, the page alias setter stays verbatim, and every Form byte is kept.
    let resources = "/ColorSpace << /GrayAlias /DeviceGray >> /XObject << /Fm 5 0 R >>";
    let form_object = cs_form(
        "/Cal [/CalRGB << /WhitePoint [1 1 1] >>]",
        b"/Cal cs 0 0 0 sc 0 0 m 1 1 l f",
    );
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        resources,
        form_object.clone(),
    );
    let output = convert_link(&input, GRAY_TO_GRAY_LINK);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(contains(&output.bytes, &form_object));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
}
