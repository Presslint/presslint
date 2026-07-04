use crate::{
    ClassicXrefTableInspection, ColorSpaceFamily, ObjectLookup, PdfName,
    SkippedColorSpaceResourceReason, inspect_classic_xref_table,
    inspect_form_color_space_resources,
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
fn form_own_color_spaces_classify_icc_and_separation() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /CS0 [ /ICCBased 2 0 R ] /CS1 [ /Separation /PANTONE /DeviceCMYK 3 0 R ] >> >> >>\nstream\n\nendstream\nendobj\n",
        b"2 0 obj\n<< /N 4 /Length 1 >>\nstream\nx\nendstream\nendobj\n",
        b"3 0 obj\n<< /FunctionType 2 /Domain [ 0 1 ] /N 1 /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report =
        inspect_form_color_space_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert_eq!(report.object_byte_offset, pdf.object_offset(1));
    assert!(report.skipped.is_empty());
    // Sorted/deduplicated by name: CS0 then CS1.
    assert_eq!(report.color_spaces.len(), 2);

    let cs0 = &report.color_spaces[0];
    assert_eq!(cs0.name, PdfName(b"CS0".to_vec()));
    assert_eq!(cs0.family, ColorSpaceFamily::IccBased);
    assert_eq!(cs0.component_count, Some(4));
    assert!(cs0.spot_names.is_empty());

    let cs1 = &report.color_spaces[1];
    assert_eq!(cs1.name, PdfName(b"CS1".to_vec()));
    assert_eq!(cs1.family, ColorSpaceFamily::Separation);
    assert_eq!(cs1.component_count, Some(1));
    assert_eq!(cs1.spot_names, vec![PdfName(b"PANTONE".to_vec())]);
    assert_eq!(cs1.alternate_space, Some(ColorSpaceFamily::DeviceCmyk));
}

#[test]
fn form_without_resources_reports_missing_color_space_resources() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report =
        inspect_form_color_space_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    // No page borrow: an absent `/Resources` yields an EMPTY environment, never
    // the invoking page's colour spaces.
    assert!(report.color_spaces.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedColorSpaceResourceReason::MissingColorSpaceResources
    ));
}

#[test]
fn form_resources_without_color_space_report_missing_color_space() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /XObject << /Fm 2 0 R >> >> >>\nstream\n\nendstream\nendobj\n",
        b"2 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report =
        inspect_form_color_space_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.color_spaces.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedColorSpaceResourceReason::MissingColorSpace
    ));
}

#[test]
fn form_unresolved_icc_reference_is_a_structured_skip() {
    // `/CS0` points its ICC stream at object 99, which the xref does not carry.
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /CS0 [ /ICCBased 99 0 R ] >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report =
        inspect_form_color_space_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.color_spaces.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        report.skipped[0].resource_name,
        Some(PdfName(b"CS0".to_vec()))
    );
    assert!(matches!(
        report.skipped[0].reason,
        SkippedColorSpaceResourceReason::UnresolvedResourceReference { .. }
    ));
}

#[test]
fn form_unknown_color_space_name_is_a_structured_skip() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /CS0 /FooBar >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report =
        inspect_form_color_space_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.color_spaces.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedColorSpaceResourceReason::UnknownColorSpaceName { .. }
    ));
}

#[test]
fn form_color_space_report_retains_no_source_bytes() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 13 /Resources << /ColorSpace << /CS0 [ /ICCBased 2 0 R ] >> >> >>\nstream\nsecret-marks!\nendstream\nendobj\n",
        b"2 0 obj\n<< /N 3 /Length 1 >>\nstream\nx\nendstream\nendobj\n",
    ]);

    let report =
        inspect_form_color_space_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("secret-marks"));
}
