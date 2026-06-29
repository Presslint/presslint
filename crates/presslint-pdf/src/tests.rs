#![allow(clippy::expect_used, clippy::missing_errors_doc)]

use super::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefEntry, ClassicXrefEntryState,
    ClassicXrefObjectLocation, ClassicXrefSubsection, ClassicXrefTableInspection,
    ClassicXrefTableInspectionRejection, IndirectObjectEditDisposition, IndirectObjectOwnership,
    IndirectRef, PDF_HEADER_SCAN_LIMIT, PdfSourceDiagnostic, PdfSourceRejection, PdfStartXrefIssue,
    PdfXrefSectionIssue, STARTXREF_SCAN_LIMIT, XREF_SECTION_SCAN_LIMIT, XrefSection,
    decide_indirect_object_edit, inspect_classic_xref_table, inspect_pdf_source,
    resolve_classic_xref_object,
};

fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

fn classic_entry(
    object_number: u32,
    generation: u16,
    byte_offset: usize,
    state: ClassicXrefEntryState,
) -> ClassicXrefEntry {
    ClassicXrefEntry {
        object_number,
        generation,
        byte_offset,
        state,
    }
}

fn classic_subsection(
    first_object_number: u32,
    entries: Vec<ClassicXrefEntry>,
) -> ClassicXrefSubsection {
    ClassicXrefSubsection {
        first_object_number,
        entry_count: entries
            .len()
            .try_into()
            .expect("test subsection length fits u32"),
        entries,
    }
}

fn classic_inspection(subsections: Vec<ClassicXrefSubsection>) -> ClassicXrefTableInspection {
    ClassicXrefTableInspection {
        table_byte_offset: 0,
        subsections,
        trailer_byte_offset: 0,
    }
}

#[test]
fn one_proven_consumer_allows_in_place_mutation() {
    let target = indirect_ref(10, 0);
    let owner = indirect_ref(2, 0);

    let decision = decide_indirect_object_edit(target, [owner]);

    assert_eq!(decision.target, target);
    assert_eq!(
        decision.ownership,
        IndirectObjectOwnership::ProvenSingleUse { owner }
    );
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::InPlaceMutation
    );
}

#[test]
fn multiple_proven_consumers_require_private_copy() {
    let target = indirect_ref(10, 0);
    let first = indirect_ref(2, 0);
    let second = indirect_ref(3, 0);

    let decision = decide_indirect_object_edit(target, [first, second]);

    assert_eq!(
        decision.ownership,
        IndirectObjectOwnership::Shared {
            consumers: vec![first, second],
        }
    );
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::PrivateCopy
    );
}

#[test]
fn no_proven_consumers_require_private_copy() {
    let target = indirect_ref(10, 0);

    let decision = decide_indirect_object_edit(target, []);

    assert_eq!(decision.ownership, IndirectObjectOwnership::Unproven);
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::PrivateCopy
    );
}

