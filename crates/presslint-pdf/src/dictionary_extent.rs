use serde::{Deserialize, Serialize};

use crate::source_utils::{skip_comment, skip_hex_string, skip_literal_string, skip_whitespace};

/// Maximum `<<` nesting depth this helper tracks before rejecting.
///
/// Kept private to bound pathological inputs: once the open-delimiter depth
/// reaches this constant, a further `<<` yields a structured
/// [`DictionaryExtentInspectionRejection::MaxNestingExceeded`] rather than
/// unbounded work.
const MAX_DICTIONARY_NESTING_DEPTH: usize = 256;

/// Balanced byte extent of a `<< ... >>` dictionary at a caller-supplied offset.
///
/// This report stores only byte offsets and a small depth scalar. It does not
/// retain or copy PDF bytes, and it interprets no key, value, name, number, or
/// indirect reference inside the dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryExtentInspection {
    /// Caller-supplied byte offset where extent inspection began.
    pub byte_offset: usize,
    /// Byte offset of the opening `<<` after optional PDF whitespace.
    pub open_byte_offset: usize,
    /// Byte offset of the matching closing `>>` for the outermost `<<`.
    pub close_byte_offset: usize,
    /// Exclusive byte offset immediately after the closing `>>`.
    pub after_close_byte_offset: usize,
    /// Deepest `<<` nesting depth observed; `1` for a flat dictionary.
    pub max_observed_depth: usize,
}

/// Error returned when a dictionary extent cannot be located.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DictionaryExtentInspectionError {
    /// Caller-supplied byte offset where extent inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: DictionaryExtentInspectionRejection,
}

/// Structured dictionary extent inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DictionaryExtentInspectionRejection {
    /// The caller-supplied offset lies beyond the source length or exactly at
    /// EOF.
    OffsetOutOfBounds,
    /// The offset and following bytes contain only PDF whitespace before EOF.
    NoSignificantToken,
    /// The first significant token is not the `<<` dictionary-open delimiter.
    NotDictionaryOpen,
    /// A literal or hex string was opened but not closed before EOF.
    UnterminatedString,
    /// EOF was reached before the `<<` depth returned to zero.
    UnterminatedDictionary,
    /// The `<<` nesting depth exceeded the bounded maximum.
    MaxNestingExceeded,
}

/// Locate the balanced `<< ... >>` extent of a dictionary at a byte offset.
///
/// The helper skips optional PDF whitespace at `byte_offset`, requires the first
/// significant token to be the `<<` dictionary-open delimiter, and scans a
/// single bounded forward pass to the matching close of the outermost `<<`. It
/// increments/decrements a `<<`/`>>` depth counter so a nested sub-dictionary
/// does not close the outer one. Literal strings `( ... )`, hex strings
/// `< ... >` (a `<` not followed by `<`), and `%` comments are skipped as
/// opaque spans so delimiter bytes inside them never affect the depth count.
///
/// It performs no filesystem I/O, decodes no string or name contents, and never
/// retains or copies PDF bytes; only byte offsets and a depth scalar are
/// reported.
///
/// # Errors
///
/// Returns [`DictionaryExtentInspectionError`] when the offset is at or beyond
/// EOF, only whitespace remains before EOF, the first significant token is not
/// `<<`, a literal or hex string is unterminated, the dictionary is unterminated
/// before the depth returns to zero, or the bounded nesting depth is exceeded.
pub fn inspect_dictionary_extent(
    input: &[u8],
    byte_offset: usize,
) -> Result<DictionaryExtentInspection, DictionaryExtentInspectionError> {
    if byte_offset >= input.len() {
        return Err(extent_error(
            input,
            byte_offset,
            DictionaryExtentInspectionRejection::OffsetOutOfBounds,
            None,
        ));
    }

    let leading_whitespace = skip_whitespace(&input[byte_offset..]);
    let open_byte_offset = byte_offset + leading_whitespace;
    if open_byte_offset == input.len() {
        return Err(extent_error(
            input,
            byte_offset,
            DictionaryExtentInspectionRejection::NoSignificantToken,
            Some(open_byte_offset),
        ));
    }

    if input[open_byte_offset] != b'<' || input.get(open_byte_offset + 1) != Some(&b'<') {
        return Err(extent_error(
            input,
            byte_offset,
            DictionaryExtentInspectionRejection::NotDictionaryOpen,
            Some(open_byte_offset),
        ));
    }

    let mut cursor = open_byte_offset + 2;
    let mut depth: usize = 1;
    let mut max_observed_depth: usize = 1;

    // Both opaque-string branches reject identically at the string's open offset.
    let unterminated_string = |at: usize| {
        extent_error(
            input,
            byte_offset,
            DictionaryExtentInspectionRejection::UnterminatedString,
            Some(at),
        )
    };

    while let Some(&byte) = input.get(cursor) {
        match byte {
            b'%' => cursor = skip_comment(input, cursor),
            b'(' => {
                cursor = skip_literal_string(input, cursor)
                    .ok_or_else(|| unterminated_string(cursor))?;
            }
            b'<' if input.get(cursor + 1) == Some(&b'<') => {
                if depth == MAX_DICTIONARY_NESTING_DEPTH {
                    return Err(extent_error(
                        input,
                        byte_offset,
                        DictionaryExtentInspectionRejection::MaxNestingExceeded,
                        Some(cursor),
                    ));
                }
                depth += 1;
                max_observed_depth = max_observed_depth.max(depth);
                cursor += 2;
            }
            b'<' => {
                cursor =
                    skip_hex_string(input, cursor).ok_or_else(|| unterminated_string(cursor))?;
            }
            b'>' if input.get(cursor + 1) == Some(&b'>') => {
                depth -= 1;
                let after_close_byte_offset = cursor + 2;
                if depth == 0 {
                    return Ok(DictionaryExtentInspection {
                        byte_offset,
                        open_byte_offset,
                        close_byte_offset: cursor,
                        after_close_byte_offset,
                        max_observed_depth,
                    });
                }
                cursor = after_close_byte_offset;
            }
            _ => cursor += 1,
        }
    }

    Err(extent_error(
        input,
        byte_offset,
        DictionaryExtentInspectionRejection::UnterminatedDictionary,
        None,
    ))
}

const fn extent_error(
    input: &[u8],
    byte_offset: usize,
    reason: DictionaryExtentInspectionRejection,
    error_byte_offset: Option<usize>,
) -> DictionaryExtentInspectionError {
    DictionaryExtentInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset,
        reason,
    }
}
