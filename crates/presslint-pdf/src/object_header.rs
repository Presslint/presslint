use serde::{Deserialize, Serialize};

use crate::IndirectRef;

const INDIRECT_OBJECT_HEADER_SCAN_LIMIT: usize = 128;

/// Source byte range covering an indirect object header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectObjectHeaderByteRange {
    /// Inclusive start offset.
    pub start: usize,
    /// Exclusive end offset.
    pub end: usize,
}

/// Parsed metadata for an indirect object header at a caller-supplied offset.
///
/// This report stores only structural metadata. It does not retain or copy PDF
/// bytes and does not inspect the indirect object body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectObjectHeaderInspection {
    /// Parsed indirect object identifier.
    pub reference: IndirectRef,
    /// Byte offset where the header begins after optional PDF whitespace.
    pub header_byte_offset: usize,
    /// Byte range covering `object_number generation obj`.
    pub header_range: IndirectObjectHeaderByteRange,
    /// Byte offset immediately after the `obj` keyword.
    pub after_obj_keyword_offset: usize,
}

/// Error returned when an indirect object header cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndirectObjectHeaderInspectionError {
    /// Caller-supplied byte offset where inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed construct was found, when available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: IndirectObjectHeaderInspectionRejection,
}

/// Structured indirect object header inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum IndirectObjectHeaderInspectionRejection {
    /// The caller-supplied offset lies beyond the source length or exactly at
    /// EOF.
    OffsetOutOfBounds,
    /// The bytes at the resolved offset are not shaped as `N G obj`.
    MalformedHeader,
    /// The parsed object number does not fit `u32`.
    ObjectNumberOutOfRange,
    /// The parsed generation number does not fit `u16`.
    GenerationOutOfRange,
}

/// Inspect an indirect object header at a caller-supplied byte offset.
///
/// The helper skips optional PDF whitespace at `byte_offset`, parses only the
/// `object-number generation obj` header, and stops immediately after the
/// `obj` keyword. It performs no filesystem I/O and does not inspect object
/// bodies, dictionaries, streams, `endobj`, trailers, page trees, catalogs, or
/// content streams.
///
/// # Errors
///
/// Returns [`IndirectObjectHeaderInspectionError`] when the offset is outside
/// the source bytes, the header is malformed, or the parsed object or
/// generation number exceeds the public `u32`/`u16` report fields.
pub fn inspect_indirect_object_header(
    input: &[u8],
    byte_offset: usize,
) -> Result<IndirectObjectHeaderInspection, IndirectObjectHeaderInspectionError> {
    if byte_offset >= input.len() {
        return Err(object_header_error(
            input,
            byte_offset,
            IndirectObjectHeaderInspectionRejection::OffsetOutOfBounds,
        ));
    }

    let window_end = byte_offset
        .saturating_add(INDIRECT_OBJECT_HEADER_SCAN_LIMIT)
        .min(input.len());
    let window = &input[byte_offset..window_end];
    let leading_whitespace = skip_whitespace(window);
    let header_byte_offset = byte_offset + leading_whitespace;
    let content = &window[leading_whitespace..];

    let object_digits = count_leading_digits(content);
    if object_digits == 0 {
        return Err(malformed_object_header_error(
            input,
            byte_offset,
            header_byte_offset,
        ));
    }
    let after_object = &content[object_digits..];

    let object_generation_gap = skip_whitespace(after_object);
    if object_generation_gap == 0 {
        return Err(malformed_object_header_error(
            input,
            byte_offset,
            header_byte_offset,
        ));
    }
    let generation_field = &after_object[object_generation_gap..];

    let generation_digits = count_leading_digits(generation_field);
    if generation_digits == 0 {
        return Err(malformed_object_header_error(
            input,
            byte_offset,
            header_byte_offset + object_digits + object_generation_gap,
        ));
    }
    let after_generation = &generation_field[generation_digits..];

    let generation_keyword_gap = skip_whitespace(after_generation);
    if generation_keyword_gap == 0 {
        return Err(malformed_object_header_error(
            input,
            byte_offset,
            header_byte_offset + object_digits + object_generation_gap + generation_digits,
        ));
    }
    let keyword_offset = header_byte_offset
        + object_digits
        + object_generation_gap
        + generation_digits
        + generation_keyword_gap;
    let keyword_content = &after_generation[generation_keyword_gap..];
    if !consume_obj_keyword(keyword_content) {
        return Err(malformed_object_header_error(
            input,
            byte_offset,
            keyword_offset,
        ));
    }

    let object_number = parse_u64_decimal(&content[..object_digits])
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            object_header_error_at(
                input,
                byte_offset,
                IndirectObjectHeaderInspectionRejection::ObjectNumberOutOfRange,
                header_byte_offset,
            )
        })?;
    let generation = parse_u64_decimal(&generation_field[..generation_digits])
        .and_then(|value| u16::try_from(value).ok())
        .ok_or_else(|| {
            object_header_error_at(
                input,
                byte_offset,
                IndirectObjectHeaderInspectionRejection::GenerationOutOfRange,
                header_byte_offset + object_digits + object_generation_gap,
            )
        })?;
    let after_obj_keyword_offset = keyword_offset + b"obj".len();

    Ok(IndirectObjectHeaderInspection {
        reference: IndirectRef {
            object_number,
            generation,
        },
        header_byte_offset,
        header_range: IndirectObjectHeaderByteRange {
            start: header_byte_offset,
            end: after_obj_keyword_offset,
        },
        after_obj_keyword_offset,
    })
}

const fn object_header_error(
    input: &[u8],
    byte_offset: usize,
    reason: IndirectObjectHeaderInspectionRejection,
) -> IndirectObjectHeaderInspectionError {
    IndirectObjectHeaderInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset: None,
        reason,
    }
}

const fn object_header_error_at(
    input: &[u8],
    byte_offset: usize,
    reason: IndirectObjectHeaderInspectionRejection,
    error_byte_offset: usize,
) -> IndirectObjectHeaderInspectionError {
    IndirectObjectHeaderInspectionError {
        byte_offset,
        byte_len: input.len(),
        error_byte_offset: Some(error_byte_offset),
        reason,
    }
}

const fn malformed_object_header_error(
    input: &[u8],
    byte_offset: usize,
    error_byte_offset: usize,
) -> IndirectObjectHeaderInspectionError {
    object_header_error_at(
        input,
        byte_offset,
        IndirectObjectHeaderInspectionRejection::MalformedHeader,
        error_byte_offset,
    )
}

fn count_leading_digits(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count()
}

fn parse_u64_decimal(bytes: &[u8]) -> Option<u64> {
    let mut value = 0u64;
    for byte in bytes {
        let digit = u64::from(byte - b'0');
        value = value.checked_mul(10)?.checked_add(digit)?;
    }
    Some(value)
}

fn consume_obj_keyword(bytes: &[u8]) -> bool {
    let Some(after_keyword) = bytes.strip_prefix(b"obj") else {
        return false;
    };
    if after_keyword
        .first()
        .is_some_and(|byte| !is_pdf_whitespace(*byte) && !is_pdf_delimiter(*byte))
    {
        return false;
    }
    true
}

fn skip_whitespace(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .position(|byte| !is_pdf_whitespace(*byte))
        .unwrap_or(bytes.len())
}

const fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

const fn is_pdf_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}
