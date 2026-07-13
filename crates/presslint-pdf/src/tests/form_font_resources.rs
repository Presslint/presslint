use crate::{
    ClassicXrefTableInspection, FontDictionaryTypeFact, FontSubtypeClass, ObjectLookup, PdfName,
    SkippedFontResourceReason, inspect_classic_xref_table, inspect_form_font_resources,
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
fn form_own_font_classifies_type_and_subtype() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /Font << /F1 << /Type /Font /Subtype /Type3 >> >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_font_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.skipped.is_empty());
    assert_eq!(report.fonts.len(), 1);
    assert_eq!(report.fonts[0].name, PdfName(b"F1".to_vec()));
    assert_eq!(
        report.fonts[0].dictionary_type,
        FontDictionaryTypeFact::Font
    );
    assert_eq!(report.fonts[0].subtype, FontSubtypeClass::Type3);
}

#[test]
fn form_scope_never_inherits_page_fonts() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm1 3 0 R >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_font_resources(&pdf.source, pdf.lookup(), pdf.object_offset(3));

    assert!(report.fonts.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        report.skipped[0].reason,
        SkippedFontResourceReason::MissingFontResources
    );
}

#[test]
fn form_own_resources_without_font_is_missing_font() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ExtGState << >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_font_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.fonts.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(
        report.skipped[0].reason,
        SkippedFontResourceReason::MissingFont
    );
}

#[test]
fn form_recognizes_escaped_font_namespace_and_preserves_raw_binding_name() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /F#6fnt << /F#31 << /Type /Font /Subtype /Type1 >> >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_font_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.skipped.is_empty());
    assert_eq!(report.fonts.len(), 1);
    assert_eq!(report.fonts[0].name, PdfName(b"F#31".to_vec()));
}
