//! Focused tests for the content-stream `/DecodeParms` resolver.
//!
//! These exercise the resolver directly over single stream objects (no
//! `/DecodeParms`, a `null` value, a full predictor dictionary, a partial
//! predictor dictionary, an array value, and every malformed-structure
//! rejection) and then prove the resolver composes with the classic-xref
//! document-access spine: a synthetic single-page `/FlateDecode` content stream
//! carrying a predictor dictionary is navigated end to end, classified as
//! `Flate`, and resolved. A serde round-trip pins the public JSON shape of the
//! resolution, rejection, and `DecodeParmsParameter` enums.

#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ContentStreamFilterClassification, ContentStreamStartInspectionRejection, DecodeParmsParameter,
    DictionaryValueKind, FlateDecodeParameters, FlateDecodeParametersResolution,
    FlateDecodeParametersResolutionError, FlateDecodeParametersResolutionRejection,
    IndirectObjectBodyLeadingTokenKind, PageContentTargetInspection,
    classify_content_stream_filter, inspect_classic_document_access, inspect_page_content_targets,
    inspect_page_contents, resolve_flate_decode_parameters,
};

const PDF_PREFIX: &[u8] = b"%PDF-1.7\n";

/// Place a single `4 0 obj` stream object with the given dictionary body right
/// after [`PDF_PREFIX`], resolve its `/DecodeParms` declaration, and return both
/// the source bytes and the result so a range-carrying result can be checked
/// against the original bytes. The stream body is arbitrary: the resolver never
/// reads it.
fn resolve_capturing(
    dictionary: &[u8],
) -> (
    Vec<u8>,
    Result<FlateDecodeParametersResolution, FlateDecodeParametersResolutionError>,
) {
    let mut source = PDF_PREFIX.to_vec();
    source.extend_from_slice(b"4 0 obj\n");
    source.extend_from_slice(dictionary);
    source.extend_from_slice(b"\nstream\nIGNORED-BODY\nendstream\nendobj\n");
    let result = resolve_flate_decode_parameters(&source, PDF_PREFIX.len());
    (source, result)
}

/// Resolve the single-object source for `dictionary`, discarding the source.
fn resolve(
    dictionary: &[u8],
) -> Result<FlateDecodeParametersResolution, FlateDecodeParametersResolutionError> {
    resolve_capturing(dictionary).1
}

/// Resolve a dictionary that is expected to succeed.
fn resolve_ok(dictionary: &[u8]) -> FlateDecodeParametersResolution {
    resolve(dictionary).expect("decode-parms resolution should succeed")
}

/// Resolve a dictionary that is expected to be rejected as malformed.
fn resolve_err(dictionary: &[u8]) -> FlateDecodeParametersResolutionRejection {
    resolve(dictionary)
        .expect_err("decode-parms resolution should reject")
        .reason
}

#[test]
fn missing_decode_parms_resolves_to_defaults() {
    assert_eq!(
        resolve_ok(b"<< /Length 12 >>"),
        FlateDecodeParametersResolution::Resolved {
            parameters: FlateDecodeParameters::default(),
            decode_parms_key_range: None,
            parameters_dictionary_range: None,
        }
    );
}

#[test]
fn null_decode_parms_resolves_to_defaults_with_key_range() {
    let (source, result) = resolve_capturing(b"<< /DecodeParms null >>");
    let resolution = result.expect("null decode-parms should resolve");

    let FlateDecodeParametersResolution::Resolved {
        parameters,
        decode_parms_key_range,
        parameters_dictionary_range,
    } = resolution
    else {
        unreachable!("expected a resolved result, got {resolution:?}");
    };
    assert_eq!(parameters, FlateDecodeParameters::default());
    assert_eq!(parameters_dictionary_range, None);
    let key_range = decode_parms_key_range.expect("null value still locates the key range");
    assert_eq!(&source[key_range.start..key_range.end], b"/DecodeParms");
}

#[test]
fn full_predictor_dictionary_resolves_each_field() {
    let dictionary =
        b"<< /DecodeParms << /Predictor 12 /Columns 4 /Colors 1 /BitsPerComponent 8 >> >>";
    let (source, result) = resolve_capturing(dictionary);
    let resolution = result.expect("full predictor dictionary should resolve");

    let FlateDecodeParametersResolution::Resolved {
        parameters,
        decode_parms_key_range,
        parameters_dictionary_range,
    } = resolution
    else {
        unreachable!("expected a resolved result, got {resolution:?}");
    };
    assert_eq!(
        parameters,
        FlateDecodeParameters {
            predictor: 12,
            colors: 1,
            bits_per_component: 8,
            columns: 4,
        }
    );
    let key_range = decode_parms_key_range.expect("dictionary value locates the key range");
    assert_eq!(&source[key_range.start..key_range.end], b"/DecodeParms");
    let dict_range = parameters_dictionary_range.expect("dictionary value locates the value range");
    assert_eq!(
        &source[dict_range.start..dict_range.end],
        b"<< /Predictor 12 /Columns 4 /Colors 1 /BitsPerComponent 8 >>"
    );
}

