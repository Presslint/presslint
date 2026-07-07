use crate::{
    ClassicXrefTableInspection, ObjectLookup, PdfName, SkippedTransparencyGroupReason,
    TransparencyGroupColorSpace, TransparencyGroupParamClass, inspect_classic_xref_table,
    inspect_document_page_transparency_groups_with_lookup,
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
fn page_group_transparency_is_inspected_from_page_dictionary() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Group << /S /Transparency >> >>\nendobj\n",
    ]);

    let report = inspect_document_page_transparency_groups_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page group inspection should succeed");

    assert_eq!(report.page_count(), 1);
    assert!(report.pages[0].skipped.is_empty());
    assert!(report.pages[0].group.as_ref().expect("group").transparency);
}

#[test]
fn page_group_records_color_space_and_booleans() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Group << /S /Transparency /CS /DeviceCMYK /I true /K false >> >>\nendobj\n",
    ]);

    let report = inspect_document_page_transparency_groups_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page group inspection should succeed");
    let group = report.pages[0].group.as_ref().expect("group");

    assert_eq!(
        group.color_space,
        TransparencyGroupParamClass::Set {
            value: TransparencyGroupColorSpace::Name {
                raw_name: PdfName(b"DeviceCMYK".to_vec()),
            },
        }
    );
    assert_eq!(
        group.isolated,
        TransparencyGroupParamClass::Set { value: true }
    );
    assert_eq!(
        group.knockout,
        TransparencyGroupParamClass::Set { value: false }
    );
}

#[test]
fn page_group_is_not_inherited_from_pages_node() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Group << /S /Transparency >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
    ]);

    let report = inspect_document_page_transparency_groups_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page group inspection should succeed");

    assert!(report.pages[0].group.is_none());
    assert!(report.pages[0].skipped.is_empty());
}

#[test]
fn malformed_page_group_becomes_structured_diagnostic() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Group 42 >>\nendobj\n",
    ]);

    let report = inspect_document_page_transparency_groups_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page group inspection should succeed");

    assert!(report.pages[0].group.is_none());
    assert_eq!(report.pages[0].skipped.len(), 1);
    assert!(matches!(
        report.pages[0].skipped[0].reason,
        SkippedTransparencyGroupReason::NonDictionaryGroup { .. }
    ));
}
