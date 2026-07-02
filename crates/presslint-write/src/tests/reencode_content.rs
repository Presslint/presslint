//! Behavior of the no-op content-stream re-encode `reencode_page_content_incremental`.
//!
//! These cover the semantic no-op over a `/FlateDecode` content stream (reopen ->
//! decode == original decoded), the true byte no-op over a raw content stream,
//! verbatim prefix preservation, reopen on classic AND xref-stream fixtures, the
//! `/Length` rewrite with every other dictionary key preserved, the structured
//! skip taxonomy, and idempotence.

use presslint_pdf::{
    DocumentAccess, DocumentAccessBackend, DocumentPageContentExtentResult, FlateDecodeParameters,
    ObjectLookup, PageContentExtentInspection, content_stream_data_slice, decode_flate_stream,
    encode_flate_stream, inspect_document_page_content_extents_with_lookup,
    inspect_indirect_object_dictionary,
};
use presslint_types::PageIndex;

use crate::{
    PageSelection, ReencodeFilterKind, ReencodePageContentError, ReencodePageContentRequest,
    ReencodePageSkipReason, reencode_page_content_incremental,
};

use super::{reopen, xref_record};

/// Valid content-stream bytes that tokenize cleanly (no strings).
const RAW_CONTENT: &[u8] = b"q 1 0 0 1 50 50 cm 0 0 1 rg 0 0 100 100 re f Q\n";
const FLATE_LIMIT: usize = 1 << 20;

/// Build a stream object body: `<< /Length N {extra} >>\nstream\n<data>\nendstream`.
fn stream_body(dict_extra: &str, data: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("<< /Length {}{dict_extra} >>\nstream\n", data.len()).as_bytes(),
    );
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

/// Assemble a classic single-`xref`-table PDF from object bodies (objects 1..=N).
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

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";

fn page_body(contents: &str) -> Vec<u8> {
    format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} >>")
        .into_bytes()
}

/// One-page classic PDF whose object 4 is a raw content stream carrying `data`.
fn classic_raw_pdf(data: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("4 0 R"),
        stream_body("", data),
    ])
}

/// One-page classic PDF whose object 4 is a single-`/FlateDecode` content stream.
fn classic_flate_pdf(dict_extra: &str) -> Vec<u8> {
    let compressed = encode_flate_stream(RAW_CONTENT, FLATE_LIMIT).expect("encode");
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("4 0 R"),
        stream_body(dict_extra, &compressed),
    ])
}

/// One-page classic PDF whose object 4 content stream borrows `/Length` from
/// object 5 (an indirect length).
fn classic_indirect_length_pdf() -> Vec<u8> {
    let data = RAW_CONTENT;
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

/// One-page classic PDF whose page declares two content streams (objects 4, 5).
fn classic_multi_stream_pdf() -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("[4 0 R 5 0 R]"),
        stream_body("", b"q Q\n"),
        stream_body("", b"q Q\n"),
    ])
}

/// Two-page classic PDF where both pages share content object 5.
fn classic_shared_content_pdf() -> Vec<u8> {
    let page = |num: u32| {
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 5 0 R >> % page {num}"
        )
        .into_bytes()
    };
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
        page(0),
        page(1),
        stream_body("", RAW_CONTENT),
    ])
}

