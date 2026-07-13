#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use crate::{
    ClassicXrefTableInspection, ClassifiedFontResource, DictionaryEntryByteRange,
    DictionaryEntryInspectionError, DictionaryEntryInspectionRejection, DictionaryValueKind,
    FontDictionaryTypeFact, FontSubtypeClass, IndirectObjectDictionaryInspectionError,
    IndirectObjectDictionaryInspectionRejection, IndirectRef, IndirectReferenceInspectionRejection,
    ObjectLookup, ObjectLookupLocation, PdfName, SkippedFontResource, SkippedFontResourceReason,
    SkippedPageXObjectResourceReason, classify_font_entry, inspect_classic_xref_table,
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

fn classify_direct(dict: &[u8]) -> ClassifiedFontResource {
    let pdf = fixture(&[]);
    let entries = inspect_dictionary_entries(dict, 0).expect("outer dictionary should inspect");
    let entry = entries.entries[0];
    let name = PdfName(dict[entry.key_range.start + 1..entry.key_range.end].to_vec());
    classify_font_entry(dict, pdf.lookup(), &name, entry).expect("font entry should classify")
}

/// Classify the first `/Font`-style entry of object 1 in a full fixture.
fn classify_object_entry(
    pdf: &Fixture,
) -> Result<ClassifiedFontResource, SkippedFontResourceReason> {
    let dictionary = inspect_indirect_object_dictionary(&pdf.source, pdf.object_offset(1))
        .expect("object 1 dictionary should inspect");
    let entry = dictionary.entries[0];
    let name = PdfName(pdf.source[entry.key_range.start + 1..entry.key_range.end].to_vec());
    classify_font_entry(&pdf.source, pdf.lookup(), &name, entry)
}

#[test]
fn decoded_font_resource_name_collisions_poison_without_rewriting_raw_names() {
    let input =
        b"<< /F1 << /Type /Font /Subtype /Type1 >> /F#31 << /Type /Font /Subtype /Type1 >> >>";
    let entries = inspect_dictionary_entries(input, 0).expect("font dictionary should inspect");
    let pdf = fixture(&[]);
    let mut skipped = Vec::new();
    let fonts = crate::font_classify::classify_font_entries(
        input,
        pdf.lookup(),
        17,
        entries.entries,
        &mut skipped,
    );

    assert_eq!(fonts.len(), 1);
    assert_eq!(fonts[0].name, PdfName(b"F1".to_vec()));
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].resource_name, Some(PdfName(b"F#31".to_vec())));
    assert!(matches!(
        skipped[0].reason,
        SkippedFontResourceReason::DuplicateFontName { .. }
    ));
}

#[test]
fn malformed_or_null_pdf_name_escapes_fail_closed() {
    for raw in [
        b"F#".as_slice(),
        b"F#3".as_slice(),
        b"F#zz".as_slice(),
        b"F#00".as_slice(),
        b"F\0".as_slice(),
    ] {
        assert!(crate::source_utils::decode_pdf_name(raw).is_none());
    }
    assert_eq!(
        crate::source_utils::decode_pdf_name(b"F#31")
            .expect("valid escape")
            .as_ref(),
        b"F1"
    );
}

#[test]
fn classifies_all_five_legal_tf_subtypes_exactly() {
    let cases: &[(&[u8], FontSubtypeClass)] = &[
        (b"Type1", FontSubtypeClass::Type1),
        (b"MMType1", FontSubtypeClass::MmType1),
        (b"TrueType", FontSubtypeClass::TrueType),
        (b"Type0", FontSubtypeClass::Type0),
        (b"Type3", FontSubtypeClass::Type3),
    ];
    for (raw, expected) in cases {
        let mut dict = b"<< /F1 << /Type /Font /Subtype /".to_vec();
        dict.extend_from_slice(raw);
        dict.extend_from_slice(b" >> >>");
        let resource = classify_direct(&dict);
        assert_eq!(&resource.subtype, expected);
        assert_eq!(resource.dictionary_type, FontDictionaryTypeFact::Font);
        assert_eq!(resource.name, PdfName(b"F1".to_vec()));
        assert_eq!(resource.reference, None);
        assert_eq!(resource.object_byte_offset, None);
    }
}

