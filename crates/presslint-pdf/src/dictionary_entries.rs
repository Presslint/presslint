use serde::{Deserialize, Serialize};

use crate::source_utils::{
    is_pdf_delimiter, is_pdf_whitespace, skip_comment, skip_hex_string, skip_literal_string,
    skip_whitespace,
};
use crate::{
    ArrayExtentInspectionRejection, DictionaryExtentInspection, DictionaryExtentInspectionRejection,
};

/// Shallow top-level entry spans from a bounded PDF dictionary.
///
/// This report stores only byte offsets and small enum values. It does not
/// retain or copy dictionary bytes, key names, values, object bodies, or stream
/// bodies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntryInspection {
    /// Balanced extent of the outer dictionary that was scanned.
    pub dictionary: DictionaryExtentInspection,
    /// Top-level `/Name value` entries in source order.
    pub entries: Vec<DictionaryEntrySpan>,
}

/// Byte ranges for one top-level dictionary entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntrySpan {
    /// Inclusive start and exclusive end of the key name object, including `/`.
    pub key_range: DictionaryEntryByteRange,
    /// Inclusive start and exclusive end of the shallow value span.
    pub value_range: DictionaryEntryByteRange,
    /// Shallow structural value family.
    pub value_kind: DictionaryValueKind,
}

/// Inclusive start and exclusive end byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntryByteRange {
    /// Inclusive start byte offset.
    pub start: usize,
    /// Exclusive end byte offset.
    pub end: usize,
}

/// Shallow family for a dictionary entry value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryValueKind {
    /// A nested `<< ... >>` dictionary value.
    Dictionary,
    /// A `[ ... ]` array value.
    Array,
    /// A `/Name` value.
    Name,
    /// A literal string `( ... )` or hex string `< ... >` value.
    String,
    /// A number-shaped scalar value.
    NumberLike,
    /// A `true` or `false` scalar value.
    Boolean,
    /// A `null` scalar value.
    Null,
    /// A scalar shaped as `N G R`.
    IndirectReferenceLike,
    /// Any other shallow scalar span.
    OtherScalar,
}

/// Error returned when top-level dictionary entries cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryEntryInspectionError {
    /// Caller-supplied byte offset where dictionary entry inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset of the outer dictionary open, when it was located.
    pub dictionary_open_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: DictionaryEntryInspectionRejection,
}

/// Structured dictionary entry inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DictionaryEntryInspectionRejection {
    /// A delegated dictionary extent could not be located.
    DictionaryExtent {
        /// Underlying dictionary extent rejection reason.
        dictionary_reason: DictionaryExtentInspectionRejection,
    },
    /// A delegated array extent could not be located.
    ArrayExtent {
        /// Underlying array extent rejection reason.
        array_reason: ArrayExtentInspectionRejection,
    },
    /// A top-level entry key was expected but the next token was not a name.
    NonNameTopLevelKey,
    /// A top-level key was present without a following value.
    MissingValue,
    /// A literal or hex string value was opened but not closed before EOF.
    UnterminatedString,
}

