//! Root Form inherited-colour effect analysis, policy folding, and end-to-end
//! alias admission.
//!
//! Two layers are exercised. The analyzer UNIT tests call
//! [`FormXObjectEffectAnalyzer::analyze`] directly on a real form object reached
//! through the request `ObjectLookup`, covering raw/Flate equivalence, the raw
//! allowlist grammar, the refusal graph, the lane truth table, the group
//! boundary, exact cache identity, and the request budgets. The END-TO-END tests
//! drive `convert_content_colors_incremental` to lock the semantic-name folding,
//! the outer `Do` lane behaviour, page-only mutation, unchanged Form bytes, and
//! the retained refusal envelope.

use std::cell::RefCell;

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    DeviceLinkInput, PageSelection, convert_content_colors_incremental,
    form_xobject_effect::FormXObjectEffectAnalyzer,
    page_xobject_policy::{PageXObjectEffect, PageXObjectPolicy},
};
use presslint_pdf::{
    DocumentAccessBackend, IndirectRef, ObjectLookup, ObjectLookupLocation,
    PageXObjectResourceTarget, PageXObjectResourcesInspection, PdfName as ResourceName,
    SkippedPageXObjectResource, SkippedPageXObjectResourceReason, encode_flate_stream,
    inspect_document_access, locate_xref_object,
};
use presslint_types::PdfName;

use super::content_color_convert::{
    GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, assemble_classic, contains, link_bytes, occurrence_count,
    page_decoded_stream, stream_body,
};
use super::reopen;

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
const GRAY_ALIAS: &str = "/ColorSpace << /GrayAlias /DeviceGray >>";
const FORM_DICT: &str = " /Type /XObject /Subtype /Form /BBox [0 0 100 100]";
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

