//! Bounded, read-only ICC profile header descriptor facts for `ICCBased`
//! profile streams.
//!
//! This module is a byte-facts slice, not a colour-management slice. It parses
//! the fixed 128-byte ICC profile header (ICC.1:2022, big-endian) into
//! [`IccProfileHeaderDescriptor`] and composes the existing PDF stream machinery
//! into a bounded [`inspect_icc_profile_stream_with_lookup`] helper. It does not
//! call a CMM (lcms/skcms), does not parse the ICC tag table, does not inspect
//! `desc`/`wtpt`/`A2B0`, does not convert colour, and never mutates PDF bytes.
//!
//! Every uninspectable or malformed outcome — an unresolved or compressed
//! profile object, an unlocatable stream extent, an unsupported filter, a
//! declared `/DecodeParms`, a decode failure, output-limit exhaustion, or a
//! decoded payload shorter than the 128-byte header — is a structured
//! [`IccProfileStreamInspection`] result, never a panic.

use serde::{Deserialize, Serialize};

use crate::{
    ContentStreamFilterClassification, FlateDecodeParametersResolution, FlateDecodeStreamRejection,
    IndirectRef, ObjectLookup, ObjectResolutionRejection, classify_content_stream_filter,
    content_stream_data_slice, decode_flate_stream, inspect_content_stream_data_extent_with_lookup,
    resolve_flate_decode_parameters, resolve_xref_object_offset,
};

/// Fixed ICC profile header length in bytes (ICC.1:2022 §7.2).
pub const ICC_PROFILE_HEADER_LEN: usize = 128;

/// Byte-level facts parsed from a decoded ICC profile header.
///
/// The descriptor records only what the first 128 bytes literally say. Raw
/// four-byte signatures are preserved verbatim next to any decoded convenience
/// field, so a signature that contains spaces or unknown bytes is a fact, not a
/// parse error. The only condition that prevents a descriptor from being built
/// is a decoded payload shorter than [`ICC_PROFILE_HEADER_LEN`].
///
/// This carries no tag-table facts, no colorimetry, and no rendering intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct IccProfileHeaderDescriptor {
    /// Total decoded profile byte length the header was parsed from.
    pub decoded_len: usize,
    /// Declared profile size from header bytes `0..4` (big-endian).
    pub declared_profile_size: u32,
    /// Raw profile version bytes `8..12`.
    pub version_raw: [u8; 4],
    /// Decoded major version (header byte `8`).
    pub version_major: u8,
    /// Decoded minor version (high nibble of header byte `9`).
    pub version_minor: u8,
    /// Decoded bug-fix version (low nibble of header byte `9`).
    pub version_bugfix: u8,
    /// Raw profile/device class signature, header bytes `12..16`.
    pub profile_class_signature: [u8; 4],
    /// Raw data colour-space signature, header bytes `16..20`.
    pub data_color_space_signature: [u8; 4],
    /// Raw profile connection space signature, header bytes `20..24`.
    pub pcs_signature: [u8; 4],
    /// Whether the `acsp` file signature is present at header bytes `36..40`.
    pub acsp_present: bool,
}

impl IccProfileHeaderDescriptor {
    /// Recognized component count implied by the data colour-space signature.
    ///
    /// Conservative by design: only the signatures with an unambiguous PDF
    /// component count map to a value. `GRAY` is 1, `RGB `/`CMY ` are 3, `CMYK`
    /// is 4, and `2CLR`..`FCLR` map to 2..15. Every other signature (including
    /// `XYZ `, `Lab `, and unknown four-byte runs) returns `None`, so no
    /// component-count comparison is attempted against it.
    #[must_use]
    pub fn data_space_component_count(&self) -> Option<usize> {
        data_space_component_count(self.data_color_space_signature)
    }
}

/// Recognized component count for a raw ICC data colour-space signature.
fn data_space_component_count(signature: [u8; 4]) -> Option<usize> {
    match &signature {
        b"GRAY" => Some(1),
        b"RGB " | b"CMY " => Some(3),
        b"CMYK" => Some(4),
        _ => nclr_component_count(signature),
    }
}

/// Component count for an `nCLR` signature, where the leading ASCII hex digit
/// `2`..`F` names 2..15 colorants (ICC.1:2022 Table 19).
fn nclr_component_count(signature: [u8; 4]) -> Option<usize> {
    if &signature[1..] != b"CLR" {
        return None;
    }
    match signature[0] {
        digit @ b'2'..=b'9' => Some(usize::from(digit - b'0')),
        letter @ b'A'..=b'F' => Some(usize::from(letter - b'A') + 10),
        _ => None,
    }
}

/// Structured outcome of parsing a decoded ICC profile header slice.
///
/// The parser's only rejection is a truncated payload; a missing `acsp` marker,
/// an unusual version, or an unknown signature all yield a populated descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum IccProfileHeaderParse {
    /// The decoded payload was at least [`ICC_PROFILE_HEADER_LEN`] bytes and the
    /// header facts were extracted.
    Parsed {
        /// Parsed header descriptor.
        descriptor: IccProfileHeaderDescriptor,
    },
    /// The decoded payload was shorter than the fixed header length.
    Truncated {
        /// Observed decoded byte length.
        decoded_len: usize,
    },
}

