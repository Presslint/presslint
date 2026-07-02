#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::missing_panics_doc,
    clippy::too_many_lines
)]

mod append;
mod page_boxes;
mod planned;
mod reject;

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
