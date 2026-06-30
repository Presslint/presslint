#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use super::{classic_entry, classic_inspection, classic_subsection};

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefEntryState, ObjectLookup, ObjectLookupLocation,
    XrefStreamEntry, XrefStreamEntryRecord, XrefStreamSection, XrefStreamSubsection,
    locate_xref_object,
};

fn xref_stream_section(entries: Vec<XrefStreamEntry>) -> XrefStreamSection {
    XrefStreamSection {
        object_byte_offset: 200,
        widths: [1, 4, 2],
        size: 20,
        index_subsections: vec![XrefStreamSubsection {
            first_object_number: 0,
            entry_count: 20,
        }],
        root_reference: super::indirect_ref(1, 0),
        prev_byte_offset: None,
        entries,
    }
}

fn entry(object_number: usize, record: XrefStreamEntryRecord) -> XrefStreamEntry {
    XrefStreamEntry {
        object_number,
        record,
    }
}

#[test]
fn locates_classic_entries_through_unified_lookup() {
    let xref = classic_inspection(vec![
        classic_subsection(
            1,
            vec![classic_entry(1, 0, 42, ClassicXrefEntryState::InUse)],
        ),
        classic_subsection(2, vec![classic_entry(2, 3, 7, ClassicXrefEntryState::Free)]),
    ]);

    assert_eq!(
        locate_xref_object(ObjectLookup::ClassicXref(&xref), 1),
        ObjectLookupLocation::ClassicInUse {
            object_number: 1,
            generation: 0,
            byte_offset: 42,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::ClassicXref(&xref), 2),
        ObjectLookupLocation::ClassicFree {
            object_number: 2,
            generation: 3,
            next_free_object_number: 7,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::ClassicXref(&xref), 9),
        ObjectLookupLocation::ClassicNotFound { object_number: 9 }
    );
}

#[test]
fn locates_classic_ambiguity() {
    let xref = classic_inspection(vec![
        classic_subsection(
            5,
            vec![classic_entry(5, 0, 100, ClassicXrefEntryState::InUse)],
        ),
        classic_subsection(
            5,
            vec![classic_entry(5, 1, 200, ClassicXrefEntryState::Free)],
        ),
    ]);

    assert_eq!(
        locate_xref_object(ObjectLookup::ClassicXref(&xref), 5),
        ObjectLookupLocation::ClassicAmbiguous {
            object_number: 5,
            first: ClassicXrefAmbiguousObjectEntry {
                generation: 0,
                byte_offset: 100,
                state: ClassicXrefEntryState::InUse,
            },
            second: ClassicXrefAmbiguousObjectEntry {
                generation: 1,
                byte_offset: 200,
                state: ClassicXrefEntryState::Free,
            },
        }
    );
}

#[test]
fn locates_xref_stream_entries_without_copying_source_bytes() {
    let section = xref_stream_section(vec![
        entry(
            2,
            XrefStreamEntryRecord::Free {
                next_free_object_number: 0,
                generation: 65535,
            },
        ),
        entry(
            3,
            XrefStreamEntryRecord::Uncompressed {
                byte_offset: 99,
                generation: 4,
            },
        ),
        entry(
            4,
            XrefStreamEntryRecord::Compressed {
                object_stream_number: 10,
                index_within_object_stream: 2,
            },
        ),
        entry(
            5,
            XrefStreamEntryRecord::Reserved {
                entry_type: 7,
                field2: 8,
                field3: 9,
            },
        ),
    ]);

    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 3),
        ObjectLookupLocation::XrefStreamUncompressed {
            object_number: 3,
            generation: 4,
            byte_offset: 99,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 2),
        ObjectLookupLocation::XrefStreamFree {
            object_number: 2,
            generation: 65535,
            next_free_object_number: 0,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 4),
        ObjectLookupLocation::XrefStreamCompressed {
            object_number: 4,
            object_stream_number: 10,
            index_within_object_stream: 2,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 5),
        ObjectLookupLocation::XrefStreamReserved {
            object_number: 5,
            entry_type: 7,
            field2: 8,
            field3: 9,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 9),
        ObjectLookupLocation::XrefStreamNotFound { object_number: 9 }
    );

    let debug = format!(
        "{:?}",
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 3)
    );
    assert!(!debug.contains("DoNotCopy"));
}

#[test]
fn reports_xref_stream_generation_overflow_without_truncating() {
    let section = xref_stream_section(vec![
        entry(
            3,
            XrefStreamEntryRecord::Uncompressed {
                byte_offset: 99,
                generation: usize::from(u16::MAX) + 1,
            },
        ),
        entry(
            4,
            XrefStreamEntryRecord::Free {
                next_free_object_number: 0,
                generation: usize::from(u16::MAX) + 2,
            },
        ),
    ]);

    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 3),
        ObjectLookupLocation::XrefStreamUncompressedGenerationOutOfRange {
            object_number: 3,
            generation: 65_536,
            byte_offset: 99,
        }
    );
    assert_eq!(
        locate_xref_object(ObjectLookup::XrefStreamSection(&section), 4),
        ObjectLookupLocation::XrefStreamFreeGenerationOutOfRange {
            object_number: 4,
            generation: 65_537,
            next_free_object_number: 0,
        }
    );
}

#[test]
fn reports_xref_stream_object_number_overflow_without_truncating() {
    let oversized_object_number = usize::try_from(u32::MAX).map_or(usize::MAX, |value| value) + 1;
    let section = xref_stream_section(vec![entry(
        oversized_object_number,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 99,
            generation: 0,
        },
    )]);

    assert_eq!(
        locate_xref_object(
            ObjectLookup::XrefStreamSection(&section),
            oversized_object_number
        ),
        ObjectLookupLocation::XrefStreamObjectNumberOutOfRange {
            object_number: oversized_object_number,
        }
    );
}

#[test]
fn serde_round_trips_lookup_location_shapes() {
    let locations = [
        ObjectLookupLocation::ClassicNotFound { object_number: 9 },
        ObjectLookupLocation::XrefStreamCompressed {
            object_number: 4,
            object_stream_number: 10,
            index_within_object_stream: 2,
        },
        ObjectLookupLocation::XrefStreamReserved {
            object_number: 5,
            entry_type: 7,
            field2: 8,
            field3: 9,
        },
        ObjectLookupLocation::XrefStreamUncompressedGenerationOutOfRange {
            object_number: 3,
            generation: 65_536,
            byte_offset: 99,
        },
        ObjectLookupLocation::XrefStreamObjectNumberOutOfRange {
            object_number: usize::try_from(u32::MAX).map_or(usize::MAX, |value| value) + 1,
        },
    ];

    for location in locations {
        let value = serde_value(&location).expect("location should serialize");
        let restored: ObjectLookupLocation =
            from_serde_value(value).expect("location should deserialize");
        assert_eq!(restored, location);
    }
}
