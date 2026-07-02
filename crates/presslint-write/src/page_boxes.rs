//! Semantic `/MediaBox` / `/CropBox` incremental writing over leaf page objects.
//!
//! This is the first *semantic* byte mutation built on the append-only classic
//! incremental writer: it sets `/MediaBox` and/or `/CropBox` on selected
//! uncompressed leaf page dictionaries and appends exactly one classic
//! incremental revision through [`crate::write_incremental_revision`].
//!
//! Structural facts — leaf references, box provenance, and per-page skip reasons
//! — are read through [`presslint_pdf::inspect_document_page_boxes`]; ownership is
//! decided with [`presslint_pdf::decide_indirect_object_edit`]. Only leaf objects
//! with a single proven page-tree owner are rewritten. Inherited or defaulted
//! boxes become explicit direct entries on the edited leaf; ancestor `/Pages`
//! dictionaries are never mutated.

use presslint_actions::{
    IncrementalRevisionPlan, PlannedDirtyObject, SetPageBox, plan_set_page_box_boundaries,
};
use presslint_pdf::{
    DictionaryValueKind, DocumentPageBoxesInspection, IndirectObjectEditDecision,
    IndirectObjectEditDisposition, IndirectRef, PageBoxInspectionError, PageBoxKind,
    PageBoxesInspection, PageRectangle, ResolvedObjectPosition, SkippedPageBox,
    SkippedPageBoxReason, decide_indirect_object_edit, inspect_document_page_boxes,
    inspect_indirect_object_dictionary, parse_indirect_reference,
};
use presslint_types::ByteRange;
use serde::{Deserialize, Serialize};

use crate::page_box_serialize::{apply_body_edits, push_box_edit};
use crate::{
    PlannedWriteError, WriteError, write_incremental_revision, write_incremental_revision_plan,
};

const PARENT_KEY: &[u8] = b"/Parent";

/// Request to set page boxes on one or more leaf pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetPageBoxesRequest {
    /// Per-page box edits. Each page index must be unique.
    pub pages: Vec<PageBoxEdit>,
}

/// One page's requested box edit.
///
/// `media_box` and `crop_box` are request-only rectangles. At least one must be
/// set; a rectangle is normalized (lower-left / upper-right ordered) before it is
/// written, and rejected when non-finite or zero-area.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PageBoxEdit {
    /// Zero-based document page index to edit.
    pub page_index: usize,
    /// Requested `/MediaBox`, when set.
    pub media_box: Option<PageRectangle>,
    /// Requested `/CropBox`, when set.
    pub crop_box: Option<PageRectangle>,
}

/// Output of a successful [`set_page_boxes_incremental`] call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SetPageBoxesOutput {
    /// The new PDF bytes: `input` verbatim plus one appended classic revision.
    pub bytes: Vec<u8>,
    /// Pages that were edited, in document order.
    pub edited: Vec<EditedPage>,
    /// Requested pages that were skipped, with structured reasons.
    pub skipped: Vec<SkippedPageEdit>,
}

/// Report for one edited leaf page.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EditedPage {
    /// Zero-based document page index.
    pub page_index: usize,
    /// Indirect reference of the rewritten leaf page.
    pub leaf_reference: IndirectRef,
    /// Applied `/MediaBox`, when requested.
    pub media_box: Option<AppliedBox>,
    /// Applied `/CropBox`, when requested.
    pub crop_box: Option<AppliedBox>,
}

/// One applied page box and how it was written into the leaf dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AppliedBox {
    /// Box kind that was written.
    pub kind: PageBoxKind,
    /// Normalized rectangle that was serialized.
    pub rectangle: PageRectangle,
    /// Whether an existing direct entry was replaced or a new entry inserted.
    pub op: DictionaryEntryWrite,
}

/// Whether a page-box entry was replaced in place or newly inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryEntryWrite {
    /// A direct leaf entry was replaced by its source range.
    Replace,
    /// An absent or inherited entry was inserted after the leaf `<<`.
    Insert,
}

