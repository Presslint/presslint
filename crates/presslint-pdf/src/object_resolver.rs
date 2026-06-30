use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefObjectLocation, ClassicXrefTableInspection, IndirectObjectHeaderInspectionRejection,
    IndirectRef, inspect_indirect_object_header, resolve_classic_xref_object,
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
    /// The cross-reference result was not exactly one in-use entry.
    UnresolvedXrefLocation {
        /// Locate-only cross-reference result for the requested object number.
        location: ClassicXrefObjectLocation,
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
    let location = resolve_classic_xref_object(xref, reference.object_number);
    let ClassicXrefObjectLocation::InUse {
        generation: xref_generation,
        byte_offset: object_byte_offset,
        ..
    } = location
    else {
        return Err(object_resolution_error(
            input,
            reference,
            None,
            None,
            ObjectResolutionRejection::UnresolvedXrefLocation { location },
        ));
    };

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
