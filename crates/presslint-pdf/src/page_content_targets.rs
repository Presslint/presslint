use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefObjectLocation, ClassicXrefTableInspection, ObjectLookup, ObjectLookupLocation,
    PageContentReference, PageContentsInspection, locate_xref_object,
};

/// Locate-only resolution report for a page object's direct `/Contents`
/// references.
///
/// This report stores only the caller-visible source length and one
/// source-ordered result per direct content reference reported by
/// [`crate::inspect_page_contents`]. It does not retain or copy PDF bytes,
/// object bodies, stream bodies, decoded streams, or source slices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageContentTargetsInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Source-ordered target resolution entries.
    pub entries: Vec<PageContentTargetInspection>,
}

/// Locate-only resolution result for one direct page-content reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PageContentTargetInspection {
    /// The content reference resolved to exactly one matching in-use xref entry.
    Resolved {
        /// Original direct `/Contents` reference reported by page inspection.
        content_reference: PageContentReference,
        /// In-use object byte offset from the matching cross-reference entry.
        object_byte_offset: usize,
        /// Generation number reported by the matching cross-reference entry.
        xref_generation: u16,
    },
    /// The content reference was intentionally skipped.
    Skipped {
        /// Original direct `/Contents` reference reported by page inspection.
        content_reference: PageContentReference,
        /// Structured skip reason.
        reason: SkippedPageContentTargetReason,
    },
}

/// Structured reason why one direct page-content reference was not resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedPageContentTargetReason {
    /// The classic xref result was free, missing, or ambiguous rather than
    /// exactly one in-use entry.
    ///
    /// This variant is preserved verbatim on the classic backend so the classic
    /// helper keeps producing byte-identical structured skips.
    UnresolvedXrefLocation {
        /// Locate-only classic xref result for the requested object number.
        location: ClassicXrefObjectLocation,
    },
    /// The backend lookup result was not a single in-use/uncompressed entry that
    /// can be turned into an object byte offset (for example a free, not-found,
    /// out-of-range, compressed, or reserved cross-reference-stream entry).
    ///
    /// This is the backend-neutral counterpart to
    /// [`Self::UnresolvedXrefLocation`] produced on the cross-reference-stream
    /// backend.
    UnresolvedLookupLocation {
        /// Backend-neutral locate-only result for the requested object number.
        location: ObjectLookupLocation,
    },
    /// The cross-reference entry generation did not match the requested content
    /// reference generation.
    GenerationMismatch {
        /// Generation number from the requested indirect reference.
        requested_generation: u16,
        /// Generation number from the matching in-use cross-reference entry.
        xref_generation: u16,
        /// In-use object byte offset from the generation-mismatched entry.
        object_byte_offset: usize,
    },
}

/// Resolve direct page `/Contents` references through an existing classic xref
/// inspection.
///
/// This is a thin classic wrapper over
/// [`inspect_page_content_targets_with_lookup`]: it delegates through
/// [`ObjectLookup::ClassicXref`] and therefore keeps the resolved targets, the
/// matching-generation check, and every structured skip
/// ([`SkippedPageContentTargetReason::UnresolvedXrefLocation`] and
/// [`SkippedPageContentTargetReason::GenerationMismatch`]) byte-identical to the
/// pre-`_with_lookup` behavior.
///
/// It does not inspect content stream dictionaries, locate `stream` or
/// `endstream`, decode streams, concatenate stream bytes, tokenize content
/// bytes, mutate PDF bytes, follow `/Prev`, or build a cache/index around the
/// xref table.
#[must_use]
pub fn inspect_page_content_targets(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    page_contents: &PageContentsInspection,
) -> PageContentTargetsInspection {
    inspect_page_content_targets_with_lookup(input, ObjectLookup::ClassicXref(xref), page_contents)
}

