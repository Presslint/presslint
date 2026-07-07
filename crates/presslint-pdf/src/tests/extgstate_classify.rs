use crate::{
    ClassicXrefTableInspection, ExtGStateAlpha, ExtGStateBlendMode, ExtGStateOverprintMode,
    ExtGStateParamClass, ExtGStateSoftMask, ObjectLookup, PdfName, classify_extgstate_entry,
    inspect_classic_xref_table, inspect_dictionary_entries,
};

struct Fixture {
    xref: ClassicXrefTableInspection,
}

impl Fixture {
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
    Fixture { xref }
}

fn classify_direct(dict: &[u8]) -> crate::ClassifiedExtGStateResource {
    let pdf = fixture(&[]);
    let entries = inspect_dictionary_entries(dict, 0).expect("outer dictionary should inspect");
    let entry = entries.entries[0];
    classify_extgstate_entry(dict, pdf.lookup(), &PdfName(b"GS".to_vec()), entry)
        .expect("ExtGState should classify")
}

#[test]
fn classifies_op_true_and_opm_one_without_defaulting_op() {
    let resource = classify_direct(b"<< /GS << /OP true /OPM 1 >> >>");

    assert_eq!(
        resource.op_stroking,
        ExtGStateParamClass::Set { value: true }
    );
    assert_eq!(resource.op_nonstroking, ExtGStateParamClass::Unset);
    assert_eq!(
        resource.overprint_mode,
        ExtGStateParamClass::Set {
            value: ExtGStateOverprintMode::One,
        }
    );
}

#[test]
fn classifies_only_bm_multiply_and_leaves_other_params_unset() {
    let resource = classify_direct(b"<< /GS << /BM /Multiply >> >>");

    assert_eq!(resource.op_stroking, ExtGStateParamClass::Unset);
    assert_eq!(resource.overprint_mode, ExtGStateParamClass::Unset);
    assert_eq!(
        resource.blend_mode,
        ExtGStateParamClass::Set {
            value: ExtGStateBlendMode::NonNormal {
                raw_name: PdfName(b"Multiply".to_vec()),
            },
        }
    );
}

#[test]
fn classifies_bm_compatible_as_normal_equivalent() {
    let resource = classify_direct(b"<< /GS << /BM /Compatible >> >>");

    assert_eq!(
        resource.blend_mode,
        ExtGStateParamClass::Set {
            value: ExtGStateBlendMode::Normal {
                raw_name: PdfName(b"Compatible".to_vec()),
            },
        }
    );
    assert!(!resource.is_transparency_active());
    assert!(!resource.has_unresolved_or_unclassified_safety_param());
}

#[test]
fn classifies_alpha_exact_one_as_opaque_and_other_numbers_non_opaque() {
    let opaque = classify_direct(b"<< /GS << /CA 1.0 >> >>");
    let non_opaque = classify_direct(b"<< /GS << /CA 0.5 >> >>");

    assert_eq!(
        opaque.stroking_alpha,
        ExtGStateParamClass::Set {
            value: ExtGStateAlpha::Opaque,
        }
    );
    assert_eq!(
        non_opaque.stroking_alpha,
        ExtGStateParamClass::Set {
            value: ExtGStateAlpha::NonOpaque {
                raw: b"0.5".to_vec(),
            },
        }
    );
}

#[test]
fn classifies_smask_none_name_vs_present_dictionary() {
    let none = classify_direct(b"<< /GS << /SMask /None >> >>");
    let present = classify_direct(b"<< /GS << /SMask << /S /Luminosity >> >> >>");

    assert_eq!(
        none.soft_mask,
        ExtGStateParamClass::Set {
            value: ExtGStateSoftMask::None,
        }
    );
    assert_eq!(
        present.soft_mask,
        ExtGStateParamClass::Set {
            value: ExtGStateSoftMask::Present,
        }
    );
}

#[test]
fn extra_key_marks_unclassified_keys_but_keeps_safety_params() {
    let resource = classify_direct(b"<< /GS << /BM /Normal /LW 2 >> >>");

    assert!(resource.has_unclassified_keys);
    assert_eq!(
        resource.blend_mode,
        ExtGStateParamClass::Set {
            value: ExtGStateBlendMode::Normal {
                raw_name: PdfName(b"Normal".to_vec()),
            },
        }
    );
}

#[test]
fn safety_predicates_mark_active_overprint_without_defaulting_op() {
    let resource = classify_direct(b"<< /GS << /OP true >> >>");

    assert!(resource.is_overprint_active());
    assert!(!resource.is_transparency_active());
    assert!(!resource.has_unresolved_or_unclassified_safety_param());
}

#[test]
fn safety_predicates_mark_non_normal_blend_as_transparency() {
    let resource = classify_direct(b"<< /GS << /BM /Multiply >> >>");

    assert!(!resource.is_overprint_active());
    assert!(resource.is_transparency_active());
    assert!(!resource.has_unresolved_or_unclassified_safety_param());
}

#[test]
fn harmless_unclassified_keys_do_not_make_safety_unknown() {
    let resource = classify_direct(b"<< /GS << /LW 2 /LC 1 >> >>");

    assert!(resource.has_unclassified_keys);
    assert!(!resource.is_overprint_active());
    assert!(!resource.is_transparency_active());
    assert!(!resource.has_unresolved_or_unclassified_safety_param());
}

#[test]
fn malformed_or_unknown_safety_params_are_unclassified() {
    let malformed = classify_direct(b"<< /GS << /OP /yes >> >>");
    let unknown_blend = classify_direct(b"<< /GS << /BM [/Normal /Multiply] >> >>");

    assert!(malformed.has_unresolved_or_unclassified_safety_param());
    assert!(unknown_blend.has_unresolved_or_unclassified_safety_param());
}
