use crate::{
    ClassicXrefObjectLocation, IndirectObjectHeaderByteRange,
    IndirectObjectHeaderInspectionRejection, IndirectRef, inspect_classic_xref_table,
    inspect_indirect_object_header, inspect_pdf_source, resolve_classic_xref_object,
};

#[test]
fn indirect_object_header_inspection_reports_reference_range_and_body_offset() {
    let source = b"12 3 obj\n<< /Ignored true >>\nendobj\n";

    let report = inspect_indirect_object_header(source, 0).expect("header should inspect");

    assert_eq!(
        report.reference,
        IndirectRef {
            object_number: 12,
            generation: 3,
        }
    );
    assert_eq!(report.header_byte_offset, 0);
    assert_eq!(
        report.header_range,
        IndirectObjectHeaderByteRange { start: 0, end: 8 }
    );
    assert_eq!(report.after_obj_keyword_offset, 8);
}

#[test]
fn indirect_object_header_inspection_tolerates_leading_pdf_whitespace() {
    let source = b"\0\t \r\n\x0c42 0 obj<</BodyNotParsed true>>";

    let report = inspect_indirect_object_header(source, 0).expect("header should inspect");

    assert_eq!(report.header_byte_offset, 6);
    assert_eq!(
        report.header_range,
        IndirectObjectHeaderByteRange { start: 6, end: 14 }
    );
    assert_eq!(
        report.reference,
        IndirectRef {
            object_number: 42,
            generation: 0,
        }
    );
}

#[test]
fn indirect_object_header_inspection_rejects_malformed_headers() {
    let source = b"12 obj\n<<>>";

    let error = inspect_indirect_object_header(source, 0).expect_err("bad header should reject");

    assert_eq!(
        error.reason,
        IndirectObjectHeaderInspectionRejection::MalformedHeader
    );
    assert_eq!(error.byte_offset, 0);
    assert_eq!(error.byte_len, source.len());
    assert_eq!(error.error_byte_offset, Some(3));
}

#[test]
fn indirect_object_header_inspection_rejects_obj_keyword_prefixes() {
    let source = b"12 0 object\n<<>>";

    let error = inspect_indirect_object_header(source, 0).expect_err("bad keyword should reject");

    assert_eq!(
        error.reason,
        IndirectObjectHeaderInspectionRejection::MalformedHeader
    );
    assert_eq!(error.error_byte_offset, Some(5));
}

#[test]
fn indirect_object_header_inspection_rejects_out_of_range_object_number() {
    let source = b"4294967296 0 obj\n";

    let error = inspect_indirect_object_header(source, 0).expect_err("large object should reject");

    assert_eq!(
        error.reason,
        IndirectObjectHeaderInspectionRejection::ObjectNumberOutOfRange
    );
    assert_eq!(error.error_byte_offset, Some(0));
}

#[test]
fn indirect_object_header_inspection_rejects_out_of_range_generation_number() {
    let source = b"1 65536 obj\n";

    let error =
        inspect_indirect_object_header(source, 0).expect_err("large generation should reject");

    assert_eq!(
        error.reason,
        IndirectObjectHeaderInspectionRejection::GenerationOutOfRange
    );
    assert_eq!(error.error_byte_offset, Some(2));
}

#[test]
fn indirect_object_header_inspection_rejects_offset_at_eof_and_out_of_bounds() {
    let source = b"1 0 obj\n";

    let at_eof =
        inspect_indirect_object_header(source, source.len()).expect_err("eof should reject");
    let out_of_bounds =
        inspect_indirect_object_header(source, source.len() + 1).expect_err("oob should reject");

    assert_eq!(
        at_eof.reason,
        IndirectObjectHeaderInspectionRejection::OffsetOutOfBounds
    );
    assert_eq!(
        out_of_bounds.reason,
        IndirectObjectHeaderInspectionRejection::OffsetOutOfBounds
    );
}

#[test]
fn indirect_object_header_reports_generation_mismatch_as_caller_checkable_data() {
    let source = b"9 7 obj\n<<>>\nendobj\n";
    let xref_generation = 6;

    let report = inspect_indirect_object_header(source, 0).expect("header should inspect");

    assert_eq!(report.reference.generation, 7);
    assert_ne!(report.reference.generation, xref_generation);
}

#[test]
fn indirect_object_header_composes_with_classic_xref_resolution() {
    let prefix = b"%PDF-1.7\n";
    let object = b"3 2 obj\n<< /BodyNotParsed true >>\nendobj\n";
    let object_offset = prefix.len();
    let xref_offset = prefix.len() + object.len();
    let source = format!(
        "{}{}xref\n0 4\n0000000000 65535 f \n0000000000 65535 f \n0000000000 65535 f \n{object_offset:010} 00002 n \ntrailer\n<< /Size 4 >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(object),
    )
    .into_bytes();

    let source_report = inspect_pdf_source(&source).expect("source should inspect");
    let startxref = source_report.startxref.expect("startxref should inspect");
    let xref_report =
        inspect_classic_xref_table(&source, startxref.byte_offset).expect("xref should inspect");
    let location = resolve_classic_xref_object(&xref_report, 3);
    let expected_location = ClassicXrefObjectLocation::InUse {
        object_number: 3,
        generation: 2,
        byte_offset: object_offset,
    };

    let ClassicXrefObjectLocation::InUse {
        object_number,
        generation,
        byte_offset,
    } = location
    else {
        assert_eq!(location, expected_location);
        return;
    };
    let header =
        inspect_indirect_object_header(&source, byte_offset).expect("header should inspect");

    assert_eq!(location, expected_location);
    assert_eq!(object_number, 3);
    assert_eq!(generation, 2);
    assert_eq!(
        header.reference,
        IndirectRef {
            object_number: 3,
            generation: 2,
        }
    );
    assert_eq!(header.header_byte_offset, object_offset);
    assert_eq!(
        header.after_obj_keyword_offset,
        object_offset + b"3 2 obj".len()
    );
}
