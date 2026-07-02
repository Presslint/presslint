//! DeviceLink-driven direct device-colour content conversion (F4-2).
//!
//! This is the first real colour conversion of PDF page content. It generalises
//! the T126 hardcoded `0 0 0 rg -> 0 0 0 1 k` rewrite into a DeviceLink-driven
//! conversion: for each direct device colour-setting operator in a selected page
//! content stream whose declared colour space equals the supplied DeviceLink's
//! **source** space, it reads the operands, applies the DeviceLink through
//! `presslint-color-lcms` (F4-1), and rewrites the operator to the DeviceLink's
//! **destination** space with the converted, deterministically serialized
//! operands.
//!
//! It is source-space GATED: an RGB->CMYK link touches only `rg`/`RG`, a
//! CMYK->CMYK link only `k`/`K`, a Gray link only `g`/`G`; every other operator
//! (including a colour operator of a different space) is left byte-verbatim, and
//! a mismatched colour operator is counted as a skip. Only direct device
//! operators `g/G`, `rg/RG`, `k/K` are handled; `cs/CS`, `sc/scn`, `SC/SCN`,
//! ICCBased/Separation/DeviceN/Indexed/Pattern colour spaces, and resource
//! colour-space lookups are out of scope (they need graphics-state tracking) and
//! are simply not matched here.

// "DeviceLink" is the ICC profile-class domain term used throughout these docs
// as prose, not always as a code identifier; mirror the `presslint-color-lcms`
// crate and do not force backticks on it.
#![allow(clippy::doc_markdown)]

use std::cell::RefCell;

use presslint_color_lcms::{
    DeviceLinkSpace, LcmsError, apply_device_link_f64, inspect_device_link,
};
use presslint_pdf::{
    DictionaryValueKind, DocumentAccessError, IndirectObjectEditDisposition, IndirectRef,
};
use presslint_syntax::{OperandRecord, Token, TokenKind, assemble_operators, tokenize};
use presslint_types::{ByteRange, PageIndex};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError,
    content_edit_pipeline::{
        EditPageContentError, EditedContent, PageSelection, PipelinePageSkip, PipelineSkipReason,
        edit_page_content_incremental,
    },
    pdf_number_serialize::serialize_color_component,
};

/// Request to convert direct device colours in selected pages via ONE DeviceLink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertContentColorsRequest {
    /// Page selection (reuses the shared content-edit pipeline selection).
    pub pages: PageSelection,
    /// Raw ICC DeviceLink profile bytes, inspected once up front.
    pub device_link_bytes: Vec<u8>,
}

/// Per-page aggregate operator-skip taxonomy (honest coverage reporting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorSkipCounts {
    /// Colour operators of a space other than the DeviceLink source space.
    pub source_space_mismatch: usize,
    /// Source-space operators whose operand count did not match the space.
    pub wrong_operand_count: usize,
    /// Source-space operators with a non-number / multi-token operand.
    pub non_number_operand: usize,
    /// Source-space operators with an operand outside `[0.0, 1.0]`.
    pub operand_out_of_range: usize,
}

/// Report for one page whose direct device colours were analysed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertedPage {
    /// Zero-based document page index.
    pub page_index: PageIndex,
    /// Indirect reference of the analysed content-stream object.
    pub content_object: IndirectRef,
    /// Number of operators converted in this page stream.
    pub operators_converted: usize,
    /// Aggregate per-page operator-skip counts.
    pub operator_skips: OperatorSkipCounts,
}

/// One requested page skipped for a structural reason before operator analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertPageSkip {
    /// Requested zero-based document page index.
    pub page_index: PageIndex,
    /// Content-stream object reference when it was located.
    pub content_object: Option<IndirectRef>,
    /// Structured skip reason.
    pub reason: ConvertPageSkipReason,
}

/// Structured reason a requested page's content stream was not analysed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ConvertPageSkipReason {
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
}

/// Output of a successful [`convert_content_colors_incremental`] call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertContentColorsOutput {
    /// The new PDF bytes: `input` verbatim plus one appended revision.
    pub bytes: Vec<u8>,
    /// Pages whose content was analysed, in document order. A page with zero
    /// conversions is still reported here (with its operator-skip counts) when
    /// it was analysed; it produces no revision object.
    pub converted: Vec<ConvertedPage>,
    /// Requested pages skipped for a structural reason, in request order.
    pub skipped: Vec<ConvertPageSkip>,
}

