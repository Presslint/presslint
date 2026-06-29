use crate::{
    ContentStreamStartInspectionRejection, DictionaryValueKind,
    DirectLengthContentStreamDataExtentInspectionRejection, IndirectObjectBodyLeadingTokenKind,
    IndirectObjectDictionaryInspectionRejection, IndirectRef, PageContentTargetInspection,
    StreamEolIssue, StreamKeywordEol, inspect_catalog_pages, inspect_classic_xref_table,
    inspect_classic_xref_trailer_root, inspect_content_stream_start,
    inspect_direct_length_content_stream_data_extent, inspect_page_content_targets,
    inspect_page_contents, inspect_page_tree_kids, inspect_page_tree_reference_target,
};

struct SinglePageContentFixture {
    source: Vec<u8>,
    content_offset: usize,
    content_reference: IndirectRef,
}

fn single_page_content_fixture() -> SinglePageContentFixture {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n";
    let content = b"5 0 obj\n<< /Length 11 >>\nstream\nABCDEFGHIJK\nendstream\nendobj\n";
    let catalog_offset = prefix.len();
    let pages_offset = prefix.len() + catalog.len();
    let page_offset = prefix.len() + catalog.len() + pages.len();
    let content_offset = prefix.len() + catalog.len() + pages.len() + page.len();
    let xref_offset = prefix.len() + catalog.len() + pages.len() + page.len() + content.len();
    let source = format!(
        "{}{}{}{}{}xref\n0 6\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{pages_offset:010} 00000 n \n{page_offset:010} 00000 n \n0000000000 00000 f \n{content_offset:010} 00000 n \ntrailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(catalog),
        String::from_utf8_lossy(pages),
        String::from_utf8_lossy(page),
        String::from_utf8_lossy(content),
    )
    .into_bytes();

    SinglePageContentFixture {
        source,
        content_offset,
        content_reference: IndirectRef {
            object_number: 5,
            generation: 0,
        },
    }
}

fn resolved_single_page_content_target(fixture: &SinglePageContentFixture) {
    let source = &fixture.source;
    let xref_offset = source
        .windows(b"xref".len())
        .position(|window| window == b"xref")
        .expect("xref keyword present");
    let xref_report = inspect_classic_xref_table(source, xref_offset).expect("xref should inspect");
    let root_report = inspect_classic_xref_trailer_root(source, xref_report.trailer_byte_offset)
        .expect("root should inspect");
    let catalog_target =
        inspect_page_tree_reference_target(source, &xref_report, root_report.root_reference)
            .expect("catalog reference should resolve");
    let catalog_pages = inspect_catalog_pages(source, catalog_target.object_byte_offset)
        .expect("catalog pages should inspect");
    let page_tree =
        inspect_page_tree_reference_target(source, &xref_report, catalog_pages.pages_reference)
            .expect("page tree should resolve");
    let kids =
        inspect_page_tree_kids(source, page_tree.object_byte_offset).expect("kids should inspect");
    let page_target =
        inspect_page_tree_reference_target(source, &xref_report, kids.kids[0].reference)
            .expect("page should resolve");
    let contents = inspect_page_contents(source, page_target.object_byte_offset)
        .expect("contents should inspect");
    assert_eq!(contents.contents[0].reference, fixture.content_reference);

    let targets = inspect_page_content_targets(source, &xref_report, &contents);
    assert_eq!(
        targets.entries[0],
        PageContentTargetInspection::Resolved {
            content_reference: contents.contents[0],
            object_byte_offset: fixture.content_offset,
            xref_generation: 0,
        }
    );
}