/// Analyze one form object in a fresh analyzer.
fn analyze_form(form: Vec<u8>) -> Option<[bool; 2]> {
    let input = form_pdf(&[form]);
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

fn form_target(
    name: &[u8],
    object_number: u32,
    object_byte_offset: usize,
) -> PageXObjectResourceTarget {
    PageXObjectResourceTarget {
        name: ResourceName(name.to_vec()),
        reference: IndirectRef {
            object_number,
            generation: 0,
        },
        object_byte_offset,
        image_metadata: None,
    }
}

fn form_report(form_xobjects: Vec<PageXObjectResourceTarget>) -> PageXObjectResourcesInspection {
    form_report_with_skips(form_xobjects, Vec::new())
}

fn form_report_with_skips(
    form_xobjects: Vec<PageXObjectResourceTarget>,
    skipped: Vec<SkippedPageXObjectResource>,
) -> PageXObjectResourcesInspection {
    PageXObjectResourcesInspection {
        ordinal: 0,
        page_reference: IndirectRef {
            object_number: 3,
            generation: 0,
        },
        page_object_byte_offset: 20,
        image_xobject_names: Vec::new(),
        form_xobject_names: form_xobjects
            .iter()
            .map(|target| target.name.clone())
            .collect(),
        image_xobjects: Vec::new(),
        form_xobjects,
        skipped,
    }
}

fn raw_form(content: &[u8]) -> Vec<u8> {
    stream_body(FORM_DICT, content)
}

fn raw_form_dict(dict_extra: &str, content: &[u8]) -> Vec<u8> {
    stream_body(&format!("{FORM_DICT}{dict_extra}"), content)
}

fn flate_form(content: &[u8]) -> Vec<u8> {
    let compressed = encode_flate_stream(content, FLATE_LIMIT).expect("encode");
    stream_body(&format!("{FORM_DICT} /Filter /FlateDecode"), &compressed)
}

// --- Raw/Flate equivalence and filter admission ------------------------------

#[test]
fn raw_and_flate_forms_share_the_same_lane_summary() {
    let content = b"0 0 m 1 1 l f";
    assert_eq!(analyze_form(raw_form(content)), Some([false, true]));
    assert_eq!(analyze_form(flate_form(content)), Some([false, true]));
}

#[test]
fn unsupported_filters_chains_predictors_and_malformed_streams_are_unknown() {
    // Unsupported single filter.
    assert_eq!(
        analyze_form(raw_form_dict(" /Filter /ASCIIHexDecode", b"30 30 6d")),
        None
    );
    // Filter chain.
    assert_eq!(
        analyze_form(raw_form_dict(
            " /Filter [/ASCIIHexDecode /FlateDecode]",
            b"x"
        )),
        None
    );
    // FlateDecode with a non-default predictor.
    assert_eq!(
        analyze_form(raw_form_dict(
            " /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 >>",
            &encode_flate_stream(b"0 0 m 1 1 l f", FLATE_LIMIT).expect("encode")
        )),
        None
    );
    // Declared FlateDecode over bytes that are not valid zlib.
    assert_eq!(
        analyze_form(raw_form_dict(" /Filter /FlateDecode", b"not zlib")),
        None
    );
    // Canonical default DecodeParms remains delegated and admitted.
    assert_eq!(
        analyze_form(raw_form_dict(
            " /Filter /FlateDecode /DecodeParms << /Predictor 1 >>",
            &encode_flate_stream(b"0 0 m 1 1 l f", FLATE_LIMIT).expect("encode")
        )),
        Some([false, true])
    );
}

#[test]
fn semantic_dictionary_preflight_refuses_execution_aliases_and_safety_key_evasion() {
    // These entries can change which program a reader paints, whether it paints
    // at all, or which raw-key inspector sees the controlling stream metadata.
    // Canonical and escaped spellings are semantically identical PDF names.
    for extra in [
        " /F (external.pdf)",
        " /#46 (external.pdf)",
        " /Ref << /Page 1 >>",
        " /R#65f << /Page 1 >>",
        " /OC << /Type /OCG >>",
        " /O#43 << /Type /OCG >>",
        " /OPI << /Version 2.0 >>",
        " /O#50I << /Version 2.0 >>",
        " /Gr#6Fup << /S /Transparency >>",
        " /Fil#74er /ASCIIHexDecode",
        " /DecodeP#61rms null",
        " /Len#67th 13",
        " /Bad#GG 1",
    ] {
        assert_eq!(
            analyze_form(raw_form_dict(extra, b"0 0 m 1 1 l f")),
            None,
            "expected semantic dictionary refusal for {extra}"
        );
    }

    // Semantic duplicates of every raw-key-delegated safety key refuse before
    // filter classification, extent lookup, group inspection, or body access.
    for extra in [
        " /Group null /Gr#6Fup null",
        " /Filter /FlateDecode /Fil#74er /FlateDecode",
        " /DecodeParms null /DecodeP#61rms null",
        " /Length 13 /Len#67th 13",
    ] {
        assert_eq!(
            analyze_form(raw_form_dict(extra, b"0 0 m 1 1 l f")),
            None,
            "expected semantic duplicate refusal for {extra}"
        );
    }
}

// --- Raw allowlist grammar ---------------------------------------------------

#[test]
fn raw_allowlist_accepts_the_closed_grammar() {
    for content in [
        b"".as_slice(),
        b"q Q",
        b"1 0 0 1 5 5 cm",
        b"0 0 m 1 1 l 2 2 3 3 4 4 c h n",
        b"0 0 100 100 re W n",
        b"0.2 G 0.3 0.4 0.5 rg 0 0 m 1 1 l S",
    ] {
        assert!(
            analyze_form(raw_form(content)).is_some(),
            "expected admission for {:?}",
            String::from_utf8_lossy(content)
        );
    }
}

#[test]
fn raw_grammar_rejects_arity_context_and_balance_violations() {
    for content in [
        b"0 0 0 m 1 1 l f".as_slice(), // wrong arity for m
        b"1 1 l S",                    // continuation without an open path
        b"0 0 m 1 1 l q",              // q inside an open path
        b"0 0 m 1 1 l 0 G",            // device setter inside an open path
        b"Q",                          // q-stack underflow
        b"q",                          // unbalanced save at stream end
        b"0 0 m 1 1 l",                // open path at stream end
        b"/Name m 1 1 l f",            // non-number operand
        b"n",                          // paint without an open path
    ] {
        assert_eq!(
            analyze_form(raw_form(content)),
            None,
            "expected refusal for {:?}",
            String::from_utf8_lossy(content)
        );
    }
}

#[test]
fn overflowing_path_operands_are_unknown_for_every_numeric_path_shape() {
    let overflow = "9".repeat(400);
    for content in [
        format!("{overflow} 0 m n"),
        format!("0 0 m {overflow} 0 l n"),
        format!("0 0 m {overflow} 0 0 0 0 0 c n"),
        format!("0 0 m {overflow} 0 0 0 v n"),
        format!("0 0 m {overflow} 0 0 0 y n"),
        format!("{overflow} 0 1 1 re n"),
    ] {
        assert_eq!(
            analyze_form(raw_form(content.as_bytes())),
            None,
            "overflowing path operand was admitted for {content}"
        );
    }
}

// --- Refusal graph -----------------------------------------------------------

#[test]
fn every_unsupported_operator_is_unknown() {
    for content in [
        b"/CS0 cs 0 0 m 1 1 l f".as_slice(), // resource colour space
        b"0 0 m 1 1 l 0.5 scn f",            // resource colour value
        b"1 w 0 0 m 1 1 l f",                // line-state operator
        b"0 J 0 0 m 1 1 l f",                // line cap
        b"/Ri ri 0 0 m 1 1 l f",             // rendering intent
        b"/GS0 gs 0 0 m 1 1 l f",            // ExtGState
        b"BT (x) Tj ET",                     // text
        b"/Fm Do",                           // nested Do
        b"/Sh sh",                           // shading
        b"BI EI",                            // inline image
        b"BX EX 0 0 m 1 1 l f",              // compatibility section
        b"/Tag BMC EMC",                     // marked content
        b"0 0 d0",                           // Type3 glyph metric
        b"zz",                               // unknown operator
    ] {
        assert_eq!(
            analyze_form(raw_form(content)),
            None,
            "expected refusal for {:?}",
            String::from_utf8_lossy(content)
        );
    }
}

#[test]
fn an_unsupported_suffix_erases_a_positive_prefix() {
    // A valid fill paint prefix followed by an unsupported line-state operator
    // never survives as a positive effect.
    assert_eq!(
        analyze_form(raw_form(b"0 0 m 1 1 l f")),
        Some([false, true])
    );
    assert_eq!(analyze_form(raw_form(b"0 0 m 1 1 l f 1 w")), None);
}

// --- Lane truth table --------------------------------------------------------

#[test]
fn lane_truth_table_maps_each_paint_to_its_inherited_lanes() {
    let cases: [(&[u8], [bool; 2]); 12] = [
        (b"0 0 m 1 1 l S", [true, false]),
        (b"0 0 m 1 1 l s", [true, false]),
        (b"0 0 m 1 1 l f", [false, true]),
        (b"0 0 m 1 1 l F", [false, true]),
        (b"0 0 m 1 1 l f*", [false, true]),
        (b"0 0 m 1 1 l B", [true, true]),
        (b"0 0 m 1 1 l B*", [true, true]),
        (b"0 0 m 1 1 l b", [true, true]),
        (b"0 0 m 1 1 l b*", [true, true]),
        (b"0 0 m 1 1 l n", [false, false]),
        (b"0 0 m 1 1 l W n", [false, false]),
        (b"1 0 0 1 0 0 cm", [false, false]),
    ];
    for (content, expected) in cases {
        assert_eq!(
            analyze_form(raw_form(content)),
            Some(expected),
            "{:?}",
            String::from_utf8_lossy(content)
        );
    }
}

// --- Setter liveness ---------------------------------------------------------

#[test]
fn direct_setters_kill_their_own_inherited_lane() {
    // A device setter on the painted lane kills inheritance; the other lane may
    // still be inherited.
    assert_eq!(
        analyze_form(raw_form(b"0 G 0 0 m 1 1 l S")),
        Some([false, false])
    );
    assert_eq!(
        analyze_form(raw_form(b"0 g 0 0 m 1 1 l f")),
        Some([false, false])
    );
    assert_eq!(
        analyze_form(raw_form(b"0.1 0.2 0.3 RG 0 0 m 1 1 l S")),
        Some([false, false])
    );
    // Combined paint: stroking is local (killed), nonstroking is inherited.
    assert_eq!(
        analyze_form(raw_form(b"0 G 0 0 m 1 1 l B")),
        Some([false, true])
    );
}

#[test]
fn save_restore_restores_a_killed_inherited_sentinel() {
    assert_eq!(
        analyze_form(raw_form(b"q 0 G Q 0 0 m 1 1 l S")),
        Some([true, false])
    );
    // A nested save around a nonstroking kill does not leak: after both Q the
    // fill still reads the inherited nonstroking sentinel.
    assert_eq!(
        analyze_form(raw_form(b"q q 0 g Q Q 0 0 m 1 1 l f")),
        Some([false, true])
    );
}

#[test]
fn a_numerically_sentinel_valued_setter_cannot_recreate_inheritance() {
    // The stroking sentinel is DeviceCMYK [0.5 0.25 0.125 0.0625]; a setter with
    // the identical components carries a real source range, so it is NOT the
    // source-less inherited sentinel and the lane reads as local.
    assert_eq!(
        analyze_form(raw_form(b"0.5 0.25 0.125 0.0625 K 0 0 m 1 1 l S")),
        Some([false, false])
    );
}

// --- Group boundary ----------------------------------------------------------

#[test]
fn a_transparency_group_or_unresolved_group_is_unknown() {
    // Proven-absent group: analyzable.
    assert_eq!(
        analyze_form(raw_form(b"0 0 m 1 1 l f")),
        Some([false, true])
    );
    // Valid transparency group.
    assert_eq!(
        analyze_form(raw_form_dict(
            " /Group << /S /Transparency >>",
            b"0 0 m 1 1 l f"
        )),
        None
    );
    // Indirect-unresolved group reference.
    assert_eq!(
        analyze_form(raw_form_dict(" /Group 99 0 R", b"0 0 m 1 1 l f")),
        None
    );
    // Malformed group: a `/Group` value that is neither a dictionary nor an
    // indirect reference.
    assert_eq!(
        analyze_form(raw_form_dict(" /Group 42", b"0 0 m 1 1 l f")),
        None
    );
    // Duplicate `/Group` keys make the effective group ambiguous.
    assert_eq!(
        analyze_form(raw_form_dict(
            " /Group << /S /Transparency >> /Group << /S /Transparency >>",
            b"0 0 m 1 1 l f"
        )),
        None
    );
}

// --- Reference XObject boundary ----------------------------------------------

#[test]
fn a_reference_xobject_ref_is_unknown_even_with_a_neutral_proxy() {
    // A reference XObject imports an external page whose content — not this local
    // proxy stream — is painted, so even a colour-consuming or neutral proxy body
    // must be Unknown. Present, malformed, and duplicate `/Ref` all refuse.
    assert_eq!(
        analyze_form(raw_form_dict(
            " /Ref << /F 9 0 R /Page 1 >>",
            b"0 0 m 1 1 l f"
        )),
        None
    );
    // Malformed `/Ref` value shape.
    assert_eq!(
        analyze_form(raw_form_dict(" /Ref 42", b"0 0 m 1 1 l f")),
        None
    );
    // Duplicate `/Ref` keys.
    assert_eq!(
        analyze_form(raw_form_dict(
            " /Ref << /Page 1 >> /Ref << /Page 2 >>",
            b"0 0 m 1 1 l f"
        )),
        None
    );
    // A neutral proxy body with `/Ref` is still Unknown, never a false Neutral.
    assert_eq!(
        analyze_form(raw_form_dict(" /Ref << /Page 1 >>", b"")),
        None
    );
}

// --- Exact cache identity ----------------------------------------------------

#[test]
fn generation_and_offset_mismatch_and_missing_targets_are_unknown() {
    let input = form_pdf(&[raw_form(b"0 0 m 1 1 l f")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset = object_offset(lookup, 5);
    let mut analyzer = FormXObjectEffectAnalyzer::new();

    // Exact identity corroborates.
    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 5,
                generation: 0
            },
            offset
        ),
        Some([false, true])
    );
    // Generation mismatch.
    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 5,
                generation: 1
            },
            offset
        ),
        None
    );
    // Offset mismatch.
    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 5,
                generation: 0
            },
            offset + 1
        ),
        None
    );
    // Missing object number.
    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 99,
                generation: 0
            },
            offset
        ),
        None
    );
}

