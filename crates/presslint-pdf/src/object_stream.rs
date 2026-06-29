use serde::{Deserialize, Serialize};

use crate::source_utils::{consume_keyword, parse_usize_decimal, skip_whitespace_and_comments};
use crate::{
    ClassicXrefIntegerObjectResolution, ClassicXrefIntegerObjectResolutionRejection,
    ClassicXrefTableInspection, DictionaryEntryByteRange, DictionaryEntrySpan, DictionaryValueKind,
    IndirectObjectBodyLeadingTokenKind, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionRejection,
};

const STREAM_KEYWORD: &[u8] = b"stream";
const ENDSTREAM_KEYWORD: &[u8] = b"endstream";
const LENGTH_KEY: &[u8] = b"/Length";

/// End-of-line marker accepted after the `stream` keyword per PDF 32000
/// §7.3.8.1.
///
/// The spec allows the keyword `stream` to be followed only by a CARRIAGE
/// RETURN and LINE FEED pair or by a single LINE FEED, and explicitly forbids a
/// CARRIAGE RETURN alone. Reporting which marker was accepted lets a future
/// `endstream`/`/Length` slice reason about the exact stream-data start offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKeywordEol {
    /// A single LINE FEED (`\n`), one byte.
    LineFeed,
    /// A CARRIAGE RETURN followed by a LINE FEED (`\r\n`), two bytes.
    CarriageReturnLineFeed,
}

impl StreamKeywordEol {
    /// Byte length of this end-of-line marker.
    #[must_use]
    pub const fn byte_len(self) -> usize {
        match self {
            Self::LineFeed => 1,
            Self::CarriageReturnLineFeed => 2,
        }
    }
}

/// Located `stream` keyword and stream-data start offset of a dictionary-bodied
/// stream object.
///
/// This report stores only the delegated dictionary inspection, byte offsets,
/// and the accepted end-of-line marker. It does not retain or copy PDF bytes,
/// object bodies, stream bodies, decoded streams, or source slices. The
/// embedded [`IndirectObjectDictionaryInspection`] already carries the parsed
/// `IndirectRef` and the dictionary open/close/after offsets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentStreamStartInspection {
    /// Delegated inspection of the stream object's dictionary body.
    pub dictionary: IndirectObjectDictionaryInspection,
    /// Byte offset where the `stream` keyword begins.
    pub stream_keyword_byte_offset: usize,
    /// Exclusive byte offset immediately after the `stream` keyword.
    pub after_stream_keyword_byte_offset: usize,
    /// End-of-line marker accepted immediately after the `stream` keyword.
    pub eol: StreamKeywordEol,
    /// Byte offset where the stream data begins, immediately after the EOL.
    pub stream_data_start_byte_offset: usize,
}

/// Located byte extent for a content stream whose `/Length` is a direct
/// non-negative integer.
///
/// This report stores only the delegated stream-start inspection, `/Length`
/// key/value ranges, parsed scalar length, and byte offsets. It does not
/// retain or copy stream bytes, decoded bytes, object bodies, dictionaries,
/// source slices, or PDF payload bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectLengthContentStreamDataExtentInspection {
    /// Delegated `stream` keyword and data-start inspection.
    pub stream_start: ContentStreamStartInspection,
    /// Byte range covering the exact top-level raw `/Length` key.
    pub length_key_range: DictionaryEntryByteRange,
    /// Byte range covering the `/Length` value span.
    pub length_value_range: DictionaryEntryByteRange,
    /// Parsed direct non-negative integer `/Length` in bytes.
    pub length: usize,
    /// Byte offset where stream data begins.
    pub stream_data_start_byte_offset: usize,
    /// Exclusive byte offset immediately after the declared stream data.
    pub stream_data_end_byte_offset: usize,
}

