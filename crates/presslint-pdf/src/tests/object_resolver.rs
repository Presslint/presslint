#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use super::{classic_entry, classic_inspection, classic_subsection, indirect_ref};

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ClassicXrefEntryState, ClassicXrefTableInspection, IndirectObjectHeaderInspectionRejection,
    ObjectLookup, ObjectLookupLocation, ObjectResolutionError, ObjectResolutionRejection,
    ResolvedObject, XrefStreamEntry, XrefStreamEntryRecord, XrefStreamSection,
    XrefStreamSubsection, resolve_classic_xref_object_offset, resolve_xref_object_offset,
};

/// Single in-use object body whose header is `3 0 obj` at offset zero.
fn page_object_source() -> Vec<u8> {
    b"3 0 obj\n<< /Type /Page >>\nendobj\n".to_vec()
}

/// Classic xref table with one in-use entry for object `3 0` at offset zero.
/// Most resolver tests share this fixture and vary only the source bytes or the
/// requested reference.
fn in_use_object_3_xref() -> ClassicXrefTableInspection {
    classic_inspection(vec![classic_subsection(
        3,
        vec![classic_entry(3, 0, 0, ClassicXrefEntryState::InUse)],
    )])
}

fn xref_stream_section(entries: Vec<XrefStreamEntry>) -> XrefStreamSection {
    XrefStreamSection {
        object_byte_offset: 200,
        widths: [1, 4, 2],
        size: 20,
        index_subsections: vec![XrefStreamSubsection {
            first_object_number: 0,
            entry_count: 20,
        }],
        root_reference: indirect_ref(1, 0),
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
fn resolves_unique_in_use_entry_with_matching_header() {
    let source = page_object_source();
    let xref = in_use_object_3_xref();

    let resolved = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 0))
        .expect("unique in-use entry with matching header should resolve");

    assert_eq!(
        resolved,
        ResolvedObject {
            reference: indirect_ref(3, 0),
            object_byte_offset: 0,
            xref_generation: 0,
        }
    );
}

#[test]
fn rejects_non_in_use_xref_location() {
    let source = page_object_source();
    let xref = classic_inspection(vec![classic_subsection(
        3,
        vec![classic_entry(3, 0, 7, ClassicXrefEntryState::Free)],
    )]);

    let error = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 0))
        .expect_err("a free entry must not resolve");

    assert_eq!(error.reference, indirect_ref(3, 0));
    assert_eq!(error.object_byte_offset, None);
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::UnresolvedXrefLocation {
            location: ObjectLookupLocation::ClassicFree {
                object_number: 3,
                generation: 0,
                next_free_object_number: 7,
            },
        }
    );
}

#[test]
fn rejects_not_found_object_number() {
    let source = page_object_source();
    let xref = in_use_object_3_xref();

    let error = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(9, 0))
        .expect_err("a missing object number must not resolve");

    assert_eq!(
        error.reason,
        ObjectResolutionRejection::UnresolvedXrefLocation {
            location: ObjectLookupLocation::ClassicNotFound { object_number: 9 },
        }
    );
}

#[test]
fn resolves_xref_stream_uncompressed_entry_with_matching_header() {
    let source = page_object_source();
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 0,
            generation: 0,
        },
    )]);

    let resolved = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect("xref-stream type-1 entry with matching header should resolve");

    assert_eq!(
        resolved,
        ResolvedObject {
            reference: indirect_ref(3, 0),
            object_byte_offset: 0,
            xref_generation: 0,
        }
    );
}

#[test]
fn rejects_xref_stream_free_and_missing_entries() {
    let source = page_object_source();
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Free {
            next_free_object_number: 0,
            generation: 65535,
        },
    )]);

    let free_error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("xref-stream free entry must not resolve");
    assert_eq!(
        free_error.reason,
        ObjectResolutionRejection::UnresolvedXrefLocation {
            location: ObjectLookupLocation::XrefStreamFree {
                object_number: 3,
                generation: 65535,
                next_free_object_number: 0,
            },
        }
    );

    let missing_error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(9, 0),
    )
    .expect_err("missing xref-stream object must not resolve");
    assert_eq!(
        missing_error.reason,
        ObjectResolutionRejection::UnresolvedXrefLocation {
            location: ObjectLookupLocation::XrefStreamNotFound { object_number: 9 },
        }
    );
}

#[test]
fn rejects_xref_stream_compressed_entry_as_unsupported() {
    let source = page_object_source();
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 10,
            index_within_object_stream: 2,
        },
    )]);

    let error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("xref-stream type-2 entry must not resolve through this helper");

    assert_eq!(error.object_byte_offset, None);
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::UnsupportedCompressedXrefStreamEntry {
            object_number: 3,
            object_stream_number: 10,
            index_within_object_stream: 2,
        }
    );
}

#[test]
fn rejects_xref_stream_reserved_entry_as_unsupported() {
    let source = page_object_source();
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Reserved {
            entry_type: 7,
            field2: 8,
            field3: 9,
        },
    )]);

    let error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("reserved xref-stream entry must not fabricate a byte offset");

    assert_eq!(error.object_byte_offset, None);
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::UnsupportedReservedXrefStreamEntry {
            object_number: 3,
            entry_type: 7,
            field2: 8,
            field3: 9,
        }
    );
}

#[test]
fn rejects_xref_stream_generation_mismatch_before_header_validation() {
    let source = b"3 0 obj\n<< /Type /Page >>\nendobj\n".to_vec();
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 0,
            generation: 4,
        },
    )]);

    let error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("xref-stream generation mismatch must not resolve");

    assert_eq!(error.object_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::GenerationMismatch {
            requested_generation: 0,
            xref_generation: 4,
        }
    );
}