#[test]
fn a_cached_identity_is_reused_without_recharging_budget() {
    let input = form_pdf(&[raw_form(b"0 0 m 1 1 l f")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset = object_offset(lookup, 5);
    let reference = IndirectRef {
        object_number: 5,
        generation: 0,
    };
    // A one-target budget: the first analysis consumes the only attempt, but the
    // same exact identity keeps returning the cached result across many calls.
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(1, FLATE_LIMIT);
    let first = analyzer.analyze(&input, lookup, reference, offset);
    assert_eq!(first, Some([false, true]));
    for _ in 0..300 {
        assert_eq!(analyzer.analyze(&input, lookup, reference, offset), first);
    }
}

#[test]
fn lazy_policy_skips_uninvoked_forms_and_reuses_one_exact_target_across_names() {
    let input = form_pdf(&[raw_form(b"0 0 m 1 1 l S"), raw_form(b"0 0 m 1 1 l f")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let unused_offset = object_offset(lookup, 5);
    let shared_offset = object_offset(lookup, 6);
    let report = form_report(vec![
        form_target(b"Unused", 5, unused_offset),
        form_target(b"A", 6, shared_offset),
        form_target(b"B", 6, shared_offset),
    ]);
    let analyzer = RefCell::new(FormXObjectEffectAnalyzer::with_bounds(1, FLATE_LIMIT));
    let policy = PageXObjectPolicy::analyzed(Some(&report), &input, lookup, &analyzer);

    // Construction does not inspect the first declared-but-uninvoked Form. The
    // only target attempt is spent when /A is actually demanded.
    let fill_effect = PageXObjectEffect::AnalyzedForm {
        consumes_stroking: false,
        consumes_nonstroking: true,
    };
    assert_eq!(policy.effect_of(&PdfName(b"A".to_vec())), fill_effect);
    // /B is a second semantic alias of the same exact target. It resolves from
    // the analyzer cache despite the exhausted one-target admission budget.
    assert_eq!(policy.effect_of(&PdfName(b"B".to_vec())), fill_effect);
    // The formerly uninvoked distinct target now has no first-seen attempt left.
    assert_eq!(
        policy.effect_of(&PdfName(b"Unused".to_vec())),
        PageXObjectEffect::Form
    );
}

#[test]
fn lazy_form_resolution_cannot_override_name_or_page_poisoning() {
    let input = form_pdf(&[raw_form(b"0 0 m 1 1 l S"), raw_form(b"0 0 m 1 1 l f")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset5 = object_offset(lookup, 5);
    let offset6 = object_offset(lookup, 6);
    let matching_skip = SkippedPageXObjectResource {
        page_object_byte_offset: 20,
        resource_name: Some(ResourceName(b"Fm#31".to_vec())),
        reason: SkippedPageXObjectResourceReason::MissingSubtype {
            object_byte_offset: offset5,
        },
    };
    let report = form_report_with_skips(
        vec![
            form_target(b"Bad#GG", 5, offset5),
            form_target(b"Fm1", 5, offset5),
            form_target(b"Good", 6, offset6),
        ],
        vec![matching_skip],
    );
    let analyzer = RefCell::new(FormXObjectEffectAnalyzer::with_bounds(1, FLATE_LIMIT));
    let policy = PageXObjectPolicy::analyzed(Some(&report), &input, lookup, &analyzer);

    assert_eq!(
        policy.effect_of(&PdfName(b"Bad#GG".to_vec())),
        PageXObjectEffect::Unknown
    );
    assert_eq!(
        policy.effect_of(&PdfName(b"Fm1".to_vec())),
        PageXObjectEffect::Unknown
    );
    // Neither malformed-name nor matching-skip poison spent the only target
    // attempt, so an unrelated unpoisoned Form still resolves exactly.
    assert_eq!(
        policy.effect_of(&PdfName(b"Good".to_vec())),
        PageXObjectEffect::AnalyzedForm {
            consumes_stroking: false,
            consumes_nonstroking: true,
        }
    );

    let page_skip = SkippedPageXObjectResource {
        page_object_byte_offset: 20,
        resource_name: None,
        reason: SkippedPageXObjectResourceReason::MissingXObject,
    };
    let incomplete =
        form_report_with_skips(vec![form_target(b"Good", 6, offset6)], vec![page_skip]);
    let second_analyzer = RefCell::new(FormXObjectEffectAnalyzer::with_bounds(1, FLATE_LIMIT));
    let policy = PageXObjectPolicy::analyzed(Some(&incomplete), &input, lookup, &second_analyzer);
    assert_eq!(
        policy.effect_of(&PdfName(b"Good".to_vec())),
        PageXObjectEffect::Unknown
    );
}

// --- Request budgets ---------------------------------------------------------

#[test]
fn the_first_seen_target_cap_refuses_further_unseen_targets() {
    let input = form_pdf(&[raw_form(b"0 0 m 1 1 l f"), raw_form(b"0 0 m 1 1 l S")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(1, FLATE_LIMIT);

    let ref5 = IndirectRef {
        object_number: 5,
        generation: 0,
    };
    let ref6 = IndirectRef {
        object_number: 6,
        generation: 0,
    };
    assert_eq!(
        analyzer.analyze(&input, lookup, ref5, object_offset(lookup, 5)),
        Some([false, true])
    );
    // The one attempt is spent, so a fresh identity is Unknown.
    assert_eq!(
        analyzer.analyze(&input, lookup, ref6, object_offset(lookup, 6)),
        None
    );
    // The already-cached first identity still resolves.
    assert_eq!(
        analyzer.analyze(&input, lookup, ref5, object_offset(lookup, 5)),
        Some([false, true])
    );
}

#[test]
fn the_aggregate_byte_budget_bounds_analysis_across_targets() {
    let big = raw_form(b"0 0 m 1 1 l 2 2 3 3 4 4 c f"); // >8-byte body
    let small = raw_form(b"f"); // refused on grammar, so no byte charge either way
    let input = form_pdf(&[big, small]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    // A tiny byte budget refuses the first form's decode/slice outright.
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(256, 4);
    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 5,
                generation: 0
            },
            object_offset(lookup, 5)
        ),
        None
    );

    // Exhaustion is terminal for unseen targets even when the raw body is
    // empty; otherwise zero-byte Forms could bypass the aggregate request cap.
    let empty_input = form_pdf(&[raw_form(b"")]);
    let empty_access = inspect_document_access(&empty_input).expect("open");
    let empty_lookup = backend_lookup(&empty_access.backend);
    let mut exhausted = FormXObjectEffectAnalyzer::with_bounds(256, 0);
    assert_eq!(
        exhausted.analyze(
            &empty_input,
            empty_lookup,
            IndirectRef {
                object_number: 5,
                generation: 0
            },
            object_offset(empty_lookup, 5)
        ),
        None
    );
}

#[test]
fn the_aggregate_byte_budget_accounts_cumulatively_across_targets() {
    // Two distinct raw forms whose bodies are 13 bytes each. A budget of 20 admits
    // the first body's slice (charging 13, leaving 7) but cannot fit the second,
    // proving the byte charge is ONE aggregate running total, not a per-form cap.
    let input = form_pdf(&[raw_form(b"0 0 m 1 1 l f"), raw_form(b"0 0 m 2 2 l S")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(256, 20);

    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 5,
                generation: 0
            },
            object_offset(lookup, 5)
        ),
        Some([false, true])
    );
    // The remaining 7-byte budget is below the second body's 13-byte extent.
    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 6,
                generation: 0
            },
            object_offset(lookup, 6)
        ),
        None
    );
}

// --- Compressed / non-addressable identity -----------------------------------

/// An xref-stream one-page PDF whose object 5 (the Form) is a Type-2 COMPRESSED
/// xref entry, so it is not source-addressable at a byte offset.
fn xref_stream_pdf_with_compressed_form() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let catalog_offset = buf.len();
    buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = buf.len();
    buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    let page_offset = buf.len();
    buf.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>\nendobj\n",
    );
    let content_offset = buf.len();
    buf.extend_from_slice(b"4 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n");
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&super::xref_record(0, 0, 0));
    body.extend_from_slice(&super::xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&super::xref_record(1, pages_offset, 0));
    body.extend_from_slice(&super::xref_record(1, page_offset, 0));
    body.extend_from_slice(&super::xref_record(1, content_offset, 0));
    // Object 5: compressed in object stream 8 at index 0.
    body.extend_from_slice(&super::xref_record(2, 8, 0));
    body.extend_from_slice(&super::xref_record(1, xref_offset, 0));

    buf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /Index [0 7] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}