/// Parse a decoded ICC profile byte slice into header facts.
///
/// Reads only the fixed 128-byte header. A payload shorter than
/// [`ICC_PROFILE_HEADER_LEN`] is reported as
/// [`Truncated`](IccProfileHeaderParse::Truncated); everything else, including a
/// corrupt `acsp` marker or an unknown class/space signature, is reported as a
/// populated [`Parsed`](IccProfileHeaderParse::Parsed) descriptor.
#[must_use]
pub fn parse_icc_profile_header(decoded: &[u8]) -> IccProfileHeaderParse {
    if decoded.len() < ICC_PROFILE_HEADER_LEN {
        return IccProfileHeaderParse::Truncated {
            decoded_len: decoded.len(),
        };
    }

    let declared_profile_size =
        u32::from_be_bytes([decoded[0], decoded[1], decoded[2], decoded[3]]);
    let version_raw = [decoded[8], decoded[9], decoded[10], decoded[11]];
    let profile_class_signature = [decoded[12], decoded[13], decoded[14], decoded[15]];
    let data_color_space_signature = [decoded[16], decoded[17], decoded[18], decoded[19]];
    let pcs_signature = [decoded[20], decoded[21], decoded[22], decoded[23]];
    let acsp_present = &decoded[36..40] == b"acsp";

    IccProfileHeaderParse::Parsed {
        descriptor: IccProfileHeaderDescriptor {
            decoded_len: decoded.len(),
            declared_profile_size,
            version_raw,
            version_major: version_raw[0],
            version_minor: version_raw[1] >> 4,
            version_bugfix: version_raw[1] & 0x0F,
            profile_class_signature,
            data_color_space_signature,
            pcs_signature,
            acsp_present,
        },
    }
}

/// Result of a bounded ICC profile stream inspection.
///
/// This is subordinate to [`IccProfileHeaderDescriptor`]: it either delivers the
/// parsed header, reports a decoded-but-truncated payload, or reports a
/// structured [`IccProfileInspectionGap`] describing why decoded header bytes
/// could not be reached. It is never an audit-fatal error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "inspection", rename_all = "snake_case")]
pub enum IccProfileStreamInspection {
    /// The profile stream decoded and its header parsed.
    Parsed {
        /// Parsed header descriptor.
        descriptor: IccProfileHeaderDescriptor,
    },
    /// The profile stream decoded but the payload was shorter than the header.
    Truncated {
        /// Observed decoded byte length.
        decoded_len: usize,
    },
    /// Decoded header bytes could not be reached; the reason is structured.
    Gap {
        /// Why the profile could not be inspected.
        reason: IccProfileInspectionGap,
    },
}

/// Why a bounded ICC profile stream inspection could not reach decoded header
/// bytes.
///
/// Every variant is a coverage gap, not an anomaly in the profile itself. All
/// variants are unit-only so each serializes as a plain `snake_case` string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IccProfileInspectionGap {
    /// The profile reference resolved to a type-2 compressed object-stream
    /// member. A stream object cannot be an object-stream member in conforming
    /// PDF, so this is reported rather than extracted.
    ProfileObjectCompressed,
    /// The profile reference could not be resolved to an in-use uncompressed
    /// object (unresolved, reserved, free, generation mismatch, or malformed
    /// header).
    ProfileObjectUnresolved,
    /// The stream-data byte extent could not be located.
    StreamExtent,
    /// The located extent could not be bridged to a byte slice.
    StreamSlice,
    /// The `/Filter` declaration could not be classified.
    FilterClassification,
    /// The stream uses a filter or filter chain this slice does not decode.
    UnsupportedFilter,
    /// A `/DecodeParms` entry is declared. This slice does not decode with
    /// declared parameters; decoding with defaults could silently produce a
    /// plausible-but-wrong header, so the declared entry is a gap.
    DecodeParmsDeclared,
    /// The `/DecodeParms` value is an array shape this single-filter slice does
    /// not resolve to effective parameters.
    UnsupportedDecodeParms,
    /// `/DecodeParms` resolution failed on malformed structure.
    DecodeParmsMalformed,
    /// The bounded decode exceeded the caller-supplied decoded-byte cap.
    DecodeOutputLimitExceeded,
    /// The compressed profile bytes could not be inflated.
    FlateDecodeFailed,
}

/// Locally borrowed (uncompressed) or owned (decoded) profile bytes.
enum ProfileBytes<'input> {
    Borrowed(&'input [u8]),
    Owned(Vec<u8>),
}

impl ProfileBytes<'_> {
    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Borrowed(bytes) => bytes,
            Self::Owned(bytes) => bytes,
        }
    }
}