#[test]
fn content_stream_start_locates_lf_stream_data_start() {
    let source = b"5 0 obj\n<< /Length 12 >>\nstream\nhello world!\nendstream\nendobj\n";

    let report = inspect_content_stream_start(source, 0).expect("stream object should inspect");

    assert_eq!(
        report.dictionary.reference,
        IndirectRef {
            object_number: 5,
            generation: 0,
        }
    );
    let keyword_offset = report.stream_keyword_byte_offset;
    assert_eq!(&source[keyword_offset..keyword_offset + 6], b"stream");
    assert_eq!(report.after_stream_keyword_byte_offset, keyword_offset + 6);
    assert_eq!(report.eol, StreamKeywordEol::LineFeed);
    assert_eq!(report.eol.byte_len(), 1);
    assert_eq!(
        report.stream_data_start_byte_offset,
        report.after_stream_keyword_byte_offset + 1
    );
    assert_eq!(
        &source[report.stream_data_start_byte_offset..report.stream_data_start_byte_offset + 12],
        b"hello world!"
    );
}

#[test]
fn content_stream_start_locates_crlf_stream_data_start() {
    let source = b"5 0 obj\n<< /Length 3 >>\nstream\r\nabc\r\nendstream\r\nendobj\r\n";

    let report = inspect_content_stream_start(source, 0).expect("stream object should inspect");

    assert_eq!(report.eol, StreamKeywordEol::CarriageReturnLineFeed);
    assert_eq!(report.eol.byte_len(), 2);
    assert_eq!(
        report.stream_data_start_byte_offset,
        report.after_stream_keyword_byte_offset + 2
    );
    assert_eq!(
        &source[report.stream_data_start_byte_offset..report.stream_data_start_byte_offset + 3],
        b"abc"
    );
}

#[test]
fn content_stream_start_allows_whitespace_and_comments_between_close_and_stream() {
    let source = b"5 0 obj\n<< /Length 1 >>  \t% comment here\n  stream\nX\nendstream\nendobj\n";

    let report = inspect_content_stream_start(source, 0).expect("stream object should inspect");

    let keyword_offset = report.stream_keyword_byte_offset;
    assert_eq!(&source[keyword_offset..keyword_offset + 6], b"stream");
    // The keyword start is past the dictionary close, the comment, and whitespace.
    assert!(keyword_offset > report.dictionary.after_dictionary_close_byte_offset);
    assert_eq!(report.eol, StreamKeywordEol::LineFeed);
    assert_eq!(source[report.stream_data_start_byte_offset], b'X');
}

#[test]
fn content_stream_start_rejects_lone_carriage_return_after_stream() {
    let source = b"5 0 obj\n<< /Length 1 >>\nstream\rX\nendstream\nendobj\n";

    let error =
        inspect_content_stream_start(source, 0).expect_err("lone CR after stream should reject");

    assert_eq!(error.byte_offset, 0);
    assert_eq!(error.byte_len, source.len());
    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::InvalidStreamEol {
            eol_issue: StreamEolIssue::LoneCarriageReturn,
        }
    );
    // The reported error offset is where the EOL marker was expected.
    let after_stream = error.error_byte_offset.expect("error offset present");
    assert_eq!(&source[after_stream - 6..after_stream], b"stream");
    assert_eq!(source[after_stream], b'\r');
}

#[test]
fn content_stream_start_rejects_non_dictionary_body() {
    let source = b"5 0 obj\n[ 1 2 3 ]\nstream\nX\nendstream\nendobj\n";

    let error =
        inspect_content_stream_start(source, 0).expect_err("array body should reject as stream");

    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::NonDictionaryBody {
            token_kind: IndirectObjectBodyLeadingTokenKind::ArrayOpen,
        }
    );
}

#[test]
fn content_stream_start_propagates_delegated_dictionary_rejection() {
    let source = b"xref\n0 1\n0000000000 65535 f \n";

    let error =
        inspect_content_stream_start(source, 0).expect_err("non-object offset should reject");

    assert!(matches!(
        error.reason,
        ContentStreamStartInspectionRejection::ObjectDictionary {
            object_dictionary_reason: IndirectObjectDictionaryInspectionRejection::Header { .. },
        }
    ));
}

