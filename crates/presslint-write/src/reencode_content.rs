//! Semantic **no-op** content-stream re-encode over page content streams.
//!
//! This public T125 wrapper preserves the original API and skip taxonomy while
//! delegating the shared locate -> decode -> edit -> encode -> `WholeStream` write
//! mechanics to `content_edit_pipeline`. Since T136 it drives the per-stream
//! ([`StreamMode::MultiStream`]) path, so a MULTI-content-stream page re-encodes
//! each of its content-stream objects independently (one [`ReencodedPage`] per
//! object) instead of being skipped whole; the no-op stays byte/semantic-identical
//! per object.

use presslint_pdf::{
    DictionaryValueKind, DocumentAccessError, IndirectObjectEditDisposition, IndirectRef,
};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError,
    content_edit_pipeline::{
        EditPageContentError, EditedContent, PageSelection, PipelineEditedPage, PipelineFilterKind,
        PipelinePageSkip, PipelineSkipReason, edit_page_content_incremental_indexed,
    },
    content_stream_plan::StreamMode,
};

/// Request to re-encode selected pages' single content streams as a no-op.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReencodePageContentRequest {
    /// Page selection.
    pub pages: PageSelection,
}

/// Filter path used to re-encode an edited page's content stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReencodeFilterKind {
    /// The content stream had no filter; decoded bytes are written verbatim.
    Raw,
    /// The content stream used a single `/FlateDecode`; decoded bytes are
    /// re-compressed.
    Flate,
}

/// Report for one re-encoded page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReencodedPage {
    /// Zero-based document page index.
    pub page_index: presslint_types::PageIndex,
    /// Indirect reference of the rewritten content-stream object.
    pub content_object: IndirectRef,
    /// Filter path used for the re-encode.
    pub filter_kind: ReencodeFilterKind,
}

/// One requested page skipped before any byte writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReencodePageSkip {
    /// Requested zero-based document page index.
    pub page_index: presslint_types::PageIndex,
    /// Content-stream object reference when it was located.
    pub content_object: Option<IndirectRef>,
    /// Structured skip reason.
    pub reason: ReencodePageSkipReason,
}

/// Structured reason a requested page's content stream was not re-encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ReencodePageSkipReason {
    /// Retained for API stability. Since T136 the re-encode path edits every
    /// content-stream object of a multi-stream page, so this variant is no longer
    /// produced.
    MultipleContentStreams {
        /// Number of direct `/Contents` references observed.
        count: usize,
    },
    /// The page had no direct content-stream reference.
    NoContentStream,
    /// The content stream object is a type-2 compressed object-stream member.
    CompressedContentObject {
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Member index within the object stream.
        index_within_object_stream: usize,
    },
    /// The stream `/Length` is an indirect reference.
    IndirectLength,
    /// The stream dictionary is missing `/Length` or declares it more than once.
    MissingOrDuplicateLength,
    /// The stream `/Length` value is neither a direct integer nor an indirect
    /// reference.
    NonDirectNumericLength {
        /// Shallow value kind reported by dictionary inspection.
        value_kind: DictionaryValueKind,
    },
    /// The content stream uses a filter other than a single `/FlateDecode`.
    UnsupportedFilter,
    /// The content stream is `/FlateDecode` with a `/DecodeParms` predictor.
    PredictorFlate {
        /// The unsupported predictor value.
        predictor: u16,
    },
    /// Decoded content did not re-serialize byte-identically, or decode/encode
    /// failed.
    ContentRoundTripMismatch,
    /// Content-stream ownership was not a single-use in-place mutation.
    OwnershipNotInPlace {
        /// How many document pages referenced this content object.
        occurrences: usize,
        /// Disposition returned by the ownership decision.
        disposition: IndirectObjectEditDisposition,
    },
}

/// Output of a successful [`reencode_page_content_incremental`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReencodePageContentOutput {
    /// The new PDF bytes: `input` verbatim plus one appended revision.
    pub bytes: Vec<u8>,
    /// Pages that were re-encoded, in document order.
    pub reencoded: Vec<ReencodedPage>,
    /// Requested pages that were skipped, with structured reasons.
    pub skipped: Vec<ReencodePageSkip>,
}

