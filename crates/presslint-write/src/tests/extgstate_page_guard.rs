//! Precision `ExtGState` page guard on the device-colour converter.

use presslint_types::PageIndex;

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    ConvertPageSkipReason, PageSelection, convert_content_colors_incremental,
};

use super::content_color_convert::{
    CMYK_TO_CMYK_LINK, assemble_classic, classic_raw_pdf, occurrence_count, one_link,
    page_encoded_stream_at, stream_body,
};

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";

fn page_body_with_resources(contents: &str, resources: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources {resources} /Contents {contents} >>"
    )
    .into_bytes()
}

fn classic_extgstate_pdf(resources: &str, data: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body_with_resources("4 0 R", resources),
        stream_body("", data),
    ])
}

fn classic_page_group_pdf(group: &str, data: &[u8]) -> Vec<u8> {
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << >> /Group {group} /Contents 4 0 R >>"
    )
    .into_bytes();
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page,
        stream_body("", data),
    ])
}

fn classic_two_stream_extgstate_pdf(resources: &str, stream_a: &[u8], stream_b: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body_with_resources("[4 0 R 5 0 R]", resources),
        stream_body("", stream_a),
        stream_body("", stream_b),
    ])
}

fn convert_cmyk(input: &[u8]) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(CMYK_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds")
}

fn assert_extgstate_skip(output: &ConvertContentColorsOutput, expected: ConvertPageSkipReason) {
    assert!(output.converted.is_empty());
    assert_eq!(output.skipped.len(), 1);
    let skip = &output.skipped[0];
    assert_eq!(skip.page_index, PageIndex(0));
    assert_eq!(skip.content_object, None);
    assert_eq!(skip.reason, expected);
}

#[test]
fn op_true_resource_skips_with_overprint_and_count() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /OP true >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: true,
            transparency: false,
            unresolved: false,
            unclassified: false,
            gs_count: 1,
        },
    );
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
}

#[test]
fn bm_multiply_resource_skips_with_transparency() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /BM /Multiply >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: true,
            unresolved: false,
            unclassified: false,
            gs_count: 1,
        },
    );
}

#[test]
fn bm_compatible_resource_converts_as_normal_equivalent() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /BM /Compatible >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 1);
}

#[test]
fn duplicate_resource_name_used_by_gs_skips_as_unclassified() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /BM /Normal >> /GS1 << /BM /Multiply >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: false,
            unclassified: true,
            gs_count: 1,
        },
    );
}

#[test]
fn unresolved_resource_name_skips_with_unresolved() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /Other << /LW 2 >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: true,
            unclassified: false,
            gs_count: 1,
        },
    );
}

#[test]
fn lw_only_resource_converts_precision_case() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /LW 2 >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
}

#[test]
fn no_gs_page_with_declared_resources_converts() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /OP true /BM /Multiply >> >> >>",
        b"0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 1);
}

#[test]
fn malformed_gs_operand_skips_with_unclassified() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /LW 2 >> >> >>",
        b"42 gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: false,
            unclassified: true,
            gs_count: 1,
        },
    );
}

#[test]
fn extgstate_inspection_failure_on_used_page_skips() {
    let input = classic_extgstate_pdf("<< /ExtGState 12 >>", b"/GS1 gs\n0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: true,
            unclassified: false,
            gs_count: 1,
        },
    );
}

#[test]
fn unsafe_gs_in_second_stream_skips_whole_page() {
    let input = classic_two_stream_extgstate_pdf(
        "<< /ExtGState << /GS1 << /OP true >> >> >>",
        b"0 0 0 1 k\n",
        b"/GS1 gs\n",
    );
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: true,
            transparency: false,
            unresolved: false,
            unclassified: false,
            gs_count: 1,
        },
    );
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert_eq!(page_encoded_stream_at(&output.bytes, 0, 0), b"0 0 0 1 k\n");
}