/// Whole-operation error: no partial conversion is emitted for these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ConvertContentColorsError {
    /// The request selected no pages (an empty index list).
    EmptyRequest,
    /// The DeviceLink bytes could not be inspected (invalid / not a DeviceLink).
    DeviceLinkInspectFailed {
        /// Delegated `presslint-color-lcms` inspection failure.
        error: LcmsError,
    },
    /// The DeviceLink source or destination space is Lab or unsupported, so no
    /// direct device operator can be converted through it.
    UnsupportedLinkSpace {
        /// The DeviceLink's inspected source space.
        source: DeviceLinkSpace,
        /// The DeviceLink's inspected destination space.
        destination: DeviceLinkSpace,
    },
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

/// A direct device colour space handled by this slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceColorSpace {
    Gray,
    Rgb,
    Cmyk,
}

impl DeviceColorSpace {
    /// Operand / channel count for this space.
    const fn channels(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }

    /// Narrow a DeviceLink space to a directly-convertible device space.
    /// Lab and unsupported spaces have no direct device operator.
    const fn from_link(space: DeviceLinkSpace) -> Option<Self> {
        match space {
            DeviceLinkSpace::Gray => Some(Self::Gray),
            DeviceLinkSpace::Rgb => Some(Self::Rgb),
            DeviceLinkSpace::Cmyk => Some(Self::Cmyk),
            DeviceLinkSpace::Lab | DeviceLinkSpace::Unsupported(_) => None,
        }
    }

    /// The direct device colour-setting operator for this space and mode
    /// (lowercase = nonstroking, uppercase = stroking).
    const fn operator(self, stroking: bool) -> &'static [u8] {
        match (self, stroking) {
            (Self::Gray, false) => b"g",
            (Self::Gray, true) => b"G",
            (Self::Rgb, false) => b"rg",
            (Self::Rgb, true) => b"RG",
            (Self::Cmyk, false) => b"k",
            (Self::Cmyk, true) => b"K",
        }
    }
}

/// Classify an operator token's source bytes as a direct device colour operator.
const fn classify_operator(operator: &[u8]) -> Option<(DeviceColorSpace, bool)> {
    match operator {
        b"g" => Some((DeviceColorSpace::Gray, false)),
        b"G" => Some((DeviceColorSpace::Gray, true)),
        b"rg" => Some((DeviceColorSpace::Rgb, false)),
        b"RG" => Some((DeviceColorSpace::Rgb, true)),
        b"k" => Some((DeviceColorSpace::Cmyk, false)),
        b"K" => Some((DeviceColorSpace::Cmyk, true)),
        _ => None,
    }
}

/// Per-page running tally captured by the edit closure.
#[derive(Default)]
struct PageTally {
    converted: usize,
    skips: OperatorSkipCounts,
}

/// Convert direct device colour operators in selected page content streams via
/// ONE DeviceLink and append one incremental revision.
///
/// The DeviceLink is inspected exactly once up front; a Lab or unsupported
/// source/destination space is a whole-operation [`ConvertContentColorsError`]
/// before any page is traversed. Only operators whose declared colour space
/// equals the DeviceLink source space are converted; every other byte is
/// preserved verbatim, so `output.bytes[..input.len()] == input`.
///
/// # Errors
///
/// Returns [`ConvertContentColorsError`] when the DeviceLink cannot be inspected
/// or its spaces are unconvertible, the request is empty, the input cannot be
/// opened, a requested page index is out of range, or the append writer / plan
/// bridge rejects the input.
pub fn convert_content_colors_incremental(
    input: &[u8],
    request: &ConvertContentColorsRequest,
) -> Result<ConvertContentColorsOutput, ConvertContentColorsError> {
    // Inspect the DeviceLink ONCE up front, before any content traversal.
    let info = inspect_device_link(&request.device_link_bytes)
        .map_err(|error| ConvertContentColorsError::DeviceLinkInspectFailed { error })?;
    let (Some(source), Some(destination)) = (
        DeviceColorSpace::from_link(info.source_space),
        DeviceColorSpace::from_link(info.destination_space),
    ) else {
        return Err(ConvertContentColorsError::UnsupportedLinkSpace {
            source: info.source_space,
            destination: info.destination_space,
        });
    };

    let tallies: RefCell<Vec<PageTally>> = RefCell::new(Vec::new());
    let link_bytes = request.device_link_bytes.as_slice();

    let output = edit_page_content_incremental(input, &request.pages, |decoded| {
        match convert_decoded(decoded, link_bytes, source, destination) {
            Some((edited, tally)) => {
                let edit_count = tally.converted;
                tallies.borrow_mut().push(tally);
                if edit_count == 0 {
                    // Analysed but nothing converted: no revision object, but the
                    // tally is still recorded above for honest reporting.
                    EditedContent::Unchanged
                } else {
                    EditedContent::Rewritten {
                        decoded: edited,
                        edit_count,
                    }
                }
            }
            None => EditedContent::Rejected(PipelineSkipReason::ContentRoundTripMismatch),
        }
    })
    .map_err(map_error)?;

    let converted = attach_tallies(&output.edited, &output.skipped, tallies.into_inner());
    let skipped = output
        .skipped
        .into_iter()
        .filter(|skip| skip.reason != PipelineSkipReason::Unchanged)
        .map(map_skip)
        .collect();

    Ok(ConvertContentColorsOutput {
        bytes: output.bytes,
        converted,
        skipped,
    })
}

