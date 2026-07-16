use std::fmt::Write as _;

use presslint_pdf::{
    ClassicXrefObjectLocation, IndirectRef, XrefStreamEntryRecord, inspect_indirect_object_header,
    resolve_classic_xref_chain_object,
};

use crate::{
    FreshObjectBytes, WriteError, reserve_fresh_object_references, write_incremental_revision,
    write_incremental_revision_with_fresh_objects,
};

use super::{
    CATALOG_BODY, PAGE_BODY, PAGES_BODY, classic_chain, dirty, last_trailer_size,
    page_leaf_numbers, reopen, sample_pdf, sample_pdf_with_trailer_tail, sample_xref_stream_pdf,
    xref_record, xref_stream_chain,
};

/// Resolve the real header at `byte_offset`, then return the exact writer-
/// placed body bytes between it and `endobj` -- proves round-trip byte
/// identity, not just header presence.
fn resolved_body_bytes(output: &[u8], byte_offset: usize) -> &[u8] {
    let header = inspect_indirect_object_header(output, byte_offset)
        .expect("appended object header resolves");
    let body_start = header.after_obj_keyword_offset + 1;
    let endobj = output[body_start..]
        .windows(6)
        .position(|window| window == b"endobj")
        .expect("endobj marker present after the appended body");
    let mut body_end = body_start + endobj;
    while matches!(output.get(body_end - 1), Some(b'\n' | b'\r')) {
        body_end -= 1;
    }
    &output[body_start..body_end]
}

/// Convenience constructor for a fresh-object append request.
pub(super) fn fresh(object_number: u32, body: &[u8]) -> FreshObjectBytes {
    FreshObjectBytes {
        reference: IndirectRef {
            object_number,
            generation: 0,
        },
        body_bytes: body.to_vec(),
    }
}

/// Start a classic-xref PDF buffer with the standard objects 1=catalog,
/// 2=pages, 3=page, returning the buffer and their byte offsets in order.
pub(super) fn classic_pdf_with_standard_objects() -> (Vec<u8>, Vec<usize>) {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");
    let offsets = [(1u32, CATALOG_BODY), (2, PAGES_BODY), (3, PAGE_BODY)]
        .iter()
        .map(|(number, body)| push_object(&mut buf, *number, body))
        .collect();
    (buf, offsets)
}

/// A single-subsection classic-xref PDF: every `(object_number, body)` pair
/// becomes an in-use object, followed by an `xref 0 size` table and a
/// trailer declaring `/Size size` (no extra free/defined entries).
pub(super) fn classic_pdf_with_objects_and_size(objects: &[(u32, &[u8])], size: u32) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");
    let offsets: Vec<usize> = objects
        .iter()
        .map(|(number, body)| push_object(&mut buf, *number, body))
        .collect();
    let xref_offset = buf.len();
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

/// Objects 1..=3 (standard) plus one caller-supplied fourth object body.
/// Shared by every allocation-floor fixture that only needs to vary object
/// 4's body.
pub(super) fn classic_pdf_with_standard_objects_plus_fourth(fourth_body: &[u8]) -> Vec<u8> {
    classic_pdf_with_objects_and_size(
        &[
            (1, CATALOG_BODY),
            (2, PAGES_BODY),
            (3, PAGE_BODY),
            (4, fourth_body),
        ],
        5,
    )
}

/// Append one `N 0 obj\n{body}\nendobj\n` object to `buf`, returning its
/// starting byte offset. Shared by every xref-stream fixture builder below.
fn push_object(buf: &mut Vec<u8>, number: u32, body: &[u8]) -> usize {
    let offset = buf.len();
    buf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
    buf.extend_from_slice(body);
    buf.extend_from_slice(b"\nendobj\n");
    offset
}

