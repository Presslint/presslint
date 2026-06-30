#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use crate::startxref::inspect_startxref;
use crate::xref_section::classify_xref_section;
use crate::{
    ContentStreamDataExtentInspectionError, ContentStreamDataExtentInspectionRejection,
    ContentStreamDataSliceError, ContentStreamDataSliceRejection,
    ContentStreamFilterClassification, ContentStreamFilterClassificationError,
    ContentStreamFilterClassificationRejection, DecodeParmsParameter, DictionaryEntryByteRange,
    FlateDecodeParameters, FlateDecodeParametersResolution, FlateDecodeParametersResolutionError,
    FlateDecodeParametersResolutionRejection, FlateDecodeStreamError, FlateDecodeStreamRejection,
    IndirectRef, XrefSection, XrefStreamDictionaryInspectionError,
    XrefStreamDictionaryInspectionRejection, XrefStreamEntriesError, XrefStreamEntriesRejection,
    XrefStreamEntry, XrefStreamEntryRecord, XrefStreamSectionError, XrefStreamSectionRejection,
    XrefStreamSubsection, XrefStreamTrailerInspectionError, XrefStreamTrailerInspectionRejection,
    decode_xref_stream_section,
};

const MAX_DECODED: usize = 4096;

/// Wrap `inner` dictionary fields plus a computed `/Length` as a `5 0 obj`
/// xref-stream object at offset zero with the given raw stream body.
fn object_with_body(inner: &str, body: &[u8]) -> Vec<u8> {
    let dictionary = format!("<< {inner} /Length {} >>", body.len());
    let mut source = b"5 0 obj\n".to_vec();
    source.extend_from_slice(dictionary.as_bytes());
    source.extend_from_slice(b"\nstream\n");
    source.extend_from_slice(body);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source
}

/// Embed `object` in a minimal document whose `startxref` points at it.
fn document_with_object(object: &[u8]) -> (Vec<u8>, usize) {
    let prefix = b"%PDF-1.5\n";
    let object_offset = prefix.len();
    let mut source = prefix.to_vec();
    source.extend_from_slice(object);
    source.extend_from_slice(format!("startxref\n{object_offset}\n%%EOF\n").as_bytes());
    (source, object_offset)
}

