//! Direct page-content RGB-black operator rewrite.
//!
//! This is an exact syntactic rewrite of direct `DeviceRGB` black color-setting
//! operators in eligible page content streams. It does not perform ICC
//! conversion, color management, or a visual equivalence claim.

use presslint_pdf::{
    DictionaryValueKind, DocumentAccessError, IndirectObjectEditDisposition, IndirectRef,
};
use presslint_syntax::{OperandRecord, TokenKind, assemble_operators, tokenize};
use presslint_types::{ByteRange, PageIndex};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError,
    content_edit_pipeline::{
        EditPageContentError, EditedContent, PageSelection, PipelineEditedPage, PipelinePageSkip,
        PipelineSkipReason, edit_page_content_incremental,
    },
};

/// Request to rewrite direct RGB-black color operators in selected pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentColorRewriteRequest {
    /// Page selection.
    pub pages: PageSelection,
}

/// Report for one page whose content stream was rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewrittenPage {
    /// Zero-based document page index.
    pub page_index: PageIndex,
    /// Indirect reference of the rewritten content-stream object.
    pub content_object: IndirectRef,
    /// Number of `rg`/`RG` operators rewritten in this page stream.
    pub operator_rewrites: usize,
}

/// One requested page skipped before any byte writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentColorRewriteSkip {
    /// Requested zero-based document page index.
    pub page_index: PageIndex,
    /// Content-stream object reference when it was located.
    pub content_object: Option<IndirectRef>,
    /// Structured skip reason.
    pub reason: ContentColorRewriteSkipReason,
}

/// Structured reason a requested page's content stream was not rewritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ContentColorRewriteSkipReason {
    /// The page declared more than one content stream; this slice edits
    /// single-stream pages only.
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
    /// Decoded content did not re-serialize byte-identically, could not be
    /// decoded/re-encoded, or could not be assembled into operator records.
    ContentRoundTripMismatch,
    /// Content-stream ownership was not a single-use in-place mutation.
    OwnershipNotInPlace {
        /// How many document pages referenced this content object.
        occurrences: usize,
        /// Disposition returned by the ownership decision.
        disposition: IndirectObjectEditDisposition,
    },
    /// The page content stream had no matching direct RGB-black `rg`/`RG`
    /// operator.
    NoMatchingOperators,
}

/// Output of a successful [`rewrite_rgb_black_to_cmyk_incremental`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentColorRewriteOutput {
    /// The new PDF bytes: `input` verbatim plus one appended revision.
    pub bytes: Vec<u8>,
    /// Pages that were rewritten, in document order.
    pub rewritten: Vec<RewrittenPage>,
    /// Requested pages that were skipped, with structured reasons.
    pub skipped: Vec<ContentColorRewriteSkip>,
}

/// Error returned when a color rewrite cannot be produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ContentColorRewriteError {
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
        page_index: PageIndex,
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

/// Rewrite direct `DeviceRGB` black `rg`/`RG` operators to `DeviceCMYK` black
/// `k`/`K` operators and append one incremental revision.
///
/// Only exact operator tokens `rg` and `RG` with exactly three single numeric
/// operands parsing to finite `0.0` are rewritten. All other decoded bytes are
/// preserved verbatim.
///
/// # Errors
///
/// Returns [`ContentColorRewriteError`] when the request is empty, the input
/// cannot be opened, a requested page index is out of range, or the append
/// writer / plan bridge rejects the input.
pub fn rewrite_rgb_black_to_cmyk_incremental(
    input: &[u8],
    request: &ContentColorRewriteRequest,
) -> Result<ContentColorRewriteOutput, ContentColorRewriteError> {
    let output = edit_page_content_incremental(input, &request.pages, rewrite_rgb_black_decoded)
        .map_err(map_error)?;

    Ok(ContentColorRewriteOutput {
        bytes: output.bytes,
        rewritten: output.edited.into_iter().map(map_rewritten).collect(),
        skipped: output.skipped.into_iter().map(map_skip).collect(),
    })
}