#[test]
fn shared_consumer_refs_are_reported_deterministically() {
    let target = indirect_ref(10, 0);
    let high_generation = indirect_ref(2, 1);
    let lowest = indirect_ref(1, 0);
    let low_generation = indirect_ref(2, 0);

    let decision =
        decide_indirect_object_edit(target, [high_generation, lowest, low_generation, lowest]);

    assert_eq!(
        decision.ownership,
        IndirectObjectOwnership::Shared {
            consumers: vec![lowest, low_generation, high_generation],
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_single_subsection_in_use_hit() {
    let inspection = classic_inspection(vec![classic_subsection(
        0,
        vec![
            classic_entry(0, 65535, 0, ClassicXrefEntryState::Free),
            classic_entry(1, 0, 42, ClassicXrefEntryState::InUse),
        ],
    )]);

    let location = resolve_classic_xref_object(&inspection, 1);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::InUse {
            object_number: 1,
            generation: 0,
            byte_offset: 42,
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_multi_subsection_in_use_hit() {
    let inspection = classic_inspection(vec![
        classic_subsection(
            0,
            vec![classic_entry(0, 65535, 0, ClassicXrefEntryState::Free)],
        ),
        classic_subsection(
            10,
            vec![
                classic_entry(10, 0, 100, ClassicXrefEntryState::InUse),
                classic_entry(11, 2, 200, ClassicXrefEntryState::InUse),
            ],
        ),
    ]);

    let location = resolve_classic_xref_object(&inspection, 11);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::InUse {
            object_number: 11,
            generation: 2,
            byte_offset: 200,
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_free_entry() {
    let inspection = classic_inspection(vec![classic_subsection(
        0,
        vec![classic_entry(0, 65535, 7, ClassicXrefEntryState::Free)],
    )]);

    let location = resolve_classic_xref_object(&inspection, 0);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::Free {
            object_number: 0,
            generation: 65535,
            next_free_object_number: 7,
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_not_found() {
    let inspection = classic_inspection(vec![classic_subsection(
        1,
        vec![classic_entry(1, 0, 42, ClassicXrefEntryState::InUse)],
    )]);

    let location = resolve_classic_xref_object(&inspection, 2);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::NotFound { object_number: 2 }
    );
}

#[test]
fn classic_xref_object_resolution_reports_duplicate_object_number_ambiguity() {
    let inspection = classic_inspection(vec![
        classic_subsection(
            5,
            vec![classic_entry(5, 0, 100, ClassicXrefEntryState::InUse)],
        ),
        classic_subsection(
            5,
            vec![classic_entry(5, 1, 200, ClassicXrefEntryState::InUse)],
        ),
    ]);

    let location = resolve_classic_xref_object(&inspection, 5);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::Ambiguous {
            object_number: 5,
            first: ClassicXrefAmbiguousObjectEntry {
                generation: 0,
                byte_offset: 100,
                state: ClassicXrefEntryState::InUse,
            },
            second: ClassicXrefAmbiguousObjectEntry {
                generation: 1,
                byte_offset: 200,
                state: ClassicXrefEntryState::InUse,
            },
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_lowest_and_highest_subsection_objects() {
    let inspection = classic_inspection(vec![classic_subsection(
        100,
        vec![
            classic_entry(100, 0, 1000, ClassicXrefEntryState::InUse),
            classic_entry(101, 0, 1001, ClassicXrefEntryState::InUse),
            classic_entry(102, 3, 1002, ClassicXrefEntryState::InUse),
        ],
    )]);

    let lowest = resolve_classic_xref_object(&inspection, 100);
    let highest = resolve_classic_xref_object(&inspection, 102);

    assert_eq!(
        lowest,
        ClassicXrefObjectLocation::InUse {
            object_number: 100,
            generation: 0,
            byte_offset: 1000,
        }
    );
    assert_eq!(
        highest,
        ClassicXrefObjectLocation::InUse {
            object_number: 102,
            generation: 3,
            byte_offset: 1002,
        }
    );
}

#[test]
fn source_inspection_detects_header_version_near_beginning() {
    let source = b"\n%PDF-1.7\n1 0 obj\n<<>>\nendobj\nstartxref\n10\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid header should inspect");

    assert_eq!(report.byte_len, source.len());
    assert_eq!(report.header.byte_offset, 1);
    assert_eq!(report.pdf_version(), (1, 7));
    assert!(report.diagnostics.is_empty());
}

#[test]
fn source_inspection_rejects_missing_header() {
    let source = b"1 0 obj\n<<>>\nendobj\nstartxref\n0\n%%EOF\n";

    let error = inspect_pdf_source(source).expect_err("missing header should reject");

    assert_eq!(error.byte_len, source.len());
    assert_eq!(
        error.reason,
        PdfSourceRejection::MissingHeader {
            searched_from: 0,
            searched_to: source.len(),
        }
    );
}

#[test]
fn source_inspection_rejects_header_outside_bounded_leading_window() {
    let mut source = vec![b' '; PDF_HEADER_SCAN_LIMIT];
    source.extend_from_slice(b"%PDF-1.7\nstartxref\n0\n%%EOF\n");

    let error = inspect_pdf_source(&source).expect_err("late header should reject");

    assert_eq!(
        error.reason,
        PdfSourceRejection::MissingHeader {
            searched_from: 0,
            searched_to: PDF_HEADER_SCAN_LIMIT,
        }
    );
}

#[test]
fn source_inspection_rejects_malformed_header_version() {
    let source = b"%PDF-1.x\nstartxref\n0\n%%EOF\n";

    let error = inspect_pdf_source(source).expect_err("malformed header should reject");

    assert_eq!(
        error.reason,
        PdfSourceRejection::MalformedHeader {
            header_byte_offset: 0,
        }
    );
}

#[test]
fn source_inspection_detects_final_startxref_offset() {
    let source = b"%PDF-1.4\nstartxref\n7\n%%EOF\nstartxref\r\n12345\r\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid trailer should inspect");
    let startxref = report
        .startxref
        .expect("final startxref should be reported");

    assert_eq!(startxref.byte_offset, 12345);
    assert_eq!(startxref.marker_byte_offset, 27);
    assert_eq!(report.xref_section, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::XrefSectionUnclassified {
            reason: PdfXrefSectionIssue::OffsetOutOfBounds,
            byte_offset: 12345,
            byte_len: source.len(),
        }]
    );
}

#[test]
fn source_inspection_reports_missing_startxref() {
    let source = b"%PDF-1.7\n1 0 obj\n<<>>\nendobj\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid header should inspect");

    assert_eq!(report.startxref, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::StartXrefUnavailable {
            reason: PdfStartXrefIssue::MissingMarker,
            searched_from: 0,
            searched_to: source.len(),
            marker_byte_offset: None,
        }]
    );
}

#[test]
fn source_inspection_reports_startxref_outside_bounded_trailing_window() {
    let mut source = b"%PDF-1.7\nstartxref\n0\n%%EOF\n".to_vec();
    source.extend(std::iter::repeat_n(b' ', STARTXREF_SCAN_LIMIT));

    let report = inspect_pdf_source(&source).expect("valid header should inspect");

    assert_eq!(report.startxref, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::StartXrefUnavailable {
            reason: PdfStartXrefIssue::MissingMarker,
            searched_from: source.len() - STARTXREF_SCAN_LIMIT,
            searched_to: source.len(),
            marker_byte_offset: None,
        }]
    );
}

#[test]
fn source_inspection_reports_malformed_startxref_offset() {
    let source = b"%PDF-1.7\nstartxref\nnot-a-number\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid header should inspect");

    assert_eq!(report.startxref, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::StartXrefUnavailable {
            reason: PdfStartXrefIssue::MissingOffset,
            searched_from: 0,
            searched_to: source.len(),
            marker_byte_offset: Some(9),
        }]
    );
}

#[test]
fn source_inspection_classifies_classic_xref_table() {
    // `%PDF-1.7\n` is 9 bytes, so the `xref` keyword begins at offset 9.
    let source = b"%PDF-1.7\nxref\nstartxref\n9\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid source should inspect");

    assert_eq!(report.xref_section, Some(XrefSection::Table));
    assert!(report.diagnostics.is_empty());
}

#[test]
fn source_inspection_classifies_xref_stream_with_object_and_generation() {
    // The indirect object header `123 7 obj` begins at offset 9.
    let source = b"%PDF-1.7\n123 7 obj\nstartxref\n9\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid source should inspect");

    assert_eq!(
        report.xref_section,
        Some(XrefSection::Stream {
            object_number: 123,
            generation: 7,
        })
    );
    assert!(report.diagnostics.is_empty());
}

