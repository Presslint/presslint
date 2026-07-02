//! Behaviour of DeviceLink-driven direct device-colour content conversion.
//!
//! Every DeviceLink is a tiny SYNTHETIC in-memory link built once (out of band)
//! through the same pinned `lcms2-sys` FFI `presslint-color-lcms` uses — a
//! grid-2 CLUT placeholder — then frozen here as hex bytes. `presslint-write` is
//! `#![forbid(unsafe_code)]`, so the FFI cannot run inside these unit tests; the
//! frozen bytes are a valid DeviceLink on which `inspect_device_link` /
//! `apply_device_link_f64` are deterministic. No ECI/FOGRA profile is vendored.
//! The converted CMYK/Gray values follow the link's baked LUT; the tests assert
//! the operator + operand-count change and that operands are finite and in
//! `[0.0, 1.0]`, not brittle exact component values.

// "DeviceLink" is the ICC domain term used as prose here, matching the module
// under test.
#![allow(clippy::doc_markdown)]

use presslint_pdf::{
    DocumentAccessBackend, DocumentPageContentExtentResult, FlateDecodeParameters, ObjectLookup,
    PageContentExtentInspection, content_stream_data_slice, decode_flate_stream,
    encode_flate_stream, inspect_document_page_content_extents_with_lookup,
};
use presslint_syntax::{TokenKind, assemble_operators, tokenize};
use presslint_types::PageIndex;

use crate::{
    ConvertContentColorsError, ConvertContentColorsOutput, ConvertContentColorsRequest,
    ConvertPageSkipReason, OperatorSkipCounts, PageSelection, convert_content_colors_incremental,
};

use super::{reopen, xref_record};

const FLATE_LIMIT: usize = 1 << 20;
const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";

// SYNTHETIC DeviceLinks (grid-2 CLUT placeholders). Source->destination spaces:
const RGB_TO_CMYK_LINK: &str = "000001746c636d73043000006c696e6b52474220434d594b07ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000000e46d4142200000000003040000000000a40000000000000000000000500000002070617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000002020200000000000000000000000000020000000005002a004f0074009900be00e30108012d01520177019c01c101e6020b0230025500220047006c009100b600db01000125014a016f019401b901de0203022870617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000070617261000000000000000000010000";
const CMYK_TO_CMYK_LINK: &str = "000001c46c636d73043000006c696e6b434d594b434d594b07ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000001346d4142200000000004040000000000f4000000000000000000000060000000207061726100000000000000000001000070617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000002020202000000000000000000000000020000000005002a004f0074009900be00e30108012d01520177019c01c101e6020b0230025500220047006c009100b600db01000125014a016f019401b901de02030228024d001a003f0064008900ae00d300f8011d01420167018c01b101d601fb0220024500120037005c008100a600cb00f00115013a015f018401a901ce01f3021870617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000070617261000000000000000000010000";
const GRAY_TO_GRAY_LINK: &str = "000000e86c636d73043000006c696e6b475241594752415907ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000000586d41422000000000010100000000004800000000000000000000003000000020706172610000000000000000000100000200000000000000000000000000000002000000000703ec70617261000000000000000000010000";
const LAB_TO_RGB_LINK: &str = "000000846c636d73043000006c696e6b4c6162205247422007ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d73000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

fn link_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

fn stream_body(dict_extra: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("<< /Length {}{dict_extra} >>\nstream\n", data.len()).as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

fn assemble_classic(bodies: &[Vec<u8>]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
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

fn page_body(contents: &str) -> Vec<u8> {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} >>")
        .into_bytes()
}

fn classic_raw_pdf(data: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("4 0 R"),
        stream_body("", data),
    ])
}

fn classic_flate_pdf(data: &[u8]) -> Vec<u8> {
    let compressed = encode_flate_stream(data, FLATE_LIMIT).expect("encode");
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("4 0 R"),
        stream_body(" /Filter /FlateDecode", &compressed),
    ])
}

fn xref_stream_pdf(data: &[u8]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");

    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
    let mut page = Vec::new();
    page.extend_from_slice(b"3 0 obj\n");
    page.extend_from_slice(&page_body("4 0 R"));
    page.extend_from_slice(b"\nendobj\n");
    let mut object4 = Vec::new();
    object4.extend_from_slice(b"4 0 obj\n");
    object4.extend_from_slice(&stream_body("", data));
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
    buf
}