fn rewrite_rgb_black_decoded(decoded: &[u8]) -> EditedContent {
    let Ok(tokens) = tokenize(decoded) else {
        return EditedContent::Rejected(PipelineSkipReason::ContentRoundTripMismatch);
    };
    let Ok(assembled) = assemble_operators(&tokens) else {
        return EditedContent::Rejected(PipelineSkipReason::ContentRoundTripMismatch);
    };

    let mut splices: Vec<(ByteRange, &'static [u8])> = Vec::new();
    for record in assembled.records {
        let Some(operator) = tokens[record.operator.token_index].source_bytes(decoded) else {
            return EditedContent::Rejected(PipelineSkipReason::ContentRoundTripMismatch);
        };
        let replacement = match operator {
            b"rg" => b"0 0 0 1 k".as_slice(),
            b"RG" => b"0 0 0 1 K".as_slice(),
            _ => continue,
        };
        if record.operands.len() == 3
            && record
                .operands
                .iter()
                .all(|operand| operand_is_exact_zero_number(operand, decoded, &tokens))
        {
            splices.push((record.range, replacement));
        }
    }

    if splices.is_empty() {
        return EditedContent::Unchanged;
    }

    splices.sort_by_key(|(range, _)| range.start);
    let mut edited = decoded.to_vec();
    for (range, replacement) in splices.iter().rev() {
        edited.splice(range.start..range.end, replacement.iter().copied());
    }

    EditedContent::Rewritten {
        decoded: edited,
        edit_count: splices.len(),
    }
}

fn operand_is_exact_zero_number(
    operand: &OperandRecord,
    decoded: &[u8],
    tokens: &[presslint_syntax::Token],
) -> bool {
    let [token_ref] = operand.tokens.as_slice() else {
        return false;
    };
    if !matches!(tokens[token_ref.token_index].kind, TokenKind::Number(_)) {
        return false;
    }
    let Some(bytes) = tokens[token_ref.token_index].source_bytes(decoded) else {
        return false;
    };
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let Ok(value) = text.parse::<f64>() else {
        return false;
    };
    value.is_finite() && value == 0.0
}

const fn map_rewritten(page: PipelineEditedPage) -> RewrittenPage {
    RewrittenPage {
        page_index: page.page_index,
        content_object: page.content_object,
        operator_rewrites: page.edit_count,
    }
}

const fn map_skip(skip: PipelinePageSkip) -> ContentColorRewriteSkip {
    ContentColorRewriteSkip {
        page_index: skip.page_index,
        content_object: skip.content_object,
        reason: map_skip_reason(skip.reason),
    }
}

const fn map_skip_reason(reason: PipelineSkipReason) -> ContentColorRewriteSkipReason {
    match reason {
        PipelineSkipReason::MultipleContentStreams { count } => {
            ContentColorRewriteSkipReason::MultipleContentStreams { count }
        }
        PipelineSkipReason::NoContentStream => ContentColorRewriteSkipReason::NoContentStream,
        PipelineSkipReason::CompressedContentObject {
            object_stream_number,
            index_within_object_stream,
        } => ContentColorRewriteSkipReason::CompressedContentObject {
            object_stream_number,
            index_within_object_stream,
        },
        PipelineSkipReason::IndirectLength => ContentColorRewriteSkipReason::IndirectLength,
        PipelineSkipReason::MissingOrDuplicateLength => {
            ContentColorRewriteSkipReason::MissingOrDuplicateLength
        }
        PipelineSkipReason::NonDirectNumericLength { value_kind } => {
            ContentColorRewriteSkipReason::NonDirectNumericLength { value_kind }
        }
        PipelineSkipReason::UnsupportedFilter => ContentColorRewriteSkipReason::UnsupportedFilter,
        PipelineSkipReason::PredictorFlate { predictor } => {
            ContentColorRewriteSkipReason::PredictorFlate { predictor }
        }
        PipelineSkipReason::ContentRoundTripMismatch => {
            ContentColorRewriteSkipReason::ContentRoundTripMismatch
        }
        PipelineSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition,
        } => ContentColorRewriteSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition,
        },
        // `Unchanged` = no matching operators. `ExtGStatePresent` is UNREACHABLE
        // here: this caller drives `edit_page_content_incremental`, whose delegated
        // no-op preflight never poisons a page, so the variant is never produced;
        // the arm exists only for match exhaustiveness and folds into the same
        // no-matching-operators reason for totality.
        PipelineSkipReason::Unchanged | PipelineSkipReason::ExtGStatePresent => {
            ContentColorRewriteSkipReason::NoMatchingOperators
        }
    }
}

fn map_error(error: EditPageContentError) -> ContentColorRewriteError {
    match error {
        EditPageContentError::EmptyRequest => ContentColorRewriteError::EmptyRequest,
        EditPageContentError::Open { error } => ContentColorRewriteError::Open { error },
        EditPageContentError::PageIndexOutOfRange {
            page_index,
            page_count,
        } => ContentColorRewriteError::PageIndexOutOfRange {
            page_index,
            page_count,
        },
        EditPageContentError::Write { error } => ContentColorRewriteError::Write { error },
        EditPageContentError::Plan { error } => ContentColorRewriteError::Plan { error },
    }
}
