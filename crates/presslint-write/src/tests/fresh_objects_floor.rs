use presslint_pdf::{IndirectRef, MAX_OBJECT_BODY_REFERENCES};

use crate::{
    WriteError, reserve_fresh_object_references, write_incremental_revision_with_fresh_objects,
};

use super::fresh_objects::{
    classic_pdf_with_objects_and_size, classic_pdf_with_standard_objects,
    classic_pdf_with_standard_objects_plus_fourth, fresh, object_stream_body,
    push_one_page_objects, sample_xref_stream_pdf_with_dictionary_tail,
    xref_stream_pdf_with_one_page,
};
use super::{
    CATALOG_BODY, PAGE_BODY, PAGES_BODY, dirty, page_leaf_numbers, reopen, sample_pdf,
    sample_pdf_with_prev_chain, sample_pdf_with_trailer_tail, xref_record,
};

// ---------------------------------------------------------------------------
// Allocation-floor proof
// ---------------------------------------------------------------------------

#[test]
fn floor_uses_whole_chain_size_over_newest_section_size() {
    let input = sample_pdf_with_prev_chain();
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 6,
            generation: 0
        }]
    );
}

/// Objects 1..=3 plus a second classic subsection declaring one free entry at
/// object 9, with `/Size` deliberately understated at 4 so only the entry
/// scan -- not `/Size` -- can raise the floor above the free entry.
fn sample_pdf_with_high_free_entry() -> Vec<u8> {
    let (mut buf, offsets) = classic_pdf_with_standard_objects();
    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(b"9 1\n0000000000 00000 f \n");
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF").as_bytes(),
    );
    buf
}

#[test]
fn floor_never_reuses_a_high_free_entry() {
    let input = sample_pdf_with_high_free_entry();
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 10,
            generation: 0
        }]
    );
}

#[test]
fn floor_raises_above_a_dangling_classic_trailer_reference() {
    let input = sample_pdf_with_trailer_tail(" /Extra 99 0 R");
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 100,
            generation: 0
        }]
    );
}

/// Objects 1..=3 plus a high in-use object 9, with `/Size` deliberately
/// understated at 4 so only the entry scan -- not `/Size` -- can raise the
/// floor above the high defined (not free) entry.
fn sample_pdf_with_high_defined_entry() -> Vec<u8> {
    let (mut buf, offsets) = classic_pdf_with_standard_objects();
    let high_offset = buf.len();
    buf.extend_from_slice(b"9 0 obj\n<< /Type /Unreferenced >>\nendobj\n");

    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(b"9 1\n");
    buf.extend_from_slice(format!("{high_offset:010} 00000 n \n").as_bytes());
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF").as_bytes(),
    );
    buf
}

#[test]
fn floor_never_reuses_a_high_defined_entry_despite_understated_size() {
    let input = sample_pdf_with_high_defined_entry();
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 10,
            generation: 0
        }]
    );
}

/// Objects 1..=3, where the `/Pages` object (2) -- reachable both forward
/// from the catalog's `/Kids` edge and backward via the page's `/Parent`
/// edge -- carries an extra dangling reference to object 66.
fn sample_pdf_with_dangling_reference_via_reachable_pages_object() -> Vec<u8> {
    classic_pdf_with_objects_and_size(
        &[
            (1, CATALOG_BODY),
            (
                2,
                b"<< /Type /Pages /Kids [3 0 R] /Count 1 /Extra 66 0 R >>",
            ),
            (3, PAGE_BODY),
        ],
        4,
    )
}

#[test]
fn floor_raises_above_a_dangling_reference_reached_via_the_page_parent_edge() {
    let input = sample_pdf_with_dangling_reference_via_reachable_pages_object();
    let access = reopen(&input);
    // Object 2 is genuinely reachable, not orphaned: it is the page's /Parent
    // target and the catalog's /Kids target.
    assert_eq!(page_leaf_numbers(&access), vec![3]);
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 67,
            generation: 0
        }]
    );
}

/// Objects 1..=3 plus an xref-defined but otherwise unreferenced object 4
/// whose body dangles a reference to object 77.
fn sample_pdf_with_unreferenced_dangling_body() -> Vec<u8> {
    classic_pdf_with_standard_objects_plus_fourth(b"<< /Dangles 77 0 R >>")
}