#[test]
fn cid_subtypes_are_distinct_variants_and_never_type0() {
    let cid0 = classify_direct(b"<< /F1 << /Subtype /CIDFontType0 >> >>");
    let cid2 = classify_direct(b"<< /F1 << /Subtype /CIDFontType2 >> >>");

    assert_eq!(cid0.subtype, FontSubtypeClass::CidFontType0);
    assert_eq!(cid2.subtype, FontSubtypeClass::CidFontType2);
    assert_ne!(cid0.subtype, FontSubtypeClass::Type0);
    assert_ne!(cid2.subtype, FontSubtypeClass::Type0);
}

#[test]
fn type1c_and_unknown_subtype_names_are_other_name_never_collapsed() {
    let type1c = classify_direct(b"<< /F1 << /Subtype /Type1C >> >>");
    let unknown = classify_direct(b"<< /F1 << /Subtype /OpenType >> >>");

    assert_eq!(
        type1c.subtype,
        FontSubtypeClass::OtherName {
            name: PdfName(b"Type1C".to_vec()),
        }
    );
    assert_eq!(
        unknown.subtype,
        FontSubtypeClass::OtherName {
            name: PdfName(b"OpenType".to_vec()),
        }
    );
}

#[test]
fn missing_subtype_and_missing_type_stay_classified_fail_closed() {
    let resource = classify_direct(b"<< /F1 << /BaseFont /Helvetica >> >>");

    assert_eq!(resource.subtype, FontSubtypeClass::Missing);
    assert_eq!(resource.dictionary_type, FontDictionaryTypeFact::Missing);
}

#[test]
fn type_fact_records_other_name_and_non_name_without_guessing() {
    let other = classify_direct(b"<< /F1 << /Type /XObject /Subtype /Type1 >> >>");
    let non_name = classify_direct(b"<< /F1 << /Type (Font) /Subtype /Type1 >> >>");

    assert_eq!(
        other.dictionary_type,
        FontDictionaryTypeFact::OtherName {
            name: PdfName(b"XObject".to_vec()),
        }
    );
    assert_eq!(other.subtype, FontSubtypeClass::Type1);
    assert_eq!(
        non_name.dictionary_type,
        FontDictionaryTypeFact::NonName {
            value_kind: DictionaryValueKind::String,
        }
    );
    assert_eq!(non_name.subtype, FontSubtypeClass::Type1);
}

#[test]
fn duplicate_type_and_subtype_keys_fail_closed_with_key_ranges() {
    let dict: &[u8] = b"<< /F1 << /Type /Font /Type /Font /Subtype /Type1 /Subtype /Type3 >> >>";
    let resource = classify_direct(dict);

    let type_ranges = match resource.dictionary_type {
        FontDictionaryTypeFact::Duplicate {
            first_key_range,
            duplicate_key_range,
        } => Some((first_key_range, duplicate_key_range)),
        _ => None,
    };
    let (first_key_range, duplicate_key_range) =
        type_ranges.expect("duplicate /Type should classify as Duplicate");
    assert_eq!(&dict[first_key_range.start..first_key_range.end], b"/Type");
    assert_eq!(
        &dict[duplicate_key_range.start..duplicate_key_range.end],
        b"/Type"
    );

    let subtype_ranges = match resource.subtype {
        FontSubtypeClass::Duplicate {
            first_key_range,
            duplicate_key_range,
        } => Some((first_key_range, duplicate_key_range)),
        _ => None,
    };
    let (first_key_range, duplicate_key_range) =
        subtype_ranges.expect("duplicate /Subtype should classify as Duplicate");
    assert_eq!(
        &dict[first_key_range.start..first_key_range.end],
        b"/Subtype"
    );
    assert_eq!(
        &dict[duplicate_key_range.start..duplicate_key_range.end],
        b"/Subtype"
    );
}

