use serde::{Deserialize, Serialize};

use crate::xref_stream::{IntegerError, parse_non_negative_integer, unique_entry};
use crate::{
    ContentStreamStartInspectionRejection, DictionaryEntryByteRange,
    DictionaryEntryInspectionRejection, DictionaryEntrySpan, DictionaryValueKind,
    FlateDecodeParameters, inspect_content_stream_start, inspect_dictionary_entries,
};

const DECODE_PARMS_KEY: &[u8] = b"/DecodeParms";
const PREDICTOR_KEY: &[u8] = b"/Predictor";
const COLORS_KEY: &[u8] = b"/Colors";
const BITS_PER_COMPONENT_KEY: &[u8] = b"/BitsPerComponent";
const COLUMNS_KEY: &[u8] = b"/Columns";

/// One of the four `/FlateDecode` predictor parameters this resolver consults.
///
/// Carried by the rejection reasons so a malformed `/DecodeParms` predictor
/// entry names which key failed without copying the key bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeParmsParameter {
    /// `/Predictor`.
    Predictor,
    /// `/Colors`.
    Colors,
    /// `/BitsPerComponent`.
    BitsPerComponent,
    /// `/Columns`.
    Columns,
}

/// Resolution of a content stream's top-level `/DecodeParms` declaration into a
/// concrete [`FlateDecodeParameters`].
///
/// This is the parameter half of the future "real PDF -> inventory" Flate
/// branch: [`crate::classify_content_stream_filter`] answers "which filter?" and
/// this resolver answers "with which decode parameters?". Together they fully
/// specify the `decode_flate_stream` call without this slice reading a single
/// stream-body byte.
///
/// Every variant carries only the small `Copy` [`FlateDecodeParameters`] and
/// byte ranges. It retains or copies no PDF bytes, object bodies, stream bodies,
/// decoded bytes, or source slices; predictor integers are parsed in place from
/// `input[range]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resolution", rename_all = "snake_case")]
pub enum FlateDecodeParametersResolution {
    /// Concrete parameters were resolved.
    ///
    /// Both ranges are `None` for an absent `/DecodeParms` (defaults); the key
    /// range is `Some` and the dictionary range `None` for a `null` value
    /// (defaults); both ranges are `Some` for a parameters dictionary.
    Resolved {
        /// Resolved decode parameters (defaults for any absent key).
        parameters: FlateDecodeParameters,
        /// Byte range covering the exact top-level raw `/DecodeParms` key, when
        /// the key is present.
        decode_parms_key_range: Option<DictionaryEntryByteRange>,
        /// Byte range covering the `/DecodeParms` dictionary value span, when the
        /// value is a parameters dictionary.
        parameters_dictionary_range: Option<DictionaryEntryByteRange>,
    },
    /// The `/DecodeParms` value is an array (the per-filter-chain form): a
    /// structured skip, deferred to a later slice rather than treated as
    /// defaults or as an error.
    UnsupportedArrayParms {
        /// Byte range covering the `/DecodeParms` array value span.
        decode_parms_value_range: DictionaryEntryByteRange,
    },
}

/// Error returned when a content stream's `/DecodeParms` declaration is
/// malformed.
///
/// `Err` is reserved for malformed structure; an array `/DecodeParms` is an `Ok`
/// [`UnsupportedArrayParms`](FlateDecodeParametersResolution::UnsupportedArrayParms)
/// skip. This report retains or copies no PDF bytes; it carries only offsets,
/// the source length, and the structured reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlateDecodeParametersResolutionError {
    /// Caller-supplied content stream object byte offset where resolution began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the malformed construct was found, when available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: FlateDecodeParametersResolutionRejection,
}

