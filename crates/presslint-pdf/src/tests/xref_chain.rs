#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ObjectLookup, ObjectLookupLocation, XrefStreamChain, XrefStreamChainError,
    XrefStreamChainRejection, XrefStreamEntry, XrefStreamEntryRecord, build_xref_stream_chain,
    locate_xref_object,
};

const MAX_DECODED: usize = 4096;

fn xref_record(entry_type: u8, field2: usize, generation: u8) -> [u8; 3] {
    [
        entry_type,
        u8::try_from(field2).expect("test field2 fits one byte"),
        generation,
    ]
}

fn xref_stream_object(number: usize, prev: Option<usize>, records: &[u8], size: usize) -> Vec<u8> {
    let prev = prev.map_or_else(String::new, |offset| format!(" /Prev {offset}"));
    let mut object = format!(
        "{number} 0 obj\n<< /Type /XRef /Size {size} /W [ 1 1 1 ] /Index [ 0 {size} ] /Root 1 0 R{prev} /Length {} >>\nstream\n",
        records.len()
    )
    .into_bytes();
    object.extend_from_slice(records);
    object.extend_from_slice(b"\nendstream\nendobj\n");
    object
}

fn push_object(source: &mut Vec<u8>, object: &[u8]) -> usize {
    let offset = source.len();
    source.extend_from_slice(object);
    offset
}

fn entry(object_number: usize, record: XrefStreamEntryRecord) -> XrefStreamEntry {
    XrefStreamEntry {
        object_number,
        record,
    }
}

#[test]
fn merges_two_section_chain_newest_wins_and_free_shadows_old_in_use() {
    let mut source = b"%PDF-1.5\n".to_vec();
    let older_records = [
        xref_record(0, 0, 0),
        xref_record(1, 10, 0),
        xref_record(1, 20, 0),
    ]
    .concat();
    let older_offset = push_object(
        &mut source,
        &xref_stream_object(10, None, &older_records, 3),
    );

    let newer_records = [
        xref_record(0, 0, 0),
        xref_record(1, 99, 0),
        xref_record(0, 0, 7),
    ]
    .concat();
    let newer_offset = push_object(
        &mut source,
        &xref_stream_object(11, Some(older_offset), &newer_records, 3),
    );

    let chain = build_xref_stream_chain(&source, newer_offset, MAX_DECODED)
        .expect("two-section xref-stream chain should build");

    assert_eq!(chain.startxref_byte_offset, newer_offset);
    assert_eq!(chain.section_byte_offsets, vec![newer_offset, older_offset]);
    assert_eq!(chain.effective_size, 3);
    assert_eq!(
        chain.entries,
        vec![
            entry(
                0,
                XrefStreamEntryRecord::Free {
                    next_free_object_number: 0,
                    generation: 0,
                },
            ),
            entry(
                1,
                XrefStreamEntryRecord::Uncompressed {
                    byte_offset: 99,
                    generation: 0,
                },
            ),
            entry(
                2,
                XrefStreamEntryRecord::Free {
                    next_free_object_number: 0,
                    generation: 7,
                },
            ),
        ]
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamChain(&chain), 1),
        ObjectLookupLocation::XrefStreamUncompressed {
            object_number: 1,
            generation: 0,
            byte_offset: 99,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamChain(&chain), 2),
        ObjectLookupLocation::XrefStreamFree {
            object_number: 2,
            generation: 7,
            next_free_object_number: 0,
        }
    );
}

#[test]
fn detects_prev_cycle_without_looping() {
    let records = xref_record(0, 0, 0);
    let source = xref_stream_object(10, Some(0), &records, 1);

    let error = build_xref_stream_chain(&source, 0, MAX_DECODED)
        .expect_err("self-referential /Prev should be a cycle");

    assert_eq!(
        error.reason,
        XrefStreamChainRejection::Cycle { byte_offset: 0 }
    );
}

#[test]
fn stops_over_long_chain_at_section_bound() {
    let mut source = b"%PDF-1.5\n".to_vec();
    let mut previous = None;
    let mut newest = 0;
    for number in 1..=65 {
        let records = xref_record(0, 0, 0);
        newest = push_object(
            &mut source,
            &xref_stream_object(number, previous, &records, 1),
        );
        previous = Some(newest);
    }

    let error = build_xref_stream_chain(&source, newest, MAX_DECODED)
        .expect_err("65 sections should exceed the chain bound");

    assert_eq!(
        error.reason,
        XrefStreamChainRejection::SectionLimitExceeded { max_sections: 64 }
    );
}

#[test]
fn rejects_out_of_bounds_prev_offset() {
    let records = xref_record(0, 0, 0);
    let source = xref_stream_object(10, Some(9999), &records, 1);

    let error = build_xref_stream_chain(&source, 0, MAX_DECODED)
        .expect_err("out-of-bounds /Prev should reject");

    assert_eq!(
        error.reason,
        XrefStreamChainRejection::OffsetOutOfBounds { byte_offset: 9999 }
    );
}

#[test]
fn rejects_mixed_classic_prev_target() {
    let mut source = b"%PDF-1.5\n".to_vec();
    let table_offset = source.len();
    source.extend_from_slice(b"xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size 1 >>\n");
    let records = xref_record(0, 0, 0);
    let newest = push_object(
        &mut source,
        &xref_stream_object(10, Some(table_offset), &records, 1),
    );

    let error = build_xref_stream_chain(&source, newest, MAX_DECODED)
        .expect_err("classic /Prev target should reject as mixed type");

    assert_eq!(
        error.reason,
        XrefStreamChainRejection::PrevSectionNotXrefStream {
            byte_offset: table_offset,
        }
    );
}

#[test]
fn serde_round_trips_chain_report_and_error_shapes() {
    let records = [xref_record(0, 0, 0), xref_record(1, 42, 0)].concat();
    let source = xref_stream_object(10, None, &records, 2);
    let chain =
        build_xref_stream_chain(&source, 0, MAX_DECODED).expect("single chain should build");

    let value = serde_value(&chain).expect("chain report should serialize");
    let restored: XrefStreamChain = from_serde_value(value).expect("chain should deserialize");
    assert_eq!(restored, chain);

    let cycle_source = xref_stream_object(10, Some(0), &xref_record(0, 0, 0), 1);
    let error =
        build_xref_stream_chain(&cycle_source, 0, MAX_DECODED).expect_err("cycle should reject");
    let value = serde_value(&error).expect("chain error should serialize");
    let restored: XrefStreamChainError =
        from_serde_value(value).expect("chain error should deserialize");
    assert_eq!(restored, error);
}
