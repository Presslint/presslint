//! Per-content-stream-object location for the page-content edit pipeline.
//!
//! T136 lifts the write path from single-content-stream pages to MULTI-stream
//! pages by locating and ownership-gating each content-stream OBJECT of a page
//! independently. This module owns the pure LOCATION step: given one leaf page's
//! delegated content-extent result, it produces an ORDERED per-stream outcome
//! list, each either a located, source-addressable stream
//! ([`LocatedContentStream`]) or a structured per-stream skip. The pipeline then
//! decodes / edits / re-encodes each located stream, ownership-gates it, and
//! assembles one incremental revision from the resulting dirty objects.
//!
//! DUPLICATE-DIRTY-OBJECT SAFETY: a page whose `/Contents` names the same
//! content-stream object more than once yields ONE outcome for that object here
//! (its edits are identical — same source bytes, same deterministic edit — so
//! they MERGE), which keeps the object edited and counted once and prevents the
//! plan from ever being handed two dirty objects with the same number.

use std::collections::BTreeSet;

use presslint_pdf::{
    ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionRejection,
    DocumentPageContentExtentInspection, DocumentPageContentExtentResult, IndirectRef,
    ObjectLookupLocation, PageContentExtentInspection, SkippedPageContentTargetReason,
};
use presslint_types::PageIndex;

use crate::content_edit_pipeline::PipelineSkipReason;

/// Whether the pipeline edits every content-stream object of a page or only
/// single-content-stream pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamMode {
    /// Legacy behaviour: a page with more than one content stream is skipped as
    /// a whole with [`PipelineSkipReason::MultipleContentStreams`]. Preserves the
    /// exact single-stream skip taxonomy for callers that have not adopted the
    /// per-stream model.
    SingleOnly,
    /// T136 behaviour: every content-stream object of a page is located and
    /// ownership-gated independently.
    MultiStream,
}

/// One located, source-addressable content-stream object of a page.
pub struct LocatedContentStream<'a> {
    /// Zero-based document page index of the owning leaf page.
    pub page_index: PageIndex,
    /// Zero-based source-order ordinal of this stream within the page's
    /// `/Contents`.
    pub stream_ordinal: usize,
    /// Indirect reference of the located content-stream object.
    pub content_object: IndirectRef,
    /// Resolved in-use byte offset of the content-stream object.
    pub object_byte_offset: usize,
    /// Delegated single-stream data extent report (direct numeric `/Length`).
    pub extent: &'a ContentStreamDataExtentInspection,
}

/// Outcome of locating one content-stream slot of a page, in stream-ordinal
/// order.
pub enum StreamOutcome<'a> {
    /// The stream object was located with a direct numeric `/Length`.
    Located(LocatedContentStream<'a>),
    /// The stream slot cannot be edited; carried through as a per-stream skip.
    Skip {
        /// Zero-based source-order ordinal of the skipped slot.
        stream_ordinal: usize,
        /// Content-stream object reference when the slot resolved to one.
        content_object: Option<IndirectRef>,
        /// Structured skip reason.
        reason: PipelineSkipReason,
    },
}

/// A leaf page's ordered per-stream location plan.
pub enum PageStreamsPlan<'a> {
    /// A single page-level skip with no per-stream detail: a non-inspected leaf,
    /// an empty `/Contents`, or (in [`StreamMode::SingleOnly`]) a page with more
    /// than one content stream.
    PageSkip {
        /// Content-stream object reference when a single one was located.
        content_object: Option<IndirectRef>,
        /// Structured skip reason.
        reason: PipelineSkipReason,
    },
    /// Ordered per-stream outcomes, deduplicated by content object.
    Streams(Vec<StreamOutcome<'a>>),
}

/// Zero-based document page index derived from a page's document ordinal.
pub fn page_index_of(page: &DocumentPageContentExtentInspection) -> PageIndex {
    PageIndex(u32::try_from(page.ordinal).unwrap_or(u32::MAX))
}

