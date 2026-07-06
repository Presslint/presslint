use crate::{
    ClassicXrefTableInspection, ExtGStateBlendMode, ExtGStateOverprintMode, ExtGStateParamClass,
    ObjectLookup, PdfName, SkippedExtGStateResourceReason, inspect_classic_xref_table,
    inspect_document_page_extgstate_resources_with_lookup,
};

struct Fixture {
    source: Vec<u8>,
    xref: ClassicXrefTableInspection,
    offsets: Vec<usize>,
}

impl Fixture {
    fn object_offset(&self, object_number: usize) -> usize {
        self.offsets[object_number - 1]
    }

    fn lookup(&self) -> ObjectLookup<'_> {
        ObjectLookup::ClassicXref(&self.xref)
    }
}

fn fixture(objects: &[&[u8]]) -> Fixture {
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
    for offset in &offsets {
        source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    source.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );

    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    Fixture {
        source,
        xref,
        offsets,
    }
}

#[test]
fn page_classifies_op_true_and_opm_one() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /ExtGState << /GS0 << /OP true /OPM 1 >> >> >> >>\nendobj\n",
    ]);

    let report = inspect_document_page_extgstate_resources_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page ExtGState resources should inspect");

    assert_eq!(report.page_count(), 1);
    assert!(report.pages[0].skipped.is_empty());
    let gs = &report.pages[0].extgstates[0];
    assert_eq!(gs.op_stroking, ExtGStateParamClass::Set { value: true });
    assert_eq!(
        gs.overprint_mode,
        ExtGStateParamClass::Set {
            value: ExtGStateOverprintMode::One,
        }
    );
    assert_eq!(gs.op_nonstroking, ExtGStateParamClass::Unset);
}

#[test]
fn unresolved_indirect_extgstate_entry_is_structured_skip() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /ExtGState << /GS0 99 0 R >> >> >>\nendobj\n",
    ]);

    let report = inspect_document_page_extgstate_resources_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page ExtGState resources should inspect");

    assert!(report.pages[0].extgstates.is_empty());
    assert_eq!(report.pages[0].skipped.len(), 1);
    assert_eq!(
        report.pages[0].skipped[0].resource_name,
        Some(PdfName(b"GS0".to_vec()))
    );
    assert!(matches!(
        report.pages[0].skipped[0].reason,
        SkippedExtGStateResourceReason::UnresolvedResourceReference { .. }
    ));
}

#[test]
fn duplicate_extgstate_resource_name_is_structured_skip() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /ExtGState << /GS0 << /BM /Normal >> /GS0 << /BM /Multiply >> >> >> >>\nendobj\n",
    ]);

    let report = inspect_document_page_extgstate_resources_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page ExtGState resources should inspect");

    assert_eq!(report.pages[0].extgstates.len(), 1);
    assert_eq!(report.pages[0].skipped.len(), 1);
    assert!(matches!(
        report.pages[0].skipped[0].reason,
        SkippedExtGStateResourceReason::DuplicateExtGStateName { .. }
    ));
}

#[test]
fn page_inherits_extgstate_from_ancestor_resources() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /ExtGState << /GS0 << /BM /Multiply >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
    ]);

    let report = inspect_document_page_extgstate_resources_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page ExtGState resources should inspect");

    assert!(report.pages[0].skipped.is_empty());
    assert_eq!(report.pages[0].extgstates.len(), 1);
    assert_eq!(
        report.pages[0].extgstates[0].blend_mode,
        ExtGStateParamClass::Set {
            value: ExtGStateBlendMode::NonNormal {
                raw_name: PdfName(b"Multiply".to_vec()),
            },
        }
    );
}