/// Build a minimal valid zlib stream using a single stored (uncompressed)
/// deflate block, so tests can exercise the `/FlateDecode` decode path without a
/// deflate encoder dependency.
fn zlib_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01, 0x01];
    let len = u16::try_from(data.len()).expect("test body length fits u16");
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&(!len).to_le_bytes());
    out.extend_from_slice(data);
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn adler32(data: &[u8]) -> u32 {
    let mut a = 1u32;
    let mut b = 0u32;
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// Apply the PNG Up predictor (filter byte 2) to fixed-width rows, the encoding
/// `decode_flate_stream` reverses for `/Predictor 12`.
fn png_up_encode(rows: &[[u8; 4]]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut previous = [0u8; 4];
    for row in rows {
        out.push(2);
        for column in 0..row.len() {
            out.push(row[column].wrapping_sub(previous[column]));
        }
        previous = *row;
    }
    out
}

fn entry(object_number: usize, record: XrefStreamEntryRecord) -> XrefStreamEntry {
    XrefStreamEntry {
        object_number,
        record,
    }
}

fn root_ref() -> IndirectRef {
    IndirectRef {
        object_number: 1,
        generation: 0,
    }
}

/// The three canonical free/uncompressed/compressed entries shared by the
/// `FlateDecode` and raw fixtures.
fn expected_entries() -> Vec<XrefStreamEntry> {
    vec![
        entry(
            0,
            XrefStreamEntryRecord::Free {
                next_free_object_number: 0,
                generation: 255,
            },
        ),
        entry(
            1,
            XrefStreamEntryRecord::Uncompressed {
                byte_offset: 300,
                generation: 0,
            },
        ),
        entry(
            2,
            XrefStreamEntryRecord::Compressed {
                object_stream_number: 5,
                index_within_object_stream: 2,
            },
        ),
    ]
}

/// The post-predictor record rows for the canonical entries (`/W [ 1 2 1 ]`).
fn canonical_rows() -> [[u8; 4]; 3] {
    [[0, 0, 0, 255], [1, 0x01, 0x2c, 0], [2, 0x00, 0x05, 2]]
}

/// Concatenate the canonical record rows into a raw (no-predictor, no-filter)
/// xref-stream body.
fn raw_canonical_body() -> Vec<u8> {
    let mut body = Vec::new();
    for row in canonical_rows() {
        body.extend_from_slice(&row);
    }
    body
}

fn reason(object: &[u8]) -> XrefStreamSectionRejection {
    decode_xref_stream_section(object, 0, MAX_DECODED)
        .expect_err("section decode should reject")
        .reason
}

#[test]
fn composes_startxref_classification_into_flate_png_section() {
    let rows = canonical_rows();
    let body = zlib_store(&png_up_encode(&rows));
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R \
         /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 >>",
        &body,
    );
    let (source, object_offset) = document_with_object(&object);

    let startxref = inspect_startxref(&source).expect("startxref should inspect");
    assert_eq!(startxref.byte_offset, object_offset);
    let section = classify_xref_section(&source, startxref.byte_offset)
        .expect("xref section should classify");
    assert_eq!(
        section,
        XrefSection::Stream {
            object_number: 5,
            generation: 0,
        }
    );

    let report = decode_xref_stream_section(&source, object_offset, MAX_DECODED)
        .expect("flate+png xref-stream section should decode");

    assert_eq!(report.object_byte_offset, object_offset);
    assert_eq!(report.widths, [1, 2, 1]);
    assert_eq!(report.size, 3);
    assert_eq!(
        report.index_subsections,
        vec![XrefStreamSubsection {
            first_object_number: 0,
            entry_count: 3,
        }]
    );
    assert_eq!(report.root_reference, root_ref());
    assert_eq!(report.prev_byte_offset, None);
    assert_eq!(report.entries, expected_entries());
}

#[test]
fn composes_startxref_classification_into_raw_section() {
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R /Prev 17",
        &raw_canonical_body(),
    );
    let (source, object_offset) = document_with_object(&object);

    let section = classify_xref_section(
        &source,
        inspect_startxref(&source)
            .expect("startxref should inspect")
            .byte_offset,
    )
    .expect("xref section should classify");
    assert_eq!(
        section,
        XrefSection::Stream {
            object_number: 5,
            generation: 0,
        }
    );

    let report = decode_xref_stream_section(&source, object_offset, MAX_DECODED)
        .expect("raw xref-stream section should decode");

    assert_eq!(report.widths, [1, 2, 1]);
    assert_eq!(report.prev_byte_offset, Some(17));
    assert_eq!(report.root_reference, root_ref());
    assert_eq!(report.entries, expected_entries());
}

#[test]
fn resolves_overlapping_index_last_subsection_wins() {
    let body = [1, 10, 0, 1, 11, 0, 1, 99, 0, 1, 20, 0];
    let object = object_with_body(
        "/Type /XRef /Size 100 /W [ 1 1 1 ] /Index [ 0 2 1 2 ] /Root 1 0 R",
        &body,
    );

    let report = decode_xref_stream_section(&object, 0, MAX_DECODED)
        .expect("overlapping-index section should decode");

    // Object 1 appears in both subsections; the later subsection (offset 99)
    // overrides the earlier one (offset 11), and entries stay ascending.
    assert_eq!(
        report.entries,
        vec![
            entry(
                0,
                XrefStreamEntryRecord::Uncompressed {
                    byte_offset: 10,
                    generation: 0,
                },
            ),
            entry(
                1,
                XrefStreamEntryRecord::Uncompressed {
                    byte_offset: 99,
                    generation: 0,
                },
            ),
            entry(
                2,
                XrefStreamEntryRecord::Uncompressed {
                    byte_offset: 20,
                    generation: 0,
                },
            ),
        ]
    );
}

