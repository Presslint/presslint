use serde::{Deserialize, Serialize};

use crate::source_utils::{consume_keyword, skip_whitespace_and_comments};
use crate::{
    IndirectObjectBodyLeadingTokenKind, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionRejection,
};

const STREAM_KEYWORD: &[u8] = b"stream";

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

/// Validate the §7.3.8.1 end-of-line marker at `after_stream_offset`.
///
/// Accepts a single LF or a CRLF pair and returns the marker plus the
/// stream-data start offset immediately after it. Rejects a lone CR, EOF, and
/// any other byte.
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