#[test]
fn floor_raises_above_a_dangling_reference_in_an_unreferenced_object_body() {
    let input = sample_pdf_with_unreferenced_dangling_body();
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 78,
            generation: 0
        }]
    );
}

/// Object 4 is a stream whose payload contains reference-shaped bytes that
/// must never be scanned, and whose dictionary carries a real dangling
/// reference that must be scanned.
fn sample_pdf_with_opaque_stream_payload_and_dict_reference() -> Vec<u8> {
    let payload = b"999999 0 R looks like a reference but is opaque stream data";
    let mut fourth_body =
        format!("<< /Length {} /Real 42 0 R >>\nstream\n", payload.len()).into_bytes();
    fourth_body.extend_from_slice(payload);
    fourth_body.extend_from_slice(b"\nendstream");
    classic_pdf_with_standard_objects_plus_fourth(&fourth_body)
}

#[test]
fn floor_ignores_opaque_stream_payload_but_sees_the_dictionary_reference() {
    let input = sample_pdf_with_opaque_stream_payload_and_dict_reference();
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 43,
            generation: 0
        }]
    );
}

/// Objects 1..=3 plus a fourth object whose body contains an out-of-range
/// reference-shaped construct (an object number that does not fit `u32`).
fn sample_pdf_with_out_of_range_reference_in_body() -> Vec<u8> {
    classic_pdf_with_standard_objects_plus_fourth(b"<< /Bad 99999999999 0 R >>")
}

#[test]
fn rejects_out_of_range_reference_shape_as_an_incomplete_proof() {
    let input = sample_pdf_with_out_of_range_reference_in_body();
    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshFloorObjectReferencesIncomplete {
            reference: IndirectRef {
                object_number: 4,
                generation: 0
            }
        }
    );
}

#[test]
fn malformed_effective_live_body_reports_body_reference_failure() {
    // The xref entry and indirect-object header are valid, but the live
    // object's leading array never closes, so body-aware reference inspection
    // cannot prove a complete syntax extent.
    let input = classic_pdf_with_standard_objects_plus_fourth(b"[1 0 R");
    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert!(matches!(
        error,
        WriteError::FreshFloorBodyReferences {
            reference: IndirectRef {
                object_number: 4,
                generation: 0,
            },
            ..
        }
    ));
}

/// Objects 1..=3 plus a fourth object whose body carries many references,
/// exceeding the writer-local cumulative reference cap in one body.
fn sample_pdf_with_many_references_in_one_body() -> Vec<u8> {
    let refs = "9 0 R ".repeat(crate::fresh_objects::MAX_FRESH_FLOOR_REFERENCES + 1);
    let body = format!("<< /Refs [{refs}] >>");
    classic_pdf_with_standard_objects_plus_fourth(body.as_bytes())
}

#[test]
fn rejects_cumulative_reference_cap_exhaustion() {
    let input = sample_pdf_with_many_references_in_one_body();
    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshFloorReferenceCapExceeded {
            max_references: crate::fresh_objects::MAX_FRESH_FLOOR_REFERENCES,
        }
    );
}

/// A body alone over the per-body [`MAX_OBJECT_BODY_REFERENCES`] scan cap,
/// well above the smaller cumulative cap
/// [`sample_pdf_with_many_references_in_one_body`] already covers.
fn sample_pdf_with_per_body_reference_truncation() -> Vec<u8> {
    let refs = "9 0 R ".repeat(MAX_OBJECT_BODY_REFERENCES + 1);
    let body = format!("<< /Refs [{refs}] >>");
    classic_pdf_with_standard_objects_plus_fourth(body.as_bytes())
}

#[test]
fn rejects_per_body_reference_truncation_as_an_incomplete_proof() {
    let input = sample_pdf_with_per_body_reference_truncation();
    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshFloorObjectReferencesIncomplete {
            reference: IndirectRef {
                object_number: 4,
                generation: 0
            }
        }
    );
}