/// Apply the DeviceLink to one decoded content stream.
///
/// Returns `Some((edited, tally))` when the stream tokenized and assembled (the
/// tally records conversions + skips, and `edited` is the spliced buffer, valid
/// only when `tally.converted > 0`), or `None` when the operators could not be
/// assembled (a round-trip rejection, no tally recorded).
fn convert_decoded(
    decoded: &[u8],
    link_bytes: &[u8],
    source: DeviceColorSpace,
    destination: DeviceColorSpace,
) -> Option<(Vec<u8>, PageTally)> {
    let tokens = tokenize(decoded).ok()?;
    let assembled = assemble_operators(&tokens).ok()?;

    let mut tally = PageTally::default();
    let mut splices: Vec<(ByteRange, Vec<u8>)> = Vec::new();

    for record in assembled.records {
        let operator = tokens[record.operator.token_index].source_bytes(decoded)?;
        let Some((space, stroking)) = classify_operator(operator) else {
            // Not a direct device colour operator: leave verbatim, do not count.
            continue;
        };
        if space != source {
            tally.skips.source_space_mismatch += 1;
            continue;
        }
        if record.operands.len() != space.channels() {
            tally.skips.wrong_operand_count += 1;
            continue;
        }
        let operands = match read_operands(&record.operands, decoded, &tokens) {
            Ok(operands) => operands,
            Err(OperandError::NonNumber) => {
                tally.skips.non_number_operand += 1;
                continue;
            }
            Err(OperandError::OutOfRange) => {
                tally.skips.operand_out_of_range += 1;
                continue;
            }
        };
        let Ok(components) = apply_device_link_f64(link_bytes, &operands) else {
            // Unreachable after the up-front space gate + per-operand validation
            // (channel count and range are already guaranteed); leave verbatim.
            continue;
        };
        splices.push((
            record.range,
            replacement_bytes(&components, destination, stroking),
        ));
        tally.converted += 1;
    }

    if splices.is_empty() {
        return Some((Vec::new(), tally));
    }

    // Apply splices DESCENDING by start offset so earlier ranges stay valid
    // (T126 precedent).
    splices.sort_by_key(|(range, _)| range.start);
    let mut edited = decoded.to_vec();
    for (range, replacement) in splices.iter().rev() {
        edited.splice(range.start..range.end, replacement.iter().copied());
    }
    Some((edited, tally))
}

/// Why a source-space operator's operands could not be used.
enum OperandError {
    NonNumber,
    OutOfRange,
}

/// Read every operand as exactly one finite `[0.0, 1.0]` number token.
fn read_operands(
    operands: &[OperandRecord],
    decoded: &[u8],
    tokens: &[Token],
) -> Result<Vec<f64>, OperandError> {
    let mut values = Vec::with_capacity(operands.len());
    for operand in operands {
        let value = operand_number(operand, decoded, tokens).ok_or(OperandError::NonNumber)?;
        if !(0.0..=1.0).contains(&value) {
            return Err(OperandError::OutOfRange);
        }
        values.push(value);
    }
    Ok(values)
}

/// Parse one operand as exactly one `TokenKind::Number` finite `f64`.
fn operand_number(operand: &OperandRecord, decoded: &[u8], tokens: &[Token]) -> Option<f64> {
    let [token_ref] = operand.tokens.as_slice() else {
        return None;
    };
    if !matches!(tokens[token_ref.token_index].kind, TokenKind::Number(_)) {
        return None;
    }
    let bytes = tokens[token_ref.token_index].source_bytes(decoded)?;
    let text = std::str::from_utf8(bytes).ok()?;
    let value = text.parse::<f64>().ok()?;
    value.is_finite().then_some(value)
}