#[test]
fn rejects_xref_stream_generation_out_of_range_structurally() {
    let source = page_object_source();
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 0,
            generation: usize::from(u16::MAX) + 1,
        },
    )]);

    let error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("oversized xref-stream generation must not truncate");

    assert_eq!(
        error.reason,
        ObjectResolutionRejection::UnresolvedXrefLocation {
            location: ObjectLookupLocation::XrefStreamUncompressedGenerationOutOfRange {
                object_number: 3,
                generation: 65_536,
                byte_offset: 0,
            },
        }
    );
}

#[test]
fn rejects_xref_stream_malformed_and_mismatched_headers() {
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 0,
            generation: 0,
        },
    )]);

    let malformed = resolve_xref_object_offset(
        b"<< not an object header >>",
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("malformed header must not resolve");
    assert_eq!(
        malformed.reason,
        ObjectResolutionRejection::ObjectHeader {
            header_reason: IndirectObjectHeaderInspectionRejection::MalformedHeader,
        }
    );

    let mismatch = resolve_xref_object_offset(
        b"9 0 obj\n<< /Type /Page >>\nendobj\n",
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("header reference mismatch must not resolve");
    assert_eq!(
        mismatch.reason,
        ObjectResolutionRejection::ObjectHeaderReferenceMismatch {
            header_reference: indirect_ref(9, 0),
        }
    );
}

#[test]
fn rejects_xref_generation_mismatch() {
    let source = page_object_source();
    let xref = in_use_object_3_xref();

    let error = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 4))
        .expect_err("xref generation mismatch must not resolve");

    assert_eq!(error.object_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::GenerationMismatch {
            requested_generation: 4,
            xref_generation: 0,
        }
    );
}

#[test]
fn rejects_malformed_object_header_at_resolved_offset() {
    let source = b"<< not an object header >>".to_vec();
    let xref = in_use_object_3_xref();

    let error = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 0))
        .expect_err("a non-header offset must not resolve");

    assert_eq!(error.object_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::ObjectHeader {
            header_reason: IndirectObjectHeaderInspectionRejection::MalformedHeader,
        }
    );
}

#[test]
fn rejects_object_header_object_number_mismatch() {
    // The xref entry claims object 3, but the header at the offset is `9 0 obj`.
    let source = b"9 0 obj\n<< /Type /Page >>\nendobj\n".to_vec();
    let xref = in_use_object_3_xref();

    let error = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 0))
        .expect_err("object-number mismatch at the header must not resolve");

    assert_eq!(error.object_byte_offset, Some(0));
    assert_eq!(error.error_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::ObjectHeaderReferenceMismatch {
            header_reference: indirect_ref(9, 0),
        }
    );
}

#[test]
fn rejects_object_header_generation_mismatch() {
    // The xref entry generation matches the request, but the header generation
    // does not. This is the second of the two generation checks.
    let source = b"3 2 obj\n<< /Type /Page >>\nendobj\n".to_vec();
    let xref = in_use_object_3_xref();

    let error = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 0))
        .expect_err("generation mismatch at the header must not resolve");

    assert_eq!(
        error.reason,
        ObjectResolutionRejection::ObjectHeaderReferenceMismatch {
            header_reference: indirect_ref(3, 2),
        }
    );
}

#[test]
fn report_retains_no_source_bytes() {
    let source = b"3 0 obj\n<< /Type /Page /DoNotCopy (secret) >>\nendobj\n".to_vec();
    let xref = in_use_object_3_xref();

    let resolved = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 0))
        .expect("object should resolve");
    let debug = format!("{resolved:?}");

    assert!(!debug.contains("DoNotCopy"));
    assert!(!debug.contains("secret"));
}

#[test]
fn xref_stream_report_retains_no_source_bytes() {
    let source = b"3 0 obj\n<< /Type /Page /DoNotCopy (secret) >>\nendobj\n".to_vec();
    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 0,
            generation: 0,
        },
    )]);

    let resolved = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect("object should resolve");
    let debug = format!("{resolved:?}");

    assert!(!debug.contains("DoNotCopy"));
    assert!(!debug.contains("secret"));
}

#[test]
fn serde_round_trips_resolved_object_and_error_shapes() {
    let source = page_object_source();
    let xref = in_use_object_3_xref();

    let resolved = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 0))
        .expect("object should resolve");
    let value = serde_value(&resolved).expect("resolved object should serialize");
    let restored: ResolvedObject =
        from_serde_value(value).expect("resolved object should deserialize");
    assert_eq!(restored, resolved);

    let error = resolve_classic_xref_object_offset(&source, &xref, indirect_ref(3, 4))
        .expect_err("generation mismatch should reject");
    let error_value = serde_value(&error).expect("error should serialize");
    let restored_error: ObjectResolutionError =
        from_serde_value(error_value).expect("error should deserialize");
    assert_eq!(restored_error, error);

    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 10,
            index_within_object_stream: 2,
        },
    )]);
    let compressed_error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("compressed entry should reject");
    let compressed_value =
        serde_value(&compressed_error).expect("compressed error should serialize");
    let restored_compressed_error: ObjectResolutionError =
        from_serde_value(compressed_value).expect("compressed error should deserialize");
    assert_eq!(restored_compressed_error, compressed_error);

    let section = xref_stream_section(vec![entry(
        3,
        XrefStreamEntryRecord::Reserved {
            entry_type: 7,
            field2: 8,
            field3: 9,
        },
    )]);
    let reserved_error = resolve_xref_object_offset(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("reserved entry should reject");
    let reserved_value = serde_value(&reserved_error).expect("reserved error should serialize");
    let restored_reserved_error: ObjectResolutionError =
        from_serde_value(reserved_value).expect("reserved error should deserialize");
    assert_eq!(restored_reserved_error, reserved_error);
}
