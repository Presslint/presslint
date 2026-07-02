//! Behavior of direct RGB-black page-content operator rewrite.

use presslint_pdf::{
    DocumentAccessBackend, DocumentPageContentExtentResult, FlateDecodeParameters, ObjectLookup,
    PageContentExtentInspection, content_stream_data_slice, decode_flate_stream,
    encode_flate_stream, inspect_document_page_content_extents_with_lookup,
};
use presslint_types::PageIndex;

use crate::{
    ContentColorRewriteOutput, ContentColorRewriteRequest, ContentColorRewriteSkipReason,
    PageSelection, rewrite_rgb_black_to_cmyk_incremental,
};

use super::{reopen, xref_record};

const FLATE_LIMIT: usize = 1 << 20;
const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";

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

fn classic_flate_pdf(data: &[u8], dict_extra: &str) -> Vec<u8> {
    let compressed = encode_flate_stream(data, FLATE_LIMIT).expect("encode");
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("4 0 R"),
        stream_body(dict_extra, &compressed),
    ])
}

fn classic_multi_stream_pdf() -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("[4 0 R 5 0 R]"),
        stream_body("", b"0 0 0 rg\n"),
        stream_body("", b"0 0 0 RG\n"),
    ])
}

fn classic_indirect_length_pdf(data: &[u8]) -> Vec<u8> {
    let mut object4 = Vec::new();
    object4.extend_from_slice(b"<< /Length 5 0 R >>\nstream\n");
    object4.extend_from_slice(data);
    object4.extend_from_slice(b"\nendstream");
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("4 0 R"),
        object4,
        format!("{}", data.len()).into_bytes(),
    ])
}

fn xref_stream_pdf(data: &[u8], dict_extra: &str) -> Vec<u8> {
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
    object4.extend_from_slice(&stream_body(dict_extra, data));
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

fn rewrite(input: &[u8]) -> ContentColorRewriteOutput {
    rewrite_rgb_black_to_cmyk_incremental(
        input,
        &ContentColorRewriteRequest {
            pages: PageSelection::All,
        },
    )
    .expect("rewrite succeeds")
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

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn raw_classic_rewrites_rgb_black_fill_and_stroke() {
    let input = classic_raw_pdf(b"q 0 0 0 rg 0 0 0 RG 10 20 m S Q\n");
    let output = rewrite(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.rewritten.len(), 1);
    assert_eq!(output.rewritten[0].operator_rewrites, 2);
    assert!(output.skipped.is_empty());

    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"0 0 0 1 k"));
    assert!(contains(&decoded, b"0 0 0 1 K"));
    assert!(!contains(&decoded, b"0 0 0 rg"));
    assert!(!contains(&decoded, b"0 0 0 RG"));
}

#[test]
fn multiple_matches_and_zero_variants_preserve_other_bytes() {
    let input_content = b"q\n1 0 0 rg\n0 0 0 g\n0 0 0 1 k\n0.0 0 -0 rg\n00.00 .0 +0 RG\nQ\n";
    let input = classic_raw_pdf(input_content);
    let output = rewrite(&input);
    let decoded = page_decoded_stream(&output.bytes, false);

    assert_eq!(output.rewritten[0].operator_rewrites, 2);
    assert!(contains(&decoded, b"1 0 0 rg"));
    assert!(contains(&decoded, b"0 0 0 g"));
    assert!(contains(&decoded, b"0 0 0 1 k"));
    assert!(contains(&decoded, b"0 0 0 1 K"));
    assert!(decoded.starts_with(b"q\n1 0 0 rg\n0 0 0 g\n0 0 0 1 k\n"));
    assert!(decoded.ends_with(b"\nQ\n"));
}

#[test]
fn page_with_no_matching_operators_is_a_structured_no_op_skip() {
    let input = classic_raw_pdf(b"q 1 0 0 rg 0 0 0 g 0 0 0 1 k Q\n");
    let output = rewrite(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(output.rewritten.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert_eq!(
        output.skipped[0].reason,
        ContentColorRewriteSkipReason::NoMatchingOperators
    );
    assert_eq!(
        page_decoded_stream(&output.bytes, false),
        b"q 1 0 0 rg 0 0 0 g 0 0 0 1 k Q\n"
    );
}

#[test]
fn flate_classic_rewrites_and_reopens_decoded_content() {
    let input = classic_flate_pdf(b"q 0 0 0 rg 0 0 0 RG Q\n", " /Filter /FlateDecode");
    let output = rewrite(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.rewritten[0].operator_rewrites, 2);
    let decoded = page_decoded_stream(&output.bytes, true);
    assert_eq!(decoded, b"q 0 0 0 1 k 0 0 0 1 K Q\n");
}

#[test]
fn xref_stream_raw_and_flate_fixtures_rewrite() {
    let raw = xref_stream_pdf(b"0 0 0 rg\n", "");
    let raw_output = rewrite(&raw);
    assert_eq!(&raw_output.bytes[..raw.len()], raw.as_slice());
    assert!(matches!(
        reopen(&raw_output.bytes).backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    assert_eq!(
        page_decoded_stream(&raw_output.bytes, false),
        b"0 0 0 1 k\n"
    );

    let compressed = encode_flate_stream(b"0 0 0 RG\n", FLATE_LIMIT).expect("encode");
    let flate = xref_stream_pdf(&compressed, " /Filter /FlateDecode");
    let flate_output = rewrite(&flate);
    assert_eq!(&flate_output.bytes[..flate.len()], flate.as_slice());
    assert_eq!(
        page_decoded_stream(&flate_output.bytes, true),
        b"0 0 0 1 K\n"
    );
}

#[test]
fn inherited_skip_shapes_are_structured_skips() {
    let multi = rewrite(&classic_multi_stream_pdf());
    assert_eq!(
        multi.skipped[0].reason,
        ContentColorRewriteSkipReason::MultipleContentStreams { count: 2 }
    );

    let predictor = rewrite(&classic_flate_pdf(
        b"0 0 0 rg\n",
        " /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 >>",
    ));
    assert_eq!(
        predictor.skipped[0].reason,
        ContentColorRewriteSkipReason::PredictorFlate { predictor: 12 }
    );

    let indirect = rewrite(&classic_indirect_length_pdf(b"0 0 0 rg\n"));
    assert_eq!(
        indirect.skipped[0].reason,
        ContentColorRewriteSkipReason::IndirectLength
    );
}

#[test]
fn second_run_is_idempotent_because_already_cmyk_has_no_match() {
    let input = classic_raw_pdf(b"0 0 0 rg\n");
    let first = rewrite(&input);
    let second = rewrite_rgb_black_to_cmyk_incremental(
        &first.bytes,
        &ContentColorRewriteRequest {
            pages: PageSelection::Indices(vec![PageIndex(0)]),
        },
    )
    .expect("second run succeeds");

    assert_eq!(&second.bytes[..first.bytes.len()], first.bytes.as_slice());
    assert!(second.rewritten.is_empty());
    assert_eq!(
        second.skipped[0].reason,
        ContentColorRewriteSkipReason::NoMatchingOperators
    );
}
