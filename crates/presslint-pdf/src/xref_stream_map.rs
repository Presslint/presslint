use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{
    ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionError,
    ContentStreamDataSliceError, ContentStreamFilterClassification,
    ContentStreamFilterClassificationError, FlateDecodeParametersResolution,
    FlateDecodeParametersResolutionError, FlateDecodeStreamError, IndirectRef,
    XrefStreamDictionaryInspection, XrefStreamDictionaryInspectionError,
    XrefStreamDictionaryInspectionRejection, XrefStreamEntriesError, XrefStreamEntry,
    XrefStreamEntryRecord, XrefStreamSubsection, XrefStreamTrailerInspectionError,
    XrefStreamTrailerInspectionRejection, classify_content_stream_filter,
    content_stream_data_slice, decode_flate_stream, inspect_content_stream_data_extent,
    inspect_xref_stream_trailer, parse_xref_stream_entries, resolve_flate_decode_parameters,
};

/// One decoded cross-reference stream section.
///
/// This is the first composing slice of the cross-reference-stream backend: it
/// turns exactly ONE `/Type /XRef` stream object into a deterministic,
/// object-number-ordered map of typed entry records by threading the existing
/// inspectors and decoders. It carries the xref-stream object byte offset, the
/// parsed `/W` widths, `/Size`, the ordered `/Index` subsections, the `/Root`
/// indirect reference, the optional `/Prev` byte offset, and the deduplicated
/// entries in ascending object-number order.
///
/// `/Prev` is surfaced for a later slice but never followed here; `/Root` is
/// surfaced but never resolved. The report retains or copies no PDF source
/// bytes; its only owned allocations are the bounded `index_subsections` and
/// `entries` vectors of small `Copy` records (the bounded decoded buffer, when a
/// `/FlateDecode` body is decoded, is dropped before this report is built).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrefStreamSection {
    /// Caller-supplied xref-stream object byte offset (the offset
    /// `classify_xref_section` reports as `XrefSection::Stream`).
    pub object_byte_offset: usize,
    /// The exactly-three `/W` field widths; a width of `0` marks an omitted
    /// cross-reference field.
    pub widths: [usize; 3],
    /// Parsed `/Size` value (one past the highest object number).
    pub size: usize,
    /// Ordered `(first_object_number, entry_count)` `/Index` subsection pairs.
    pub index_subsections: Vec<XrefStreamSubsection>,
    /// Parsed `/Root` document catalog indirect reference (surfaced, not
    /// resolved).
    pub root_reference: IndirectRef,
    /// Parsed `/Prev` previous cross-reference byte offset (surfaced, not
    /// followed), when present.
    pub prev_byte_offset: Option<usize>,
    /// Decoded entries in ascending object-number order, with duplicate object
    /// numbers across `/Index` subsections resolved last-subsection-wins.
    pub entries: Vec<XrefStreamEntry>,
}

/// Error returned when a single cross-reference stream section cannot be
/// decoded.
///
/// Every decode failure is a distinct structured rejection that carries the
/// delegated error or classification and never returns partial entries. This
/// report retains or copies no PDF bytes; it carries only the object byte
/// offset, the source length, and the structured reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrefStreamSectionError {
    /// Caller-supplied xref-stream object byte offset where decoding began.
    pub object_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Structured failure reason naming the composed stage that stopped.
    pub reason: XrefStreamSectionRejection,
}

/// Structured single-section cross-reference-stream decode rejection reasons.
///
/// Each variant names exactly one composed stage and preserves the delegated
/// failure verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum XrefStreamSectionRejection {
    /// The delegated `/Type /XRef`, `/W`, `/Size`, or `/Index` geometry
    /// inspection failed.
    DictionaryGeometry {
        /// Delegated cross-reference-stream dictionary geometry failure.
        error: XrefStreamDictionaryInspectionError,
    },
    /// The delegated `/Root`/`/Prev` trailer-navigation inspection failed.
    TrailerNavigation {
        /// Delegated cross-reference-stream trailer-navigation failure.
        error: XrefStreamTrailerInspectionError,
    },
    /// The stream-data byte extent could not be located.
    StreamExtent {
        /// Delegated stream-extent failure.
        error: ContentStreamDataExtentInspectionError,
    },
    /// The located extent could not be bridged to a borrowed source slice.
    Slice {
        /// Delegated slice failure.
        error: ContentStreamDataSliceError,
    },
    /// The stream `/Filter` declaration was malformed.
    FilterClassification {
        /// Delegated filter-classification failure.
        error: ContentStreamFilterClassificationError,
    },
    /// The stream uses a filter shape this slice does not decode (non-Flate,
    /// chained, or otherwise unsupported).
    UnsupportedFilter {
        /// Delegated filter classification.
        classification: ContentStreamFilterClassification,
    },
    /// The stream `/DecodeParms` declaration was malformed.
    DecodeParms {
        /// Delegated `/DecodeParms` failure.
        error: FlateDecodeParametersResolutionError,
    },
    /// The stream uses the unsupported array `/DecodeParms` form.
    UnsupportedDecodeParms {
        /// Delegated `/DecodeParms` resolution.
        resolution: FlateDecodeParametersResolution,
    },
    /// The bounded `/FlateDecode` operation failed.
    FlateDecode {
        /// Delegated Flate decode failure.
        error: FlateDecodeStreamError,
    },
    /// The decoded body could not be parsed into entry records.
    EntryParse {
        /// Delegated entry-parse failure.
        error: XrefStreamEntriesError,
    },
}

