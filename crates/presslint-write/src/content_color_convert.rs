//! DeviceLink-driven direct device-colour content conversion (F4-2..F4-5).
//!
//! This is the first real colour conversion of PDF page content. A request now
//! carries a SET of DeviceLinks ([`DeviceLinkInput`]); each is inspected ONCE up
//! front and routed by its **source** space (see [`crate::link_routing`]). For
//! each direct device colour-setting operator in a selected page content stream,
//! the converter looks up the link whose source space equals the operator's
//! declared space, reads the operands, optionally applies the black-preservation
//! overlay, else applies that link through `presslint-color-lcms` (F4-1), and
//! rewrites the operator to the link's **destination** space with
//! deterministically serialized operands.
//!
//! Routing keeps the exact source-space gate: an RGB->CMYK link touches only
//! `rg`/`RG`, a CMYK->CMYK link only `k`/`K`, a Gray link only `g`/`G`. An
//! operator whose declared space matches NO supplied link's source is left
//! byte-verbatim and counted as `no_matching_link`. Only direct device operators
//! `g/G`, `rg/RG`, `k/K` are handled; `cs/CS`, `sc/scn`, `SC/SCN`,
//! ICCBased/Separation/DeviceN/Indexed/Pattern colour spaces, and resource
//! colour-space lookups are out of scope (no resource-space conversion here) and
//! are left byte-verbatim.
//!
//! Candidate discovery is PAINT-DRIVEN: each physical content stream is parsed
//! once by the shared pipeline seam and walked once through
//! [`presslint_paint::PaintProgram`]. An emitted colour event is eligible only
//! when the EXACT bytes at its operator range are one of the six direct device
//! shortcuts; the event's already-parsed colour components and its record range
//! drive the splice. This converter is FAIL-CLOSED per stream: any graphics-walk
//! error (stack underflow, malformed operands of ANY supported operator)
//! refuses the entire physical stream through the existing round-trip mismatch
//! skip — no partial conversion of a stream the shared interpreter cannot walk.

// "DeviceLink" is the ICC profile-class domain term used throughout these docs
// as prose, not always as a code identifier; mirror the `presslint-color-lcms`
// crate and do not force backticks on it.
#![allow(clippy::doc_markdown)]

use std::cell::RefCell;
use std::collections::BTreeMap;

use presslint_color_lcms::{ColorEngine, DeviceLinkSpace, LcmsColorEngine, LcmsError};
use presslint_paint::{ColorSpaceEnv, PaintOpKind, PaintProgram};
use presslint_pdf::{
    DictionaryValueKind, DocumentAccessError, IndirectObjectEditDisposition, IndirectRef,
};
use presslint_selectors::Selector;
use presslint_types::{ByteRange, ColorUsage, PageIndex};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError,
    black_preservation::{BlackPreservationPolicy, black_preservation_replacement},
    content_edit_pipeline::{
        EditPageContentError, EditedContent, PagePreflight, PageSelection, PipelinePageSkip,
        PipelineSkipReason, edit_page_content_incremental_parsed_with_preflight,
    },
    content_stream_plan::StreamMode,
    extgstate_page_guard::{extgstate_page_skip_reason, transparency_group_page_skip_reason},
    link_routing::{DeviceLinkInput, LinkConversionCounts, LinkRouting, build_link_routing},
    parsed_content::ParsedContent,
    pdf_number_serialize::serialize_color_component,
    selector_match::{
        UnsupportedTargetLeaf, collect_unsupported_leaves, selector_matches_operator,
    },
};