#[test]
fn indirect_subtype_is_non_name_class_and_never_resolved() {
    // Object 3 is a resolvable name object; a correct classifier must still
    // refuse to chase the indirect /Subtype and report the shallow shape.
    let pdf = fixture(&[
        b"1 0 obj\n<< /F1 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype 3 0 R >>\nendobj\n",
        b"3 0 obj\n/Type1\nendobj\n",
    ]);

    let resource = classify_object_entry(&pdf).expect("indirect font entry should classify");

    assert_eq!(
        resource.subtype,
        FontSubtypeClass::NonName {
            value_kind: DictionaryValueKind::IndirectReferenceLike,
        }
    );
    assert_eq!(resource.dictionary_type, FontDictionaryTypeFact::Font);
}

#[test]
fn indirect_entry_classifies_with_resolved_target_identity() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /F1 2 0 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype /TrueType >>\nendobj\n",
    ]);

    let resource = classify_object_entry(&pdf).expect("indirect font entry should classify");

    assert_eq!(resource.subtype, FontSubtypeClass::TrueType);
    assert_eq!(
        resource.reference,
        Some(IndirectRef {
            object_number: 2,
            generation: 0,
        })
    );
    assert_eq!(resource.object_byte_offset, Some(pdf.object_offset(2)));
}

#[test]
fn unresolved_and_generation_mismatch_references_fail_closed() {
    let unresolved = fixture(&[b"1 0 obj\n<< /F1 99 0 R >>\nendobj\n"]);
    let reason = classify_object_entry(&unresolved)
        .expect_err("unresolved reference should be a structured skip");
    assert!(matches!(
        reason,
        SkippedFontResourceReason::UnresolvedResourceReference {
            location: Some(_),
            ..
        }
    ));

    let mismatch = fixture(&[
        b"1 0 obj\n<< /F1 2 1 R >>\nendobj\n",
        b"2 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n",
    ]);
    let reason = classify_object_entry(&mismatch)
        .expect_err("generation mismatch should be a structured skip");
    assert!(matches!(
        reason,
        SkippedFontResourceReason::UnresolvedResourceReference { location: None, .. }
    ));
}

#[test]
fn non_dictionary_entry_and_target_fail_closed() {
    let array_entry = fixture(&[b"1 0 obj\n<< /F1 [ 1 2 ] >>\nendobj\n"]);
    let reason =
        classify_object_entry(&array_entry).expect_err("array entry should be a structured skip");
    assert_eq!(
        reason,
        SkippedFontResourceReason::NonDictionaryEntry {
            value_kind: DictionaryValueKind::Array,
        }
    );

    let integer_target = fixture(&[
        b"1 0 obj\n<< /F1 2 0 R >>\nendobj\n",
        b"2 0 obj\n42\nendobj\n",
    ]);
    let reason = classify_object_entry(&integer_target)
        .expect_err("non-dictionary target should be a structured skip");
    assert!(matches!(
        reason,
        SkippedFontResourceReason::ResourceDictionaryFailed { .. }
    ));
}

#[test]
fn raw_names_are_preserved_without_escape_decoding() {
    let resource = classify_direct(b"<< /F#31 << /Subtype /Type#31 >> >>");

    assert_eq!(resource.name, PdfName(b"F#31".to_vec()));
    assert_eq!(
        resource.subtype,
        FontSubtypeClass::OtherName {
            name: PdfName(b"Type#31".to_vec()),
        }
    );
}

fn kind_map(kind: &str) -> TestSerdeValue {
    TestSerdeValue::Map(vec![(
        "kind".to_string(),
        TestSerdeValue::String(kind.to_string()),
    )])
}

fn pdf_name_value(bytes: &[u8]) -> TestSerdeValue {
    TestSerdeValue::Seq(
        bytes
            .iter()
            .copied()
            .map(u64::from)
            .map(TestSerdeValue::U64)
            .collect(),
    )
}