#[test]
fn gs_operand_and_operator_split_across_streams_are_guarded_globally() {
    let input = classic_two_stream_extgstate_pdf(
        "<< /ExtGState << /GS1 << /OP true >> >> >>",
        b"/GS1 ",
        b"gs\n0 0 0 1 k\n",
    );
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: true,
            transparency: false,
            unresolved: false,
            unclassified: false,
            gs_count: 1,
        },
    );
}

#[test]
fn page_without_declared_resources_and_no_gs_still_converts() {
    let input = classic_raw_pdf(b"0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
}

#[test]
fn page_transparency_group_without_gs_skips_whole_page() {
    let input = classic_page_group_pdf("<< /S /Transparency >>", b"0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::TransparencyGroupUnsafe {
            transparency: true,
            unresolved: false,
            unclassified: false,
        },
    );
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
}

// --- Semantic resource-name matching ------------------------------------------

/// `/GS1` and `/GS#31` are the same PDF name (§7.3.5); two classified resources
/// with that semantic name are an ambiguous duplicate the guard cannot first-win,
/// so a `gs` on EITHER raw spelling fails closed as unclassified.
#[test]
fn semantic_resource_collision_poisons_regardless_of_operand_spelling() {
    for operand in ["GS1", "GS#31"] {
        let stream = format!("/{operand} gs\n0 0 0 1 k\n").into_bytes();
        let input = classic_extgstate_pdf(
            "<< /ExtGState << /GS1 << /BM /Normal >> /GS#31 << /BM /Normal >> >> >>",
            &stream,
        );
        let output = convert_cmyk(&input);
        assert_extgstate_skip(
            &output,
            ConvertPageSkipReason::ExtGStateUnsafe {
                overprint: false,
                transparency: false,
                unresolved: false,
                unclassified: true,
                gs_count: 1,
            },
        );
    }
}

/// An escaped `gs` operand resolves the correct classified safety resource by
/// decoded-name equality, so an escaped operand still finds an unsafe raw
/// resource (and vice versa).
#[test]
fn escaped_operand_and_resource_resolve_the_correct_safety_resource() {
    // Escaped operand, raw resource.
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /OP true >> >> >>",
        b"/GS#31 gs\n0 0 0 1 k\n",
    );
    assert_extgstate_skip(
        &convert_cmyk(&input),
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: true,
            transparency: false,
            unresolved: false,
            unclassified: false,
            gs_count: 1,
        },
    );

    // Escaped-only resource declaration, raw operand: same overprint outcome.
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS#31 << /OP true >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    assert_extgstate_skip(
        &convert_cmyk(&input),
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: true,
            transparency: false,
            unresolved: false,
            unclassified: false,
            gs_count: 1,
        },
    );
}

/// A structural skip whose decoded name matches the operand poisons the match,
/// even when its raw spelling differs from every classified resource.
#[test]
fn semantic_matching_skip_poisons_as_unclassified() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS#31 << /BM /Normal >> /GS#31 << /BM /Normal >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    assert_extgstate_skip(
        &convert_cmyk(&input),
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: false,
            unclassified: true,
            gs_count: 1,
        },
    );
}

/// A malformed (non-hex) escape in the `gs` operand can never be proven to name
/// one classified resource; fail closed as unclassified.
#[test]
fn malformed_escaped_operand_fails_closed() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS1 << /LW 2 >> >> >>",
        b"/GS#ZZ gs\n0 0 0 1 k\n",
    );
    assert_extgstate_skip(
        &convert_cmyk(&input),
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: false,
            unclassified: true,
            gs_count: 1,
        },
    );
}

/// An unrelated unknown key never invents a semantic match: a `gs` naming an
/// absent resource stays a plain unresolved fail-closed skip.
#[test]
fn unrelated_unknown_key_stays_unresolved() {
    let input = classic_extgstate_pdf(
        "<< /ExtGState << /GS2 << /LW 2 >> >> >>",
        b"/GS1 gs\n0 0 0 1 k\n",
    );
    assert_extgstate_skip(
        &convert_cmyk(&input),
        ConvertPageSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: true,
            unclassified: false,
            gs_count: 1,
        },
    );
}