/// One requested page skipped before any byte writing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPageEdit {
    /// Requested zero-based document page index.
    pub page_index: usize,
    /// Leaf reference when the page was located.
    pub leaf_reference: Option<IndirectRef>,
    /// Structured skip reason.
    pub reason: SetPageBoxSkipReason,
}

/// Structured reason a requested page was not edited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SetPageBoxSkipReason {
    /// No leaf page with the requested index was enumerated.
    PageNotFound,
    /// The leaf page dictionary lives in an object stream and has no source byte
    /// offsets to rewrite.
    CompressedLeafDictionary {
        /// Object stream containing the compressed page object.
        object_stream_number: usize,
        /// Member index within the object stream.
        index_within_object_stream: usize,
    },
    /// Page-tree ownership of the leaf could not be proven single-use, so the
    /// object must not be mutated in place.
    OwnershipNotProven {
        /// How many document page slots referenced this leaf object.
        occurrences: usize,
        /// Disposition returned by the ownership decision.
        disposition: IndirectObjectEditDisposition,
    },
    /// A relevant box key appeared more than once in the leaf dictionary.
    DuplicateBoxKey {
        /// Box whose key was duplicated.
        kind: PageBoxKind,
    },
    /// A relevant box value was not a direct array (for example an indirect
    /// reference).
    UnsupportedBoxValue {
        /// Box whose value was unsupported.
        kind: PageBoxKind,
        /// Shallow value kind reported by dictionary inspection.
        value_kind: DictionaryValueKind,
    },
    /// A relevant box value was a malformed rectangle array.
    MalformedBoxValue {
        /// Box whose value was malformed.
        kind: PageBoxKind,
    },
    /// The leaf had no effective `/MediaBox` anywhere in the page tree.
    MissingEffectiveMediaBox,
    /// The leaf page object could not be resolved or its dictionary read.
    LeafUnreadable,
}

/// Error returned when a page-box incremental write cannot be produced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum SetPageBoxesError {
    /// Document-level page-box inspection failed.
    Inspect {
        /// Delegated inspection failure.
        error: Box<PageBoxInspectionError>,
    },
    /// The append writer rejected the input or the assembled revision. Used for
    /// the all-skipped case, which delegates to the append writer directly rather
    /// than through the plan bridge (an empty plan is a plan-layer rejection).
    Write {
        /// Delegated append-writer failure.
        error: Box<WriteError>,
    },
    /// The plan bridge rejected the assembled incremental-revision plan.
    Plan {
        /// Delegated plan-bridge failure.
        error: Box<PlannedWriteError>,
    },
    /// The same page index was requested more than once.
    DuplicatePageIndex {
        /// The repeated page index.
        page_index: usize,
    },
    /// A page edit requested neither `/MediaBox` nor `/CropBox`.
    NoBoxesRequested {
        /// The empty page edit's index.
        page_index: usize,
    },
    /// A requested rectangle carried a non-finite coordinate.
    NonFiniteRectangle {
        /// Page index of the offending edit.
        page_index: usize,
        /// Box whose rectangle was non-finite.
        kind: PageBoxKind,
    },
    /// A requested rectangle had zero width or height after normalization.
    ZeroAreaRectangle {
        /// Page index of the offending edit.
        page_index: usize,
        /// Box whose rectangle was zero-area.
        kind: PageBoxKind,
    },
    /// A requested `/CropBox` was not contained by the effective or requested
    /// `/MediaBox`. The writer never auto-intersects.
    CropOutsideMedia {
        /// Page index of the offending edit.
        page_index: usize,
        /// Normalized requested crop rectangle.
        crop: PageRectangle,
        /// Effective or requested media rectangle the crop must fit inside.
        media: PageRectangle,
    },
}