#[test]
fn subtype_taxonomy_serde_tags_are_locked() {
    let range = DictionaryEntryByteRange { start: 3, end: 11 };
    let taxonomy = vec![
        FontSubtypeClass::Type1,
        FontSubtypeClass::MmType1,
        FontSubtypeClass::TrueType,
        FontSubtypeClass::Type0,
        FontSubtypeClass::Type3,
        FontSubtypeClass::CidFontType0,
        FontSubtypeClass::CidFontType2,
        FontSubtypeClass::OtherName {
            name: PdfName(b"Type1C".to_vec()),
        },
        FontSubtypeClass::Missing,
        FontSubtypeClass::Duplicate {
            first_key_range: range,
            duplicate_key_range: range,
        },
        FontSubtypeClass::NonName {
            value_kind: DictionaryValueKind::Array,
        },
    ];

    let value = serde_value(&taxonomy).expect("subtype taxonomy should serialize");

    let range_value = TestSerdeValue::Map(vec![
        ("start".to_string(), TestSerdeValue::U64(3)),
        ("end".to_string(), TestSerdeValue::U64(11)),
    ]);
    assert_eq!(
        value,
        TestSerdeValue::Seq(vec![
            kind_map("type1"),
            kind_map("mm_type1"),
            kind_map("true_type"),
            kind_map("type0"),
            kind_map("type3"),
            kind_map("cid_font_type0"),
            kind_map("cid_font_type2"),
            TestSerdeValue::Map(vec![
                (
                    "kind".to_string(),
                    TestSerdeValue::String("other_name".to_string()),
                ),
                ("name".to_string(), pdf_name_value(b"Type1C")),
            ]),
            kind_map("missing"),
            TestSerdeValue::Map(vec![
                (
                    "kind".to_string(),
                    TestSerdeValue::String("duplicate".to_string()),
                ),
                ("first_key_range".to_string(), range_value.clone()),
                ("duplicate_key_range".to_string(), range_value),
            ]),
            TestSerdeValue::Map(vec![
                (
                    "kind".to_string(),
                    TestSerdeValue::String("non_name".to_string()),
                ),
                (
                    "value_kind".to_string(),
                    TestSerdeValue::String("array".to_string()),
                ),
            ]),
        ])
    );

    let decoded: Vec<FontSubtypeClass> =
        from_serde_value(value).expect("subtype taxonomy should deserialize");
    assert_eq!(decoded, taxonomy);
}

#[test]
fn type_fact_serde_tags_are_locked() {
    let range = DictionaryEntryByteRange { start: 3, end: 11 };
    let taxonomy = vec![
        FontDictionaryTypeFact::Font,
        FontDictionaryTypeFact::Missing,
        FontDictionaryTypeFact::OtherName {
            name: PdfName(b"XObject".to_vec()),
        },
        FontDictionaryTypeFact::Duplicate {
            first_key_range: range,
            duplicate_key_range: range,
        },
        FontDictionaryTypeFact::NonName {
            value_kind: DictionaryValueKind::NumberLike,
        },
    ];

    let value = serde_value(&taxonomy).expect("type facts should serialize");

    let range_value = TestSerdeValue::Map(vec![
        ("start".to_string(), TestSerdeValue::U64(3)),
        ("end".to_string(), TestSerdeValue::U64(11)),
    ]);
    assert_eq!(
        value,
        TestSerdeValue::Seq(vec![
            kind_map("font"),
            kind_map("missing"),
            TestSerdeValue::Map(vec![
                (
                    "kind".to_string(),
                    TestSerdeValue::String("other_name".to_string()),
                ),
                ("name".to_string(), pdf_name_value(b"XObject")),
            ]),
            TestSerdeValue::Map(vec![
                (
                    "kind".to_string(),
                    TestSerdeValue::String("duplicate".to_string()),
                ),
                ("first_key_range".to_string(), range_value.clone()),
                ("duplicate_key_range".to_string(), range_value),
            ]),
            TestSerdeValue::Map(vec![
                (
                    "kind".to_string(),
                    TestSerdeValue::String("non_name".to_string()),
                ),
                (
                    "value_kind".to_string(),
                    TestSerdeValue::String("number_like".to_string()),
                ),
            ]),
        ])
    );

    let decoded: Vec<FontDictionaryTypeFact> =
        from_serde_value(value).expect("type facts should deserialize");
    assert_eq!(decoded, taxonomy);
}