#[test]
fn rejects_unsupported_filter_without_partial_entries() {
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R /Filter /ASCIIHexDecode",
        &[0u8; 12],
    );

    assert!(matches!(
        reason(&object),
        XrefStreamSectionRejection::UnsupportedFilter {
            classification: ContentStreamFilterClassification::UnsupportedFilter { .. },
        }
    ));
}

#[test]
fn rejects_unsupported_array_decode_parms() {
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R \
         /Filter /FlateDecode /DecodeParms [ << /Predictor 12 >> ]",
        b"xxxx",
    );

    assert!(matches!(
        reason(&object),
        XrefStreamSectionRejection::UnsupportedDecodeParms {
            resolution: FlateDecodeParametersResolution::UnsupportedArrayParms { .. },
        }
    ));
}

#[test]
fn rejects_flate_decode_failure() {
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R /Filter /FlateDecode",
        b"not a zlib payload",
    );

    assert!(matches!(
        reason(&object),
        XrefStreamSectionRejection::FlateDecode {
            error: FlateDecodeStreamError {
                reason: FlateDecodeStreamRejection::InflateFailed,
                ..
            },
        }
    ));
}

#[test]
fn rejects_bad_geometry_as_dictionary_failure() {
    let object = object_with_body("/Type /XRef /Size 3 /W [ 1 2 ] /Root 1 0 R", &[0u8; 4]);

    assert!(matches!(
        reason(&object),
        XrefStreamSectionRejection::DictionaryGeometry {
            error: XrefStreamDictionaryInspectionError {
                reason: XrefStreamDictionaryInspectionRejection::WrongWLength { width_count: 2 },
                ..
            },
        }
    ));
}

#[test]
fn rejects_missing_root_as_trailer_failure() {
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ]",
        &[0u8; 12],
    );

    assert!(matches!(
        reason(&object),
        XrefStreamSectionRejection::TrailerNavigation {
            error: XrefStreamTrailerInspectionError {
                reason: XrefStreamTrailerInspectionRejection::MissingRoot,
                ..
            },
        }
    ));
}

#[test]
fn rejects_entry_parse_length_mismatch_without_partial_entries() {
    // `/W [ 1 2 1 ]` over `/Index [ 0 3 ]` needs 12 decoded bytes; supply 8.
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R",
        &[0u8; 8],
    );

    assert!(matches!(
        reason(&object),
        XrefStreamSectionRejection::EntryParse {
            error: XrefStreamEntriesError {
                reason: XrefStreamEntriesRejection::LengthMismatch {
                    expected: 12,
                    actual: 8,
                },
                ..
            },
        }
    ));
}

#[test]
fn rejects_stream_extent_failure_for_non_stream_object() {
    // No `stream` keyword, so the stream-data extent cannot be located.
    let object =
        b"5 0 obj\n<< /Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R /Length 4 >>\nendobj\n";

    assert!(matches!(
        reason(object),
        XrefStreamSectionRejection::StreamExtent { .. }
    ));
}

#[test]
fn report_does_not_retain_source_bytes() {
    let object = object_with_body(
        "/Secret (do-not-copy) /Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R",
        &raw_canonical_body(),
    );

    let report =
        decode_xref_stream_section(&object, 0, MAX_DECODED).expect("section should decode");
    let debug_report = format!("{report:?}");

    assert!(!debug_report.contains("do-not-copy"));
    assert!(!debug_report.contains("Secret"));
}

fn round_trip<T>(value: &T)
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let serialized = serde_value(value).expect("value should serialize");
    let decoded: T = from_serde_value(serialized).expect("value should deserialize");
    assert_eq!(&decoded, value);
}