/// One `/ObjStm` container (object 5) holding `member_a` at object 6 and
/// `member_b` at object 7, in a one-page xref-stream document (catalog=1,
/// pages=2, page=3, info=4). The xref-stream self-object is object 8, and
/// `/Index [0 9]` covers every object number contiguously.
fn xref_stream_pdf_with_compressed_members(member_a: &[u8], member_b: &[u8]) -> Vec<u8> {
    xref_stream_pdf_with_compressed_members_custom((6, member_a), (7, member_b))
}

#[test]
fn floor_raises_above_a_dangling_reference_in_a_compressed_member() {
    let input =
        xref_stream_pdf_with_compressed_members(b"<< /Dangles 999 0 R >>", b"<< /Plain (ok) >>");
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 1000,
            generation: 0
        }]
    );
}

/// Like [`xref_stream_pdf_with_compressed_members`], but the caller controls
/// the physical `(object_number, body)` packing order inside the `/ObjStm`
/// container (independent of the always-ascending xref entry order) and
/// whether entries use `/W [1 2 1]` or (`wide`) `/W [1 3 1]` offsets.
fn xref_stream_pdf_with_compressed_members_custom_wide(
    member_first: (u32, &[u8]),
    member_second: (u32, &[u8]),
    wide: bool,
) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let (catalog_offset, pages_offset, page_offset) = push_one_page_objects(&mut buf);
    let info_offset = buf.len();
    buf.extend_from_slice(b"4 0 obj\n<< /Producer (presslint-test) >>\nendobj\n");

    let (first, members_body) = object_stream_body(&[member_first, member_second]);
    let objstm_offset = buf.len();
    buf.extend_from_slice(
        format!(
            "5 0 obj\n<< /Type /ObjStm /N 2 /First {first} /Length {} >>\nstream\n",
            members_body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&members_body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");

    let index_within_container =
        |object_number: u32| -> u8 { u8::from(object_number != member_first.0) };
    let (lower, higher) = if member_first.0 < member_second.0 {
        (member_first.0, member_second.0)
    } else {
        (member_second.0, member_first.0)
    };

    let xref_offset = buf.len();
    let mut body = Vec::new();
    let mut push = |entry_type: u8, field2: usize, field3: u8| {
        if wide {
            body.extend_from_slice(&xref_record_wide(entry_type, field2, field3));
        } else {
            body.extend_from_slice(&xref_record(entry_type, field2, field3));
        }
    };
    push(0, 0, 0);
    push(1, catalog_offset, 0);
    push(1, pages_offset, 0);
    push(1, page_offset, 0);
    push(1, info_offset, 0);
    push(1, objstm_offset, 0);
    push(2, 5, index_within_container(lower));
    push(2, 5, index_within_container(higher));
    push(1, xref_offset, 0);

    let widths = if wide { "[1 3 1]" } else { "[1 2 1]" };
    buf.extend_from_slice(
        format!(
            "8 0 obj\n<< /Type /XRef /Size 9 /Index [0 9] /W {widths} /Root 1 0 R /Info 4 0 R /Length {} >>\nstream\n",
            body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");
    buf.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());
    buf
}

fn xref_stream_pdf_with_compressed_members_custom(
    member_first: (u32, &[u8]),
    member_second: (u32, &[u8]),
) -> Vec<u8> {
    xref_stream_pdf_with_compressed_members_custom_wide(member_first, member_second, false)
}

#[test]
fn compressed_member_floor_is_independent_of_physical_container_order() {
    let member_6: &[u8] = b"<< /Dangles 50 0 R >>";
    let member_7: &[u8] = b"<< /Other 60 0 R >>";

    let physical_ascending =
        xref_stream_pdf_with_compressed_members_custom((6, member_6), (7, member_7));
    let physical_descending =
        xref_stream_pdf_with_compressed_members_custom((7, member_7), (6, member_6));

    let floor_ascending =
        reserve_fresh_object_references(&physical_ascending, 1).expect("floor proof a");
    let floor_descending =
        reserve_fresh_object_references(&physical_descending, 1).expect("floor proof b");

    assert_eq!(floor_ascending, floor_descending);
    assert_eq!(
        floor_ascending,
        vec![IndirectRef {
            object_number: 61,
            generation: 0
        }]
    );
}

/// Pack one `/W [1 3 1]` wide-offset entry (3-byte `field2`), for fixtures
/// whose total byte length exceeds the `/W [1 2 1]` 65535 offset limit
/// [`xref_record`] assumes.
fn xref_record_wide(entry_type: u8, field2: usize, field3: u8) -> [u8; 5] {
    let bytes = u32::try_from(field2)
        .expect("test field2 fits u32")
        .to_be_bytes();
    [entry_type, bytes[1], bytes[2], bytes[3], field3]
}

/// Like [`xref_stream_pdf_with_compressed_members`] but uses `/W [1 3 1]`
/// wide byte offsets, for members large enough to push the total file past
/// the 65535 two-byte offset limit.
fn xref_stream_pdf_with_compressed_members_wide(member_a: &[u8], member_b: &[u8]) -> Vec<u8> {
    xref_stream_pdf_with_compressed_members_custom_wide((6, member_a), (7, member_b), true)
}

#[test]
fn rejects_cumulative_compressed_decode_work_cap_exhaustion() {
    // Two members from the SAME container: resolving each re-decodes the
    // whole container (no caching), so two ~600KB decodes exceed the 1 MiB
    // writer-local cumulative cap on the second member.
    let filler = "x".repeat(600_000);
    let member_a = format!("<< /Filler ({filler}) >>");
    let input =
        xref_stream_pdf_with_compressed_members_wide(member_a.as_bytes(), b"<< /Small (y) >>");

    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshFloorDecodeWorkCapExceeded {
            max_decoded_bytes: crate::fresh_objects::MAX_FRESH_FLOOR_DECODE_WORK_BYTES,
        }
    );
}

/// A defective section whose entry map omits its own self-entry (object 4).
fn xref_stream_pdf_with_missing_self_entry_and_size(declared_size: u32) -> Vec<u8> {
    xref_stream_pdf_with_one_page(false, declared_size, "")
}

fn xref_stream_pdf_with_missing_self_entry() -> Vec<u8> {
    xref_stream_pdf_with_missing_self_entry_and_size(4)
}

#[test]
fn floor_reserves_above_a_defective_sections_own_header_identity() {
    let input = xref_stream_pdf_with_missing_self_entry();
    // Without proving the section's own header number (4), the entry-only
    // floor would stop at 4 (from /Size and the highest entry, object 3).
    assert_eq!(
        reserve_fresh_object_references(&input, 1).expect("floor proof"),
        vec![IndirectRef {
            object_number: 5,
            generation: 0
        }]
    );
}

/// A one-page xref-stream document whose entry map declares one reserved
/// (type 3) entry at object 4.
fn xref_stream_pdf_with_reserved_entry() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let (catalog_offset, pages_offset, page_offset) = push_one_page_objects(&mut buf);
    let xref_offset = buf.len();

    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(1, page_offset, 0));
    body.extend_from_slice(&xref_record(3, 0, 0));
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

