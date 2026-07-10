//! Resource-aware end-to-end conversion: page device aliases, inheritance,
//! the fail-closed `/Default*` interlock, multi-stream alias state, byte
//! preservation, and reopen behaviour.

use presslint_pdf::{DocumentAccessBackend, encode_flate_stream};
use presslint_selectors::Predicate;
use presslint_types::{ColorSpace, PageIndex};

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    ConvertPageSkipReason, DeviceLinkInput, PageSelection, convert_content_colors_incremental,
};

use super::content_color_convert::{
    CMYK_TO_CMYK_LINK, GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, assemble_classic, contains, convert,
    convert_with_target, link_bytes, occurrence_count, page_body, page_decoded_stream, predicate,
    stream_body,
};
use super::{reopen, xref_record};

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
const FLATE_LIMIT: usize = 1 << 20;

/// Exact direct page device aliases for all three families.
const ALIAS_RESOURCES: &str =
    "<< /ColorSpace << /GrayAlias /DeviceGray /RgbAlias /DeviceRGB /CmykAlias /DeviceCMYK >> >>";

fn resource_page_body(contents: &str, resources: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} /Resources {resources} >>"
    )
    .into_bytes()
}

/// One-page classic PDF whose leaf page carries `/Resources`.
fn resource_pdf(stream: &[u8], resources: &str) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        resource_page_body("4 0 R", resources),
        stream_body("", stream),
    ])
}

/// Convert through several links, `PageSelection::All`, no black preservation.
fn convert_links(input: &[u8], links: &[&str]) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: links
                .iter()
                .map(|hex| DeviceLinkInput {
                    id: None,
                    bytes: link_bytes(hex),
                })
                .collect(),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds")
}

// --- Alias eligibility counts, bytes verbatim ---------------------------------

#[test]
fn exact_alias_setters_count_eligible_while_all_alias_bytes_stay_verbatim() {
    let stream = b"/GrayAlias cs 0.5 sc\n/GrayAlias CS 0.6 SC\n/RgbAlias cs 0.1 0.2 0.3 scn\n/CmykAlias CS 0 0 0 1 SCN\n1 0 0 rg\n";
    let input = resource_pdf(stream, ALIAS_RESOURCES);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 4);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.operator_skips.default_color_space_unsafe, 0);

    // The direct rg shortcut converted; every alias byte is verbatim.
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(contains(&decoded, b"/GrayAlias CS 0.6 SC"));
    assert!(contains(&decoded, b"/RgbAlias cs 0.1 0.2 0.3 scn"));
    assert!(contains(&decoded, b"/CmykAlias CS 0 0 0 1 SCN"));
    assert!(!contains(&decoded, b" rg"));
}

#[test]
fn indirect_alias_definition_counts_eligible() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        resource_page_body("4 0 R", "<< /ColorSpace << /GrayAlias 5 0 R >> >>"),
        stream_body("", b"/GrayAlias cs 0.5 sc\n"),
        b"/DeviceGray".to_vec(),
    ]);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted[0].resource_alias_setters_eligible, 1);
    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc\n"
    );
}

#[test]
fn malformed_alias_setters_count_ineligible_and_stay_verbatim() {
    let stream =
        b"/GrayAlias cs 1.5 sc\n/GrayAlias cs 0.5 0.5 sc\n/GrayAlias cs 0.5 /GrayAlias scn\n";
    let input = resource_pdf(stream, ALIAS_RESOURCES);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 0);
    assert_eq!(page.resource_alias_setters_ineligible, 3);
    assert_eq!(page.operators_converted, 0);
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(page_decoded_stream(&output.bytes, false), stream.as_slice());
}

#[test]
fn unwalkable_alias_setters_refuse_the_page_without_false_counts() {
    let mut non_finite = b"/GrayAlias cs ".to_vec();
    non_finite.extend(std::iter::repeat_n(b'9', 400));
    non_finite.extend_from_slice(b" sc\n");
    let cases = [
        b"/GrayAlias cs true sc\n".to_vec(),
        b"/GrayAlias cs [0.5] sc\n".to_vec(),
        non_finite,
    ];

    for stream in cases {
        let input = resource_pdf(&stream, ALIAS_RESOURCES);
        let output = convert(&input, GRAY_TO_GRAY_LINK);

        assert!(output.converted.is_empty());
        assert_eq!(output.skipped.len(), 1);
        assert_eq!(
            output.skipped[0].reason,
            ConvertPageSkipReason::ContentRoundTripMismatch
        );
        assert_eq!(&output.bytes[..input.len()], input.as_slice());
        assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    }
}