#[test]
fn partial_predictor_dictionary_defaults_absent_keys() {
    let resolution = resolve_ok(b"<< /DecodeParms << /Predictor 12 >> >>");
    let FlateDecodeParametersResolution::Resolved { parameters, .. } = resolution else {
        unreachable!("expected a resolved result, got {resolution:?}");
    };
    // Only `/Predictor` is present; the other three fall back to defaults.
    assert_eq!(
        parameters,
        FlateDecodeParameters {
            predictor: 12,
            colors: 1,
            bits_per_component: 8,
            columns: 1,
        }
    );
}

#[test]
fn array_decode_parms_is_unsupported_array_skip() {
    let (source, result) = resolve_capturing(b"<< /DecodeParms [ null ] >>");
    let resolution = result.expect("array decode-parms should be a structured skip");

    let FlateDecodeParametersResolution::UnsupportedArrayParms {
        decode_parms_value_range,
    } = resolution
    else {
        unreachable!("expected an unsupported array skip, got {resolution:?}");
    };
    assert_eq!(
        &source[decode_parms_value_range.start..decode_parms_value_range.end],
        b"[ null ]"
    );
}

#[test]
fn duplicate_decode_parms_is_rejected() {
    let reason = resolve_err(b"<< /DecodeParms null /Other 0 /DecodeParms null >>");
    assert!(matches!(
        reason,
        FlateDecodeParametersResolutionRejection::DuplicateDecodeParms { .. }
    ));
}

#[test]
fn indirect_reference_decode_parms_value_is_rejected() {
    assert_eq!(
        resolve_err(b"<< /DecodeParms 5 0 R >>"),
        FlateDecodeParametersResolutionRejection::NonDictionaryParmsValue {
            value_kind: DictionaryValueKind::IndirectReferenceLike,
        }
    );
}

#[test]
fn duplicate_predictor_key_is_rejected() {
    let reason = resolve_err(b"<< /DecodeParms << /Predictor 12 /Predictor 12 >> >>");
    assert!(matches!(
        reason,
        FlateDecodeParametersResolutionRejection::DuplicateParameter {
            parameter: DecodeParmsParameter::Predictor,
            ..
        }
    ));
}

#[test]
fn non_integer_predictor_value_is_rejected() {
    assert_eq!(
        resolve_err(b"<< /DecodeParms << /Predictor /Nope >> >>"),
        FlateDecodeParametersResolutionRejection::NonIntegerParameterValue {
            parameter: DecodeParmsParameter::Predictor,
            value_kind: DictionaryValueKind::Name,
        }
    );
}

#[test]
fn malformed_digit_predictor_value_is_rejected() {
    // `1.0` is number-like but not pure ASCII digits over its full span.
    assert_eq!(
        resolve_err(b"<< /DecodeParms << /Predictor 1.0 >> >>"),
        FlateDecodeParametersResolutionRejection::MalformedParameterInteger {
            parameter: DecodeParmsParameter::Predictor,
        }
    );
}

#[test]
fn out_of_range_predictor_value_is_rejected() {
    // `99999` does not fit `bits_per_component: u8`.
    assert_eq!(
        resolve_err(b"<< /DecodeParms << /BitsPerComponent 99999 >> >>"),
        FlateDecodeParametersResolutionRejection::ParameterOutOfRange {
            parameter: DecodeParmsParameter::BitsPerComponent,
        }
    );
}

#[test]
fn malformed_parms_dictionary_entries_are_rejected() {
    // The outer scan classifies `<< 1 2 >>` as a balanced dictionary value, but
    // its inner entry scan finds a non-name top-level key.
    let reason = resolve_err(b"<< /DecodeParms << 1 2 >> >>");
    assert!(matches!(
        reason,
        FlateDecodeParametersResolutionRejection::ParmsDictEntries { .. }
    ));
}

#[test]
fn delegated_stream_start_failure_is_rejected() {
    // An array-bodied object is not a dictionary-bodied stream, so the delegated
    // `inspect_content_stream_start` failure surfaces verbatim.
    let mut source = PDF_PREFIX.to_vec();
    source.extend_from_slice(b"4 0 obj\n[ /DecodeParms null ]\nendobj\n");

    let reason = resolve_flate_decode_parameters(&source, PDF_PREFIX.len())
        .expect_err("non-dictionary body should reject")
        .reason;

    assert_eq!(
        reason,
        FlateDecodeParametersResolutionRejection::StreamStart {
            stream_start_reason: ContentStreamStartInspectionRejection::NonDictionaryBody {
                token_kind: IndirectObjectBodyLeadingTokenKind::ArrayOpen,
            },
        }
    );
}