#[test]
fn content_stream_start_rejects_misspelled_stream_keyword() {
    let source = b"5 0 obj\n<< /Length 1 >>\nstreams\nX\nendstream\nendobj\n";

    let error = inspect_content_stream_start(source, 0)
        .expect_err("misspelled keyword should reject as malformed");

    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::MissingStreamKeyword
    );
    let keyword_offset = error.error_byte_offset.expect("error offset present");
    assert_eq!(&source[keyword_offset..keyword_offset + 7], b"streams");
}

#[test]
fn content_stream_start_rejects_digit_suffixed_stream_keyword() {
    let source = b"5 0 obj\n<< /Length 1 >>\nstream0\nX\nendstream\nendobj\n";

    let error = inspect_content_stream_start(source, 0)
        .expect_err("stream0 keyword should reject as malformed");

    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::MissingStreamKeyword
    );
}

#[test]
fn content_stream_start_rejects_missing_stream_keyword_after_dictionary() {
    let source = b"5 0 obj\n<< /Length 1 >>\nendobj\n";

    let error = inspect_content_stream_start(source, 0)
        .expect_err("object without stream keyword should reject");

    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::MissingStreamKeyword
    );
}

#[test]
fn content_stream_start_rejects_eof_right_after_stream_keyword() {
    let source = b"5 0 obj\n<< /Length 1 >>\nstream";

    let error =
        inspect_content_stream_start(source, 0).expect_err("EOF after stream should reject");

    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::InvalidStreamEol {
            eol_issue: StreamEolIssue::EndOfFile,
        }
    );
}

#[test]
fn content_stream_start_rejects_offset_out_of_bounds_when_only_whitespace_follows() {
    let source = b"5 0 obj\n<< /Length 1 >>\n   \n";

    let error = inspect_content_stream_start(source, 0)
        .expect_err("whitespace-only tail should reject as out of bounds");

    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::OffsetOutOfBounds
    );
    assert_eq!(error.error_byte_offset, Some(source.len()));
}

#[test]
fn content_stream_start_rejects_non_eol_byte_after_stream_keyword() {
    // `stream` followed by a space then a newline is not a valid §7.3.8.1 EOL.
    let source = b"5 0 obj\n<< /Length 1 >>\nstream \nX\nendstream\nendobj\n";

    let error = inspect_content_stream_start(source, 0)
        .expect_err("space after stream should reject as invalid EOL");

    assert_eq!(
        error.reason,
        ContentStreamStartInspectionRejection::InvalidStreamEol {
            eol_issue: StreamEolIssue::NotEndOfLine,
        }
    );
}

#[test]
fn content_stream_start_report_does_not_retain_source_bytes() {
    // The dictionary key, its value, and the stream payload are all source
    // content; only offsets, ranges, and the EOL marker should be reported. The
    // struct/field names legitimately contain "stream", so the assertions check
    // source byte content rather than the report's own field names.
    let source = b"5 0 obj\n<< /Secret (do-not-copy) /Length 14 >>\nstream\nsecret-payload\nendstream\nendobj\n";

    let report = inspect_content_stream_start(source, 0).expect("stream object should inspect");

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("secret-payload"));
    assert!(!debug_report.contains("do-not-copy"));
    assert!(!debug_report.contains("Secret"));
}

#[test]
fn direct_length_stream_extent_locates_lf_terminated_data_range() {
    let source = b"5 0 obj\n<< /Length 12 >>\nstream\nhello world!\nendstream\nendobj\n";

    let report = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect("direct-length stream extent should inspect");

    assert_eq!(report.stream_start.eol, StreamKeywordEol::LineFeed);
    assert_eq!(report.length, 12);
    assert_eq!(
        &source[report.length_key_range.start..report.length_key_range.end],
        b"/Length"
    );
    assert_eq!(
        &source[report.length_value_range.start..report.length_value_range.end],
        b"12"
    );
    assert_eq!(
        report.stream_data_start_byte_offset,
        report.stream_start.stream_data_start_byte_offset
    );
    assert_eq!(
        report.stream_data_end_byte_offset,
        report.stream_data_start_byte_offset + report.length
    );
    assert_eq!(
        &source[report.stream_data_start_byte_offset..report.stream_data_end_byte_offset],
        b"hello world!"
    );
}

