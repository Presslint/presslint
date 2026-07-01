use serde::{Deserialize, Serialize};

use crate::{
    DictionaryEntryInspectionRejection, DictionaryEntrySpan, IndirectObjectBodyLeadingTokenKind,
    IndirectObjectBodyTokenInspectionRejection, IndirectObjectHeaderByteRange,
    IndirectObjectHeaderInspectionRejection, IndirectRef, ResolvedObjectData,
    inspect_dictionary_entries, inspect_indirect_object_body_token,
};

/// Top-level dictionary entry spans of a dictionary-bodied indirect object.
///
/// This report stores only byte offsets, ranges, a small depth scalar, the
/// parsed `IndirectRef`, and the delegated top-level entry spans. It does not
/// retain or copy PDF bytes, object bodies, stream bodies, key bytes, or value
/// bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectObjectDictionaryInspection {
    /// Parsed indirect object identifier from the resolved header.
    pub reference: IndirectRef,
    /// Byte range covering `object_number generation obj`.
    pub header_range: IndirectObjectHeaderByteRange,
    /// Byte offset of the body's opening `<<` after the header and optional
    /// PDF whitespace.
    pub dictionary_open_byte_offset: usize,
    /// Byte offset of the matching closing `>>` for the outermost dictionary.
    pub dictionary_close_byte_offset: usize,
    /// Exclusive byte offset immediately after the closing `>>`.
    pub after_dictionary_close_byte_offset: usize,
    /// Deepest `<<` nesting depth observed; `1` for a flat object dictionary.
    pub max_observed_dictionary_depth: usize,
    /// Top-level `/Name value` entries in source order.
    pub entries: Vec<DictionaryEntrySpan>,
}

/// Error returned when an indirect object's top-level dictionary entries cannot
/// be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectObjectDictionaryInspectionError {
    /// Caller-supplied byte offset where indirect object inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the resolved object header begins, when it was located.
    pub header_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: IndirectObjectDictionaryInspectionRejection,
}

/// Structured indirect object dictionary inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum IndirectObjectDictionaryInspectionRejection {
    /// A delegated indirect object header inspection failed.
    Header {
        /// Underlying header inspection rejection reason.
        header_reason: IndirectObjectHeaderInspectionRejection,
    },
    /// A delegated indirect object body leading-token classification failed.
    BodyToken {
        /// Underlying body token inspection rejection reason.
        body_token_reason: IndirectObjectBodyTokenInspectionRejection,
    },
    /// The object body's leading token is not a dictionary open (`<<`).
    NonDictionaryBody {
        /// Classified leading token family that was not a dictionary open.
        token_kind: IndirectObjectBodyLeadingTokenKind,
    },
    /// A delegated top-level dictionary entry inspection failed.
    DictionaryEntries {
        /// Underlying dictionary entry rejection reason.
        dictionary_entries_reason: DictionaryEntryInspectionRejection,
    },
}