#[test]
fn report_retains_no_predictor_value_bytes() {
    let error = resolve(b"<< /DecodeParms << /Predictor /SecretPredictorName >> >>")
        .expect_err("non-integer predictor should reject");
    let debug = format!("{error:?}");
    assert!(!debug.contains("SecretPredictorName"));
}

/// Build a synthetic single-page classic-xref PDF whose one content stream uses
/// the given dictionary body. The stream body is arbitrary; only the `/Filter`
/// and `/DecodeParms` declarations are consulted.
fn single_page_pdf(content_dict: &[u8]) -> Vec<u8> {
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n";

    let mut content = Vec::new();
    content.extend_from_slice(b"4 0 obj\n");
    content.extend_from_slice(content_dict);
    content.extend_from_slice(b"\nstream\nIGNORED-BODY\nendstream\nendobj\n");

    let mut source = Vec::new();
    source.extend_from_slice(PDF_PREFIX);
    let catalog_offset = source.len();
    source.extend_from_slice(catalog);
    let pages_offset = source.len();
    source.extend_from_slice(pages);
    let page_offset = source.len();
    source.extend_from_slice(page);
    let content_offset = source.len();
    source.extend_from_slice(&content);

    let xref_offset = source.len();
    source.extend_from_slice(b"xref\n0 5\n");
    source.extend_from_slice(b"0000000000 65535 f \n");
    for offset in [catalog_offset, pages_offset, page_offset, content_offset] {
        source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    source.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
    );
    source
}

#[test]
fn composes_classifier_and_resolver_over_document_access_spine() {
    let source = single_page_pdf(
        b"<< /Length 12 /Filter /FlateDecode /DecodeParms << /Predictor 12 /Columns 4 /Colors 1 /BitsPerComponent 8 >> >>",
    );

    let access = inspect_classic_document_access(&source)
        .expect("classic document-access spine should compose");
    assert_eq!(access.page_leaves.leaf_count(), 1);
    let page_offset = access.page_leaves.leaves[0].object_byte_offset;

    let contents =
        inspect_page_contents(&source, page_offset).expect("page /Contents should inspect");
    let targets = inspect_page_content_targets(&source, &access.xref_table, &contents);
    let PageContentTargetInspection::Resolved {
        object_byte_offset, ..
    } = targets.entries[0]
    else {
        unreachable!("the single content reference should resolve to an object offset");
    };

    let classification = classify_content_stream_filter(&source, object_byte_offset)
        .expect("resolved content stream should classify");
    assert_eq!(classification, ContentStreamFilterClassification::Flate);

    let resolution = resolve_flate_decode_parameters(&source, object_byte_offset)
        .expect("resolved content stream should resolve decode parameters");
    let FlateDecodeParametersResolution::Resolved { parameters, .. } = resolution else {
        unreachable!("expected a resolved result, got {resolution:?}");
    };
    assert_eq!(
        parameters,
        FlateDecodeParameters {
            predictor: 12,
            colors: 1,
            bits_per_component: 8,
            columns: 4,
        }
    );
}

#[test]
fn serde_round_trips_resolution_shapes() {
    for resolution in [
        resolve_ok(b"<< /Length 12 >>"),
        resolve_ok(b"<< /DecodeParms null >>"),
        resolve_ok(b"<< /DecodeParms << /Predictor 12 /Columns 4 >> >>"),
        resolve_ok(b"<< /DecodeParms [ null ] >>"),
    ] {
        let value = serde_value(&resolution).expect("resolution should serialize");
        let restored: FlateDecodeParametersResolution =
            from_serde_value(value).expect("resolution should deserialize");
        assert_eq!(restored, resolution);
    }
}

#[test]
fn serde_round_trips_rejection_shape() {
    let error =
        resolve(b"<< /DecodeParms 5 0 R >>").expect_err("indirect decode-parms should reject");

    let value = serde_value(&error).expect("error should serialize");
    let restored: FlateDecodeParametersResolutionError =
        from_serde_value(value).expect("error should deserialize");
    assert_eq!(restored, error);
}

#[test]
fn serde_round_trips_parameter_enum() {
    for parameter in [
        DecodeParmsParameter::Predictor,
        DecodeParmsParameter::Colors,
        DecodeParmsParameter::BitsPerComponent,
        DecodeParmsParameter::Columns,
    ] {
        let value = serde_value(&parameter).expect("parameter should serialize");
        let restored: DecodeParmsParameter =
            from_serde_value(value).expect("parameter should deserialize");
        assert_eq!(restored, parameter);
    }
}