/// Set `/MediaBox` and/or `/CropBox` on selected leaf pages and append one
/// classic incremental revision.
///
/// The output preserves `input` verbatim as its prefix
/// (`output.bytes[..input.len()] == input`) and appends exactly one classic
/// incremental revision that rewrites only the edited leaf page objects. Every
/// unrelated byte inside each rewritten leaf dictionary body is preserved.
///
/// Requested rectangles are normalized by ordering lower-left and upper-right
/// coordinates. A non-finite or zero-area requested rectangle, or a `/CropBox`
/// outside the effective/requested `/MediaBox`, is a hard error. Per-page
/// document conditions (compressed leaf, unproven ownership, duplicate/malformed/
/// indirect box entries, missing page) are reported as structured skips and the
/// remaining editable pages are still written.
///
/// # Errors
///
/// Returns [`SetPageBoxesError`] when page-box inspection fails, the request is
/// malformed (duplicate page index or empty page edit), a requested rectangle is
/// non-finite/zero-area or a crop falls outside the media, or the append writer
/// rejects the input.
pub fn set_page_boxes_incremental(
    input: &[u8],
    request: &SetPageBoxesRequest,
) -> Result<SetPageBoxesOutput, SetPageBoxesError> {
    validate_request(request)?;

    let inspection =
        inspect_document_page_boxes(input).map_err(|error| SetPageBoxesError::Inspect {
            error: Box::new(error),
        })?;

    let mut edited = Vec::new();
    let mut skipped = Vec::new();
    let mut dirty_objects: Vec<PlannedDirtyObject> = Vec::new();

    for edit in &request.pages {
        match plan_page_edit(input, &inspection, edit)? {
            PagePlan::Edit { report, planned } => {
                edited.push(report);
                dirty_objects.push(planned);
            }
            PagePlan::Skip(skip) => skipped.push(skip),
        }
    }

    // Route proven leaf edits through the backend-agnostic plan bridge. When
    // every page was skipped the plan is empty, which the bridge rejects by
    // contract, so delegate the no-op revision straight to the append writer.
    let bytes = if dirty_objects.is_empty() {
        write_incremental_revision(input, &[]).map_err(|error| SetPageBoxesError::Write {
            error: Box::new(error),
        })?
    } else {
        let plan = IncrementalRevisionPlan { dirty_objects };
        write_incremental_revision_plan(input, &plan).map_err(|error| SetPageBoxesError::Plan {
            error: Box::new(error),
        })?
    };

    // Reports are collected in request order above; the public contract is
    // document order, so sort by document page index before returning. Page
    // indexes are unique (validated) and the sort is independent of `dirty`
    // ordering, which the append writer keys by object.
    edited.sort_by_key(|report| report.page_index);

    Ok(SetPageBoxesOutput {
        bytes,
        edited,
        skipped,
    })
}

/// Validate request-level invariants that do not depend on the document.
fn validate_request(request: &SetPageBoxesRequest) -> Result<(), SetPageBoxesError> {
    let mut seen = Vec::new();
    for edit in &request.pages {
        if seen.contains(&edit.page_index) {
            return Err(SetPageBoxesError::DuplicatePageIndex {
                page_index: edit.page_index,
            });
        }
        seen.push(edit.page_index);
        if edit.media_box.is_none() && edit.crop_box.is_none() {
            return Err(SetPageBoxesError::NoBoxesRequested {
                page_index: edit.page_index,
            });
        }
    }
    Ok(())
}

/// Outcome of planning one requested page edit.
enum PagePlan {
    Edit {
        report: EditedPage,
        planned: PlannedDirtyObject,
    },
    Skip(SkippedPageEdit),
}

