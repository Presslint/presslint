use serde::{Deserialize, Serialize};

use crate::object_stream::{LengthEntryIssue, find_length_entry};
use crate::source_utils::{
    consume_keyword, count_leading_digits, is_pdf_delimiter, is_pdf_whitespace, parse_usize_decimal,
};
use crate::{
    ClassicXrefIntegerObjectResolution, ClassicXrefTableInspection, ContentStreamStartInspection,
    ContentStreamStartInspectionRejection, DictionaryEntryByteRange, DictionaryEntrySpan,
    DictionaryValueKind, DirectLengthContentStreamDataExtentInspection,
    DirectLengthContentStreamDataExtentInspectionRejection,
    IndirectLengthContentStreamDataExtentInspection,
    IndirectLengthContentStreamDataExtentInspectionRejection, IndirectObjectBodyLeadingTokenKind,
    IndirectObjectBodyTokenInspectionRejection, IndirectReferenceInspectionRejection,
    IntegerObjectValueByteRange, ObjectLookup, ObjectResolutionRejection, StreamEolIssue,
    inspect_content_stream_start, inspect_direct_length_content_stream_data_extent,
    inspect_indirect_length_content_stream_data_extent, inspect_indirect_object_body_token,
    inspect_indirect_object_header, parse_indirect_reference, resolve_xref_object_offset,
};

const ENDOBJ_KEYWORD: &[u8] = b"endobj";
const ENDSTREAM_KEYWORD: &[u8] = b"endstream";

/// Located byte extent for a content stream whose top-level `/Length` is either
/// direct or resolved through a caller-supplied cross-reference backend.
///
/// This report stores only one fixed-size delegated focused-helper report. It
/// does not retain or copy stream bytes, decoded bytes, object bodies,
/// dictionaries, source slices, or PDF payload bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "length_kind", rename_all = "snake_case")]
pub enum ContentStreamDataExtentInspection {
    /// Extent located by the direct integer `/Length` helper.
    DirectLength(DirectLengthContentStreamDataExtentInspection),
    /// Extent located by the indirect-reference `/Length` helper.
    IndirectLength(IndirectLengthContentStreamDataExtentInspection),
}

impl ContentStreamDataExtentInspection {
    /// Resolved stream-data length in bytes.
    #[must_use]
    pub const fn length(&self) -> usize {
        match self {
            Self::DirectLength(report) => report.length,
            Self::IndirectLength(report) => report.length,
        }
    }

    /// Byte offset where stream data begins.
    #[must_use]
    pub const fn stream_data_start_byte_offset(&self) -> usize {
        match self {
            Self::DirectLength(report) => report.stream_data_start_byte_offset,
            Self::IndirectLength(report) => report.stream_data_start_byte_offset,
        }
    }

    /// Exclusive byte offset immediately after the stream data.
    #[must_use]
    pub const fn stream_data_end_byte_offset(&self) -> usize {
        match self {
            Self::DirectLength(report) => report.stream_data_end_byte_offset,
            Self::IndirectLength(report) => report.stream_data_end_byte_offset,
        }
    }
}

/// Error returned when a direct-or-indirect content-stream data extent cannot
/// be located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentStreamDataExtentInspectionError {
    /// Caller-supplied object byte offset where inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed or unsupported construct was found,
    /// when available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: ContentStreamDataExtentInspectionRejection,
}

/// Structured direct-or-indirect content-stream extent rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ContentStreamDataExtentInspectionRejection {
    /// A delegated content-stream start inspection failed during dispatch.
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
    /// The `/Length` value is an indirect reference but no cross-reference
    /// backend was supplied.
    IndirectLengthRequiresXrefTable,
    /// The `/Length` value kind is neither number-like nor indirect-reference
    /// like.
    UnsupportedLengthValueKind {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// Delegated direct-length extent inspection failed.
    DirectLength {
        /// Underlying direct-length rejection reason.
        direct_length_reason: DirectLengthContentStreamDataExtentInspectionRejection,
    },
    /// Delegated classic-xref indirect-length extent inspection failed.
    IndirectLength {
        /// Underlying indirect-length rejection reason.
        indirect_length_reason: IndirectLengthContentStreamDataExtentInspectionRejection,
    },
    /// Lookup-backed indirect-length extent inspection failed.
    ///
    /// This is the backend-neutral counterpart to [`Self::IndirectLength`]
    /// produced when the `/Length` indirect reference is resolved through a
    /// non-classic [`ObjectLookup`] backend (today, a single decoded
    /// cross-reference-stream section).
    LookupIndirectLength {
        /// Underlying lookup-backed indirect-length rejection reason.
        lookup_indirect_length_reason: LookupIndirectLengthRejection,
    },
}