#[test]
fn source_inspection_tolerates_whitespace_before_xref_section() {
    // PDF whitespace (CR, LF, spaces) precedes the `xref` keyword at offset 9.
    let source = b"%PDF-1.7\n\r\n  xref\nstartxref\n9\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid source should inspect");

    assert_eq!(report.xref_section, Some(XrefSection::Table));
    assert!(report.diagnostics.is_empty());
}

#[test]
fn source_inspection_tolerates_whitespace_before_indirect_object_header() {
    // PDF whitespace precedes the `12 5 obj` header at offset 9.
    let source = b"%PDF-1.7\n\n 12 5 obj\nstartxref\n9\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid source should inspect");

    assert_eq!(
        report.xref_section,
        Some(XrefSection::Stream {
            object_number: 12,
            generation: 5,
        })
    );
    assert!(report.diagnostics.is_empty());
}

#[test]
fn source_inspection_reports_out_of_bounds_xref_offset() {
    let source = b"%PDF-1.7\nxref\nstartxref\n9000\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid source should inspect");

    assert_eq!(report.xref_section, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::XrefSectionUnclassified {
            reason: PdfXrefSectionIssue::OffsetOutOfBounds,
            byte_offset: 9000,
            byte_len: source.len(),
        }]
    );
}

#[test]
fn source_inspection_reports_unrecognized_xref_section() {
    // Offset 9 points at `trailer`, which is neither `xref` nor `N G obj`.
    let source = b"%PDF-1.7\ntrailer<<>>startxref\n9\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid source should inspect");

    assert_eq!(report.xref_section, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::XrefSectionUnclassified {
            reason: PdfXrefSectionIssue::Unrecognized,
            byte_offset: 9,
            byte_len: source.len(),
        }]
    );
}

