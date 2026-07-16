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
use presslint_selectors::{Predicate, Selector};
use presslint_syntax::{TokenKind, assemble_operators, tokenize};
use presslint_types::PageIndex;

use crate::{
    BlackPreservationPolicy, ConvertContentColorsError, ConvertContentColorsOutput,
    ConvertContentColorsRequest, DeviceLinkInput, FormXObjectRefusalCounts, OperatorSkipCounts,
    PageSelection, convert_content_colors_incremental,
};

use super::{reopen, xref_record};

const FLATE_LIMIT: usize = 1 << 20;
const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";

// SYNTHETIC DeviceLinks (grid-2 CLUT placeholders). Source->destination spaces:
pub(super) const RGB_TO_CMYK_LINK: &str = "000001746c636d73043000006c696e6b52474220434d594b07ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000000e46d4142200000000003040000000000a40000000000000000000000500000002070617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000002020200000000000000000000000000020000000005002a004f0074009900be00e30108012d01520177019c01c101e6020b0230025500220047006c009100b600db01000125014a016f019401b901de0203022870617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000070617261000000000000000000010000";
pub(super) const CMYK_TO_CMYK_LINK: &str = "000001c46c636d73043000006c696e6b434d594b434d594b07ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000001346d4142200000000004040000000000f4000000000000000000000060000000207061726100000000000000000001000070617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000002020202000000000000000000000000020000000005002a004f0074009900be00e30108012d01520177019c01c101e6020b0230025500220047006c009100b600db01000125014a016f019401b901de02030228024d001a003f0064008900ae00d300f8011d01420167018c01b101d601fb0220024500120037005c008100a600cb00f00115013a015f018401a901ce01f3021870617261000000000000000000010000706172610000000000000000000100007061726100000000000000000001000070617261000000000000000000010000";
pub(super) const GRAY_TO_GRAY_LINK: &str = "000000e86c636d73043000006c696e6b475241594752415907ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d730000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000014132423000000090000000586d41422000000000010100000000004800000000000000000000003000000020706172610000000000000000000100000200000000000000000000000000000002000000000703ec70617261000000000000000000010000";
pub(super) const LAB_TO_RGB_LINK: &str = "000000846c636d73043000006c696e6b4c6162205247422007ea00070002000f00040004616373704150504c0000000000000000000000000000000000000000000000000000f6d6000100000000d32d6c636d73000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000";

pub(super) fn link_bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("hex"))
        .collect()
}

/// A one-element `device_links` vec (single-link caller) from a hex link.
pub(super) fn one_link(hex: &str) -> Vec<DeviceLinkInput> {
    vec![DeviceLinkInput {
        id: None,
        bytes: link_bytes(hex),
    }]
}

pub(super) fn stream_body(dict_extra: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("<< /Length {}{dict_extra} >>\nstream\n", data.len()).as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

pub(super) fn assemble_classic(bodies: &[Vec<u8>]) -> Vec<u8> {
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

pub(super) fn page_body(contents: &str) -> Vec<u8> {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} >>")
        .into_bytes()
}

pub(super) fn classic_raw_pdf(data: &[u8]) -> Vec<u8> {
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

/// Two-page classic PDF, each leaf owning its own single content stream.
pub(super) fn classic_two_page_pdf(page0: &[u8], page1: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
        stream_body("", page0),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 6 0 R >>".to_vec(),
        stream_body("", page1),
    ])
}

pub(super) fn predicate(predicate: Predicate) -> Selector {
    Selector::Predicate { predicate }
}

pub(super) fn xref_stream_pdf(data: &[u8]) -> Vec<u8> {
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
    page_encoded_stream_at(bytes, 0, 0)
}