/// Locate the ordered content-stream objects of one leaf page.
///
/// In [`StreamMode::SingleOnly`] a page with more than one content stream (or any
/// non-reference `/Contents` array member) is a whole-page
/// [`PipelineSkipReason::MultipleContentStreams`] skip, exactly preserving the
/// pre-T136 single-stream taxonomy. In [`StreamMode::MultiStream`] every resolved
/// `/Contents` reference becomes one ordered per-stream outcome; a content object
/// named more than once on the page appears exactly once (its edits merge).
pub fn plan_page_streams(
    page: &DocumentPageContentExtentInspection,
    mode: StreamMode,
) -> PageStreamsPlan<'_> {
    // Only an `Inspected` result carries offset-addressable content streams. A
    // `ContentsFailed`, `CompressedLeaf`, or `CompressedLeafInspected` leaf has no
    // source offset for its dictionary; compressed-leaf conversion needs a
    // separate resolved + ownership-safe edit design (out of scope). The offset-
    // based bridge the write pipeline drives never emits either compressed-leaf
    // variant, but this match stays total and future-proof.
    let DocumentPageContentExtentResult::Inspected {
        contents, extents, ..
    } = &page.result
    else {
        return PageStreamsPlan::PageSkip {
            content_object: None,
            reason: PipelineSkipReason::NoContentStream,
        };
    };

    let count = contents.contents.len();
    if count == 0 {
        return PageStreamsPlan::PageSkip {
            content_object: None,
            reason: PipelineSkipReason::NoContentStream,
        };
    }

    if mode == StreamMode::SingleOnly && (count > 1 || !contents.skipped.is_empty()) {
        return PageStreamsPlan::PageSkip {
            content_object: None,
            reason: PipelineSkipReason::MultipleContentStreams { count },
        };
    }

    let page_index = page_index_of(page);
    let mut seen: BTreeSet<IndirectRef> = BTreeSet::new();
    let mut outcomes = Vec::with_capacity(extents.entries.len());
    for (stream_ordinal, entry) in extents.entries.iter().enumerate() {
        let outcome = locate_entry(page_index, stream_ordinal, entry);
        if let Some(object) = outcome_content_object(&outcome) {
            if !seen.insert(object) {
                // Same content-stream object already located on this page: its
                // edits MERGE into the first occurrence (identical object bytes and
                // deterministic edit), so it is neither re-edited nor double-counted
                // and the plan is never handed two dirty objects with one number.
                continue;
            }
        }
        outcomes.push(outcome);
    }
    PageStreamsPlan::Streams(outcomes)
}

/// The content object a per-stream outcome resolved to, when any.
const fn outcome_content_object(outcome: &StreamOutcome<'_>) -> Option<IndirectRef> {
    match outcome {
        StreamOutcome::Located(located) => Some(located.content_object),
        StreamOutcome::Skip { content_object, .. } => *content_object,
    }
}

/// Locate one source-ordered `/Contents` extent entry as a per-stream outcome.
const fn locate_entry(
    page_index: PageIndex,
    stream_ordinal: usize,
    entry: &PageContentExtentInspection,
) -> StreamOutcome<'_> {
    match entry {
        PageContentExtentInspection::Located {
            content_reference,
            object_byte_offset,
            extent,
        } => {
            if matches!(extent, ContentStreamDataExtentInspection::IndirectLength(_)) {
                return StreamOutcome::Skip {
                    stream_ordinal,
                    content_object: Some(content_reference.reference),
                    reason: PipelineSkipReason::IndirectLength,
                };
            }
            StreamOutcome::Located(LocatedContentStream {
                page_index,
                stream_ordinal,
                content_object: content_reference.reference,
                object_byte_offset: *object_byte_offset,
                extent,
            })
        }
        PageContentExtentInspection::Skipped {
            content_reference,
            reason,
        } => StreamOutcome::Skip {
            stream_ordinal,
            content_object: Some(content_reference.reference),
            reason: skip_reason_from_target(reason),
        },
        PageContentExtentInspection::Failed {
            content_reference,
            error,
            ..
        } => StreamOutcome::Skip {
            stream_ordinal,
            content_object: Some(content_reference.reference),
            reason: skip_reason_from_extent_failure(&error.reason),
        },
    }
}

/// Map a carried-through target skip to the pipeline skip taxonomy.
const fn skip_reason_from_target(reason: &SkippedPageContentTargetReason) -> PipelineSkipReason {
    if let SkippedPageContentTargetReason::UnresolvedLookupLocation {
        location:
            ObjectLookupLocation::XrefStreamCompressed {
                object_stream_number,
                index_within_object_stream,
                ..
            },
    } = reason
    {
        return PipelineSkipReason::CompressedContentObject {
            object_stream_number: *object_stream_number,
            index_within_object_stream: *index_within_object_stream,
        };
    }
    PipelineSkipReason::NoContentStream
}

/// Map a delegated content-extent inspection failure to the pipeline taxonomy.
const fn skip_reason_from_extent_failure(
    reason: &ContentStreamDataExtentInspectionRejection,
) -> PipelineSkipReason {
    match reason {
        ContentStreamDataExtentInspectionRejection::MissingLength
        | ContentStreamDataExtentInspectionRejection::DuplicateLength { .. } => {
            PipelineSkipReason::MissingOrDuplicateLength
        }
        ContentStreamDataExtentInspectionRejection::UnsupportedLengthValueKind { value_kind } => {
            PipelineSkipReason::NonDirectNumericLength {
                value_kind: *value_kind,
            }
        }
        ContentStreamDataExtentInspectionRejection::IndirectLengthRequiresXrefTable
        | ContentStreamDataExtentInspectionRejection::IndirectLength { .. }
        | ContentStreamDataExtentInspectionRejection::LookupIndirectLength { .. } => {
            PipelineSkipReason::IndirectLength
        }
        ContentStreamDataExtentInspectionRejection::StreamStart { .. }
        | ContentStreamDataExtentInspectionRejection::DirectLength { .. } => {
            PipelineSkipReason::NoContentStream
        }
    }
}
