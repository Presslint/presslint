//! Multi-link routing behaviour for DeviceLink content-colour conversion (F4-5).
//!
//! These tests reuse the SYNTHETIC grid-2 CLUT DeviceLinks and PDF builders from
//! the sibling `content_color_convert` test module (see its header for why the
//! links are frozen hex). They exercise the parts F4-5 adds on top of F4-2..F4-4:
//! source-space routing across a SET of links, the `no_matching_link` coverage
//! gap, the per-link report, and the up-front routing validation errors.

// "DeviceLink" is the ICC domain term used as prose here.
#![allow(clippy::doc_markdown)]

use presslint_color_lcms::{DeviceLinkSpace, LcmsError};
use presslint_syntax::{assemble_operators, tokenize};

use crate::{
    ConvertContentColorsError, ConvertContentColorsOutput, ConvertContentColorsRequest,
    DeviceLinkInput, PageSelection, convert_content_colors_incremental,
};

use super::content_color_convert::{
    CMYK_TO_CMYK_LINK, GRAY_TO_GRAY_LINK, LAB_TO_RGB_LINK, RGB_TO_CMYK_LINK, classic_raw_pdf,
    contains, link_bytes, page_decoded_stream,
};

/// A labelled link input from a hex link.
fn link(id: &str, hex: &str) -> DeviceLinkInput {
    DeviceLinkInput {
        id: Some(id.to_string()),
        bytes: link_bytes(hex),
    }
}

/// Convert `input` through an explicit `device_links` vec, no black preservation.
fn convert_links(input: &[u8], device_links: Vec<DeviceLinkInput>) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links,
            black_preservation: crate::BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("convert succeeds")
}

/// Attempt a conversion and return the whole-op error.
fn convert_links_err(
    input: &[u8],
    device_links: Vec<DeviceLinkInput>,
) -> ConvertContentColorsError {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links,
            black_preservation: crate::BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect_err("routing rejects the request")
}

/// Count assembled operator records whose operator bytes equal `op`.
fn op_count(decoded: &[u8], op: &[u8]) -> usize {
    let tokens = tokenize(decoded).expect("tokenize");
    let assembled = assemble_operators(&tokens).expect("assemble");
    assembled
        .records
        .iter()
        .filter(|record| tokens[record.operator.token_index].source_bytes(decoded) == Some(op))
        .count()
}

#[test]
fn two_links_route_rg_via_first_and_k_via_second_in_one_call() {
    let input = classic_raw_pdf(b"1 0 0 rg\n0 0 0 1 k\n");
    let output = convert_links(
        &input,
        vec![
            link("rgb", RGB_TO_CMYK_LINK),
            link("cmyk", CMYK_TO_CMYK_LINK),
        ],
    );

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    let page = &output.converted[0];
    // Both operators convert: the `rg` via the RGB link, the `k` via the CMYK
    // link. Both destinations are CMYK, so the stream ends up with two `k` ops.
    assert_eq!(page.operators_converted, 2);
    assert_eq!(page.black_preserved, 0);

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    assert_eq!(op_count(&decoded, b"k"), 2);

    // Per-link report attributes exactly one conversion to each link.
    assert_eq!(page.links.len(), 2);
    assert_eq!(page.links[0].link_index, 0);
    assert_eq!(page.links[0].link_id.as_deref(), Some("rgb"));
    assert_eq!(page.links[0].source, DeviceLinkSpace::Rgb);
    assert_eq!(page.links[0].destination, DeviceLinkSpace::Cmyk);
    assert_eq!(page.links[0].operators_converted, 1);
    assert_eq!(page.links[1].link_index, 1);
    assert_eq!(page.links[1].link_id.as_deref(), Some("cmyk"));
    assert_eq!(page.links[1].source, DeviceLinkSpace::Cmyk);
    assert_eq!(page.links[1].destination, DeviceLinkSpace::Cmyk);
    assert_eq!(page.links[1].operators_converted, 1);
}

