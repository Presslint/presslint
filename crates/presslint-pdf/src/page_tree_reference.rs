use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefObjectLocation, ClassicXrefTableInspection, IndirectRef, ObjectLookup,
    ObjectLookupLocation, PageTreeNodeTypeInspection, PageTreeNodeTypeInspectionRejection,
    locate_xref_object,
};

/// Resolved and classified target of one page-tree indirect reference.
///
/// This report stores only the requested reference, resolved in-use xref
/// metadata, and the delegated node-type inspection. It does not retain or copy
/// PDF bytes, object bodies, stream bodies, page dictionaries, page-tree
/// dictionaries, contents streams, or referenced-object bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeReferenceTargetInspection {
    /// Requested page-tree indirect reference.
    pub reference: IndirectRef,
    /// In-use object byte offset resolved from the cross-reference backend
    /// (a classic in-use entry or a cross-reference-stream uncompressed entry).
    pub object_byte_offset: usize,
    /// Generation number reported by the matching in-use cross-reference entry.
    pub xref_generation: u16,
    /// Delegated classification of the referenced object's `/Type`.
    pub node_type: PageTreeNodeTypeInspection,
}

/// Error returned when a page-tree reference target cannot be resolved and
/// classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeReferenceTargetInspectionError {
    /// Requested page-tree indirect reference.
    pub reference: IndirectRef,
    /// Total source length.
    pub byte_len: usize,
    /// Resolved in-use object byte offset, when xref resolution reached one.
    pub object_byte_offset: Option<usize>,
    /// Byte offset where delegated node-type inspection found a malformed or
    /// unsupported construct, when available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: PageTreeReferenceTargetInspectionRejection,
}

/// Structured page-tree reference target rejection reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageTreeReferenceTargetInspectionRejection {
    /// The classic xref result was not exactly one in-use entry.
    ///
    /// This variant is preserved verbatim on the classic backend so the classic
    /// helper keeps producing byte-identical errors.
    UnresolvedXrefLocation {
        /// Locate-only xref result for the requested object number.
        location: ClassicXrefObjectLocation,
    },
    /// The backend lookup result was not a single in-use/uncompressed entry that
    /// can be turned into an object byte offset (for example a free, not-found,
    /// out-of-range, compressed, or reserved cross-reference-stream entry).
    ///
    /// This is the backend-neutral counterpart to [`Self::UnresolvedXrefLocation`]
    /// produced on the cross-reference-stream backend.
    UnresolvedLookupLocation {
        /// Backend-neutral locate-only result for the requested object number.
        location: ObjectLookupLocation,
    },
    /// The xref entry generation did not match the requested reference
    /// generation.
    GenerationMismatch {
        /// Generation number from the requested indirect reference.
        requested_generation: u16,
        /// Generation number from the matching in-use xref entry.
        xref_generation: u16,
    },
    /// Delegated page-tree node-type inspection failed.
    NodeType {
        /// Underlying page-tree node-type rejection reason.
        node_type_reason: PageTreeNodeTypeInspectionRejection,
    },
}

/// Resolve one page-tree indirect reference through a classic xref inspection
/// and classify the referenced object's `/Type`.
///
/// This is a thin classic wrapper over
/// [`inspect_page_tree_reference_target_with_lookup`]: it delegates through
/// [`ObjectLookup::ClassicXref`] and therefore keeps the classic locate result,
/// the matching generation check, the node-type classification, and every error
/// variant byte-identical to the pre-`_with_lookup` behavior.
///
/// It does not implement page-tree traversal, recurse through `/Kids`, validate
/// `/Count`, inspect page contents/resources/annotations, parse streams, follow
/// `/Prev`, or add any cache/index around the xref table.
///
/// # Errors
///
/// Returns [`PageTreeReferenceTargetInspectionError`] when xref resolution is
/// free, not found, or ambiguous; when the in-use xref generation does not match
/// the requested reference generation; or when delegated node-type inspection
/// fails.
pub fn inspect_page_tree_reference_target(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    reference: IndirectRef,
) -> Result<PageTreeReferenceTargetInspection, PageTreeReferenceTargetInspectionError> {
    inspect_page_tree_reference_target_with_lookup(
        input,
        ObjectLookup::ClassicXref(xref),
        reference,
    )
}