/// Structured `/DecodeParms` resolution rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum FlateDecodeParametersResolutionRejection {
    /// The delegated [`inspect_content_stream_start`] inspection failed, so the
    /// object is not a dictionary-bodied content stream.
    StreamStart {
        /// Underlying content-stream start rejection reason.
        stream_start_reason: ContentStreamStartInspectionRejection,
    },
    /// The stream dictionary has more than one exact top-level raw
    /// `/DecodeParms` key.
    DuplicateDecodeParms {
        /// First `/DecodeParms` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/DecodeParms` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/DecodeParms` value is none of dictionary, `null`, or array.
    NonDictionaryParmsValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// A delegated parms-dictionary entry scan failed.
    ParmsDictEntries {
        /// Underlying dictionary entry inspection rejection reason.
        dictionary_entries_reason: DictionaryEntryInspectionRejection,
    },
    /// A predictor key appears more than once in the parms dictionary.
    DuplicateParameter {
        /// Predictor parameter whose key was duplicated.
        parameter: DecodeParmsParameter,
        /// First predictor key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate predictor key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// A predictor value's shallow kind is not number-like.
    NonIntegerParameterValue {
        /// Predictor parameter whose value was not number-like.
        parameter: DecodeParmsParameter,
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// A predictor value is number-like but not pure ASCII digits over its full
    /// value span.
    MalformedParameterInteger {
        /// Predictor parameter whose value was malformed.
        parameter: DecodeParmsParameter,
    },
    /// A predictor integer does not fit its field type
    /// (`predictor: u16`, `colors: u32`, `bits_per_component: u8`,
    /// `columns: u32`) or overflows `usize`.
    ParameterOutOfRange {
        /// Predictor parameter whose value was out of range.
        parameter: DecodeParmsParameter,
    },
}

/// Resolve a content stream object's top-level `/DecodeParms` declaration into a
/// concrete [`FlateDecodeParameters`], a structured array skip, or a malformed
/// rejection.
///
/// The helper delegates dictionary and `stream`-keyword validation to
/// [`inspect_content_stream_start`] and reuses its delegated top-level
/// `dictionary.entries`, reimplementing no header, body-token, dictionary-open,
/// or entry-span scanning. It matches the exact top-level raw key bytes
/// `/DecodeParms` with [`crate::xref_stream::unique_entry`], exactly as
/// [`crate::classify_content_stream_filter`] matches `/Filter`:
///
/// - a missing `/DecodeParms` resolves to [`FlateDecodeParameters::default`]
///   with both ranges `None` (per PDF 32000 §7.4.1, an absent parameters entry
///   means default values);
/// - a single `null` value resolves the same way, with the key range `Some` and
///   the dictionary range `None`;
/// - a single array value is an `Ok`
///   [`UnsupportedArrayParms`](FlateDecodeParametersResolution::UnsupportedArrayParms)
///   skip, the deferred per-filter-chain follow-up;
/// - a single dictionary value is located with [`inspect_dictionary_entries`]
///   and its `/Predictor`, `/Colors`, `/BitsPerComponent`, and `/Columns`
///   entries are each matched once with `unique_entry` and parsed in place with
///   [`crate::xref_stream::parse_non_negative_integer`]; an absent key falls back
///   to its `FlateDecodeParameters` default.
///
/// It reads, decodes, inflates, and tokenizes no stream-body bytes, calls no
/// `decode_flate_stream`, resolves no indirect-reference value, parses no `/DP`,
/// and mutates no PDF bytes. The report carries only byte ranges, the small
/// `Copy` `FlateDecodeParameters`, and `Copy` enums.
///
/// # Errors
///
/// Returns [`FlateDecodeParametersResolutionError`] for a delegated
/// [`StreamStart`](FlateDecodeParametersResolutionRejection::StreamStart)
/// failure, a
/// [`DuplicateDecodeParms`](FlateDecodeParametersResolutionRejection::DuplicateDecodeParms),
/// a `/DecodeParms` value that is none of dictionary/`null`/array
/// ([`NonDictionaryParmsValue`](FlateDecodeParametersResolutionRejection::NonDictionaryParmsValue)),
/// a delegated parms-dictionary entry-scan failure
/// ([`ParmsDictEntries`](FlateDecodeParametersResolutionRejection::ParmsDictEntries)),
/// a duplicated predictor key
/// ([`DuplicateParameter`](FlateDecodeParametersResolutionRejection::DuplicateParameter)),
/// a non-number-like predictor value
/// ([`NonIntegerParameterValue`](FlateDecodeParametersResolutionRejection::NonIntegerParameterValue)),
/// a number-like but non-digit predictor value
/// ([`MalformedParameterInteger`](FlateDecodeParametersResolutionRejection::MalformedParameterInteger)),
/// or a predictor integer that overflows its field type
/// ([`ParameterOutOfRange`](FlateDecodeParametersResolutionRejection::ParameterOutOfRange)).
pub fn resolve_flate_decode_parameters(
    input: &[u8],
    object_offset: usize,
) -> Result<FlateDecodeParametersResolution, FlateDecodeParametersResolutionError> {
    let ctx = ErrorContext {
        byte_offset: object_offset,
        byte_len: input.len(),
    };

    let stream_start = inspect_content_stream_start(input, object_offset).map_err(|error| {
        ctx.error(
            FlateDecodeParametersResolutionRejection::StreamStart {
                stream_start_reason: error.reason,
            },
            error.error_byte_offset,
        )
    })?;

    let Some(entry) = unique_entry(input, &stream_start.dictionary.entries, DECODE_PARMS_KEY)
        .map_err(|(first_key_range, duplicate_key_range)| {
            ctx.error(
                FlateDecodeParametersResolutionRejection::DuplicateDecodeParms {
                    first_key_range,
                    duplicate_key_range,
                },
                Some(duplicate_key_range.start),
            )
        })?
    else {
        return Ok(FlateDecodeParametersResolution::Resolved {
            parameters: FlateDecodeParameters::default(),
            decode_parms_key_range: None,
            parameters_dictionary_range: None,
        });
    };

    match entry.value_kind {
        DictionaryValueKind::Null => Ok(FlateDecodeParametersResolution::Resolved {
            parameters: FlateDecodeParameters::default(),
            decode_parms_key_range: Some(entry.key_range),
            parameters_dictionary_range: None,
        }),
        DictionaryValueKind::Array => Ok(FlateDecodeParametersResolution::UnsupportedArrayParms {
            decode_parms_value_range: entry.value_range,
        }),
        DictionaryValueKind::Dictionary => resolve_parms_dictionary(input, entry, ctx),
        value_kind => Err(ctx.error(
            FlateDecodeParametersResolutionRejection::NonDictionaryParmsValue { value_kind },
            Some(entry.value_range.start),
        )),
    }
}

/// Resolve the four predictor integer keys inside a located `/DecodeParms`
/// dictionary value.
///
/// The only new abstraction over the existing inspectors: a bounded consultation
/// of `/Predictor`, `/Colors`, `/BitsPerComponent`, and `/Columns`, each via the
/// shared unique-entry and non-negative-integer helpers, defaulting any absent
/// key to its `FlateDecodeParameters` default.
fn resolve_parms_dictionary(
    input: &[u8],
    entry: DictionaryEntrySpan,
    ctx: ErrorContext,
) -> Result<FlateDecodeParametersResolution, FlateDecodeParametersResolutionError> {
    let inner = inspect_dictionary_entries(input, entry.value_range.start).map_err(|error| {
        ctx.error(
            FlateDecodeParametersResolutionRejection::ParmsDictEntries {
                dictionary_entries_reason: error.reason,
            },
            error.error_byte_offset,
        )
    })?;

    let defaults = FlateDecodeParameters::default();
    let predictor = resolve_parameter(
        input,
        &inner.entries,
        PREDICTOR_KEY,
        DecodeParmsParameter::Predictor,
        defaults.predictor,
        ctx,
    )?;
    let colors = resolve_parameter(
        input,
        &inner.entries,
        COLORS_KEY,
        DecodeParmsParameter::Colors,
        defaults.colors,
        ctx,
    )?;
    let bits_per_component = resolve_parameter(
        input,
        &inner.entries,
        BITS_PER_COMPONENT_KEY,
        DecodeParmsParameter::BitsPerComponent,
        defaults.bits_per_component,
        ctx,
    )?;
    let columns = resolve_parameter(
        input,
        &inner.entries,
        COLUMNS_KEY,
        DecodeParmsParameter::Columns,
        defaults.columns,
        ctx,
    )?;

    Ok(FlateDecodeParametersResolution::Resolved {
        parameters: FlateDecodeParameters {
            predictor,
            colors,
            bits_per_component,
            columns,
        },
        decode_parms_key_range: Some(entry.key_range),
        parameters_dictionary_range: Some(entry.value_range),
    })
}

/// Resolve a single predictor key into its field type, defaulting an absent key.
///
/// Locates the exact raw `key` with [`unique_entry`], requires a number-like
/// value, parses it in place with [`parse_non_negative_integer`], and fits it
/// into the field type `T` with [`TryFrom<usize>`]. A duplicate key, a
/// non-number-like value, a number-like-but-non-digit value, and an
/// over-`usize`/over-field-type value are each a distinct structured rejection;
/// no PDF bytes are retained.
fn resolve_parameter<T: TryFrom<usize>>(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
    key: &[u8],
    parameter: DecodeParmsParameter,
    default: T,
    ctx: ErrorContext,
) -> Result<T, FlateDecodeParametersResolutionError> {
    let Some(entry) =
        unique_entry(input, entries, key).map_err(|(first_key_range, duplicate_key_range)| {
            ctx.error(
                FlateDecodeParametersResolutionRejection::DuplicateParameter {
                    parameter,
                    first_key_range,
                    duplicate_key_range,
                },
                Some(duplicate_key_range.start),
            )
        })?
    else {
        return Ok(default);
    };

    if entry.value_kind != DictionaryValueKind::NumberLike {
        return Err(ctx.error(
            FlateDecodeParametersResolutionRejection::NonIntegerParameterValue {
                parameter,
                value_kind: entry.value_kind,
            },
            Some(entry.value_range.start),
        ));
    }

    let value =
        match parse_non_negative_integer(&input[entry.value_range.start..entry.value_range.end]) {
            Ok(value) => value,
            Err(IntegerError::Malformed) => {
                return Err(ctx.error(
                    FlateDecodeParametersResolutionRejection::MalformedParameterInteger {
                        parameter,
                    },
                    Some(entry.value_range.start),
                ));
            }
            Err(IntegerError::OutOfRange) => {
                return Err(ctx.error(
                    FlateDecodeParametersResolutionRejection::ParameterOutOfRange { parameter },
                    Some(entry.value_range.start),
                ));
            }
        };

    T::try_from(value).map_err(|_| {
        ctx.error(
            FlateDecodeParametersResolutionRejection::ParameterOutOfRange { parameter },
            Some(entry.value_range.start),
        )
    })
}

/// Copyable byte-context shared by the resolution helpers so each can build a
/// [`FlateDecodeParametersResolutionError`] without re-threading the caller
/// offset and source length.
#[derive(Clone, Copy)]
struct ErrorContext {
    byte_offset: usize,
    byte_len: usize,
}

impl ErrorContext {
    const fn error(
        self,
        reason: FlateDecodeParametersResolutionRejection,
        error_byte_offset: Option<usize>,
    ) -> FlateDecodeParametersResolutionError {
        FlateDecodeParametersResolutionError {
            byte_offset: self.byte_offset,
            byte_len: self.byte_len,
            error_byte_offset,
            reason,
        }
    }
}
