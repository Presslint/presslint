//! Form-local proven-neutral `gs`/`ExtGState` admission (T191).
//!
//! These matrices exercise the `gs` gate the analyzer gained: the raw
//! single-name-operand grammar, the demand-built bounded decoded-name
//! `/Resources /ExtGState` authority (canonical-key proof, collision/
//! named-skip/literal/nameless poisoning, the fact and operand-spelling caps),
//! the exact neutrality predicate over the shipped classifier facts (including
//! the `has_unclassified_keys` gate), the no-fallback scope rule, cache and
//! charge behaviour, T190 recursion composition, and the end-to-end page-only
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

/// One fully-neutral `ExtGState` binding: every present safety parameter is
/// exactly its safe value and no other key exists.
const NEUTRAL: &str = "/GS0 << /CA 1.0 /BM /Compatible /SMask /None >>";

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

/// Like [`form_pdf`], but the page dictionary declares its own `/Resources`.
fn form_pdf_with_page_resources(resources: &str, objects: &[Vec<u8>]) -> Vec<u8> {
    let mut bodies = vec![
        CATALOG.to_vec(),
        PAGES.to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << {resources} >> >>"
        )
        .into_bytes(),
        stream_body("", b""),
    ];
    bodies.extend_from_slice(objects);
    assemble_classic(&bodies)
}

/// A raw form stream with an extra dictionary fragment and a content body.
fn form(dict_extra: &str, content: &[u8]) -> Vec<u8> {
    stream_body(&format!("{FORM_BASE}{dict_extra}"), content)
}

/// A raw form declaring a Form-local `/Resources /ExtGState` sub-dictionary.
fn gsform(extgstates: &str, content: &[u8]) -> Vec<u8> {
    form(
        &format!(" /Resources << /ExtGState << {extgstates} >> >>"),
        content,
    )
}

/// A raw form declaring a Form-local `/Resources /XObject` sub-dictionary.
fn xform(xobjects: &str, content: &[u8]) -> Vec<u8> {
    form(
        &format!(" /Resources << /XObject << {xobjects} >> >>"),
        content,
    )
}