#[test]
fn direct_length_stream_extent_accepts_crlf_before_endstream() {
    let source = b"5 0 obj\n<< /Length 3 >>\nstream\r\nabc\r\nendstream\r\nendobj\r\n";

    let report = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect("CRLF-terminated stream extent should inspect");

    assert_eq!(
        report.stream_start.eol,
        StreamKeywordEol::CarriageReturnLineFeed
    );
    assert_eq!(report.length, 3);
    assert_eq!(
        &source[report.stream_data_start_byte_offset..report.stream_data_end_byte_offset],
        b"abc"
    );
    assert_eq!(source[report.stream_data_end_byte_offset], b'\r');
    assert_eq!(source[report.stream_data_end_byte_offset + 1], b'\n');
}

#[test]
fn direct_length_stream_extent_accepts_zero_length_stream() {
    let source = b"5 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n";

    let report = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect("zero-length stream extent should inspect");

    assert_eq!(report.length, 0);
    assert_eq!(
        report.stream_data_start_byte_offset,
        report.stream_data_end_byte_offset
    );
    assert_eq!(source[report.stream_data_end_byte_offset], b'\n');
}

#[test]
fn direct_length_stream_extent_rejects_missing_length() {
    let source = b"5 0 obj\n<< /Other 12 >>\nstream\nhello world!\nendstream\nendobj\n";

    let error = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect_err("missing /Length should reject");

    assert_eq!(
        error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::MissingLength
    );
}

#[test]
fn direct_length_stream_extent_rejects_duplicate_length() {
    let source = b"5 0 obj\n<< /Length 1 /Other 0 /Length 1 >>\nstream\nX\nendstream\nendobj\n";

    let error = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect_err("duplicate /Length should reject");

    assert!(matches!(
        error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::DuplicateLength { .. }
    ));
}

#[test]
fn direct_length_stream_extent_rejects_indirect_length() {
    let source = b"5 0 obj\n<< /Length 8 0 R >>\nstream\nX\nendstream\nendobj\n";

    let error = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect_err("indirect /Length should reject");

    assert_eq!(
        error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::IndirectLength
    );
}

#[test]
fn direct_length_stream_extent_rejects_negative_decimal_and_non_numeric_length() {
    for source in [
        &b"5 0 obj\n<< /Length -1 >>\nstream\nX\nendstream\nendobj\n"[..],
        &b"5 0 obj\n<< /Length 1.0 >>\nstream\nX\nendstream\nendobj\n"[..],
    ] {
        let error = inspect_direct_length_content_stream_data_extent(source, 0)
            .expect_err("malformed numeric /Length should reject");

        assert_eq!(
            error.reason,
            DirectLengthContentStreamDataExtentInspectionRejection::MalformedLength
        );
    }

    let source = b"5 0 obj\n<< /Length /One >>\nstream\nX\nendstream\nendobj\n";

    let error = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect_err("non-numeric /Length should reject");

    assert_eq!(
        error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::NonNumericLength {
            value_kind: DictionaryValueKind::Name,
        }
    );
}

#[test]
fn direct_length_stream_extent_rejects_length_numeric_overflow() {
    let source = format!(
        "5 0 obj\n<< /Length {}0 >>\nstream\nX\nendstream\nendobj\n",
        usize::MAX
    )
    .into_bytes();

    let error = inspect_direct_length_content_stream_data_extent(&source, 0)
        .expect_err("too-large /Length should reject");

    assert_eq!(
        error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::LengthOutOfRange
    );
}

