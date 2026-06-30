use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefTableInspection, IndirectObjectHeaderInspectionRejection, IndirectRef, ObjectLookup,
    ObjectLookupLocation, inspect_indirect_object_header, locate_xref_object,
};

/// In-use object location resolved from a cross-reference backend.
///
/// This is the backend-neutral success currency of object resolution. A classic
/// xref table produces it today through [`resolve_classic_xref_object_offset`];
/// a future cross-reference-stream backend can produce the same report without
/// changing consumers.
///
/// This report stores only structural metadata. It does not retain or copy PDF
/// bytes, object bodies, stream bodies, dictionaries, or referenced-object bytes,
/// and it does not read the resolved object body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedObject {
    /// Requested indirect reference, proven to match both the cross-reference
    /// entry generation and the indirect object header at the resolved offset.
    pub reference: IndirectRef,
    /// Resolved in-use object byte offset.
    pub object_byte_offset: usize,
    /// Generation number reported by the matching in-use cross-reference entry.
    pub xref_generation: u16,
}

/// Error returned when an indirect reference cannot be resolved to an in-use
/// object byte offset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectResolutionError {
    /// Requested indirect reference.
    pub reference: IndirectRef,
    /// Total source length.
    pub byte_len: usize,
    /// Resolved in-use object byte offset, when cross-reference resolution
    /// reached one before a later check failed.
    pub object_byte_offset: Option<usize>,
    /// Byte offset where delegated object-header inspection found a malformed
    /// construct, when available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: ObjectResolutionRejection,
}

/// Structured object-resolution rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ObjectResolutionRejection {
    /// The cross-reference result was not an uncompressed in-use object entry.
    UnresolvedXrefLocation {
        /// Locate-only cross-reference result for the requested object number.
        location: ObjectLookupLocation,
    },
    /// The cross-reference stream reports the object as compressed inside an
    /// object stream. This resolver does not extract object streams.
    UnsupportedCompressedXrefStreamEntry {
        /// Requested object number.
        object_number: usize,
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Index of this object inside the object stream.
        index_within_object_stream: usize,
    },
    /// The cross-reference stream reports a reserved or future entry type.
    UnsupportedReservedXrefStreamEntry {
        /// Requested object number.
        object_number: usize,
        /// Raw type field value.
        entry_type: u64,
        /// Raw second field value.
        field2: u64,
        /// Raw third field value.
        field3: u64,
    },
    /// The in-use cross-reference entry generation did not match the requested
    /// reference generation.
    GenerationMismatch {
        /// Generation number from the requested indirect reference.
        requested_generation: u16,
        /// Generation number from the matching in-use cross-reference entry.
        xref_generation: u16,
    },
    /// The indirect object header at the resolved byte offset could not be
    /// parsed.
    ObjectHeader {
        /// Underlying object-header rejection reason.
        header_reason: IndirectObjectHeaderInspectionRejection,
    },
    /// The indirect object header parsed but its object/generation did not match
    /// the requested reference.
    ObjectHeaderReferenceMismatch {
        /// Indirect reference parsed from the object header at the resolved
        /// offset.
        header_reference: IndirectRef,
    },
}

/// Resolve an indirect reference to an in-use object byte offset through a
/// parsed classic xref table.
///
/// The resolution accepts the reference only when every check holds:
///
/// - [`resolve_classic_xref_object`] reports exactly one in-use entry for the
///   object number (free, not-found, and ambiguous results are rejected);
/// - the in-use entry generation matches the requested reference generation;
/// - the indirect object header at the resolved byte offset parses and its
///   object number and generation match the requested reference.
///
/// The generation is therefore validated twice: once against the cross-reference
/// entry and once against the object header at the resolved offset.
///
/// It performs no `/Prev` traversal, incremental-section merging, object-stream
/// extraction, object-body reading, caching, or object-map construction; it only
/// scans the already-parsed xref table and reads the short header at the resolved
/// offset.
///
/// # Errors
///
/// Returns [`ObjectResolutionError`] when the cross-reference result is not a
/// single in-use entry, the entry generation does not match, the object header
/// fails to parse, or the parsed header reference does not match the requested
/// reference.
pub fn resolve_classic_xref_object_offset(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    reference: IndirectRef,
) -> Result<ResolvedObject, ObjectResolutionError> {
    resolve_xref_object_offset(input, ObjectLookup::ClassicXref(xref), reference)
}

