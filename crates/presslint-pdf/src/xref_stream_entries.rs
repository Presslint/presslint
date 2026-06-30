use serde::{Deserialize, Serialize};

use crate::XrefStreamSubsection;

const XREF_STREAM_FIELD_COUNT: usize = 3;
const MAX_FIELD_WIDTH: usize = 8;
const DEFAULT_ENTRY_TYPE: u64 = 1;

/// One typed cross-reference-stream entry decoded from already-decoded stream
/// body bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrefStreamEntry {
    /// Object number derived from the ordered `/Index` subsections.
    pub object_number: usize,
    /// Typed entry record decoded from the `/W`-width fields.
    pub record: XrefStreamEntryRecord,
}

/// Typed cross-reference-stream entry record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum XrefStreamEntryRecord {
    /// Type 0 free-object entry.
    Free {
        /// Object number of the next free object.
        next_free_object_number: usize,
        /// Generation number of the free object.
        generation: usize,
    },
    /// Type 1 uncompressed-object entry.
    Uncompressed {
        /// Byte offset of the object in the PDF source.
        byte_offset: usize,
        /// Generation number of the uncompressed object.
        generation: usize,
    },
    /// Type 2 compressed-object entry.
    Compressed {
        /// Object number of the object stream containing this object.
        object_stream_number: usize,
        /// Index of this object within the object stream.
        index_within_object_stream: usize,
    },
    /// Reserved or future entry type. The raw decoded field values are
    /// preserved rather than fabricated into a known entry shape.
    Reserved {
        /// Raw type field value.
        entry_type: u64,
        /// Raw second field value.
        field2: u64,
        /// Raw third field value.
        field3: u64,
    },
}

/// Error returned when cross-reference-stream entry records cannot be decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrefStreamEntriesError {
    /// Length of the caller-supplied decoded xref-stream body.
    pub decoded_len: usize,
    /// Byte offset within `decoded` where the malformed record was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Object number associated with an entry-level error, when available.
    pub object_number: Option<usize>,
    /// Structured failure reason.
    pub reason: XrefStreamEntriesRejection,
}

/// Structured cross-reference-stream entry decoding rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum XrefStreamEntriesRejection {
    /// The `/W` field widths sum to zero, so records have no byte extent.
    ZeroRecordWidth,
    /// A single `/W` field width cannot be decoded into the fixed integer
    /// accumulator used for entry fields.
    FieldWidthOutOfRange {
        /// Zero-based field index within `/W`.
        field_index: usize,
        /// Caller-supplied field width.
        width: usize,
    },
    /// An entry-count sum or byte-count multiplication overflowed `usize`.
    IntegerOverflow,
    /// The decoded byte length does not match the declared entry geometry.
    LengthMismatch {
        /// Expected decoded byte length.
        expected: usize,
        /// Actual decoded byte length.
        actual: usize,
    },
    /// A decoded known-entry field value does not fit `usize`.
    FieldValueOutOfRange {
        /// Zero-based field index within `/W`.
        field_index: usize,
        /// Decoded field value.
        value: u64,
    },
    /// A derived object number cannot be represented as `usize`.
    ObjectNumberOutOfRange {
        /// First object number declared by the subsection.
        first_object_number: usize,
        /// Zero-based entry index within the subsection.
        entry_index: usize,
    },
}

/// Decode already-decoded cross-reference-stream body bytes into typed entries.
///
/// The caller supplies the decoded stream bytes, the three `/W` field widths,
/// and the ordered `/Index` subsections already parsed from the stream
/// dictionary. This helper only slices fixed-width records, decodes
/// big-endian unsigned integer fields, applies the PDF default that an omitted
/// type field (`W[0] == 0`) is type 1, derives object numbers from `/Index`,
/// and returns small owned report records. It retains or copies no stream
/// bytes and performs no stream extent lookup, decompression, predictor
/// processing, trailer navigation, or object-stream extraction.
///
/// # Errors
///
/// Returns [`XrefStreamEntriesError`] for zero record width, over-wide fields,
/// entry-count or byte-count overflow, decoded length mismatch, decoded
/// known-entry fields that do not fit `usize`, or object-number overflow. It
/// never returns partial entries on error.
pub fn parse_xref_stream_entries(
    decoded: &[u8],
    widths: [usize; XREF_STREAM_FIELD_COUNT],
    subsections: &[XrefStreamSubsection],
) -> Result<Vec<XrefStreamEntry>, XrefStreamEntriesError> {
    let ctx = ErrorContext {
        decoded_len: decoded.len(),
    };
    validate_widths(widths, ctx)?;
    let record_width = record_width(widths, ctx)?;
    let total_entries = total_entries(subsections, ctx)?;
    let expected_len = total_entries
        .checked_mul(record_width)
        .ok_or_else(|| ctx.error(XrefStreamEntriesRejection::IntegerOverflow, None, None))?;

    if decoded.len() != expected_len {
        return Err(ctx.error(
            XrefStreamEntriesRejection::LengthMismatch {
                expected: expected_len,
                actual: decoded.len(),
            },
            None,
            None,
        ));
    }

    let mut entries = Vec::with_capacity(total_entries);
    let mut record_byte_offset = 0;
    for subsection in subsections {
        for entry_index in 0..subsection.entry_count {
            let object_number = subsection
                .first_object_number
                .checked_add(entry_index)
                .ok_or_else(|| {
                    ctx.error(
                        XrefStreamEntriesRejection::ObjectNumberOutOfRange {
                            first_object_number: subsection.first_object_number,
                            entry_index,
                        },
                        Some(record_byte_offset),
                        None,
                    )
                })?;

            let record = decode_record(decoded, record_byte_offset, widths, object_number, ctx)?;
            entries.push(XrefStreamEntry {
                object_number,
                record,
            });
            record_byte_offset += record_width;
        }
    }

    Ok(entries)
}

