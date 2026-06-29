#![allow(clippy::expect_used, clippy::missing_errors_doc)]

use super::{
    IndirectObjectEditDisposition, IndirectObjectOwnership, IndirectRef, PDF_HEADER_SCAN_LIMIT,
    PdfSourceDiagnostic, PdfSourceRejection, PdfStartXrefIssue, STARTXREF_SCAN_LIMIT,
    decide_indirect_object_edit, inspect_pdf_source,
};

fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
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
fn source_inspection_detects_header_version_near_beginning() {
    let source = b"\n%PDF-1.7\n1 0 obj\n<<>>\nendobj\nstartxref\n42\n%%EOF\n";

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
    assert!(report.diagnostics.is_empty());
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