fn plan_page_edit(
    input: &[u8],
    inspection: &DocumentPageBoxesInspection,
    edit: &PageBoxEdit,
) -> Result<PagePlan, SetPageBoxesError> {
    let Some(page) = inspection
        .pages
        .iter()
        .find(|page| page.page_index == edit.page_index)
    else {
        return Ok(PagePlan::Skip(skip_from_inspection(inspection, edit)));
    };

    // Normalize and validate the requested rectangles up front.
    let media = normalize_option(edit.media_box, edit.page_index, PageBoxKind::MediaBox)?;
    let crop = normalize_option(edit.crop_box, edit.page_index, PageBoxKind::CropBox)?;

    if let Some(crop_rect) = crop {
        let media_bound = media.unwrap_or(page.media_box.effective);
        if !contains(media_bound, crop_rect) {
            return Err(SetPageBoxesError::CropOutsideMedia {
                page_index: edit.page_index,
                crop: crop_rect,
                media: media_bound,
            });
        }
    }

    // Prove single-use page-tree ownership before rewriting the leaf object.
    let occurrences = inspection
        .pages
        .iter()
        .filter(|other| other.leaf_reference == page.leaf_reference)
        .count();
    let parent = read_parent(input, page);
    let decision = decide_indirect_object_edit(page.leaf_reference, parent);
    if occurrences != 1 || decision.disposition != IndirectObjectEditDisposition::InPlaceMutation {
        return Ok(PagePlan::Skip(SkippedPageEdit {
            page_index: edit.page_index,
            leaf_reference: Some(page.leaf_reference),
            reason: SetPageBoxSkipReason::OwnershipNotProven {
                occurrences,
                disposition: decision.disposition,
            },
        }));
    }

    Ok(build_leaf_edit(input, page, edit, media, crop, &decision))
}

/// Normalize an optional requested rectangle, rejecting non-finite/zero-area.
fn normalize_option(
    rectangle: Option<PageRectangle>,
    page_index: usize,
    kind: PageBoxKind,
) -> Result<Option<PageRectangle>, SetPageBoxesError> {
    rectangle
        .map(|rectangle| normalize(rectangle, page_index, kind))
        .transpose()
}

/// Order lower-left / upper-right coordinates and reject degenerate rectangles.
///
/// Exact float equality is intentional: a zero-area rectangle is one whose
/// normalized coordinates coincide exactly, and we reject it rather than
/// silently writing a degenerate box.
#[allow(clippy::float_cmp)]
fn normalize(
    rectangle: PageRectangle,
    page_index: usize,
    kind: PageBoxKind,
) -> Result<PageRectangle, SetPageBoxesError> {
    if ![rectangle.llx, rectangle.lly, rectangle.urx, rectangle.ury]
        .iter()
        .all(|value| value.is_finite())
    {
        return Err(SetPageBoxesError::NonFiniteRectangle { page_index, kind });
    }
    let normalized = PageRectangle {
        llx: rectangle.llx.min(rectangle.urx),
        lly: rectangle.lly.min(rectangle.ury),
        urx: rectangle.llx.max(rectangle.urx),
        ury: rectangle.lly.max(rectangle.ury),
    };
    if normalized.llx == normalized.urx || normalized.lly == normalized.ury {
        return Err(SetPageBoxesError::ZeroAreaRectangle { page_index, kind });
    }
    Ok(normalized)
}

/// True when `inner` is fully contained by `outer` (edges may touch).
fn contains(outer: PageRectangle, inner: PageRectangle) -> bool {
    inner.llx >= outer.llx
        && inner.lly >= outer.lly
        && inner.urx <= outer.urx
        && inner.ury <= outer.ury
}

/// Map an absent leaf page to the inspector's structured skip, if it recorded
/// one for this page index, otherwise `PageNotFound`.
fn skip_from_inspection(
    inspection: &DocumentPageBoxesInspection,
    edit: &PageBoxEdit,
) -> SkippedPageEdit {
    let matched = inspection
        .skipped
        .iter()
        .find(|skip| skip.page_index == Some(edit.page_index));
    let (leaf_reference, reason) = matched.map_or_else(
        || (None, SetPageBoxSkipReason::PageNotFound),
        |skip| (skip.leaf_reference, map_inspector_skip(skip)),
    );
    SkippedPageEdit {
        page_index: edit.page_index,
        leaf_reference,
        reason,
    }
}

