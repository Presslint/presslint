use crate::{
    DictionaryExtentInspectionRejection, IndirectObjectBodyLeadingTokenKind,
    inspect_dictionary_extent, inspect_indirect_object_body_token, inspect_indirect_object_header,
};

#[test]
fn flat_dictionary_reports_close_and_after_close_offsets() {
    let source = b"<< /Type /Example >>";

    let report = inspect_dictionary_extent(source, 0).expect("flat dictionary should inspect");

    assert_eq!(report.byte_offset, 0);
    assert_eq!(report.open_byte_offset, 0);
    assert_eq!(report.close_byte_offset, source.len() - 2);
    assert_eq!(report.after_close_byte_offset, source.len());
    assert_eq!(report.max_observed_depth, 1);
    assert_eq!(
        &source[report.close_byte_offset..report.after_close_byte_offset],
        b">>"
    );
}

#[test]
fn leading_whitespace_is_skipped_before_the_dictionary_open() {
    let source = b" \t\r\n<< /K 1 >>tail";

    let report = inspect_dictionary_extent(source, 0).expect("dictionary should inspect");

    assert_eq!(report.open_byte_offset, 4);
    assert_eq!(report.after_close_byte_offset, source.len() - b"tail".len());
    assert_eq!(
        &source[report.open_byte_offset..report.after_close_byte_offset],
        b"<< /K 1 >>"
    );
}

#[test]
fn nested_dictionary_reports_outermost_matching_close() {
    let source = b"<< /Outer << /Inner 1 >> /After 2 >>";

    let report = inspect_dictionary_extent(source, 0).expect("nested dictionary should inspect");

    assert_eq!(report.close_byte_offset, source.len() - 2);
    assert_eq!(report.after_close_byte_offset, source.len());
    assert_eq!(report.max_observed_depth, 2);
    // The first inner `>>` must not be reported as the close.
    let first_inner_close = source
        .windows(2)
        .position(|window| window == b">>")
        .expect("inner close exists");
    assert!(report.close_byte_offset > first_inner_close);
}

#[test]
fn nested_hex_string_close_does_not_terminate_dictionary() {
    // The `<41>` hex string sits flush against the dictionary close `>>`.
    let source = b"<< /K <41>>>";

    let report = inspect_dictionary_extent(source, 0).expect("dictionary should inspect");

    assert_eq!(report.close_byte_offset, source.len() - 2);
    assert_eq!(report.after_close_byte_offset, source.len());
    assert_eq!(report.max_observed_depth, 1);
}

#[test]
fn close_delimiter_inside_literal_string_does_not_close_dictionary() {
    let source = b"<< /K (a >> b) /M 1 >>";

    let report = inspect_dictionary_extent(source, 0).expect("dictionary should inspect");

    assert_eq!(report.close_byte_offset, source.len() - 2);
    assert_eq!(report.after_close_byte_offset, source.len());
}

#[test]
fn close_delimiter_inside_hex_string_does_not_close_dictionary() {
    let source = b"<< /K <3E3E> /M 1 >>";

    let report = inspect_dictionary_extent(source, 0).expect("dictionary should inspect");

    assert_eq!(report.close_byte_offset, source.len() - 2);
    assert_eq!(report.after_close_byte_offset, source.len());
}

#[test]
fn escaped_parentheses_inside_literal_string_keep_string_balanced() {
    // The escaped `\)` does not close the string, so the `>>` inside it is opaque.
    let source = b"<< /K (escaped \\) >> still open) /M 1 >>";

    let report = inspect_dictionary_extent(source, 0).expect("dictionary should inspect");

    assert_eq!(report.close_byte_offset, source.len() - 2);
    assert_eq!(report.after_close_byte_offset, source.len());
}

#[test]
fn close_delimiter_inside_comment_does_not_close_dictionary() {
    let source = b"<< /K 1 % a comment with >>\n/M 2 >>";

    let report = inspect_dictionary_extent(source, 0).expect("dictionary should inspect");

    assert_eq!(report.close_byte_offset, source.len() - 2);
    assert_eq!(report.after_close_byte_offset, source.len());
}

#[test]
fn unterminated_dictionary_is_rejected() {
    let source = b"<< /Outer << /Inner 1 >> ";

    let error = inspect_dictionary_extent(source, 0).expect_err("missing close should reject");

    assert_eq!(
        error.reason,
        DictionaryExtentInspectionRejection::UnterminatedDictionary
    );
    assert_eq!(error.error_byte_offset, None);
    assert_eq!(error.byte_len, source.len());
}

#[test]
fn unterminated_literal_string_is_rejected_at_its_open() {
    let source = b"<< /K (never closed >> ";

    let error = inspect_dictionary_extent(source, 0).expect_err("open string should reject");

    assert_eq!(
        error.reason,
        DictionaryExtentInspectionRejection::UnterminatedString
    );
    let open_paren = source
        .iter()
        .position(|&b| b == b'(')
        .expect("paren exists");
    assert_eq!(error.error_byte_offset, Some(open_paren));
}