fn test_lookup(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
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

fn page_encoded_stream(bytes: &[u8]) -> Vec<u8> {
    let access = reopen(bytes);
    let lookup = test_lookup(&access.backend);
    let document = inspect_document_page_content_extents_with_lookup(
        bytes,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .expect("locate content extents");
    let page = &document.pages[0];
    let DocumentPageContentExtentResult::Inspected { extents, .. } = &page.result else {
        panic!("page content not inspected");
    };
    let PageContentExtentInspection::Located { extent, .. } = &extents.entries[0] else {
        panic!("page content not located");
    };
    content_stream_data_slice(bytes, extent)
        .expect("slice")
        .to_vec()
}

fn page_decoded_stream(bytes: &[u8], flate: bool) -> Vec<u8> {
    let data = page_encoded_stream(bytes);
    if flate {
        decode_flate_stream(&data, FlateDecodeParameters::default(), FLATE_LIMIT).expect("decode")
    } else {
        data
    }
}

/// Assembled operator records as `(operator bytes, numeric operands)` pairs.
fn operators(decoded: &[u8]) -> Vec<(Vec<u8>, Vec<f64>)> {
    let tokens = tokenize(decoded).expect("tokenize");
    let assembled = assemble_operators(&tokens).expect("assemble");
    assembled
        .records
        .iter()
        .map(|record| {
            let op = tokens[record.operator.token_index]
                .source_bytes(decoded)
                .expect("operator bytes")
                .to_vec();
            let operands = record
                .operands
                .iter()
                .filter_map(|operand| {
                    let [token_ref] = operand.tokens.as_slice() else {
                        return None;
                    };
                    if !matches!(tokens[token_ref.token_index].kind, TokenKind::Number(_)) {
                        return None;
                    }
                    let bytes = tokens[token_ref.token_index].source_bytes(decoded)?;
                    std::str::from_utf8(bytes).ok()?.parse::<f64>().ok()
                })
                .collect();
            (op, operands)
        })
        .collect()
}

/// The numeric operands of the single record with operator `op`.
fn operands_of(decoded: &[u8], op: &[u8]) -> Vec<f64> {
    let mut matches: Vec<Vec<f64>> = operators(decoded)
        .into_iter()
        .filter(|(operator, _)| operator == op)
        .map(|(_, operands)| operands)
        .collect();
    assert_eq!(matches.len(), 1, "exactly one {op:?} operator expected");
    matches.remove(0)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn convert(input: &[u8], link: &str) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_link_bytes: link_bytes(link),
        },
    )
    .expect("convert succeeds")
}

fn assert_component_range(operands: &[f64]) {
    assert!(
        operands
            .iter()
            .all(|value| value.is_finite() && (0.0..=1.0).contains(value)),
        "converted operands must be finite and in [0,1]: {operands:?}"
    );
}

#[test]
fn rgb_link_converts_nonstroking_rg_to_k() {
    let input = classic_raw_pdf(b"q 1 0 0 rg 10 20 m S Q\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        output.converted[0].operator_skips,
        OperatorSkipCounts::default()
    );
    assert!(output.skipped.is_empty());

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    let operands = operands_of(&decoded, b"k");
    assert_eq!(operands.len(), 4);
    assert_component_range(&operands);
    // Unrelated operators are preserved verbatim.
    assert!(contains(&decoded, b"10 20 m"));
    assert!(decoded.starts_with(b"q "));
    assert!(decoded.ends_with(b" Q\n"));
}

#[test]
fn rgb_link_converts_stroking_rg_to_uppercase_k() {
    let input = classic_raw_pdf(b"1 0 0 RG\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"RG"));
    let operands = operands_of(&decoded, b"K");
    assert_eq!(operands.len(), 4);
    assert_component_range(&operands);
}

#[test]
fn source_space_gate_leaves_cmyk_and_gray_verbatim_under_rgb_link() {
    let input = classic_raw_pdf(b"0 0 0 1 k\n0.5 g\n1 0 0 rg\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted.len(), 1);
    // Only the RGB operator converts; the k and g operators are counted as
    // source-space mismatches and left byte-verbatim.
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].operator_skips.source_space_mismatch, 2);

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"0 0 0 1 k"));
    assert!(contains(&decoded, b"0.5 g"));
    assert!(!contains(&decoded, b"rg"));
}

