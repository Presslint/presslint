use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::source_utils::{
    count_leading_digits, is_pdf_whitespace, parse_u64_decimal, skip_comment, skip_hex_string,
    skip_literal_string, skip_name, skip_scalar_token, skip_whitespace,
};
use crate::{
    ArrayExtentInspectionRejection, DictionaryExtentInspectionRejection,
    IndirectObjectBodyLeadingTokenKind, IndirectObjectBodyTokenInspectionRejection,
    IndirectObjectHeaderInspectionRejection, IndirectRef, IndirectReferenceInspectionRejection,
    ResolvedObjectData, inspect_array_extent, inspect_dictionary_extent,
    inspect_indirect_object_body_token, inspect_indirect_object_header, parse_indirect_reference,
};

/// Maximum number of indirect references reported for one object body.
///
/// When a body yields more references than this bound, the scan stops and the
/// report records a structured [`ObjectBodyReferencesTruncation`] marker rather
/// than dropping references silently or growing without bound.
pub const MAX_OBJECT_BODY_REFERENCES: usize = 65_536;

/// Indirect references extracted from one object body, in source order.
///
/// This report stores only parsed [`IndirectRef`] values, structured skip
/// markers, and an optional truncation marker. It carries no byte ranges and
/// does not retain or copy PDF bytes, object bodies, stream bodies, or decoded
/// object-stream bytes. Duplicate references are preserved: the same reference
/// appearing twice in one body is a legitimate structural fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectBodyReferencesInspection {
    /// Extracted `N G R` indirect references in source order, without dedup.
    pub references: Vec<IndirectRef>,
    /// Reference-shaped constructs skipped for out-of-range numbers, in source
    /// order.
    pub skipped_references: Vec<SkippedObjectBodyReference>,
    /// Set when the per-body reference cap stopped the scan early.
    pub truncation: Option<ObjectBodyReferencesTruncation>,
}

/// Structured reason a reference-shaped construct was skipped.
///
/// These mirror the overflow checks of [`crate::parse_indirect_reference`]: a
/// construct shaped as `N G R` whose numbers do not fit the public
/// `u32`/`u16` reference fields is reported as a skip, never silently dropped
/// and never truncated into a wrong reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedObjectBodyReference {
    /// The object number does not fit `u32`.
    ObjectNumberOutOfRange,
    /// The generation number does not fit `u16`.
    GenerationOutOfRange,
}

/// Bound that stopped a reference scan before the body extent was exhausted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "limit", rename_all = "snake_case")]
pub enum ObjectBodyReferencesTruncation {
    /// The per-body reference cap was reached and at least one further
    /// reference remained unreported.
    MaxReferences {
        /// Configured per-body reference cap.
        max_references: usize,
    },
}

/// Error returned when an object body cannot be scanned for references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectBodyReferencesInspectionError {
    /// Caller-supplied byte offset where object inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed or unsupported construct was found,
    /// when available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: ObjectBodyReferencesInspectionRejection,
}

/// Structured object-body reference inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ObjectBodyReferencesInspectionRejection {
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
    /// A delegated dictionary extent inspection of a dictionary-led body
    /// failed.
    DictionaryExtent {
        /// Underlying dictionary extent rejection reason.
        dictionary_extent_reason: DictionaryExtentInspectionRejection,
    },
    /// A delegated array extent inspection of an array-led body failed.
    ArrayExtent {
        /// Underlying array extent rejection reason.
        array_extent_reason: ArrayExtentInspectionRejection,
    },
}