/// Inspect an indirect object's top-level dictionary entry spans.
///
/// The helper composes existing bounded inspectors: it resolves the object
/// header with [`crate::inspect_indirect_object_header`], classifies the body's
/// leading token with [`crate::inspect_indirect_object_body_token`], requires
/// that token to be the dictionary-open `<<`, and scans the top-level
/// `/Name value` spans with [`crate::inspect_dictionary_entries`].
///
/// It is the object-level sibling of
/// [`crate::inspect_classic_xref_trailer_dictionary`]: where that helper bridges
/// a `trailer` keyword to a dictionary, this one bridges an `N G obj` header to
/// a dictionary-bodied object's top-level entries.
///
/// The report carries only the resolved header byte range, the parsed
/// `IndirectRef`, the dictionary open/close/after offsets, the maximum observed
/// dictionary nesting depth, and the delegated entry-span list. It interprets no
/// keys such as `/Type`, `/Pages`, or `/Contents`, resolves no indirect
/// references found in values, decodes no name escapes or key/value bytes, and
/// never retains or copies PDF bytes.
///
/// # Errors
///
/// Returns [`IndirectObjectDictionaryInspectionError`] for a delegated header
/// inspection failure, a delegated body-token classification failure (including
/// offsets at or beyond EOF surfaced by that helper), a non-dictionary body
/// leading token, or a delegated dictionary-entry inspection failure.
pub fn inspect_indirect_object_dictionary(
    input: &[u8],
    object_offset: usize,
) -> Result<IndirectObjectDictionaryInspection, IndirectObjectDictionaryInspectionError> {
    let header = crate::inspect_indirect_object_header(input, object_offset).map_err(|error| {
        object_dictionary_error(
            input,
            object_offset,
            None,
            IndirectObjectDictionaryInspectionRejection::Header {
                header_reason: error.reason,
            },
            error.error_byte_offset,
        )
    })?;

    let body_token =
        crate::inspect_indirect_object_body_token(input, header.after_obj_keyword_offset).map_err(
            |error| {
                object_dictionary_error(
                    input,
                    object_offset,
                    Some(header.header_byte_offset),
                    IndirectObjectDictionaryInspectionRejection::BodyToken {
                        body_token_reason: error.reason,
                    },
                    error.error_byte_offset,
                )
            },
        )?;

    if body_token.token_kind != IndirectObjectBodyLeadingTokenKind::DictionaryOpen {
        return Err(object_dictionary_error(
            input,
            object_offset,
            Some(header.header_byte_offset),
            IndirectObjectDictionaryInspectionRejection::NonDictionaryBody {
                token_kind: body_token.token_kind,
            },
            Some(body_token.first_token_byte_offset),
        ));
    }

    let entries = crate::inspect_dictionary_entries(input, body_token.first_token_byte_offset)
        .map_err(|error| {
            object_dictionary_error(
                input,
                object_offset,
                Some(header.header_byte_offset),
                IndirectObjectDictionaryInspectionRejection::DictionaryEntries {
                    dictionary_entries_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;

    Ok(IndirectObjectDictionaryInspection {
        reference: header.reference,
        header_range: header.header_range,
        dictionary_open_byte_offset: entries.dictionary.open_byte_offset,
        dictionary_close_byte_offset: entries.dictionary.close_byte_offset,
        after_dictionary_close_byte_offset: entries.dictionary.after_close_byte_offset,
        max_observed_dictionary_depth: entries.dictionary.max_observed_depth,
        entries: entries.entries,
    })
}

const fn object_dictionary_error(
    input: &[u8],
    byte_offset: usize,
    header_byte_offset: Option<usize>,
    reason: IndirectObjectDictionaryInspectionRejection,
    error_byte_offset: Option<usize>,
) -> IndirectObjectDictionaryInspectionError {
    IndirectObjectDictionaryInspectionError {
        byte_offset,
        byte_len: input.len(),
        header_byte_offset,
        error_byte_offset,
        reason,
    }
}

/// Top-level dictionary entry spans of a bare compressed object body.
///
/// A compressed object stream member is a bare object body with no
/// `N G obj ... endobj` wrapper, so this report carries the `reference` from the
/// resolving cross-reference entry rather than a parsed header, and its offsets
/// are relative to the extracted member body slice, not to the source `input`.
/// It retains or copies no PDF bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressedObjectDictionaryInspection {
    /// Requested indirect reference of the compressed object.
    pub reference: IndirectRef,
    /// Byte offset of the body's opening `<<` within the extracted member body.
    pub dictionary_open_byte_offset: usize,
    /// Byte offset of the matching closing `>>` within the extracted member
    /// body.
    pub dictionary_close_byte_offset: usize,
    /// Exclusive byte offset immediately after the closing `>>`.
    pub after_dictionary_close_byte_offset: usize,
    /// Deepest `<<` nesting depth observed; `1` for a flat object dictionary.
    pub max_observed_dictionary_depth: usize,
    /// Top-level `/Name value` entries in member-body order.
    pub entries: Vec<DictionaryEntrySpan>,
}

pub fn compressed_dictionary_as_indirect_object_dictionary(
    dictionary: CompressedObjectDictionaryInspection,
) -> IndirectObjectDictionaryInspection {
    IndirectObjectDictionaryInspection {
        reference: dictionary.reference,
        header_range: IndirectObjectHeaderByteRange { start: 0, end: 0 },
        dictionary_open_byte_offset: dictionary.dictionary_open_byte_offset,
        dictionary_close_byte_offset: dictionary.dictionary_close_byte_offset,
        after_dictionary_close_byte_offset: dictionary.after_dictionary_close_byte_offset,
        max_observed_dictionary_depth: dictionary.max_observed_dictionary_depth,
        entries: dictionary.entries,
    }
}

/// Dictionary inspection over body-aware resolved object data.
///
/// Uncompressed data delegates to [`inspect_indirect_object_dictionary`];
/// compressed data reports the member-body-relative
/// [`CompressedObjectDictionaryInspection`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedObjectDictionaryInspection {
    /// Inspection of an uncompressed object at its source byte offset.
    Uncompressed(IndirectObjectDictionaryInspection),
    /// Inspection of a compressed object's extracted member body.
    Compressed(CompressedObjectDictionaryInspection),
}

/// Error returned when a resolved object's dictionary cannot be inspected.
///
/// This report retains or copies no PDF bytes; it carries only an optional error
/// offset (relative to `input` for the uncompressed path or to the member body
/// for the compressed path) and the structured reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedObjectDictionaryInspectionError {
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: ResolvedObjectDictionaryInspectionRejection,
}

/// Structured resolved-object dictionary inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ResolvedObjectDictionaryInspectionRejection {
    /// The delegated uncompressed object-dictionary inspection failed.
    Uncompressed {
        /// Underlying object-dictionary rejection reason.
        object_dictionary_reason: IndirectObjectDictionaryInspectionRejection,
    },
    /// The compressed member body's leading token could not be classified.
    CompressedBodyToken {
        /// Underlying body-token classification rejection reason.
        body_token_reason: IndirectObjectBodyTokenInspectionRejection,
    },
    /// The compressed member body's leading token is not a dictionary open
    /// (`<<`).
    CompressedNonDictionaryBody {
        /// Classified leading token family that was not a dictionary open.
        token_kind: IndirectObjectBodyLeadingTokenKind,
    },
    /// A delegated top-level dictionary entry inspection of the compressed
    /// member body failed.
    CompressedDictionaryEntries {
        /// Underlying dictionary entry rejection reason.
        dictionary_entries_reason: DictionaryEntryInspectionRejection,
    },
}

