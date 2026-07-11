use crate::{
    ClassicXrefTableInspection, ImageColorSpaceMetadata, ImageIntegerMetadata, ImageMaskMetadata,
    ImageXObjectMetadata, IndirectRef, ObjectLookup, PageXObjectResourceTarget, PdfName,
    SkippedPageXObjectResourceReason, inspect_classic_xref_table, inspect_form_xobject_resources,
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

/// Form target helper: forms never carry image metadata.
fn target(name: &[u8], object_number: u32, object_byte_offset: usize) -> PageXObjectResourceTarget {
    PageXObjectResourceTarget {
        name: PdfName(name.to_vec()),
        reference: IndirectRef {
            object_number,
            generation: 0,
        },
        object_byte_offset,
        image_metadata: None,
    }
}

/// Image target helper carrying the structural image dictionary metadata.
fn image_target(
    name: &[u8],
    object_number: u32,
    object_byte_offset: usize,
    image_metadata: ImageXObjectMetadata,
) -> PageXObjectResourceTarget {
    PageXObjectResourceTarget {
        name: PdfName(name.to_vec()),
        reference: IndirectRef {
            object_number,
            generation: 0,
        },
        object_byte_offset,
        image_metadata: Some(image_metadata),
    }
}

/// Metadata for a `/Width w /Height h /BitsPerComponent bpc` image with no
/// `/ColorSpace` entry.
fn wh_bpc_no_colorspace(width: u32, height: u32, bits_per_component: u32) -> ImageXObjectMetadata {
    ImageXObjectMetadata {
        width: ImageIntegerMetadata::Value { value: width },
        height: ImageIntegerMetadata::Value { value: height },
        bits_per_component: ImageIntegerMetadata::Value {
            value: bits_per_component,
        },
        color_space: ImageColorSpaceMetadata::Missing,
        image_mask: ImageMaskMetadata::Missing,
    }
}

#[test]
fn form_own_resources_classify_nested_image_and_form_targets() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 10 /Resources << /XObject << /In 2 0 R /Fn 3 0 R >> >> >>\nstream\n/In Do\nendstream\nendobj\n",
        b"2 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 >>\nstream\nx\nendstream\nendobj\n",
        b"3 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_xobject_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert_eq!(report.object_byte_offset, pdf.object_offset(1));
    assert_eq!(
        report.image_xobjects,
        vec![image_target(
            b"In",
            2,
            pdf.object_offset(2),
            wh_bpc_no_colorspace(1, 1, 8)
        )]
    );
    assert_eq!(
        report.form_xobjects,
        vec![target(b"Fn", 3, pdf.object_offset(3))]
    );
    assert_eq!(report.image_xobject_names, vec![PdfName(b"In".to_vec())]);
    assert_eq!(report.form_xobject_names, vec![PdfName(b"Fn".to_vec())]);
    assert!(report.skipped.is_empty());
}

#[test]
fn form_image_target_carries_direct_colorspace_metadata() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 7 /Resources << /XObject << /In 2 0 R >> >> >>\nstream\n/In Do\nendstream\nendobj\n",
        b"2 0 obj\n<< /Type /XObject /Subtype /Image /Width 4 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceCMYK >>\nstream\nx\nendstream\nendobj\n",
    ]);

    let report = inspect_form_xobject_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert_eq!(
        report.image_xobjects,
        vec![image_target(
            b"In",
            2,
            pdf.object_offset(2),
            ImageXObjectMetadata {
                width: ImageIntegerMetadata::Value { value: 4 },
                height: ImageIntegerMetadata::Value { value: 2 },
                bits_per_component: ImageIntegerMetadata::Value { value: 8 },
                color_space: ImageColorSpaceMetadata::DeviceCmyk,
                image_mask: ImageMaskMetadata::Missing,
            }
        )]
    );
}

#[test]
fn form_nested_stencil_target_carries_generic_metadata_without_form_admission() {
    // The generic metadata (including the `/ImageMask` fact) propagates through
    // the shared classifier for form-scope resources exactly as for pages;
    // the enclosing Form target itself never carries image metadata.
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 7 /Resources << /XObject << /St 2 0 R /Fn 3 0 R >> >> >>\nstream\n/St Do\nendstream\nendobj\n",
        b"2 0 obj\n<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ImageMask true >>\nstream\nx\nendstream\nendobj\n",
        b"3 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_xobject_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert_eq!(
        report.image_xobjects,
        vec![image_target(
            b"St",
            2,
            pdf.object_offset(2),
            ImageXObjectMetadata {
                width: ImageIntegerMetadata::Value { value: 2 },
                height: ImageIntegerMetadata::Value { value: 2 },
                bits_per_component: ImageIntegerMetadata::Missing,
                color_space: ImageColorSpaceMetadata::Missing,
                image_mask: ImageMaskMetadata::True,
            }
        )]
    );
    assert_eq!(
        report.form_xobjects,
        vec![target(b"Fn", 3, pdf.object_offset(3))]
    );
}

#[test]
fn form_without_resources_reports_missing_resources() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_xobject_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.image_xobjects.is_empty());
    assert!(report.form_xobjects.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedPageXObjectResourceReason::MissingResources
    ));
}

#[test]
fn form_resources_without_xobject_dictionary_report_missing_xobject() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /Font << /F1 2 0 R >> >> >>\nstream\n\nendstream\nendobj\n",
        b"2 0 obj\n<< /Type /Font >>\nendobj\n",
    ]);

    let report = inspect_form_xobject_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.form_xobjects.is_empty());
    assert!(report.image_xobjects.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedPageXObjectResourceReason::MissingXObject
    ));
}

#[test]
fn form_resources_report_retains_no_source_bytes() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 13 /Resources << /XObject << /In 2 0 R >> >> >>\nstream\nsecret-marks!\nendstream\nendobj\n",
        b"2 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 >>\nstream\nx\nendstream\nendobj\n",
    ]);

    let report = inspect_form_xobject_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("secret-marks"));
}