#[test]
fn a_compressed_or_non_addressable_target_is_unknown() {
    let input = xref_stream_pdf_with_compressed_form();
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::new();
    // Object 5 resolves to a compressed xref entry, so it is not addressable at a
    // source byte offset: corroboration fails and the result is Unknown for any
    // offset the caller claims to have reached.
    assert_eq!(
        analyzer.analyze(
            &input,
            lookup,
            IndirectRef {
                object_number: 5,
                generation: 0
            },
            8
        ),
        None
    );
}

// --- End-to-end helpers ------------------------------------------------------

fn page_body(contents: &str, resources: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {contents} /Resources << {resources} >> >>"
    )
    .into_bytes()
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

fn convert(input: &[u8]) -> ConvertContentColorsOutput {
    convert_link(input, GRAY_TO_GRAY_LINK)
}

fn resource_pdf(content: &[u8], resources: &str, form: Vec<u8>) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("4 0 R", resources),
        stream_body("", content),
        form,
    ])
}

// --- Outer Do lane behaviour and end-to-end conversion -----------------------

#[test]
fn a_consuming_form_closes_the_matching_root_and_converts_only_the_page_setter() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let form = raw_form(b"0 0 m 1 1 l f"); // fill consumes nonstroking
    let input = resource_pdf(b"/GrayAlias cs 0.5 sc /Fm Do\n", &resources, form.clone());
    let output = convert(&input);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    // Only page setter bytes changed; the Form object is byte-identical and
    // appears exactly once (never re-appended into the revision).
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(!contains(
        &page_decoded_stream(&output.bytes, false),
        b"GrayAlias"
    ));
    assert!(contains(&output.bytes, &form));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    reopen(&output.bytes);
}