#[test]
fn rejects_a_reserved_xref_stream_entry_type() {
    let input = xref_stream_pdf_with_reserved_entry();
    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshFloorReservedEntry {
            object_number: 4,
            entry_type: 3,
        }
    );
}

// ---------------------------------------------------------------------------
// Fail-closed validation
// ---------------------------------------------------------------------------

#[test]
fn rejects_duplicate_fresh_object_number() {
    let input = sample_pdf();
    let error = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(4, b"<< /A true >>"), fresh(4, b"<< /B true >>")],
    )
    .unwrap_err();
    assert_eq!(error, WriteError::DuplicateFreshObject { object_number: 4 });
}

#[test]
fn rejects_nonzero_fresh_generation() {
    let input = sample_pdf();
    let mut object = fresh(4, b"<< /A true >>");
    object.reference.generation = 1;
    let error =
        write_incremental_revision_with_fresh_objects(&input, &[], &[object.clone()]).unwrap_err();
    assert_eq!(
        error,
        WriteError::NonZeroFreshGeneration {
            reference: object.reference
        }
    );
}

#[test]
fn rejects_gapped_fresh_reservation() {
    let input = sample_pdf();
    let error = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(4, b"<< /A true >>"), fresh(6, b"<< /B true >>")],
    )
    .unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshReservationNotContiguous {
            previous: IndirectRef {
                object_number: 4,
                generation: 0
            },
            next: IndirectRef {
                object_number: 6,
                generation: 0
            },
        }
    );
}

