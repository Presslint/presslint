use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefTableInspection, ContentStreamDataExtentInspection,
    ContentStreamDataExtentInspectionError, PageContentReference, PageContentTargetInspection,
    PageContentTargetsInspection, SkippedPageContentTargetReason,
    inspect_content_stream_data_extent,
};

/// Locate-only report for the ordered content-stream data byte extents of a
/// single leaf page.
///
/// This report aggregates an existing [`PageContentTargetsInspection`] (the
/// resolved `/Contents` targets) over the combined single-stream extent helper
/// [`inspect_content_stream_data_extent`]. It stores only the caller-visible
/// source length and one source-ordered result per target, preserving the
/// page's content-stream order. It does not retain or copy stream bytes, decoded
/// bytes, object bodies, dictionaries, source slices, or a concatenated content
/// buffer; owned data is limited to the source-ordered `Vec` of fixed-size
/// delegated reports and the original [`PageContentReference`] values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageContentExtentsInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Source-ordered per-target extent results, one per `targets` entry.
    pub entries: Vec<PageContentExtentInspection>,
}

impl PageContentExtentsInspection {
    /// Count of successfully located content-stream data extents.
    ///
    /// A page is fully located when this equals `entries.len()`, so callers can
    /// detect a fully-located page without re-matching variants.
    #[must_use]
    pub fn located_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| matches!(entry, PageContentExtentInspection::Located { .. }))
            .count()
    }
}

/// Locate-only result for one source-ordered page content target.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PageContentExtentInspection {
    /// The resolved target's content-stream data extent was located.
    Located {
        /// Original direct `/Contents` reference carried from the target.
        content_reference: PageContentReference,
        /// Resolved in-use object byte offset delegated to the extent helper.
        object_byte_offset: usize,
        /// Delegated combined single-stream data extent report.
        extent: ContentStreamDataExtentInspection,
    },
    /// The target was carried through unchanged as a structured skip.
    Skipped {
        /// Original direct `/Contents` reference carried from the target.
        content_reference: PageContentReference,
        /// Structured skip reason preserved from the target resolution.
        reason: SkippedPageContentTargetReason,
    },
    /// The resolved target's extent inspection failed.
    Failed {
        /// Original direct `/Contents` reference carried from the target.
        content_reference: PageContentReference,
        /// Resolved in-use object byte offset delegated to the extent helper.
        object_byte_offset: usize,
        /// Delegated combined single-stream extent failure, preserved verbatim.
        error: ContentStreamDataExtentInspectionError,
    },
}

/// Locate the ordered content-stream data byte extents for a leaf page.
///
/// The helper performs a deterministic locate-only pass over `targets.entries`
/// in source order, producing exactly one source-ordered result per target and
/// never reordering or deduplicating. Each
/// [`PageContentTargetInspection::Resolved`] target is delegated to
/// [`inspect_content_stream_data_extent`] at its resolved `object_byte_offset`
/// with `Some(xref)`: on success the entry carries the original
/// [`PageContentReference`], the resolved offset, and the delegated
/// [`ContentStreamDataExtentInspection`]; on failure the entry preserves the
/// underlying [`ContentStreamDataExtentInspectionError`] and the resolved offset
/// without aborting the remaining targets. Each
/// [`PageContentTargetInspection::Skipped`] target is carried through unchanged,
/// preserving its [`PageContentReference`] and
/// [`SkippedPageContentTargetReason`], without attempting extent inspection.
///
/// For a given input the per-target located extent equals byte-for-byte what
/// [`inspect_content_stream_data_extent`] returns for the same resolved object
/// offset and xref table; this helper reimplements no `/Length`,
/// indirect-resolution, or `endstream` logic and only iterates targets and
/// dispatches. It reads, copies, slices, decodes, concatenates, and tokenizes no
/// stream-data bytes, builds no concatenated content buffer, follows no `/Prev`,
/// parses no xref or object streams, and builds no object map or cache.
#[must_use]
pub fn inspect_page_content_extents(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    targets: &PageContentTargetsInspection,
) -> PageContentExtentsInspection {
    let entries = targets
        .entries
        .iter()
        .map(|target| locate_target_extent(input, xref, target))
        .collect();

    PageContentExtentsInspection {
        byte_len: input.len(),
        entries,
    }
}

fn locate_target_extent(
    input: &[u8],
    xref: &ClassicXrefTableInspection,
    target: &PageContentTargetInspection,
) -> PageContentExtentInspection {
    match target {
        PageContentTargetInspection::Resolved {
            content_reference,
            object_byte_offset,
            ..
        } => match inspect_content_stream_data_extent(input, Some(xref), *object_byte_offset) {
            Ok(extent) => PageContentExtentInspection::Located {
                content_reference: *content_reference,
                object_byte_offset: *object_byte_offset,
                extent,
            },
            Err(error) => PageContentExtentInspection::Failed {
                content_reference: *content_reference,
                object_byte_offset: *object_byte_offset,
                error,
            },
        },
        PageContentTargetInspection::Skipped {
            content_reference,
            reason,
        } => PageContentExtentInspection::Skipped {
            content_reference: *content_reference,
            reason: reason.clone(),
        },
    }
}
