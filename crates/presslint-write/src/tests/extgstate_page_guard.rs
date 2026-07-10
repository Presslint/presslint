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