#[test]
fn a_neutral_form_leaves_the_alias_root_live() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        &resources,
        raw_form(b"0 0 100 100 re W n"),
    );
    let output = convert(&input);
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
}

#[test]
fn a_form_only_closes_the_lane_it_paints() {
    // A stroke-only Form does NOT consume a nonstroking (cs/sc) alias root.
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        &resources,
        raw_form(b"0 0 m 1 1 l S"),
    );
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_converted,
        0
    );

    // The same stroke-only Form DOES consume a stroking (CS/SC) alias root.
    let input = resource_pdf(
        b"/GrayAlias CS 0.5 SC /Fm Do\n",
        &resources,
        raw_form(b"0 0 m 1 1 l S"),
    );
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_converted,
        2
    );
}

#[test]
fn an_invalid_graphics_object_context_refuses_before_form_analysis() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let form = raw_form(b"0 0 m 1 1 l f");
    for content in [
        b"/GrayAlias cs 0.5 sc BT /Fm Do ET\n".as_slice(),
        b"/GrayAlias cs 0.5 sc 0 0 m /Fm Do 1 1 l f\n",
    ] {
        let input = resource_pdf(content, &resources, form.clone());
        assert_eq!(
            convert(&input).converted[0].resource_alias_candidates_refused,
            2
        );
    }
}