/// Translate an inspector page-box skip into a set-page-box skip reason.
fn map_inspector_skip(skip: &SkippedPageBox) -> SetPageBoxSkipReason {
    match &skip.reason {
        SkippedPageBoxReason::CompressedLeafDictionary {
            object_stream_number,
            index_within_object_stream,
        } => SetPageBoxSkipReason::CompressedLeafDictionary {
            object_stream_number: *object_stream_number,
            index_within_object_stream: *index_within_object_stream,
        },
        SkippedPageBoxReason::DuplicateKey { .. } => SetPageBoxSkipReason::DuplicateBoxKey {
            kind: skip.kind.unwrap_or(PageBoxKind::MediaBox),
        },
        SkippedPageBoxReason::UnsupportedValueKind { value_kind } => {
            SetPageBoxSkipReason::UnsupportedBoxValue {
                kind: skip.kind.unwrap_or(PageBoxKind::MediaBox),
                value_kind: *value_kind,
            }
        }
        SkippedPageBoxReason::MalformedRectangle => SetPageBoxSkipReason::MalformedBoxValue {
            kind: skip.kind.unwrap_or(PageBoxKind::MediaBox),
        },
        SkippedPageBoxReason::MissingEffectiveMediaBox => {
            SetPageBoxSkipReason::MissingEffectiveMediaBox
        }
        SkippedPageBoxReason::NodeExpansionFailed { .. }
        | SkippedPageBoxReason::ObjectResolution { .. }
        | SkippedPageBoxReason::TraversalTruncated { .. } => SetPageBoxSkipReason::LeafUnreadable,
    }
}

/// Read the leaf's single `/Parent` indirect reference, if well-formed.
///
/// Returns an empty iterator source when `/Parent` is absent, duplicated, or not
/// a plain indirect reference, so the ownership decision stays unproven.
fn read_parent(input: &[u8], page: &PageBoxesInspection) -> Option<IndirectRef> {
    let object_byte_offset = match page.leaf_position {
        ResolvedObjectPosition::Uncompressed {
            object_byte_offset, ..
        } => object_byte_offset,
        ResolvedObjectPosition::Compressed { .. } => return None,
    };
    let dictionary = inspect_indirect_object_dictionary(input, object_byte_offset).ok()?;
    let mut parent = None;
    for entry in &dictionary.entries {
        if input.get(entry.key_range.start..entry.key_range.end) != Some(PARENT_KEY) {
            continue;
        }
        if parent.is_some() {
            // Duplicate /Parent: ownership cannot be proven.
            return None;
        }
        if entry.value_kind != DictionaryValueKind::IndirectReferenceLike {
            return None;
        }
        parent = parse_indirect_reference(input, entry.value_range.start)
            .ok()
            .map(|inspection| inspection.reference);
    }
    parent
}

