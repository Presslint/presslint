#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use crate::{
    XrefStreamEntriesError, XrefStreamEntriesRejection, XrefStreamEntry, XrefStreamEntryRecord,
    XrefStreamSubsection, parse_xref_stream_entries,
};

fn subsection(first_object_number: usize, entry_count: usize) -> XrefStreamSubsection {
    XrefStreamSubsection {
        first_object_number,
        entry_count,
    }
}

fn entry(object_number: usize, record: XrefStreamEntryRecord) -> XrefStreamEntry {
    XrefStreamEntry {
        object_number,
        record,
    }
}

fn reason(
    decoded: &[u8],
    widths: [usize; 3],
    subsections: &[XrefStreamSubsection],
) -> XrefStreamEntriesRejection {
    parse_xref_stream_entries(decoded, widths, subsections)
        .expect_err("xref-stream entries should reject")
        .reason
}

#[test]
fn decodes_type_one_uncompressed_entry() {
    let entries = parse_xref_stream_entries(&[1, 0x01, 0x2c, 0, 7], [1, 2, 2], &[subsection(4, 1)])
        .expect("entries should decode");

    assert_eq!(
        entries,
        vec![entry(
            4,
            XrefStreamEntryRecord::Uncompressed {
                byte_offset: 300,
                generation: 7,
            },
        )]
    );
}

#[test]
fn decodes_type_zero_and_type_two_entries() {
    let entries = parse_xref_stream_entries(
        &[0, 0, 9, 0, 2, 2, 0, 15, 0, 3],
        [1, 2, 2],
        &[subsection(0, 2)],
    )
    .expect("entries should decode");

    assert_eq!(
        entries,
        vec![
            entry(
                0,
                XrefStreamEntryRecord::Free {
                    next_free_object_number: 9,
                    generation: 2,
                },
            ),
            entry(
                1,
                XrefStreamEntryRecord::Compressed {
                    object_stream_number: 15,
                    index_within_object_stream: 3,
                },
            ),
        ]
    );
}

#[test]
fn defaults_missing_type_field_to_uncompressed() {
    let entries = parse_xref_stream_entries(&[0x01, 0xf4, 0, 0], [0, 2, 2], &[subsection(10, 1)])
        .expect("entries should decode");

    assert_eq!(
        entries,
        vec![entry(
            10,
            XrefStreamEntryRecord::Uncompressed {
                byte_offset: 500,
                generation: 0,
            },
        )]
    );
}

#[test]
fn derives_object_numbers_across_multiple_subsections() {
    let entries = parse_xref_stream_entries(
        &[1, 0, 100, 1, 0, 101, 1, 0, 200],
        [1, 2, 0],
        &[subsection(2, 2), subsection(8, 1)],
    )
    .expect("entries should decode");

    assert_eq!(
        entries,
        vec![
            entry(
                2,
                XrefStreamEntryRecord::Uncompressed {
                    byte_offset: 100,
                    generation: 0,
                },
            ),
            entry(
                3,
                XrefStreamEntryRecord::Uncompressed {
                    byte_offset: 101,
                    generation: 0,
                },
            ),
            entry(
                8,
                XrefStreamEntryRecord::Uncompressed {
                    byte_offset: 200,
                    generation: 0,
                },
            ),
        ]
    );
}

#[test]
fn surfaces_reserved_entry_type_with_raw_fields() {
    let entries = parse_xref_stream_entries(&[7, 0, 42, 0, 9], [1, 2, 2], &[subsection(6, 1)])
        .expect("entries should decode");

    assert_eq!(
        entries,
        vec![entry(
            6,
            XrefStreamEntryRecord::Reserved {
                entry_type: 7,
                field2: 42,
                field3: 9,
            },
        )]
    );
}

#[test]
fn rejects_length_mismatch() {
    assert_eq!(
        reason(&[1, 0, 10], [1, 2, 2], &[subsection(0, 1)]),
        XrefStreamEntriesRejection::LengthMismatch {
            expected: 5,
            actual: 3,
        }
    );
}

#[test]
fn rejects_zero_record_width() {
    assert_eq!(
        reason(&[], [0, 0, 0], &[subsection(0, 0)]),
        XrefStreamEntriesRejection::ZeroRecordWidth
    );
}

#[test]
fn rejects_field_width_out_of_range() {
    assert_eq!(
        reason(&[], [1, 9, 1], &[subsection(0, 0)]),
        XrefStreamEntriesRejection::FieldWidthOutOfRange {
            field_index: 1,
            width: 9,
        }
    );
}

#[test]
fn rejects_object_number_overflow() {
    assert_eq!(
        reason(&[1, 0, 0, 1, 0, 0], [1, 1, 1], &[subsection(usize::MAX, 2)]),
        XrefStreamEntriesRejection::ObjectNumberOutOfRange {
            first_object_number: usize::MAX,
            entry_index: 1,
        }
    );
}

#[test]
fn serde_round_trip_preserves_report_and_rejection_shapes() {
    let report = parse_xref_stream_entries(&[1, 0, 42, 0, 3], [1, 2, 2], &[subsection(5, 1)])
        .expect("entries should decode");

    let value = serde_value(&report).expect("report should serialize");
    assert_eq!(
        value,
        TestSerdeValue::Seq(vec![TestSerdeValue::Map(vec![
            ("object_number".to_string(), TestSerdeValue::U64(5)),
            (
                "record".to_string(),
                TestSerdeValue::Map(vec![
                    (
                        "type".to_string(),
                        TestSerdeValue::String("uncompressed".to_string()),
                    ),
                    ("byte_offset".to_string(), TestSerdeValue::U64(42)),
                    ("generation".to_string(), TestSerdeValue::U64(3)),
                ]),
            ),
        ])])
    );
    let decoded: Vec<XrefStreamEntry> = from_serde_value(value).expect("report should deserialize");
    assert_eq!(decoded, report);

    let error = parse_xref_stream_entries(&[1, 0, 42], [1, 2, 2], &[subsection(5, 1)])
        .expect_err("length mismatch should reject");
    let error_value = serde_value(&error).expect("error should serialize");
    assert_eq!(
        error_value,
        TestSerdeValue::Map(vec![
            ("decoded_len".to_string(), TestSerdeValue::U64(3)),
            ("error_byte_offset".to_string(), TestSerdeValue::None),
            ("object_number".to_string(), TestSerdeValue::None),
            (
                "reason".to_string(),
                TestSerdeValue::Map(vec![
                    (
                        "reason".to_string(),
                        TestSerdeValue::String("length_mismatch".to_string()),
                    ),
                    ("expected".to_string(), TestSerdeValue::U64(5)),
                    ("actual".to_string(), TestSerdeValue::U64(3)),
                ]),
            ),
        ])
    );
    let decoded_error: XrefStreamEntriesError =
        from_serde_value(error_value).expect("error should deserialize");
    assert_eq!(decoded_error, error);
}