#[test]
fn a_flate_consuming_form_converts_and_preserves_the_source_prefix() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let form = flate_form(b"0 0 m 1 1 l f");
    let input = resource_pdf(b"/GrayAlias cs 0.5 sc /Fm Do\n", &resources, form.clone());
    let output = convert(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert!(contains(&output.bytes, &form));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
}

#[test]
fn a_shared_form_reached_under_two_names_analyzes_once_and_stays_verbatim() {
    let resources = format!("{GRAY_ALIAS} /XObject << /A 5 0 R /B 5 0 R >>");
    let form = raw_form(b"0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /A Do /B Do\n",
        &resources,
        form.clone(),
    );
    let output = convert(&input);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert!(contains(&output.bytes, &form));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
}

/// A two-page classic PDF whose pages both alias-set and `Do` the SAME shared
/// Form object 7. Object layout: 1 catalog, 2 pages, 3 page-0, 4 content-0,
/// 5 page-1, 6 content-1, 7 form.
fn two_page_shared_form_pdf(form: Vec<u8>) -> Vec<u8> {
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 7 0 R >>");
    let leaf = |contents: &str| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {contents} /Resources << {resources} >> >>"
        )
        .into_bytes()
    };
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
        leaf("4 0 R"),
        stream_body("", b"/GrayAlias cs 0.5 sc /Fm Do\n"),
        leaf("6 0 R"),
        stream_body("", b"/GrayAlias cs 0.5 sc /Fm Do\n"),
        form,
    ])
}