/// Inspect shallow top-level `/Name value` entry spans in a PDF dictionary.
///
/// The helper first bounds the outer dictionary with
/// [`crate::inspect_dictionary_extent`], then scans only the bytes between the
/// outer `<<` and matching `>>`. Nested dictionaries and arrays are skipped as
/// opaque values through the existing extent helpers. Literal strings, hex
/// strings, and comments are skipped as opaque spans while looking for the next
/// top-level key, so delimiters inside them never split entries.
///
/// It decodes no names, strings, numbers, booleans, nulls, indirect references,
/// or key semantics, and reports only byte ranges plus a shallow value kind.
///
/// # Errors
///
/// Returns [`DictionaryEntryInspectionError`] when the outer dictionary extent
/// is rejected, a top-level key is not a name, a key has no following value, or
/// a delegated nested dictionary/array/string span is rejected.
pub fn inspect_dictionary_entries(
    input: &[u8],
    byte_offset: usize,
) -> Result<DictionaryEntryInspection, DictionaryEntryInspectionError> {
    let dictionary = crate::inspect_dictionary_extent(input, byte_offset).map_err(|error| {
        entry_error(
            input,
            byte_offset,
            None,
            DictionaryEntryInspectionRejection::DictionaryExtent {
                dictionary_reason: error.reason,
            },
            error.error_byte_offset,
        )
    })?;

    let mut entries = Vec::new();
    let mut cursor = dictionary.open_byte_offset + 2;
    let body_end = dictionary.close_byte_offset;

    loop {
        cursor = skip_whitespace_and_comments(input, cursor, body_end);
        if cursor >= body_end {
            break;
        }

        if input[cursor] != b'/' {
            return Err(entry_error(
                input,
                byte_offset,
                Some(dictionary.open_byte_offset),
                DictionaryEntryInspectionRejection::NonNameTopLevelKey,
                Some(cursor),
            ));
        }

        let key_start = cursor;
        cursor = skip_name(input, cursor, body_end);
        let key_range = DictionaryEntryByteRange {
            start: key_start,
            end: cursor,
        };

        cursor = skip_whitespace_and_comments(input, cursor, body_end);
        if cursor >= body_end {
            return Err(entry_error(
                input,
                byte_offset,
                Some(dictionary.open_byte_offset),
                DictionaryEntryInspectionRejection::MissingValue,
                Some(cursor),
            ));
        }

        let value = scan_value(
            input,
            cursor,
            body_end,
            byte_offset,
            dictionary.open_byte_offset,
        )?;
        entries.push(DictionaryEntrySpan {
            key_range,
            value_range: value.range,
            value_kind: value.kind,
        });
        cursor = value.range.end;
    }

    Ok(DictionaryEntryInspection {
        dictionary,
        entries,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ScannedValue {
    range: DictionaryEntryByteRange,
    kind: DictionaryValueKind,
}

fn scan_value(
    input: &[u8],
    value_start: usize,
    body_end: usize,
    byte_offset: usize,
    dictionary_open_byte_offset: usize,
) -> Result<ScannedValue, DictionaryEntryInspectionError> {
    match input[value_start] {
        b'<' if input.get(value_start + 1) == Some(&b'<') => {
            let dictionary =
                crate::inspect_dictionary_extent(input, value_start).map_err(|error| {
                    entry_error(
                        input,
                        byte_offset,
                        Some(dictionary_open_byte_offset),
                        DictionaryEntryInspectionRejection::DictionaryExtent {
                            dictionary_reason: error.reason,
                        },
                        error.error_byte_offset,
                    )
                })?;
            Ok(scanned_value(
                value_start,
                dictionary.after_close_byte_offset,
                DictionaryValueKind::Dictionary,
            ))
        }
        b'[' => {
            let array = crate::inspect_array_extent(input, value_start).map_err(|error| {
                entry_error(
                    input,
                    byte_offset,
                    Some(dictionary_open_byte_offset),
                    DictionaryEntryInspectionRejection::ArrayExtent {
                        array_reason: error.reason,
                    },
                    error.error_byte_offset,
                )
            })?;
            Ok(scanned_value(
                value_start,
                array.after_close_byte_offset,
                DictionaryValueKind::Array,
            ))
        }
        b'(' => {
            let value_end = skip_literal_string(input, value_start).ok_or_else(|| {
                entry_error(
                    input,
                    byte_offset,
                    Some(dictionary_open_byte_offset),
                    DictionaryEntryInspectionRejection::UnterminatedString,
                    Some(value_start),
                )
            })?;
            Ok(scanned_value(
                value_start,
                value_end,
                DictionaryValueKind::String,
            ))
        }
        b'<' => {
            let value_end = skip_hex_string(input, value_start).ok_or_else(|| {
                entry_error(
                    input,
                    byte_offset,
                    Some(dictionary_open_byte_offset),
                    DictionaryEntryInspectionRejection::UnterminatedString,
                    Some(value_start),
                )
            })?;
            Ok(scanned_value(
                value_start,
                value_end,
                DictionaryValueKind::String,
            ))
        }
        b'/' => {
            let value_end = skip_name(input, value_start, body_end);
            Ok(scanned_value(
                value_start,
                value_end,
                DictionaryValueKind::Name,
            ))
        }
        _ => scan_scalar_value(
            input,
            value_start,
            body_end,
            byte_offset,
            dictionary_open_byte_offset,
        ),
    }
}

fn scan_scalar_value(
    input: &[u8],
    value_start: usize,
    body_end: usize,
    byte_offset: usize,
    dictionary_open_byte_offset: usize,
) -> Result<ScannedValue, DictionaryEntryInspectionError> {
    let mut cursor = value_start;
    let mut value_end = value_start;
    let mut token_count = 0usize;
    let mut first_tokens = [(0usize, 0usize); 3];

    while cursor < body_end {
        match input[cursor] {
            byte if is_pdf_whitespace(byte) => cursor += skip_whitespace(&input[cursor..body_end]),
            b'%' => cursor = skip_comment(input, cursor).min(body_end),
            b'/' => break,
            b'(' => {
                cursor = skip_literal_string(input, cursor).ok_or_else(|| {
                    entry_error(
                        input,
                        byte_offset,
                        Some(dictionary_open_byte_offset),
                        DictionaryEntryInspectionRejection::UnterminatedString,
                        Some(cursor),
                    )
                })?;
                value_end = cursor;
            }
            b'<' if input.get(cursor + 1) == Some(&b'<') => {
                let dictionary =
                    crate::inspect_dictionary_extent(input, cursor).map_err(|error| {
                        entry_error(
                            input,
                            byte_offset,
                            Some(dictionary_open_byte_offset),
                            DictionaryEntryInspectionRejection::DictionaryExtent {
                                dictionary_reason: error.reason,
                            },
                            error.error_byte_offset,
                        )
                    })?;
                cursor = dictionary.after_close_byte_offset;
                value_end = cursor;
            }
            b'<' => {
                cursor = skip_hex_string(input, cursor).ok_or_else(|| {
                    entry_error(
                        input,
                        byte_offset,
                        Some(dictionary_open_byte_offset),
                        DictionaryEntryInspectionRejection::UnterminatedString,
                        Some(cursor),
                    )
                })?;
                value_end = cursor;
            }
            b'[' => {
                let array = crate::inspect_array_extent(input, cursor).map_err(|error| {
                    entry_error(
                        input,
                        byte_offset,
                        Some(dictionary_open_byte_offset),
                        DictionaryEntryInspectionRejection::ArrayExtent {
                            array_reason: error.reason,
                        },
                        error.error_byte_offset,
                    )
                })?;
                cursor = array.after_close_byte_offset;
                value_end = cursor;
            }
            _ => {
                let token_start = cursor;
                cursor = skip_scalar_token(input, cursor, body_end);
                if token_count < first_tokens.len() {
                    first_tokens[token_count] = (token_start, cursor);
                }
                token_count += 1;
                value_end = cursor;
            }
        }
    }

    let kind = classify_scalar(input, token_count, first_tokens);
    Ok(scanned_value(value_start, value_end, kind))
}

const fn scanned_value(
    value_start: usize,
    value_end: usize,
    kind: DictionaryValueKind,
) -> ScannedValue {
    ScannedValue {
        range: DictionaryEntryByteRange {
            start: value_start,
            end: value_end,
        },
        kind,
    }
}

fn classify_scalar(
    input: &[u8],
    token_count: usize,
    first_tokens: [(usize, usize); 3],
) -> DictionaryValueKind {
    if token_count == 3
        && token_is_unsigned_integer(input, first_tokens[0])
        && token_is_unsigned_integer(input, first_tokens[1])
        && token_bytes(input, first_tokens[2]) == b"R"
    {
        return DictionaryValueKind::IndirectReferenceLike;
    }

    if token_count != 1 {
        return DictionaryValueKind::OtherScalar;
    }

    let token = token_bytes(input, first_tokens[0]);
    match token {
        b"true" | b"false" => DictionaryValueKind::Boolean,
        b"null" => DictionaryValueKind::Null,
        _ if token_is_number_like(token) => DictionaryValueKind::NumberLike,
        _ => DictionaryValueKind::OtherScalar,
    }
}

fn token_bytes(input: &[u8], token: (usize, usize)) -> &[u8] {
    &input[token.0..token.1]
}

fn token_is_unsigned_integer(input: &[u8], token: (usize, usize)) -> bool {
    let bytes = token_bytes(input, token);
    !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit)
}

fn token_is_number_like(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|byte| byte.is_ascii_digit() || matches!(*byte, b'+' | b'-' | b'.'))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(*byte, b'+' | b'-' | b'.'))
}