#[test]
fn direct_length_stream_extent_rejects_computed_end_overflow_and_out_of_bounds() {
    let overflow_source = format!(
        "5 0 obj\n<< /Length {} >>\nstream\nX\nendstream\nendobj\n",
        usize::MAX
    )
    .into_bytes();

    let overflow_error = inspect_direct_length_content_stream_data_extent(&overflow_source, 0)
        .expect_err("computed stream end overflow should reject");

    assert_eq!(
        overflow_error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOverflow
    );

    let out_of_bounds_source = b"5 0 obj\n<< /Length 99 >>\nstream\nX\nendstream\nendobj\n";

    let out_of_bounds_error =
        inspect_direct_length_content_stream_data_extent(out_of_bounds_source, 0)
            .expect_err("computed stream end past EOF should reject");

    assert_eq!(
        out_of_bounds_error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOutOfBounds
    );
}

#[test]
fn direct_length_stream_extent_rejects_missing_or_invalid_endstream_terminator() {
    let eof_source = b"5 0 obj\n<< /Length 1 >>\nstream\nX";

    let eof_error = inspect_direct_length_content_stream_data_extent(eof_source, 0)
        .expect_err("EOF after data should reject");

    assert_eq!(
        eof_error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::InvalidEndstreamEol {
            eol_issue: StreamEolIssue::EndOfFile,
        }
    );

    let cr_source = b"5 0 obj\n<< /Length 1 >>\nstream\nX\rendstream\nendobj\n";

    let cr_error = inspect_direct_length_content_stream_data_extent(cr_source, 0)
        .expect_err("lone CR before endstream should reject");

    assert_eq!(
        cr_error.reason,
        DirectLengthContentStreamDataExtentInspectionRejection::InvalidEndstreamEol {
            eol_issue: StreamEolIssue::LoneCarriageReturn,
        }
    );
}

#[test]
fn direct_length_stream_extent_rejects_missing_or_misspelled_endstream_keyword() {
    for source in [
        &b"5 0 obj\n<< /Length 1 >>\nstream\nX\nendstram\nendobj\n"[..],
        &b"5 0 obj\n<< /Length 1 >>\nstream\nX\nendstream0\nendobj\n"[..],
    ] {
        let error = inspect_direct_length_content_stream_data_extent(source, 0)
            .expect_err("missing exact endstream keyword should reject");

        assert_eq!(
            error.reason,
            DirectLengthContentStreamDataExtentInspectionRejection::MissingEndstreamKeyword
        );
    }
}

#[test]
fn direct_length_stream_extent_report_does_not_retain_source_bytes() {
    let source = b"5 0 obj\n<< /Secret (do-not-copy) /Length 14 >>\nstream\nsecret-payload\nendstream\nendobj\n";

    let report = inspect_direct_length_content_stream_data_extent(source, 0)
        .expect("direct-length stream extent should inspect");

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("secret-payload"));
    assert!(!debug_report.contains("do-not-copy"));
    assert!(!debug_report.contains("Secret"));
}

#[test]
fn content_stream_start_composes_from_classic_xref_page_contents_to_stream_data_start() {
    let fixture = single_page_content_fixture();
    resolved_single_page_content_target(&fixture);

    let stream_start = inspect_content_stream_start(&fixture.source, fixture.content_offset)
        .expect("resolved content stream should locate its data start");

    assert_eq!(stream_start.dictionary.reference, fixture.content_reference);
    assert_eq!(stream_start.eol, StreamKeywordEol::LineFeed);
    assert_eq!(
        &fixture.source[stream_start.stream_data_start_byte_offset
            ..stream_start.stream_data_start_byte_offset + 11],
        b"ABCDEFGHIJK"
    );
}

#[test]
fn direct_length_stream_extent_composes_from_resolved_page_content_target() {
    let fixture = single_page_content_fixture();
    resolved_single_page_content_target(&fixture);

    let extent =
        inspect_direct_length_content_stream_data_extent(&fixture.source, fixture.content_offset)
            .expect("resolved direct-length stream extent should inspect");

    assert_eq!(extent.length, 11);
    assert_eq!(
        &fixture.source[extent.stream_data_start_byte_offset..extent.stream_data_end_byte_offset],
        b"ABCDEFGHIJK"
    );
}