/// Build the rewritten leaf body, its edit report, and the backend-agnostic
/// dirty-object plan record, or a skip when the leaf dictionary cannot be read
/// for editing.
///
/// Boundary records are built with [`plan_set_page_box_boundaries`] from the
/// same replace/insert decision that drives the byte edits, so the planned
/// mutation intent and the rewritten body always agree. Ownership is passed in
/// already proven `InPlaceMutation` by [`plan_page_edit`].
fn build_leaf_edit(
    input: &[u8],
    page: &PageBoxesInspection,
    edit: &PageBoxEdit,
    media: Option<PageRectangle>,
    crop: Option<PageRectangle>,
    ownership: &IndirectObjectEditDecision,
) -> PagePlan {
    let leaf_unreadable = || {
        PagePlan::Skip(SkippedPageEdit {
            page_index: edit.page_index,
            leaf_reference: Some(page.leaf_reference),
            reason: SetPageBoxSkipReason::LeafUnreadable,
        })
    };
    let ResolvedObjectPosition::Uncompressed {
        object_byte_offset, ..
    } = page.leaf_position
    else {
        return leaf_unreadable();
    };
    let Ok(dictionary) = inspect_indirect_object_dictionary(input, object_byte_offset) else {
        return leaf_unreadable();
    };

    let dict_open = dictionary.dictionary_open_byte_offset;
    let dict_after_close = dictionary.after_dictionary_close_byte_offset;
    let dictionary_range = ByteRange {
        start: dict_open,
        end: dict_after_close,
    };

    let mut edits = Vec::new();
    let (media_applied, media_locator) = media
        .map(|rect| {
            push_box_edit(
                &mut edits,
                PageBoxKind::MediaBox,
                rect,
                &page.media_box.source,
                page.leaf_reference,
                dictionary_range,
            )
        })
        .unzip();
    let (crop_applied, crop_locator) = crop
        .map(|rect| {
            push_box_edit(
                &mut edits,
                PageBoxKind::CropBox,
                rect,
                &page.crop_box.source,
                page.leaf_reference,
                dictionary_range,
            )
        })
        .unzip();

    let mut body = input[dict_open..dict_after_close].to_vec();
    apply_body_edits(&mut body, edits, dict_open);

    // Boundary records reuse the action planner over the same normalized
    // rectangles and located locators; the writer never gates on them beyond the
    // plan-bridge validation.
    let action = SetPageBox {
        media_box: media,
        crop_box: crop,
    };
    let boundaries = plan_set_page_box_boundaries(
        &action,
        page.leaf_reference,
        ownership,
        media_locator,
        crop_locator,
    );

    PagePlan::Edit {
        report: EditedPage {
            page_index: edit.page_index,
            leaf_reference: page.leaf_reference,
            media_box: media_applied,
            crop_box: crop_applied,
        },
        planned: PlannedDirtyObject {
            reference: page.leaf_reference,
            boundaries,
            body_bytes: body,
        },
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp, clippy::expect_used, clippy::unwrap_used)]

    use presslint_pdf::{PageBoxKind, PageRectangle, SkippedPageBox, SkippedPageBoxReason};

    use super::{SetPageBoxSkipReason, map_inspector_skip, normalize};

    #[test]
    fn normalize_orders_and_rejects() {
        let flipped = PageRectangle {
            llx: 612.0,
            lly: 792.0,
            urx: 0.0,
            ury: 0.0,
        };
        let normalized = normalize(flipped, 0, PageBoxKind::MediaBox).expect("orderable");
        assert_eq!(normalized.llx, 0.0);
        assert_eq!(normalized.lly, 0.0);
        assert_eq!(normalized.urx, 612.0);
        assert_eq!(normalized.ury, 792.0);

        let zero_area = PageRectangle {
            llx: 10.0,
            lly: 0.0,
            urx: 10.0,
            ury: 100.0,
        };
        assert!(normalize(zero_area, 3, PageBoxKind::CropBox).is_err());

        let non_finite = PageRectangle {
            llx: 0.0,
            lly: 0.0,
            urx: f64::INFINITY,
            ury: 10.0,
        };
        assert!(normalize(non_finite, 1, PageBoxKind::MediaBox).is_err());
    }

    #[test]
    fn compressed_leaf_maps_to_structured_skip() {
        let skip = SkippedPageBox {
            page_index: Some(2),
            leaf_reference: None,
            leaf_position: None,
            kind: None,
            reason: SkippedPageBoxReason::CompressedLeafDictionary {
                object_stream_number: 9,
                index_within_object_stream: 1,
            },
        };
        assert_eq!(
            map_inspector_skip(&skip),
            SetPageBoxSkipReason::CompressedLeafDictionary {
                object_stream_number: 9,
                index_within_object_stream: 1,
            }
        );
    }
}