/// Structured rejection reasons for lookup-backed indirect `/Length` resolution.
///
/// These cover the steps the classic indirect helper performs through a classic
/// xref table, but expressed over the backend-neutral object-resolution
/// machinery so cross-reference-stream compressed, reserved, free, missing,
/// out-of-range, and generation-mismatched entries surface as structured
/// failures and are never fabricated into byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum LookupIndirectLengthRejection {
    /// The `/Length` value could not be parsed as an `N G R` indirect reference.
    Reference {
        /// Underlying indirect-reference parsing rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// Backend object resolution of the `/Length` reference failed (free,
    /// not-found, out-of-range, compressed, reserved, generation-mismatched, or
    /// a malformed object header at the resolved offset).
    ObjectResolution {
        /// Underlying backend object-resolution rejection reason.
        object_resolution_reason: ObjectResolutionRejection,
    },
    /// The resolved `/Length` object body's leading token could not be
    /// classified.
    BodyToken {
        /// Underlying body-token classification rejection reason.
        body_token_reason: IndirectObjectBodyTokenInspectionRejection,
    },
    /// The resolved `/Length` object body's leading token is not number-like.
    NonIntegerBody {
        /// Classified leading token family that was not number-like.
        token_kind: IndirectObjectBodyLeadingTokenKind,
    },
    /// The resolved `/Length` body is not a non-negative ASCII-digit run
    /// terminated by PDF whitespace, a delimiter, or `endobj`.
    MalformedInteger,
    /// The resolved non-negative integer `/Length` does not fit `usize`.
    IntegerOutOfRange,
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

/// Locate the stream-data byte range for a dictionary-bodied content stream
/// through a classic xref table.
///
/// This is a thin classic wrapper over
/// [`inspect_content_stream_data_extent_with_lookup`]: it maps the optional
/// classic xref table to an [`ObjectLookup::ClassicXref`] backend and therefore
/// keeps the direct-length path, the classic indirect-length path, and every
/// error variant byte-identical to the pre-`_with_lookup` behavior. A missing
/// xref table keeps rejecting an indirect `/Length` as
/// [`IndirectLengthRequiresXrefTable`](ContentStreamDataExtentInspectionRejection::IndirectLengthRequiresXrefTable).
///
/// # Errors
///
/// Returns [`ContentStreamDataExtentInspectionError`] for dispatch-time
/// stream-start failures, missing or duplicate `/Length`, an indirect `/Length`
/// without a supplied classic xref table, unsupported `/Length` value kinds,
/// or delegated direct/indirect focused-helper failures with the underlying
/// rejection reason preserved.
pub fn inspect_content_stream_data_extent(
    input: &[u8],
    xref_table: Option<&ClassicXrefTableInspection>,
    object_offset: usize,
) -> Result<ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionError> {
    inspect_content_stream_data_extent_with_lookup(
        input,
        xref_table.map(ObjectLookup::ClassicXref),
        object_offset,
    )
}

/// Locate the stream-data byte range for a dictionary-bodied content stream
/// through any [`ObjectLookup`] backend.
///
/// The top-level `/Length` may be either a direct non-negative integer or an
/// indirect reference. The helper first inspects the stream start to classify
/// exactly one exact raw top-level `/Length` entry by [`DictionaryValueKind`].
/// `NumberLike` always dispatches to
/// [`inspect_direct_length_content_stream_data_extent`], regardless of the
/// supplied backend, because a direct `/Length` needs no object resolution.
///
/// `IndirectReferenceLike` uses the backend only for resolution:
/// - no backend rejects as
///   [`IndirectLengthRequiresXrefTable`](ContentStreamDataExtentInspectionRejection::IndirectLengthRequiresXrefTable);
/// - [`ObjectLookup::ClassicXref`] delegates to
///   [`inspect_indirect_length_content_stream_data_extent`] so the classic path
///   stays byte-identical;
/// - any other backend resolves the `/Length` reference through the shared
///   [`resolve_xref_object_offset`] machinery and reads the referenced
///   non-negative integer, mapping compressed, reserved, free, missing,
///   out-of-range, and generation-mismatched results into structured
///   [`LookupIndirectLength`](ContentStreamDataExtentInspectionRejection::LookupIndirectLength)
///   failures rather than fabricating offsets.
///
/// Other value kinds reject as
/// [`UnsupportedLengthValueKind`](ContentStreamDataExtentInspectionRejection::UnsupportedLengthValueKind).
///
/// It keeps locate-only semantics: it reads, copies, slices, decodes, and
/// tokenizes no stream-data bytes, builds no concatenated content buffer,
/// follows no `/Prev`, parses no object streams, and builds no object map or
/// cache.
///
/// # Errors
///
/// Returns [`ContentStreamDataExtentInspectionError`] for dispatch-time
/// stream-start failures, missing or duplicate `/Length`, an indirect `/Length`
/// without a supplied backend, unsupported `/Length` value kinds, delegated
/// classic direct/indirect focused-helper failures, or lookup-backed
/// indirect-length resolution failures with the underlying rejection reason
/// preserved.
pub fn inspect_content_stream_data_extent_with_lookup(
    input: &[u8],
    lookup: Option<ObjectLookup<'_>>,
    object_offset: usize,
) -> Result<ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionError> {
    let stream_start = inspect_content_stream_start(input, object_offset).map_err(|error| {
        content_stream_data_extent_error(
            input,
            object_offset,
            error.error_byte_offset,
            ContentStreamDataExtentInspectionRejection::StreamStart {
                stream_start_reason: error.reason,
            },
        )
    })?;

    let length_entry = find_length_entry(input, &stream_start).map_err(|issue| {
        content_stream_data_extent_entry_error(input, object_offset, &stream_start, issue)
    })?;

    match length_entry.value_kind {
        DictionaryValueKind::NumberLike => {
            inspect_direct_length_content_stream_data_extent(input, object_offset)
                .map(ContentStreamDataExtentInspection::DirectLength)
                .map_err(|error| {
                    content_stream_data_extent_error(
                        input,
                        object_offset,
                        error.error_byte_offset,
                        ContentStreamDataExtentInspectionRejection::DirectLength {
                            direct_length_reason: error.reason,
                        },
                    )
                })
        }
        DictionaryValueKind::IndirectReferenceLike => {
            let Some(lookup) = lookup else {
                return Err(content_stream_data_extent_error(
                    input,
                    object_offset,
                    Some(length_entry.value_range.start),
                    ContentStreamDataExtentInspectionRejection::IndirectLengthRequiresXrefTable,
                ));
            };

            match lookup {
                ObjectLookup::ClassicXref(xref_table) => {
                    inspect_indirect_length_content_stream_data_extent(
                        input,
                        xref_table,
                        object_offset,
                    )
                    .map(ContentStreamDataExtentInspection::IndirectLength)
                    .map_err(|error| {
                        content_stream_data_extent_error(
                            input,
                            object_offset,
                            error.error_byte_offset,
                            ContentStreamDataExtentInspectionRejection::IndirectLength {
                                indirect_length_reason: error.reason,
                            },
                        )
                    })
                }
                other @ ObjectLookup::XrefStreamSection(_) => resolve_indirect_length_via_lookup(
                    input,
                    other,
                    object_offset,
                    stream_start,
                    length_entry,
                ),
            }
        }
        value_kind => Err(content_stream_data_extent_error(
            input,
            object_offset,
            Some(length_entry.value_range.start),
            ContentStreamDataExtentInspectionRejection::UnsupportedLengthValueKind { value_kind },
        )),
    }
}

/// Resolve an indirect `/Length` content-stream extent over a non-classic
/// [`ObjectLookup`] backend, reusing the shared object-resolution machinery.
///
/// The caller has already validated the stream start and classified exactly one
/// `IndirectReferenceLike` top-level `/Length` entry, so this routes the
/// reference through [`resolve_xref_object_offset`], reads the referenced
/// non-negative integer, computes the checked stream-data end, and validates the
/// `endstream` terminator. It owns no source bytes; `stream_start` is moved in
/// to fill the report without an extra inspection or copy.
fn resolve_indirect_length_via_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_offset: usize,
    stream_start: ContentStreamStartInspection,
    length_entry: DictionaryEntrySpan,
) -> Result<ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionError> {
    let length_resolution =
        resolve_lookup_integer(input, lookup, object_offset, length_entry.value_range.start)?;
    let length = length_resolution.value;

    let stream_data_start_byte_offset = stream_start.stream_data_start_byte_offset;
    let stream_data_end_byte_offset = stream_data_start_byte_offset
        .checked_add(length)
        .ok_or_else(|| {
            lookup_indirect_length_error(
                input,
                object_offset,
                Some(stream_data_start_byte_offset),
                LookupIndirectLengthRejection::StreamDataEndOverflow,
            )
        })?;

    validate_lookup_endstream(input, object_offset, stream_data_end_byte_offset)?;

    Ok(ContentStreamDataExtentInspection::IndirectLength(
        IndirectLengthContentStreamDataExtentInspection {
            stream_start,
            length_key_range: length_entry.key_range,
            length_value_range: length_entry.value_range,
            length_resolution,
            length,
            stream_data_start_byte_offset,
            stream_data_end_byte_offset,
        },
    ))
}

