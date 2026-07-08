use crate::{
    ColorSpaceFamily, DefaultColorSpaceKind, ObjectLookup, PdfName,
    SkippedColorSpaceResourceReason, SkippedDefaultColorSpaceReason, inspect_classic_xref_table,
    inspect_document_page_default_color_spaces, inspect_form_default_color_spaces,
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

    fn xref(&self) -> crate::ClassicXrefTableInspection {
        inspect_classic_xref_table(&self.source, self.xref_offset).expect("xref should inspect")
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

#[test]
fn default_color_spaces_page_inherits_default_cmyk() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /Resources << /ColorSpace << /DefaultCMYK /DeviceCMYK >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report =
        inspect_document_page_default_color_spaces(&pdf.source, &xref, pdf.object_offset(2))
            .expect("page defaults should inspect");

    assert_eq!(report.pages.len(), 1);
    assert!(report.pages[0].skipped.is_empty());
    assert_eq!(report.pages[0].defaults.len(), 1);
    assert_eq!(
        report.pages[0].defaults[0].kind,
        DefaultColorSpaceKind::DefaultCmyk
    );
    assert_eq!(
        report.pages[0].defaults[0].color_space.family,
        ColorSpaceFamily::DeviceCmyk
    );
}

#[test]
fn default_color_spaces_child_page_resources_override_parent_defaults() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /Resources << /ColorSpace << /DefaultRGB /DeviceRGB >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ColorSpace << /DefaultRGB /DeviceCMYK >> >> >>\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report =
        inspect_document_page_default_color_spaces(&pdf.source, &xref, pdf.object_offset(2))
            .expect("page defaults should inspect");

    assert_eq!(report.pages[0].defaults.len(), 1);
    assert_eq!(
        report.pages[0].defaults[0].kind,
        DefaultColorSpaceKind::DefaultRgb
    );
    assert_eq!(
        report.pages[0].defaults[0].color_space.family,
        ColorSpaceFamily::DeviceCmyk
    );
}

#[test]
fn default_color_spaces_form_uses_only_own_resources() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /DefaultRGB /DeviceRGB >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report = inspect_form_default_color_spaces(
        &pdf.source,
        ObjectLookup::ClassicXref(&xref),
        pdf.object_offset(1),
    );

    assert!(report.skipped.is_empty());
    assert_eq!(report.defaults.len(), 1);
    assert_eq!(report.defaults[0].kind, DefaultColorSpaceKind::DefaultRgb);
    assert_eq!(
        report.defaults[0].color_space.family,
        ColorSpaceFamily::DeviceRgb
    );
}

#[test]
fn default_color_spaces_ignores_top_level_resource_default_key() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /DefaultRGB /DeviceRGB >> >>\nstream\n\nendstream\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report = inspect_form_default_color_spaces(
        &pdf.source,
        ObjectLookup::ClassicXref(&xref),
        pdf.object_offset(1),
    );

    assert!(report.defaults.is_empty());
    assert!(report.skipped.is_empty());
}

#[test]
fn default_color_spaces_malformed_default_entry_is_structured_skip() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /DefaultRGB 7 >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report = inspect_form_default_color_spaces(
        &pdf.source,
        ObjectLookup::ClassicXref(&xref),
        pdf.object_offset(1),
    );

    assert!(report.defaults.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        report.skipped[0].kind,
        Some(DefaultColorSpaceKind::DefaultRgb)
    );
    assert!(matches!(
        report.skipped[0].reason,
        SkippedDefaultColorSpaceReason::ColorSpace {
            color_space_reason: SkippedColorSpaceResourceReason::MalformedColorSpaceOperand
        }
    ));
}

#[test]
fn default_color_spaces_classifies_icc_based_n_four_without_stream_bytes() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /DefaultCMYK [ /ICCBased 2 0 R ] >> >> >>\nstream\n\nendstream\nendobj\n",
        b"2 0 obj\n<< /N 4 /Length 13 >>\nstream\nprivate-bytes\nendstream\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report = inspect_form_default_color_spaces(
        &pdf.source,
        ObjectLookup::ClassicXref(&xref),
        pdf.object_offset(1),
    );

    assert!(report.skipped.is_empty());
    assert_eq!(
        report.defaults[0].color_space.family,
        ColorSpaceFamily::IccBased
    );
    assert_eq!(report.defaults[0].color_space.component_count, Some(4));
    assert!(!format!("{report:?}").contains("private-bytes"));
}