/// Append the standard one-page skeleton (1=catalog, 2=pages, 3=page) to an
/// xref-stream fixture buffer, returning their byte offsets in order.
pub(super) fn push_one_page_objects(buf: &mut Vec<u8>) -> (usize, usize, usize) {
    let catalog_offset = push_object(buf, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    let pages_offset = push_object(buf, 2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let page_offset = push_object(
        buf,
        3,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>",
    );
    (catalog_offset, pages_offset, page_offset)
}

/// Build the raw decoded `/ObjStm` body for `(object_number, body)` members,
/// returning the `/First` header length and the concatenated body bytes.
pub(super) fn object_stream_body(members: &[(u32, &[u8])]) -> (usize, Vec<u8>) {
    let mut header = String::new();
    let mut objects = Vec::new();
    let mut offset = 0usize;
    for (number, body) in members {
        let _ = write!(header, "{number} {offset} ");
        objects.extend_from_slice(body);
        offset += body.len();
    }
    let first = header.len();
    let mut decoded = header.into_bytes();
    decoded.extend_from_slice(&objects);
    (first, decoded)
}

/// A minimal xref-stream document whose page entry is compressed. The
/// container need not resolve because the legacy dirty-object admission path
/// rejects the type-2 entry before attempting body resolution.
fn xref_stream_pdf_with_compressed_dirty_target() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let catalog_offset = push_object(&mut buf, 1, b"<< /Type /Catalog /Pages 2 0 R >>");
    let pages_offset = push_object(&mut buf, 2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(2, 8, 0));
    body.extend_from_slice(&xref_record(1, xref_offset, 0));

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

// ---------------------------------------------------------------------------
// Legacy compatibility
// ---------------------------------------------------------------------------

#[test]
fn classic_empty_fresh_matches_legacy_write() {
    let input = sample_pdf();
    let dirty_objects = [dirty(3, 0, PAGE_BODY)];
    assert_eq!(
        write_incremental_revision_with_fresh_objects(&input, &dirty_objects, &[]),
        write_incremental_revision(&input, &dirty_objects)
    );
}

#[test]
fn xref_stream_empty_fresh_matches_legacy_write() {
    let input = sample_xref_stream_pdf();
    let dirty_objects = [dirty(3, 0, PAGE_BODY)];
    assert_eq!(
        write_incremental_revision_with_fresh_objects(&input, &dirty_objects, &[]),
        write_incremental_revision(&input, &dirty_objects)
    );
}

#[test]
fn empty_fresh_matches_legacy_with_zero_and_reversed_dirty_objects() {
    let input = sample_pdf();
    assert_eq!(
        write_incremental_revision_with_fresh_objects(&input, &[], &[]),
        write_incremental_revision(&input, &[])
    );

    let forward = [dirty(2, 0, PAGES_BODY), dirty(3, 0, PAGE_BODY)];
    let reversed = [dirty(3, 0, PAGE_BODY), dirty(2, 0, PAGES_BODY)];
    assert_eq!(
        write_incremental_revision_with_fresh_objects(&input, &forward, &[]).unwrap(),
        write_incremental_revision_with_fresh_objects(&input, &reversed, &[]).unwrap(),
    );
}

#[test]
fn empty_fresh_write_matches_legacy_rejection_matrix() {
    let cases: Vec<(Vec<u8>, Vec<crate::DirtyObjectBytes>)> = vec![
        (
            sample_pdf_with_trailer_tail(" /Encrypt 4 0 R"),
            vec![dirty(3, 0, PAGE_BODY)],
        ),
        (
            sample_pdf_with_trailer_tail(" /XRefStm 123"),
            vec![dirty(3, 0, PAGE_BODY)],
        ),
        (
            sample_pdf(),
            vec![dirty(3, 0, PAGE_BODY), dirty(3, 0, PAGE_BODY)],
        ),
        (sample_pdf(), vec![dirty(3, 7, PAGE_BODY)]),
        (sample_pdf(), vec![dirty(9, 0, PAGE_BODY)]),
        (b"definitely not a pdf".to_vec(), vec![]),
    ];
    for (input, dirty_objects) in cases {
        assert_eq!(
            write_incremental_revision_with_fresh_objects(&input, &dirty_objects, &[]),
            write_incremental_revision(&input, &dirty_objects),
        );
    }
}

#[test]
fn empty_fresh_matches_legacy_on_a_structurally_malformed_pdf() {
    let input = b"%PDF-1.4\nxref\n0 nope\nstartxref\n9\n%%EOF";
    let legacy = write_incremental_revision(input, &[]);
    assert!(matches!(legacy, Err(WriteError::XrefTable { .. })));
    assert_eq!(
        write_incremental_revision_with_fresh_objects(input, &[], &[]),
        legacy
    );
}

#[test]
fn empty_fresh_matches_legacy_for_a_compressed_dirty_target() {
    let input = xref_stream_pdf_with_compressed_dirty_target();
    let dirty_objects = [dirty(3, 0, PAGE_BODY)];
    let legacy = write_incremental_revision(&input, &dirty_objects);
    assert!(matches!(
        legacy,
        Err(WriteError::CompressedDirtyObject {
            reference: IndirectRef {
                object_number: 3,
                generation: 0,
            },
            object_stream_number: 8,
            index_within_object_stream: 0,
        })
    ));
    assert_eq!(
        write_incremental_revision_with_fresh_objects(&input, &dirty_objects, &[]),
        legacy
    );
}

#[test]
fn reserve_zero_fresh_objects_returns_empty_without_a_scan() {
    assert_eq!(
        reserve_fresh_object_references(b"definitely not a pdf", 0),
        Ok(Vec::new())
    );
    assert_eq!(
        reserve_fresh_object_references(&sample_pdf(), 0),
        Ok(Vec::new())
    );
}

#[test]
fn xref_stream_empty_fresh_matches_legacy_with_zero_and_reversed_dirty_objects() {
    let input = sample_xref_stream_pdf();
    assert_eq!(
        write_incremental_revision_with_fresh_objects(&input, &[], &[]),
        write_incremental_revision(&input, &[])
    );

    let forward = [dirty(2, 0, PAGES_BODY), dirty(3, 0, PAGE_BODY)];
    let reversed = [dirty(3, 0, PAGE_BODY), dirty(2, 0, PAGES_BODY)];
    assert_eq!(
        write_incremental_revision_with_fresh_objects(&input, &forward, &[]).unwrap(),
        write_incremental_revision_with_fresh_objects(&input, &reversed, &[]).unwrap(),
    );
}

/// A one-page xref-stream document (catalog=1, pages=2, page=3, xref
/// self-object=4). `include_self_entry` toggles a defective `/Index [0 4]`
/// entry map that omits the self-entry; `declared_size`/`dictionary_tail`
/// vary `/Size` and append extra dictionary keys.
pub(super) fn xref_stream_pdf_with_one_page(
    include_self_entry: bool,
    declared_size: u32,
    dictionary_tail: &str,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let (catalog_offset, pages_offset, page_offset) = push_one_page_objects(&mut buf);
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(1, page_offset, 0));
    let index_count = if include_self_entry {
        body.extend_from_slice(&xref_record(1, xref_offset, 0));
        5
    } else {
        4
    };

    buf.extend_from_slice(
        format!(
            "4 0 obj\n<< /Type /XRef /Size {declared_size} /Index [0 {index_count}] /W [1 2 1] /Root 1 0 R{dictionary_tail} /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}

pub(super) fn sample_xref_stream_pdf_with_dictionary_tail(tail: &str) -> Vec<u8> {
    xref_stream_pdf_with_one_page(true, 5, tail)
}

#[test]
fn reserve_nonempty_rejects_encrypted_and_hybrid_inputs() {
    assert_eq!(
        reserve_fresh_object_references(&sample_pdf_with_trailer_tail(" /Encrypt 4 0 R"), 1),
        Err(WriteError::EncryptedInput)
    );
    assert_eq!(
        reserve_fresh_object_references(&sample_pdf_with_trailer_tail(" /XRefStm 123"), 1),
        Err(WriteError::HybridXrefStmInput)
    );
    assert_eq!(
        reserve_fresh_object_references(
            &sample_xref_stream_pdf_with_dictionary_tail(" /Encrypt 4 0 R"),
            1
        ),
        Err(WriteError::EncryptedInput)
    );
}

// Successful vertical: classic
// ---------------------------------------------------------------------------

#[test]
fn classic_mixed_dirty_owner_and_fresh_target_round_trips() {
    let input = sample_pdf();
    let reserved = reserve_fresh_object_references(&input, 2).expect("reserve two fresh objects");
    assert_eq!(
        reserved,
        vec![
            IndirectRef {
                object_number: 4,
                generation: 0
            },
            IndirectRef {
                object_number: 5,
                generation: 0
            },
        ]
    );

    let owner_body = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Extra {} 0 R >>",
        reserved[0].object_number
    );
    let dirty_objects = [dirty(3, 0, owner_body.as_bytes())];
    let fresh_objects = [
        fresh(
            reserved[0].object_number,
            b"<< /Kind (fresh-a) /Next 5 0 R >>",
        ),
        fresh(reserved[1].object_number, b"<< /Kind (fresh-b) >>"),
    ];

    let output =
        write_incremental_revision_with_fresh_objects(&input, &dirty_objects, &fresh_objects)
            .expect("mixed dirty+fresh append");

    assert_eq!(&output[..input.len()], input.as_slice());

    let access = reopen(&output);
    let chain = classic_chain(&access);

    let expected_bodies: [(u32, &[u8]); 3] = [
        (3, owner_body.as_bytes()),
        (
            reserved[0].object_number,
            b"<< /Kind (fresh-a) /Next 5 0 R >>",
        ),
        (reserved[1].object_number, b"<< /Kind (fresh-b) >>"),
    ];
    for (object_number, expected_body) in expected_bodies {
        let ClassicXrefObjectLocation::InUse { byte_offset, .. } =
            resolve_classic_xref_chain_object(chain, object_number)
        else {
            panic!("object {object_number} should resolve in-use");
        };
        assert!(byte_offset >= input.len());
        assert!(output[byte_offset..].starts_with(format!("{object_number} 0 obj").as_bytes()));
        assert_eq!(resolved_body_bytes(&output, byte_offset), expected_body);
    }

    assert!(last_trailer_size(&output) >= 6);
    assert_eq!(page_leaf_numbers(&access), vec![3]);
}

#[test]
fn classic_fresh_only_with_no_dirty_objects() {
    let input = sample_pdf();
    let reserved = reserve_fresh_object_references(&input, 1).expect("reserve one fresh object");

    let output = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(reserved[0].object_number, b"<< /Solo true >>")],
    )
    .expect("fresh-only append");

    assert_eq!(&output[..input.len()], input.as_slice());
    let access = reopen(&output);
    assert_eq!(page_leaf_numbers(&access), vec![3]);

    let chain = classic_chain(&access);
    let ClassicXrefObjectLocation::InUse { byte_offset, .. } =
        resolve_classic_xref_chain_object(chain, reserved[0].object_number)
    else {
        panic!("fresh object should resolve in-use");
    };
    assert_eq!(
        resolved_body_bytes(&output, byte_offset),
        b"<< /Solo true >>"
    );
}

#[test]
fn classic_output_is_deterministic_regardless_of_caller_order() {
    let input = sample_pdf();
    let reserved = reserve_fresh_object_references(&input, 2).expect("reserve");

    let dirty_forward = [dirty(2, 0, PAGES_BODY), dirty(3, 0, PAGE_BODY)];
    let dirty_reversed = [dirty(3, 0, PAGE_BODY), dirty(2, 0, PAGES_BODY)];
    let fresh_forward = [
        fresh(reserved[0].object_number, b"<< /A true >>"),
        fresh(reserved[1].object_number, b"<< /B true >>"),
    ];
    let fresh_reversed = [
        fresh(reserved[1].object_number, b"<< /B true >>"),
        fresh(reserved[0].object_number, b"<< /A true >>"),
    ];

    let forward =
        write_incremental_revision_with_fresh_objects(&input, &dirty_forward, &fresh_forward)
            .expect("forward");
    let reversed =
        write_incremental_revision_with_fresh_objects(&input, &dirty_reversed, &fresh_reversed)
            .expect("reversed");

    assert_eq!(forward, reversed);
}

#[test]
fn classic_second_reservation_starts_above_first_revisions_fresh_objects() {
    let input = sample_pdf();
    let first_reserved = reserve_fresh_object_references(&input, 1).expect("reserve first");
    assert_eq!(
        first_reserved,
        vec![IndirectRef {
            object_number: 4,
            generation: 0
        }]
    );

    let first_output = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(
            first_reserved[0].object_number,
            b"<< /First true /Dangles 999 0 R >>",
        )],
    )
    .expect("first append");

    let second_reserved =
        reserve_fresh_object_references(&first_output, 1).expect("reserve second");
    assert_eq!(
        second_reserved,
        vec![IndirectRef {
            object_number: 1000,
            generation: 0
        }]
    );
}