#[test]
fn non_device_resource_families_are_excluded_and_uncounted() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        resource_page_body(
            "4 0 R",
            "<< /ColorSpace << /Icc [/ICCBased 5 0 R] /Sep [/Separation /Spot /DeviceCMYK 6 0 R] >> >>",
        ),
        stream_body("", b"/Icc cs 0 0 0 sc\n/Sep CS 1 SCN\n1 0 0 rg\n"),
        b"<< /N 3 /Length 0 >>\nstream\n\nendstream".to_vec(),
        b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [1 1 1 1] /N 1 >>".to_vec(),
    ]);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 0);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    // The direct shortcut still converts next to the untouched special spaces.
    assert_eq!(page.operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/Icc cs 0 0 0 sc"));
    assert!(contains(&decoded, b"/Sep CS 1 SCN"));
}

#[test]
fn cross_stream_alias_selection_feeds_a_later_occurrence_setter() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        resource_page_body("[4 0 R 5 0 R]", ALIAS_RESOURCES),
        stream_body("", b"/GrayAlias cs\n"),
        stream_body("", b"0.5 sc\n"),
    ]);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
}

#[test]
fn restore_drops_alias_state_and_the_later_setter_is_uncounted() {
    let stream = b"q /GrayAlias cs Q 0.5 sc\n";
    let input = resource_pdf(stream, ALIAS_RESOURCES);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 0);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    assert_eq!(page_decoded_stream(&output.bytes, false), stream.as_slice());
}

#[test]
fn distinct_trailing_pattern_name_counts_the_selected_alias_ineligible() {
    let stream = b"/GrayAlias cs 0.5 /PatternPaint scn\n";
    let input = resource_pdf(stream, ALIAS_RESOURCES);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 0);
    assert_eq!(page.resource_alias_setters_ineligible, 1);
    assert_eq!(page_decoded_stream(&output.bytes, false), stream.as_slice());
}

#[test]
fn malformed_unresolved_cyclic_and_wrong_object_aliases_are_uncounted() {
    let cases = [
        (
            "malformed array",
            "<< /ColorSpace << /BadAlias [] >> >>",
            Vec::new(),
            false,
        ),
        (
            "unresolved reference",
            "<< /ColorSpace << /BadAlias 99 0 R >> >>",
            Vec::new(),
            true,
        ),
        (
            "cyclic resources reference",
            "5 0 R",
            vec![b"6 0 R".to_vec(), b"5 0 R".to_vec()],
            false,
        ),
        (
            "wrong referenced object kind",
            "<< /ColorSpace << /BadAlias 5 0 R >> >>",
            vec![b"42".to_vec()],
            false,
        ),
    ];

    for (label, resources, extras, ownership_vetoed) in cases {
        let mut objects = vec![
            CATALOG.to_vec(),
            PAGES.to_vec(),
            resource_page_body("4 0 R", resources),
            stream_body("", b"/BadAlias cs 0.5 sc\n"),
        ];
        objects.extend(extras);
        let input = assemble_classic(&objects);
        let output = convert(&input, GRAY_TO_GRAY_LINK);
        if ownership_vetoed {
            assert!(output.converted.is_empty(), "{label}");
            assert_eq!(output.skipped.len(), 1, "{label}");
            assert!(
                matches!(
                    output.skipped[0].reason,
                    ConvertPageSkipReason::OwnershipNotInPlace { .. }
                ),
                "{label}: {:?}",
                output.skipped
            );
            assert_eq!(&output.bytes[..input.len()], input.as_slice(), "{label}");
            continue;
        }
        let page = output
            .converted
            .first()
            .unwrap_or_else(|| panic!("{label}: unexpected skips {:?}", output.skipped));

        assert_eq!(page.resource_alias_setters_eligible, 0, "{label}");
        assert_eq!(page.resource_alias_setters_ineligible, 0, "{label}");
        assert_eq!(
            page_decoded_stream(&output.bytes, false),
            b"/BadAlias cs 0.5 sc\n",
            "{label}"
        );
    }
}