/// Resolve an `N G R` `/Length` reference to a non-negative integer object value
/// through a borrowed [`ObjectLookup`] backend.
///
/// This mirrors the classic one-level integer-object resolution but routes the
/// object location through [`resolve_xref_object_offset`], which validates the
/// matching generation and the indirect object header at the resolved offset and
/// reports compressed, reserved, free, missing, out-of-range, and
/// generation-mismatched results as structured failures. It resolves exactly one
/// reference one level deep and retains or copies no PDF bytes.
fn resolve_lookup_integer(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_offset: usize,
    value_byte_offset: usize,
) -> Result<ClassicXrefIntegerObjectResolution, ContentStreamDataExtentInspectionError> {
    let reference = parse_indirect_reference(input, value_byte_offset)
        .map_err(|error| {
            lookup_indirect_length_error(
                input,
                object_offset,
                error.error_byte_offset,
                LookupIndirectLengthRejection::Reference {
                    reference_reason: error.reason,
                },
            )
        })?
        .reference;

    let resolved = resolve_xref_object_offset(input, lookup, reference).map_err(|error| {
        lookup_indirect_length_error(
            input,
            object_offset,
            error.error_byte_offset,
            LookupIndirectLengthRejection::ObjectResolution {
                object_resolution_reason: error.reason,
            },
        )
    })?;
    let object_byte_offset = resolved.object_byte_offset;

    let header = inspect_indirect_object_header(input, object_byte_offset).map_err(|error| {
        lookup_indirect_length_error(
            input,
            object_offset,
            error.error_byte_offset,
            LookupIndirectLengthRejection::ObjectResolution {
                object_resolution_reason: ObjectResolutionRejection::ObjectHeader {
                    header_reason: error.reason,
                },
            },
        )
    })?;

    let body = inspect_indirect_object_body_token(input, header.after_obj_keyword_offset).map_err(
        |error| {
            lookup_indirect_length_error(
                input,
                object_offset,
                error.error_byte_offset,
                LookupIndirectLengthRejection::BodyToken {
                    body_token_reason: error.reason,
                },
            )
        },
    )?;

    if body.token_kind != IndirectObjectBodyLeadingTokenKind::NumberLike {
        return Err(lookup_indirect_length_error(
            input,
            object_offset,
            Some(body.first_token_byte_offset),
            LookupIndirectLengthRejection::NonIntegerBody {
                token_kind: body.token_kind,
            },
        ));
    }

    let (value_range, value) = parse_lookup_integer_body(input, body.first_token_byte_offset)
        .map_err(|(offset, reason)| {
            lookup_indirect_length_error(input, object_offset, Some(offset), reason)
        })?;

    Ok(ClassicXrefIntegerObjectResolution {
        reference,
        object_byte_offset,
        value_range,
        value,
    })
}