/// Analyze object 5 of an assembled document in a fresh request analyzer.
fn analyze_document(input: &[u8]) -> Option<[bool; 2]> {
    let access = inspect_document_access(input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset = object_offset(lookup, 5);
    FormXObjectEffectAnalyzer::new().analyze(
        input,
        lookup,
        IndirectRef {
            object_number: 5,
            generation: 0,
        },
        offset,
    )
}

/// Analyze object 5 (the first supplied body) in a fresh request analyzer.
fn analyze(objects: &[Vec<u8>]) -> Option<[bool; 2]> {
    analyze_document(&form_pdf(objects))
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

// --- Admission and neutrality matrix ------------------------------------------

#[test]
fn a_proven_neutral_gs_is_inert_for_the_lane_question() {
    // The admitted `gs` neither consumes nor resurrects a lane: the summary is
    // exactly the path truth of the remaining program.
    let cases: [(&[u8], [bool; 2]); 5] = [
        (b"/GS0 gs", [false, false]),
        (b"/GS0 gs 0 0 m 1 1 l f", [false, true]),
        (b"/GS0 gs 0 0 m 1 1 l S", [true, false]),
        (b"q /GS0 gs Q 0 0 m 1 1 l B", [true, true]),
        (b"0.5 g /GS0 gs 0 0 m 1 1 l f", [false, false]),
    ];
    for (content, expected) in cases {
        assert_eq!(
            analyze(&[gsform(NEUTRAL, content)]),
            Some(expected),
            "{}",
            String::from_utf8_lossy(content)
        );
    }
}

#[test]
fn the_parameter_matrix_matches_the_page_guard_entry_for_entry() {
    // Admissible: absent parameters, explicit `false` overprint flags, exactly
    // opaque alpha, Normal/Compatible blend, `/None` soft mask, and `/Type`.
    for entry in [
        "<< >>",
        "<< /Type /ExtGState >>",
        "<< /OP false /op false >>",
        "<< /O#50 false /o#70 false >>",
        "<< /CA 1.0 /ca 1.0 >>",
        "<< /BM /Normal >>",
        "<< /BM /Compatible >>",
        "<< /SMask /None >>",
        "<< /CA 1.0 /BM /Compatible /SMask /None >>",
    ] {
        assert_eq!(
            analyze(&[gsform(&format!("/GS0 {entry}"), b"/GS0 gs 0 0 m 1 1 l f")]),
            Some([false, true]),
            "{entry}"
        );
    }
    // Refused: ANY set `/OPM` (including `0`), `true` overprint flags,
    // non-opaque alpha, non-Normal/Compatible or unclassified blend, present
    // non-None soft mask, malformed safety params, and any unclassified key.
    for entry in [
        "<< /OPM 0 >>",
        "<< /OPM 1 >>",
        "<< /OPM 2 >>",
        "<< /OP true >>",
        "<< /op true >>",
        "<< /OP 1 >>",
        "<< /CA 0.5 >>",
        "<< /ca 0.99 >>",
        "<< /CA (x) >>",
        "<< /BM /Multiply >>",
        "<< /BM [/Normal] >>",
        "<< /SMask << /S /Alpha >> >>",
        "<< /LW 1 >>",
    ] {
        assert_eq!(
            analyze(&[gsform(&format!("/GS0 {entry}"), b"/GS0 gs 0 0 m 1 1 l f")]),
            None,
            "{entry}"
        );
    }
}

#[test]
fn any_font_effect_or_uniquely_escaped_unsafe_key_refuses() {
    // A structurally valid `/Font` is still a font effect: `font_effect` is not
    // `Unset`, and the aggregate unclassified-key flag fires as well.
    assert_eq!(
        analyze(&[
            gsform("/GS0 << /Font [6 0 R 12] >>", b"/GS0 gs"),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        ]),
        None
    );
    // A malformed `/Font` value refuses through the same two gates.
    assert_eq!(analyze(&[gsform("/GS0 << /Font (x) >>", b"/GS0 gs")]), None);
    // A uniquely escaped safety key is dispatched by its decoded semantic
    // spelling, so this explicit true value refuses as active overprint.
    assert_eq!(
        analyze(&[gsform("/GS0 << /O#50 true >>", b"/GS0 gs")]),
        None
    );
}

#[test]
fn duplicate_safety_keys_refuse_without_first_or_last_wins_recovery() {
    for entry in [
        // Both orders of the unsafe-first/safe-last examples that originally
        // exposed false-neutral classifier facts.
        "<< /OP true /OP false >>",
        "<< /OP false /OP true >>",
        "<< /CA 0.5 /CA 1 >>",
        "<< /CA 1 /CA 0.5 >>",
        "<< /BM /Multiply /BM /Normal >>",
        "<< /BM /Normal /BM /Multiply >>",
        // Identical-value duplicates still violate semantic uniqueness for
        // every safety key and therefore remain unknowable.
        "<< /OP false /OP false >>",
        "<< /op false /op false >>",
        "<< /OPM 0 /OPM 0 >>",
        "<< /CA 1 /CA 1 >>",
        "<< /ca 1 /ca 1 >>",
        "<< /BM /Normal /BM /Normal >>",
        "<< /SMask /None /SMask /None >>",
        // Raw and escaped spellings decode to one key and collide.
        "<< /OP false /O#50 false >>",
    ] {
        assert_eq!(
            analyze(&[gsform(&format!("/GS0 {entry}"), b"/GS0 gs")]),
            None,
            "{entry}"
        );
    }
}

// --- Scope, authority and poisoning --------------------------------------------

#[test]
fn a_gs_finds_no_page_or_caller_fallback() {
    // The page defines a fully-neutral `/GS0`, but the Form has no own
    // `/ExtGState`: the activation refuses — scopes are never merged.
    let page_resources = "/ExtGState << /GS0 << /CA 1.0 >> >>";
    assert_eq!(
        analyze_document(&form_pdf_with_page_resources(
            page_resources,
            &[form("", b"/GS0 gs 0 0 m 1 1 l f")],
        )),
        None
    );
    // An own `/Resources` without `/ExtGState` refuses the same way.
    assert_eq!(
        analyze_document(&form_pdf_with_page_resources(
            page_resources,
            &[form(" /Resources << >>", b"/GS0 gs 0 0 m 1 1 l f")],
        )),
        None
    );
}

#[test]
fn escaped_or_duplicate_resources_and_extgstate_authority_refuses() {
    for dict in [
        // Semantic duplicate top-level `/Resources` under an escaped spelling.
        " /Resources << /ExtGState << /GS0 << >> >> >> /Resour#63es << >>",
        // Semantic duplicate nested `/ExtGState` under an escaped spelling.
        " /Resources << /ExtGState << /GS0 << >> >> /ExtG#53tate << >> >>",
        // An escaped-only `/ExtGState` key can never be canonical authority.
        " /Resources << /E#78tGState << /GS0 << >> >> >>",
        // A malformed spelling whose valid decoded prefix may hide /ExtGState.
        " /Resources << /ExtGState << /GS0 << >> >> /ExtGStat#GG << >> >>",
    ] {
        assert_eq!(analyze(&[form(dict, b"/GS0 gs")]), None, "{dict}");
    }
    // A malformed sibling whose valid prefix cannot denote `/ExtGState` stays
    // isolated from the proven canonical authority.
    assert_eq!(
        analyze(&[form(
            " /Resources << /ExtGState << /GS0 << >> >> /Other#GG null >>",
            b"/GS0 gs",
        )]),
        Some([false, false])
    );
}

#[test]
fn escaped_operand_and_resource_spellings_resolve_through_decoded_equality() {
    // Escaped resource key, canonical operand (#53 == 'S').
    assert_eq!(
        analyze(&[gsform("/G#53 << >>", b"/GS gs")]),
        Some([false, false])
    );
    // Canonical resource key, escaped operand.
    assert_eq!(
        analyze(&[gsform("/GS << >>", b"/G#53 gs")]),
        Some([false, false])
    );
}

#[test]
fn decode_equal_collisions_and_literal_poison_refuse_without_first_win() {
    // `/GS1` and `/GS#31` decode to one semantic name: whichever declaration
    // is the safe one, the activated name refuses, never first-win.
    for entries in [
        "/GS1 << >> /GS#31 << /OPM 1 >>",
        "/GS1 << /OPM 1 >> /GS#31 << >>",
    ] {
        assert_eq!(analyze(&[gsform(entries, b"/GS1 gs")]), None, "{entries}");
    }
    // The unrelated proven sibling stays isolated from the collision.
    assert_eq!(
        analyze(&[gsform(
            "/GS1 << >> /GS#31 << /OPM 1 >> /Other << >>",
            b"/Other gs",
        )]),
        Some([false, false])
    );
    // An undecodable classified spelling colliding with a decoded operand
    // retains literal poison; an unrelated proven name stays isolated.
    let entries = "/Bad#GG << >> /Good << >>";
    assert_eq!(analyze(&[gsform(entries, b"/Bad#23GG gs")]), None);
    assert_eq!(
        analyze(&[gsform(entries, b"/Good gs")]),
        Some([false, false])
    );
}

#[test]
fn matching_named_skips_poison_the_name_and_structural_gaps_the_namespace() {
    // A non-dictionary entry value is a named structural skip: its name
    // refuses while the neutral sibling stays proven.
    let entries = "/Bad (x) /Good << >>";
    assert_eq!(analyze(&[gsform(entries, b"/Bad gs")]), None);
    assert_eq!(
        analyze(&[gsform(entries, b"/Good gs")]),
        Some([false, false])
    );
    // A raw duplicate resource name is a named skip that overrides the
    // classified first entry, never first-win.
    assert_eq!(
        analyze(&[gsform("/GS0 << >> /GS0 << /OPM 1 >>", b"/GS0 gs")]),
        None
    );
    // A non-dictionary or indirect `/ExtGState` value is uncertain namespace
    // authority: no `gs` is admissible even under a matching neutral entry.
    assert_eq!(
        analyze(&[form(" /Resources << /ExtGState (bad) >>", b"/GS0 gs")]),
        None
    );
    assert_eq!(
        analyze(&[
            form(" /Resources << /ExtGState 6 0 R >>", b"/GS0 gs"),
            b"<< /GS0 << >> >>".to_vec(),
        ]),
        None
    );
}

#[test]
fn fact_and_operand_spelling_caps_bound_the_gate() {
    use std::fmt::Write as _;

    let mut entries = String::new();
    for index in 0..256 {
        write!(entries, "/K{index} << >> ").expect("write");
    }
    // Up to 256 classified entries stay within the fact cap; one more poisons
    // the whole namespace even for an otherwise proven name.
    assert_eq!(
        analyze(&[gsform(&entries, b"/K0 gs")]),
        Some([false, false])
    );
    let overflowing = format!("{entries}/K256 << >> ");
    assert_eq!(analyze(&[gsform(&overflowing, b"/K0 gs")]), None);
    // 256 distinct operand spellings validate; a 257th distinct spelling —
    // even one decoding to an already-proven name — overflows the cap.
    let mut content = String::new();
    for index in 0..256 {
        write!(content, "/K{index} gs ").expect("write");
    }
    assert_eq!(
        analyze(&[gsform(&entries, content.as_bytes())]),
        Some([false, false])
    );
    let overflowing_content = format!("{content}/K#30 gs");
    assert_eq!(
        analyze(&[gsform(&entries, overflowing_content.as_bytes())]),
        None
    );
}

#[test]
fn an_unused_unsafe_declaration_does_not_block_a_used_safe_entry() {
    let entries = "/Safe << >> /Unsafe << /OPM 1 /SMask << >> /LW 2 >>";
    assert_eq!(
        analyze(&[gsform(entries, b"/Safe gs 0 0 m 1 1 l f")]),
        Some([false, true])
    );
    assert_eq!(
        analyze(&[gsform(entries, b"/Unsafe gs 0 0 m 1 1 l f")]),
        None
    );
}

#[test]
fn generation_mismatch_and_unresolved_indirect_entries_refuse() {
    // An exact indirect neutral entry resolves and admits.
    assert_eq!(
        analyze(&[
            form(" /Resources << /ExtGState << /GS0 6 0 R >> >>", b"/GS0 gs"),
            b"<< /CA 1.0 >>".to_vec(),
        ]),
        Some([false, false])
    );
    // A generation mismatch against the in-use xref entry refuses.
    assert_eq!(
        analyze(&[
            form(" /Resources << /ExtGState << /GS0 6 1 R >> >>", b"/GS0 gs"),
            b"<< /CA 1.0 >>".to_vec(),
        ]),
        None
    );
    // An unresolvable reference refuses.
    assert_eq!(
        analyze(&[form(
            " /Resources << /ExtGState << /GS0 99 0 R >> >>",
            b"/GS0 gs",
        )]),
        None
    );
}

/// A classic PDF whose object-6 in-use xref entry deliberately points at the
/// object-7 header offset. The Form's `/ExtGState` binds `/GS0 6 0 R` and
/// `/Good 7 0 R`; the mispointed xref resolves `6 0 R` to a fully-neutral
/// dictionary whose header reads `7 0 obj`, while the real object 6 — which a
/// repairing reader may locate and activate instead — is unsafe.
fn pdf_with_mispointed_extgstate_entry(content: &[u8]) -> Vec<u8> {
    let bodies = [
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream_body("", b""),
        form(
            " /Resources << /ExtGState << /GS0 6 0 R /Good 7 0 R >> >>",
            content,
        ),
        b"<< /OPM 1 >>".to_vec(),
        b"<< /CA 1.0 >>".to_vec(),
    ];
    let mut buf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    // Corrupt only object 6's recorded offset so the xref binds `6 0 R` to
    // the neutral body headed `7 0 obj`.
    offsets[5] = offsets[6];
    let xref_offset = buf.len();
    let size = bodies.len() + 1;
    buf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF")
            .as_bytes(),
    );
    buf
}

#[test]
fn a_mispointed_xref_binding_never_classifies_the_wrong_objects_body() {
    // The entry resolves as in-use with a matching generation, and the
    // dictionary at the resolved offset is fully neutral — but its object
    // header identifies object 7, not the requested object 6. Header identity
    // is corroborated before any classified fact is trusted, so the
    // false-neutral binding refuses.
    assert_eq!(
        analyze_document(&pdf_with_mispointed_extgstate_entry(b"/GS0 gs")),
        None
    );
    // The mispointed binding poisons only its own name: the sibling whose
    // header identity corroborates stays proven neutral and admits.
    assert_eq!(
        analyze_document(&pdf_with_mispointed_extgstate_entry(b"/Good gs")),
        Some([false, false])
    );
}

/// Xref-stream PDF whose analyzed Form is uncompressed object 5 while its
/// named `/ExtGState` entry is the Type-2 member 6 of object stream 7.
fn xref_stream_pdf_with_compressed_extgstate_entry() -> Vec<u8> {
    let mut buf = b"%PDF-1.5\n".to_vec();
    let uncompressed = [
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream_body("", b""),
        form(" /Resources << /ExtGState << /GS0 6 0 R >> >>", b"/GS0 gs"),
    ];
    let mut offsets = Vec::new();
    for (index, body) in uncompressed.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }

    let member_header = b"6 0 ";
    let member = b"<< /CA 1.0 >>";
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
fn a_compressed_extgstate_entry_refuses_without_member_descent() {
    let input = xref_stream_pdf_with_compressed_extgstate_entry();
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    assert!(matches!(
        locate_xref_object(lookup, 5),
        ObjectLookupLocation::XrefStreamUncompressed { .. }
    ));
    assert!(matches!(
        locate_xref_object(lookup, 6),
        ObjectLookupLocation::XrefStreamCompressed { .. }
    ));
    assert_eq!(
        FormXObjectEffectAnalyzer::new().analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 5,
                generation: 0,
            },
            object_offset(lookup, 5),
        ),
        None
    );
}

