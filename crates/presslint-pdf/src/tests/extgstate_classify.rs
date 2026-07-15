#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use crate::{
    ClassicXrefTableInspection, ClassifiedExtGStateResource, DictionaryEntryByteRange,
    DictionaryEntryInspectionRejection, DictionaryValueKind, ExtGStateAlpha, ExtGStateBlendMode,
    ExtGStateFontEffect, ExtGStateOverprintMode, ExtGStateParamClass, ExtGStateSoftMask,
    FontDictionaryTypeFact, FontSubtypeClass, IndirectObjectDictionaryInspectionError,
    IndirectObjectDictionaryInspectionRejection, IndirectRef, IndirectReferenceInspectionRejection,
    ObjectLookup, ObjectLookupLocation, PdfName, XrefStreamEntry, XrefStreamEntryRecord,
    XrefStreamSection, XrefStreamSubsection, classify_extgstate_entry, inspect_classic_xref_table,
    inspect_dictionary_entries, inspect_indirect_object_dictionary,
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

fn classify_direct(dict: &[u8]) -> crate::ClassifiedExtGStateResource {
    let pdf = fixture(&[]);
    let entries = inspect_dictionary_entries(dict, 0).expect("outer dictionary should inspect");
    let entry = entries.entries[0];
    classify_extgstate_entry(dict, pdf.lookup(), &PdfName(b"GS".to_vec()), entry)
        .expect("ExtGState should classify")
}

