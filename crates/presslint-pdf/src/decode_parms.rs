use serde::{Deserialize, Serialize};

use crate::source_utils::{
    skip_hex_string, skip_literal_string, skip_name, skip_scalar_token,
    skip_whitespace_and_comments,
};
use crate::xref_stream::{IntegerError, parse_non_negative_integer, unique_entry};
use crate::{
    ContentStreamStartInspectionRejection, DictionaryEntryByteRange,
    DictionaryEntryInspectionRejection, DictionaryEntrySpan, DictionaryValueKind,
    FlateDecodeParameters, inspect_array_extent, inspect_content_stream_start,
    inspect_dictionary_entries, inspect_dictionary_extent,
};

const NULL_KEYWORD: &[u8] = b"null";

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
    /// The `/DecodeParms` value is an array shape this single-filter slice does
    /// not treat as effective parameters: an empty array, a two-or-more-element
    /// array (the genuine per-filter-chain form), or a single element that is
    /// neither `null` nor a dictionary. A structured skip, deferred to a later
    /// multi-filter slice rather than treated as defaults or as an error.
    ///
    /// A single-element `[null]` or `[<< ... >>]` array is instead resolved to
    /// [`Resolved`](Self::Resolved), since it is semantically equivalent to the
    /// direct `null`/dictionary form for the already-classified single Flate
    /// filter (PDF 32000 §7.4.1, `/DecodeParms` parallel to a one-filter
    /// `/Filter`).
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
    /// A `/DecodeParms` array element could not be shallowly bounded within the
    /// array body: an unbalanced or overrunning dictionary, sub-array, or string
    /// element. Through the public stream resolver, unbalanced inner
    /// dictionaries are normally caught earlier by the outer stream-dictionary
    /// extent scan as `StreamStart`; this variant remains a defensive outcome for
    /// the bounded array-body scanner and is pinned by serde coverage.
    MalformedArrayElement {
        /// Byte range covering the `/DecodeParms` array value span.
        decode_parms_value_range: DictionaryEntryByteRange,
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
/// - an array value is scanned shallowly for its elements: a single-element
///   `[null]` resolves to defaults and a single-element `[<< ... >>]` resolves
///   its inner predictor dictionary, both exactly as their direct `null` and
///   dictionary forms would (PDF 32000 §7.4.1 treats `/DecodeParms` as parallel
///   to `/Filter`, so a one-element array is the one-filter case). An empty
///   array, a two-or-more-element array, or a single element that is neither
///   `null` nor a dictionary stays an `Ok`
///   [`UnsupportedArrayParms`](FlateDecodeParametersResolution::UnsupportedArrayParms)
///   skip, the deferred multi-filter follow-up;
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
/// a `/DecodeParms` array element that cannot be shallowly bounded
/// ([`MalformedArrayElement`](FlateDecodeParametersResolutionRejection::MalformedArrayElement)),
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
        DictionaryValueKind::Array => resolve_parms_array(input, entry, ctx),
        DictionaryValueKind::Dictionary => resolve_parms_dictionary(input, entry, ctx),
        value_kind => Err(ctx.error(
            FlateDecodeParametersResolutionRejection::NonDictionaryParmsValue { value_kind },
            Some(entry.value_range.start),
        )),
    }
}

/// Shallow classification of a single `/DecodeParms` array element.
///
/// Only enough is decided to route the single-filter slice: a `null` element and
/// a dictionary element are the two effective single-element forms; every other
/// shape (name, number, string, sub-array, or a multi-token scalar run such as an
/// `N G R` indirect reference, whose tokens each count as one `Other` element) is
/// not effective and keeps the array a structured skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ParmsArrayElement {
    /// A `null` scalar element: defaults, mirroring a direct `null` value.
    Null,
    /// A `<< ... >>` dictionary element; the range covers its balanced extent.
    Dictionary {
        /// Byte range covering the dictionary element's balanced `<< ... >>`
        /// span.
        value_range: DictionaryEntryByteRange,
    },
    /// Any other element kind: not effective for this single-filter slice.
    Other,
}

/// Result of the shallow `/DecodeParms` array element scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParmsArrayScan {
    /// Number of shallow elements observed in the array body.
    element_count: usize,
    /// The first element, when the array is non-empty.
    first_element: Option<ParmsArrayElement>,
}