/// Located byte extent for a content stream whose `/Length` is resolved through
/// a caller-supplied classic xref table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectLengthContentStreamDataExtentInspection {
    /// Delegated `stream` keyword and data-start inspection.
    pub stream_start: ContentStreamStartInspection,
    /// Byte range covering the exact top-level raw `/Length` key.
    pub length_key_range: DictionaryEntryByteRange,
    /// Byte range covering the `/Length` indirect-reference value span.
    pub length_value_range: DictionaryEntryByteRange,
    /// Delegated one-level classic-xref integer-object resolution.
    pub length_resolution: ClassicXrefIntegerObjectResolution,
    /// Resolved non-negative integer `/Length` in bytes.
    pub length: usize,
    /// Byte offset where stream data begins.
    pub stream_data_start_byte_offset: usize,
    /// Exclusive byte offset immediately after the declared stream data.
    pub stream_data_end_byte_offset: usize,
}

/// Error returned when a stream object's `stream` keyword and data start cannot
/// be located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentStreamStartInspectionError {
    /// Caller-supplied object byte offset where inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: ContentStreamStartInspectionRejection,
}

/// Error returned when a direct-length stream-data extent cannot be located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirectLengthContentStreamDataExtentInspectionError {
    /// Caller-supplied object byte offset where inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: DirectLengthContentStreamDataExtentInspectionRejection,
}

/// Error returned when an indirect-length stream-data extent cannot be located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectLengthContentStreamDataExtentInspectionError {
    /// Caller-supplied object byte offset where inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: IndirectLengthContentStreamDataExtentInspectionRejection,
}

/// Structured content stream start inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ContentStreamStartInspectionRejection {
    /// A delegated object-dictionary inspection failed (excluding the dedicated
    /// non-dictionary body case below).
    ObjectDictionary {
        /// Underlying object-dictionary rejection reason.
        object_dictionary_reason: IndirectObjectDictionaryInspectionRejection,
    },
    /// The indirect object's body is not dictionary-bodied, so it cannot be a
    /// stream object.
    NonDictionaryBody {
        /// Classified leading token family that was not a dictionary open.
        token_kind: IndirectObjectBodyLeadingTokenKind,
    },
    /// After the dictionary close and optional whitespace/comments, the offset
    /// where the `stream` keyword would begin is at or beyond EOF.
    OffsetOutOfBounds,
    /// The exact `stream` keyword was missing or malformed at the resolved
    /// offset (e.g. `streams` or `stream0`).
    MissingStreamKeyword,
    /// The end-of-line marker after the `stream` keyword violates §7.3.8.1.
    InvalidStreamEol {
        /// Specific end-of-line violation.
        eol_issue: StreamEolIssue,
    },
}

/// Structured direct-length stream-data extent rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DirectLengthContentStreamDataExtentInspectionRejection {
    /// A delegated content-stream start inspection failed.
    StreamStart {
        /// Underlying content-stream start rejection reason.
        stream_start_reason: ContentStreamStartInspectionRejection,
    },
    /// The stream dictionary has no exact top-level raw `/Length` key.
    MissingLength,
    /// The stream dictionary has more than one exact top-level raw `/Length`
    /// key.
    DuplicateLength {
        /// First `/Length` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Length` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Length` value is shaped as an indirect reference; resolving it is
    /// out of scope for the direct-length helper.
    IndirectLength,
    /// The `/Length` value is not a scalar number-like value.
    NonNumericLength {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The direct `/Length` scalar is not made only of ASCII digits consuming
    /// the full delegated value span.
    MalformedLength,
    /// The direct `/Length` integer does not fit `usize`.
    LengthOutOfRange,
    /// `stream_data_start + length` overflowed `usize`.
    StreamDataEndOverflow,
    /// The computed exclusive stream-data end offset is past EOF.
    StreamDataEndOutOfBounds,
    /// The byte(s) immediately after the declared stream data are not the
    /// required LF or CRLF terminator before `endstream`.
    InvalidEndstreamEol {
        /// Specific end-of-line violation.
        eol_issue: StreamEolIssue,
    },
    /// The exact `endstream` keyword was not found immediately after the
    /// required stream-data terminator EOL.
    MissingEndstreamKeyword,
}

/// Structured indirect-length stream-data extent rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(missing_docs)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum IndirectLengthContentStreamDataExtentInspectionRejection {
    StreamStart {
        /// Underlying content-stream start rejection reason.
        stream_start_reason: ContentStreamStartInspectionRejection,
    },
    MissingLength,
    DuplicateLength {
        /// First `/Length` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Length` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    NonReferenceLength {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    LengthResolution {
        /// Underlying classic-xref integer-object resolution rejection reason.
        length_resolution_reason: ClassicXrefIntegerObjectResolutionRejection,
    },
    StreamDataEndOverflow,
    StreamDataEndOutOfBounds,
    InvalidEndstreamEol {
        /// Specific end-of-line violation.
        eol_issue: StreamEolIssue,
    },
    MissingEndstreamKeyword,
}