/// Encoded content-stream data of the `ordinal`-th located stream of `page`.
pub(super) fn page_encoded_stream_at(bytes: &[u8], page: usize, ordinal: usize) -> Vec<u8> {
    let access = reopen(bytes);
    let lookup = test_lookup(&access.backend);
    let document = inspect_document_page_content_extents_with_lookup(
        bytes,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .expect("locate content extents");
    let page = &document.pages[page];
    let DocumentPageContentExtentResult::Inspected { extents, .. } = &page.result else {
        panic!("page content not inspected");
    };
    let PageContentExtentInspection::Located { extent, .. } = &extents.entries[ordinal] else {
        panic!("page content not located");
    };
    content_stream_data_slice(bytes, extent)
        .expect("slice")
        .to_vec()
}

pub(super) fn page_decoded_stream(bytes: &[u8], flate: bool) -> Vec<u8> {
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
pub(super) fn operands_of(decoded: &[u8], op: &[u8]) -> Vec<f64> {
    let mut matches: Vec<Vec<f64>> = operators(decoded)
        .into_iter()
        .filter(|(operator, _)| operator == op)
        .map(|(_, operands)| operands)
        .collect();
    assert_eq!(matches.len(), 1, "exactly one {op:?} operator expected");
    matches.remove(0)
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

pub(super) fn occurrence_count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn convert_with_policy(
    input: &[u8],
    link: &str,
    black_preservation: BlackPreservationPolicy,
) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(link),
            black_preservation,
            target: None,
        },
    )
    .expect("convert succeeds")
}

pub(super) fn convert(input: &[u8], link: &str) -> ConvertContentColorsOutput {
    convert_with_policy(input, link, BlackPreservationPolicy::None)
}

/// Convert with a `target` selector, `PageSelection::All`, no black preservation.
pub(super) fn convert_with_target(
    input: &[u8],
    link: &str,
    target: Selector,
) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(link),
            black_preservation: BlackPreservationPolicy::None,
            target: Some(target),
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
    assert_eq!(output.converted[0].black_preserved, 0);
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
    assert_eq!(output.converted[0].black_preserved, 0);
    assert_eq!(output.converted[0].operator_skips.no_matching_link, 2);

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
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 0);
    assert_eq!(output.converted[0].resource_alias_candidates_refused, 0);
    let decoded = page_decoded_stream(&output.bytes, false);
    let operands = operands_of(&decoded, b"g");
    assert_eq!(operands.len(), 1);
    assert_component_range(&operands);
}

#[test]
fn black_preservation_maps_rgb_black_to_k_before_device_link() {
    let input = classic_raw_pdf(b"q 0 0 0 rg 1 0 0 RG Q\n");
    let output = convert_with_policy(
        &input,
        RGB_TO_CMYK_LINK,
        BlackPreservationPolicy::NeutralBlackToK,
    );

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].black_preserved, 1);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 0);
    assert_eq!(output.converted[0].resource_alias_candidates_refused, 0);

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"0 0 0 1 k"));
    assert!(!contains(&decoded, b"0 0 0 rg"));
    assert!(!contains(&decoded, b"RG"));
    assert_eq!(operands_of(&decoded, b"K").len(), 4);
}

#[test]
fn policy_none_keeps_rgb_black_as_rich_device_link_conversion() {
    let input = classic_raw_pdf(b"0 0 0 rg\n");
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].black_preserved, 0);
    let decoded = page_decoded_stream(&output.bytes, false);
    let operands = operands_of(&decoded, b"k");
    assert_eq!(operands.len(), 4);
    assert_ne!(operands, vec![0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn black_preservation_preserves_cmyk_k_only_without_identical_splice() {
    let input = classic_raw_pdf(b"0 0 0 1 k\n");
    let output = convert_with_policy(
        &input,
        CMYK_TO_CMYK_LINK,
        BlackPreservationPolicy::NeutralBlackToK,
    );

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].black_preserved, 1);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 0);
    assert_eq!(output.converted[0].resource_alias_candidates_refused, 0);
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 1);
    assert_eq!(page_decoded_stream(&output.bytes, false), b"0 0 0 1 k\n");
}

#[test]
fn black_preservation_rewrites_noncanonical_cmyk_k_only() {
    let input = classic_raw_pdf(b"0.0 0.0 0.0 1.0 K\n");
    let output = convert_with_policy(
        &input,
        CMYK_TO_CMYK_LINK,
        BlackPreservationPolicy::NeutralBlackToK,
    );

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].black_preserved, 1);
    assert_eq!(page_decoded_stream(&output.bytes, false), b"0 0 0 1 K\n");
}