/// Resolve an indirect reference to an in-use object byte offset through a
/// borrowed xref backend.
///
/// Classic and cross-reference-stream type-1 entries share the same success
/// currency and the same object-header validation. Cross-reference-stream
/// type-2 compressed entries and reserved/future entry types are reported as
/// structured unsupported paths and are never treated as not found or fabricated
/// into byte offsets.
///
/// # Errors
///
/// Returns [`ObjectResolutionError`] when lookup does not produce an
/// uncompressed in-use object entry, the xref generation does not match, the
/// object header fails to parse, or the parsed header reference does not match
/// the requested reference.
pub fn resolve_xref_object_offset(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reference: IndirectRef,
) -> Result<ResolvedObject, ObjectResolutionError> {
    let (xref_generation, object_byte_offset) = resolve_lookup_location(input, lookup, reference)?;

    if xref_generation != reference.generation {
        return Err(object_resolution_error(
            input,
            reference,
            Some(object_byte_offset),
            None,
            ObjectResolutionRejection::GenerationMismatch {
                requested_generation: reference.generation,
                xref_generation,
            },
        ));
    }

    let header = inspect_indirect_object_header(input, object_byte_offset).map_err(|error| {
        object_resolution_error(
            input,
            reference,
            Some(object_byte_offset),
            error.error_byte_offset,
            ObjectResolutionRejection::ObjectHeader {
                header_reason: error.reason,
            },
        )
    })?;

    if header.reference != reference {
        return Err(object_resolution_error(
            input,
            reference,
            Some(object_byte_offset),
            Some(header.header_range.start),
            ObjectResolutionRejection::ObjectHeaderReferenceMismatch {
                header_reference: header.reference,
            },
        ));
    }

    Ok(ResolvedObject {
        reference,
        object_byte_offset,
        xref_generation,
    })
}

fn resolve_lookup_location(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reference: IndirectRef,
) -> Result<(u16, usize), ObjectResolutionError> {
    let location = locate_xref_object(
        lookup,
        usize::try_from(reference.object_number).map_or(usize::MAX, |value| value),
    );
    match location {
        ObjectLookupLocation::ClassicInUse {
            generation,
            byte_offset,
            ..
        }
        | ObjectLookupLocation::XrefStreamUncompressed {
            generation,
            byte_offset,
            ..
        } => Ok((generation, byte_offset)),
        ObjectLookupLocation::XrefStreamCompressed {
            object_number,
            object_stream_number,
            index_within_object_stream,
        } => Err(object_resolution_error(
            input,
            reference,
            None,
            None,
            ObjectResolutionRejection::UnsupportedCompressedXrefStreamEntry {
                object_number,
                object_stream_number,
                index_within_object_stream,
            },
        )),
        ObjectLookupLocation::XrefStreamReserved {
            object_number,
            entry_type,
            field2,
            field3,
        } => Err(object_resolution_error(
            input,
            reference,
            None,
            None,
            ObjectResolutionRejection::UnsupportedReservedXrefStreamEntry {
                object_number,
                entry_type,
                field2,
                field3,
            },
        )),
        _ => Err(object_resolution_error(
            input,
            reference,
            None,
            None,
            ObjectResolutionRejection::UnresolvedXrefLocation { location },
        )),
    }
}

const fn object_resolution_error(
    input: &[u8],
    reference: IndirectRef,
    object_byte_offset: Option<usize>,
    error_byte_offset: Option<usize>,
    reason: ObjectResolutionRejection,
) -> ObjectResolutionError {
    ObjectResolutionError {
        reference,
        byte_len: input.len(),
        object_byte_offset,
        error_byte_offset,
        reason,
    }
}