/// Decode one cross-reference stream object into a deterministic entry map.
///
/// The helper composes existing pieces and reimplements none of them:
///
/// - [`inspect_xref_stream_trailer`] supplies the `/W`/`/Size`/`/Index` geometry
///   (via its embedded [`inspect_xref_stream_dictionary`](crate::inspect_xref_stream_dictionary)
///   inspection) plus the `/Root` reference and optional `/Prev` byte offset;
/// - [`inspect_content_stream_data_extent`] (with no classic xref table, so an
///   indirect `/Length` is a structured stream-extent rejection) and
///   [`content_stream_data_slice`] locate and borrow the body bytes;
/// - [`classify_content_stream_filter`], [`resolve_flate_decode_parameters`],
///   and [`decode_flate_stream`] select the decode path, mirroring the classic
///   PDF inventory bridge: a raw body passes through borrowed and a single
///   `/FlateDecode` with resolved non-array `/DecodeParms` is decoded into the
///   bounded buffer `decode_flate_stream` returns under `max_decoded_stream_bytes`;
/// - [`parse_xref_stream_entries`] slices the fixed-width records.
///
/// Entries are returned in ascending object-number order. When `/Index`
/// subsections overlap, a duplicate object number resolves deterministically by
/// the documented rule **last subsection wins**, matching how a later xref
/// section overrides an earlier one (PDF 32000 §7.5.8).
///
/// This slice decodes exactly ONE section: it surfaces `/Prev` and `/Root` but
/// never follows `/Prev`, merges incremental sections, resolves `/Root`, or
/// extracts `Compressed` entries to byte offsets. The returned report retains or
/// copies no PDF source bytes; the only owned allocations are the bounded
/// decoded buffer (when a `/FlateDecode` body is decoded, and dropped before the
/// report is built) and the bounded entry/geometry vectors of small `Copy`
/// records.
///
/// # Errors
///
/// Returns [`XrefStreamSectionError`] for a dictionary-geometry failure,
/// trailer-navigation failure, stream-extent-locate failure, slice failure,
/// filter-classification failure, unsupported filter, `/DecodeParms` failure,
/// unsupported array `/DecodeParms`, Flate decode failure, or entry-parse
/// failure. Each variant carries the delegated error or classification and never
/// returns partial entries.
pub fn decode_xref_stream_section(
    input: &[u8],
    object_byte_offset: usize,
    max_decoded_stream_bytes: usize,
) -> Result<XrefStreamSection, XrefStreamSectionError> {
    let trailer = inspect_xref_stream_trailer(input, object_byte_offset)
        .map_err(|error| trailer_stage_error(input, object_byte_offset, error))?;

    let geometry = &trailer.xref_stream_dictionary;
    let widths = match geometry.widths.as_slice() {
        &[first, second, third] => [first, second, third],
        // The trailer succeeded, so geometry inspection already guaranteed
        // exactly three `/W` widths; this arm is unreachable but keeps the
        // conversion panic-free.
        _ => return Err(geometry_width_error(input, object_byte_offset, geometry)),
    };

    let extent =
        inspect_content_stream_data_extent(input, None, object_byte_offset).map_err(|error| {
            section_error(
                input,
                object_byte_offset,
                XrefStreamSectionRejection::StreamExtent { error },
            )
        })?;

    let body = decode_body(input, object_byte_offset, &extent, max_decoded_stream_bytes)?;

    let parsed = parse_xref_stream_entries(body.as_slice(), widths, &geometry.index_subsections)
        .map_err(|error| {
            section_error(
                input,
                object_byte_offset,
                XrefStreamSectionRejection::EntryParse { error },
            )
        })?;

    Ok(XrefStreamSection {
        object_byte_offset,
        widths,
        size: geometry.size,
        index_subsections: geometry.index_subsections.clone(),
        root_reference: trailer.root_reference,
        prev_byte_offset: trailer.prev_byte_offset,
        entries: resolve_duplicate_object_numbers(parsed),
    })
}

/// Borrowed (raw) or owned (decoded) section body bytes.
enum SectionBody<'input> {
    Borrowed(&'input [u8]),
    Owned(Vec<u8>),
}