/// Parse a non-negative ASCII-digit integer beginning at `first_token_byte_offset`.
///
/// The digit run must be non-empty and terminated by PDF whitespace, a
/// delimiter, or the `endobj` keyword; otherwise it is rejected as malformed. A
/// value that does not fit `usize` is a distinct out-of-range rejection.
fn parse_lookup_integer_body(
    input: &[u8],
    first_token_byte_offset: usize,
) -> Result<(IntegerObjectValueByteRange, usize), (usize, LookupIndirectLengthRejection)> {
    let value_start = first_token_byte_offset;
    let value_end = value_start + count_leading_digits(&input[value_start..]);
    if value_end == value_start || !integer_terminated(input, value_end) {
        return Err((value_start, LookupIndirectLengthRejection::MalformedInteger));
    }

    let value = parse_usize_decimal(&input[value_start..value_end]).ok_or((
        value_start,
        LookupIndirectLengthRejection::IntegerOutOfRange,
    ))?;

    Ok((
        IntegerObjectValueByteRange {
            start: value_start,
            end: value_end,
        },
        value,
    ))
}

/// Accept only an integer digit run terminated by PDF whitespace, a delimiter,
/// or the exact `endobj` keyword. A trailing non-delimiter byte (including the
/// decimal point or a sign) and end-of-file are rejected as malformed.
fn integer_terminated(input: &[u8], after_digits: usize) -> bool {
    match input.get(after_digits) {
        Some(&byte) if is_pdf_whitespace(byte) || is_pdf_delimiter(byte) => true,
        Some(_) => consume_keyword(&input[after_digits..], ENDOBJ_KEYWORD).is_some(),
        None => false,
    }
}

