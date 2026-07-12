use crate::{
    ClassicXrefTableInspection, ExtGStateBlendMode, ExtGStateFontEffect, ExtGStateParamClass,
    FontDictionaryTypeFact, FontSubtypeClass, IndirectRef, ObjectLookup, PdfName,
    SkippedExtGStateResourceReason, inspect_classic_xref_table, inspect_form_extgstate_resources,
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
fn form_own_extgstate_classifies_bm() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ExtGState << /GS1 << /BM /Multiply >> >> >> >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_extgstate_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.skipped.is_empty());
    assert_eq!(report.extgstates.len(), 1);
    assert_eq!(report.extgstates[0].name, PdfName(b"GS1".to_vec()));
    assert_eq!(
        report.extgstates[0].blend_mode,
        ExtGStateParamClass::Set {
            value: ExtGStateBlendMode::NonNormal {
                raw_name: PdfName(b"Multiply".to_vec()),
            },
        }
    );
}

#[test]
fn form_own_scope_transports_font_effect() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 /Resources << /ExtGState << /GS1 << /Font [2 0 R 7.5] >> >> >> >>\nstream\n\nendstream\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype /TrueType >>\nendobj\n",
    ]);

    let report = inspect_form_extgstate_resources(&pdf.source, pdf.lookup(), pdf.object_offset(1));

    assert!(report.skipped.is_empty());
    let gs = &report.extgstates[0];
    assert!(gs.has_unclassified_keys, "/Font stays unclassified");
    assert_eq!(
        gs.font_effect,
        ExtGStateFontEffect::StructurallyValid {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            object_byte_offset: pdf.object_offset(2),
            size_bits: 7.5f64.to_bits(),
            dictionary_type: FontDictionaryTypeFact::Font,
            subtype: FontSubtypeClass::TrueType,
        }
    );
}

#[test]
fn form_never_inherits_page_extgstate_font_effects() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /ExtGState << /GS0 << /Font [4 0 R 12] >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm1 3 0 R >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
        b"4 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);

    let report = inspect_form_extgstate_resources(&pdf.source, pdf.lookup(), pdf.object_offset(3));

    assert!(report.extgstates.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedExtGStateResourceReason::MissingExtGStateResources
    ));
}

#[test]
fn form_scope_does_not_inherit_page_resources() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /ExtGState << /GS0 << /BM /Multiply >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm1 3 0 R >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /XObject /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
    ]);

    let report = inspect_form_extgstate_resources(&pdf.source, pdf.lookup(), pdf.object_offset(3));

    assert!(report.extgstates.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedExtGStateResourceReason::MissingExtGStateResources
    ));
}