/// Specific §7.3.8.1 end-of-line violation after the `stream` keyword.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamEolIssue {
    /// A lone CARRIAGE RETURN not followed by a LINE FEED; §7.3.8.1 forbids it.
    LoneCarriageReturn,
    /// The `stream` keyword is the last token before EOF; no EOL marker follows.
    EndOfFile,
    /// The byte after `stream` is neither a CRLF pair nor a single LF.
    NotEndOfLine,
}

/// Locate a dictionary-bodied stream object's `stream` keyword and stream-data
/// start byte offset.
///
/// The helper accepts caller-provided bytes and a stream object byte offset
/// (typically a `PageContentTargetInspection::Resolved` `object_byte_offset`).
/// It delegates object-dictionary validation to
/// [`crate::inspect_indirect_object_dictionary`], then from the reported
/// dictionary close/after offset skips optional PDF whitespace and comments,
/// requires the exact `stream` keyword via the shared
/// [`consume_keyword`](crate::source_utils) boundary rule so `streams` or
/// `stream0` is rejected, validates the PDF 32000 §7.3.8.1 end-of-line rule
/// (CRLF or a single LF, never a lone CR), and reports the stream-data start
/// offset immediately after the EOL.
///
/// It locates only the *start* of the stream body. It does not locate
/// `endstream`, read/parse/resolve `/Length`, compute the stream-data end
/// offset, read/decode/decompress stream bytes, validate `/Filter` or `/Type`,
/// or mutate PDF bytes. The report retains or copies no PDF bytes; it carries
/// only the delegated dictionary inspection, offsets, and the EOL marker.
///
/// # Errors
///
/// Returns [`ContentStreamStartInspectionError`] for a delegated object
/// dictionary failure (`ObjectDictionary`), a non-dictionary object body
/// (`NonDictionaryBody`), a post-dictionary offset at or beyond EOF before any
/// `stream` keyword (`OffsetOutOfBounds`), a missing or malformed `stream`
/// keyword (`MissingStreamKeyword`), or an invalid post-`stream` end-of-line
/// marker including a lone CR or EOF (`InvalidStreamEol`).
pub fn inspect_content_stream_start(
    input: &[u8],
    object_offset: usize,
) -> Result<ContentStreamStartInspection, ContentStreamStartInspectionError> {
    let dictionary =
        crate::inspect_indirect_object_dictionary(input, object_offset).map_err(|error| {
            let reason = match error.reason {
                IndirectObjectDictionaryInspectionRejection::NonDictionaryBody { token_kind } => {
                    ContentStreamStartInspectionRejection::NonDictionaryBody { token_kind }
                }
                other => ContentStreamStartInspectionRejection::ObjectDictionary {
                    object_dictionary_reason: other,
                },
            };
            content_stream_start_error(input, object_offset, error.error_byte_offset, reason)
        })?;

    let stream_keyword_byte_offset = skip_whitespace_and_comments(
        input,
        dictionary.after_dictionary_close_byte_offset,
        input.len(),
    );

    if stream_keyword_byte_offset >= input.len() {
        return Err(content_stream_start_error(
            input,
            object_offset,
            Some(stream_keyword_byte_offset),
            ContentStreamStartInspectionRejection::OffsetOutOfBounds,
        ));
    }

    let Some(keyword_len) = consume_keyword(&input[stream_keyword_byte_offset..], STREAM_KEYWORD)
    else {
        return Err(content_stream_start_error(
            input,
            object_offset,
            Some(stream_keyword_byte_offset),
            ContentStreamStartInspectionRejection::MissingStreamKeyword,
        ));
    };

    let after_stream_keyword_byte_offset = stream_keyword_byte_offset + keyword_len;
    let (eol, stream_data_start_byte_offset) =
        accept_stream_eol(input, after_stream_keyword_byte_offset).map_err(|eol_issue| {
            content_stream_start_error(
                input,
                object_offset,
                Some(after_stream_keyword_byte_offset),
                ContentStreamStartInspectionRejection::InvalidStreamEol { eol_issue },
            )
        })?;

    Ok(ContentStreamStartInspection {
        dictionary,
        stream_keyword_byte_offset,
        after_stream_keyword_byte_offset,
        eol,
        stream_data_start_byte_offset,
    })
}