#[test]
fn operator_with_no_matching_link_is_left_verbatim_and_counted() {
    // Only an RGB->CMYK link is supplied; the well-formed `g` (Gray) operator
    // matches no link's source and is an honest coverage gap.
    let input = classic_raw_pdf(b"0.5 g\n1 0 0 rg\n");
    let output = convert_links(&input, vec![link("rgb", RGB_TO_CMYK_LINK)]);

    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.operator_skips.no_matching_link, 1);
    assert_eq!(page.links.len(), 1);
    assert_eq!(page.links[0].operators_converted, 1);

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"0.5 g"));
    assert!(!contains(&decoded, b"rg"));
}

#[test]
fn duplicate_source_space_is_ambiguous_link_source_up_front() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_links_err(
        &input,
        vec![
            link("first", RGB_TO_CMYK_LINK),
            link("second", RGB_TO_CMYK_LINK),
        ],
    );

    assert_eq!(
        error,
        ConvertContentColorsError::AmbiguousLinkSource {
            space: DeviceLinkSpace::Rgb,
            first_index: 0,
            second_index: 1,
        }
    );
}

#[test]
fn empty_device_links_is_no_device_links() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_links_err(&input, vec![]);
    assert_eq!(error, ConvertContentColorsError::NoDeviceLinks);
}

#[test]
fn bad_link_is_inspect_failed_with_its_index_and_id() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_links_err(
        &input,
        vec![
            link("good", RGB_TO_CMYK_LINK),
            DeviceLinkInput {
                id: Some("broken".to_string()),
                bytes: b"not an icc profile".to_vec(),
            },
        ],
    );

    assert!(matches!(
        error,
        ConvertContentColorsError::DeviceLinkInspectFailed {
            index: 1,
            id: Some(ref id),
            error: LcmsError::InvalidProfile,
        } if id == "broken"
    ));
}

#[test]
fn unsupported_link_space_reports_raw_spaces_index_and_id() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_links_err(&input, vec![link("lab", LAB_TO_RGB_LINK)]);

    assert_eq!(
        error,
        ConvertContentColorsError::UnsupportedLinkSpace {
            index: 0,
            id: Some("lab".to_string()),
            source: DeviceLinkSpace::Lab,
            destination: DeviceLinkSpace::Rgb,
        }
    );
}

#[test]
fn no_matching_link_reported_for_every_supplied_link_even_at_zero() {
    // A Gray->Gray link supplied alongside content that has no gray operator:
    // the link still appears in the per-link report with zero conversions.
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let output = convert_links(
        &input,
        vec![
            link("rgb", RGB_TO_CMYK_LINK),
            link("gray", GRAY_TO_GRAY_LINK),
        ],
    );

    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.links.len(), 2);
    assert_eq!(page.links[0].link_id.as_deref(), Some("rgb"));
    assert_eq!(page.links[0].operators_converted, 1);
    assert_eq!(page.links[1].link_id.as_deref(), Some("gray"));
    assert_eq!(page.links[1].source, DeviceLinkSpace::Gray);
    assert_eq!(page.links[1].operators_converted, 0);
}

#[test]
fn single_link_vec_reproduces_f4_behaviour_with_one_link_report() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let output = convert_links(&input, vec![link("only", RGB_TO_CMYK_LINK)]);

    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 1);
    assert_eq!(page.links.len(), 1);
    assert_eq!(page.links[0].link_index, 0);
    assert_eq!(page.links[0].link_id.as_deref(), Some("only"));
    assert_eq!(page.links[0].operators_converted, 1);

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    assert_eq!(op_count(&decoded, b"k"), 1);
}

#[test]
fn black_preservation_fires_per_routed_cmyk_destination_link() {
    // Two links; the RGB->CMYK link routes the neutral RGB black, and because its
    // destination is CMYK the black-preservation overlay maps it to K-only.
    let input = classic_raw_pdf(b"0 0 0 rg\n");
    let output = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: vec![
                link("rgb", RGB_TO_CMYK_LINK),
                link("cmyk", CMYK_TO_CMYK_LINK),
            ],
            black_preservation: crate::BlackPreservationPolicy::NeutralBlackToK,
            target: None,
        },
    )
    .expect("convert succeeds");

    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 0);
    assert_eq!(page.black_preserved, 1);
    assert_eq!(page.links[0].operators_converted, 0);
    assert_eq!(page_decoded_stream(&output.bytes, false), b"0 0 0 1 k\n");
}