// --- Demand, grammar and cache --------------------------------------------------

#[test]
fn a_malformed_extgstate_without_gs_never_builds_the_authority() {
    // A malformed or unsafe `/ExtGState` value with NO `gs` operator stays
    // admissible with the same lattice as the resource-less program.
    let baseline = analyze(&[form("", b"0 0 m 1 1 l f")]);
    assert_eq!(baseline, Some([false, true]));
    assert_eq!(
        analyze(&[form(" /Resources << /ExtGState (bad) >>", b"0 0 m 1 1 l f")]),
        baseline
    );
    assert_eq!(
        analyze(&[gsform("/GS0 << /OPM 1 >>", b"0 0 m 1 1 l f")]),
        baseline
    );
    // With the `gs` present the same malformed authority refuses.
    assert_eq!(
        analyze(&[form(" /Resources << /ExtGState (bad) >>", b"/GS0 gs")]),
        None
    );
}

#[test]
fn raw_gs_grammar_requires_exactly_one_name_outside_an_open_path() {
    assert_eq!(
        analyze(&[gsform(NEUTRAL, b"/GS0 gs")]),
        Some([false, false])
    );
    for content in [
        &b"gs"[..],
        b"/GS0 /GS1 gs",
        b"1 gs",
        b"(x) gs",
        b"0 0 m /GS0 gs 1 1 l f",
    ] {
        assert_eq!(
            analyze(&[gsform(NEUTRAL, content)]),
            None,
            "{}",
            String::from_utf8_lossy(content)
        );
    }
}