#[test]
fn the_same_form_across_two_pages_is_reused_from_the_request_cache() {
    // One request-scoped analyzer is shared across both selected pages, so the
    // exact shared Form is analyzed once and its lane effect closes the alias root
    // on BOTH pages while the Form object stays byte-identical and unduplicated.
    let form = raw_form(b"0 0 m 1 1 l f");
    let input = two_page_shared_form_pdf(form.clone());
    let output = convert(&input);

    assert_eq!(output.converted.len(), 2);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert_eq!(output.converted[1].resource_alias_candidates_converted, 2);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(contains(&output.bytes, &form));
    assert_eq!(occurrence_count(&output.bytes, b"7 0 obj"), 1);
    reopen(&output.bytes);
}

// --- Semantic-name folding ---------------------------------------------------

#[test]
fn analyzed_form_folds_through_one_unpoisoned_semantic_name() {
    // A decoded-name Do matches an escaped report name.
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm#31 5 0 R >>");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm1 Do\n",
        &resources,
        raw_form(b"0 0 m 1 1 l f"),
    );
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_converted,
        2
    );
}

#[test]
fn a_semantic_form_name_collision_poisons_the_name_to_unknown() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm1 5 0 R /Fm#31 6 0 R >>");
    let form = raw_form(b"0 0 m 1 1 l f");
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("4 0 R", &resources),
        stream_body("", b"/GrayAlias cs 0.5 sc /Fm1 Do\n"),
        form.clone(),
        form,
    ]);
    // The colliding semantic name is Unknown, so the outer Do refuses the epoch.
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_refused,
        2
    );
}

// --- Retained refusal boundaries ---------------------------------------------