#[test]
fn unterminated_hex_string_is_rejected_at_its_open() {
    let source = b"<< /K <41 42 43 ";

    let error = inspect_dictionary_extent(source, 0).expect_err("open hex should reject");

    assert_eq!(
        error.reason,
        DictionaryExtentInspectionRejection::UnterminatedString
    );
    let open_hex = source
        .iter()
        .rposition(|&b| b == b'<')
        .expect("hex open exists");
    assert_eq!(error.error_byte_offset, Some(open_hex));
}

#[test]
fn non_dictionary_first_token_is_rejected() {
    let source = b"[ /NotADict ]";

    let error = inspect_dictionary_extent(source, 0).expect_err("array should reject");

    assert_eq!(
        error.reason,
        DictionaryExtentInspectionRejection::NotDictionaryOpen
    );
    assert_eq!(error.error_byte_offset, Some(0));
}

#[test]
fn single_hex_open_first_token_is_not_a_dictionary_open() {
    let source = b"<41424344>";

    let error = inspect_dictionary_extent(source, 0).expect_err("hex string should reject");

    assert_eq!(
        error.reason,
        DictionaryExtentInspectionRejection::NotDictionaryOpen
    );
}

#[test]
fn offset_at_or_after_eof_is_rejected() {
    let source = b"<<>>";

    let at_eof = inspect_dictionary_extent(source, source.len()).expect_err("eof should reject");
    let out_of_bounds =
        inspect_dictionary_extent(source, source.len() + 1).expect_err("oob should reject");

    assert_eq!(
        at_eof.reason,
        DictionaryExtentInspectionRejection::OffsetOutOfBounds
    );
    assert_eq!(
        out_of_bounds.reason,
        DictionaryExtentInspectionRejection::OffsetOutOfBounds
    );
}

#[test]
fn whitespace_only_tail_is_rejected() {
    let source = b"obj \t\r\n";

    let error = inspect_dictionary_extent(source, 3).expect_err("no token should reject");

    assert_eq!(
        error.reason,
        DictionaryExtentInspectionRejection::NoSignificantToken
    );
    assert_eq!(error.error_byte_offset, Some(source.len()));
}

#[test]
fn empty_dictionary_reports_zero_length_body() {
    let report = inspect_dictionary_extent(b"<<>>", 0).expect("empty dictionary should inspect");

    assert_eq!(report.open_byte_offset, 0);
    assert_eq!(report.close_byte_offset, 2);
    assert_eq!(report.after_close_byte_offset, 4);
    assert_eq!(report.max_observed_depth, 1);
}

#[test]
fn excessive_nesting_is_rejected_without_unbounded_work() {
    let source = "<<".repeat(300).into_bytes();

    let error = inspect_dictionary_extent(&source, 0).expect_err("deep nesting should reject");

    assert_eq!(
        error.reason,
        DictionaryExtentInspectionRejection::MaxNestingExceeded
    );
    assert!(error.error_byte_offset.is_some());
}

#[test]
fn rejection_does_not_retain_dictionary_bytes() {
    let source = b"<< /Secret (corpus-detail not copied) /K 1 ";

    let error = inspect_dictionary_extent(source, 0).expect_err("unterminated should reject");

    let debug_report = format!("{error:?}");
    assert!(!debug_report.contains("Secret"));
    assert!(!debug_report.contains("corpus-detail"));
}

#[test]
fn dictionary_extent_composes_with_object_header_and_body_token() {
    let prefix = b"%PDF-1.7\n";
    let header = b"4 0 obj\n";
    let dict = b"<< /Type /Page /Kids << /Inner 1 >> /Count 0 >>";
    let suffix = b"\nendobj\n";

    let mut source = Vec::new();
    source.extend_from_slice(prefix);
    source.extend_from_slice(header);
    source.extend_from_slice(dict);
    source.extend_from_slice(suffix);

    let object_offset = prefix.len();
    let dict_offset = prefix.len() + header.len();

    let header_report =
        inspect_indirect_object_header(&source, object_offset).expect("header should inspect");
    let body = inspect_indirect_object_body_token(&source, header_report.after_obj_keyword_offset)
        .expect("body token should inspect");

    assert_eq!(
        body.token_kind,
        IndirectObjectBodyLeadingTokenKind::DictionaryOpen
    );
    assert_eq!(body.first_token_byte_offset, dict_offset);

    let extent = inspect_dictionary_extent(&source, body.first_token_byte_offset)
        .expect("extent should inspect");

    assert_eq!(extent.open_byte_offset, dict_offset);
    assert_eq!(extent.after_close_byte_offset, dict_offset + dict.len());
    assert_eq!(
        &source[extent.close_byte_offset..extent.after_close_byte_offset],
        b">>"
    );
    assert_eq!(extent.max_observed_depth, 2);
    // The reported extent ends exactly where the trailing `\nendobj` begins.
    assert_eq!(&source[extent.after_close_byte_offset..], suffix);
}