/// Error returned when a content-stream re-encode cannot be produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ReencodePageContentError {
    /// The request selected no pages (an empty index list).
    EmptyRequest,
    /// The input could not be opened through the document-access spine.
    Open {
        /// Delegated document-access failure.
        error: Box<DocumentAccessError>,
    },
    /// A requested page index is beyond the enumerated document pages.
    PageIndexOutOfRange {
        /// The offending requested page index.
        page_index: presslint_types::PageIndex,
        /// Number of enumerated document pages.
        page_count: usize,
    },
    /// The append writer rejected the input or the assembled revision.
    Write {
        /// Delegated append-writer failure.
        error: Box<WriteError>,
    },
    /// The plan bridge rejected the assembled incremental-revision plan.
    Plan {
        /// Delegated plan-bridge failure.
        error: Box<PlannedWriteError>,
    },
}

/// Re-encode selected pages' single content streams as a semantic no-op and
/// append one incremental revision.
///
/// The output preserves `input` verbatim as its prefix
/// (`output.bytes[..input.len()] == input`) and appends exactly one incremental
/// revision that rewrites only the selected eligible content-stream objects.
///
/// # Errors
///
/// Returns [`ReencodePageContentError`] when the request is empty, the input
/// cannot be opened, a requested page index is out of range, or the append
/// writer / plan bridge rejects the input.
pub fn reencode_page_content_incremental(
    input: &[u8],
    request: &ReencodePageContentRequest,
) -> Result<ReencodePageContentOutput, ReencodePageContentError> {
    let output = edit_page_content_incremental_indexed(
        input,
        &request.pages,
        StreamMode::MultiStream,
        |_page_index, decoded| EditedContent::Rewritten {
            decoded: decoded.to_vec(),
            edit_count: 0,
        },
    )
    .map_err(map_error)?;

    Ok(ReencodePageContentOutput {
        bytes: output.bytes,
        reencoded: output.edited.into_iter().map(map_reencoded).collect(),
        skipped: output.skipped.into_iter().map(map_skip).collect(),
    })
}

const fn map_filter(filter: PipelineFilterKind) -> ReencodeFilterKind {
    match filter {
        PipelineFilterKind::Raw => ReencodeFilterKind::Raw,
        PipelineFilterKind::Flate => ReencodeFilterKind::Flate,
    }
}

const fn map_reencoded(page: PipelineEditedPage) -> ReencodedPage {
    ReencodedPage {
        page_index: page.page_index,
        content_object: page.content_object,
        filter_kind: map_filter(page.filter_kind),
    }
}

const fn map_skip(skip: PipelinePageSkip) -> ReencodePageSkip {
    ReencodePageSkip {
        page_index: skip.page_index,
        content_object: skip.content_object,
        reason: map_skip_reason(skip.reason),
    }
}

const fn map_skip_reason(reason: PipelineSkipReason) -> ReencodePageSkipReason {
    match reason {
        PipelineSkipReason::MultipleContentStreams { count } => {
            ReencodePageSkipReason::MultipleContentStreams { count }
        }
        PipelineSkipReason::NoContentStream | PipelineSkipReason::Unchanged => {
            ReencodePageSkipReason::NoContentStream
        }
        PipelineSkipReason::CompressedContentObject {
            object_stream_number,
            index_within_object_stream,
        } => ReencodePageSkipReason::CompressedContentObject {
            object_stream_number,
            index_within_object_stream,
        },
        PipelineSkipReason::IndirectLength => ReencodePageSkipReason::IndirectLength,
        PipelineSkipReason::MissingOrDuplicateLength => {
            ReencodePageSkipReason::MissingOrDuplicateLength
        }
        PipelineSkipReason::NonDirectNumericLength { value_kind } => {
            ReencodePageSkipReason::NonDirectNumericLength { value_kind }
        }
        PipelineSkipReason::UnsupportedFilter => ReencodePageSkipReason::UnsupportedFilter,
        PipelineSkipReason::PredictorFlate { predictor } => {
            ReencodePageSkipReason::PredictorFlate { predictor }
        }
        PipelineSkipReason::ContentRoundTripMismatch => {
            ReencodePageSkipReason::ContentRoundTripMismatch
        }
        PipelineSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition,
        } => ReencodePageSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition,
        },
    }
}

fn map_error(error: EditPageContentError) -> ReencodePageContentError {
    match error {
        EditPageContentError::EmptyRequest => ReencodePageContentError::EmptyRequest,
        EditPageContentError::Open { error } => ReencodePageContentError::Open { error },
        EditPageContentError::PageIndexOutOfRange {
            page_index,
            page_count,
        } => ReencodePageContentError::PageIndexOutOfRange {
            page_index,
            page_count,
        },
        EditPageContentError::Write { error } => ReencodePageContentError::Write { error },
        EditPageContentError::Plan { error } => ReencodePageContentError::Plan { error },
    }
}