fn section_error(reason: XrefStreamSectionRejection) -> XrefStreamSectionError {
    XrefStreamSectionError {
        object_byte_offset: 0,
        byte_len: 0,
        reason,
    }
}

fn empty_range() -> DictionaryEntryByteRange {
    DictionaryEntryByteRange { start: 0, end: 0 }
}

#[test]
fn serde_round_trip_pins_report_shape() {
    let object = object_with_body(
        "/Type /XRef /Size 3 /W [ 1 2 1 ] /Index [ 0 3 ] /Root 1 0 R /Prev 42",
        &raw_canonical_body(),
    );
    let report =
        decode_xref_stream_section(&object, 0, MAX_DECODED).expect("section should decode");
    round_trip(&report);
}

#[test]
fn serde_round_trip_pins_every_rejection_variant() {
    round_trip(&section_error(
        XrefStreamSectionRejection::DictionaryGeometry {
            error: XrefStreamDictionaryInspectionError {
                byte_offset: 0,
                byte_len: 0,
                object_header_byte_offset: None,
                error_byte_offset: None,
                reason: XrefStreamDictionaryInspectionRejection::MissingType,
            },
        },
    ));
    round_trip(&section_error(
        XrefStreamSectionRejection::TrailerNavigation {
            error: XrefStreamTrailerInspectionError {
                byte_offset: 0,
                byte_len: 0,
                object_header_byte_offset: None,
                error_byte_offset: None,
                reason: XrefStreamTrailerInspectionRejection::MissingRoot,
            },
        },
    ));
    round_trip(&section_error(XrefStreamSectionRejection::StreamExtent {
        error: ContentStreamDataExtentInspectionError {
            byte_offset: 0,
            byte_len: 0,
            error_byte_offset: None,
            reason: ContentStreamDataExtentInspectionRejection::MissingLength,
        },
    }));
    round_trip(&section_error(XrefStreamSectionRejection::Slice {
        error: ContentStreamDataSliceError {
            start_byte_offset: 0,
            end_byte_offset: 0,
            byte_len: 0,
            reason: ContentStreamDataSliceRejection::InvertedExtent,
        },
    }));
    round_trip(&section_error(
        XrefStreamSectionRejection::FilterClassification {
            error: ContentStreamFilterClassificationError {
                byte_offset: 0,
                byte_len: 0,
                error_byte_offset: None,
                reason: ContentStreamFilterClassificationRejection::NonNameFilterArrayElement,
            },
        },
    ));
    round_trip(&section_error(
        XrefStreamSectionRejection::UnsupportedFilter {
            classification: ContentStreamFilterClassification::UnsupportedFilter {
                filter_name_range: empty_range(),
            },
        },
    ));
    round_trip(&section_error(XrefStreamSectionRejection::DecodeParms {
        error: FlateDecodeParametersResolutionError {
            byte_offset: 0,
            byte_len: 0,
            error_byte_offset: None,
            reason: FlateDecodeParametersResolutionRejection::MalformedParameterInteger {
                parameter: DecodeParmsParameter::Predictor,
            },
        },
    }));
    round_trip(&section_error(
        XrefStreamSectionRejection::UnsupportedDecodeParms {
            resolution: FlateDecodeParametersResolution::UnsupportedArrayParms {
                decode_parms_value_range: empty_range(),
            },
        },
    ));
    round_trip(&section_error(XrefStreamSectionRejection::FlateDecode {
        error: FlateDecodeStreamError {
            compressed_len: 0,
            output_limit: 0,
            parameters: FlateDecodeParameters::default(),
            reason: FlateDecodeStreamRejection::InflateFailed,
        },
    }));
    round_trip(&section_error(XrefStreamSectionRejection::EntryParse {
        error: XrefStreamEntriesError {
            decoded_len: 0,
            error_byte_offset: None,
            object_number: None,
            reason: XrefStreamEntriesRejection::ZeroRecordWidth,
        },
    }));
}
