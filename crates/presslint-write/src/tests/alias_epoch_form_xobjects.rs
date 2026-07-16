//! Root Form LOCAL Image and stencil colour-effect admission (T189).
//!
//! These matrices exercise the invoked Form-local `XObject` admission the T188
//! analyzer gained: the bounded decoded-name `/Resources /XObject` authority
//! (canonical-key proof, exact target corroboration, collision/named-skip/
//! literal/nameless poisoning, the fact cap), the intrinsic ordinary-Image and
//! stencil lane semantics over the seeded walk's live lanes, and the retained
//! refusal envelope (substitution escapes, ambiguous stencil gates, nested
//! Forms outside the T190 recursion's admission envelope; substantive nested
//! recursion coverage lives in `alias_epoch_form_recursion.rs`). Analyzer UNIT
//! tests call [`FormXObjectEffectAnalyzer::analyze`] on a
//! real Form reached through the request `ObjectLookup`; the END-TO-END tests
//! drive `convert_content_colors_incremental` to lock the page-only mutation,
//! the unchanged Form/resource/image bytes, and the outer `Do` lane behaviour.

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    DeviceLinkInput, FormXObjectRefusalClass, PageSelection, convert_content_colors_incremental,
    form_xobject_effect::{
        FormXObjectEffectAnalyzer, xobject_target_identity_corroborates_for_test,
    },
};
use presslint_pdf::{
    DocumentAccessBackend, IndirectRef, ObjectLookup, ObjectLookupLocation,
    PageXObjectResourceTarget, PdfName, encode_flate_stream, inspect_document_access,
    inspect_indirect_object_dictionary, locate_xref_object,
};

use super::alias_epoch_form::assert_only_class;
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

/// An ordinary gray image `XObject` with an extra dictionary fragment.
fn image_object(dict_extra: &str) -> Vec<u8> {
    stream_body(
        &format!(
            " /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray{dict_extra}"
        ),
        b"\x00",
    )
}