// --- Incomplete-coverage (namespace-level skip) fail-closed ------------------

use presslint_pdf::{
    ClassifiedExtGStateResource, DictionaryValueKind, ExtGStateFontEffect, ExtGStateParamClass,
    IndirectObjectEditDisposition, IndirectRef, PageExtGStateResourcesInspection, PdfName,
    SkippedExtGStateResource, SkippedExtGStateResourceReason,
};

use crate::content_edit_pipeline::PipelineSkipReason;
use crate::extgstate_page_guard::extgstate_page_skip_reason;
use crate::page_content_sequence::{OccurrenceInput, PageContentSequence};

/// A classified `/ExtGState` resource carrying no unsafe safety parameter: the
/// guard converts a `gs` naming it, absent any coverage doubt.
fn safe_classified(name: &[u8]) -> ClassifiedExtGStateResource {
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
        font_effect: ExtGStateFontEffect::Unset,
    }
}

fn extgstate_inspection(
    extgstates: Vec<ClassifiedExtGStateResource>,
    skipped: Vec<SkippedExtGStateResource>,
) -> PageExtGStateResourcesInspection {
    PageExtGStateResourcesInspection {
        ordinal: 0,
        page_reference: IndirectRef {
            object_number: 3,
            generation: 0,
        },
        page_object_byte_offset: 30,
        extgstates,
        skipped,
    }
}

fn content_sequence(decoded: &[u8]) -> PageContentSequence {
    let inputs = [OccurrenceInput {
        stream_ordinal: 0,
        content_object: IndirectRef {
            object_number: 4,
            generation: 0,
        },
        decoded,
        disposition: IndirectObjectEditDisposition::InPlaceMutation,
    }];
    PageContentSequence::new(&inputs, 1 << 20).expect("sequence parses")
}

fn gs_sequence_for(raw_operand: &[u8]) -> PageContentSequence {
    let mut decoded = Vec::with_capacity(raw_operand.len() + 18);
    decoded.push(b'/');
    decoded.extend_from_slice(raw_operand);
    decoded.extend_from_slice(b" gs\n0 0 0 1 k\n");
    content_sequence(&decoded)
}

fn gs_sequence() -> PageContentSequence {
    gs_sequence_for(b"GS1")
}

fn named_skip(name: &[u8]) -> SkippedExtGStateResource {
    SkippedExtGStateResource {
        object_byte_offset: 30,
        resource_name: Some(PdfName(name.to_vec())),
        reason: SkippedExtGStateResourceReason::NonDictionaryEntry {
            value_kind: DictionaryValueKind::Boolean,
        },
    }
}

/// A `gs` naming a resource that WAS classified converts, because the classified
/// set is authoritative when nothing was skipped.
#[test]
fn classified_gs_without_any_skip_converts() {
    let report = extgstate_inspection(vec![safe_classified(b"GS1")], Vec::new());
    assert_eq!(
        extgstate_page_skip_reason(Some(&report), &gs_sequence()),
        None
    );
}

/// A namespace-level (nameless) structural skip proves the classified set is
/// incomplete. Even though `/GS1` is still classified and safe, the guard can no
/// longer trust it hides no unsafe sibling, so it fails closed as unclassified.
/// (The guard keys on the missing resource name, not the specific reason, so any
/// nameless skip triggers this.)
#[test]
fn namespace_level_skip_beside_a_classified_resource_fails_closed() {
    let report = extgstate_inspection(
        vec![safe_classified(b"GS1")],
        vec![SkippedExtGStateResource {
            object_byte_offset: 30,
            resource_name: None,
            reason: SkippedExtGStateResourceReason::MissingExtGState,
        }],
    );
    assert_eq!(
        extgstate_page_skip_reason(Some(&report), &gs_sequence()),
        Some(PipelineSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: false,
            unclassified: true,
            gs_count: 1,
        })
    );
}