#[test]
fn classified_resource_and_skip_serde_shapes_are_locked() {
    let resource = ClassifiedFontResource {
        name: PdfName(b"F1".to_vec()),
        dictionary_type: FontDictionaryTypeFact::Font,
        subtype: FontSubtypeClass::Type0,
        reference: Some(IndirectRef {
            object_number: 7,
            generation: 0,
        }),
        object_byte_offset: Some(120),
    };
    let skip = SkippedFontResource {
        object_byte_offset: 9,
        resource_name: Some(PdfName(b"F2".to_vec())),
        reason: SkippedFontResourceReason::MissingFont,
    };

    let resource_value = serde_value(&resource).expect("classified resource should serialize");
    assert_eq!(
        resource_value,
        TestSerdeValue::Map(vec![
            ("name".to_string(), pdf_name_value(b"F1")),
            ("dictionary_type".to_string(), kind_map("font")),
            ("subtype".to_string(), kind_map("type0")),
            (
                "reference".to_string(),
                TestSerdeValue::Some(Box::new(TestSerdeValue::Map(vec![
                    ("object_number".to_string(), TestSerdeValue::U64(7)),
                    ("generation".to_string(), TestSerdeValue::U64(0)),
                ]))),
            ),
            (
                "object_byte_offset".to_string(),
                TestSerdeValue::Some(Box::new(TestSerdeValue::U64(120))),
            ),
        ])
    );
    let decoded: ClassifiedFontResource =
        from_serde_value(resource_value).expect("classified resource should deserialize");
    assert_eq!(decoded, resource);

    let skip_value = serde_value(&skip).expect("skip should serialize");
    assert_eq!(
        skip_value,
        TestSerdeValue::Map(vec![
            ("object_byte_offset".to_string(), TestSerdeValue::U64(9)),
            (
                "resource_name".to_string(),
                TestSerdeValue::Some(Box::new(pdf_name_value(b"F2"))),
            ),
            (
                "reason".to_string(),
                TestSerdeValue::Map(vec![(
                    "reason".to_string(),
                    TestSerdeValue::String("missing_font".to_string()),
                )]),
            ),
        ])
    );
    let decoded: SkippedFontResource =
        from_serde_value(skip_value).expect("skip should deserialize");
    assert_eq!(decoded, skip);
}

fn reason_map(reason: &str, mut fields: Vec<(String, TestSerdeValue)>) -> TestSerdeValue {
    let mut entries = vec![(
        "reason".to_string(),
        TestSerdeValue::String(reason.to_string()),
    )];
    entries.append(&mut fields);
    TestSerdeValue::Map(entries)
}

