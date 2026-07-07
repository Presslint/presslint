use crate::{
    DictionaryValueKind, PdfOutputIntentSubtype, SkippedOutputIntentReason,
    inspect_document_output_intents,
};

fn classic_pdf(objects: &[&[u8]]) -> Vec<u8> {
    let mut source = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for object in objects {
        offsets.push(source.len());
        source.extend_from_slice(object);
    }

    let xref_offset = source.len();
    let object_count = objects.len() + 1;
    source.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
    source.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    source.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );
    source
}

#[test]
fn output_intents_absent_is_empty_success() {
    let pdf = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ]);

    let report = inspect_document_output_intents(&pdf).expect("output intents should inspect");

    assert!(report.output_intents.is_empty());
    assert!(report.skipped.is_empty());
}

#[test]
fn output_intents_direct_dictionary_is_classified() {
    let pdf = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OutputIntents [ << /Type /OutputIntent /S /GTS_PDFX /OutputConditionIdentifier (CGATS TR 001) >> ] >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ]);

    let report = inspect_document_output_intents(&pdf).expect("output intents should inspect");

    assert!(report.skipped.is_empty());
    assert_eq!(report.output_intents.len(), 1);
    assert_eq!(
        report.output_intents[0].subtype,
        PdfOutputIntentSubtype::GtsPdfx
    );
    assert_eq!(
        report.output_intents[0].output_condition_identifier,
        "CGATS TR 001"
    );
    assert!(!report.output_intents[0].dest_output_profile.present);
}

#[test]
fn output_intents_indirect_dictionary_and_dest_profile_reference_are_classified() {
    let pdf = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OutputIntents [ 4 0 R ] >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier <464f4f> /DestOutputProfile 5 0 R >>\nendobj\n",
        b"5 0 obj\n<< /N 4 /Length 12 >>\nstream\nprofilebytes\nendstream\nendobj\n",
    ]);

    let report = inspect_document_output_intents(&pdf).expect("output intents should inspect");

    assert!(report.skipped.is_empty());
    assert_eq!(
        report.output_intents[0].subtype,
        PdfOutputIntentSubtype::GtsPdfa1
    );
    assert_eq!(report.output_intents[0].output_condition_identifier, "FOO");
    assert!(report.output_intents[0].dest_output_profile.present);
    assert_eq!(
        report.output_intents[0].dest_output_profile.value_kind,
        Some(DictionaryValueKind::IndirectReferenceLike)
    );
    assert_eq!(
        report.output_intents[0]
            .dest_output_profile
            .reference
            .expect("profile reference")
            .object_number,
        5
    );
    assert!(!format!("{report:?}").contains("profilebytes"));
}

#[test]
fn output_intents_unsupported_subtype_is_structured_skip() {
    let pdf = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OutputIntents [ << /S /Foo /OutputConditionIdentifier (FOO) >> ] >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ]);

    let report = inspect_document_output_intents(&pdf).expect("output intents should inspect");

    assert!(report.output_intents.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedOutputIntentReason::UnsupportedSubtype { .. }
    ));
}

#[test]
fn output_intents_non_array_is_structured_skip() {
    let pdf = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OutputIntents << /S /GTS_PDFX >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ]);

    let report = inspect_document_output_intents(&pdf).expect("output intents should inspect");

    assert!(report.output_intents.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedOutputIntentReason::NonArrayOutputIntents { .. }
    ));
}

#[test]
fn output_intents_malformed_entry_is_structured_skip() {
    let pdf = classic_pdf(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /OutputIntents [ /NotADictionary ] >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ]);

    let report = inspect_document_output_intents(&pdf).expect("output intents should inspect");

    assert!(report.output_intents.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedOutputIntentReason::NonDictionaryOutputIntent { .. }
    ));
}