/// One-page PDF whose final section is a raw xref stream and whose object 4 is a
/// raw content stream.
fn xref_stream_raw_pdf(data: &[u8]) -> Vec<u8> {
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

/// Re-encode a request over `input`, panicking on a whole-op error.
fn reencode(input: &[u8], pages: PageSelection) -> crate::ReencodePageContentOutput {
    reencode_page_content_incremental(input, &ReencodePageContentRequest { pages })
        .expect("re-encode succeeds")
}

/// Map a document-access backend to its borrowed object-lookup view.
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

/// Return a page's single located content-stream encoded data plus its resolved
/// object byte offset.
fn page_stream(bytes: &[u8], page_index: usize) -> (Vec<u8>, usize) {
    let access = reopen(bytes);
    let lookup = test_lookup(&access.backend);
    let document = inspect_document_page_content_extents_with_lookup(
        bytes,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .expect("locate content extents");
    let page = &document.pages[page_index];
    let DocumentPageContentExtentResult::Inspected { extents, .. } = &page.result else {
        panic!("page {page_index} content not inspected");
    };
    let PageContentExtentInspection::Located {
        extent,
        object_byte_offset,
        ..
    } = &extents.entries[0]
    else {
        panic!("page {page_index} content not located");
    };
    let data = content_stream_data_slice(bytes, extent)
        .expect("slice")
        .to_vec();
    (data, *object_byte_offset)
}

/// Dictionary bytes (`<< ... >>`) of a page's content-stream object.
fn content_dict_bytes(bytes: &[u8], page_index: usize) -> Vec<u8> {
    let (_, offset) = page_stream(bytes, page_index);
    let dictionary = inspect_indirect_object_dictionary(bytes, offset).expect("dict");
    bytes[dictionary.dictionary_open_byte_offset..dictionary.after_dictionary_close_byte_offset]
        .to_vec()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn leaf_numbers(access: &DocumentAccess) -> Vec<u32> {
    access
        .page_leaves
        .leaves
        .iter()
        .map(|leaf| leaf.reference.object_number)
        .collect()
}

#[test]
fn raw_content_stream_is_a_true_byte_no_op() {
    let input = classic_raw_pdf(RAW_CONTENT);
    let output = reencode(&input, PageSelection::All);

    // Verbatim prefix.
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    // One re-encoded page via the raw path, no skips.
    assert_eq!(output.reencoded.len(), 1);
    assert_eq!(output.reencoded[0].filter_kind, ReencodeFilterKind::Raw);
    assert_eq!(output.reencoded[0].content_object.object_number, 4);
    assert!(output.skipped.is_empty());

    // Reopened stream data is byte-identical to the original content.
    let (data, _) = page_stream(&output.bytes, 0);
    assert_eq!(data, RAW_CONTENT);

    // Output reopens and preserves the single leaf page.
    assert_eq!(leaf_numbers(&reopen(&output.bytes)), vec![3]);
}

#[test]
fn flate_content_stream_is_a_semantic_no_op() {
    let input = classic_flate_pdf(" /Filter /FlateDecode");
    let output = reencode(&input, PageSelection::All);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.reencoded.len(), 1);
    assert_eq!(output.reencoded[0].filter_kind, ReencodeFilterKind::Flate);
    assert!(output.skipped.is_empty());

    // decode(reopened stream) == decode(original stream) == the original content.
    let (data, _) = page_stream(&output.bytes, 0);
    let decoded =
        decode_flate_stream(&data, FlateDecodeParameters::default(), FLATE_LIMIT).expect("decode");
    assert_eq!(decoded, RAW_CONTENT);

    // The rewritten dictionary keeps /Filter and updates /Length to the new data
    // length; only the /Length value differs from a fixed template.
    let dict = content_dict_bytes(&output.bytes, 0);
    assert!(contains(&dict, b"/Filter /FlateDecode"));
    assert!(contains(
        &dict,
        format!("/Length {}", data.len()).as_bytes()
    ));
}

#[test]
fn length_is_updated_and_other_dict_keys_are_preserved() {
    // A Flate stream carrying an extra dictionary key that must survive verbatim.
    let input = classic_flate_pdf(" /Filter /FlateDecode /PressLintMark (keep)");
    let output = reencode(&input, PageSelection::All);
    assert_eq!(output.reencoded.len(), 1);

    let (data, _) = page_stream(&output.bytes, 0);
    let dict = content_dict_bytes(&output.bytes, 0);
    // Unrelated keys are preserved verbatim.
    assert!(contains(&dict, b"/Filter /FlateDecode"));
    assert!(contains(&dict, b"/PressLintMark (keep)"));
    // Exactly one /Length, equal to the new encoded data length.
    assert!(contains(
        &dict,
        format!("/Length {}", data.len()).as_bytes()
    ));
    assert_eq!(
        dict.windows(b"/Length".len())
            .filter(|w| *w == b"/Length")
            .count(),
        1
    );
}

#[test]
fn reopens_on_an_xref_stream_fixture() {
    let compressed = encode_flate_stream(RAW_CONTENT, FLATE_LIMIT).expect("encode");
    let input = xref_stream_raw_pdf(&compressed);
    // The xref-stream page's content stream is raw-wrapped Flate bytes; classify
    // it as raw by omitting /Filter so the raw byte no-op path is exercised on the
    // xref-stream backend.
    let output = reencode(&input, PageSelection::All);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.reencoded.len(), 1);
    assert_eq!(output.reencoded[0].filter_kind, ReencodeFilterKind::Raw);

    // Reopens through the xref-stream chain backend with the leaf intact.
    let access = reopen(&output.bytes);
    assert!(matches!(
        access.backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    assert_eq!(leaf_numbers(&access), vec![3]);

    // The raw stream data is byte-identical after reopening.
    let (data, _) = page_stream(&output.bytes, 0);
    assert_eq!(data, compressed);
}

#[test]
fn multiple_content_streams_are_skipped() {
    let input = classic_multi_stream_pdf();
    let output = reencode(&input, PageSelection::All);
    assert!(output.reencoded.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert_eq!(
        output.skipped[0].reason,
        ReencodePageSkipReason::MultipleContentStreams { count: 2 }
    );
}

#[test]
fn predictor_flate_is_skipped() {
    let input =
        classic_flate_pdf(" /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 >>");
    let output = reencode(&input, PageSelection::All);
    assert!(output.reencoded.is_empty());
    assert_eq!(
        output.skipped[0].reason,
        ReencodePageSkipReason::PredictorFlate { predictor: 12 }
    );
}

#[test]
fn indirect_length_is_skipped() {
    let input = classic_indirect_length_pdf();
    let output = reencode(&input, PageSelection::All);
    assert!(output.reencoded.is_empty());
    assert_eq!(
        output.skipped[0].reason,
        ReencodePageSkipReason::IndirectLength
    );
}

#[test]
fn shared_content_stream_is_skipped_as_unproven_ownership() {
    let input = classic_shared_content_pdf();
    let output = reencode(&input, PageSelection::Indices(vec![PageIndex(0)]));
    assert!(output.reencoded.is_empty());
    assert_eq!(output.skipped.len(), 1);
    assert!(matches!(
        output.skipped[0].reason,
        ReencodePageSkipReason::OwnershipNotInPlace { occurrences: 2, .. }
    ));
    assert_eq!(
        output.skipped[0].content_object.map(|r| r.object_number),
        Some(5)
    );
}

#[test]
fn non_round_tripping_content_is_skipped() {
    // A raw content stream whose bytes do not tokenize (unterminated string).
    let input = classic_raw_pdf(b"(unterminated");
    let output = reencode(&input, PageSelection::All);
    assert!(output.reencoded.is_empty());
    assert_eq!(
        output.skipped[0].reason,
        ReencodePageSkipReason::ContentRoundTripMismatch
    );
}

#[test]
fn a_second_run_is_idempotent() {
    let input = classic_flate_pdf(" /Filter /FlateDecode");
    let first = reencode(&input, PageSelection::All);
    let second = reencode(&first.bytes, PageSelection::All);

    // The second append keeps the first output as a verbatim prefix and re-encodes
    // the same content object again.
    assert_eq!(&second.bytes[..first.bytes.len()], first.bytes.as_slice());
    assert_eq!(second.reencoded.len(), 1);
    let (data, _) = page_stream(&second.bytes, 0);
    let decoded =
        decode_flate_stream(&data, FlateDecodeParameters::default(), FLATE_LIMIT).expect("decode");
    assert_eq!(decoded, RAW_CONTENT);
}

#[test]
fn empty_index_request_is_rejected() {
    let input = classic_raw_pdf(RAW_CONTENT);
    let error = reencode_page_content_incremental(
        &input,
        &ReencodePageContentRequest {
            pages: PageSelection::Indices(Vec::new()),
        },
    )
    .unwrap_err();
    assert_eq!(error, ReencodePageContentError::EmptyRequest);
}

#[test]
fn out_of_range_page_index_is_rejected() {
    let input = classic_raw_pdf(RAW_CONTENT);
    let error = reencode_page_content_incremental(
        &input,
        &ReencodePageContentRequest {
            pages: PageSelection::Indices(vec![PageIndex(7)]),
        },
    )
    .unwrap_err();
    assert_eq!(
        error,
        ReencodePageContentError::PageIndexOutOfRange {
            page_index: PageIndex(7),
            page_count: 1,
        }
    );
}

#[test]
fn unsupported_single_filter_is_skipped() {
    let compressed = encode_flate_stream(RAW_CONTENT, FLATE_LIMIT).expect("encode");
    let input = classic_flate_pdf_with_filter(" /Filter /LZWDecode", &compressed);
    let output = reencode(&input, PageSelection::All);
    assert!(output.reencoded.is_empty());
    assert_eq!(
        output.skipped[0].reason,
        ReencodePageSkipReason::UnsupportedFilter
    );
}

/// Classic Flate-shaped fixture with an explicit filter name and stream bytes.
fn classic_flate_pdf_with_filter(dict_extra: &str, data: &[u8]) -> Vec<u8> {
    assemble_classic(&[
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page_body("4 0 R"),
        stream_body(dict_extra, data),
    ])
}
