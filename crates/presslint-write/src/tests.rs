#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::too_many_lines
)]

mod alias_epoch_convert;
mod alias_epoch_plan;
mod alias_epoch_xobject;
mod append;
mod content_color_convert;
mod content_color_convert_malformed;
mod content_color_convert_multistream;
mod content_color_convert_paint_adapter;
mod content_color_convert_resource_space;
mod content_color_rewrite;
mod content_object_ownership;
mod extgstate_page_guard;
mod link_routing;
mod page_boxes;
mod page_boxes_xref_stream;
mod page_content_sequence;
mod page_device_space_policy;
mod planned;
mod reencode_content;
mod reject;
mod selector_match;
mod selector_match_paint_adapter;
mod xref_stream_writer;

use presslint_pdf::{
    DocumentAccess, DocumentAccessBackend, IndirectRef, inspect_document_access, inspect_pdf_source,
};

use crate::DirtyObjectBytes;

/// Original body of object 1 (document catalog) in [`sample_pdf`].
const CATALOG_BODY: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
/// Original body of object 2 (page-tree root) in [`sample_pdf`].
const PAGES_BODY: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
/// Original body of object 3 (single leaf page) in [`sample_pdf`].
const PAGE_BODY: &[u8] = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>";

/// Build a minimal, valid single-page classic-xref PDF with objects 1..=3 and a
/// trailer tail supplied by the caller (for example ` /ID [<a> <b>]`).
fn sample_pdf_with_trailer_tail(trailer_tail: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");

    let bodies: [&[u8]; 3] = [CATALOG_BODY, PAGES_BODY, PAGE_BODY];
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(buf.len());
        let number = index + 1;
        buf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }

    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R{trailer_tail} >>\nstartxref\n{xref_offset}\n")
            .as_bytes(),
    );
    buf.extend_from_slice(b"%%EOF");
    buf
}

/// A minimal, valid single-page classic-xref PDF with no extra trailer keys.
fn sample_pdf() -> Vec<u8> {
    sample_pdf_with_trailer_tail("")
}

/// Encode one `/W [ 1 2 1 ]` xref-stream record.
fn xref_record(entry_type: u8, field2: usize, generation: u8) -> [u8; 4] {
    let [hi, lo] = u16::try_from(field2)
        .expect("test field2 fits u16")
        .to_be_bytes();
    [entry_type, hi, lo, generation]
}

/// Minimal one-page PDF whose final section is a raw xref stream.
fn sample_xref_stream_pdf_with_catalog_tail(catalog_tail: &str) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");

    let catalog =
        format!("1 0 obj\n<< /Type /Catalog /Pages 2 0 R{catalog_tail} >>\nendobj\n").into_bytes();
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n";
    let info = b"4 0 obj\n<< /Producer (presslint-test) >>\nendobj\n";

    let catalog_offset = buf.len();
    buf.extend_from_slice(&catalog);
    let pages_offset = buf.len();
    buf.extend_from_slice(pages);
    let page_offset = buf.len();
    buf.extend_from_slice(page);
    let info_offset = buf.len();
    buf.extend_from_slice(info);
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(1, page_offset, 0));
    body.extend_from_slice(&xref_record(1, info_offset, 0));
    body.extend_from_slice(&xref_record(1, xref_offset, 0));

    buf.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /XRef /Size 6 /Index [0 6] /W [1 2 1] /Root 1 0 R /Info 4 0 R /ID [<0011> <2233>] /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}

fn sample_xref_stream_pdf() -> Vec<u8> {
    sample_xref_stream_pdf_with_catalog_tail("")
}

/// Build a two-revision classic PDF whose newest section rewrites only object 3
/// and declares a deliberately-too-small `/Size 4`, while an older `/Prev`
/// section defines the highest object number (5). This mirrors the
/// `PDFBOX-5945` input shape used to prove whole-chain `/Size` computation.
fn sample_pdf_with_prev_chain() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");

    let bodies: [&[u8]; 5] = [
        CATALOG_BODY,
        PAGES_BODY,
        PAGE_BODY,
        b"<< /Type /Unreferenced >>",
        b"<< /Note (highest object number) >>",
    ];
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(buf.len());
        let number = index + 1;
        buf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }

    let xref_a = buf.len();
    buf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_a}\n%%EOF\n").as_bytes(),
    );

    let obj3_offset = buf.len();
    buf.extend_from_slice(b"3 0 obj\n");
    buf.extend_from_slice(PAGE_BODY);
    buf.extend_from_slice(b"\nendobj\n");

    let xref_b = buf.len();
    buf.extend_from_slice(format!("xref\n3 1\n{obj3_offset:010} 00000 n \n").as_bytes());
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R /Prev {xref_a} >>\nstartxref\n{xref_b}\n%%EOF")
            .as_bytes(),
    );
    buf
}

/// Convenience constructor for a dirty-object rewrite request.
fn dirty(object_number: u32, generation: u16, body: &[u8]) -> DirtyObjectBytes {
    DirtyObjectBytes {
        reference: IndirectRef {
            object_number,
            generation,
        },
        body_bytes: body.to_vec(),
    }
}

/// Reopen `bytes` through the neutral document-access spine.
fn reopen(bytes: &[u8]) -> DocumentAccess {
    inspect_document_access(bytes).expect("output reopens through inspect_document_access")
}

/// Leaf `/Page` object numbers in document order.
fn page_leaf_numbers(access: &DocumentAccess) -> Vec<u32> {
    access
        .page_leaves
        .leaves
        .iter()
        .map(|leaf| leaf.reference.object_number)
        .collect()
}

/// Byte offset the final `startxref` in `bytes` points at.
fn final_startxref_target(bytes: &[u8]) -> usize {
    inspect_pdf_source(bytes)
        .expect("source inspects")
        .startxref
        .expect("startxref present")
        .byte_offset
}

/// The merged classic `/Prev` chain behind a reopened document, or panic when a
/// different backend was selected.
fn classic_chain(access: &DocumentAccess) -> &presslint_pdf::ClassicXrefChain {
    match &access.backend {
        DocumentAccessBackend::ClassicXrefChain { chain } => chain,
        other => panic!("expected a classic /Prev chain backend, got {other:?}"),
    }
}

/// The merged xref-stream `/Prev` chain behind a reopened document.
fn xref_stream_chain(access: &DocumentAccess) -> &presslint_pdf::XrefStreamChain {
    match &access.backend {
        DocumentAccessBackend::XrefStreamChain { chain } => chain,
        other => panic!("expected an xref-stream /Prev chain backend, got {other:?}"),
    }
}

/// Parse the integer following the last `/Size ` key in `bytes`.
fn last_trailer_size(bytes: &[u8]) -> usize {
    let needle = b"/Size ";
    let position = bytes
        .windows(needle.len())
        .rposition(|window| window == needle)
        .expect("a /Size key");
    let mut cursor = position + needle.len();
    let mut value = 0usize;
    while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
        value = value * 10 + usize::from(bytes[cursor] - b'0');
        cursor += 1;
    }
    value
}
