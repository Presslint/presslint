use crate::{
    ClassicXrefTrailerDictionaryInspectionRejection, DictionaryExtentInspectionRejection,
    inspect_classic_xref_table, inspect_classic_xref_trailer_dictionary,
};

#[test]
fn classic_xref_trailer_dictionary_reports_flat_dictionary_extent() {
    let source = b"trailer<< /Size 2 >>startxref";

    let report =
        inspect_classic_xref_trailer_dictionary(source, 0).expect("trailer should inspect");

    assert_eq!(report.byte_offset, 0);
    assert_eq!(report.trailer_byte_offset, 0);
    assert_eq!(report.dictionary_open_byte_offset, b"trailer".len());
    assert_eq!(report.dictionary_close_byte_offset, 18);
    assert_eq!(report.after_dictionary_close_byte_offset, 20);
    assert_eq!(report.max_observed_dictionary_depth, 1);
    assert_eq!(
        &source[report.dictionary_close_byte_offset..report.after_dictionary_close_byte_offset],
        b">>"
    );
}

#[test]
fn classic_xref_trailer_dictionary_skips_whitespace_after_keyword() {
    let source = b" \t\r\ntrailer \t\r\n<< /Size 2 >>";

    let report =
        inspect_classic_xref_trailer_dictionary(source, 0).expect("trailer should inspect");

    assert_eq!(report.byte_offset, 0);
    assert_eq!(report.trailer_byte_offset, 4);
    assert_eq!(report.dictionary_open_byte_offset, 15);
    assert_eq!(report.after_dictionary_close_byte_offset, source.len());
}

#[test]
fn classic_xref_trailer_dictionary_reports_nested_dictionary_depth() {
    let source = b"trailer\n<< /Size 2 /Info << /Producer (synthetic) >> >>";

    let report =
        inspect_classic_xref_trailer_dictionary(source, 0).expect("trailer should inspect");

    assert_eq!(report.dictionary_open_byte_offset, 8);
    assert_eq!(report.dictionary_close_byte_offset, source.len() - 2);
    assert_eq!(report.after_dictionary_close_byte_offset, source.len());
    assert_eq!(report.max_observed_dictionary_depth, 2);
}

#[test]
fn classic_xref_trailer_dictionary_rejects_missing_keyword() {
    let source = b"nottrailer << /Size 2 >>";

    let error =
        inspect_classic_xref_trailer_dictionary(source, 0).expect_err("keyword should reject");

    assert_eq!(error.byte_offset, 0);
    assert_eq!(error.byte_len, source.len());
    assert_eq!(error.trailer_byte_offset, None);
    assert_eq!(error.error_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        ClassicXrefTrailerDictionaryInspectionRejection::MissingTrailerKeyword
    );
}

#[test]
fn classic_xref_trailer_dictionary_rejects_keyword_without_valid_boundary() {
    let source = b"trailers << /Size 2 >>";

    let error =
        inspect_classic_xref_trailer_dictionary(source, 0).expect_err("keyword should reject");

    assert_eq!(
        error.reason,
        ClassicXrefTrailerDictionaryInspectionRejection::MissingTrailerKeyword
    );
    assert_eq!(error.error_byte_offset, Some(0));
}

#[test]
fn classic_xref_trailer_dictionary_rejects_offset_at_or_after_eof() {
    let source = b"trailer <<>>";

    let at_eof = inspect_classic_xref_trailer_dictionary(source, source.len())
        .expect_err("eof should reject");
    let out_of_bounds = inspect_classic_xref_trailer_dictionary(source, source.len() + 1)
        .expect_err("oob should reject");

    assert_eq!(
        at_eof.reason,
        ClassicXrefTrailerDictionaryInspectionRejection::OffsetOutOfBounds
    );
    assert_eq!(at_eof.error_byte_offset, None);
    assert_eq!(
        out_of_bounds.reason,
        ClassicXrefTrailerDictionaryInspectionRejection::OffsetOutOfBounds
    );
    assert_eq!(out_of_bounds.error_byte_offset, None);
}

#[test]
fn classic_xref_trailer_dictionary_propagates_dictionary_extent_rejections() {
    let source = b"trailer\n[ /NotADictionary ]";

    let error =
        inspect_classic_xref_trailer_dictionary(source, 0).expect_err("array should reject");

    assert_eq!(error.trailer_byte_offset, Some(0));
    assert_eq!(error.error_byte_offset, Some(8));
    assert_eq!(
        error.reason,
        ClassicXrefTrailerDictionaryInspectionRejection::DictionaryExtent {
            dictionary_reason: DictionaryExtentInspectionRejection::NotDictionaryOpen,
        }
    );
}

#[test]
fn classic_xref_trailer_dictionary_rejection_does_not_retain_trailer_bytes() {
    let source = b"trailer\n<< /Secret (corpus-detail not copied) ";

    let error = inspect_classic_xref_trailer_dictionary(source, 0)
        .expect_err("unterminated dictionary should reject");

    let debug_report = format!("{error:?}");
    assert!(!debug_report.contains("Secret"));
    assert!(!debug_report.contains("corpus-detail"));
}

#[test]
fn classic_xref_trailer_dictionary_composes_with_classic_xref_table() {
    let source = b"%PDF-1.7\nxref\n0 2\n0000000000 65535 f \n0000000017 00000 n \ntrailer\n<< /Size 2 /Root << /Nested true >> >>\nstartxref\n9\n%%EOF\n";

    let xref_report = inspect_classic_xref_table(source, 9).expect("xref should inspect");
    let trailer_report =
        inspect_classic_xref_trailer_dictionary(source, xref_report.trailer_byte_offset)
            .expect("trailer dictionary should inspect");

    assert_eq!(trailer_report.byte_offset, xref_report.trailer_byte_offset);
    assert_eq!(
        trailer_report.trailer_byte_offset,
        xref_report.trailer_byte_offset
    );
    assert_eq!(trailer_report.dictionary_open_byte_offset, 66);
    let startxref_offset = source
        .windows(b"startxref".len())
        .position(|window| window == b"startxref")
        .expect("startxref marker exists");
    assert_eq!(
        trailer_report.after_dictionary_close_byte_offset,
        startxref_offset - 1
    );
    assert_eq!(trailer_report.max_observed_dictionary_depth, 2);
}