#[test]
fn duplicate_alias_name_is_ineligible_and_never_enters_the_environment() {
    let stream = b"/GrayAlias cs 0.5 sc\n";
    let input = resource_pdf(
        stream,
        "<< /ColorSpace << /GrayAlias /DeviceGray /GrayAlias /DeviceRGB >> >>",
    );
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 0);
    assert_eq!(page.resource_alias_setters_ineligible, 1);
    assert_eq!(page_decoded_stream(&output.bytes, false), stream.as_slice());
}

#[test]
fn inherited_alias_is_available_to_the_leaf_page() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /ColorSpace << /GrayAlias /DeviceGray >> >> >>"
            .to_vec(),
        page_body("4 0 R"),
        stream_body("", b"/GrayAlias cs 0.5 sc\n"),
    ]);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted[0].resource_alias_setters_eligible, 1);
    assert_eq!(output.converted[0].resource_alias_setters_ineligible, 0);
}

#[test]
fn child_resources_replace_inherited_aliases_instead_of_merging() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /ColorSpace << /GrayAlias /DeviceGray >> >> >>"
            .to_vec(),
        resource_page_body("4 0 R", "<< >>"),
        stream_body("", b"/GrayAlias cs 0.5 sc\n"),
    ]);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted[0].resource_alias_setters_eligible, 0);
    assert_eq!(output.converted[0].resource_alias_setters_ineligible, 0);
}

#[test]
fn form_local_alias_does_not_leak_into_the_page_policy_or_descend() {
    let page_stream = b"/GrayAlias cs 0.5 sc\n/Fm Do\n";
    let form_stream = b"/GrayAlias cs 0.6 sc\n";
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        resource_page_body("4 0 R", "<< /XObject << /Fm 5 0 R >> >>"),
        stream_body("", page_stream),
        stream_body(
            "/Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /ColorSpace << /GrayAlias /DeviceGray >> >>",
            form_stream,
        ),
    ]);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 0);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    assert_eq!(page_decoded_stream(&output.bytes, false), page_stream);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
    assert!(contains(&output.bytes, form_stream));
}

#[test]
fn repeated_content_reference_reconciles_alias_counts_once() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        resource_page_body("[4 0 R 4 0 R]", ALIAS_RESOURCES),
        stream_body("", b"/GrayAlias cs 0.5 sc\n"),
    ]);
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted[0].resource_alias_setters_eligible, 1);
    assert_eq!(output.converted[0].resource_alias_setters_ineligible, 0);
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
}

#[test]
fn ownership_vetoed_alias_selection_still_feeds_a_private_setter() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 2 >>".to_vec(),
        resource_page_body("[5 0 R 4 0 R]", ALIAS_RESOURCES),
        stream_body("", b"0.5 sc\n"),
        stream_body("", b"/GrayAlias cs\n"),
        page_body("5 0 R"),
    ]);
    let output = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::Indices(vec![PageIndex(0)]),
            device_links: vec![DeviceLinkInput {
                id: None,
                bytes: link_bytes(GRAY_TO_GRAY_LINK),
            }],
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds");

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    assert_eq!(
        page.content_objects
            .iter()
            .map(|object| object.object_number)
            .collect::<Vec<_>>(),
        vec![4]
    );
    assert_eq!(output.skipped.len(), 1);
    assert!(matches!(
        output.skipped[0].reason,
        ConvertPageSkipReason::OwnershipNotInPlace { occurrences: 2, .. }
    ));
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(occurrence_count(&output.bytes, b"5 0 obj"), 1);
}

#[test]
fn conservative_epoch_boundaries_keep_counts_bytes_and_direct_conversion() {
    // Text showing, XObject invocation, compatibility sections, and unknown
    // operators refuse only the PRIVATE alias epoch: the structural setter
    // counts, every alias byte, and the neighbouring direct shortcut
    // conversion are all unchanged.
    let stream = b"/GrayAlias cs 0.5 sc BT (x) Tj ET /Fm Do BX EX XY 1 0 0 rg\n";
    let input = resource_pdf(stream, ALIAS_RESOURCES);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    assert_eq!(page.operators_converted, 1);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(contains(&decoded, b"BT (x) Tj ET /Fm Do BX EX XY"));
    assert!(!contains(&decoded, b" rg"));
}