#[test]
fn source_inspection_reports_out_of_range_xref_stream_object_number() {
    // The object number `99999999999` does not fit `u32`.
    let source = b"%PDF-1.7\n99999999999 0 obj\nstartxref\n9\n%%EOF\n";

    let report = inspect_pdf_source(source).expect("valid source should inspect");

    assert_eq!(report.xref_section, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::XrefSectionUnclassified {
            reason: PdfXrefSectionIssue::ObjectNumberOutOfRange,
            byte_offset: 9,
            byte_len: source.len(),
        }]
    );
}

#[test]
fn source_inspection_keeps_xref_classification_window_bounded() {
    // The `xref` keyword sits past the bounded classification window, so the
    // section must read as unrecognized rather than triggering a wider scan.
    let mut source = b"%PDF-1.7\n".to_vec();
    source.extend(std::iter::repeat_n(b' ', XREF_SECTION_SCAN_LIMIT + 8));
    source.extend_from_slice(b"xref\nstartxref\n9\n%%EOF\n");

    let report = inspect_pdf_source(&source).expect("valid source should inspect");

    assert_eq!(report.xref_section, None);
    assert_eq!(
        report.diagnostics,
        vec![PdfSourceDiagnostic::XrefSectionUnclassified {
            reason: PdfXrefSectionIssue::Unrecognized,
            byte_offset: 9,
            byte_len: source.len(),
        }]
    );
}

#[test]
fn classic_xref_table_inspection_reports_one_subsection() {
    let source =
        b"%PDF-1.7\nxref\n0 2\n0000000000 65535 f \n0000000017 00000 n \ntrailer\n<< /Size 2 >>\n";

    let report = inspect_classic_xref_table(source, 9).expect("xref table should inspect");

    assert_eq!(report.table_byte_offset, 9);
    assert_eq!(report.trailer_byte_offset, 58);
    assert_eq!(
        report.subsections,
        vec![ClassicXrefSubsection {
            first_object_number: 0,
            entry_count: 2,
            entries: vec![
                ClassicXrefEntry {
                    object_number: 0,
                    generation: 65535,
                    byte_offset: 0,
                    state: ClassicXrefEntryState::Free,
                },
                ClassicXrefEntry {
                    object_number: 1,
                    generation: 0,
                    byte_offset: 17,
                    state: ClassicXrefEntryState::InUse,
                },
            ],
        }]
    );
}

#[test]
fn classic_xref_table_inspection_reports_multiple_subsections_in_source_order() {
    let source = b"xref\r\n0 1\r\n0000000000 65535 f \r\n5 2\r\n0000000100 00000 n \r\n0000000200 00002 n \r\ntrailer\r\n<<>>";

    let report = inspect_classic_xref_table(source, 0).expect("xref table should inspect");

    assert_eq!(report.trailer_byte_offset, 79);
    assert_eq!(report.subsections.len(), 2);
    assert_eq!(report.subsections[0].first_object_number, 0);
    assert_eq!(report.subsections[0].entries[0].object_number, 0);
    assert_eq!(report.subsections[1].first_object_number, 5);
    assert_eq!(
        report.subsections[1].entries,
        vec![
            ClassicXrefEntry {
                object_number: 5,
                generation: 0,
                byte_offset: 100,
                state: ClassicXrefEntryState::InUse,
            },
            ClassicXrefEntry {
                object_number: 6,
                generation: 2,
                byte_offset: 200,
                state: ClassicXrefEntryState::InUse,
            },
        ]
    );
}

#[test]
fn classic_xref_table_inspection_tolerates_pdf_whitespace() {
    let source = b"\0\t \rxref\n0\t1\x0c\n0000000000 65535 f \r\n \t\ntrailer\n<< /Ignored true >>";

    let report = inspect_classic_xref_table(source, 0).expect("xref table should inspect");

    assert_eq!(report.table_byte_offset, 4);
    assert_eq!(report.trailer_byte_offset, 38);
    assert_eq!(
        report.subsections[0].entries[0].state,
        ClassicXrefEntryState::Free
    );
}

#[test]
fn classic_xref_table_inspection_stops_before_trailer_dictionary_and_body_bytes() {
    let source =
        b"xref\n1 1\n0000000042 00000 n \ntrailer\n<< /Size 2 /Prev not-parsed /Root 9 0 R >>\n1 0 obj\n<< /NotParsed true >>\nendobj\n";

    let report = inspect_classic_xref_table(source, 0).expect("xref table should inspect");

    assert_eq!(report.trailer_byte_offset, 29);
    assert_eq!(report.subsections.len(), 1);
    assert_eq!(report.subsections[0].entries.len(), 1);

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("/Prev"));
    assert!(!debug_report.contains("NotParsed"));
    assert!(!debug_report.contains("endobj"));
}