#[test]
fn non_cmyk_destination_does_not_preserve_black() {
    let input = classic_raw_pdf(b"0 g\n");
    let output = convert_with_policy(
        &input,
        GRAY_TO_GRAY_LINK,
        BlackPreservationPolicy::NeutralBlackToK,
    );

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].black_preserved, 0);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"k"));
    assert_eq!(operands_of(&decoded, b"g").len(), 1);
}

#[test]
fn source_space_gate_still_leaves_rgb_black_under_cmyk_link() {
    let input = classic_raw_pdf(b"0 0 0 rg\n0 0 0 1 k\n");
    let output = convert_with_policy(
        &input,
        CMYK_TO_CMYK_LINK,
        BlackPreservationPolicy::NeutralBlackToK,
    );

    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].black_preserved, 1);
    assert_eq!(output.converted[0].operator_skips.no_matching_link, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"0 0 0 rg"));
    assert!(contains(&decoded, b"0 0 0 1 k"));
}

#[test]
fn xref_stream_black_preservation_rewrites_rgb_black_and_reopens() {
    let input = xref_stream_pdf(b"0 0 0 rg\n");
    let output = convert_with_policy(
        &input,
        RGB_TO_CMYK_LINK,
        BlackPreservationPolicy::NeutralBlackToK,
    );

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].black_preserved, 1);
    assert!(matches!(
        reopen(&output.bytes).backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    assert_eq!(page_decoded_stream(&output.bytes, false), b"0 0 0 1 k\n");
}

// Malformed direct device operands (wrong count, non-number, non-finite) now
// refuse the WHOLE physical stream through the shared paint walk instead of
// skipping one operator; see `content_color_convert_malformed`.

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
            device_links: one_link(LAB_TO_RGB_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
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
            device_links: vec![DeviceLinkInput {
                id: None,
                bytes: b"not an icc profile".to_vec(),
            }],
            black_preservation: BlackPreservationPolicy::None,
            target: None,
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
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect_err("empty index list is rejected");
    assert!(matches!(error, ConvertContentColorsError::EmptyRequest));
}

#[test]
fn multi_content_stream_page_is_no_longer_skipped_and_converts_both_streams() {
    // Two content streams on one page: since T136 each content-stream object is
    // edited independently, so the page converts instead of being skipped whole as
    // `MultipleContentStreams`.
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("[4 0 R 5 0 R]"),
        stream_body("", b"1 0 0 rg\n"),
        stream_body("", b"0 0 1 RG\n"),
    ]);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.skipped.is_empty());
    assert_eq!(output.converted.len(), 1);
    let page = &output.converted[0];
    assert_eq!(page.operators_converted, 2);
    // Both content-stream objects are reported in stream-ordinal order.
    assert_eq!(
        page.content_objects
            .iter()
            .map(|reference| reference.object_number)
            .collect::<Vec<_>>(),
        vec![4, 5]
    );
    assert!(reopen(&output.bytes).page_leaves.leaves.len() == 1);
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
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("second run succeeds");

    assert_eq!(&second.bytes[..first.bytes.len()], first.bytes.as_slice());
    // The operator is now `k` (CMYK), a source-space mismatch under the RGB link.
    assert_eq!(second.converted[0].operators_converted, 0);
    assert_eq!(second.converted[0].operator_skips.no_matching_link, 1);
}

// --- Form refusal-class per-page counting (T192) -----------------------------

/// Two-page classic PDF whose page 0 declares two distinct refused Form
/// identities (object 5 twice-aliased plus repeated `Do`, object 6 once) and
/// whose page 1 re-`Do`s object 5 under a fresh name, exercising the analyzer
/// cache-hit path on a new page.
fn two_page_refusal_pdf() -> Vec<u8> {
    // Object layout: 1 catalog, 2 pages, 3 page-0, 4 content-0, 5 page-1,
    // 6 content-1, 7 raw-grammar form, 8 transparency-group form.
    let page_one_resources = "/XObject << /A 7 0 R /B 7 0 R /C 8 0 R >>";
    let page_two_resources = "/XObject << /D 7 0 R >>";
    let page = |contents: &str, resources: &str| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {contents} /Resources << {resources} >> >>"
        )
        .into_bytes()
    };
    let form_dict = " /Type /XObject /Subtype /Form /BBox [0 0 100 100]";
    // Object 5: an unsupported raw operator (RawGrammar).
    let raw_grammar_form = stream_body(form_dict, b"BT (x) Tj ET");
    // Object 6: a declared transparency group (TransparencyGroup).
    let group_form = stream_body(
        &format!("{form_dict} /Group << /S /Transparency >>"),
        b"0 0 m 1 1 l f",
    );
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 >>".to_vec(),
        page("4 0 R", page_one_resources),
        stream_body("", b"/A Do /B Do /A Do /C Do\n"),
        page("6 0 R", page_two_resources),
        stream_body("", b"/D Do\n"),
        raw_grammar_form,
        group_form,
    ])
}