#[test]
fn default_color_spaces_propagate_icc_descriptor_facts() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /DefaultCMYK [ /ICCBased 2 0 R ] >> >> >>\nstream\n\nendstream\nendobj\n",
        b"2 0 obj\n<< /N 4 /Range [ 0 1 0 1 0 1 0 1 ] /Alternate /DeviceCMYK /Length 1 >>\nstream\nx\nendstream\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report = inspect_form_default_color_spaces(
        &pdf.source,
        ObjectLookup::ClassicXref(&xref),
        pdf.object_offset(1),
    );

    assert!(report.skipped.is_empty());
    let fact = &report.defaults[0].color_space;
    assert_eq!(fact.family, ColorSpaceFamily::IccBased);
    assert_eq!(
        fact.icc_profile_stream,
        Some(crate::tests::indirect_ref(2, 0))
    );
    assert_eq!(fact.icc_range_entry_count, Some(8));
    assert_eq!(fact.icc_alternate_present, Some(true));
    assert_eq!(fact.alternate_space, Some(ColorSpaceFamily::DeviceCmyk));
}

#[test]
fn default_color_spaces_classifies_indexed_replacement_fact() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /DefaultRGB [ /Indexed /DeviceRGB 255 <000102> ] >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report = inspect_form_default_color_spaces(
        &pdf.source,
        ObjectLookup::ClassicXref(&xref),
        pdf.object_offset(1),
    );

    assert!(report.skipped.is_empty());
    assert_eq!(report.defaults.len(), 1);
    assert_eq!(report.defaults[0].kind, DefaultColorSpaceKind::DefaultRgb);
    let fact = &report.defaults[0].color_space;
    assert_eq!(fact.family, ColorSpaceFamily::Indexed);
    assert_eq!(fact.component_count, Some(1));
    assert!(fact.spot_names.is_empty());
    assert_eq!(fact.base_space, Some(ColorSpaceFamily::DeviceRgb));
    assert_eq!(fact.indexed_hival, Some(255));
    assert!(!format!("{report:?}").contains("000102"));
}

#[test]
fn default_color_spaces_classifies_separation_and_device_n_facts() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ColorSpace << /DefaultGray [ /Separation /SpotA /DeviceCMYK 2 0 R ] /DefaultCMYK [ /DeviceN [ /Cyan /SpotB ] /DeviceCMYK 2 0 R ] >> >> >>\nstream\n\nendstream\nendobj\n",
        b"2 0 obj\n<< /FunctionType 2 /Domain [ 0 1 ] /N 1 /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);
    let xref = pdf.xref();

    let report = inspect_form_default_color_spaces(
        &pdf.source,
        ObjectLookup::ClassicXref(&xref),
        pdf.object_offset(1),
    );

    assert!(report.skipped.is_empty());
    assert_eq!(report.defaults.len(), 2);
    assert_eq!(report.defaults[0].kind, DefaultColorSpaceKind::DefaultGray);
    assert_eq!(
        report.defaults[0].color_space.family,
        ColorSpaceFamily::Separation
    );
    assert_eq!(
        report.defaults[0].color_space.spot_names,
        vec![PdfName(b"SpotA".to_vec())]
    );
    assert_eq!(
        report.defaults[0].color_space.alternate_space,
        Some(ColorSpaceFamily::DeviceCmyk)
    );
    assert_eq!(report.defaults[1].kind, DefaultColorSpaceKind::DefaultCmyk);
    assert_eq!(
        report.defaults[1].color_space.family,
        ColorSpaceFamily::DeviceN
    );
    assert_eq!(
        report.defaults[1].color_space.spot_names,
        vec![PdfName(b"Cyan".to_vec()), PdfName(b"SpotB".to_vec())]
    );
}