// ---------------------------------------------------------------------------
// Successful vertical: xref stream
// ---------------------------------------------------------------------------

#[test]
fn xref_stream_mixed_dirty_owner_and_fresh_target_round_trips() {
    let input = sample_xref_stream_pdf();
    let reserved = reserve_fresh_object_references(&input, 2).expect("reserve two fresh objects");
    assert_eq!(
        reserved,
        vec![
            IndirectRef {
                object_number: 6,
                generation: 0
            },
            IndirectRef {
                object_number: 7,
                generation: 0
            },
        ]
    );

    let owner_body = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Extra {} 0 R >>",
        reserved[0].object_number
    );
    let dirty_objects = [dirty(3, 0, owner_body.as_bytes())];
    let fresh_objects = [
        fresh(
            reserved[0].object_number,
            b"<< /Kind (fresh-a) /Next 7 0 R >>",
        ),
        fresh(reserved[1].object_number, b"<< /Kind (fresh-b) >>"),
    ];

    let output =
        write_incremental_revision_with_fresh_objects(&input, &dirty_objects, &fresh_objects)
            .expect("mixed dirty+fresh append");

    assert_eq!(&output[..input.len()], input.as_slice());

    let access = reopen(&output);
    let chain = xref_stream_chain(&access);

    let self_number = chain
        .entries
        .iter()
        .map(|entry| entry.object_number)
        .max()
        .expect("chain has entries");
    assert!(self_number > usize::try_from(reserved[1].object_number).unwrap());

    let expected_bodies: [(u32, &[u8]); 3] = [
        (3, owner_body.as_bytes()),
        (
            reserved[0].object_number,
            b"<< /Kind (fresh-a) /Next 7 0 R >>",
        ),
        (reserved[1].object_number, b"<< /Kind (fresh-b) >>"),
    ];
    for (object_number, expected_body) in expected_bodies {
        let entry = chain
            .entries
            .iter()
            .find(|entry| entry.object_number == object_number as usize)
            .expect("entry present");
        let XrefStreamEntryRecord::Uncompressed { byte_offset, .. } = entry.record else {
            panic!("object {object_number} should be an uncompressed entry");
        };
        assert!(byte_offset >= input.len());
        assert!(output[byte_offset..].starts_with(format!("{object_number} 0 obj").as_bytes()));
        assert_eq!(resolved_body_bytes(&output, byte_offset), expected_body);
    }

    assert!(last_trailer_size(&output) > self_number);
    assert_eq!(page_leaf_numbers(&access), vec![3]);
}

