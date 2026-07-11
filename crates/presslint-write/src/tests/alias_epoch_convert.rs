//! Root-atomic page-alias conversion and physical-record reporting.

use presslint_selectors::{Predicate, Selector};
use presslint_types::ColorUsage;
use presslint_types::PageIndex;

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    DeviceLinkInput, PageSelection, convert_content_colors_incremental,
};

use super::content_color_convert::{
    CMYK_TO_CMYK_LINK, GRAY_TO_GRAY_LINK, LAB_TO_RGB_LINK, RGB_TO_CMYK_LINK, assemble_classic,
    contains, link_bytes, occurrence_count, page_decoded_stream, stream_body,
};

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
const ALIASES: &str =
    "<< /ColorSpace << /GrayAlias /DeviceGray /RgbAlias /DeviceRGB /CmykAlias /DeviceCMYK >> >>";

fn page(contents: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} /Resources {ALIASES} >>"
    )
    .into_bytes()
}

fn raw_pdf(content: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page("4 0 R"),
        stream_body("", content),
    ])
}

fn convert_links(
    input: &[u8],
    links: Vec<DeviceLinkInput>,
    black_preservation: BlackPreservationPolicy,
) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: links,
            black_preservation,
            target: None,
        },
    )
    .expect("convert succeeds")
}

fn links(hex: &[&str]) -> Vec<DeviceLinkInput> {
    hex.iter()
        .map(|value| DeviceLinkInput {
            id: None,
            bytes: link_bytes(value),
        })
        .collect()
}

#[test]
fn selection_and_setter_records_convert_whole_for_every_family_and_lane() {
    let content = b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f\n\
/GrayAlias CS 0.6 SC 0 0 m 1 1 l S\n\
/RgbAlias cs 0.1 0.2 0.3 scn 0 0 m 1 1 l f\n\
/RgbAlias CS 0.3 0.2 0.1 SCN 0 0 m 1 1 l S\n\
/CmykAlias cs 0.1 0.2 0.3 0.4 sc 0 0 m 1 1 l f\n\
/CmykAlias CS 0.4 0.3 0.2 0.1 SC 0 0 m 1 1 l S\n";
    let input = raw_pdf(content);
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK, CMYK_TO_CMYK_LINK]),
        BlackPreservationPolicy::None,
    );

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 12);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert_eq!(page.resource_alias_setters_eligible, 6);
    assert_eq!(page.operators_converted, 12);
    assert_eq!(page.links[0].operators_converted, 4);
    assert_eq!(page.links[1].operators_converted, 4);
    assert_eq!(page.links[2].operators_converted, 4);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"Alias"));
    assert!(!contains(&decoded, b" sc"));
    assert!(!contains(&decoded, b" SC"));
    assert!(contains(&decoded, b" g"));
    assert!(contains(&decoded, b" G"));
    assert!(contains(&decoded, b" k"));
    assert!(contains(&decoded, b" K"));
}

#[test]
fn black_overlay_precedes_apply_for_selection_and_explicit_setter() {
    let input = raw_pdf(b"/RgbAlias cs 0 0 0 scn 0 0 m 1 1 l f\n");
    let output = convert_links(
        &input,
        links(&[RGB_TO_CMYK_LINK]),
        BlackPreservationPolicy::NeutralBlackToK,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(page.operators_converted, 0);
    assert_eq!(page.black_preserved, 2);
    assert_eq!(page.links[0].operators_converted, 0);
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"0 0 0 1 k 0 0 0 1 k 0 0 m 1 1 l f\n"
    );
}

#[test]
fn consumed_structural_refusal_rolls_back_root_but_not_direct_operator() {
    let input = raw_pdf(b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f /Fm Do\n1 0 0 rg\n");
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.links[0].operators_converted, 0);
    assert_eq!(page.links[1].operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/GrayAlias cs 0.5 sc"));
    assert!(!contains(&decoded, b"1 0 0 rg"));
}

#[test]
fn structural_refusal_before_a_supported_consumer_is_reported() {
    let input = raw_pdf(b"/GrayAlias cs /Fm Do\n");
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 1);
    assert_eq!(page.operators_converted, 0);
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs /Fm Do\n"
    );
}

#[test]
fn retained_selector_decision_refuses_whole_root_without_reselection() {
    let input = raw_pdf(b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f\n");
    let output = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: links(&[GRAY_TO_GRAY_LINK]),
            black_preservation: BlackPreservationPolicy::None,
            target: Some(Selector::Predicate {
                predicate: Predicate::ColorUsage {
                    usage: ColorUsage::Stroke,
                },
            }),
        },
    )
    .expect("convert succeeds");

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(page.operator_skips.selector_excluded, 0);
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f\n"
    );
}