/// Classify the ExtGState-shaped first entry of object 1 in a full fixture.
fn classify_object_entry(pdf: &Fixture) -> ClassifiedExtGStateResource {
    let dictionary = inspect_indirect_object_dictionary(&pdf.source, pdf.object_offset(1))
        .expect("object 1 dictionary should inspect");
    let entry = dictionary.entries[0];
    classify_extgstate_entry(&pdf.source, pdf.lookup(), &PdfName(b"GS".to_vec()), entry)
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

#[test]
fn all_duplicate_safety_keys_fail_closed_even_with_identical_safe_values() {
    let cases: &[&[u8]] = &[
        b"<< /GS << /OP false /OP false >> >>",
        b"<< /GS << /op false /op false >> >>",
        b"<< /GS << /OPM 0 /OPM 0 >> >>",
        b"<< /GS << /CA 1 /CA 1 >> >>",
        b"<< /GS << /ca 1 /ca 1 >> >>",
        b"<< /GS << /BM /Normal /BM /Normal >> >>",
        b"<< /GS << /SMask /None /SMask /None >> >>",
    ];

    for dict in cases {
        let resource = classify_direct(dict);
        assert!(
            resource.has_unresolved_or_unclassified_safety_param(),
            "duplicate safety key did not fail closed: {}",
            String::from_utf8_lossy(dict)
        );
        assert!(
            !resource.has_unclassified_keys,
            "known safety duplicate escaped into the unknown-key aggregate: {}",
            String::from_utf8_lossy(dict)
        );
    }
}

#[test]
fn unsafe_and_safe_duplicate_orders_never_first_or_last_win() {
    let cases: &[&[u8]] = &[
        b"<< /GS << /OP true /OP false >> >>",
        b"<< /GS << /OP false /OP true >> >>",
        b"<< /GS << /CA 0.5 /CA 1 >> >>",
        b"<< /GS << /CA 1 /CA 0.5 >> >>",
        b"<< /GS << /BM /Multiply /BM /Normal >> >>",
        b"<< /GS << /BM /Normal /BM /Multiply >> >>",
    ];

    for dict in cases {
        let resource = classify_direct(dict);
        assert!(
            resource.has_unresolved_or_unclassified_safety_param(),
            "duplicate order recovered a value: {}",
            String::from_utf8_lossy(dict)
        );
        assert!(!resource.has_unclassified_keys);
    }
}

#[test]
fn escaped_safety_keys_dispatch_semantically_and_collide_with_raw_spelling() {
    let safe = classify_direct(b"<< /GS << /O#50 false >> >>");
    assert!(!safe.is_overprint_active());
    assert!(!safe.has_unresolved_or_unclassified_safety_param());
    assert!(!safe.has_unclassified_keys);

    let unsafe_key = classify_direct(b"<< /GS << /O#50 true >> >>");
    assert!(unsafe_key.is_overprint_active());
    assert!(!unsafe_key.has_unresolved_or_unclassified_safety_param());
    assert!(!unsafe_key.has_unclassified_keys);

    let duplicate = classify_direct(b"<< /GS << /OP false /O#50 false >> >>");
    assert!(duplicate.has_unresolved_or_unclassified_safety_param());
    assert!(!duplicate.has_unclassified_keys);
}

#[test]
fn absent_font_key_is_unset_effect_without_unclassified_flag() {
    let resource = classify_direct(b"<< /GS << /OP true >> >>");

    assert_eq!(resource.font_effect, ExtGStateFontEffect::Unset);
    assert!(!resource.has_unclassified_keys);
}

#[test]
fn escaped_font_spelling_is_the_semantic_font_effect_key() {
    let resource = classify_direct(b"<< /GS << /Fo#6Et [2 0 R 12] >> >>");

    assert!(matches!(
        resource.font_effect,
        ExtGStateFontEffect::UnresolvedTarget {
            reference: IndirectRef {
                object_number: 2,
                generation: 0
            },
            ..
        }
    ));
    assert!(resource.has_unclassified_keys);
}

#[test]
fn exact_and_escaped_font_keys_poison_as_a_semantic_duplicate() {
    let resource = classify_direct(b"<< /GS << /Font [2 0 R 12] /F#6fnt [3 0 R 9] >> >>");

    assert!(matches!(
        resource.font_effect,
        ExtGStateFontEffect::DuplicateKey { .. }
    ));
    assert!(resource.has_unclassified_keys);
}

#[test]
fn unrelated_and_malformed_keys_remain_distinct_from_semantic_font() {
    let unrelated = classify_direct(b"<< /GS << /Font [2 0 R 12] /LineWidth 2 >> >>");
    assert!(matches!(
        unrelated.font_effect,
        ExtGStateFontEffect::UnresolvedTarget { .. }
    ));
    assert!(unrelated.has_unclassified_keys);

    let malformed = classify_direct(b"<< /GS << /F#6znt [2 0 R 12] >> >>");
    assert_eq!(malformed.font_effect, ExtGStateFontEffect::Unset);
    assert!(malformed.has_unclassified_keys);
}

#[test]
fn font_effect_classifies_all_five_legal_subtypes_structurally_valid() {
    let cases: &[(&[u8], FontSubtypeClass)] = &[
        (b"Type1", FontSubtypeClass::Type1),
        (b"MMType1", FontSubtypeClass::MmType1),
        (b"TrueType", FontSubtypeClass::TrueType),
        (b"Type0", FontSubtypeClass::Type0),
        (b"Type3", FontSubtypeClass::Type3),
    ];
    for (raw, expected) in cases {
        let mut font_object = b"2 0 obj\n<< /Type /Font /Subtype /".to_vec();
        font_object.extend_from_slice(raw);
        font_object.extend_from_slice(b" >>\nendobj\n");
        let pdf = fixture(&[
            b"1 0 obj\n<< /GS << /Font [2 0 R 12] >> >>\nendobj\n",
            &font_object,
        ]);

        let resource = classify_object_entry(&pdf);

        assert!(resource.has_unclassified_keys, "/Font stays unclassified");
        assert_eq!(
            resource.font_effect,
            ExtGStateFontEffect::StructurallyValid {
                reference: IndirectRef {
                    object_number: 2,
                    generation: 0,
                },
                object_byte_offset: pdf.object_offset(2),
                size_bits: 12.0f64.to_bits(),
                dictionary_type: FontDictionaryTypeFact::Font,
                subtype: expected.clone(),
            }
        );
    }
}

#[test]
fn font_effect_accepts_legal_whitespace_and_comments_in_the_array() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /GS << /Font [ % font\n 2 0 R % size\n 0.5 % done\n ] >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);

    let resource = classify_object_entry(&pdf);

    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::StructurallyValid {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            object_byte_offset: pdf.object_offset(2),
            size_bits: 0.5f64.to_bits(),
            dictionary_type: FontDictionaryTypeFact::Font,
            subtype: FontSubtypeClass::Type1,
        }
    );
}

#[test]
fn font_size_bits_preserve_zero_sign_negatives_and_fractions_exactly() {
    let cases: &[(&[u8], f64)] = &[
        (b"0", 0.0),
        (b"-0", -0.0),
        (b"-3", -3.0),
        (b"12.25", 12.25),
        (b"+7.", 7.0),
        (b".5", 0.5),
    ];
    for (raw, expected) in cases {
        let mut holder = b"1 0 obj\n<< /GS << /Font [2 0 R ".to_vec();
        holder.extend_from_slice(raw);
        holder.extend_from_slice(b"] >> >>\nendobj\n");
        let pdf = fixture(&[
            &holder,
            b"2 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
        ]);

        let resource = classify_object_entry(&pdf);

        let size_bits = match resource.font_effect {
            ExtGStateFontEffect::StructurallyValid { size_bits, .. } => Some(size_bits),
            _ => None,
        };
        let size_bits = size_bits.expect("size should be structurally valid");
        assert_eq!(size_bits, expected.to_bits(), "size lexeme {raw:?}");
    }

    assert_ne!(0.0f64.to_bits(), (-0.0f64).to_bits());
}