#[test]
fn trailing_unmatched_save_keeps_counts_and_direct_conversion() {
    // The walker tolerates a trailing unmatched q; the plan privately refuses
    // every alias epoch for the page while the public structural counts and
    // the direct shortcut conversion stay exactly as before.
    let stream = b"q /GrayAlias cs 0.5 sc f 1 0 0 rg\n";
    let input = resource_pdf(stream, ALIAS_RESOURCES);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_setters_ineligible, 0);
    assert_eq!(page.operators_converted, 1);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"q /GrayAlias cs 0.5 sc f"));
    assert!(!contains(&decoded, b" rg"));
}

// --- The fail-closed /Default* interlock --------------------------------------

#[test]
fn replaced_source_default_refuses_the_direct_conversion() {
    let input = resource_pdf(
        b"1 0 0 rg\n",
        "<< /ColorSpace << /DefaultRGB /DeviceCMYK >> >>",
    );
    let output = convert(&input, RGB_TO_CMYK_LINK);

    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 0);
    assert_eq!(page.black_preserved, 0);
    assert_eq!(page.operator_skips.default_color_space_unsafe, 1);
    assert_eq!(page.operator_skips.no_matching_link, 0);
    assert_eq!(page.links[0].operators_converted, 0);
    // No revision object is appended for the page: the input is verbatim.
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(page_decoded_stream(&output.bytes, false), b"1 0 0 rg\n");
}

#[test]
fn replaced_destination_default_refuses_the_direct_conversion() {
    let input = resource_pdf(
        b"1 0 0 rg\n",
        "<< /ColorSpace << /DefaultCMYK /DeviceRGB >> >>",
    );
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        1
    );
    assert_eq!(page_decoded_stream(&output.bytes, false), b"1 0 0 rg\n");
}

#[test]
fn replaced_source_and_destination_count_once_per_operator() {
    let input = resource_pdf(
        b"1 0 0 rg\n0 1 0 RG\n",
        "<< /ColorSpace << /DefaultRGB /DeviceCMYK /DefaultCMYK /DeviceRGB >> >>",
    );
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        2
    );
}

#[test]
fn identity_defaults_keep_the_direct_conversion_safe() {
    let input = resource_pdf(
        b"1 0 0 rg\n",
        "<< /ColorSpace << /DefaultRGB /DeviceRGB /DefaultCMYK /DeviceCMYK >> >>",
    );
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        0
    );
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    assert!(contains(&decoded, b" k"));
}

#[test]
fn same_family_link_checks_one_identity_status() {
    let input = resource_pdf(
        b"0.5 g\n",
        "<< /ColorSpace << /DefaultGray /DeviceGray >> >>",
    );
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        0
    );
}

#[test]
fn empty_color_space_dictionary_proves_absence_and_converts() {
    let input = resource_pdf(b"1 0 0 rg\n", "<< /ColorSpace << >> >>");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        0
    );
}

#[test]
fn family_specific_unknown_refuses_only_that_family() {
    // /CalRGB is a family-specific unclassified default: RGB becomes unknown
    // while gray/cmyk stay provably absent.
    let input = resource_pdf(
        b"1 0 0 rg\n0 0 0 1 k\n",
        "<< /ColorSpace << /DefaultRGB [/CalRGB << >>] >> >>",
    );
    let output = convert_links(&input, &[RGB_TO_CMYK_LINK, CMYK_TO_CMYK_LINK]);

    let page = &output.converted[0];
    assert_eq!(page.operator_skips.default_color_space_unsafe, 1);
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.links[0].operators_converted, 0);
    assert_eq!(page.links[1].operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"1 0 0 rg"));
    assert!(!contains(&decoded, b"0 0 0 1 k"));
}

#[test]
fn malformed_resources_value_poisons_all_families() {
    // A non-dictionary /Resources value is a general resource failure: it
    // yields BOTH a Resources skip and a MissingResources fact, and the
    // failure takes precedence, so every family is unknown and refused.
    // (A dangling /Resources reference is exercised separately: it already
    // fails closed earlier through the document ownership veto.)
    let input = resource_pdf(b"1 0 0 rg\n", "(bad)");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        1
    );
    assert_eq!(page_decoded_stream(&output.bytes, false), b"1 0 0 rg\n");
}

#[test]
fn inherited_default_from_the_pages_node_applies_to_the_leaf() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /ColorSpace << /DefaultRGB /DeviceCMYK >> >> >>"
            .to_vec(),
        page_body("4 0 R"),
        stream_body("", b"1 0 0 rg\n"),
    ]);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        1
    );
}