#[test]
fn xref_stream_fresh_only_with_no_dirty_objects() {
    let input = sample_xref_stream_pdf();
    let reserved = reserve_fresh_object_references(&input, 1).expect("reserve one fresh object");

    let output = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(reserved[0].object_number, b"<< /Solo true >>")],
    )
    .expect("fresh-only append");

    assert_eq!(&output[..input.len()], input.as_slice());
    let access = reopen(&output);
    assert_eq!(page_leaf_numbers(&access), vec![3]);

    let chain = xref_stream_chain(&access);
    let entry = chain
        .entries
        .iter()
        .find(|entry| entry.object_number == reserved[0].object_number as usize)
        .expect("fresh entry present");
    let XrefStreamEntryRecord::Uncompressed { byte_offset, .. } = entry.record else {
        panic!("fresh object should be an uncompressed entry");
    };
    assert_eq!(
        resolved_body_bytes(&output, byte_offset),
        b"<< /Solo true >>"
    );
}

#[test]
fn xref_stream_output_is_deterministic_regardless_of_caller_order() {
    let input = sample_xref_stream_pdf();
    let reserved = reserve_fresh_object_references(&input, 2).expect("reserve");

    let dirty_forward = [dirty(3, 0, PAGE_BODY)];
    let fresh_forward = [
        fresh(reserved[0].object_number, b"<< /A true >>"),
        fresh(reserved[1].object_number, b"<< /B true >>"),
    ];
    let fresh_reversed = [
        fresh(reserved[1].object_number, b"<< /B true >>"),
        fresh(reserved[0].object_number, b"<< /A true >>"),
    ];

    let forward =
        write_incremental_revision_with_fresh_objects(&input, &dirty_forward, &fresh_forward)
            .expect("forward");
    let reversed =
        write_incremental_revision_with_fresh_objects(&input, &dirty_forward, &fresh_reversed)
            .expect("reversed");

    assert_eq!(forward, reversed);
}