/// Resolve an array `/DecodeParms` value.
///
/// A single-element `[null]` resolves to defaults and a single-element
/// `[<< ... >>]` delegates its inner dictionary to [`resolve_parms_dictionary`],
/// reusing the same `/DecodeParms` key range so the resolved report is
/// indistinguishable from the direct forms. Every other cardinality or element
/// shape stays a structured
/// [`UnsupportedArrayParms`](FlateDecodeParametersResolution::UnsupportedArrayParms)
/// skip; a malformed array element is a
/// [`MalformedArrayElement`](FlateDecodeParametersResolutionRejection::MalformedArrayElement)
/// rejection.
fn resolve_parms_array(
    input: &[u8],
    entry: DictionaryEntrySpan,
    ctx: ErrorContext,
) -> Result<FlateDecodeParametersResolution, FlateDecodeParametersResolutionError> {
    let scan = scan_parms_array(input, entry.value_range, ctx)?;

    match (scan.element_count, scan.first_element) {
        (1, Some(ParmsArrayElement::Null)) => Ok(FlateDecodeParametersResolution::Resolved {
            parameters: FlateDecodeParameters::default(),
            decode_parms_key_range: Some(entry.key_range),
            parameters_dictionary_range: None,
        }),
        (1, Some(ParmsArrayElement::Dictionary { value_range })) => resolve_parms_dictionary(
            input,
            DictionaryEntrySpan {
                key_range: entry.key_range,
                value_range,
                value_kind: DictionaryValueKind::Dictionary,
            },
            ctx,
        ),
        _ => Ok(FlateDecodeParametersResolution::UnsupportedArrayParms {
            decode_parms_value_range: entry.value_range,
        }),
    }
}

/// Scan the shallow elements of a `/DecodeParms` array, counting them and
/// classifying the first.
///
/// The array is already known balanced (its value was classified as
/// [`DictionaryValueKind::Array`]); this bounds every element scan to the array
/// body by slicing the source at the closing `]`, so a dictionary/sub-array/
/// string element that would overrun the array is rejected as
/// [`MalformedArrayElement`](FlateDecodeParametersResolutionRejection::MalformedArrayElement)
/// rather than silently consuming trailing bytes. It decodes no element contents
/// and retains no PDF bytes; only byte ranges and small counts flow out.
fn scan_parms_array(
    input: &[u8],
    value_range: DictionaryEntryByteRange,
    ctx: ErrorContext,
) -> Result<ParmsArrayScan, FlateDecodeParametersResolutionError> {
    let malformed = |error_byte_offset: Option<usize>| {
        ctx.error(
            FlateDecodeParametersResolutionRejection::MalformedArrayElement {
                decode_parms_value_range: value_range,
            },
            error_byte_offset,
        )
    };

    let array = inspect_array_extent(input, value_range.start)
        .map_err(|error| malformed(error.error_byte_offset))?;
    let body_end = array.close_byte_offset;
    // Bounding the source at the closing `]` keeps every delegated element scan
    // inside the array body, so an unterminated element cannot run past it.
    let bounded = &input[..body_end];

    let mut cursor = array.open_byte_offset + 1;
    let mut element_count = 0usize;
    let mut first_element = None;

    while cursor < body_end {
        cursor = skip_whitespace_and_comments(input, cursor, body_end);
        if cursor >= body_end {
            break;
        }

        let (element, next) = classify_parms_array_element(bounded, cursor, body_end, malformed)?;
        if first_element.is_none() {
            first_element = Some(element);
        }
        element_count += 1;
        cursor = next;
    }

    Ok(ParmsArrayScan {
        element_count,
        first_element,
    })
}

/// Classify the array element at `cursor` and return the offset just past it.
///
/// `bounded` is the source truncated at the array's closing `]`, so the
/// delegated dictionary/array/string extent helpers cannot cross the array
/// boundary; a delegated failure maps to a `malformed` rejection.
fn classify_parms_array_element(
    bounded: &[u8],
    cursor: usize,
    body_end: usize,
    malformed: impl Fn(Option<usize>) -> FlateDecodeParametersResolutionError,
) -> Result<(ParmsArrayElement, usize), FlateDecodeParametersResolutionError> {
    match bounded[cursor] {
        b'<' if bounded.get(cursor + 1) == Some(&b'<') => {
            let dictionary = inspect_dictionary_extent(bounded, cursor)
                .map_err(|error| malformed(error.error_byte_offset))?;
            Ok((
                ParmsArrayElement::Dictionary {
                    value_range: DictionaryEntryByteRange {
                        start: cursor,
                        end: dictionary.after_close_byte_offset,
                    },
                },
                dictionary.after_close_byte_offset,
            ))
        }
        b'[' => {
            let array = inspect_array_extent(bounded, cursor)
                .map_err(|error| malformed(error.error_byte_offset))?;
            Ok((ParmsArrayElement::Other, array.after_close_byte_offset))
        }
        b'(' => {
            let end =
                skip_literal_string(bounded, cursor).ok_or_else(|| malformed(Some(cursor)))?;
            Ok((ParmsArrayElement::Other, end))
        }
        b'<' => {
            let end = skip_hex_string(bounded, cursor).ok_or_else(|| malformed(Some(cursor)))?;
            Ok((ParmsArrayElement::Other, end))
        }
        b'/' => {
            let end = skip_name(bounded, cursor, body_end);
            Ok((ParmsArrayElement::Other, end))
        }
        _ => {
            let end = skip_scalar_token(bounded, cursor, body_end);
            let element = if &bounded[cursor..end] == NULL_KEYWORD {
                ParmsArrayElement::Null
            } else {
                ParmsArrayElement::Other
            };
            Ok((element, end))
        }
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