#[test]
fn size_overflow_malformed_and_indirect_sizes_are_distinct_outcomes() {
    // A numeric overflow must stay lexically valid PDF number syntax: a
    // plain decimal larger than f64::MAX, never exponential notation.
    let mut overflow_dict = b"<< /GS << /Font [2 0 R 1".to_vec();
    overflow_dict.extend_from_slice(&[b'0'; 400]);
    overflow_dict.extend_from_slice(b"] >> >>");
    let overflow = classify_direct(&overflow_dict);
    let malformed = classify_direct(b"<< /GS << /Font [2 0 R 1.2.3] >> >>");
    let name_size = classify_direct(b"<< /GS << /Font [2 0 R /F1] >> >>");
    let indirect = classify_direct(b"<< /GS << /Font [2 0 R 3 0 R] >> >>");

    assert!(matches!(
        overflow.font_effect,
        ExtGStateFontEffect::NonFiniteSize { .. }
    ));
    assert!(matches!(
        malformed.font_effect,
        ExtGStateFontEffect::MalformedSize { size_range } if size_range.start < size_range.end
    ));
    assert!(matches!(
        name_size.font_effect,
        ExtGStateFontEffect::MalformedSize { .. }
    ));
    assert_eq!(
        indirect.font_effect,
        ExtGStateFontEffect::IndirectSizeUnsupported {
            reference: IndirectRef {
                object_number: 3,
                generation: 0,
            },
        }
    );
}