/// Scan an indirect object's body for `N G R` indirect references.
///
/// The helper validates the `N G obj` header with
/// [`crate::inspect_indirect_object_header`], classifies the body's leading
/// token with [`crate::inspect_indirect_object_body_token`], and scans the
/// extent that matches the body shape:
///
/// - a dictionary-led body is bounded to its balanced `<< ... >>` extent via
///   [`crate::inspect_dictionary_extent`], so stream data after the dictionary
///   close is never scanned;
/// - an array-led body is bounded to its balanced `[ ... ]` extent via
///   [`crate::inspect_array_extent`];
/// - a number-like scalar body gets a bounded three-token reference-shape
///   check via [`crate::parse_indirect_reference`], so an object whose whole
///   body is `N G R` reports exactly that reference;
/// - any other scalar body (name, string, boolean, null) yields an empty
///   report.
///
/// The scan is linear, not recursive: inside object-body syntax a bare
/// boundary-checked `R` keyword can only be the tail of an `N G R` reference,
/// so one bounded pass finds every reference at any nesting depth. Literal
/// strings, hex strings, and comments are skipped as opaque spans and never
/// produce references. The report is in source order by construction and
/// retains no PDF bytes.
///
/// # Errors
///
/// Returns [`ObjectBodyReferencesInspectionError`] for a delegated header
/// inspection failure, a delegated body-token classification failure, or a
/// delegated dictionary/array extent failure.
pub fn inspect_object_body_references(
    input: &[u8],
    object_byte_offset: usize,
) -> Result<ObjectBodyReferencesInspection, ObjectBodyReferencesInspectionError> {
    let header = inspect_indirect_object_header(input, object_byte_offset).map_err(|error| {
        references_error(
            input,
            object_byte_offset,
            ObjectBodyReferencesInspectionRejection::Header {
                header_reason: error.reason,
            },
            error.error_byte_offset,
        )
    })?;

    let body_token = inspect_indirect_object_body_token(input, header.after_obj_keyword_offset)
        .map_err(|error| {
            references_error(
                input,
                object_byte_offset,
                ObjectBodyReferencesInspectionRejection::BodyToken {
                    body_token_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;

    match body_token.token_kind {
        IndirectObjectBodyLeadingTokenKind::DictionaryOpen => {
            let extent = inspect_dictionary_extent(input, body_token.first_token_byte_offset)
                .map_err(|error| {
                    references_error(
                        input,
                        object_byte_offset,
                        ObjectBodyReferencesInspectionRejection::DictionaryExtent {
                            dictionary_extent_reason: error.reason,
                        },
                        error.error_byte_offset,
                    )
                })?;
            Ok(scan_indirect_references_in_span(
                input,
                extent.open_byte_offset..extent.after_close_byte_offset,
            ))
        }
        IndirectObjectBodyLeadingTokenKind::ArrayOpen => {
            let extent = inspect_array_extent(input, body_token.first_token_byte_offset).map_err(
                |error| {
                    references_error(
                        input,
                        object_byte_offset,
                        ObjectBodyReferencesInspectionRejection::ArrayExtent {
                            array_extent_reason: error.reason,
                        },
                        error.error_byte_offset,
                    )
                },
            )?;
            Ok(scan_indirect_references_in_span(
                input,
                extent.open_byte_offset..extent.after_close_byte_offset,
            ))
        }
        IndirectObjectBodyLeadingTokenKind::NumberLike => Ok(scalar_body_reference_check(
            input,
            body_token.first_token_byte_offset,
        )),
        IndirectObjectBodyLeadingTokenKind::HexStringOpen
        | IndirectObjectBodyLeadingTokenKind::Name
        | IndirectObjectBodyLeadingTokenKind::LiteralString
        | IndirectObjectBodyLeadingTokenKind::Boolean
        | IndirectObjectBodyLeadingTokenKind::Null => Ok(empty_inspection()),
    }
}

/// Scan body-aware resolved object data for `N G R` indirect references.
///
/// [`ResolvedObjectData::Uncompressed`] delegates to
/// [`inspect_object_body_references`] at the resolved source byte offset.
/// [`ResolvedObjectData::Compressed`] scans the member's `object_body_span`
/// inside the decoded object-stream buffer with
/// [`scan_indirect_references_in_span`]; compressed members are never streams,
/// so the whole span is object syntax and no stream exclusion is needed.
///
/// The report intentionally carries no byte ranges: compressed-member spans
/// address a decoded buffer the caller may drop, not the source `input`. It
/// retains no PDF bytes and no decoded stream bytes.
///
/// # Errors
///
/// Returns [`ObjectBodyReferencesInspectionError`] when the delegated
/// uncompressed inspection fails. The compressed path scans an
/// already-extracted span and does not fail.
pub fn inspect_object_body_references_resolved(
    input: &[u8],
    resolved: &ResolvedObjectData,
) -> Result<ObjectBodyReferencesInspection, ObjectBodyReferencesInspectionError> {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => {
            inspect_object_body_references(input, resolved.object_byte_offset)
        }
        ResolvedObjectData::Compressed {
            decoded_object_stream,
            object_body_span,
            ..
        } => Ok(scan_indirect_references_in_span(
            decoded_object_stream,
            object_body_span.clone(),
        )),
    }
}

/// Scan a byte span for `N G R` indirect references with one linear pass.
///
/// The scanner keeps a sliding window of the last two unsigned digit-only
/// integer tokens. A bare `R` keyword token (delimiter/whitespace-bounded, so
/// `Robot` and `R0` never match) with a full window emits one reference in
/// source order and resets the window; every other token — names, delimiters,
/// signed numbers, reals, booleans — resets it. Literal strings, hex strings,
/// and comments are skipped as opaque spans and are never scanned for
/// references; a comment otherwise counts as whitespace between tokens, per
/// the PDF token rules, so a comment between `N G` and `R` does not hide a
/// real reference. A string that opens inside the span but does not terminate
/// within it ends the scan: every remaining span byte is string interior.
///
/// The pass is span-bounded end to end: skippers run over a slice truncated
/// at the span end, so no byte at or past `span.end` — sibling object-stream
/// members, stream data, unrelated suffix bytes — is ever read.
///
/// The span is clamped to `buffer` bounds. Reference numbers that do not fit
/// `u32`/`u16` become structured [`SkippedObjectBodyReference`] markers, and
/// the per-body [`MAX_OBJECT_BODY_REFERENCES`] cap stops the scan with a
/// structured truncation marker. The report retains no scanned bytes.
#[must_use]
pub fn scan_indirect_references_in_span(
    buffer: &[u8],
    span: Range<usize>,
) -> ObjectBodyReferencesInspection {
    let end = span.end.min(buffer.len());
    // Truncating the buffer at the span end keeps every skipper span-bounded:
    // an unbounded skipper over `buffer` could otherwise walk past `span.end`
    // into sibling object-stream members or stream bytes before clamping.
    let bounded = &buffer[..end];
    let mut cursor = span.start.min(end);
    let mut references = Vec::new();
    let mut skipped_references = Vec::new();
    let mut truncation = None;
    // Sliding window of the last two unsigned digit-only integer tokens; the
    // inner `Option` is `None` when the digit run overflows `u64`.
    let mut previous_integer: Option<Option<u64>> = None;
    let mut latest_integer: Option<Option<u64>> = None;

    'scan: while cursor < end {
        match bounded[cursor] {
            byte if is_pdf_whitespace(byte) => {
                cursor += skip_whitespace(&bounded[cursor..]);
            }
            b'%' => cursor = skip_comment(bounded, cursor),
            b'(' => {
                let Some(after_string) = skip_literal_string(bounded, cursor) else {
                    break 'scan;
                };
                cursor = after_string;
                previous_integer = None;
                latest_integer = None;
            }
            b'<' if bounded.get(cursor + 1) == Some(&b'<') => {
                cursor += 2;
                previous_integer = None;
                latest_integer = None;
            }
            b'<' => {
                let Some(after_string) = skip_hex_string(bounded, cursor) else {
                    break 'scan;
                };
                cursor = after_string;
                previous_integer = None;
                latest_integer = None;
            }
            b'>' if bounded.get(cursor + 1) == Some(&b'>') => {
                cursor += 2;
                previous_integer = None;
                latest_integer = None;
            }
            b'/' => {
                cursor = skip_name(bounded, cursor, end);
                previous_integer = None;
                latest_integer = None;
            }
            b')' | b'>' | b'[' | b']' | b'{' | b'}' => {
                cursor += 1;
                previous_integer = None;
                latest_integer = None;
            }
            b'0'..=b'9' => {
                let token_end = skip_scalar_token(bounded, cursor, end);
                let digits = count_leading_digits(&bounded[cursor..token_end]);
                if cursor + digits == token_end {
                    previous_integer = latest_integer.take();
                    latest_integer = Some(parse_u64_decimal(&bounded[cursor..token_end]));
                } else {
                    previous_integer = None;
                    latest_integer = None;
                }
                cursor = token_end;
            }
            _ => {
                let token_end = skip_scalar_token(bounded, cursor, end);
                if &bounded[cursor..token_end] == b"R" {
                    if let (Some(object), Some(generation)) = (previous_integer, latest_integer) {
                        match reference_from_window(object, generation) {
                            Ok(reference) => {
                                if references.len() == MAX_OBJECT_BODY_REFERENCES {
                                    truncation =
                                        Some(ObjectBodyReferencesTruncation::MaxReferences {
                                            max_references: MAX_OBJECT_BODY_REFERENCES,
                                        });
                                    break 'scan;
                                }
                                references.push(reference);
                            }
                            Err(skipped) => skipped_references.push(skipped),
                        }
                    }
                }
                previous_integer = None;
                latest_integer = None;
                cursor = token_end;
            }
        }
    }

    ObjectBodyReferencesInspection {
        references,
        skipped_references,
        truncation,
    }
}

/// Reference-shape check for a number-like scalar body.
///
/// The leading-token classifier reports a `2 0 R` body as `NumberLike`, so a
/// scalar-led body needs this explicit three-token check or reference-bodied
/// objects would be silently dropped. The check reuses
/// [`crate::parse_indirect_reference`], inheriting its bounded scan window and
/// keyword-boundary rule; out-of-range numbers become structured skips and any
/// other failure yields an empty report (the body is a plain number).
fn scalar_body_reference_check(
    input: &[u8],
    first_token_byte_offset: usize,
) -> ObjectBodyReferencesInspection {
    match parse_indirect_reference(input, first_token_byte_offset) {
        Ok(inspection) => ObjectBodyReferencesInspection {
            references: vec![inspection.reference],
            skipped_references: Vec::new(),
            truncation: None,
        },
        Err(error) => {
            let skipped_references = match error.reason {
                IndirectReferenceInspectionRejection::ObjectNumberOutOfRange => {
                    vec![SkippedObjectBodyReference::ObjectNumberOutOfRange]
                }
                IndirectReferenceInspectionRejection::GenerationOutOfRange => {
                    vec![SkippedObjectBodyReference::GenerationOutOfRange]
                }
                IndirectReferenceInspectionRejection::OffsetOutOfBounds
                | IndirectReferenceInspectionRejection::MalformedReference => Vec::new(),
            };
            ObjectBodyReferencesInspection {
                references: Vec::new(),
                skipped_references,
                truncation: None,
            }
        }
    }
}

fn reference_from_window(
    object: Option<u64>,
    generation: Option<u64>,
) -> Result<IndirectRef, SkippedObjectBodyReference> {
    let object_number = object
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(SkippedObjectBodyReference::ObjectNumberOutOfRange)?;
    let generation = generation
        .and_then(|value| u16::try_from(value).ok())
        .ok_or(SkippedObjectBodyReference::GenerationOutOfRange)?;
    Ok(IndirectRef {
        object_number,
        generation,
    })
}

const fn empty_inspection() -> ObjectBodyReferencesInspection {
    ObjectBodyReferencesInspection {
        references: Vec::new(),
        skipped_references: Vec::new(),
        truncation: None,
    }
}

const fn references_error(
    input: &[u8],
    byte_offset: usize,
    reason: ObjectBodyReferencesInspectionRejection,
    error_byte_offset: Option<usize>,
) -> ObjectBodyReferencesInspectionError {
    ObjectBodyReferencesInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset,
        reason,
    }
}