#[test]
fn unique_closed_no_consumer_root_is_a_silent_noop() {
    let input = raw_pdf(b"/GrayAlias cs 0.5 sc\n");
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert_eq!(page.resource_alias_setters_eligible, 1);
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc\n"
    );
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
}

#[test]
fn shared_no_consumer_root_blocks_consumed_cotenant_and_reports_once() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page("[4 0 R 4 0 R 5 0 R]"),
        stream_body("", b"/GrayAlias cs 0.5 sc\n"),
        stream_body("", b"0 0 m 1 1 l f\n"),
    ]);
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(page.operators_converted, 0);
    assert!(contains(&output.bytes, b"/GrayAlias cs 0.5 sc"));
}

#[test]
fn save_restore_converts_the_complete_root_with_restored_outer_tuple() {
    let input = raw_pdf(b"/GrayAlias cs 0.2 sc q 0.8 sc 0 0 m 1 1 l f Q 0 0 m 1 1 l f\n");
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 3);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert_eq!(page.operators_converted, 3);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"Alias"));
    assert!(!contains(&decoded, b" sc"));
    assert!(contains(&decoded, b" q "));
    assert!(contains(&decoded, b" Q "));
}

#[test]
fn invalid_path_placement_remains_refused_at_real_conversion_boundary() {
    let input = raw_pdf(b"/GrayAlias cs 0 0 m 0.5 sc 1 1 l f\n");
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0 0 m 0.5 sc 1 1 l f\n"
    );
}

#[test]
fn identical_repeated_roots_convert_once_per_physical_record() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page("[4 0 R 4 0 R]"),
        stream_body("", b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f\n"),
    ]);
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(page.operators_converted, 2);
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"Alias"));
}

#[test]
fn three_occurrence_late_divergence_stays_poisoned_at_conversion() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page("[4 0 R 5 0 R 6 0 R 5 0 R 4 0 R 5 0 R 7 0 R]"),
        stream_body("", b"/GrayAlias cs\n"),
        stream_body("", b"0.5 sc 0 0 m 1 1 l f\n"),
        stream_body("", b"0.25 g\n"),
        stream_body("", b"1 0 0 rg\n"),
    ]);
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(page.operators_converted, 2);
    assert!(contains(&output.bytes, b"/GrayAlias cs"));
    assert!(contains(&output.bytes, b"0.5 sc"));
}

#[test]
fn shared_record_failure_propagates_transitively_but_direct_survives() {
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page("[4 0 R 5 0 R 6 0 R 4 0 R 7 0 R 8 0 R 5 0 R 6 0 R 9 0 R]"),
        stream_body("", b"/GrayAlias cs\n"),
        stream_body("", b"0.5 sc\n"),
        stream_body("", b"0 0 m 1 1 l f\n"),
        stream_body("", b"0.6 sc 0 0 m 1 1 l f /Fm Do\n"),
        stream_body("", b"/GrayAlias cs\n"),
        stream_body("", b"1 0 0 rg\n"),
    ]);
    let output = convert_links(
        &input,
        links(&[GRAY_TO_GRAY_LINK, RGB_TO_CMYK_LINK]),
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 4);
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.links[0].operators_converted, 0);
    assert_eq!(page.links[1].operators_converted, 1);
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(contains(&output.bytes, b"/GrayAlias cs"));
}

#[test]
fn lazy_native_build_failure_rolls_back_all_root_candidates() {
    let mut broken = link_bytes(LAB_TO_RGB_LINK);
    broken[16..20].copy_from_slice(b"GRAY");
    broken[20..24].copy_from_slice(b"GRAY");
    let input = raw_pdf(b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f\n1 0 0 rg\n");
    let output = convert_links(
        &input,
        vec![
            DeviceLinkInput {
                id: Some("broken".to_owned()),
                bytes: broken,
            },
            DeviceLinkInput {
                id: Some("rgb".to_owned()),
                bytes: link_bytes(RGB_TO_CMYK_LINK),
            },
        ],
        BlackPreservationPolicy::None,
    );

    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 2);
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.links[0].operators_converted, 0);
    assert_eq!(page.links[1].operators_converted, 1);
}

#[test]
fn selected_page_append_is_prefix_preserving_and_reports_page_index() {
    let input = raw_pdf(b"/GrayAlias cs 0.5 sc 0 0 m 1 1 l f\n");
    let output = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::Indices(vec![PageIndex(0)]),
            device_links: links(&[GRAY_TO_GRAY_LINK]),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds");

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].page_index, PageIndex(0));
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
}
