use serde::{Deserialize, Serialize};

use crate::DictionaryExtentInspectionRejection;
use crate::source_utils::{consume_keyword, skip_whitespace};

const TRAILER_KEYWORD: &[u8] = b"trailer";

/// Balanced byte extent of the dictionary following a classic xref `trailer`.
///
/// This report stores only byte offsets and a small depth scalar. It does not
/// retain or copy trailer dictionary bytes, keys, values, object bodies, stream
/// bodies, or indirect references.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicXrefTrailerDictionaryInspection {
    /// Caller-supplied byte offset where trailer inspection began.
    pub byte_offset: usize,
    /// Byte offset where the resolved `trailer` keyword begins.
    pub trailer_byte_offset: usize,
    /// Byte offset of the opening `<<` after the `trailer` keyword and
    /// optional PDF whitespace.
    pub dictionary_open_byte_offset: usize,
    /// Byte offset of the matching closing `>>` for the outermost dictionary.
    pub dictionary_close_byte_offset: usize,
    /// Exclusive byte offset immediately after the closing `>>`.
    pub after_dictionary_close_byte_offset: usize,
    /// Deepest `<<` nesting depth observed; `1` for a flat trailer dictionary.
    pub max_observed_dictionary_depth: usize,
}

/// Error returned when a classic xref trailer dictionary cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicXrefTrailerDictionaryInspectionError {
    /// Caller-supplied byte offset where trailer inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the resolved `trailer` keyword begins, when present.
    pub trailer_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: ClassicXrefTrailerDictionaryInspectionRejection,
}

/// Structured classic xref trailer dictionary inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ClassicXrefTrailerDictionaryInspectionRejection {
    /// The caller-supplied offset lies beyond the source length or exactly at
    /// EOF.
    OffsetOutOfBounds,
    /// The resolved offset does not point at a `trailer` keyword.
    MissingTrailerKeyword,
    /// The `trailer` keyword was present, but the following dictionary extent
    /// could not be located.
    DictionaryExtent {
        /// Underlying dictionary extent rejection reason.
        dictionary_reason: DictionaryExtentInspectionRejection,
    },
}

/// Inspect the balanced trailer dictionary following a classic xref `trailer`.
///
/// The helper skips optional PDF whitespace at `byte_offset`, validates the
/// `trailer` keyword at the resolved offset using the shared keyword-boundary
/// rules, skips optional PDF whitespace after the keyword, then delegates
/// dictionary balancing to [`crate::inspect_dictionary_extent`].
///
/// It performs no filesystem I/O, interprets no trailer keys or values, and
/// never retains or copies trailer dictionary bytes.
///
/// # Errors
///
/// Returns [`ClassicXrefTrailerDictionaryInspectionError`] when the offset is
/// at or beyond EOF, the resolved offset does not contain a bounded `trailer`
/// keyword, or the following dictionary extent is rejected.
pub fn inspect_classic_xref_trailer_dictionary(
    input: &[u8],
    byte_offset: usize,
) -> Result<ClassicXrefTrailerDictionaryInspection, ClassicXrefTrailerDictionaryInspectionError> {
    if byte_offset >= input.len() {
        return Err(trailer_dictionary_error(
            input,
            byte_offset,
            None,
            ClassicXrefTrailerDictionaryInspectionRejection::OffsetOutOfBounds,
            None,
        ));
    }

    let trailer_byte_offset = byte_offset + skip_whitespace(&input[byte_offset..]);
    let Some(after_trailer_keyword) = input
        .get(trailer_byte_offset..)
        .and_then(|content| consume_keyword(content, TRAILER_KEYWORD))
    else {
        return Err(trailer_dictionary_error(
            input,
            byte_offset,
            None,
            ClassicXrefTrailerDictionaryInspectionRejection::MissingTrailerKeyword,
            Some(trailer_byte_offset),
        ));
    };

    let dictionary_offset = trailer_byte_offset + after_trailer_keyword;
    let dictionary =
        crate::inspect_dictionary_extent(input, dictionary_offset).map_err(|error| {
            trailer_dictionary_error(
                input,
                byte_offset,
                Some(trailer_byte_offset),
                ClassicXrefTrailerDictionaryInspectionRejection::DictionaryExtent {
                    dictionary_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;

    Ok(ClassicXrefTrailerDictionaryInspection {
        byte_offset,
        trailer_byte_offset,
        dictionary_open_byte_offset: dictionary.open_byte_offset,
        dictionary_close_byte_offset: dictionary.close_byte_offset,
        after_dictionary_close_byte_offset: dictionary.after_close_byte_offset,
        max_observed_dictionary_depth: dictionary.max_observed_depth,
    })
}

const fn trailer_dictionary_error(
    input: &[u8],
    byte_offset: usize,
    trailer_byte_offset: Option<usize>,
    reason: ClassicXrefTrailerDictionaryInspectionRejection,
    error_byte_offset: Option<usize>,
) -> ClassicXrefTrailerDictionaryInspectionError {
    ClassicXrefTrailerDictionaryInspectionError {
        byte_offset,
        byte_len: input.len(),
        trailer_byte_offset,
        error_byte_offset,
        reason,
    }
}