/// Locate the stream-data byte range for a dictionary-bodied content stream
/// object whose `/Length` is a direct non-negative integer.
///
/// The helper composes [`inspect_content_stream_start`] for object,
/// dictionary, `stream` keyword, and stream-data-start validation. It then uses
/// the delegated top-level dictionary entries to find exactly one exact raw
/// `/Length` key, accepts only a direct ASCII-digit integer value that consumes
/// the full reported value span, computes the exclusive stream-data end offset
/// with checked addition, and requires an LF or CRLF followed immediately by an
/// exact `endstream` keyword at that computed end.
///
/// It does not resolve indirect `/Length` objects, fallback-scan for
/// `endstream`, read/copy/decode/decompress stream bytes, inspect filters,
/// tokenize content streams, or validate page/content semantics.
///
/// # Errors
///
/// Returns [`DirectLengthContentStreamDataExtentInspectionError`] for delegated
/// stream-start failures, missing/duplicate `/Length`, indirect/non-numeric or
/// malformed direct `/Length` values, numeric/offset overflow, out-of-bounds
/// data ends, invalid post-data EOL, or a missing/malformed `endstream`
/// keyword at the computed position.
pub fn inspect_direct_length_content_stream_data_extent(
    input: &[u8],
    object_offset: usize,
) -> Result<
    DirectLengthContentStreamDataExtentInspection,
    DirectLengthContentStreamDataExtentInspectionError,
> {
    let stream_start = inspect_content_stream_start(input, object_offset).map_err(|error| {
        direct_length_error(
            input,
            object_offset,
            error.error_byte_offset,
            DirectLengthContentStreamDataExtentInspectionRejection::StreamStart {
                stream_start_reason: error.reason,
            },
        )
    })?;

    let length_entry = find_length_entry(input, &stream_start)
        .map_err(|issue| direct_length_entry_error(input, object_offset, &stream_start, issue))?;
    let length = parse_direct_length(input, object_offset, length_entry)?;
    let stream_data_start_byte_offset = stream_start.stream_data_start_byte_offset;
    let stream_data_end_byte_offset = stream_data_start_byte_offset
        .checked_add(length)
        .ok_or_else(|| {
            direct_length_error(
                input,
                object_offset,
                Some(stream_data_start_byte_offset),
                DirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOverflow,
            )
        })?;

    validate_direct_endstream(input, object_offset, stream_data_end_byte_offset)?;

    Ok(DirectLengthContentStreamDataExtentInspection {
        stream_start,
        length_key_range: length_entry.key_range,
        length_value_range: length_entry.value_range,
        length,
        stream_data_start_byte_offset,
        stream_data_end_byte_offset,
    })
}

