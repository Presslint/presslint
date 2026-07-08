use crate::{
    ColorSpaceFamily, PdfName, inspect_classic_xref_table,
    inspect_document_page_color_space_resources,
};

struct Fixture {
    source: Vec<u8>,
    xref_offset: usize,
    offsets: Vec<usize>,
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

    Fixture {
        source,
        xref_offset,
        offsets,
    }
}

fn page_color_space_pdf(color_space: &'static [u8], profile: &'static [u8]) -> Fixture {
    fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        color_space,
        profile,
    ])
}

#[test]
fn page_icc_based_records_profile_range_arity_and_absent_alternate() {
    let pdf = page_color_space_pdf(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /CS0 [ /ICCBased 4 0 R ] >> >> >>\nendobj\n",
        b"4 0 obj\n<< /N 4 /Range [ 0 1 0 1 0 1 0 1 ] /Length 1 >>\nstream\nx\nendstream\nendobj\n",
    );
    let xref = inspect_classic_xref_table(&pdf.source, pdf.xref_offset).expect("xref");

    let report =
        inspect_document_page_color_space_resources(&pdf.source, &xref, pdf.object_offset(2))
            .expect("page color spaces should inspect");

    assert!(report.pages[0].skipped.is_empty());
    let cs0 = &report.pages[0].color_spaces[0];
    assert_eq!(cs0.name, PdfName(b"CS0".to_vec()));
    assert_eq!(cs0.family, ColorSpaceFamily::IccBased);
    assert_eq!(cs0.component_count, Some(4));
    assert_eq!(
        cs0.icc_profile_stream,
        Some(crate::tests::indirect_ref(4, 0))
    );
    assert_eq!(cs0.icc_range_entry_count, Some(8));
    assert_eq!(cs0.icc_alternate_present, Some(false));
}

#[test]
fn page_icc_based_records_alternate_presence_and_family() {
    let pdf = page_color_space_pdf(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /CS0 [ /ICCBased 4 0 R ] >> >> >>\nendobj\n",
        b"4 0 obj\n<< /N 4 /Alternate /DeviceCMYK /Length 1 >>\nstream\nx\nendstream\nendobj\n",
    );
    let xref = inspect_classic_xref_table(&pdf.source, pdf.xref_offset).expect("xref");

    let report =
        inspect_document_page_color_space_resources(&pdf.source, &xref, pdf.object_offset(2))
            .expect("page color spaces should inspect");

    let cs0 = &report.pages[0].color_spaces[0];
    assert_eq!(cs0.alternate_space, Some(ColorSpaceFamily::DeviceCmyk));
    assert_eq!(cs0.icc_alternate_present, Some(true));
}

#[test]
fn page_icc_based_non_direct_range_has_no_range_count() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /CS0 [ /ICCBased 4 0 R ] /CS1 [ /ICCBased 5 0 R ] >> >> >>\nendobj\n",
        b"4 0 obj\n<< /N 3 /Range 6 0 R /Length 1 >>\nstream\nx\nendstream\nendobj\n",
        b"5 0 obj\n<< /N 3 /Range /BadRange /Length 1 >>\nstream\nx\nendstream\nendobj\n",
        b"6 0 obj\n[ 0 1 0 1 0 1 ]\nendobj\n",
    ]);
    let xref = inspect_classic_xref_table(&pdf.source, pdf.xref_offset).expect("xref");

    let report =
        inspect_document_page_color_space_resources(&pdf.source, &xref, pdf.object_offset(2))
            .expect("page color spaces should inspect");

    assert!(report.pages[0].skipped.is_empty());
    assert_eq!(report.pages[0].color_spaces[0].icc_range_entry_count, None);
    assert_eq!(report.pages[0].color_spaces[1].icc_range_entry_count, None);
}