/// Request to convert direct device colours in selected pages via a SET of
/// DeviceLinks, routed by source space.
///
/// `PartialEq` only (not `Eq`): the optional `target` selector may carry
/// floating-point colour components.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConvertContentColorsRequest {
    /// Page selection (reuses the shared content-edit pipeline selection).
    pub pages: PageSelection,
    /// The DeviceLinks to route content colours through, each inspected once up
    /// front. A single-link caller passes a one-element vec.
    pub device_links: Vec<DeviceLinkInput>,
    /// Optional pre-DeviceLink black-preservation overlay.
    #[serde(default)]
    pub black_preservation: BlackPreservationPolicy,
    /// Optional operator-local selector narrowing WHICH matching-source colour
    /// operators are converted.
    ///
    /// `None` (the default) converts every matching-source operator, exactly as
    /// F4-2/F4-3 did. `Some(selector)` converts only operators the canonical
    /// selector matcher accepts, evaluated per operator over the facts the
    /// operator makes locally available; non-matching operators are left
    /// byte-verbatim and counted as
    /// [`OperatorSkipCounts::selector_excluded`]. Selector leaves
    /// that are not operator-local (object kind, editability, scope, image/shading
    /// usage, non-device colour spaces) are rejected up front — see
    /// [`ConvertContentColorsError::UnsupportedTargetSelector`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Selector>,
}

/// Per-page aggregate operator-skip taxonomy (honest coverage reporting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorSkipCounts {
    /// Valid direct device operators whose declared space matched NO supplied
    /// link's source space (a coverage gap; left byte-verbatim).
    pub no_matching_link: usize,
    /// Direct device operators whose operand count did not match the space.
    ///
    /// Retained for report-shape compatibility: since candidate discovery moved
    /// to the shared paint walk, a malformed direct device operator refuses its
    /// WHOLE content stream (reported as a stream skip), so this stays `0`.
    pub wrong_operand_count: usize,
    /// Direct device operators with a non-number / multi-token operand.
    ///
    /// Retained for report-shape compatibility: like `wrong_operand_count`,
    /// the paint walk now refuses the whole stream instead, so this stays `0`.
    pub non_number_operand: usize,
    /// Direct device operators with an operand outside `[0.0, 1.0]`.
    pub operand_out_of_range: usize,
    /// Valid direct device operators excluded by the request `target` selector.
    pub selector_excluded: usize,
}