impl SectionBody<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// Locate, borrow, and (when `/FlateDecode`) decode the section body bytes,
/// mirroring the classic PDF inventory bridge's `decode_content` decode branch.
fn decode_body<'input>(
    input: &'input [u8],
    object_byte_offset: usize,
    extent: &ContentStreamDataExtentInspection,
    max_decoded_stream_bytes: usize,
) -> Result<SectionBody<'input>, XrefStreamSectionError> {
    let stream_data = content_stream_data_slice(input, extent).map_err(|error| {
        section_error(
            input,
            object_byte_offset,
            XrefStreamSectionRejection::Slice { error },
        )
    })?;

    match classify_content_stream_filter(input, object_byte_offset).map_err(|error| {
        section_error(
            input,
            object_byte_offset,
            XrefStreamSectionRejection::FilterClassification { error },
        )
    })? {
        ContentStreamFilterClassification::Uncompressed => Ok(SectionBody::Borrowed(stream_data)),
        ContentStreamFilterClassification::Flate => {
            let resolution =
                resolve_flate_decode_parameters(input, object_byte_offset).map_err(|error| {
                    section_error(
                        input,
                        object_byte_offset,
                        XrefStreamSectionRejection::DecodeParms { error },
                    )
                })?;
            let FlateDecodeParametersResolution::Resolved { parameters, .. } = resolution else {
                return Err(section_error(
                    input,
                    object_byte_offset,
                    XrefStreamSectionRejection::UnsupportedDecodeParms { resolution },
                ));
            };
            let decoded = decode_flate_stream(stream_data, parameters, max_decoded_stream_bytes)
                .map_err(|error| {
                    section_error(
                        input,
                        object_byte_offset,
                        XrefStreamSectionRejection::FlateDecode { error },
                    )
                })?;
            Ok(SectionBody::Owned(decoded))
        }
        classification @ (ContentStreamFilterClassification::UnsupportedFilter { .. }
        | ContentStreamFilterClassification::UnsupportedFilterChain { .. }) => Err(section_error(
            input,
            object_byte_offset,
            XrefStreamSectionRejection::UnsupportedFilter { classification },
        )),
    }
}

/// Resolve duplicate object numbers across `/Index` subsections by
/// last-subsection-wins and return the entries in ascending object-number order.
///
/// `parse_xref_stream_entries` already emits entries in `/Index` traversal
/// order, so inserting each into a `BTreeMap` keyed by object number lets a
/// later subsection's record overwrite an earlier one (last subsection wins)
/// while the map's ordered iteration yields ascending object-number order.
fn resolve_duplicate_object_numbers(parsed: Vec<XrefStreamEntry>) -> Vec<XrefStreamEntry> {
    let mut by_object_number: BTreeMap<usize, XrefStreamEntryRecord> = BTreeMap::new();
    for entry in parsed {
        by_object_number.insert(entry.object_number, entry.record);
    }
    by_object_number
        .into_iter()
        .map(|(object_number, record)| XrefStreamEntry {
            object_number,
            record,
        })
        .collect()
}

/// Split a delegated trailer failure into the distinct dictionary-geometry and
/// trailer-navigation rejections.
///
/// `inspect_xref_stream_trailer` builds its error 1:1 from the delegated
/// dictionary error, so a nested
/// [`XrefStreamTrailerInspectionRejection::XrefStreamDictionary`] reason is
/// recovered losslessly into the dictionary-geometry rejection; every other
/// reason is a trailer-navigation rejection.
const fn trailer_stage_error(
    input: &[u8],
    object_byte_offset: usize,
    error: XrefStreamTrailerInspectionError,
) -> XrefStreamSectionError {
    let reason = match error.reason {
        XrefStreamTrailerInspectionRejection::XrefStreamDictionary { xref_stream_reason } => {
            XrefStreamSectionRejection::DictionaryGeometry {
                error: XrefStreamDictionaryInspectionError {
                    byte_offset: error.byte_offset,
                    byte_len: error.byte_len,
                    object_header_byte_offset: error.object_header_byte_offset,
                    error_byte_offset: error.error_byte_offset,
                    reason: xref_stream_reason,
                },
            }
        }
        _ => XrefStreamSectionRejection::TrailerNavigation { error },
    };
    section_error(input, object_byte_offset, reason)
}

/// Build the unreachable wrong-`/W`-length geometry rejection without panicking.
const fn geometry_width_error(
    input: &[u8],
    object_byte_offset: usize,
    geometry: &XrefStreamDictionaryInspection,
) -> XrefStreamSectionError {
    section_error(
        input,
        object_byte_offset,
        XrefStreamSectionRejection::DictionaryGeometry {
            error: XrefStreamDictionaryInspectionError {
                byte_offset: object_byte_offset,
                byte_len: input.len(),
                object_header_byte_offset: Some(geometry.object_dictionary.header_range.start),
                error_byte_offset: None,
                reason: XrefStreamDictionaryInspectionRejection::WrongWLength {
                    width_count: geometry.widths.len(),
                },
            },
        },
    )
}

const fn section_error(
    input: &[u8],
    object_byte_offset: usize,
    reason: XrefStreamSectionRejection,
) -> XrefStreamSectionError {
    XrefStreamSectionError {
        object_byte_offset,
        byte_len: input.len(),
        reason,
    }
}