/// Inspect an `ICCBased` profile stream and parse its ICC header, bounded.
///
/// This composes existing stream machinery and adds no new decode path:
///
/// 1. [`resolve_xref_object_offset`] resolves the profile reference to an in-use
///    uncompressed object byte offset. A compressed, reserved, free, or
///    otherwise unresolvable reference becomes a structured
///    [`IccProfileInspectionGap`]; stream objects cannot be object-stream
///    members, so the offset-only resolver is sufficient.
/// 2. [`inspect_content_stream_data_extent_with_lookup`] locates the stream data
///    and [`content_stream_data_slice`] borrows it.
/// 3. [`classify_content_stream_filter`] classifies the filter. Only the
///    identity path and a single `/FlateDecode` are decoded here.
/// 4. For `/FlateDecode`, [`resolve_flate_decode_parameters`] must resolve
///    without a declared `/DecodeParms` key; a declared key is a gap rather than
///    a silent default decode. [`decode_flate_stream`] then inflates under
///    `output_limit`, so an oversized profile is an output-limit gap, never an
///    unbounded allocation.
/// 5. [`parse_icc_profile_header`] reads header facts from the decoded bytes.
///
/// The decoded buffer is dropped as soon as the header facts are extracted. No
/// step ever panics; every failure is an [`IccProfileStreamInspection`] value.
#[must_use]
pub fn inspect_icc_profile_stream_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reference: IndirectRef,
    output_limit: usize,
) -> IccProfileStreamInspection {
    let resolved = match resolve_xref_object_offset(input, lookup, reference) {
        Ok(resolved) => resolved,
        Err(error) => {
            let reason = match error.reason {
                ObjectResolutionRejection::UnsupportedCompressedXrefStreamEntry { .. } => {
                    IccProfileInspectionGap::ProfileObjectCompressed
                }
                _ => IccProfileInspectionGap::ProfileObjectUnresolved,
            };
            return IccProfileStreamInspection::Gap { reason };
        }
    };
    let object_offset = resolved.object_byte_offset;

    let Ok(extent) =
        inspect_content_stream_data_extent_with_lookup(input, Some(lookup), object_offset)
    else {
        return IccProfileStreamInspection::Gap {
            reason: IccProfileInspectionGap::StreamExtent,
        };
    };

    let Ok(stream_data) = content_stream_data_slice(input, &extent) else {
        return IccProfileStreamInspection::Gap {
            reason: IccProfileInspectionGap::StreamSlice,
        };
    };

    let decoded = match decode_profile_bytes(input, object_offset, stream_data, output_limit) {
        Ok(decoded) => decoded,
        Err(reason) => return IccProfileStreamInspection::Gap { reason },
    };

    match parse_icc_profile_header(decoded.as_slice()) {
        IccProfileHeaderParse::Parsed { descriptor } => {
            IccProfileStreamInspection::Parsed { descriptor }
        }
        IccProfileHeaderParse::Truncated { decoded_len } => {
            IccProfileStreamInspection::Truncated { decoded_len }
        }
    }
}

/// Locate the filter classification and return the raw or `/FlateDecode`-decoded
/// profile bytes, or the structured gap that prevented decoding.
fn decode_profile_bytes<'input>(
    input: &'input [u8],
    object_offset: usize,
    stream_data: &'input [u8],
    output_limit: usize,
) -> Result<ProfileBytes<'input>, IccProfileInspectionGap> {
    match classify_content_stream_filter(input, object_offset) {
        Ok(ContentStreamFilterClassification::Uncompressed) => {
            Ok(ProfileBytes::Borrowed(stream_data))
        }
        Ok(ContentStreamFilterClassification::Flate) => {
            decode_flate_profile(input, object_offset, stream_data, output_limit)
        }
        Ok(
            ContentStreamFilterClassification::UnsupportedFilter { .. }
            | ContentStreamFilterClassification::UnsupportedFilterChain { .. },
        ) => Err(IccProfileInspectionGap::UnsupportedFilter),
        Err(_) => Err(IccProfileInspectionGap::FilterClassification),
    }
}

/// Decode a single `/FlateDecode` profile stream under `output_limit`, refusing
/// any declared `/DecodeParms` to avoid a silent default-parameter decode.
fn decode_flate_profile<'input>(
    input: &[u8],
    object_offset: usize,
    stream_data: &'input [u8],
    output_limit: usize,
) -> Result<ProfileBytes<'input>, IccProfileInspectionGap> {
    let resolution = resolve_flate_decode_parameters(input, object_offset)
        .map_err(|_| IccProfileInspectionGap::DecodeParmsMalformed)?;
    let parameters = match resolution {
        FlateDecodeParametersResolution::Resolved {
            parameters,
            decode_parms_key_range,
            ..
        } => {
            if decode_parms_key_range.is_some() {
                return Err(IccProfileInspectionGap::DecodeParmsDeclared);
            }
            parameters
        }
        FlateDecodeParametersResolution::UnsupportedArrayParms { .. } => {
            return Err(IccProfileInspectionGap::UnsupportedDecodeParms);
        }
    };

    decode_flate_stream(stream_data, parameters, output_limit)
        .map(ProfileBytes::Owned)
        .map_err(|error| match error.reason {
            FlateDecodeStreamRejection::OutputLimitExceeded => {
                IccProfileInspectionGap::DecodeOutputLimitExceeded
            }
            _ => IccProfileInspectionGap::FlateDecodeFailed,
        })
}