/// Resolve direct page `/Contents` references through any [`ObjectLookup`]
/// backend.
///
/// The helper performs a deterministic locate-only pass over
/// [`PageContentsInspection::contents`]. Each content object number is located
/// through [`locate_xref_object`], and only a single in-use classic entry or a
/// single uncompressed cross-reference-stream entry whose generation matches the
/// requested reference is reported as resolved. The classic locate result is
/// mapped back into the verbatim
/// [`SkippedPageContentTargetReason::UnresolvedXrefLocation`] variant so the
/// classic backend stays byte-identical; every cross-reference-stream
/// non-resolvable entry (free, not found, out-of-range, compressed, or reserved)
/// surfaces through
/// [`SkippedPageContentTargetReason::UnresolvedLookupLocation`] and is never
/// fabricated into a byte offset. Generation-mismatched in-use entries become
/// source-ordered structured skips so later references are still processed.
///
/// It does not inspect content stream dictionaries, locate `stream` or
/// `endstream`, decode streams, concatenate stream bytes, tokenize content
/// bytes, mutate PDF bytes, follow `/Prev`, extract object streams, or build a
/// cache/index around the backend.
#[must_use]
pub fn inspect_page_content_targets_with_lookup(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page_contents: &PageContentsInspection,
) -> PageContentTargetsInspection {
    let entries = page_contents
        .contents
        .iter()
        .copied()
        .map(|content_reference| resolve_content_reference(lookup, content_reference))
        .collect();

    PageContentTargetsInspection {
        byte_len: input.len(),
        entries,
    }
}

fn resolve_content_reference(
    lookup: ObjectLookup<'_>,
    content_reference: PageContentReference,
) -> PageContentTargetInspection {
    let location = locate_xref_object(
        lookup,
        usize::try_from(content_reference.reference.object_number)
            .map_or(usize::MAX, |value| value),
    );
    let Some((xref_generation, object_byte_offset)) = in_use_offset(location) else {
        return PageContentTargetInspection::Skipped {
            content_reference,
            reason: unresolved_lookup_rejection(location),
        };
    };

    if xref_generation != content_reference.reference.generation {
        return PageContentTargetInspection::Skipped {
            content_reference,
            reason: SkippedPageContentTargetReason::GenerationMismatch {
                requested_generation: content_reference.reference.generation,
                xref_generation,
                object_byte_offset,
            },
        };
    }

    PageContentTargetInspection::Resolved {
        content_reference,
        object_byte_offset,
        xref_generation,
    }
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

/// Map an unresolved locate result into a structured skip reason.
///
/// Classic locate results are mapped back into the verbatim
/// [`ClassicXrefObjectLocation`]-carrying variant so the classic backend stays
/// byte-identical; every cross-reference-stream result keeps the backend-neutral
/// [`ObjectLookupLocation`].
fn unresolved_lookup_rejection(location: ObjectLookupLocation) -> SkippedPageContentTargetReason {
    match location {
        ObjectLookupLocation::ClassicFree {
            object_number,
            generation,
            next_free_object_number,
        } => SkippedPageContentTargetReason::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::Free {
                object_number: classic_object_number(object_number),
                generation,
                next_free_object_number,
            },
        },
        ObjectLookupLocation::ClassicNotFound { object_number } => {
            SkippedPageContentTargetReason::UnresolvedXrefLocation {
                location: ClassicXrefObjectLocation::NotFound {
                    object_number: classic_object_number(object_number),
                },
            }
        }
        ObjectLookupLocation::ClassicAmbiguous {
            object_number,
            first,
            second,
        } => SkippedPageContentTargetReason::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::Ambiguous {
                object_number: classic_object_number(object_number),
                first,
                second,
            },
        },
        other => SkippedPageContentTargetReason::UnresolvedLookupLocation { location: other },
    }
}

/// Narrow a backend-reported object number back to the classic `u32` contract.
///
/// Classic locate results originate from `u32` object numbers, so this never
/// truncates in practice; the saturating fallback keeps the conversion total.
fn classic_object_number(object_number: usize) -> u32 {
    u32::try_from(object_number).unwrap_or(u32::MAX)
}