/// A structurally valid stencil-mask `XObject` with an extra fragment.
fn stencil_object(dict_extra: &str) -> Vec<u8> {
    stream_body(
        &format!(
            " /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 1 /ImageMask true{dict_extra}"
        ),
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

/// Analyze a form whose sole `XObject` resource `/St` is a valid stencil.
fn analyze_stencil_form(content: &[u8]) -> Option<[bool; 2]> {
    analyze(&[xform("/St 6 0 R", content), stencil_object("")])
}

/// Analyze object 5 (the first supplied body) once in a fresh request
/// analyzer and return its tallied per-page refusal-class counts.
fn refusal_counts_for(objects: &[Vec<u8>]) -> crate::FormXObjectRefusalCounts {
    let input = form_pdf(objects);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset = object_offset(lookup, 5);
    let mut analyzer = FormXObjectEffectAnalyzer::new();
    analyzer.analyze(
        &input,
        lookup,
        IndirectRef {
            object_number: 5,
            generation: 0,
        },
        offset,
    );
    analyzer.take_page_refusal_counts()
}

// --- Refusal-class taxonomy locks (T192) -------------------------------------

#[test]
fn an_unresolved_do_name_classifies_xobject_authority() {
    // No `/Resources /XObject` at all.
    assert_only_class(
        &refusal_counts_for(&[form("", b"/Missing Do")]),
        FormXObjectRefusalClass::XObjectAuthority,
    );
    // A generation-mismatched Form target: poisoned at authority-build time,
    // not at the root's own exact-identity corroboration.
    assert_only_class(
        &refusal_counts_for(&[xform("/Fm 6 1 R", b"/Fm Do"), form("", b"0 0 m 1 1 l f")]),
        FormXObjectRefusalClass::XObjectAuthority,
    );
}

#[test]
fn a_root_exact_identity_mismatch_classifies_structural_preflight() {
    // The generation the caller reached does not corroborate: this is the
    // ROOT's own exact-identity gate, not the XObject authority the rest of
    // this module exercises.
    let input = form_pdf(&[form("", b"0 0 m 1 1 l f")]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let offset = object_offset(lookup, 5);
    let mut analyzer = FormXObjectEffectAnalyzer::new();
    analyzer.analyze(
        &input,
        lookup,
        IndirectRef {
            object_number: 5,
            generation: 1,
        },
        offset,
    );
    assert_only_class(
        &analyzer.take_page_refusal_counts(),
        FormXObjectRefusalClass::StructuralPreflight,
    );
}

#[test]
fn raw_do_grammar_requires_exactly_one_name_and_no_open_path() {
    assert_eq!(
        analyze(&[xform("/Im 6 0 R", b"/Im Do"), image_object("")]),
        Some([false, false])
    );
    for content in [
        &b"Do"[..],
        &b"/Im /Other Do"[..],
        &b"/Im 1 Do"[..],
        &b"1 Do"[..],
        &b"0 0 m /Im Do 1 1 l S"[..],
    ] {
        assert_eq!(
            analyze(&[xform("/Im 6 0 R", content), image_object("")]),
            None,
            "{}",
            String::from_utf8_lossy(content),
        );
    }
}

// --- Intrinsic Image/stencil effects ------------------------------------------

#[test]
fn an_invoked_ordinary_image_is_neutral_to_both_lanes() {
    // The image alone consumes nothing; a later paint still reads whatever the
    // untouched inherited lanes hold, so neutrality composes with path truth.
    assert_eq!(
        analyze(&[xform("/Im 6 0 R", b"/Im Do"), image_object("")]),
        Some([false, false])
    );
    assert_eq!(
        analyze(&[
            xform("/Im 6 0 R", b"/Im Do 0 0 m 1 1 l B"),
            image_object("")
        ]),
        Some([true, true])
    );
}

#[test]
fn an_invoked_stencil_consumes_only_the_inherited_nonstroking_lane() {
    assert_eq!(analyze_stencil_form(b"/St Do"), Some([false, true]));
    // Inside an untouched `q` frame the lane is still the inherited sentinel.
    assert_eq!(analyze_stencil_form(b"q /St Do Q"), Some([false, true]));
}

#[test]
fn a_local_direct_nonstroking_colour_before_a_stencil_kills_inherited_consumption() {
    assert_eq!(analyze_stencil_form(b"0.5 g /St Do"), Some([false, false]));
    assert_eq!(
        analyze_stencil_form(b"0.1 0.2 0.3 rg /St Do"),
        Some([false, false])
    );
    // A stroking-side setter does NOT feed a stencil: the nonstroking lane is
    // still inherited and consumed.
    assert_eq!(analyze_stencil_form(b"0.5 G /St Do"), Some([false, true]));
}

#[test]
fn a_local_form_resource_colour_before_a_stencil_kills_inherited_consumption() {
    // A Form-local alias selection plus setter proves a local nonstroking lane.
    assert_eq!(
        analyze(&[
            form(
                " /Resources << /ColorSpace << /L /DeviceGray >> /XObject << /St 6 0 R >> >>",
                b"/L cs 0.5 sc /St Do",
            ),
            stencil_object(""),
        ]),
        Some([false, false])
    );
    // The direct reserved selection alone kills the lane via its ISO initial
    // colour, so the stencil consumes a local colour, not the inherited root.
    assert_eq!(
        analyze_stencil_form(b"/DeviceGray cs /St Do"),
        Some([false, false])
    );
}

#[test]
fn q_local_colour_stencil_q_stencil_consumes_only_at_the_restored_sentinel() {
    assert_eq!(
        analyze_stencil_form(b"q 0.5 g /St Do Q /St Do"),
        Some([false, true])
    );
    assert_eq!(
        analyze_stencil_form(b"q 0.5 g /St Do Q"),
        Some([false, false])
    );
}

#[test]
fn stroking_state_is_independent_and_combined_programs_aggregate_exact_bits() {
    // The stencil consumes the fill lane; the stroke paint consumes the
    // stroking lane independently.
    assert_eq!(
        analyze_stencil_form(b"/St Do 0 0 m 1 1 l S"),
        Some([true, true])
    );
    // A local stroking colour leaves only the stencil's consumption.
    assert_eq!(
        analyze_stencil_form(b"0.5 G /St Do 0 0 m 1 1 l S"),
        Some([false, true])
    );
    // A local fill colour after the stencil leaves only its consumption.
    assert_eq!(
        analyze_stencil_form(b"/St Do 0.5 g 0 0 m 1 1 l f"),
        Some([false, true])
    );
}

#[test]
fn a_sentinel_valued_local_setter_cannot_recreate_inherited_identity() {
    // `k` with the exact numeric components of the inherited nonstroking
    // sentinel still stamps a concrete source range, so the stencil reads a
    // proven local colour and consumes nothing inherited.
    assert_eq!(
        analyze_stencil_form(b"0.0625 0.125 0.25 0.5 k /St Do"),
        Some([false, false])
    );
}

#[test]
fn masked_smasked_and_jpx_ordinary_images_remain_lane_neutral() {
    // `/Mask`, `/SMask`, `/Decode`, `/Interpolate`, `/Intent` and a JPX-style
    // missing `/ColorSpace` affect coverage/sample interpretation only; none
    // makes an ordinary image read a current colour lane, and none is decoded.
    for extra in [
        " /Mask [0 1]",
        " /Decode [1 0] /Interpolate true /Intent /Perceptual",
    ] {
        assert_eq!(
            analyze(&[xform("/Im 6 0 R", b"/Im Do"), image_object(extra)]),
            Some([false, false]),
            "{extra}",
        );
    }
    assert_eq!(
        analyze(&[
            xform("/Im 6 0 R", b"/Im Do"),
            image_object(" /SMask 7 0 R"),
            image_object(""),
        ]),
        Some([false, false])
    );
    assert_eq!(
        analyze(&[
            xform("/Im 6 0 R", b"/Im Do"),
            stream_body(
                " /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /Filter /JPXDecode",
                b"\x00",
            ),
        ]),
        Some([false, false])
    );
}

#[test]
fn alternates_opi_oc_and_external_execution_refuse() {
    for extra in [
        " /Alternates [7 0 R]",
        " /OPI << >>",
        " /OC 7 0 R",
        " /F (ext.dat)",
        " /Ref << >>",
        // The same escape under a valid `#xx` spelling cannot evade the gate.
        " /O#43 7 0 R",
    ] {
        assert_eq!(
            analyze(&[
                xform("/Im 6 0 R", b"/Im Do"),
                image_object(extra),
                image_object(""),
            ]),
            None,
            "{extra}",
        );
    }
}

#[test]
fn unsupported_stencil_shapes_refuse() {
    for body in [
        // BitsPerComponent must be absent or exactly 1.
        stream_body(
            " /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ImageMask true",
            b"\x00",
        ),
        // A stencil must not declare a colour space.
        stencil_object(" /ColorSpace /DeviceGray"),
        // Dimensions must be positive.
        stream_body(
            " /Type /XObject /Subtype /Image /Width 0 /Height 1 /BitsPerComponent 1 /ImageMask true",
            b"\x00",
        ),
        // `/ImageMask` must be an exact boolean scalar.
        stream_body(
            " /Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask 1",
            b"\x00",
        ),
        // A raw duplicate `/ImageMask` is ambiguous authority.
        stream_body(
            " /Type /XObject /Subtype /Image /Width 1 /Height 1 /ImageMask true /ImageMask true",
            b"\x00",
        ),
    ] {
        assert_eq!(analyze(&[xform("/St 6 0 R", b"/St Do"), body]), None);
    }
}

// --- Authority, laziness and bounds --------------------------------------------

#[test]
fn escaped_do_and_resource_spellings_resolve_through_decoded_equality() {
    // Escaped resource key, canonical operand (#6D == 'm').
    assert_eq!(
        analyze(&[xform("/I#6D 6 0 R", b"/Im Do"), image_object("")]),
        Some([false, false])
    );
    // Canonical resource key, escaped operand.
    assert_eq!(
        analyze(&[xform("/Im 6 0 R", b"/I#6D Do"), image_object("")]),
        Some([false, false])
    );
}

#[test]
fn semantic_name_collisions_and_named_skips_poison_only_the_invoked_name() {
    // `/A` and `/#41` decode to one semantic name: invoking it refuses, while
    // the unrelated `/B` binding stays proven.
    let colliding = "/A 6 0 R /#41 6 0 R /B 6 0 R";
    assert_eq!(
        analyze(&[xform(colliding, b"/A Do"), image_object("")]),
        None
    );
    assert_eq!(
        analyze(&[xform(colliding, b"/B Do"), image_object("")]),
        Some([false, false])
    );
    // A named structural skip (a non-reference value) poisons only its name.
    let skipping = "/Bad (x) /Good 6 0 R";
    assert_eq!(
        analyze(&[xform(skipping, b"/Bad Do"), image_object("")]),
        None
    );
    assert_eq!(
        analyze(&[xform(skipping, b"/Good Do"), image_object("")]),
        Some([false, false])
    );
}

#[test]
fn nameless_skip_and_fact_cap_overflow_poison_the_namespace() {
    use std::fmt::Write as _;

    // A non-dictionary `/XObject` value is a nameless structural gap: no `Do`
    // is admissible.
    assert_eq!(
        analyze(&[
            form(" /Resources << /XObject (bad) >>", b"/Im Do"),
            image_object(""),
        ]),
        None
    );
    // Up to 256 classified targets stay within the fact cap; one more poisons
    // the whole namespace even for an otherwise proven name.
    let mut in_bounds = String::new();
    for index in 0..256 {
        write!(in_bounds, "/K{index} 6 0 R ").expect("write");
    }
    let overflowing = format!("{in_bounds}/K256 6 0 R ");
    assert_eq!(
        analyze(&[xform(&in_bounds, b"/K0 Do"), image_object("")]),
        Some([false, false])
    );
    assert_eq!(
        analyze(&[xform(&overflowing, b"/K0 Do"), image_object("")]),
        None
    );
}

#[test]
fn a_malformed_literal_spelling_colliding_with_an_invoked_decoded_name_refuses() {
    let resources = "/Bad#GG 6 0 R /Good 6 0 R";
    // The valid operand decodes to the malformed key's literal byte spelling.
    assert_eq!(
        analyze(&[xform(resources, b"/Bad#23GG Do"), image_object("")]),
        None
    );
    // The unrelated proven name stays isolated from the malformed key.
    assert_eq!(
        analyze(&[xform(resources, b"/Good Do"), image_object("")]),
        Some([false, false])
    );
}

#[test]
fn escaped_or_duplicate_resources_and_xobject_authority_refuses() {
    for dict in [
        // Semantic duplicate top-level `/Resources` under an escaped spelling.
        " /Resources << /XObject << /Im 6 0 R >> >> /Resour#63es << >>",
        // Semantic duplicate nested `/XObject` under an escaped spelling.
        " /Resources << /XObject << /Im 6 0 R >> /X#4Fbject << >> >>",
        // An escaped-only `/XObject` key can never be canonical authority.
        " /Resources << /X#4Fbject << /Im 6 0 R >> >>",
    ] {
        assert_eq!(
            analyze(&[form(dict, b"/Im Do"), image_object("")]),
            None,
            "{dict}",
        );
    }
}

#[test]
fn prefix_relevant_malformed_resources_and_xobject_authority_refuses() {
    // A malformed spelling whose valid decoded prefix can still be
    // `/Resources` makes the Form-level authority uncertain.
    assert_eq!(
        analyze(&[
            form(
                " /Resources << /XObject << /Im 6 0 R >> >> /Resource#GG << >>",
                b"/Im Do",
            ),
            image_object(""),
        ]),
        None
    );
    // The same prefix uncertainty inside the proven Resources dictionary
    // poisons the entire XObject namespace.
    assert_eq!(
        analyze(&[
            form(
                " /Resources << /XObject << /Im 6 0 R >> /XObjec#GG << >> >>",
                b"/Im Do",
            ),
            image_object(""),
        ]),
        None
    );
    // Malformed keys whose valid prefix cannot denote `/XObject`, or any
    // target authority key consulted for this ordinary Image, stay isolated.
    assert_eq!(
        analyze(&[
            form(
                " /Resources << /XObject << /Im 6 0 R >> /Other#GG null >>",
                b"/Im Do",
            ),
            image_object(" /Unrelated#GG 1"),
        ]),
        Some([false, false])
    );
}

#[test]
fn escaped_or_duplicate_subtype_and_imagemask_target_authority_refuses() {
    // A semantically duplicate `/Subtype` hidden behind an escape refuses even
    // though the raw report only saw the canonical `/Image`.
    assert_eq!(
        analyze(&[
            xform("/Im 6 0 R", b"/Im Do"),
            image_object(" /Sub#74ype /Form"),
        ]),
        None
    );
    // An `/ImageMask` reachable only through an escaped spelling makes the
    // raw Missing fact untrustworthy.
    assert_eq!(
        analyze(&[
            xform("/Im 6 0 R", b"/Im Do"),
            image_object(" /Image#4Dask true"),
        ]),
        None
    );
}

#[test]
fn prefix_relevant_malformed_target_authority_is_name_local() {
    for malformed in [" /Subtyp#GG /Form", " /ImageMas#GG true"] {
        // The malformed key may hide the affected target's required authority,
        // so invoking that target refuses.
        assert_eq!(
            analyze(&[
                xform("/Bad 6 0 R /Good 7 0 R", b"/Bad Do"),
                image_object(malformed),
                image_object(""),
            ]),
            None,
            "{malformed}",
        );
        // Per-target poison does not contaminate an unrelated exact binding.
        assert_eq!(
            analyze(&[
                xform("/Bad 6 0 R /Good 7 0 R", b"/Good Do"),
                image_object(malformed),
                image_object(""),
            ]),
            Some([false, false]),
            "{malformed}",
        );
    }
}

#[test]
fn stencil_dimension_authority_ambiguity_refuses() {
    for extra in [
        " /W#69dth 1",
        " /H#65ight 1",
        " /Bits#50erComponent 1",
        " /Color#53pace /DeviceGray",
    ] {
        assert_eq!(
            analyze(&[xform("/St 6 0 R", b"/St Do"), stencil_object(extra)]),
            None,
            "{extra}",
        );
    }
}

#[test]
fn generation_offset_and_subtype_identity_mismatches_refuse() {
    // Generation mismatch against the in-use xref entry.
    assert_eq!(
        analyze(&[xform("/Im 6 1 R", b"/Im Do"), image_object("")]),
        None
    );
    // Unresolvable target object.
    assert_eq!(
        analyze(&[xform("/Im 99 0 R", b"/Im Do"), image_object("")]),
        None
    );
    // A non-Image/Form subtype is outside the classified report.
    assert_eq!(
        analyze(&[
            xform("/Ps 6 0 R", b"/Ps Do"),
            stream_body(" /Type /XObject /Subtype /PS", b""),
        ]),
        None
    );
}

/// Like [`form_pdf`], but every object header number is supplied explicitly
/// while the xref still numbers the slots 1..=N sequentially, so one target
/// body's header can disagree with the resource reference that reaches it.
fn form_pdf_with_headers(objects: &[(usize, Vec<u8>)]) -> Vec<u8> {
    let mut bodies = vec![
        (1, CATALOG.to_vec()),
        (2, PAGES.to_vec()),
        (
            3,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        ),
        (4, stream_body("", b"")),
    ];
    bodies.extend_from_slice(objects);
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (header_number, body) in &bodies {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{header_number} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
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

/// Xref-stream PDF whose analyzed root Form is uncompressed object 5 while its
/// invoked local Image target is the Type-2 member 6 of object stream 7.
fn xref_stream_pdf_with_compressed_local_image() -> Vec<u8> {
    let mut buf = b"%PDF-1.5\n".to_vec();
    let uncompressed = [
        CATALOG.to_vec(),
        PAGES.to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R >>".to_vec(),
        stream_body("", b""),
        xform("/Im 6 0 R", b"/Im Do"),
    ];
    let mut offsets = Vec::new();
    for (index, body) in uncompressed.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }

    let member_header = b"6 0 ";
    let member = b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 \
                   /BitsPerComponent 8 /ColorSpace /DeviceGray >>";
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
fn a_compressed_form_local_target_refuses_without_target_descent() {
    let input = xref_stream_pdf_with_compressed_local_image();
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

#[test]
fn a_stale_reached_offset_refuses_even_when_its_dictionary_header_matches() {
    // Xref slot 6 reaches the first `6 0 obj`; slot 7 reaches a second body
    // whose own header also says `6 0 obj`. A stale inspector tuple naming the
    // latter offset therefore passes a header-only check but not exact lookup
    // corroboration.
    let input = form_pdf_with_headers(&[
        (5, xform("/Im 6 0 R", b"/Im Do")),
        (6, image_object("")),
        (6, image_object("")),
    ]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let located_offset = object_offset(lookup, 6);
    let stale_reached_offset = object_offset(lookup, 7);
    assert_ne!(located_offset, stale_reached_offset);
    assert_eq!(
        inspect_indirect_object_dictionary(&input, stale_reached_offset)
            .expect("matching reached dictionary")
            .reference,
        IndirectRef {
            object_number: 6,
            generation: 0,
        }
    );
    let stale = PageXObjectResourceTarget {
        name: PdfName(b"Im".to_vec()),
        reference: IndirectRef {
            object_number: 6,
            generation: 0,
        },
        object_byte_offset: stale_reached_offset,
        image_metadata: None,
    };
    assert!(!xobject_target_identity_corroborates_for_test(
        &input, lookup, &stale
    ));
}

#[test]
fn retained_form_targets_require_exact_identity_and_canonical_subtype() {
    // A generation mismatch or an unresolvable reference on an invoked Form
    // target refuses exactly like the Image matrix above.
    assert_eq!(
        analyze(&[xform("/Fm 6 1 R", b"/Fm Do"), form("", b"")]),
        None
    );
    assert_eq!(
        analyze(&[xform("/Fm 99 0 R", b"/Fm Do"), form("", b"")]),
        None
    );
    // The xref slot for object 6 reaches a body whose own header claims
    // `7 0 obj`: the reinspected object header disagrees with the resource
    // reference, so the tuple is not exact and the invocation refuses.
    let input = form_pdf_with_headers(&[(5, xform("/Fm 6 0 R", b"/Fm Do")), (7, form("", b""))]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
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
    // A semantically duplicate `/Subtype` hidden behind an escape is ambiguous
    // authority: the raw report classified the canonical `/Form`, but the
    // tuple may not be retained as exact, so the invocation refuses through
    // per-name poison rather than the retained-Form refusal.
    assert_eq!(
        analyze(&[xform("/Fm 6 0 R", b"/Fm Do"), form(" /Sub#74ype /PS", b"")]),
        None
    );
    // The same uncorroborated Form target, declared but never invoked, poisons
    // nothing else: the unrelated proven Image stays admissible.
    assert_eq!(
        analyze(&[
            xform("/Fm 6 0 R /Im 7 0 R", b"/Im Do"),
            form(" /Sub#74ype /PS", b""),
            image_object(""),
        ]),
        Some([false, false])
    );
}

#[test]
fn an_invoked_nested_form_proves_while_unsupported_and_uninvoked_targets_stay_isolated() {
    // The bounded recursion now descends into the retained nested Form: its
    // fill consumes the parent's still-live inherited nonstroking lane.
    let nested_form_do = [
        xform("/Fm 6 0 R /Im 7 0 R /Bad (x)", b"/Fm Do"),
        form("", b"0 0 m 1 1 l f"),
        image_object(""),
    ];
    assert_eq!(analyze(&nested_form_do), Some([false, true]));
    // A nested Form outside the admission envelope (a transparency `/Group`)
    // still refuses the invoking parent fail-closed.
    let refused_nested_do = [
        xform("/Fm 6 0 R /Im 7 0 R /Bad (x)", b"/Fm Do"),
        form(" /Group << /S /Transparency >>", b"0 0 m 1 1 l f"),
        image_object(""),
    ];
    assert_eq!(analyze(&refused_nested_do), None);
    // The same declaration set with only the Image invoked: the retained Form
    // target and the named skip poison nothing else, and no Form descent runs.
    let image_do = [
        xform("/Fm 6 0 R /Im 7 0 R /Bad (x)", b"/Im Do"),
        form(" /Group << /S /Transparency >>", b"0 0 m 1 1 l f"),
        image_object(""),
    ];
    assert_eq!(analyze(&image_do), Some([false, false]));
}

#[test]
fn aliases_and_repeated_do_are_deterministic_and_cache_by_exact_root() {
    let input = form_pdf(&[
        xform("/A 6 0 R /B 6 0 R", b"/A Do /B Do /A Do"),
        image_object(""),
        form("", b"0 0 m 1 1 l f"),
    ]);
    let access = inspect_document_access(&input).expect("open");
    let lookup = backend_lookup(&access.backend);
    let mut analyzer = FormXObjectEffectAnalyzer::with_bounds(1, FLATE_LIMIT);
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
    // Same image under two aliases and a repeated `Do`: one deterministic
    // neutral summary.
    assert_eq!(analyze_target(&mut analyzer, 5), Some([false, false]));
    // The single first-seen target attempt is spent; the repeat is served from
    // the request cache without a second decode/authority build or charge.
    assert_eq!(analyze_target(&mut analyzer, 5), Some([false, false]));
    assert_eq!(analyze_target(&mut analyzer, 7), None);
}

#[test]
fn raw_and_flate_image_programs_share_the_same_summary() {
    let content = b"/St Do";
    let raw = xform("/St 6 0 R", content);
    let compressed = encode_flate_stream(content, FLATE_LIMIT).expect("encode");
    let flate = form(
        " /Resources << /XObject << /St 6 0 R >> >> /Filter /FlateDecode",
        &compressed,
    );
    assert_eq!(analyze(&[raw, stencil_object("")]), Some([false, true]));
    assert_eq!(analyze(&[flate, stencil_object("")]), Some([false, true]));
}

// --- End-to-end page-only mutation ---------------------------------------------

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
fn ordinary_image_neutrality_leaves_the_page_alias_live_for_a_later_consumer() {
    // The Form only paints its own image, so the page alias root stays live
    // through the `Do` and is consumed by the LATER page fill, which converts
    // the existing page setter.
    let form_object = xform("/Im 6 0 R", b"/Im Do");
    let image = image_object("");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do 0 0 m 1 1 l f\n",
        PAGE_RESOURCES,
        &[form_object.clone(), image.clone()],
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
    assert!(contains(&output.bytes, &image));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"6 0 obj"), 1);
    reopen(&output.bytes);

    // Without any later consumer the neutral Form leaves the alias setter
    // verbatim: nothing converts and nothing is refused.
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[form_object.clone(), image],
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
}

#[test]
fn stencil_consumption_closes_the_page_alias_and_converts_only_the_page_setter() {
    // The Form's stencil reads the caller's inherited nonstroking colour, so
    // the page alias root is consumed at the `Do` and the existing page setter
    // converts. Every Form/resource/image byte stays identical and unduplicated.
    let form_object = xform("/St 6 0 R", b"/St Do");
    let stencil = stencil_object("");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[form_object.clone(), stencil.clone()],
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
    assert!(contains(&output.bytes, &stencil));
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"6 0 obj"), 1);
    assert_eq!(output.converted.len(), 1);
    reopen(&output.bytes);
}

#[test]
fn a_form_local_stencil_under_a_local_colour_leaves_the_page_root_live() {
    // The Form kills its nonstroking lane before the stencil, so nothing
    // inherited is consumed and the page alias setter stays verbatim.
    let form_object = xform("/St 6 0 R", b"0.5 g /St Do");
    let stencil = stencil_object("");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[form_object.clone(), stencil],
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
fn an_unsupported_invoked_target_retains_the_fail_closed_refusal_end_to_end() {
    // The Form invokes a nested Form outside the admission envelope (a
    // transparency `/Group`): its analysis stays Unknown, the outer `Do` keeps
    // the fail-closed refusal, and the page alias setter survives.
    let form_object = xform("/Nested 6 0 R", b"/Nested Do");
    let refused_nested = form(" /Group << /S /Transparency >>", b"0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[form_object.clone(), refused_nested.clone()],
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
    assert!(contains(&output.bytes, &refused_nested));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"6 0 obj"), 1);
    reopen(&output.bytes);

    // The same invocation over an admissible nested Form now proves through
    // the bounded recursion: the nested fill consumes the inherited
    // nonstroking lane, the alias root closes, and only the existing page
    // setter converts while every Form byte stays identical and unduplicated.
    let proven_nested = form("", b"0 0 m 1 1 l f");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        PAGE_RESOURCES,
        &[form_object.clone(), proven_nested.clone()],
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
    assert!(contains(&output.bytes, &proven_nested));
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"6 0 obj"), 1);
    reopen(&output.bytes);
}