// One exhaustive snapshot over the whole 11-variant skip vocabulary is the
// point of this test; splitting it would hide that exhaustiveness.
#[allow(clippy::too_many_lines)]
#[test]
fn skip_reason_serde_tags_are_locked_for_all_eleven_variants() {
    let range = DictionaryEntryByteRange { start: 3, end: 11 };
    let vocabulary = vec![
        SkippedFontResourceReason::Resources {
            resources_reason: SkippedPageXObjectResourceReason::MissingResources,
        },
        SkippedFontResourceReason::MissingFontResources,
        SkippedFontResourceReason::MissingFont,
        SkippedFontResourceReason::DuplicateFont {
            first_key_range: range,
            duplicate_key_range: range,
        },
        SkippedFontResourceReason::NonDictionaryFont {
            value_kind: DictionaryValueKind::Array,
        },
        SkippedFontResourceReason::FontDictionaryFailed {
            error: DictionaryEntryInspectionError {
                byte_offset: 5,
                byte_len: 40,
                dictionary_open_byte_offset: None,
                error_byte_offset: None,
                reason: DictionaryEntryInspectionRejection::NonNameTopLevelKey,
            },
        },
        SkippedFontResourceReason::DuplicateFontName {
            first_key_range: range,
            duplicate_key_range: range,
        },
        SkippedFontResourceReason::NonDictionaryEntry {
            value_kind: DictionaryValueKind::String,
        },
        SkippedFontResourceReason::MalformedResourceReference {
            reference_reason: IndirectReferenceInspectionRejection::OffsetOutOfBounds,
        },
        SkippedFontResourceReason::UnresolvedResourceReference {
            reference: IndirectRef {
                object_number: 7,
                generation: 0,
            },
            location: Some(ObjectLookupLocation::ClassicInUse {
                object_number: 7,
                generation: 0,
                byte_offset: 120,
            }),
        },
        SkippedFontResourceReason::ResourceDictionaryFailed {
            reference: IndirectRef {
                object_number: 2,
                generation: 0,
            },
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
    ];

    let value = serde_value(&vocabulary).expect("skip vocabulary should serialize");

    let range_value = TestSerdeValue::Map(vec![
        ("start".to_string(), TestSerdeValue::U64(3)),
        ("end".to_string(), TestSerdeValue::U64(11)),
    ]);
    let reference_value = |object_number: u64| {
        TestSerdeValue::Map(vec![
            (
                "object_number".to_string(),
                TestSerdeValue::U64(object_number),
            ),
            ("generation".to_string(), TestSerdeValue::U64(0)),
        ])
    };
    assert_eq!(
        value,
        TestSerdeValue::Seq(vec![
            reason_map(
                "resources",
                vec![(
                    "resources_reason".to_string(),
                    reason_map("missing_resources", Vec::new()),
                )],
            ),
            reason_map("missing_font_resources", Vec::new()),
            reason_map("missing_font", Vec::new()),
            reason_map(
                "duplicate_font",
                vec![
                    ("first_key_range".to_string(), range_value.clone()),
                    ("duplicate_key_range".to_string(), range_value.clone()),
                ],
            ),
            reason_map(
                "non_dictionary_font",
                vec![(
                    "value_kind".to_string(),
                    TestSerdeValue::String("array".to_string()),
                )],
            ),
            reason_map(
                "font_dictionary_failed",
                vec![(
                    "error".to_string(),
                    TestSerdeValue::Map(vec![
                        ("byte_offset".to_string(), TestSerdeValue::U64(5)),
                        ("byte_len".to_string(), TestSerdeValue::U64(40)),
                        (
                            "dictionary_open_byte_offset".to_string(),
                            TestSerdeValue::None,
                        ),
                        ("error_byte_offset".to_string(), TestSerdeValue::None),
                        (
                            "reason".to_string(),
                            reason_map("non_name_top_level_key", Vec::new()),
                        ),
                    ]),
                )],
            ),
            reason_map(
                "duplicate_font_name",
                vec![
                    ("first_key_range".to_string(), range_value.clone()),
                    ("duplicate_key_range".to_string(), range_value),
                ],
            ),
            reason_map(
                "non_dictionary_entry",
                vec![(
                    "value_kind".to_string(),
                    TestSerdeValue::String("string".to_string()),
                )],
            ),
            reason_map(
                "malformed_resource_reference",
                vec![(
                    "reference_reason".to_string(),
                    reason_map("offset_out_of_bounds", Vec::new()),
                )],
            ),
            reason_map(
                "unresolved_resource_reference",
                vec![
                    ("reference".to_string(), reference_value(7)),
                    (
                        "location".to_string(),
                        TestSerdeValue::Some(Box::new(TestSerdeValue::Map(vec![
                            (
                                "location".to_string(),
                                TestSerdeValue::String("classic_in_use".to_string()),
                            ),
                            ("object_number".to_string(), TestSerdeValue::U64(7)),
                            ("generation".to_string(), TestSerdeValue::U64(0)),
                            ("byte_offset".to_string(), TestSerdeValue::U64(120)),
                        ]))),
                    ),
                ],
            ),
            reason_map(
                "resource_dictionary_failed",
                vec![
                    ("reference".to_string(), reference_value(2)),
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
                                reason_map(
                                    "dictionary_entries",
                                    vec![(
                                        "dictionary_entries_reason".to_string(),
                                        reason_map("missing_value", Vec::new()),
                                    )],
                                ),
                            ),
                        ]),
                    ),
                ],
            ),
        ])
    );

    let decoded: Vec<SkippedFontResourceReason> =
        from_serde_value(value).expect("skip vocabulary should deserialize");
    assert_eq!(decoded, vocabulary);
}