#[test]
fn classic_xref_table_inspection_accepts_trailer_keyword_at_eof() {
    let source = b"xref\n0 1\n0000000000 65535 f \ntrailer";

    let report = inspect_classic_xref_table(source, 0).expect("eof trailer should inspect");

    assert_eq!(report.trailer_byte_offset, 29);
    assert_eq!(report.subsections.len(), 1);
    assert_eq!(report.subsections[0].entry_count, 1);
}

#[test]
fn classic_xref_table_inspection_rejects_non_table_offset() {
    let source = b"1 0 obj\n<<>>\nendobj\n";

    let error = inspect_classic_xref_table(source, 0).expect_err("object body is not xref table");

    assert_eq!(error.byte_offset, 0);
    assert_eq!(error.byte_len, source.len());
    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::NotXrefTable
    );
}

#[test]
fn classic_xref_table_inspection_rejects_malformed_subsection_header() {
    let source = b"xref\n0 nope\n0000000000 65535 f \ntrailer\n<<>>";

    let error = inspect_classic_xref_table(source, 0).expect_err("bad header should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::MalformedSubsectionHeader
    );
    assert_eq!(error.error_byte_offset, Some(5));
    assert_eq!(error.object_number, None);
}

#[test]
fn classic_xref_table_inspection_rejects_malformed_entry() {
    let source = b"xref\n0 1\n0000000000 65535 x \ntrailer\n<<>>";

    let error = inspect_classic_xref_table(source, 0).expect_err("bad entry should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::MalformedEntry
    );
    assert_eq!(error.error_byte_offset, Some(9));
    assert_eq!(error.object_number, Some(0));
}

#[test]
fn classic_xref_table_inspection_rejects_entry_without_trailing_separator() {
    let source = b"xref\n0 1\n0000000000 65535 f\ntrailer\n<<>>";

    let error = inspect_classic_xref_table(source, 0).expect_err("short entry should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::MalformedEntry
    );
    assert_eq!(error.error_byte_offset, Some(9));
    assert_eq!(error.object_number, Some(0));
}

#[test]
fn classic_xref_table_inspection_rejects_short_entry_before_blank_line() {
    let source = b"xref\n0 1\n0000000000 65535 f\n \ntrailer\n<<>>";

    let error = inspect_classic_xref_table(source, 0).expect_err("short entry should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::MalformedEntry
    );
    assert_eq!(error.error_byte_offset, Some(9));
    assert_eq!(error.object_number, Some(0));
}

#[test]
fn classic_xref_table_inspection_rejects_out_of_range_subsection_object_number() {
    let source = b"xref\n99999999999 1\n0000000000 65535 f \ntrailer\n<<>>";

    let error = inspect_classic_xref_table(source, 0).expect_err("large object should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::SubsectionObjectNumberOutOfRange
    );
    assert_eq!(error.error_byte_offset, Some(5));
    assert_eq!(error.object_number, None);
}

#[test]
fn classic_xref_table_inspection_rejects_out_of_range_generation_number() {
    let source = b"xref\n0 1\n0000000000 99999 f \ntrailer\n<<>>";

    let error = inspect_classic_xref_table(source, 0).expect_err("large generation should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::EntryGenerationOutOfRange
    );
    assert_eq!(error.error_byte_offset, Some(9));
    assert_eq!(error.object_number, Some(0));
}

#[test]
fn classic_xref_table_inspection_rejects_missing_trailer() {
    let source = b"xref\n0 1\n0000000000 65535 f \n";

    let error = inspect_classic_xref_table(source, 0).expect_err("missing trailer should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::MissingTrailer
    );
    assert_eq!(error.error_byte_offset, Some(source.len()));
    assert_eq!(error.object_number, None);
}

#[test]
fn classic_xref_table_inspection_reports_bare_xref_as_missing_trailer() {
    let source = b"xref";

    let error = inspect_classic_xref_table(source, 0).expect_err("bare xref should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::MissingTrailer
    );
    assert_eq!(error.error_byte_offset, Some(source.len()));
}

#[test]
fn classic_xref_table_inspection_reports_whitespace_only_xref_as_missing_trailer() {
    let source = b"xref \t\r\n\x0c";

    let error =
        inspect_classic_xref_table(source, 0).expect_err("xref without trailer should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTableInspectionRejection::MissingTrailer
    );
    assert_eq!(error.error_byte_offset, Some(source.len()));
}