#[test]
fn a_gate_refusal_caches_as_all_unknown_without_a_second_charge() {
    let input = form_pdf(&[gsform("/GS0 << /OPM 1 >>", b"/GS0 gs 0 0 m 1 1 l f")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(1, FLATE_LIMIT);
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    let remaining = analyzer.remaining_bytes_for_test();
    assert!(
        remaining < FLATE_LIMIT,
        "the refused compute still read its decoded body once"
    );
    // The second query serves the cached all-Unknown lattice: the single
    // first-seen target is already spent and no byte recharges.
    assert_eq!(analyze_object(&mut analyzer, &input, lookup, 5), None);
    assert_eq!(analyzer.remaining_bytes_for_test(), remaining);
}

#[test]
fn raw_and_flate_gs_programs_share_the_same_summary() {
    let content = b"/GS0 gs 0 0 m 1 1 l f";
    assert_eq!(analyze(&[gsform(NEUTRAL, content)]), Some([false, true]));
    let compressed = encode_flate_stream(content, FLATE_LIMIT).expect("encode");
    assert_eq!(
        analyze(&[form(
            &format!(" /Resources << /ExtGState << {NEUTRAL} >> >> /Filter /FlateDecode"),
            &compressed,
        )]),
        Some([false, true])
    );
    assert_eq!(
        analyze(&[form(
            " /Resources << /ExtGState << /GS0 << /OPM 1 >> >> >> /Filter /FlateDecode",
            &compressed,
        )]),
        None
    );
}

// --- Recursion composition ------------------------------------------------------

#[test]
fn a_neutral_gs_child_composes_and_an_unsafe_gs_child_refuses_the_root() {
    // Depth two: root -> mid -> leaf; the leaf's admitted `gs` stays inert and
    // its fill composes onto the root's still-live inherited nonstroking lane.
    assert_eq!(
        analyze(&[
            xform("/N 6 0 R", b"/N Do"),
            xform("/N 7 0 R", b"/N Do"),
            gsform(NEUTRAL, b"/GS0 gs 0 0 m 1 1 l f"),
        ]),
        Some([false, true])
    );
    // An unsafe-`gs` child is Unknown and refuses the invoking root.
    assert_eq!(
        analyze(&[
            xform("/N 6 0 R", b"/N Do"),
            gsform("/GS0 << /OPM 1 >>", b"/GS0 gs 0 0 m 1 1 l f"),
        ]),
        None
    );
}

#[test]
fn gates_are_built_per_identity_from_each_forms_own_resources() {
    // The root defines and uses a neutral `/GS0`; the child activates `/GS0`
    // without defining it. The child never sees the root's entries and
    // refuses, making the root Unknown.
    let root = form(
        " /Resources << /ExtGState << /GS0 << /CA 1.0 >> >> /XObject << /N 6 0 R >> >>",
        b"/GS0 gs /N Do",
    );
    assert_eq!(
        analyze(&[root.clone(), form("", b"/GS0 gs 0 0 m 1 1 l f")]),
        None
    );
    // Root and child defining DIFFERENT entries under the same name are gated
    // per-identity: each proves against its own resources and composes.
    assert_eq!(
        analyze(&[
            root,
            gsform("/GS0 << /BM /Normal >>", b"/GS0 gs 0 0 m 1 1 l f"),
        ]),
        Some([false, true])
    );
}

#[test]
fn repeated_invocation_of_a_neutral_gs_child_charges_the_target_once() {
    let input = form_pdf(&[
        xform("/N 6 0 R", b"/N Do /N Do"),
        gsform(NEUTRAL, b"/GS0 gs 0 0 m 1 1 l f"),
    ]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // Two first-seen targets (root + child) suffice: the repeated `Do` serves
    // the child's cached lattice without a second decode or charge.
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(2, FLATE_LIMIT);
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 5),
        Some([false, true])
    );
    let remaining = analyzer.remaining_bytes_for_test();
    assert_eq!(
        analyze_object(&mut analyzer, &input, lookup, 5),
        Some([false, true])
    );
    assert_eq!(analyzer.remaining_bytes_for_test(), remaining);
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
fn a_neutral_gs_form_conversion_rewrites_only_the_page() {
    // The Form's admitted `gs` no longer blocks the lane proof: its fill
    // consumes the caller's inherited nonstroking colour, the page alias root
    // closes, and only the existing page setter converts. Every Form/resource/
    // ExtGState byte stays identical and unduplicated.
    let form_object = gsform(NEUTRAL, b"/GS0 gs 0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        std::slice::from_ref(&form_object),
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
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(output.converted.len(), 1);
    reopen(&output.bytes);
}

#[test]
fn an_unsafe_gs_form_retains_the_fail_closed_refusal_end_to_end() {
    let form_object = gsform("/GS0 << /OPM 1 >>", b"/GS0 gs 0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        std::slice::from_ref(&form_object),
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
    assert!(contains(&output.bytes, &form_object));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    reopen(&output.bytes);
}