#[test]
fn rejects_fresh_reservation_starting_too_low() {
    let input = sample_pdf();
    let error = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(1, b"<< /TooLow true >>")],
    )
    .unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshReservationFloorMismatch {
            expected_floor: 4,
            found_first: 1,
        }
    );
}

#[test]
fn rejects_fresh_reservation_starting_too_high() {
    let input = sample_pdf();
    let error = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(10, b"<< /TooHigh true >>")],
    )
    .unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshReservationFloorMismatch {
            expected_floor: 4,
            found_first: 10,
        }
    );
}

#[test]
fn rejects_dirty_fresh_object_number_collision() {
    let input = sample_pdf();
    let error = write_incremental_revision_with_fresh_objects(
        &input,
        &[dirty(3, 0, PAGE_BODY)],
        &[fresh(3, b"<< /Bad true >>")],
    )
    .unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshDirtyObjectCollision { object_number: 3 }
    );
}

/// A valid classic source whose page is object `3 7`, proving collision is
/// by object number even when the dirty identity has a nonzero generation.
fn classic_pdf_with_nonzero_generation_page() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.4\n");

    let catalog_offset = buf.len();
    buf.extend_from_slice(b"1 0 obj\n");
    buf.extend_from_slice(CATALOG_BODY);
    buf.extend_from_slice(b"\nendobj\n");
    let pages_offset = buf.len();
    buf.extend_from_slice(b"2 0 obj\n");
    buf.extend_from_slice(PAGES_BODY);
    buf.extend_from_slice(b"\nendobj\n");
    let page_offset = buf.len();
    buf.extend_from_slice(b"3 7 obj\n");
    buf.extend_from_slice(PAGE_BODY);
    buf.extend_from_slice(b"\nendobj\n");

    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    buf.extend_from_slice(format!("{catalog_offset:010} 00000 n \n").as_bytes());
    buf.extend_from_slice(format!("{pages_offset:010} 00000 n \n").as_bytes());
    buf.extend_from_slice(format!("{page_offset:010} 00007 n \n").as_bytes());
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF").as_bytes(),
    );
    buf
}

#[test]
fn rejects_dirty_fresh_collision_with_valid_nonzero_dirty_generation() {
    let input = classic_pdf_with_nonzero_generation_page();
    let error = write_incremental_revision_with_fresh_objects(
        &input,
        &[dirty(3, 7, PAGE_BODY)],
        &[fresh(3, b"<< /Bad true >>")],
    )
    .unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshDirtyObjectCollision { object_number: 3 }
    );
}

#[test]
fn reservation_computed_for_one_input_is_rejected_for_another_with_a_higher_floor() {
    let small_input = sample_pdf();
    let reserved = reserve_fresh_object_references(&small_input, 1).expect("floor proof");
    assert_eq!(
        reserved,
        vec![IndirectRef {
            object_number: 4,
            generation: 0
        }]
    );

    let bigger_floor_input = sample_pdf_with_trailer_tail(" /Extra 50 0 R");
    let error = write_incremental_revision_with_fresh_objects(
        &bigger_floor_input,
        &[],
        &[fresh(4, b"<< /Fresh true >>")],
    )
    .unwrap_err();
    assert_eq!(
        error,
        WriteError::FreshReservationFloorMismatch {
            expected_floor: 51,
            found_first: 4,
        }
    );
}

/// Objects 1..=3 with a trailer that declares a `/Size` at exactly `u32::MAX`,
/// so a two-object reservation cannot fit the public `u32` object-number
/// field.
fn sample_pdf_with_huge_declared_size() -> Vec<u8> {
    let (mut buf, offsets) = classic_pdf_with_standard_objects();
    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4294967295 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF")
            .as_bytes(),
    );
    buf
}

#[test]
fn rejects_reservation_count_overflow_against_the_u32_object_number_field() {
    let input = sample_pdf_with_huge_declared_size();
    let error = reserve_fresh_object_references(&input, 2).unwrap_err();
    assert_eq!(error, WriteError::FreshReservationNumberOverflow);
}