#[test]
fn per_page_refusal_counts_dedup_aliases_and_recount_on_cache_hit() {
    let input = two_page_refusal_pdf();
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(output.converted.len(), 2);
    // Page 0: two aliases (`A`/`B`) and a repeated `Do` of the SAME identity
    // (object 5) count once; the distinct object-6 identity counts once too.
    let want_page_0 = FormXObjectRefusalCounts {
        raw_grammar: 1,
        transparency_group: 1,
        ..FormXObjectRefusalCounts::default()
    };
    assert_eq!(output.converted[0].form_xobject_refusal_counts, want_page_0);
    // Page 1: a cache hit on object 5 under a fresh name still counts once on
    // this page (zero decode/walk/budget charge), independent of page 0.
    let want_page_1 = FormXObjectRefusalCounts {
        raw_grammar: 1,
        ..FormXObjectRefusalCounts::default()
    };
    assert_eq!(output.converted[1].form_xobject_refusal_counts, want_page_1);

    // The per-page counts fold to the exact cross-page total of distinct
    // (page, identity) impacts: two `raw_grammar` impacts (one per page) and
    // one `transparency_group` impact.
    let mut total = output.converted[0].form_xobject_refusal_counts;
    total.fold(&output.converted[1].form_xobject_refusal_counts);
    assert_eq!(total.raw_grammar, 2);
    assert_eq!(total.transparency_group, 1);
}

/// One-page classic PDF whose page invokes two distinct RawGrammar-refused
/// Form identities (objects 5 and 6) through two aliases of the SAME
/// resource dictionary.
fn two_distinct_same_class_targets_pdf() -> Vec<u8> {
    let resources = "/XObject << /A 5 0 R /B 6 0 R >>";
    let form_dict = " /Type /XObject /Subtype /Form /BBox [0 0 100 100]";
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << {resources} >> >>"
        )
        .into_bytes(),
        stream_body("", b"/A Do /B Do\n"),
        stream_body(form_dict, b"BT (x) Tj ET"),
        stream_body(form_dict, b"zz"),
    ])
}

#[test]
fn two_distinct_same_class_targets_each_count_once() {
    let input = two_distinct_same_class_targets_pdf();
    let output = convert(&input, RGB_TO_CMYK_LINK);

    let want = FormXObjectRefusalCounts {
        raw_grammar: 2,
        ..FormXObjectRefusalCounts::default()
    };
    assert_eq!(output.converted[0].form_xobject_refusal_counts, want);
}

#[test]
fn a_refused_form_do_is_observe_only_and_never_changes_an_unrelated_operators_admission() {
    // A refused `Do` (RawGrammar) sits on the SAME page as an unrelated `rg`
    // device-colour operator: the refusal tally must never influence that
    // operator's own admission, converted bytes, or existing tallies.
    let form = stream_body(
        " /Type /XObject /Subtype /Form /BBox [0 0 100 100]",
        b"BT (x) Tj ET",
    );
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Resources << /XObject << /Fm 5 0 R >> >> >>".to_vec(),
        stream_body("", b"1 0 0 rg /Fm Do\n"),
        form.clone(),
    ]);
    let output = convert(&input, RGB_TO_CMYK_LINK);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        output.converted[0].operator_skips,
        OperatorSkipCounts::default()
    );
    assert!(contains(&output.bytes, &form));
    let want = FormXObjectRefusalCounts {
        raw_grammar: 1,
        ..FormXObjectRefusalCounts::default()
    };
    assert_eq!(output.converted[0].form_xobject_refusal_counts, want);
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
            device_links: one_link(CMYK_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("second run succeeds");
    assert_eq!(second.converted[0].operators_converted, 1);
}