/// Strict decoding rejects a truncated escape, a non-hex escape, and `#00`.
/// A permissive reader may instead retain those bytes literally, so a malformed
/// classified sibling must poison a well-formed spelling that decodes to those
/// exact bytes rather than disappear as an unrelated non-match.
#[test]
fn malformed_classified_sibling_is_a_literal_spelling_poison() {
    for (malformed, encoded_literal) in [
        (&b"GS#"[..], &b"GS#23"[..]),
        (&b"GS#X"[..], &b"GS#23X"[..]),
        (&b"GS#00"[..], &b"GS#2300"[..]),
    ] {
        let report = extgstate_inspection(
            vec![safe_classified(encoded_literal), safe_classified(malformed)],
            Vec::new(),
        );
        assert_eq!(
            extgstate_page_skip_reason(Some(&report), &gs_sequence_for(encoded_literal)),
            Some(PipelineSkipReason::ExtGStateUnsafe {
                overprint: false,
                transparency: false,
                unresolved: false,
                unclassified: true,
                gs_count: 1,
            }),
            "malformed classified spelling {malformed:?} did not poison"
        );
    }
}

/// The same literal-spelling rule applies to named structural skips: none may
/// be discarded when its raw bytes equal the decoded `gs` operand.
#[test]
fn malformed_named_skip_is_a_literal_spelling_poison() {
    for (malformed, encoded_literal) in [
        (&b"GS#"[..], &b"GS#23"[..]),
        (&b"GS#X"[..], &b"GS#23X"[..]),
        (&b"GS#00"[..], &b"GS#2300"[..]),
    ] {
        let report = extgstate_inspection(
            vec![safe_classified(encoded_literal)],
            vec![named_skip(malformed)],
        );
        assert_eq!(
            extgstate_page_skip_reason(Some(&report), &gs_sequence_for(encoded_literal)),
            Some(PipelineSkipReason::ExtGStateUnsafe {
                overprint: false,
                transparency: false,
                unresolved: false,
                unclassified: true,
                gs_count: 1,
            }),
            "malformed skipped spelling {malformed:?} did not poison"
        );
    }
}

#[test]
fn unrelated_malformed_names_do_not_poison_a_distinct_safe_match() {
    let report = extgstate_inspection(
        vec![safe_classified(b"GS1"), safe_classified(b"Other#X")],
        vec![named_skip(b"Skipped#")],
    );
    assert_eq!(
        extgstate_page_skip_reason(Some(&report), &gs_sequence()),
        None
    );
}

#[test]
fn well_formed_unmatched_operand_stays_unresolved_beside_malformed_names() {
    let report = extgstate_inspection(
        vec![safe_classified(b"GS1"), safe_classified(b"Other#X")],
        vec![named_skip(b"Skipped#")],
    );
    assert_eq!(
        extgstate_page_skip_reason(Some(&report), &gs_sequence_for(b"GS2")),
        Some(PipelineSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: true,
            unclassified: false,
            gs_count: 1,
        })
    );
}

#[test]
fn malformed_declarations_without_an_executed_gs_do_not_skip() {
    let report = extgstate_inspection(vec![safe_classified(b"GS#X")], vec![named_skip(b"GS#")]);
    assert_eq!(
        extgstate_page_skip_reason(Some(&report), &content_sequence(b"0 0 0 1 k\n")),
        None
    );
}

#[test]
fn malformed_page_group_without_gs_skips_whole_page() {
    let input = classic_page_group_pdf("42", b"0 0 0 1 k\n");
    let output = convert_cmyk(&input);

    assert_extgstate_skip(
        &output,
        ConvertPageSkipReason::TransparencyGroupUnsafe {
            transparency: false,
            unresolved: true,
            unclassified: true,
        },
    );
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
}