/// Locate stream data for an indirect `/Length` content stream.
///
/// This composes stream-start inspection, one-level classic-xref integer
/// resolution, checked arithmetic, and fixed-position `endstream` validation.
/// It does not follow reference chains, `/Prev`, xref streams, or object
/// streams, and it does not read, copy, decode, or tokenize stream bytes.
///
/// # Errors
///
/// Returns structured rejections for delegated failures, bad `/Length` shape or
/// resolution, offset overflow/bounds errors, and malformed `endstream`.
pub fn inspect_indirect_length_content_stream_data_extent(
    input: &[u8],
    xref_table: &ClassicXrefTableInspection,
    object_offset: usize,
) -> Result<
    IndirectLengthContentStreamDataExtentInspection,
    IndirectLengthContentStreamDataExtentInspectionError,
> {
    let stream_start = inspect_content_stream_start(input, object_offset).map_err(|error| {
        indirect_length_error(
            input,
            object_offset,
            error.error_byte_offset,
            IndirectLengthContentStreamDataExtentInspectionRejection::StreamStart {
                stream_start_reason: error.reason,
            },
        )
    })?;

    let length_entry = find_length_entry(input, &stream_start)
        .map_err(|issue| indirect_length_entry_error(input, object_offset, &stream_start, issue))?;
    if length_entry.value_kind != DictionaryValueKind::IndirectReferenceLike {
        return Err(indirect_length_error(
            input,
            object_offset,
            Some(length_entry.value_range.start),
            IndirectLengthContentStreamDataExtentInspectionRejection::NonReferenceLength {
                value_kind: length_entry.value_kind,
            },
        ));
    }

    let length_resolution = crate::resolve_classic_xref_integer_object(
        input,
        xref_table,
        length_entry.value_range.start,
    )
    .map_err(|error| {
        indirect_length_error(
            input,
            object_offset,
            error.error_byte_offset,
            IndirectLengthContentStreamDataExtentInspectionRejection::LengthResolution {
                length_resolution_reason: error.reason,
            },
        )
    })?;
    let length = length_resolution.value;
    let stream_data_start_byte_offset = stream_start.stream_data_start_byte_offset;
    let stream_data_end_byte_offset = stream_data_start_byte_offset
        .checked_add(length)
        .ok_or_else(|| {
            indirect_length_error(
                input,
                object_offset,
                Some(stream_data_start_byte_offset),
                IndirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOverflow,
            )
        })?;

    validate_indirect_endstream(input, object_offset, stream_data_end_byte_offset)?;

    Ok(IndirectLengthContentStreamDataExtentInspection {
        stream_start,
        length_key_range: length_entry.key_range,
        length_value_range: length_entry.value_range,
        length_resolution,
        length,
        stream_data_start_byte_offset,
        stream_data_end_byte_offset,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LengthEntryIssue {
    Missing,
    Duplicate(DictionaryEntryByteRange, DictionaryEntryByteRange),
}

fn find_length_entry(
    input: &[u8],
    stream_start: &ContentStreamStartInspection,
) -> Result<DictionaryEntrySpan, LengthEntryIssue> {
    let mut length_entry: Option<DictionaryEntrySpan> = None;
    for entry in &stream_start.dictionary.entries {
        if input.get(entry.key_range.start..entry.key_range.end) != Some(LENGTH_KEY) {
            continue;
        }

        if let Some(first) = length_entry {
            return Err(LengthEntryIssue::Duplicate(
                first.key_range,
                entry.key_range,
            ));
        }

        length_entry = Some(*entry);
    }

    length_entry.ok_or(LengthEntryIssue::Missing)
}

fn parse_direct_length(
    input: &[u8],
    object_offset: usize,
    length_entry: DictionaryEntrySpan,
) -> Result<usize, DirectLengthContentStreamDataExtentInspectionError> {
    match length_entry.value_kind {
        DictionaryValueKind::IndirectReferenceLike => {
            return Err(direct_length_error(
                input,
                object_offset,
                Some(length_entry.value_range.start),
                DirectLengthContentStreamDataExtentInspectionRejection::IndirectLength,
            ));
        }
        DictionaryValueKind::NumberLike => {}
        value_kind => {
            return Err(direct_length_error(
                input,
                object_offset,
                Some(length_entry.value_range.start),
                DirectLengthContentStreamDataExtentInspectionRejection::NonNumericLength {
                    value_kind,
                },
            ));
        }
    }

    let value_bytes = &input[length_entry.value_range.start..length_entry.value_range.end];
    if value_bytes.is_empty() || !value_bytes.iter().all(u8::is_ascii_digit) {
        return Err(direct_length_error(
            input,
            object_offset,
            Some(length_entry.value_range.start),
            DirectLengthContentStreamDataExtentInspectionRejection::MalformedLength,
        ));
    }

    parse_usize_decimal(value_bytes).ok_or_else(|| {
        direct_length_error(
            input,
            object_offset,
            Some(length_entry.value_range.start),
            DirectLengthContentStreamDataExtentInspectionRejection::LengthOutOfRange,
        )
    })
}

fn validate_direct_endstream(
    input: &[u8],
    object_offset: usize,
    stream_data_end_byte_offset: usize,
) -> Result<(), DirectLengthContentStreamDataExtentInspectionError> {
    validate_endstream(input, stream_data_end_byte_offset).map_err(|issue| match issue {
        EndstreamValidationIssue::DataEndOutOfBounds => direct_length_error(
            input,
            object_offset,
            Some(stream_data_end_byte_offset),
            DirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOutOfBounds,
        ),
        EndstreamValidationIssue::InvalidEol(eol_issue) => direct_length_error(
            input,
            object_offset,
            Some(stream_data_end_byte_offset),
            DirectLengthContentStreamDataExtentInspectionRejection::InvalidEndstreamEol {
                eol_issue,
            },
        ),
        EndstreamValidationIssue::MissingKeyword(keyword_byte_offset) => direct_length_error(
            input,
            object_offset,
            Some(keyword_byte_offset),
            DirectLengthContentStreamDataExtentInspectionRejection::MissingEndstreamKeyword,
        ),
    })
}

fn validate_indirect_endstream(
    input: &[u8],
    object_offset: usize,
    stream_data_end_byte_offset: usize,
) -> Result<(), IndirectLengthContentStreamDataExtentInspectionError> {
    validate_endstream(input, stream_data_end_byte_offset).map_err(|issue| match issue {
        EndstreamValidationIssue::DataEndOutOfBounds => indirect_length_error(
            input,
            object_offset,
            Some(stream_data_end_byte_offset),
            IndirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOutOfBounds,
        ),
        EndstreamValidationIssue::InvalidEol(eol_issue) => indirect_length_error(
            input,
            object_offset,
            Some(stream_data_end_byte_offset),
            IndirectLengthContentStreamDataExtentInspectionRejection::InvalidEndstreamEol {
                eol_issue,
            },
        ),
        EndstreamValidationIssue::MissingKeyword(keyword_byte_offset) => indirect_length_error(
            input,
            object_offset,
            Some(keyword_byte_offset),
            IndirectLengthContentStreamDataExtentInspectionRejection::MissingEndstreamKeyword,
        ),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndstreamValidationIssue {
    DataEndOutOfBounds,
    InvalidEol(StreamEolIssue),
    MissingKeyword(usize),
}

fn validate_endstream(
    input: &[u8],
    stream_data_end_byte_offset: usize,
) -> Result<(), EndstreamValidationIssue> {
    if stream_data_end_byte_offset > input.len() {
        return Err(EndstreamValidationIssue::DataEndOutOfBounds);
    }

    let (_, after_eol_offset) = accept_stream_eol(input, stream_data_end_byte_offset)
        .map_err(EndstreamValidationIssue::InvalidEol)?;

    if consume_keyword(&input[after_eol_offset..], ENDSTREAM_KEYWORD).is_none() {
        return Err(EndstreamValidationIssue::MissingKeyword(after_eol_offset));
    }

    Ok(())
}

const fn direct_length_entry_error(
    input: &[u8],
    byte_offset: usize,
    stream_start: &ContentStreamStartInspection,
    issue: LengthEntryIssue,
) -> DirectLengthContentStreamDataExtentInspectionError {
    match issue {
        LengthEntryIssue::Missing => direct_length_error(
            input,
            byte_offset,
            Some(stream_start.dictionary.dictionary_close_byte_offset),
            DirectLengthContentStreamDataExtentInspectionRejection::MissingLength,
        ),
        LengthEntryIssue::Duplicate(first_key_range, duplicate_key_range) => direct_length_error(
            input,
            byte_offset,
            Some(duplicate_key_range.start),
            DirectLengthContentStreamDataExtentInspectionRejection::DuplicateLength {
                first_key_range,
                duplicate_key_range,
            },
        ),
    }
}

const fn indirect_length_entry_error(
    input: &[u8],
    byte_offset: usize,
    stream_start: &ContentStreamStartInspection,
    issue: LengthEntryIssue,
) -> IndirectLengthContentStreamDataExtentInspectionError {
    match issue {
        LengthEntryIssue::Missing => indirect_length_error(
            input,
            byte_offset,
            Some(stream_start.dictionary.dictionary_close_byte_offset),
            IndirectLengthContentStreamDataExtentInspectionRejection::MissingLength,
        ),
        LengthEntryIssue::Duplicate(first_key_range, duplicate_key_range) => indirect_length_error(
            input,
            byte_offset,
            Some(duplicate_key_range.start),
            IndirectLengthContentStreamDataExtentInspectionRejection::DuplicateLength {
                first_key_range,
                duplicate_key_range,
            },
        ),
    }
}

fn accept_stream_eol(
    input: &[u8],
    after_stream_offset: usize,
) -> Result<(StreamKeywordEol, usize), StreamEolIssue> {
    match input.get(after_stream_offset) {
        Some(b'\n') => Ok((StreamKeywordEol::LineFeed, after_stream_offset + 1)),
        Some(b'\r') => {
            if input.get(after_stream_offset + 1) == Some(&b'\n') {
                Ok((
                    StreamKeywordEol::CarriageReturnLineFeed,
                    after_stream_offset + 2,
                ))
            } else {
                Err(StreamEolIssue::LoneCarriageReturn)
            }
        }
        Some(_) => Err(StreamEolIssue::NotEndOfLine),
        None => Err(StreamEolIssue::EndOfFile),
    }
}

const fn content_stream_start_error(
    input: &[u8],
    byte_offset: usize,
    error_byte_offset: Option<usize>,
    reason: ContentStreamStartInspectionRejection,
) -> ContentStreamStartInspectionError {
    ContentStreamStartInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset,
        reason,
    }
}

const fn direct_length_error(
    input: &[u8],
    byte_offset: usize,
    error_byte_offset: Option<usize>,
    reason: DirectLengthContentStreamDataExtentInspectionRejection,
) -> DirectLengthContentStreamDataExtentInspectionError {
    DirectLengthContentStreamDataExtentInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset,
        reason,
    }
}

const fn indirect_length_error(
    input: &[u8],
    byte_offset: usize,
    error_byte_offset: Option<usize>,
    reason: IndirectLengthContentStreamDataExtentInspectionRejection,
) -> IndirectLengthContentStreamDataExtentInspectionError {
    IndirectLengthContentStreamDataExtentInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset,
        reason,
    }
}