#[test]
fn font_and_indirect_size_references_accept_comment_separators() {
    let valid = fixture(&[
        b"1 0 obj\n<< /GS << /Font [2%comment\n0%comment\nR 12] >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);
    let resource = classify_object_entry(&valid);
    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::StructurallyValid {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            object_byte_offset: valid.object_offset(2),
            size_bits: 12.0f64.to_bits(),
            dictionary_type: FontDictionaryTypeFact::Font,
            subtype: FontSubtypeClass::Type1,
        }
    );

    let indirect_size = classify_direct(b"<< /GS << /Font [2 0 R 3%comment\n0%comment\nR] >> >>");
    assert_eq!(
        indirect_size.font_effect,
        ExtGStateFontEffect::IndirectSizeUnsupported {
            reference: IndirectRef {
                object_number: 3,
                generation: 0,
            },
        }
    );
}

#[test]
fn comment_separated_reference_missing_generation_remains_malformed() {
    let resource = classify_direct(b"<< /GS << /Font [2%comment\nR 12] >> >>");
    let bad_keyword_boundary = classify_direct(b"<< /GS << /Font [2 0 R0 12] >> >>");
    let generation_out_of_range = classify_direct(b"<< /GS << /Font [2 65536 R 12] >> >>");

    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
    assert_eq!(
        bad_keyword_boundary.font_effect,
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
    assert_eq!(
        generation_out_of_range.font_effect,
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::GenerationOutOfRange,
        }
    );
}

#[test]
fn exponential_size_notation_is_malformed_not_a_number() {
    // ISO 32000-1 §7.3.3 numbers carry no exponent: Rust's f64 parser would
    // accept these lexemes, so the PDF grammar gate must reject them first.
    for dict in [
        b"<< /GS << /Font [2 0 R 1e2] >> >>".as_slice(),
        b"<< /GS << /Font [2 0 R 1e999] >> >>".as_slice(),
        b"<< /GS << /Font [2 0 R inf] >> >>".as_slice(),
        b"<< /GS << /Font [2 0 R NaN] >> >>".as_slice(),
    ] {
        let resource = classify_direct(dict);
        assert!(
            matches!(
                resource.font_effect,
                ExtGStateFontEffect::MalformedSize { size_range } if size_range.start < size_range.end
            ),
            "lexeme in {dict:?} should be malformed"
        );
    }
}

#[test]
fn non_array_values_and_wrong_arity_arrays_fail_closed_distinctly() {
    let number = classify_direct(b"<< /GS << /Font 12 >> >>");
    let dictionary = classify_direct(b"<< /GS << /Font << /Type /Font >> >> >>");
    let empty = classify_direct(b"<< /GS << /Font [] >> >>");
    let one_element = classify_direct(b"<< /GS << /Font [2 0 R] >> >>");
    let dict = b"<< /GS << /Font [2 0 R 12 14] >> >>";
    let three_elements = classify_direct(dict);

    assert_eq!(
        number.font_effect,
        ExtGStateFontEffect::NonArrayValue {
            value_kind: DictionaryValueKind::NumberLike,
        }
    );
    assert_eq!(
        dictionary.font_effect,
        ExtGStateFontEffect::NonArrayValue {
            value_kind: DictionaryValueKind::Dictionary,
        }
    );
    assert_eq!(
        empty.font_effect,
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
    assert!(matches!(
        one_element.font_effect,
        ExtGStateFontEffect::MalformedSize { size_range } if size_range.start == size_range.end
    ));
    let trailing_range = match three_elements.font_effect {
        ExtGStateFontEffect::ExtraArrayElements { trailing_range } => Some(trailing_range),
        _ => None,
    };
    let trailing_range = trailing_range.expect("three-element array should report extra elements");
    assert_eq!(&dict[trailing_range.start..trailing_range.end], b"14");
}

#[test]
fn direct_dictionary_name_and_out_of_range_first_operands_never_become_valid() {
    let direct_dictionary = classify_direct(b"<< /GS << /Font [<< /Type /Font >> 12] >> >>");
    let direct_name = classify_direct(b"<< /GS << /Font [/F1 12] >> >>");
    let out_of_range = classify_direct(b"<< /GS << /Font [99999999999 0 R 12] >> >>");

    assert_eq!(
        direct_dictionary.font_effect,
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
    assert_eq!(
        direct_name.font_effect,
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
    assert_eq!(
        out_of_range.font_effect,
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::ObjectNumberOutOfRange,
        }
    );
}

#[test]
fn undefined_free_and_generation_mismatched_targets_are_unresolved_facts() {
    let undefined = fixture(&[b"1 0 obj\n<< /GS << /Font [99 0 R 12] >> >>\nendobj\n"]);
    let resource = classify_object_entry(&undefined);
    assert!(matches!(
        resource.font_effect,
        ExtGStateFontEffect::UnresolvedTarget {
            reference: IndirectRef {
                object_number: 99,
                generation: 0,
            },
            location: Some(ObjectLookupLocation::ClassicNotFound { .. }),
        }
    ));

    let free = fixture(&[b"1 0 obj\n<< /GS << /Font [0 65535 R 12] >> >>\nendobj\n"]);
    let resource = classify_object_entry(&free);
    assert!(matches!(
        resource.font_effect,
        ExtGStateFontEffect::UnresolvedTarget {
            location: Some(ObjectLookupLocation::ClassicFree { .. }),
            ..
        }
    ));

    let mismatch = fixture(&[
        b"1 0 obj\n<< /GS << /Font [2 1 R 12] >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);
    let resource = classify_object_entry(&mismatch);
    assert!(matches!(
        resource.font_effect,
        ExtGStateFontEffect::UnresolvedTarget {
            reference: IndirectRef {
                object_number: 2,
                generation: 1,
            },
            location: None,
        }
    ));
}

#[test]
fn non_dictionary_target_is_a_target_dictionary_failure() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /GS << /Font [2 0 R 12] >> >>\nendobj\n",
        b"2 0 obj\n42\nendobj\n",
    ]);

    let resource = classify_object_entry(&pdf);

    assert!(matches!(
        resource.font_effect,
        ExtGStateFontEffect::TargetDictionaryFailed {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            ..
        }
    ));
}

#[test]
fn header_reference_mismatch_fails_closed_and_never_classifies_the_foreign_body() {
    // The xref entry for object 2 points at a body whose header says
    // `3 0 obj`: the binding is unusable even though the body is a valid
    // font dictionary.
    let number_mismatch = fixture(&[
        b"1 0 obj\n<< /GS << /Font [2 0 R 12] >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);
    let resource = classify_object_entry(&number_mismatch);
    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::TargetHeaderMismatch {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            object_byte_offset: number_mismatch.object_offset(2),
            header_reference: IndirectRef {
                object_number: 3,
                generation: 0,
            },
        }
    );

    // Same offset, right object number, wrong header generation.
    let generation_mismatch = fixture(&[
        b"1 0 obj\n<< /GS << /Font [2 0 R 12] >> >>\nendobj\n",
        b"2 5 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);
    let resource = classify_object_entry(&generation_mismatch);
    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::TargetHeaderMismatch {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            object_byte_offset: generation_mismatch.object_offset(2),
            header_reference: IndirectRef {
                object_number: 2,
                generation: 5,
            },
        }
    );

    // Header identity is checked before the body's shape. A foreign
    // non-dictionary body cannot mask the mismatched xref binding as a
    // dictionary-inspection failure.
    let foreign_non_dictionary = fixture(&[
        b"1 0 obj\n<< /GS << /Font [2 0 R 12] >> >>\nendobj\n",
        b"3 0 obj\n42\nendobj\n",
    ]);
    let resource = classify_object_entry(&foreign_non_dictionary);
    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::TargetHeaderMismatch {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            object_byte_offset: foreign_non_dictionary.object_offset(2),
            header_reference: IndirectRef {
                object_number: 3,
                generation: 0,
            },
        }
    );
}

#[test]
fn cid_descendants_missing_type_and_unknown_subtypes_are_inadmissible_facts() {
    let cases: &[(&[u8], FontDictionaryTypeFact, FontSubtypeClass)] = &[
        (
            b"2 0 obj\n<< /Type /Font /Subtype /CIDFontType0 >>\nendobj\n",
            FontDictionaryTypeFact::Font,
            FontSubtypeClass::CidFontType0,
        ),
        (
            b"2 0 obj\n<< /Type /Font /Subtype /CIDFontType2 >>\nendobj\n",
            FontDictionaryTypeFact::Font,
            FontSubtypeClass::CidFontType2,
        ),
        (
            b"2 0 obj\n<< /Subtype /Type1 >>\nendobj\n",
            FontDictionaryTypeFact::Missing,
            FontSubtypeClass::Type1,
        ),
        (
            b"2 0 obj\n<< /Type /Font /Subtype /Type1C >>\nendobj\n",
            FontDictionaryTypeFact::Font,
            FontSubtypeClass::OtherName {
                name: PdfName(b"Type1C".to_vec()),
            },
        ),
    ];
    for (font_object, expected_type, expected_subtype) in cases {
        let pdf = fixture(&[
            b"1 0 obj\n<< /GS << /Font [2 0 R 12] >> >>\nendobj\n",
            font_object,
        ]);

        let resource = classify_object_entry(&pdf);

        assert_eq!(
            resource.font_effect,
            ExtGStateFontEffect::InadmissibleTarget {
                reference: IndirectRef {
                    object_number: 2,
                    generation: 0,
                },
                object_byte_offset: pdf.object_offset(2),
                size_bits: 12.0f64.to_bits(),
                dictionary_type: expected_type.clone(),
                subtype: expected_subtype.clone(),
            }
        );
    }
}

#[test]
fn duplicate_font_keys_record_both_ranges_without_recovery() {
    let dict: &[u8] = b"<< /GS << /Font [2 0 R 12] /Font [3 0 R 9] >> >>";
    let resource = classify_direct(dict);

    assert!(resource.has_unclassified_keys);
    let ranges = match resource.font_effect {
        ExtGStateFontEffect::DuplicateKey {
            first_key_range,
            duplicate_key_range,
        } => Some((first_key_range, duplicate_key_range)),
        _ => None,
    };
    let (first_key_range, duplicate_key_range) =
        ranges.expect("duplicate /Font keys should classify as DuplicateKey");
    assert_eq!(&dict[first_key_range.start..first_key_range.end], b"/Font");
    assert_eq!(
        &dict[duplicate_key_range.start..duplicate_key_range.end],
        b"/Font"
    );
    assert!(first_key_range.start < duplicate_key_range.start);
}

#[test]
fn bad_font_never_erases_safety_params_and_success_keeps_the_flag() {
    let without_font = classify_direct(
        b"<< /GS << /OP true /op false /OPM 1 /CA 0.5 /ca 1.0 /BM /Multiply /SMask /None >> >>",
    );
    let with_bad_font = classify_direct(
        b"<< /GS << /OP true /op false /OPM 1 /CA 0.5 /ca 1.0 /BM /Multiply /SMask /None /Font 12 >> >>",
    );

    assert_eq!(with_bad_font.op_stroking, without_font.op_stroking);
    assert_eq!(with_bad_font.op_nonstroking, without_font.op_nonstroking);
    assert_eq!(with_bad_font.overprint_mode, without_font.overprint_mode);
    assert_eq!(with_bad_font.stroking_alpha, without_font.stroking_alpha);
    assert_eq!(
        with_bad_font.nonstroking_alpha,
        without_font.nonstroking_alpha
    );
    assert_eq!(with_bad_font.blend_mode, without_font.blend_mode);
    assert_eq!(with_bad_font.soft_mask, without_font.soft_mask);
    assert!(!without_font.has_unclassified_keys);
    assert!(with_bad_font.has_unclassified_keys);
    assert_eq!(
        with_bad_font.font_effect,
        ExtGStateFontEffect::NonArrayValue {
            value_kind: DictionaryValueKind::NumberLike,
        }
    );

    let valid = fixture(&[
        b"1 0 obj\n<< /GS << /OP true /Font [2 0 R 12] >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);
    let resource = classify_object_entry(&valid);
    assert_eq!(
        resource.op_stroking,
        ExtGStateParamClass::Set { value: true }
    );
    assert!(
        resource.has_unclassified_keys,
        "a valid /Font still keeps the aggregate flag in this slice"
    );
    assert!(matches!(
        resource.font_effect,
        ExtGStateFontEffect::StructurallyValid { .. }
    ));
}

/// Xref-stream section marking object 2 as a compressed member of stream 9.
fn compressed_object_section() -> XrefStreamSection {
    XrefStreamSection {
        object_byte_offset: 200,
        widths: [1, 4, 2],
        size: 20,
        index_subsections: vec![XrefStreamSubsection {
            first_object_number: 0,
            entry_count: 20,
        }],
        root_reference: IndirectRef {
            object_number: 1,
            generation: 0,
        },
        prev_byte_offset: None,
        entries: vec![XrefStreamEntry {
            object_number: 2,
            record: XrefStreamEntryRecord::Compressed {
                object_stream_number: 9,
                index_within_object_stream: 3,
            },
        }],
    }
}

fn classify_compressed_section_entry(dict: &[u8]) -> ClassifiedExtGStateResource {
    let section = compressed_object_section();
    let entries = inspect_dictionary_entries(dict, 0).expect("outer dictionary should inspect");
    classify_extgstate_entry(
        dict,
        ObjectLookup::XrefStreamSection(&section),
        &PdfName(b"GS".to_vec()),
        entries.entries[0],
    )
    .expect("ExtGState should classify")
}

#[test]
fn compressed_font_target_is_uninspected_not_malformed_and_not_valid() {
    let resource = classify_compressed_section_entry(b"<< /GS << /Font [2 0 R 12] >> >>");

    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::CompressedTargetUninspected {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
            location: ObjectLookupLocation::XrefStreamCompressed {
                object_number: 2,
                object_stream_number: 9,
                index_within_object_stream: 3,
            },
        }
    );
}

#[test]
fn nonzero_generation_compressed_reference_is_a_generation_mismatch() {
    // Compressed objects have implicit generation zero, so `2 1 R` must be
    // the generation-mismatch outcome, never a possible compressed target.
    let resource = classify_compressed_section_entry(b"<< /GS << /Font [2 1 R 12] >> >>");

    assert_eq!(
        resource.font_effect,
        ExtGStateFontEffect::UnresolvedTarget {
            reference: IndirectRef {
                object_number: 2,
                generation: 1,
            },
            location: None,
        }
    );
}

fn effect_map(effect: &str, mut fields: Vec<(String, TestSerdeValue)>) -> TestSerdeValue {
    let mut entries = vec![(
        "effect".to_string(),
        TestSerdeValue::String(effect.to_string()),
    )];
    entries.append(&mut fields);
    TestSerdeValue::Map(entries)
}

fn kind_map(kind: &str) -> TestSerdeValue {
    TestSerdeValue::Map(vec![(
        "kind".to_string(),
        TestSerdeValue::String(kind.to_string()),
    )])
}

fn reference_value(object_number: u64, generation: u64) -> TestSerdeValue {
    TestSerdeValue::Map(vec![
        (
            "object_number".to_string(),
            TestSerdeValue::U64(object_number),
        ),
        ("generation".to_string(), TestSerdeValue::U64(generation)),
    ])
}

// One exhaustive snapshot over the whole 14-variant effect vocabulary is the
// point of this test; splitting it would hide that exhaustiveness.
#[allow(clippy::too_many_lines)]
#[test]
fn font_effect_serde_tags_are_locked_for_all_fourteen_variants() {
    let range = DictionaryEntryByteRange { start: 3, end: 11 };
    let reference = IndirectRef {
        object_number: 2,
        generation: 0,
    };
    let vocabulary = vec![
        ExtGStateFontEffect::Unset,
        ExtGStateFontEffect::DuplicateKey {
            first_key_range: range,
            duplicate_key_range: range,
        },
        ExtGStateFontEffect::NonArrayValue {
            value_kind: DictionaryValueKind::NumberLike,
        },
        ExtGStateFontEffect::MalformedFontReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        },
        ExtGStateFontEffect::MalformedSize { size_range: range },
        ExtGStateFontEffect::NonFiniteSize { size_range: range },
        ExtGStateFontEffect::IndirectSizeUnsupported { reference },
        ExtGStateFontEffect::ExtraArrayElements {
            trailing_range: range,
        },
        ExtGStateFontEffect::UnresolvedTarget {
            reference,
            location: None,
        },
        ExtGStateFontEffect::CompressedTargetUninspected {
            reference,
            location: ObjectLookupLocation::XrefStreamCompressed {
                object_number: 2,
                object_stream_number: 9,
                index_within_object_stream: 3,
            },
        },
        ExtGStateFontEffect::TargetDictionaryFailed {
            reference,
            object_byte_offset: 9,
            error: IndirectObjectDictionaryInspectionError {
                byte_offset: 9,
                byte_len: 40,
                header_byte_offset: Some(9),
                error_byte_offset: None,
                reason: IndirectObjectDictionaryInspectionRejection::DictionaryEntries {
                    dictionary_entries_reason: DictionaryEntryInspectionRejection::MissingValue,
                },
            },
        },
        ExtGStateFontEffect::TargetHeaderMismatch {
            reference,
            object_byte_offset: 9,
            header_reference: IndirectRef {
                object_number: 3,
                generation: 0,
            },
        },
        ExtGStateFontEffect::InadmissibleTarget {
            reference,
            object_byte_offset: 120,
            size_bits: 12.0f64.to_bits(),
            dictionary_type: FontDictionaryTypeFact::Font,
            subtype: FontSubtypeClass::CidFontType0,
        },
        ExtGStateFontEffect::StructurallyValid {
            reference,
            object_byte_offset: 120,
            size_bits: (-0.0f64).to_bits(),
            dictionary_type: FontDictionaryTypeFact::Font,
            subtype: FontSubtypeClass::Type0,
        },
    ];

    let value = serde_value(&vocabulary).expect("effect vocabulary should serialize");

    let range_value = TestSerdeValue::Map(vec![
        ("start".to_string(), TestSerdeValue::U64(3)),
        ("end".to_string(), TestSerdeValue::U64(11)),
    ]);
    assert_eq!(
        value,
        TestSerdeValue::Seq(vec![
            effect_map("unset", Vec::new()),
            effect_map(
                "duplicate_key",
                vec![
                    ("first_key_range".to_string(), range_value.clone()),
                    ("duplicate_key_range".to_string(), range_value.clone()),
                ],
            ),
            effect_map(
                "non_array_value",
                vec![(
                    "value_kind".to_string(),
                    TestSerdeValue::String("number_like".to_string()),
                )],
            ),
            effect_map(
                "malformed_font_reference",
                vec![(
                    "reference_reason".to_string(),
                    TestSerdeValue::Map(vec![(
                        "reason".to_string(),
                        TestSerdeValue::String("malformed_reference".to_string()),
                    )]),
                )],
            ),
            effect_map(
                "malformed_size",
                vec![("size_range".to_string(), range_value.clone())],
            ),
            effect_map(
                "non_finite_size",
                vec![("size_range".to_string(), range_value.clone())],
            ),
            effect_map(
                "indirect_size_unsupported",
                vec![("reference".to_string(), reference_value(2, 0))],
            ),
            effect_map(
                "extra_array_elements",
                vec![("trailing_range".to_string(), range_value)],
            ),
            effect_map(
                "unresolved_target",
                vec![
                    ("reference".to_string(), reference_value(2, 0)),
                    ("location".to_string(), TestSerdeValue::None),
                ],
            ),
            effect_map(
                "compressed_target_uninspected",
                vec![
                    ("reference".to_string(), reference_value(2, 0)),
                    (
                        "location".to_string(),
                        TestSerdeValue::Map(vec![
                            (
                                "location".to_string(),
                                TestSerdeValue::String("xref_stream_compressed".to_string()),
                            ),
                            ("object_number".to_string(), TestSerdeValue::U64(2)),
                            ("object_stream_number".to_string(), TestSerdeValue::U64(9)),
                            (
                                "index_within_object_stream".to_string(),
                                TestSerdeValue::U64(3),
                            ),
                        ]),
                    ),
                ],
            ),
            effect_map(
                "target_dictionary_failed",
                vec![
                    ("reference".to_string(), reference_value(2, 0)),
                    ("object_byte_offset".to_string(), TestSerdeValue::U64(9)),
                    (
                        "error".to_string(),
                        TestSerdeValue::Map(vec![
                            ("byte_offset".to_string(), TestSerdeValue::U64(9)),
                            ("byte_len".to_string(), TestSerdeValue::U64(40)),
                            (
                                "header_byte_offset".to_string(),
                                TestSerdeValue::Some(Box::new(TestSerdeValue::U64(9))),
                            ),
                            ("error_byte_offset".to_string(), TestSerdeValue::None),
                            (
                                "reason".to_string(),
                                TestSerdeValue::Map(vec![
                                    (
                                        "reason".to_string(),
                                        TestSerdeValue::String("dictionary_entries".to_string()),
                                    ),
                                    (
                                        "dictionary_entries_reason".to_string(),
                                        TestSerdeValue::Map(vec![(
                                            "reason".to_string(),
                                            TestSerdeValue::String("missing_value".to_string()),
                                        )]),
                                    ),
                                ]),
                            ),
                        ]),
                    ),
                ],
            ),
            effect_map(
                "target_header_mismatch",
                vec![
                    ("reference".to_string(), reference_value(2, 0)),
                    ("object_byte_offset".to_string(), TestSerdeValue::U64(9)),
                    ("header_reference".to_string(), reference_value(3, 0)),
                ],
            ),
            effect_map(
                "inadmissible_target",
                vec![
                    ("reference".to_string(), reference_value(2, 0)),
                    ("object_byte_offset".to_string(), TestSerdeValue::U64(120)),
                    (
                        "size_bits".to_string(),
                        TestSerdeValue::U64(12.0f64.to_bits()),
                    ),
                    ("dictionary_type".to_string(), kind_map("font")),
                    ("subtype".to_string(), kind_map("cid_font_type0")),
                ],
            ),
            effect_map(
                "structurally_valid",
                vec![
                    ("reference".to_string(), reference_value(2, 0)),
                    ("object_byte_offset".to_string(), TestSerdeValue::U64(120)),
                    (
                        "size_bits".to_string(),
                        TestSerdeValue::U64(0x8000_0000_0000_0000),
                    ),
                    ("dictionary_type".to_string(), kind_map("font")),
                    ("subtype".to_string(), kind_map("type0")),
                ],
            ),
        ])
    );

    let decoded: Vec<ExtGStateFontEffect> =
        from_serde_value(value).expect("effect vocabulary should deserialize");
    assert_eq!(decoded, vocabulary);
}

#[test]
fn classified_resource_serde_appends_font_effect_and_defaults_when_absent() {
    let resource = classify_direct(b"<< /GS << /OP true >> >>");

    let value = serde_value(&resource).expect("classified resource should serialize");
    let unset_param = TestSerdeValue::Map(vec![(
        "class".to_string(),
        TestSerdeValue::String("unset".to_string()),
    )]);
    assert_eq!(
        value,
        TestSerdeValue::Map(vec![
            (
                "name".to_string(),
                TestSerdeValue::Seq(vec![
                    TestSerdeValue::U64(u64::from(b'G')),
                    TestSerdeValue::U64(u64::from(b'S')),
                ]),
            ),
            (
                "op_stroking".to_string(),
                TestSerdeValue::Map(vec![
                    (
                        "class".to_string(),
                        TestSerdeValue::String("set".to_string()),
                    ),
                    ("value".to_string(), TestSerdeValue::Bool(true)),
                ]),
            ),
            ("op_nonstroking".to_string(), unset_param.clone()),
            ("overprint_mode".to_string(), unset_param.clone()),
            ("stroking_alpha".to_string(), unset_param.clone()),
            ("nonstroking_alpha".to_string(), unset_param.clone()),
            ("blend_mode".to_string(), unset_param.clone()),
            ("soft_mask".to_string(), unset_param),
            (
                "has_unclassified_keys".to_string(),
                TestSerdeValue::Bool(false),
            ),
            ("font_effect".to_string(), effect_map("unset", Vec::new())),
        ])
    );

    // Older report JSON without the trailing `font_effect` field must
    // deserialize to the defaulted `Unset` effect.
    let entries = match value {
        TestSerdeValue::Map(entries) => Some(entries),
        _ => None,
    };
    let mut entries = entries.expect("classified resource should serialize as a map");
    let (last_key, _) = entries.pop().expect("map should carry entries");
    assert_eq!(last_key, "font_effect");
    let decoded: ClassifiedExtGStateResource =
        from_serde_value(TestSerdeValue::Map(entries)).expect("older shape should deserialize");
    assert_eq!(decoded, resource);
}