#[test]
fn xref_stream_self_object_skips_above_reference_to_the_nominal_next_object() {
    let input = sample_xref_stream_pdf();
    let reserved = reserve_fresh_object_references(&input, 1).expect("reserve one fresh object");
    assert_eq!(
        reserved,
        vec![IndirectRef {
            object_number: 6,
            generation: 0
        }]
    );

    // Without the self-avoidance scan, the naive "chain max + reservation"
    // self-slot would land on 7 -- exactly the object number this fresh body
    // references without it ever being part of the reservation.
    let fresh_objects = [fresh(6, b"<< /Peek 7 0 R >>")];
    let output = write_incremental_revision_with_fresh_objects(&input, &[], &fresh_objects)
        .expect("fresh-only append");

    let access = reopen(&output);
    let chain = xref_stream_chain(&access);
    let self_number = chain
        .entries
        .iter()
        .map(|entry| entry.object_number)
        .max()
        .expect("chain has entries");
    assert!(
        self_number >= 8,
        "self-object must skip above the referenced object 7, got {self_number}"
    );
    assert!(
        !chain.entries.iter().any(|entry| entry.object_number == 7),
        "object 7 must stay dangling, not accidentally satisfied by the self-object"
    );
}

#[test]
fn xref_stream_second_reservation_starts_above_first_revisions_self_object_and_dangling_refs() {
    let input = sample_xref_stream_pdf();
    let first_reserved = reserve_fresh_object_references(&input, 1).expect("reserve first");
    assert_eq!(
        first_reserved,
        vec![IndirectRef {
            object_number: 6,
            generation: 0
        }]
    );

    let first_output = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(6, b"<< /First true /Dangles 999 0 R >>")],
    )
    .expect("first append");

    let access = reopen(&first_output);
    let chain = xref_stream_chain(&access);
    let first_self_number = chain
        .entries
        .iter()
        .map(|entry| entry.object_number)
        .max()
        .expect("chain has entries");

    let second_reserved =
        reserve_fresh_object_references(&first_output, 1).expect("reserve second");
    let second_first = usize::try_from(second_reserved[0].object_number).unwrap();
    assert!(second_first > first_self_number);
    assert!(second_first > 999);
}

// ---------------------------------------------------------------------------