fn skip_whitespace_and_comments(input: &[u8], mut cursor: usize, limit: usize) -> usize {
    loop {
        cursor += skip_whitespace(&input[cursor..limit]);
        if cursor < limit && input[cursor] == b'%' {
            cursor = skip_comment(input, cursor).min(limit);
            continue;
        }
        return cursor;
    }
}

fn skip_name(input: &[u8], start: usize, limit: usize) -> usize {
    let mut cursor = start + 1;
    while cursor < limit && !is_pdf_whitespace(input[cursor]) && !is_pdf_delimiter(input[cursor]) {
        cursor += 1;
    }
    cursor
}

fn skip_scalar_token(input: &[u8], start: usize, limit: usize) -> usize {
    let mut cursor = start;
    while cursor < limit && !is_pdf_whitespace(input[cursor]) && !is_pdf_delimiter(input[cursor]) {
        cursor += 1;
    }
    if cursor == start { cursor + 1 } else { cursor }
}

const fn entry_error(
    input: &[u8],
    byte_offset: usize,
    dictionary_open_byte_offset: Option<usize>,
    reason: DictionaryEntryInspectionRejection,
    error_byte_offset: Option<usize>,
) -> DictionaryEntryInspectionError {
    DictionaryEntryInspectionError {
        byte_offset,
        byte_len: input.len(),
        dictionary_open_byte_offset,
        error_byte_offset,
        reason,
    }
}
