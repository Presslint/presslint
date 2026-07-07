use crate::{
    ClassicXrefTableInspection, ObjectLookup, PdfName, SkippedTransparencyGroupReason,
    TransparencyGroupColorSpace, TransparencyGroupParamClass, classify_transparency_group_entry,
    inspect_classic_xref_table, inspect_indirect_object_dictionary,
};

struct Fixture {
    source: Vec<u8>,
    xref: ClassicXrefTableInspection,
}

impl Fixture {
    fn lookup(&self) -> ObjectLookup<'_> {
        ObjectLookup::ClassicXref(&self.xref)
    }
}

fn fixture(body: &[u8]) -> Fixture {
    let mut source = b"%PDF-1.7\n1 0 obj\n".to_vec();
    source.extend_from_slice(body);
    source.extend_from_slice(b"\nendobj\n");
    let xref_offset = source.len();
    source.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
    source.extend_from_slice(b"0000000009 00000 n \n");
    source.extend_from_slice(
        format!("trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
    );
    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    Fixture { source, xref }
}

fn classify(
    dict: &[u8],
) -> Result<Option<crate::ClassifiedTransparencyGroup>, crate::SkippedTransparencyGroup> {
    let fixture = fixture(dict);
    let inspection =
        inspect_indirect_object_dictionary(&fixture.source, 9).expect("dictionary should inspect");
    classify_transparency_group_entry(&fixture.source, fixture.lookup(), 9, &inspection.entries)
}

#[test]
fn classifies_transparency_group_fields_without_defaults() {
    let group = classify(b"<< /Group << /S /Transparency /CS /DeviceCMYK /I true /K false >> >>")
        .expect("group should classify")
        .expect("group should be present");

    assert!(group.transparency);
    assert_eq!(
        group.color_space,
        TransparencyGroupParamClass::Set {
            value: TransparencyGroupColorSpace::Name {
                raw_name: PdfName(b"DeviceCMYK".to_vec()),
            },
        }
    );
    assert_eq!(
        group.isolated,
        TransparencyGroupParamClass::Set { value: true }
    );
    assert_eq!(
        group.knockout,
        TransparencyGroupParamClass::Set { value: false }
    );
}

#[test]
fn absent_optional_fields_remain_unset() {
    let group = classify(b"<< /Group << /S /Transparency >> >>")
        .expect("group should classify")
        .expect("group should be present");

    assert_eq!(group.color_space, TransparencyGroupParamClass::Unset);
    assert_eq!(group.isolated, TransparencyGroupParamClass::Unset);
    assert_eq!(group.knockout, TransparencyGroupParamClass::Unset);
}

#[test]
fn duplicate_color_space_is_unclassified_safety_field() {
    let group = classify(b"<< /Group << /S /Transparency /CS /DeviceCMYK /CS /DeviceRGB >> >>")
        .expect("group should classify")
        .expect("group should be present");

    assert!(matches!(
        group.color_space,
        TransparencyGroupParamClass::Duplicate {
            first_key_range,
            duplicate_key_range,
        } if first_key_range.start < duplicate_key_range.start
    ));
    assert!(group.has_unclassified_safety_field());
}

#[test]
fn duplicate_isolated_flag_is_unclassified_safety_field() {
    let group = classify(b"<< /Group << /S /Transparency /I 42 /I false >> >>")
        .expect("group should classify")
        .expect("group should be present");

    assert!(matches!(
        group.isolated,
        TransparencyGroupParamClass::Duplicate {
            first_key_range,
            duplicate_key_range,
        } if first_key_range.start < duplicate_key_range.start
    ));
    assert!(group.has_unclassified_safety_field());
}

#[test]
fn duplicate_knockout_flag_is_unclassified_safety_field() {
    let group = classify(b"<< /Group << /S /Transparency /K true /K false >> >>")
        .expect("group should classify")
        .expect("group should be present");

    assert!(matches!(
        group.knockout,
        TransparencyGroupParamClass::Duplicate {
            first_key_range,
            duplicate_key_range,
        } if first_key_range.start < duplicate_key_range.start
    ));
    assert!(group.has_unclassified_safety_field());
}

#[test]
fn malformed_group_value_is_structured_diagnostic() {
    let skip = classify(b"<< /Group 42 >>").expect_err("numeric group should skip");

    assert!(matches!(
        skip.reason,
        SkippedTransparencyGroupReason::NonDictionaryGroup { .. }
    ));
}

#[test]
fn non_transparency_group_is_structured_diagnostic() {
    let skip =
        classify(b"<< /Group << /S /Other >> >>").expect_err("non-transparency group should skip");

    assert!(matches!(
        skip.reason,
        SkippedTransparencyGroupReason::NonTransparencySubtype {
            raw_name: PdfName(ref name),
        } if name == b"Other"
    ));
}