#[test]
fn child_resources_replace_inherited_defaults_not_merge() {
    // The leaf's own (default-free) /Resources REPLACES the inherited unsafe
    // dictionary, so absence is proven and the conversion proceeds.
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /ColorSpace << /DefaultRGB /DeviceCMYK >> >> >>"
            .to_vec(),
        resource_page_body("4 0 R", "<< >>"),
        stream_body("", b"1 0 0 rg\n"),
    ]);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        0
    );
}

#[test]
fn selector_exclusion_takes_precedence_over_the_default_interlock() {
    let input = resource_pdf(
        b"1 0 0 rg\n",
        "<< /ColorSpace << /DefaultRGB /DeviceCMYK >> >>",
    );
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::ColorSpace {
            space: ColorSpace::DeviceCmyk,
        }),
    );

    assert_eq!(output.converted[0].operator_skips.selector_excluded, 1);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        0
    );
}

#[test]
fn missing_route_takes_precedence_over_the_default_interlock() {
    let input = resource_pdf(
        b"0.5 g\n",
        "<< /ColorSpace << /DefaultGray /DeviceCMYK >> >>",
    );
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operator_skips.no_matching_link, 1);
    assert_eq!(
        output.converted[0]
            .operator_skips
            .default_color_space_unsafe,
        0
    );
}

#[test]
fn safe_family_still_converts_next_to_verbatim_aliases_and_unsafe_family() {
    // One page: RGB route unsafe (replaced default), CMYK route safe, plus an
    // eligible gray alias setter that stays verbatim.
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc\n1 0 0 rg\n0 0 0 1 k\n",
        "<< /ColorSpace << /GrayAlias /DeviceGray /DefaultRGB /DeviceCMYK >> >>",
    );
    let output = convert_links(&input, &[RGB_TO_CMYK_LINK, CMYK_TO_CMYK_LINK]);

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.operator_skips.default_color_space_unsafe, 1);
    assert_eq!(page.operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(contains(&decoded, b"1 0 0 rg"));
    assert!(!contains(&decoded, b"0 0 0 1 k"));
}

// --- Encoding and backend round trips ------------------------------------------

#[test]
fn flate_resource_page_converts_and_keeps_aliases_verbatim() {
    let decoded_input = b"/GrayAlias cs 0.5 sc\n1 0 0 rg\n";
    let compressed = encode_flate_stream(decoded_input, FLATE_LIMIT).expect("encode");
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        resource_page_body("4 0 R", ALIAS_RESOURCES),
        stream_body(" /Filter /FlateDecode", &compressed),
    ]);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].resource_alias_setters_eligible, 1);
    assert_eq!(output.converted[0].operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, true);
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(!contains(&decoded, b" rg"));
    reopen(&output.bytes);
}

#[test]
fn xref_stream_resource_page_converts_and_reopens() {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
    let mut page = Vec::new();
    page.extend_from_slice(b"3 0 obj\n");
    page.extend_from_slice(&resource_page_body("4 0 R", ALIAS_RESOURCES));
    page.extend_from_slice(b"\nendobj\n");
    let mut object4 = Vec::new();
    object4.extend_from_slice(b"4 0 obj\n");
    object4.extend_from_slice(&stream_body("", b"/GrayAlias cs 0.5 sc\n1 0 0 rg\n"));
    object4.extend_from_slice(b"\nendobj\n");

    let catalog_offset = buf.len();
    buf.extend_from_slice(catalog);
    let pages_offset = buf.len();
    buf.extend_from_slice(pages);
    let page_offset = buf.len();
    buf.extend_from_slice(&page);
    let content_offset = buf.len();
    buf.extend_from_slice(&object4);
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(1, page_offset, 0));
    body.extend_from_slice(&xref_record(1, content_offset, 0));
    body.extend_from_slice(&xref_record(1, xref_offset, 0));
    buf.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /XRef /Size 6 /Index [0 6] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());

    let output = convert(&buf, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..buf.len()], buf.as_slice());
    assert_eq!(output.converted[0].resource_alias_setters_eligible, 1);
    assert_eq!(output.converted[0].operators_converted, 1);
    assert!(matches!(
        reopen(&output.bytes).backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(!contains(&decoded, b" rg"));
}