fn validate_widths(
    widths: [usize; XREF_STREAM_FIELD_COUNT],
    ctx: ErrorContext,
) -> Result<(), XrefStreamEntriesError> {
    for (field_index, width) in widths.into_iter().enumerate() {
        if width > MAX_FIELD_WIDTH {
            return Err(ctx.error(
                XrefStreamEntriesRejection::FieldWidthOutOfRange { field_index, width },
                None,
                None,
            ));
        }
    }
    Ok(())
}

fn record_width(
    widths: [usize; XREF_STREAM_FIELD_COUNT],
    ctx: ErrorContext,
) -> Result<usize, XrefStreamEntriesError> {
    let width = widths.into_iter().try_fold(0usize, |sum, width| {
        sum.checked_add(width)
            .ok_or_else(|| ctx.error(XrefStreamEntriesRejection::IntegerOverflow, None, None))
    })?;

    if width == 0 {
        return Err(ctx.error(XrefStreamEntriesRejection::ZeroRecordWidth, None, None));
    }

    Ok(width)
}

fn total_entries(
    subsections: &[XrefStreamSubsection],
    ctx: ErrorContext,
) -> Result<usize, XrefStreamEntriesError> {
    subsections.iter().try_fold(0usize, |sum, subsection| {
        sum.checked_add(subsection.entry_count)
            .ok_or_else(|| ctx.error(XrefStreamEntriesRejection::IntegerOverflow, None, None))
    })
}

fn decode_record(
    decoded: &[u8],
    record_byte_offset: usize,
    widths: [usize; XREF_STREAM_FIELD_COUNT],
    object_number: usize,
    ctx: ErrorContext,
) -> Result<XrefStreamEntryRecord, XrefStreamEntriesError> {
    let mut cursor = record_byte_offset;
    let entry_type = decode_field(decoded, &mut cursor, widths[0], DEFAULT_ENTRY_TYPE);
    let field2 = decode_field(decoded, &mut cursor, widths[1], 0);
    let field3 = decode_field(decoded, &mut cursor, widths[2], 0);

    match entry_type {
        0 => Ok(XrefStreamEntryRecord::Free {
            next_free_object_number: field_to_usize(
                field2,
                1,
                record_byte_offset,
                object_number,
                ctx,
            )?,
            generation: field_to_usize(field3, 2, record_byte_offset, object_number, ctx)?,
        }),
        1 => Ok(XrefStreamEntryRecord::Uncompressed {
            byte_offset: field_to_usize(field2, 1, record_byte_offset, object_number, ctx)?,
            generation: field_to_usize(field3, 2, record_byte_offset, object_number, ctx)?,
        }),
        2 => Ok(XrefStreamEntryRecord::Compressed {
            object_stream_number: field_to_usize(
                field2,
                1,
                record_byte_offset,
                object_number,
                ctx,
            )?,
            index_within_object_stream: field_to_usize(
                field3,
                2,
                record_byte_offset,
                object_number,
                ctx,
            )?,
        }),
        _ => Ok(XrefStreamEntryRecord::Reserved {
            entry_type,
            field2,
            field3,
        }),
    }
}

fn decode_field(decoded: &[u8], cursor: &mut usize, width: usize, default: u64) -> u64 {
    if width == 0 {
        return default;
    }

    let mut value = 0u64;
    for byte in &decoded[*cursor..*cursor + width] {
        value = (value << 8) | u64::from(*byte);
    }
    *cursor += width;

    value
}

fn field_to_usize(
    value: u64,
    field_index: usize,
    record_byte_offset: usize,
    object_number: usize,
    ctx: ErrorContext,
) -> Result<usize, XrefStreamEntriesError> {
    usize::try_from(value).map_err(|_| {
        ctx.error(
            XrefStreamEntriesRejection::FieldValueOutOfRange { field_index, value },
            Some(record_byte_offset),
            Some(object_number),
        )
    })
}

#[derive(Debug, Clone, Copy)]
struct ErrorContext {
    decoded_len: usize,
}

impl ErrorContext {
    const fn error(
        self,
        reason: XrefStreamEntriesRejection,
        error_byte_offset: Option<usize>,
        object_number: Option<usize>,
    ) -> XrefStreamEntriesError {
        XrefStreamEntriesError {
            decoded_len: self.decoded_len,
            error_byte_offset,
            object_number,
            reason,
        }
    }
}
