use presslint_pdf::IndirectRef;

use crate::{WriteError, write_incremental_revision};

use super::{PAGE_BODY, dirty, sample_pdf, sample_pdf_with_trailer_tail};

/// A minimal document whose final `startxref` points at a cross-reference stream
/// object header rather than a classic `xref` table.
fn xref_stream_pdf() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let object_offset = buf.len();
    buf.extend_from_slice(
        b"1 0 obj\n<< /Type /XRef /Size 1 /W [1 1 1] /Root 2 0 R /Encrypt 4 0 R /Length 0 >>\nstream\n\nendstream\nendobj\n",
    );
    buf.extend_from_slice(format!("startxref\n{object_offset}\n%%EOF").as_bytes());
    buf
}

#[test]
fn rejects_encrypted_xref_stream_input() {
    let input = xref_stream_pdf();
    let error = write_incremental_revision(&input, &[]).unwrap_err();
    assert_eq!(error, WriteError::EncryptedInput);
}

#[test]
fn rejects_encrypted_input() {
    let input = sample_pdf_with_trailer_tail(" /Encrypt 4 0 R");
    let error = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).unwrap_err();
    assert_eq!(error, WriteError::EncryptedInput);
}

#[test]
fn rejects_hybrid_xref_stm_input() {
    let input = sample_pdf_with_trailer_tail(" /XRefStm 123");
    let error = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).unwrap_err();
    assert_eq!(error, WriteError::HybridXrefStmInput);
}

fn xref_stream_with_compressed_page_entry() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let catalog_offset = buf.len();
    buf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    let pages_offset = buf.len();
    buf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&super::xref_record(0, 0, 0));
    body.extend_from_slice(&super::xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&super::xref_record(1, pages_offset, 0));
    body.extend_from_slice(&super::xref_record(2, 8, 0));
    body.extend_from_slice(&super::xref_record(1, xref_offset, 0));

    buf.extend_from_slice(
        format!(
            "4 0 obj\n<< /Type /XRef /Size 5 /Index [0 5] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}

#[test]
fn rejects_compressed_dirty_object_on_xref_stream_input() {
    let input = xref_stream_with_compressed_page_entry();
    let error = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).unwrap_err();
    assert_eq!(
        error,
        WriteError::CompressedDirtyObject {
            reference: IndirectRef {
                object_number: 3,
                generation: 0,
            },
            object_stream_number: 8,
            index_within_object_stream: 0,
        }
    );
}

#[test]
fn rejects_duplicate_dirty_object_numbers() {
    let input = sample_pdf();
    let error =
        write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY), dirty(3, 0, PAGE_BODY)])
            .unwrap_err();
    assert_eq!(error, WriteError::DuplicateDirtyObject { object_number: 3 });
}

#[test]
fn rejects_generation_mismatch() {
    let input = sample_pdf();
    let error = write_incremental_revision(&input, &[dirty(3, 7, PAGE_BODY)]).unwrap_err();
    assert_eq!(
        error,
        WriteError::GenerationMismatch {
            object_number: 3,
            expected: 0,
            found: 7,
        }
    );
}

/// A classic PDF whose object-3 in-use xref entry deliberately points at the
/// object-2 header offset. The entry resolves as `InUse` with a matching
/// generation, but the indirect object header at that offset reads `2 0 obj`,
/// so header validation must reject it.
fn sample_pdf_with_mispointed_object_3() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");

    let bodies: [&[u8]; 3] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        PAGE_BODY,
    ];
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(buf.len());
        let number = index + 1;
        buf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    // Corrupt only object 3's recorded offset so it points at object 2's header.
    offsets[2] = offsets[1];

    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF").as_bytes(),
    );
    buf
}

#[test]
fn rejects_dirty_object_with_mismatched_header_offset() {
    let input = sample_pdf_with_mispointed_object_3();
    let error = write_incremental_revision(&input, &[dirty(3, 0, PAGE_BODY)]).unwrap_err();
    match error {
        WriteError::DirtyObjectHeaderMismatch { reference, .. } => {
            assert_eq!(
                reference,
                IndirectRef {
                    object_number: 3,
                    generation: 0,
                }
            );
        }
        other => panic!("expected DirtyObjectHeaderMismatch, got {other:?}"),
    }
}

#[test]
fn rejects_dirty_object_not_in_use() {
    let input = sample_pdf();
    let error = write_incremental_revision(&input, &[dirty(9, 0, PAGE_BODY)]).unwrap_err();
    assert_eq!(
        error,
        WriteError::DirtyObjectNotInUse {
            reference: IndirectRef {
                object_number: 9,
                generation: 0,
            },
        }
    );
}

#[test]
fn rejects_non_pdf_input() {
    let error = write_incremental_revision(b"definitely not a pdf", &[]).unwrap_err();
    assert!(matches!(error, WriteError::Source { .. }));
}