pub const fn resolved_dictionary_rejection_as_indirect(
    reason: ResolvedObjectDictionaryInspectionRejection,
) -> IndirectObjectDictionaryInspectionRejection {
    match reason {
        ResolvedObjectDictionaryInspectionRejection::Uncompressed {
            object_dictionary_reason,
        } => object_dictionary_reason,
        ResolvedObjectDictionaryInspectionRejection::CompressedBodyToken { body_token_reason } => {
            IndirectObjectDictionaryInspectionRejection::BodyToken { body_token_reason }
        }
        ResolvedObjectDictionaryInspectionRejection::CompressedNonDictionaryBody { token_kind } => {
            IndirectObjectDictionaryInspectionRejection::NonDictionaryBody { token_kind }
        }
        ResolvedObjectDictionaryInspectionRejection::CompressedDictionaryEntries {
            dictionary_entries_reason,
        } => IndirectObjectDictionaryInspectionRejection::DictionaryEntries {
            dictionary_entries_reason,
        },
    }
}

/// Inspect the top-level dictionary entries of body-aware resolved object data.
///
/// [`ResolvedObjectData::Uncompressed`] delegates to
/// [`inspect_indirect_object_dictionary`] at the resolved source byte offset.
/// [`ResolvedObjectData::Compressed`] scans the extracted member body: it
/// requires a leading `<<` (a bare dictionary body, since compressed members
/// carry no indirect header) and reports member-body-relative entry spans.
///
/// # Errors
///
/// Returns [`ResolvedObjectDictionaryInspectionError`] for a delegated
/// uncompressed dictionary failure, or for a compressed member body whose
/// leading token cannot be classified, is not a dictionary open, or whose
/// top-level entries cannot be scanned.
pub fn inspect_object_dictionary(
    input: &[u8],
    resolved: &ResolvedObjectData,
) -> Result<ResolvedObjectDictionaryInspection, ResolvedObjectDictionaryInspectionError> {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => {
            let inspection = inspect_indirect_object_dictionary(input, resolved.object_byte_offset)
                .map_err(|error| ResolvedObjectDictionaryInspectionError {
                    error_byte_offset: error.error_byte_offset,
                    reason: ResolvedObjectDictionaryInspectionRejection::Uncompressed {
                        object_dictionary_reason: error.reason,
                    },
                })?;
            Ok(ResolvedObjectDictionaryInspection::Uncompressed(inspection))
        }
        ResolvedObjectData::Compressed {
            reference,
            decoded_object_stream,
            object_body_span,
            ..
        } => {
            let body = decoded_object_stream
                .get(object_body_span.start..object_body_span.end)
                .unwrap_or(&[]);
            inspect_compressed_object_dictionary(body, *reference)
                .map(ResolvedObjectDictionaryInspection::Compressed)
        }
    }
}

fn inspect_compressed_object_dictionary(
    body: &[u8],
    reference: IndirectRef,
) -> Result<CompressedObjectDictionaryInspection, ResolvedObjectDictionaryInspectionError> {
    let body_token = inspect_indirect_object_body_token(body, 0).map_err(|error| {
        ResolvedObjectDictionaryInspectionError {
            error_byte_offset: error.error_byte_offset,
            reason: ResolvedObjectDictionaryInspectionRejection::CompressedBodyToken {
                body_token_reason: error.reason,
            },
        }
    })?;

    if body_token.token_kind != IndirectObjectBodyLeadingTokenKind::DictionaryOpen {
        return Err(ResolvedObjectDictionaryInspectionError {
            error_byte_offset: Some(body_token.first_token_byte_offset),
            reason: ResolvedObjectDictionaryInspectionRejection::CompressedNonDictionaryBody {
                token_kind: body_token.token_kind,
            },
        });
    }

    let entries =
        inspect_dictionary_entries(body, body_token.first_token_byte_offset).map_err(|error| {
            ResolvedObjectDictionaryInspectionError {
                error_byte_offset: error.error_byte_offset,
                reason: ResolvedObjectDictionaryInspectionRejection::CompressedDictionaryEntries {
                    dictionary_entries_reason: error.reason,
                },
            }
        })?;

    Ok(CompressedObjectDictionaryInspection {
        reference,
        dictionary_open_byte_offset: entries.dictionary.open_byte_offset,
        dictionary_close_byte_offset: entries.dictionary.close_byte_offset,
        after_dictionary_close_byte_offset: entries.dictionary.after_close_byte_offset,
        max_observed_dictionary_depth: entries.dictionary.max_observed_depth,
        entries: entries.entries,
    })
}
