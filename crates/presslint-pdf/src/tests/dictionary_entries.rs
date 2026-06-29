use crate::{
    DictionaryEntryInspection, DictionaryEntryInspectionRejection, DictionaryValueKind,
    inspect_classic_xref_table, inspect_classic_xref_trailer_dictionary,
    inspect_dictionary_entries,
};

#[test]
fn flat_dictionary_reports_top_level_entry_ranges_and_kinds() {
    let source = b"<< /Size 2 /Type /Catalog /Open true /Gone null >>";

    let report = inspect_dictionary_entries(source, 0).expect("entries should inspect");

    assert_eq!(report.dictionary.open_byte_offset, 0);
    assert_eq!(report.dictionary.after_close_byte_offset, source.len());
    assert_eq!(report.entries.len(), 4);
    assert_entry(
        &report,
        source,
        0,
        b"/Size",
        b"2",
        DictionaryValueKind::NumberLike,
    );
    assert_entry(
        &report,
        source,
        1,
        b"/Type",
        b"/Catalog",
        DictionaryValueKind::Name,
    );
    assert_entry(
        &report,
        source,
        2,
        b"/Open",
        b"true",
        DictionaryValueKind::Boolean,
    );
    assert_entry(
        &report,
        source,
        3,
        b"/Gone",
        b"null",
        DictionaryValueKind::Null,
    );
}

#[test]
fn nested_dictionary_value_is_one_opaque_top_level_value() {
    let source = b"<< /Root << /Type /Catalog /Nested (not /Top) >> /Size 1 >>";

    let report = inspect_dictionary_entries(source, 0).expect("entries should inspect");

    assert_eq!(report.entries.len(), 2);
    assert_entry(
        &report,
        source,
        0,
        b"/Root",
        b"<< /Type /Catalog /Nested (not /Top) >>",
        DictionaryValueKind::Dictionary,
    );
    assert_entry(
        &report,
        source,
        1,
        b"/Size",
        b"1",
        DictionaryValueKind::NumberLike,
    );
}

#[test]
fn array_value_is_one_opaque_top_level_value() {
    let source = b"<< /Kids [ 1 0 R << /Name /NotTop >> (also /NotTop) ] /Count 1 >>";

    let report = inspect_dictionary_entries(source, 0).expect("entries should inspect");

    assert_eq!(report.entries.len(), 2);
    assert_entry(
        &report,
        source,
        0,
        b"/Kids",
        b"[ 1 0 R << /Name /NotTop >> (also /NotTop) ]",
        DictionaryValueKind::Array,
    );
    assert_entry(
        &report,
        source,
        1,
        b"/Count",
        b"1",
        DictionaryValueKind::NumberLike,
    );
}

#[test]
fn strings_and_comments_shield_delimiters_while_scanning_entries() {
    let source = b"<< /Title (not /Next >>) /N 1 % /Fake 2 >>\n/Real <2F5265616C> >>";

    let report = inspect_dictionary_entries(source, 0).expect("entries should inspect");

    assert_eq!(report.entries.len(), 3);
    assert_entry(
        &report,
        source,
        0,
        b"/Title",
        b"(not /Next >>)",
        DictionaryValueKind::String,
    );
    assert_entry(
        &report,
        source,
        1,
        b"/N",
        b"1",
        DictionaryValueKind::NumberLike,
    );
    assert_entry(
        &report,
        source,
        2,
        b"/Real",
        b"<2F5265616C>",
        DictionaryValueKind::String,
    );
}

#[test]
fn indirect_reference_shaped_scalar_is_one_value_span() {
    let source = b"<< /Root 12 0 R /Size 2 >>";

    let report = inspect_dictionary_entries(source, 0).expect("entries should inspect");

    assert_eq!(report.entries.len(), 2);
    assert_entry(
        &report,
        source,
        0,
        b"/Root",
        b"12 0 R",
        DictionaryValueKind::IndirectReferenceLike,
    );
    assert_entry(
        &report,
        source,
        1,
        b"/Size",
        b"2",
        DictionaryValueKind::NumberLike,
    );
}

#[test]
fn missing_value_is_rejected() {
    let source = b"<< /Size >>";

    let error = inspect_dictionary_entries(source, 0).expect_err("missing value should reject");

    assert_eq!(
        error.reason,
        DictionaryEntryInspectionRejection::MissingValue
    );
    assert_eq!(error.dictionary_open_byte_offset, Some(0));
    assert_eq!(error.error_byte_offset, Some(source.len() - 2));
}

#[test]
fn malformed_non_name_top_level_key_is_rejected() {
    let source = b"<< 123 /Size 2 >>";

    let error = inspect_dictionary_entries(source, 0).expect_err("non-name key should reject");

    assert_eq!(
        error.reason,
        DictionaryEntryInspectionRejection::NonNameTopLevelKey
    );
    assert_eq!(error.error_byte_offset, Some(3));
}

#[test]
fn delegated_array_extent_rejection_is_structured() {
    let source = b"<< /Broken [ 1 2 >>";

    let error = inspect_dictionary_entries(source, 0).expect_err("array should reject");

    assert!(matches!(
        error.reason,
        DictionaryEntryInspectionRejection::ArrayExtent { .. }
    ));
    assert_eq!(error.dictionary_open_byte_offset, Some(0));
}

#[test]
fn report_does_not_retain_dictionary_bytes() {
    let source = b"<< /Secret (corpus-detail not copied) >>";

    let report = inspect_dictionary_entries(source, 0).expect("entries should inspect");

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("Secret"));
    assert!(!debug_report.contains("corpus-detail"));
}

#[test]
fn dictionary_entry_scanner_composes_with_classic_xref_trailer_dictionary() {
    let source = b"%PDF-1.7\nxref\n0 2\n0000000000 65535 f \n0000000017 00000 n \ntrailer\n<< /Size 2 /Root 1 0 R /Prev 123 >>\nstartxref\n9\n%%EOF\n";
    let xref_offset = source
        .windows(b"xref".len())
        .position(|window| window == b"xref")
        .expect("xref exists");

    let xref_report = inspect_classic_xref_table(source, xref_offset).expect("xref should inspect");
    let trailer_report =
        inspect_classic_xref_trailer_dictionary(source, xref_report.trailer_byte_offset)
            .expect("trailer dictionary should inspect");
    let entries = inspect_dictionary_entries(source, trailer_report.dictionary_open_byte_offset)
        .expect("entries should inspect");

    assert_eq!(entries.entries.len(), 3);
    assert_entry(
        &entries,
        source,
        0,
        b"/Size",
        b"2",
        DictionaryValueKind::NumberLike,
    );
    assert_entry(
        &entries,
        source,
        1,
        b"/Root",
        b"1 0 R",
        DictionaryValueKind::IndirectReferenceLike,
    );
    assert_entry(
        &entries,
        source,
        2,
        b"/Prev",
        b"123",
        DictionaryValueKind::NumberLike,
    );
}

fn assert_entry(
    report: &DictionaryEntryInspection,
    source: &[u8],
    index: usize,
    expected_key: &[u8],
    expected_value: &[u8],
    expected_kind: DictionaryValueKind,
) {
    let entry = &report.entries[index];
    assert_eq!(
        &source[entry.key_range.start..entry.key_range.end],
        expected_key
    );
    assert_eq!(
        &source[entry.value_range.start..entry.value_range.end],
        expected_value
    );
    assert_eq!(entry.value_kind, expected_kind);
}