#[test]
fn cmyk_link_rewrites_k_with_converted_operands() {
    let input = classic_raw_pdf(b"0 0 0 1 k\n");
    let output = convert(&input, CMYK_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    let operands = operands_of(&decoded, b"k");
    assert_eq!(operands.len(), 4);
    assert_component_range(&operands);
    // The link is not the identity, so the operands are rewritten.
    assert_ne!(operands, vec![0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn gray_link_converts_g_to_g() {
    let input = classic_raw_pdf(b"0.5 g\n");
    let output = convert(&input, GRAY_TO_GRAY_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    let operands = operands_of(&decoded, b"g");
    assert_eq!(operands.len(), 1);
    assert_component_range(&operands);
}

#[test]
fn wrong_operand_count_is_skipped_and_counted() {
    let input = classic_raw_pdf(b"0 0 rg\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].operator_skips.wrong_operand_count, 1);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"0 0 rg"
    ));
}

#[test]
fn non_number_operand_is_skipped_and_counted() {
    let input = classic_raw_pdf(b"1 0 /X rg\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].operator_skips.non_number_operand, 1);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"1 0 /X rg"
    ));
}

#[test]
fn out_of_range_operand_is_skipped_and_counted() {
    let input = classic_raw_pdf(b"1 0 2 rg\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].operator_skips.operand_out_of_range, 1);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"1 0 2 rg"
    ));
}

#[test]
fn multiple_matches_convert_descending_and_preserve_other_bytes() {
    let input = classic_raw_pdf(b"q\n1 0 0 rg\n0.2 0.4 0.6 rg\nfoo bar\n0 0 1 RG\nQ\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 3);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    assert!(!contains(&decoded, b"RG"));
    // Unrelated tokens between converted operators are untouched.
    assert!(contains(&decoded, b"foo bar"));
    assert!(decoded.starts_with(b"q\n"));
    assert!(decoded.ends_with(b"\nQ\n"));
}

#[test]
fn flate_classic_converts_and_reopens_decoded() {
    let input = classic_flate_pdf(b"q 1 0 0 rg Q\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].operators_converted, 1);
    let decoded = page_decoded_stream(&output.bytes, true);
    assert!(!contains(&decoded, b"rg"));
    let operands = operands_of(&decoded, b"k");
    assert_eq!(operands.len(), 4);
    assert_component_range(&operands);
}

#[test]
fn xref_stream_fixture_converts_and_reopens_as_xref_chain() {
    let input = xref_stream_pdf(b"1 0 0 rg\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(matches!(
        reopen(&output.bytes).backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    assert_eq!(operands_of(&decoded, b"k").len(), 4);
}

#[test]
fn page_with_no_matching_operators_produces_no_revision_object() {
    let input = classic_raw_pdf(b"q 10 20 m S Q\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted.len(), 1);
    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(
        output.converted[0].operator_skips,
        OperatorSkipCounts::default()
    );
    assert!(output.skipped.is_empty());
    // The content stream decodes unchanged (no dirty object appended for it).
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"q 10 20 m S Q\n"
    );
}

#[test]
fn lab_sided_link_is_a_whole_op_unsupported_space_error() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_link_bytes: link_bytes(LAB_TO_RGB_LINK),
        },
    )
    .expect_err("Lab-sided link is rejected");
    assert!(matches!(
        error,
        ConvertContentColorsError::UnsupportedLinkSpace { .. }
    ));
}

#[test]
fn invalid_link_bytes_are_a_whole_op_inspect_failure() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_link_bytes: b"not an icc profile".to_vec(),
        },
    )
    .expect_err("invalid link bytes are rejected");
    assert!(matches!(
        error,
        ConvertContentColorsError::DeviceLinkInspectFailed { .. }
    ));
}

#[test]
fn empty_page_index_request_is_rejected() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::Indices(vec![]),
            device_link_bytes: link_bytes(RGB_TO_CMYK_LINK),
        },
    )
    .expect_err("empty index list is rejected");
    assert!(matches!(error, ConvertContentColorsError::EmptyRequest));
}

#[test]
fn structural_skip_is_reported_separately_from_converted() {
    // Two content streams on one page is an inherited structural skip.
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("[4 0 R 5 0 R]"),
        stream_body("", b"1 0 0 rg\n"),
        stream_body("", b"0 0 1 RG\n"),
    ]);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert!(output.converted.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert_eq!(
        output.skipped[0].reason,
        ConvertPageSkipReason::MultipleContentStreams { count: 2 }
    );
}

#[test]
fn rerunning_rgb_link_does_not_reconvert_the_now_cmyk_operator() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let first = convert(&input, RGB_TO_CMYK_LINK);
    assert_eq!(first.converted[0].operators_converted, 1);

    let second = convert_content_colors_incremental(
        &first.bytes,
        &ConvertContentColorsRequest {
            pages: PageSelection::Indices(vec![PageIndex(0)]),
            device_link_bytes: link_bytes(RGB_TO_CMYK_LINK),
        },
    )
    .expect("second run succeeds");

    assert_eq!(&second.bytes[..first.bytes.len()], first.bytes.as_slice());
    // The operator is now `k` (CMYK), a source-space mismatch under the RGB link.
    assert_eq!(second.converted[0].operators_converted, 0);
    assert_eq!(second.converted[0].operator_skips.source_space_mismatch, 1);
}

#[test]
fn cmyk_link_reconverts_an_already_cmyk_operator() {
    let input = classic_raw_pdf(b"0 0 0 1 k\n");
    let first = convert(&input, CMYK_TO_CMYK_LINK);
    assert_eq!(first.converted[0].operators_converted, 1);

    // A CMYK-source link re-touches the `k` operator on a second pass.
    let second = convert_content_colors_incremental(
        &first.bytes,
        &ConvertContentColorsRequest {
            pages: PageSelection::Indices(vec![PageIndex(0)]),
            device_link_bytes: link_bytes(CMYK_TO_CMYK_LINK),
        },
    )
    .expect("second run succeeds");
    assert_eq!(second.converted[0].operators_converted, 1);
}