#[test]
fn form_local_resource_gs_text_and_nested_constructs_refuse_without_leaking() {
    // Each form body reaches a construct outside the raw allowlist, so the outer
    // Do keeps the fail-closed refusal and every byte stays verbatim.
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    for body in [
        b"/CS0 cs 0 0 m 1 1 l f".as_slice(), // form-local resource colour
        b"/GS0 gs 0 0 m 1 1 l f",            // ExtGState
        b"BT (x) Tj ET",                     // text
        b"/Other Do",                        // nested Do
    ] {
        let form = raw_form(body);
        let input = resource_pdf(b"/GrayAlias cs 0.5 sc /Fm Do\n", &resources, form.clone());
        let output = convert(&input);
        assert_eq!(
            output.converted[0].resource_alias_candidates_refused,
            2,
            "{:?}",
            String::from_utf8_lossy(body)
        );
        assert!(contains(&output.bytes, &form));
        assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
        assert!(contains(
            &page_decoded_stream(&output.bytes, false),
            b"/GrayAlias cs 0.5 sc"
        ));
    }
}

#[test]
fn a_reference_xobject_form_retains_the_fail_closed_refusal() {
    // A root Form declaring `/Ref` is a reference XObject: its imported page
    // content, not this proxy stream, is painted. The outer Do keeps the
    // fail-closed `XObjectInvoke` refusal, no page setter converts, and the Form
    // object stays byte-identical and unduplicated.
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let form = raw_form_dict(" /Ref << /Page 1 >>", b"0 0 m 1 1 l f");
    let input = resource_pdf(b"/GrayAlias cs 0.5 sc /Fm Do\n", &resources, form.clone());
    let output = convert(&input);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert!(contains(&output.bytes, &form));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
}

#[test]
fn semantic_dictionary_refusals_keep_outer_do_fail_closed_and_form_bytes_verbatim() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    for extra in [
        " /F (external.pdf)",
        " /#46 (external.pdf)",
        " /Ref << /Page 1 >>",
        " /R#65f << /Page 1 >>",
        " /OC << /Type /OCG >>",
        " /O#43 << /Type /OCG >>",
        " /OPI << /Version 2.0 >>",
        " /O#50I << /Version 2.0 >>",
        " /Gr#6Fup << /S /Transparency >>",
        " /Fil#74er /ASCIIHexDecode",
        " /DecodeP#61rms null",
        " /Len#67th 13",
        " /Group null /Gr#6Fup null",
        " /Filter /FlateDecode /Fil#74er /FlateDecode",
        " /DecodeParms null /DecodeP#61rms null",
        " /Length 13 /Len#67th 13",
        " /Bad#GG 1",
    ] {
        let form = raw_form_dict(extra, b"0 0 m 1 1 l f");
        let input = resource_pdf(b"/GrayAlias cs 0.5 sc /Fm Do\n", &resources, form.clone());
        let output = convert(&input);
        let page = &output.converted[0];

        assert_eq!(
            page.resource_alias_candidates_converted, 0,
            "unexpected conversion for {extra}"
        );
        assert_eq!(
            page.resource_alias_candidates_refused, 2,
            "missing outer-Do refusal for {extra}"
        );
        assert_eq!(&output.bytes[..input.len()], input.as_slice());
        assert!(contains(&output.bytes, &form), "changed Form for {extra}");
        assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
        assert!(contains(
            &page_decoded_stream(&output.bytes, false),
            b"/GrayAlias cs 0.5 sc"
        ));
    }
}

#[test]
fn matrix_and_bbox_do_not_change_the_lane_summary_or_form_bytes() {
    // A restrictive BBox and a non-identity Matrix make no difference to the
    // syntactic lane summary, and the Form bytes are never touched.
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let form = stream_body(
        " /Type /XObject /Subtype /Form /BBox [10 10 20 20] /Matrix [2 0 0 2 5 5]",
        b"0 0 m 1 1 l f",
    );
    let input = resource_pdf(b"/GrayAlias cs 0.5 sc /Fm Do\n", &resources, form.clone());
    let output = convert(&input);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert!(contains(&output.bytes, &form));
}

#[test]
fn rgb_alias_form_consumption_converts_through_the_routed_link() {
    // A different device family end-to-end: an RGB alias consumed by a form fill,
    // routed RGB->CMYK.
    let resources = "/ColorSpace << /RgbAlias /DeviceRGB >> /XObject << /Fm 5 0 R >>".to_string();
    let input = resource_pdf(
        b"/RgbAlias cs 0.1 0.2 0.3 scn /Fm Do\n",
        &resources,
        raw_form(b"0 0 m 1 1 l f"),
    );
    let output = convert_link(&input, RGB_TO_CMYK_LINK);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert!(!contains(
        &page_decoded_stream(&output.bytes, false),
        b"RgbAlias"
    ));
}