/// Resolve one page-tree indirect reference through any [`ObjectLookup`] backend
/// and classify the referenced object's `/Type`.
///
/// The helper composes existing bounded helpers only: it locates the requested
/// object number through [`locate_xref_object`], accepts only a single in-use
/// classic entry or a single uncompressed cross-reference-stream entry whose
/// generation matches the requested [`IndirectRef`], then delegates object
/// classification to [`crate::inspect_page_tree_node_type`] at the resolved byte
/// offset. The classic locate result is mapped back into the
/// [`PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation`] variant
/// so the classic backend stays byte-identical; every cross-reference-stream
/// non-resolvable entry (free, not found, out-of-range, compressed, or reserved)
/// surfaces through [`PageTreeReferenceTargetInspectionRejection::UnresolvedLookupLocation`]
/// and is never fabricated into a byte offset.
///
/// It does not implement page-tree traversal, recurse through `/Kids`, validate
/// `/Count`, inspect page contents/resources/annotations, parse streams, follow
/// `/Prev`, extract object streams, or add any cache/index around the backend.
///
/// # Errors
///
/// Returns [`PageTreeReferenceTargetInspectionError`] when lookup does not
/// produce a single in-use/uncompressed entry, when the entry generation does
/// not match the requested reference generation, or when delegated node-type
/// inspection fails.
pub fn inspect_page_tree_reference_target_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reference: IndirectRef,
) -> Result<PageTreeReferenceTargetInspection, PageTreeReferenceTargetInspectionError> {
    let location = locate_xref_object(
        lookup,
        usize::try_from(reference.object_number).map_or(usize::MAX, |value| value),
    );
    let Some((xref_generation, object_byte_offset)) = in_use_offset(location) else {
        return Err(page_tree_reference_target_error(
            input,
            reference,
            None,
            None,
            unresolved_lookup_rejection(location),
        ));
    };

    if xref_generation != reference.generation {
        return Err(page_tree_reference_target_error(
            input,
            reference,
            Some(object_byte_offset),
            None,
            PageTreeReferenceTargetInspectionRejection::GenerationMismatch {
                requested_generation: reference.generation,
                xref_generation,
            },
        ));
    }

    let node_type =
        crate::inspect_page_tree_node_type(input, object_byte_offset).map_err(|error| {
            page_tree_reference_target_error(
                input,
                reference,
                Some(object_byte_offset),
                error.error_byte_offset,
                PageTreeReferenceTargetInspectionRejection::NodeType {
                    node_type_reason: error.reason,
                },
            )
        })?;

    Ok(PageTreeReferenceTargetInspection {
        reference,
        object_byte_offset,
        xref_generation,
        node_type,
    })
}

/// Extract the `(generation, byte_offset)` pair of a resolvable in-use entry.
///
/// Only a single classic in-use entry or a single uncompressed
/// cross-reference-stream entry carries a usable object byte offset; every other
/// locate result is unresolved.
const fn in_use_offset(location: ObjectLookupLocation) -> Option<(u16, usize)> {
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
        } => Some((generation, byte_offset)),
        _ => None,
    }
}

/// Map an unresolved locate result into a rejection reason.
///
/// Classic locate results are mapped back into the verbatim
/// [`ClassicXrefObjectLocation`]-carrying variant so the classic backend stays
/// byte-identical; every cross-reference-stream result keeps the backend-neutral
/// [`ObjectLookupLocation`].
fn unresolved_lookup_rejection(
    location: ObjectLookupLocation,
) -> PageTreeReferenceTargetInspectionRejection {
    match location {
        ObjectLookupLocation::ClassicFree {
            object_number,
            generation,
            next_free_object_number,
        } => PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::Free {
                object_number: classic_object_number(object_number),
                generation,
                next_free_object_number,
            },
        },
        ObjectLookupLocation::ClassicNotFound { object_number } => {
            PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
                location: ClassicXrefObjectLocation::NotFound {
                    object_number: classic_object_number(object_number),
                },
            }
        }
        ObjectLookupLocation::ClassicAmbiguous {
            object_number,
            first,
            second,
        } => PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::Ambiguous {
                object_number: classic_object_number(object_number),
                first,
                second,
            },
        },
        other => {
            PageTreeReferenceTargetInspectionRejection::UnresolvedLookupLocation { location: other }
        }
    }
}

/// Narrow a backend-reported object number back to the classic `u32` contract.
///
/// Classic locate results originate from `u32` object numbers, so this never
/// truncates in practice; the saturating fallback keeps the conversion total.
fn classic_object_number(object_number: usize) -> u32 {
    u32::try_from(object_number).unwrap_or(u32::MAX)
}

const fn page_tree_reference_target_error(
    input: &[u8],
    reference: IndirectRef,
    object_byte_offset: Option<usize>,
    error_byte_offset: Option<usize>,
    reason: PageTreeReferenceTargetInspectionRejection,
) -> PageTreeReferenceTargetInspectionError {
    PageTreeReferenceTargetInspectionError {
        reference,
        byte_len: input.len(),
        object_byte_offset,
        error_byte_offset,
        reason,
    }
}