/// Report for one page whose direct device colours were analysed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertedPage {
    /// Zero-based document page index.
    pub page_index: PageIndex,
    /// Indirect references of the analysed content-stream objects of this page,
    /// in stream-ordinal order. A multi-content-stream page carries one entry per
    /// analysed stream object; a single-stream page carries exactly one.
    pub content_objects: Vec<IndirectRef>,
    /// Total number of operators converted in this page (all streams, all links).
    pub operators_converted: usize,
    /// Number of neutral-black operators preserved before the routed link.
    pub black_preserved: usize,
    /// Aggregate per-page operator-skip counts.
    pub operator_skips: OperatorSkipCounts,
    /// Per-link conversion counts, one entry per supplied link in request order.
    pub links: Vec<LinkConversionCounts>,
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
    /// decoded/re-encoded, could not be assembled into operator records, or
    /// could not be walked whole by the shared graphics interpreter (any
    /// graphics-walk error refuses the entire physical stream, fail-closed).
    ContentRoundTripMismatch,
    /// Content-stream ownership was not a single-use in-place mutation.
    OwnershipNotInPlace {
        /// How many document pages referenced this content object.
        occurrences: usize,
        /// Disposition returned by the ownership decision.
        disposition: IndirectObjectEditDisposition,
    },
    /// Deprecated compatibility variant from the coarse T140 guard. Retained for
    /// consumers, but the converter now emits [`Self::ExtGStateUnsafe`].
    ExtGStatePresent,
    /// A used `gs` resource activated overprint/transparency, named an
    /// unresolved resource, or carried malformed/unknown safety parameters.
    ExtGStateUnsafe {
        /// True when `OP`/`op` is true or `OPM` is set.
        overprint: bool,
        /// True when alpha, blend mode, or soft mask activates transparency.
        transparency: bool,
        /// True when a `gs` name is missing from classified resources.
        unresolved: bool,
        /// True when a `gs` operand or safety parameter is malformed/unknown.
        unclassified: bool,
        /// Number of `gs` operators seen in the page's decoded streams.
        gs_count: u32,
    },
    /// A page top-level `/Group` establishes, or hides whether it establishes,
    /// transparency semantics that this direct converter cannot safely edit.
    TransparencyGroupUnsafe {
        /// True when `/Group << /S /Transparency ... >>` was classified.
        transparency: bool,
        /// True when the page-group inspection could not resolve the fact.
        unresolved: bool,
        /// True when the group shape or a group safety field is malformed or
        /// outside the Phase-1 classifier.
        unclassified: bool,
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
    /// The request supplied no DeviceLinks (`device_links` was empty).
    NoDeviceLinks,
    /// One link's bytes could not be inspected (invalid / not a DeviceLink).
    DeviceLinkInspectFailed {
        /// Zero-based index of the offending link in `device_links`.
        index: usize,
        /// The offending link's opaque caller label, if any.
        id: Option<String>,
        /// Delegated `presslint-color-lcms` inspection failure.
        error: LcmsError,
    },
    /// The `target` selector contains one or more leaves this operator-local
    /// converter cannot evaluate (object kind, editability, scope, image/shading
    /// usage, or a non-direct-device colour space). Rejected up front, before any
    /// page is traversed, so the request never silently under-converts.
    UnsupportedTargetSelector {
        /// Every unsupported leaf found in the selector tree, in pre-order.
        unsupported: Vec<UnsupportedTargetLeaf>,
    },
    /// A link's source or destination space is Lab or unsupported, so no direct
    /// device operator can be converted through it.
    UnsupportedLinkSpace {
        /// Zero-based index of the offending link in `device_links`.
        index: usize,
        /// The offending link's opaque caller label, if any.
        id: Option<String>,
        /// The link's inspected source space.
        source: DeviceLinkSpace,
        /// The link's inspected destination space.
        destination: DeviceLinkSpace,
    },
    /// Two supplied links declared the same source space, so routing an operator
    /// of that space would be an ambiguous silent guess.
    AmbiguousLinkSource {
        /// The shared inspected source space.
        space: DeviceLinkSpace,
        /// Index of the first link declaring `space`.
        first_index: usize,
        /// Index of the second link declaring `space`.
        second_index: usize,
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

/// A direct device colour space shared by this crate's content-colour helpers.
///
/// `Ord` is derived (declaration order Gray < Rgb < Cmyk) so it can key the
/// deterministic routing `BTreeMap` in [`crate::link_routing`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeviceColorSpace {
    Gray,
    Rgb,
    Cmyk,
}

impl DeviceColorSpace {
    /// Narrow a DeviceLink space to a directly-convertible device space.
    /// Lab and unsupported spaces have no direct device operator.
    pub const fn from_link(space: DeviceLinkSpace) -> Option<Self> {
        match space {
            DeviceLinkSpace::Gray => Some(Self::Gray),
            DeviceLinkSpace::Rgb => Some(Self::Rgb),
            DeviceLinkSpace::Cmyk => Some(Self::Cmyk),
            DeviceLinkSpace::Lab | DeviceLinkSpace::Unsupported(_) => None,
        }
    }

    /// The direct device colour-setting operator for this space and mode
    /// (lowercase = nonstroking, uppercase = stroking).
    pub const fn operator(self, stroking: bool) -> &'static [u8] {
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
    black_preserved: usize,
    skips: OperatorSkipCounts,
    /// Conversions attributed to each link, indexed by `link_index`.
    link_converted: Vec<usize>,
}

/// Convert direct device colour operators in selected page content streams via
/// a SET of DeviceLinks (routed by source space) and append one incremental
/// revision.
///
/// Every link is inspected exactly once up front and a deterministic routing map
/// is built (see [`build_link_routing`]); an empty `device_links`, a bad link, a
/// Lab/unsupported link space, or two links sharing a source space are all
/// whole-operation [`ConvertContentColorsError`]s before any page is traversed.
/// Only operators whose declared colour space equals SOME link's source space
/// are converted; every other byte is preserved verbatim, so
/// `output.bytes[..input.len()] == input`.
///
/// # Errors
///
/// Returns [`ConvertContentColorsError`] when routing rejects the links (empty,
/// inspect failure, unsupported space, ambiguous source), the request selects no
/// pages, the input cannot be opened, a requested page index is out of range, or
/// the append writer / plan bridge rejects the input.
pub fn convert_content_colors_incremental(
    input: &[u8],
    request: &ConvertContentColorsRequest,
) -> Result<ConvertContentColorsOutput, ConvertContentColorsError> {
    // Inspect every link ONCE up front and build the routing map before any
    // content traversal.
    let routing = build_link_routing(&request.device_links)?;

    // Reject unsupported selector leaves UP FRONT, before any page traversal, so
    // an unanswerable target never silently under-converts.
    if let Some(selector) = &request.target {
        let unsupported = collect_unsupported_leaves(selector);
        if !unsupported.is_empty() {
            return Err(ConvertContentColorsError::UnsupportedTargetSelector { unsupported });
        }
    }

    let tallies: RefCell<Vec<PageTally>> = RefCell::new(Vec::new());
    let target = request.target.as_ref();

    let output = edit_page_content_incremental_parsed_with_preflight(
        input,
        &request.pages,
        StreamMode::MultiStream,
        // Page streams share graphics state, so any unsafe `gs` activation
        // poisons the whole page. Harmless declared or unused resources do not
        // block conversion.
        |_page, extgstate_page, group_page, decoded_streams| {
            transparency_group_page_skip_reason(group_page)
                .or_else(|| extgstate_page_skip_reason(extgstate_page, decoded_streams))
                .map_or(PagePreflight::Continue, PagePreflight::SkipPage)
        },
        |page_index, parsed| {
            match convert_parsed(
                page_index,
                parsed,
                &routing,
                request.black_preservation,
                target,
            ) {
                Some((edited, tally)) => {
                    let edit_count = tally.converted + tally.black_preserved;
                    let has_splices = !edited.is_empty();
                    tallies.borrow_mut().push(tally);
                    if has_splices {
                        EditedContent::Rewritten {
                            decoded: edited,
                            edit_count,
                        }
                    } else {
                        // Analysed but no real byte splice: no revision object, but
                        // the tally is still recorded above for honest reporting.
                        EditedContent::Unchanged
                    }
                }
                None => EditedContent::Rejected(PipelineSkipReason::ContentRoundTripMismatch),
            }
        },
    )
    .map_err(map_error)?;

    let converted = attach_tallies(
        &output.edited,
        &output.skipped,
        tallies.into_inner(),
        &routing,
    );
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

/// Convert one parsed content stream, routing each eligible operator to its
/// source link.
///
/// Candidate discovery is paint-driven: one fresh [`PaintProgram`] walk over
/// the already-assembled records yields the colour events, and an event is
/// eligible only when the EXACT bytes at its operator range are one of the
/// direct device shortcuts (`g`/`G`, `rg`/`RG`, `k`/`K`). Resource colour
/// selection (`cs`/`CS` + `sc`/`scn`/`SC`/`SCN`) also emits colour events but
/// never has shortcut operator bytes, so it stays byte-verbatim; eligibility is
/// never inferred from device state or a resource name. The event's parsed
/// [`presslint_paint::GraphicsColor`] components and its record range drive the
/// splice.
///
/// FAIL-CLOSED: any graphics-walk error (stack underflow, malformed operands of
/// ANY supported operator, invalid ranges) returns `None` — every candidate
/// collected before the error is discarded, no splice is applied, and no tally
/// is recorded. The caller maps `None` to the existing round-trip mismatch
/// skip. Candidate splice metadata stays bounded to eligible operators; the
/// walk is not materialized.
///
/// Returns `Some((edited, tally))` when the whole stream walked. `edited` is
/// empty when no real byte splice is needed; `tally` still records conversions,
/// black-preserved operators, and skips.
fn convert_parsed(
    page_index: PageIndex,
    parsed: &ParsedContent<'_>,
    routing: &LinkRouting,
    black_preservation: BlackPreservationPolicy,
    target: Option<&Selector>,
) -> Option<(Vec<u8>, PageTally)> {
    let decoded = parsed.bytes();
    let program = PaintProgram::new(decoded, parsed.records(), ColorSpaceEnv::empty());

    let mut tally = PageTally {
        link_converted: vec![0; routing.links().len()],
        ..PageTally::default()
    };
    let mut splices: Vec<(ByteRange, Vec<u8>)> = Vec::new();

    for op in program.ops() {
        // A walk error refuses the ENTIRE stream: all candidates are discarded.
        let op = op.ok()?;
        let (PaintOpKind::SetStrokingColor { color } | PaintOpKind::SetNonstrokingColor { color }) =
            &op.kind
        else {
            continue;
        };
        let operator = decoded.get(op.operator_range.start()..op.operator_range.end())?;
        // Exact shortcut bytes decide eligibility. A colour event whose
        // operator is not one of the six shortcuts is left verbatim, uncounted.
        let Some((space, stroking)) = classify_operator(operator) else {
            continue;
        };
        // The walker already guarantees the shortcut's operand count and finite
        // numbers (anything else refused the stream above); the `[0,1]` range
        // gate stays converter-local and per-operator, exactly as before.
        let operands = color.components.as_slice();
        if !operands.iter().all(|value| (0.0..=1.0).contains(value)) {
            tally.skips.operand_out_of_range += 1;
            continue;
        }
        // Selector check (F4-4): a cheap per-operator boolean eval through the
        // canonical matcher, BEFORE routing and the heavier black-preservation
        // / DeviceLink apply. A non-match leaves it verbatim.
        if let Some(selector) = target {
            let usage = if stroking {
                ColorUsage::Stroke
            } else {
                ColorUsage::Fill
            };
            if !selector_matches_operator(selector, page_index, space, usage, operands) {
                tally.skips.selector_excluded += 1;
                continue;
            }
        }
        // Route lookup: the link whose SOURCE space equals this operator's space.
        // No matching link is an honest coverage gap, left byte-verbatim.
        let Some(link) = routing.route(space) else {
            tally.skips.no_matching_link += 1;
            continue;
        };
        let range = op.record_range.into_byte_range();
        if let Some(replacement) = black_preservation_replacement(
            operands,
            space,
            link.destination,
            stroking,
            black_preservation,
        ) {
            tally.black_preserved += 1;
            if decoded.get(range.start..range.end) != Some(replacement.as_slice()) {
                splices.push((range, replacement));
            }
            continue;
        }
        let Ok(components) = LcmsColorEngine.apply_device_link(&link.prepared, operands) else {
            // Unreachable after the source-space route + walker validation
            // (channel count and range are already guaranteed); leave verbatim.
            continue;
        };
        splices.push((
            range,
            replacement_bytes(&components, link.destination, stroking),
        ));
        tally.converted += 1;
        tally.link_converted[link.index] += 1;
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

/// One analysed content-stream object of a page, in closure-invocation order.
struct AnalysedStream {
    page_index: PageIndex,
    stream_ordinal: usize,
    content_object: IndirectRef,
}

/// Per-page accumulator that aggregates every analysed stream of one page.
struct PageAccumulator {
    content_objects: Vec<IndirectRef>,
    converted: usize,
    black_preserved: usize,
    skips: OperatorSkipCounts,
    link_converted: Vec<usize>,
}

/// Associate the per-invocation tallies with the analysed content-stream objects
/// and AGGREGATE them per page.
///
/// The edit closure is invoked once per content-stream OBJECT reaching operator
/// analysis, in `(page index, stream ordinal)` order, pushing exactly one tally
/// each time. Each analysed stream surfaces either as an edited stream (a splice
/// happened) or as an `Unchanged` skip (analysed, zero splices). Collecting both
/// and ordering by `(page index, stream ordinal)` reproduces the closure-
/// invocation order, so a positional zip is exact; the streams are then folded
/// into one [`ConvertedPage`] per page (deterministic page order via `BTreeMap`),
/// with `content_objects` in stream-ordinal order and every count summed.
fn attach_tallies(
    edited: &[crate::content_edit_pipeline::PipelineEditedPage],
    skipped: &[PipelinePageSkip],
    tallies: Vec<PageTally>,
    routing: &LinkRouting,
) -> Vec<ConvertedPage> {
    let mut analysed: Vec<AnalysedStream> = Vec::new();
    for page in edited {
        analysed.push(AnalysedStream {
            page_index: page.page_index,
            stream_ordinal: page.stream_ordinal,
            content_object: page.content_object,
        });
    }
    for skip in skipped {
        if skip.reason == PipelineSkipReason::Unchanged {
            if let Some(content_object) = skip.content_object {
                analysed.push(AnalysedStream {
                    page_index: skip.page_index,
                    stream_ordinal: skip.stream_ordinal,
                    content_object,
                });
            }
        }
    }
    analysed.sort_by_key(|stream| (stream.page_index.0, stream.stream_ordinal));

    let mut pages: BTreeMap<u32, PageAccumulator> = BTreeMap::new();
    for (stream, tally) in analysed.into_iter().zip(tallies) {
        let accumulator = pages
            .entry(stream.page_index.0)
            .or_insert_with(|| PageAccumulator {
                content_objects: Vec::new(),
                converted: 0,
                black_preserved: 0,
                skips: OperatorSkipCounts::default(),
                link_converted: vec![0; routing.links().len()],
            });
        accumulator.content_objects.push(stream.content_object);
        accumulator.converted += tally.converted;
        accumulator.black_preserved += tally.black_preserved;
        add_operator_skips(&mut accumulator.skips, &tally.skips);
        for (slot, count) in accumulator
            .link_converted
            .iter_mut()
            .zip(&tally.link_converted)
        {
            *slot += count;
        }
    }

    pages
        .into_iter()
        .map(|(page_index, accumulator)| ConvertedPage {
            page_index: PageIndex(page_index),
            content_objects: accumulator.content_objects,
            operators_converted: accumulator.converted,
            black_preserved: accumulator.black_preserved,
            operator_skips: accumulator.skips,
            links: link_counts(routing, &accumulator.link_converted),
        })
        .collect()
}

/// Add one stream's operator-skip counts into a per-page aggregate.
const fn add_operator_skips(total: &mut OperatorSkipCounts, part: &OperatorSkipCounts) {
    total.no_matching_link += part.no_matching_link;
    total.wrong_operand_count += part.wrong_operand_count;
    total.non_number_operand += part.non_number_operand;
    total.operand_out_of_range += part.operand_out_of_range;
    total.selector_excluded += part.selector_excluded;
}

/// Build the per-page per-link report, one entry per supplied link in request
/// order (`link_index`), carrying that page's conversions through each link.
fn link_counts(routing: &LinkRouting, link_converted: &[usize]) -> Vec<LinkConversionCounts> {
    routing
        .links()
        .iter()
        .map(|link| LinkConversionCounts {
            link_index: link.index,
            link_id: link.id.clone(),
            source: link.source_link_space,
            destination: link.destination_link_space,
            operators_converted: link_converted.get(link.index).copied().unwrap_or(0),
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
        PipelineSkipReason::ExtGStatePresent => ConvertPageSkipReason::ExtGStatePresent,
        PipelineSkipReason::ExtGStateUnsafe {
            overprint,
            transparency,
            unresolved,
            unclassified,
            gs_count,
        } => ConvertPageSkipReason::ExtGStateUnsafe {
            overprint,
            transparency,
            unresolved,
            unclassified,
            gs_count,
        },
        PipelineSkipReason::TransparencyGroupUnsafe {
            transparency,
            unresolved,
            unclassified,
        } => ConvertPageSkipReason::TransparencyGroupUnsafe {
            transparency,
            unresolved,
            unclassified,
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