#[test]
fn reserve_fails_closed_on_a_broken_prev_chain() {
    // /Prev points far past the end of the source bytes, so the /Prev chain
    // walk cannot locate a section there, in either backend.
    let classic = sample_pdf_with_trailer_tail(" /Prev 999999999");
    assert!(matches!(
        reserve_fresh_object_references(&classic, 1).unwrap_err(),
        WriteError::ClassicXrefChain { .. }
    ));

    let stream = sample_xref_stream_pdf_with_dictionary_tail(" /Prev 999999999");
    assert!(matches!(
        reserve_fresh_object_references(&stream, 1).unwrap_err(),
        WriteError::XrefStreamChain { .. }
    ));
}

/// A minimal classic-xref PDF whose trailer dictionary never closes its
/// `<<`, so the trailer scan cannot complete.
fn sample_pdf_with_unclosed_trailer_dictionary() -> Vec<u8> {
    let (mut buf, offsets) = classic_pdf_with_standard_objects();
    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size 4 /Root 1 0 R\nstartxref\n{xref_offset}\n%%EOF").as_bytes(),
    );
    buf
}

#[test]
fn reserve_fails_closed_on_a_malformed_trailer_dictionary() {
    let input = sample_pdf_with_unclosed_trailer_dictionary();
    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert!(
        matches!(error, WriteError::ActiveTrailer { .. }),
        "expected an active-trailer scan failure, got {error:?}"
    );
}

/// A compressed member whose `/ObjStm` header declares object 999 while the
/// xref entry requests object 5 -- a genuinely malformed member, distinct
/// from hitting the decode-work cap.
fn xref_stream_pdf_with_object_number_mismatch_in_compressed_member() -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(b"%PDF-1.5\n");
    let (catalog_offset, pages_offset, page_offset) = push_one_page_objects(&mut buf);

    let (first, members_body) = object_stream_body(&[(999, b"<< /Bad true >>")]);
    let objstm_offset = buf.len();
    buf.extend_from_slice(
        format!(
            "4 0 obj\n<< /Type /ObjStm /N 1 /First {first} /Length {} >>\nstream\n",
            members_body.len()
        )
        .as_bytes(),
    );
    buf.extend_from_slice(&members_body);
    buf.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_offset = buf.len();
    let mut body = Vec::new();
    body.extend_from_slice(&xref_record(0, 0, 0));
    body.extend_from_slice(&xref_record(1, catalog_offset, 0));
    body.extend_from_slice(&xref_record(1, pages_offset, 0));
    body.extend_from_slice(&xref_record(1, page_offset, 0));
    body.extend_from_slice(&xref_record(1, objstm_offset, 0));
    body.extend_from_slice(&xref_record(2, 4, 0));
    body.extend_from_slice(&xref_record(1, xref_offset, 0));

    buf.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /Index [0 7] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
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
fn reserve_fails_closed_on_a_malformed_compressed_member() {
    let input = xref_stream_pdf_with_object_number_mismatch_in_compressed_member();
    let error = reserve_fresh_object_references(&input, 1).unwrap_err();
    assert!(
        matches!(
            error,
            WriteError::FreshFloorResolution {
                reference: IndirectRef {
                    object_number: 5,
                    generation: 0
                },
                ..
            }
        ),
        "expected a resolution failure for object 5, got {error:?}"
    );
}

#[test]
fn xref_stream_self_object_overflow_after_otherwise_valid_fresh_objects() {
    // /Size declared at exactly u32::MAX leaves a single-object reservation
    // landing exactly at u32::MAX, with no room for the self-object above it.
    let input = xref_stream_pdf_with_missing_self_entry_and_size(u32::MAX);
    let reserved = reserve_fresh_object_references(&input, 1).expect("floor proof");
    assert_eq!(
        reserved,
        vec![IndirectRef {
            object_number: u32::MAX,
            generation: 0
        }]
    );

    let error = write_incremental_revision_with_fresh_objects(
        &input,
        &[],
        &[fresh(u32::MAX, b"<< /AtTheEdge true >>")],
    )
    .unwrap_err();
    assert_eq!(error, WriteError::FreshXrefSelfObjectOverflow);
}