/// Build `<serialized components joined by single spaces> <dest operator>`.
fn replacement_bytes(components: &[f64], destination: DeviceColorSpace, stroking: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for component in components {
        bytes.extend_from_slice(serialize_color_component(*component).as_bytes());
        bytes.push(b' ');
    }
    bytes.extend_from_slice(destination.operator(stroking));
    bytes
}

/// Associate the per-invocation tallies with the analysed pages.
///
/// The edit closure is invoked once per page reaching operator analysis, in
/// ascending selected-page order, pushing exactly one tally each time. Analysed
/// pages surface either as an edited page (a conversion happened) or as an
/// `Unchanged` skip (analysed, zero conversions). Collecting both and ordering
/// by page index reproduces the closure-invocation order, so a positional zip is
/// exact.
fn attach_tallies(
    edited: &[crate::content_edit_pipeline::PipelineEditedPage],
    skipped: &[PipelinePageSkip],
    tallies: Vec<PageTally>,
) -> Vec<ConvertedPage> {
    let mut analysed: Vec<(PageIndex, IndirectRef)> = Vec::new();
    for page in edited {
        analysed.push((page.page_index, page.content_object));
    }
    for skip in skipped {
        if skip.reason == PipelineSkipReason::Unchanged {
            if let Some(content_object) = skip.content_object {
                analysed.push((skip.page_index, content_object));
            }
        }
    }
    analysed.sort_by_key(|(page_index, _)| page_index.0);

    analysed
        .into_iter()
        .zip(tallies)
        .map(|((page_index, content_object), tally)| ConvertedPage {
            page_index,
            content_object,
            operators_converted: tally.converted,
            operator_skips: tally.skips,
        })
        .collect()
}

const fn map_skip(skip: PipelinePageSkip) -> ConvertPageSkip {
    ConvertPageSkip {
        page_index: skip.page_index,
        content_object: skip.content_object,
        reason: map_skip_reason(skip.reason),
    }
}

const fn map_skip_reason(reason: PipelineSkipReason) -> ConvertPageSkipReason {
    match reason {
        PipelineSkipReason::MultipleContentStreams { count } => {
            ConvertPageSkipReason::MultipleContentStreams { count }
        }
        // `Unchanged` is filtered out before mapping (it is an analysed page,
        // reported under `converted`), so it is folded here for totality only.
        PipelineSkipReason::NoContentStream | PipelineSkipReason::Unchanged => {
            ConvertPageSkipReason::NoContentStream
        }
        PipelineSkipReason::CompressedContentObject {
            object_stream_number,
            index_within_object_stream,
        } => ConvertPageSkipReason::CompressedContentObject {
            object_stream_number,
            index_within_object_stream,
        },
        PipelineSkipReason::IndirectLength => ConvertPageSkipReason::IndirectLength,
        PipelineSkipReason::MissingOrDuplicateLength => {
            ConvertPageSkipReason::MissingOrDuplicateLength
        }
        PipelineSkipReason::NonDirectNumericLength { value_kind } => {
            ConvertPageSkipReason::NonDirectNumericLength { value_kind }
        }
        PipelineSkipReason::UnsupportedFilter => ConvertPageSkipReason::UnsupportedFilter,
        PipelineSkipReason::PredictorFlate { predictor } => {
            ConvertPageSkipReason::PredictorFlate { predictor }
        }
        PipelineSkipReason::ContentRoundTripMismatch => {
            ConvertPageSkipReason::ContentRoundTripMismatch
        }
        PipelineSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition,
        } => ConvertPageSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition,
        },
    }
}

fn map_error(error: EditPageContentError) -> ConvertContentColorsError {
    match error {
        EditPageContentError::EmptyRequest => ConvertContentColorsError::EmptyRequest,
        EditPageContentError::Open { error } => ConvertContentColorsError::Open { error },
        EditPageContentError::PageIndexOutOfRange {
            page_index,
            page_count,
        } => ConvertContentColorsError::PageIndexOutOfRange {
            page_index,
            page_count,
        },
        EditPageContentError::Write { error } => ConvertContentColorsError::Write { error },
        EditPageContentError::Plan { error } => ConvertContentColorsError::Plan { error },
    }
}