/// Validate the `endstream` terminator at the computed stream-data end offset.
///
/// This mirrors the classic indirect helper's terminator validation: the
/// computed end must be in bounds, followed by an LF or CRLF marker (never a
/// lone CR), then the exact `endstream` keyword.
fn validate_lookup_endstream(
    input: &[u8],
    object_offset: usize,
    stream_data_end_byte_offset: usize,
) -> Result<(), ContentStreamDataExtentInspectionError> {
    if stream_data_end_byte_offset > input.len() {
        return Err(lookup_indirect_length_error(
            input,
            object_offset,
            Some(stream_data_end_byte_offset),
            LookupIndirectLengthRejection::StreamDataEndOutOfBounds,
        ));
    }

    let after_eol_offset = match input.get(stream_data_end_byte_offset) {
        Some(b'\n') => stream_data_end_byte_offset + 1,
        Some(b'\r') => {
            if input.get(stream_data_end_byte_offset + 1) == Some(&b'\n') {
                stream_data_end_byte_offset + 2
            } else {
                return Err(lookup_indirect_length_error(
                    input,
                    object_offset,
                    Some(stream_data_end_byte_offset),
                    LookupIndirectLengthRejection::InvalidEndstreamEol {
                        eol_issue: StreamEolIssue::LoneCarriageReturn,
                    },
                ));
            }
        }
        Some(_) => {
            return Err(lookup_indirect_length_error(
                input,
                object_offset,
                Some(stream_data_end_byte_offset),
                LookupIndirectLengthRejection::InvalidEndstreamEol {
                    eol_issue: StreamEolIssue::NotEndOfLine,
                },
            ));
        }
        None => {
            return Err(lookup_indirect_length_error(
                input,
                object_offset,
                Some(stream_data_end_byte_offset),
                LookupIndirectLengthRejection::InvalidEndstreamEol {
                    eol_issue: StreamEolIssue::EndOfFile,
                },
            ));
        }
    };

    if consume_keyword(&input[after_eol_offset..], ENDSTREAM_KEYWORD).is_none() {
        return Err(lookup_indirect_length_error(
            input,
            object_offset,
            Some(after_eol_offset),
            LookupIndirectLengthRejection::MissingEndstreamKeyword,
        ));
    }

    Ok(())
}

const fn content_stream_data_extent_entry_error(
    input: &[u8],
    byte_offset: usize,
    stream_start: &ContentStreamStartInspection,
    issue: LengthEntryIssue,
) -> ContentStreamDataExtentInspectionError {
    match issue {
        LengthEntryIssue::Missing => content_stream_data_extent_error(
            input,
            byte_offset,
            Some(stream_start.dictionary.dictionary_close_byte_offset),
            ContentStreamDataExtentInspectionRejection::MissingLength,
        ),
        LengthEntryIssue::Duplicate(first_key_range, duplicate_key_range) => {
            content_stream_data_extent_error(
                input,
                byte_offset,
                Some(duplicate_key_range.start),
                ContentStreamDataExtentInspectionRejection::DuplicateLength {
                    first_key_range,
                    duplicate_key_range,
                },
            )
        }
    }
}

const fn content_stream_data_extent_error(
    input: &[u8],
    byte_offset: usize,
    error_byte_offset: Option<usize>,
    reason: ContentStreamDataExtentInspectionRejection,
) -> ContentStreamDataExtentInspectionError {
    ContentStreamDataExtentInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset,
        reason,
    }
}

const fn lookup_indirect_length_error(
    input: &[u8],
    byte_offset: usize,
    error_byte_offset: Option<usize>,
    reason: LookupIndirectLengthRejection,
) -> ContentStreamDataExtentInspectionError {
    content_stream_data_extent_error(
        input,
        byte_offset,
        error_byte_offset,
        ContentStreamDataExtentInspectionRejection::LookupIndirectLength {
            lookup_indirect_length_reason: reason,
        },
    )
}
