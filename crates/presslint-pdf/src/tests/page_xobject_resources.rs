use crate::{
    ClassicXrefEntryState, ClassicXrefTableInspection, IndirectRef, PageXObjectResourceTarget,
    PdfName, SkippedPageXObjectResourceReason, inspect_classic_xref_table,
    inspect_document_page_xobject_resources,
};

struct Fixture {
    source: Vec<u8>,
    xref: ClassicXrefTableInspection,
    offsets: Vec<usize>,
    pages_offset: usize,
}

impl Fixture {
    fn object_offset(&self, object_number: usize) -> usize {
        self.offsets[object_number - 1]
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
    assert_eq!(
        xref.subsections[0].entries[2].state,
        ClassicXrefEntryState::InUse
    );
    let pages_offset = offsets[1];
    Fixture {
        source,
        xref,
        offsets,
        pages_offset,
    }
}

fn target(name: &[u8], object_number: u32, object_byte_offset: usize) -> PageXObjectResourceTarget {
    PageXObjectResourceTarget {
        name: PdfName(name.to_vec()),
        reference: IndirectRef {
            object_number,
            generation: 0,
        },
        object_byte_offset,
    }
}

#[test]
fn page_resources_report_image_and_form_targets_and_legacy_names() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm 4 0 R /Im 5 0 R >> >> /Contents 6 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
        b"5 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 >>\nstream\nx\nendstream\nendobj\n",
        b"6 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let report = inspect_document_page_xobject_resources(&pdf.source, &pdf.xref, pdf.pages_offset)
        .expect("resources should inspect");

    assert_eq!(report.page_count(), 1);
    assert_eq!(
        report.pages[0].image_xobjects,
        vec![target(b"Im", 5, pdf.object_offset(5))]
    );
    assert_eq!(
        report.pages[0].form_xobjects,
        vec![target(b"Fm", 4, pdf.object_offset(4))]
    );
    assert_eq!(
        report.pages[0].image_xobject_names,
        vec![PdfName(b"Im".to_vec())]
    );
    assert_eq!(
        report.pages[0].form_xobject_names,
        vec![PdfName(b"Fm".to_vec())]
    );
    assert!(report.pages[0].skipped.is_empty());
}

#[test]
fn inherited_resources_classify_image_and_form_targets_in_page_order() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 /Resources << /XObject << /Fm 5 0 R /Im 6 0 R >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 8 0 R >>\nendobj\n",
        b"5 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
        b"6 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 >>\nstream\nx\nendstream\nendobj\n",
        b"7 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
        b"8 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let report = inspect_document_page_xobject_resources(&pdf.source, &pdf.xref, pdf.pages_offset)
        .expect("resources should inspect");

    assert_eq!(report.page_count(), 2);
    assert_eq!(report.pages[0].ordinal, 0);
    assert_eq!(report.pages[1].ordinal, 1);
    assert_eq!(
        report.pages[0].image_xobjects,
        vec![target(b"Im", 6, pdf.object_offset(6))]
    );
    assert_eq!(
        report.pages[0].form_xobjects,
        vec![target(b"Fm", 5, pdf.object_offset(5))]
    );
    assert_eq!(
        report.pages[1].image_xobjects,
        vec![target(b"Im", 6, pdf.object_offset(6))]
    );
    assert_eq!(
        report.pages[1].form_xobjects,
        vec![target(b"Fm", 5, pdf.object_offset(5))]
    );
    assert_eq!(
        report.pages[0].image_xobject_names,
        vec![PdfName(b"Im".to_vec())]
    );
    assert_eq!(
        report.pages[0].form_xobject_names,
        vec![PdfName(b"Fm".to_vec())]
    );
    assert_eq!(
        report.pages[1].image_xobject_names,
        vec![PdfName(b"Im".to_vec())]
    );
    assert_eq!(
        report.pages[1].form_xobject_names,
        vec![PdfName(b"Fm".to_vec())]
    );
    assert!(report.pages.iter().all(|page| page.skipped.is_empty()));
}

#[test]
fn page_resources_replace_inherited_resources() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 /Resources << /XObject << /Inherited 5 0 R >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Local 6 0 R >> >> /Contents 8 0 R >>\nendobj\n",
        b"5 0 obj\n<< /Type /XObject /Subtype /Image >>\nstream\nx\nendstream\nendobj\n",
        b"6 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
        b"7 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
        b"8 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let report = inspect_document_page_xobject_resources(&pdf.source, &pdf.xref, pdf.pages_offset)
        .expect("resources should inspect");

    assert_eq!(
        report.pages[0].image_xobject_names,
        vec![PdfName(b"Inherited".to_vec())]
    );
    assert!(report.pages[0].form_xobject_names.is_empty());
    assert_eq!(
        report.pages[0].image_xobjects,
        vec![target(b"Inherited", 5, pdf.object_offset(5))]
    );
    assert!(report.pages[0].form_xobjects.is_empty());
    assert!(report.pages[1].image_xobjects.is_empty());
    assert_eq!(
        report.pages[1].form_xobjects,
        vec![target(b"Local", 6, pdf.object_offset(6))]
    );
    assert!(report.pages[1].image_xobject_names.is_empty());
    assert_eq!(
        report.pages[1].form_xobject_names,
        vec![PdfName(b"Local".to_vec())]
    );
}

#[test]
fn unknown_subtype_and_non_reference_xobjects_are_page_skips() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Ps 4 0 R /Direct << /Subtype /Image >> >> >> /Contents 5 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /XObject /Subtype /PS >>\nstream\nx\nendstream\nendobj\n",
        b"5 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
    ]);

    let report = inspect_document_page_xobject_resources(&pdf.source, &pdf.xref, pdf.pages_offset)
        .expect("resources should inspect");

    assert!(report.pages[0].image_xobject_names.is_empty());
    assert!(report.pages[0].form_xobject_names.is_empty());
    assert!(report.pages[0].image_xobjects.is_empty());
    assert!(report.pages[0].form_xobjects.is_empty());
    assert!(report.pages[0].skipped.iter().any(|skip| matches!(
        skip.reason,
        SkippedPageXObjectResourceReason::UnknownSubtype { .. }
    )));
    assert!(report.pages[0].skipped.iter().any(|skip| matches!(
        skip.reason,
        SkippedPageXObjectResourceReason::NonReferenceXObject { .. }
    )));
}

#[test]
fn duplicate_xobject_names_are_skipped_before_conflicting_classification() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Shared 4 0 R /Shared 5 0 R >> >> /Contents 6 0 R >>\nendobj\n",
        b"4 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 >>\nstream\nx\nendstream\nendobj\n",
        b"5 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
        b"6 0 obj\n<< /Length 10 >>\nstream\n/Shared Do\nendstream\nendobj\n",
    ]);

    let report = inspect_document_page_xobject_resources(&pdf.source, &pdf.xref, pdf.pages_offset)
        .expect("resources should inspect");

    assert_eq!(
        report.pages[0].image_xobject_names,
        vec![PdfName(b"Shared".to_vec())]
    );
    assert_eq!(
        report.pages[0].image_xobjects,
        vec![target(b"Shared", 4, pdf.object_offset(4))]
    );
    assert!(report.pages[0].form_xobject_names.is_empty());
    assert!(report.pages[0].form_xobjects.is_empty());
    assert_eq!(
        report.pages[0]
            .skipped
            .iter()
            .filter(|skip| matches!(
                skip.reason,
                SkippedPageXObjectResourceReason::DuplicateXObjectName { .. }
            ))
            .count(),
        1
    );
}
