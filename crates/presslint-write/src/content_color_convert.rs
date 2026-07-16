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
//! byte-verbatim and counted as `no_matching_link`. Proven exact page aliases
//! of DeviceGray/RGB/CMYK are converted root-atomically after the same paint
//! walk; a named `Do` of a proven ordinary image is colour-neutral to that
//! proof and a proven stencil consumes the nonstroking alias lane, while
//! forms, unknown/invalid images, and inline images keep refusing the epoch.
//! ICCBased/Separation/DeviceN/Indexed/Pattern spaces remain out of scope.
//!
//! Each analysed page consults a private
//! [`crate::page_device_space_policy::PageDeviceSpacePolicy`]: exact page
//! DeviceGray/RGB/CMYK aliases resolve in the single paint walk, exact numeric
//! alias setters retain their structural eligible/ineligible counts, and a
//! selected + routed direct shortcut converts only
//! when the matching effective `/DefaultGray`/`/DefaultRGB`/`/DefaultCMYK` is
//! proven absent or identity for BOTH the source and the emitted destination
//! family; otherwise the operator stays byte-verbatim and is counted as
//! `default_color_space_unsafe`.
//!
//! Candidate discovery is PAINT-DRIVEN: each page's exact decoded `/Contents`
//! sequence is parsed and walked once through [`presslint_paint::PaintProgram`].
//! An emitted colour event is eligible only
//! when the EXACT bytes at its operator range are one of the six direct device
//! shortcuts; the event's already-parsed colour components and its record range
//! drive the splice. This converter is FAIL-CLOSED per page: any graphics-walk
//! error (stack underflow, malformed operands of ANY supported operator)
//! refuses the entire page through the existing round-trip mismatch skip.

// "DeviceLink" is the ICC profile-class domain term used throughout these docs
// as prose, not always as a code identifier; mirror the `presslint-color-lcms`
// crate and do not force backticks on it.
#![allow(clippy::doc_markdown)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use presslint_color_lcms::{ColorEngine, DeviceLinkSpace, LcmsColorEngine, LcmsError};
use presslint_paint::{GraphicsStateSnapshot, PaintOpKind, PaintProgram};
use presslint_pdf::{
    DictionaryValueKind, DocumentAccessError, IndirectObjectEditDisposition, IndirectRef,
    ObjectLookup, PageExtGStateResourcesInspection, PageFontResourcesInspection,
    PageXObjectResourcesInspection,
};
use presslint_selectors::Selector;
use presslint_types::{ColorUsage, PageIndex};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError,
    alias_epoch_plan::{AliasEpochOutcome, AliasEpochPlan, EpochStatus, LaneSide},
    black_preservation::{BlackPreservationPolicy, black_preservation_components},
    content_edit_pipeline::{
        EditPageContentError, PageSelection, PipelinePageSkip, PipelineSkipReason,
    },
    content_sequence_pipeline::{PageSequenceEdit, edit_page_content_incremental_sequence},
    extgstate_page_guard::{extgstate_page_skip_reason, transparency_group_page_skip_reason},
    form_xobject_effect::{FormXObjectEffectAnalyzer, FormXObjectRefusalCounts},
    link_routing::{
        DeviceLinkInput, LinkConversionCounts, LinkRouting, RoutedLink, build_link_routing,
    },
    page_content_sequence::{LocalSplice, PageContentSequence},
    page_device_space_policy::{PageColorFacts, PageDeviceSpacePolicy},
    page_font_policy::PageFontPolicy,
    page_xobject_policy::PageXObjectPolicy,
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
    /// WHOLE page content sequence (reported as a page skip), so this stays `0`.
    pub wrong_operand_count: usize,
    /// Direct device operators with a non-number / multi-token operand.
    ///
    /// Retained for report-shape compatibility: like `wrong_operand_count`,
    /// the paint walk now refuses the whole page instead, so this stays `0`.
    pub non_number_operand: usize,
    /// Direct device operators with an operand outside `[0.0, 1.0]`.
    pub operand_out_of_range: usize,
    /// Valid direct device operators excluded by the request `target` selector.
    pub selector_excluded: usize,
    /// Selected + routed direct operators left byte-verbatim because the
    /// source or emitted destination device family had a replaced or
    /// unprovable effective `/Default*` colour space (fail-closed interlock).
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub default_color_space_unsafe: usize,
}

/// Serde helper: omit an additive scalar count while it is zero, so existing
/// zero-count JSON shapes stay byte-compatible.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn count_is_zero(count: &usize) -> bool {
    *count == 0
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
    /// Exact numeric `sc`/`SC`/`scn`/`SCN` setters under classified page
    /// device aliases that are structurally eligible for a closed
    /// alias-conversion epoch.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub resource_alias_setters_eligible: usize,
    /// Setters under classified page device aliases that failed a structural
    /// eligibility requirement. This structural count is independent of later
    /// root execution or shared-record refusal.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub resource_alias_setters_ineligible: usize,
    /// Unique physical retained alias-candidate records replaced by canonical
    /// direct destination operators.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub resource_alias_candidates_converted: usize,
    /// Unique physical retained alias-candidate records held verbatim because
    /// a consumed root or its shared-record component was non-executable.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub resource_alias_candidates_refused: usize,
    /// Aggregate per-page operator-skip counts.
    pub operator_skips: OperatorSkipCounts,
    /// Per-page refusal-class counts for refused demanded Form identities
    /// (root `Do` targets only; the taxonomy is observe-only and never
    /// influences admission).
    #[serde(default, skip_serializing_if = "FormXObjectRefusalCounts::is_empty")]
    pub form_xobject_refusal_counts: FormXObjectRefusalCounts,
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
    /// graphics-walk error refuses the entire page content sequence,
    /// fail-closed).
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

    const fn component_count(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }
}

/// Classify an operator token's source bytes as a direct device colour operator.
pub const fn classify_operator(operator: &[u8]) -> Option<(DeviceColorSpace, bool)> {
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

/// Execute the one shared component path used by direct and retained alias
/// candidates. Exact black preservation precedes one prepared-link apply.
fn execute_components(
    operands: &[f64],
    source: DeviceColorSpace,
    link: &RoutedLink,
    black_preservation: BlackPreservationPolicy,
) -> Option<ComponentExecution> {
    let info = link.prepared.info();
    if operands.len() != source.component_count()
        || operands.len() != info.input_channels
        || link.destination.component_count() != info.output_channels
        || DeviceColorSpace::from_link(info.source_space) != Some(source)
        || DeviceColorSpace::from_link(info.destination_space) != Some(link.destination)
        || !operands.iter().all(|value| (0.0..=1.0).contains(value))
    {
        return None;
    }
    if let Some(components) =
        black_preservation_components(operands, source, link.destination, black_preservation)
    {
        return Some(ComponentExecution {
            components,
            attribution: ComponentAttribution::BlackPreserved,
        });
    }
    let components = LcmsColorEngine
        .apply_device_link(&link.prepared, operands)
        .ok()?;
    (components.len() == link.destination.component_count()).then_some(ComponentExecution {
        components,
        attribution: ComponentAttribution::Link(link.index),
    })
}

/// Per-page running tally captured by the edit closure.
#[derive(Default)]
struct PageTally {
    converted: usize,
    black_preserved: usize,
    alias_setters_eligible: usize,
    alias_setters_ineligible: usize,
    skips: OperatorSkipCounts,
    /// Conversions attributed to each link, indexed by `link_index`.
    link_converted: Vec<usize>,
}

type PhysicalCandidateKey = (IndirectRef, usize, usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ComponentAttribution {
    BlackPreserved,
    Link(usize),
}

struct ComponentExecution {
    components: Vec<f64>,
    attribution: ComponentAttribution,
}

struct AliasCandidateLocation {
    key: PhysicalCandidateKey,
    occurrence_index: usize,
    range: presslint_types::ByteRange,
}

#[derive(Default)]
struct AliasExecutionTally {
    converted: usize,
    refused: usize,
    operators_converted: usize,
    black_preserved: usize,
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

    let target = request.target.as_ref();

    // One request-scoped root Form colour-effect analyzer, shared (with its
    // exact-identity cache and aggregate bounds) across every selected page.
    let analyzer = RefCell::new(FormXObjectEffectAnalyzer::new());

    let output = edit_page_content_incremental_sequence(
        input,
        &request.pages,
        // Page streams share graphics state, so any unsafe `gs` activation
        // poisons the whole page. Harmless declared or unused resources do not
        // block conversion.
        |_page, extgstate_page, group_page, sequence| {
            transparency_group_page_skip_reason(group_page)
                .or_else(|| extgstate_page_skip_reason(extgstate_page, sequence))
        },
        |page_index, sequence, facts, xobjects, fonts, extgstates, lookup| {
            convert_sequence(
                page_index,
                sequence,
                facts,
                xobjects,
                fonts,
                extgstates,
                &routing,
                request.black_preservation,
                target,
                input,
                lookup,
                &analyzer,
            )
        },
    )
    .map_err(map_error)?;

    let converted = output
        .pages
        .into_iter()
        .filter(|page| !page.content_objects.is_empty())
        .collect();
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

/// Convert one parsed logical page-content sequence, routing each eligible
/// operator to its source link.
///
/// Candidate discovery is paint-driven: one fresh [`PaintProgram`] walk over
/// the already-assembled records (with the page policy's borrowed alias
/// environment) yields the colour events, and an event is
/// eligible only when the EXACT bytes at its operator range are one of the
/// direct device shortcuts (`g`/`G`, `rg`/`RG`, `k`/`K`). Every walked op is
/// first observed by the private [`AliasEpochPlan`], which owns the structural
/// alias-setter tallies and retains exact closed/refused roots for one
/// post-walk execution pass. Conversion eligibility is never reconstructed
/// from raw bytes, device state, or a resource name. The event's parsed
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
/// Returns a page-scoped edit plan and tally metadata when the whole sequence
/// walks successfully. Occurrence plans without splices still participate in
/// repeated-object reconciliation; metadata is published only after the caller
/// stages and validates the complete page transaction.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn convert_sequence(
    page_index: PageIndex,
    sequence: &PageContentSequence,
    facts: &PageColorFacts<'_>,
    xobject_report: Option<&PageXObjectResourcesInspection>,
    font_report: Option<&PageFontResourcesInspection>,
    extgstate_report: Option<&PageExtGStateResourcesInspection>,
    routing: &LinkRouting,
    black_preservation: BlackPreservationPolicy,
    target: Option<&Selector>,
    input: &[u8],
    lookup: ObjectLookup<'_>,
    analyzer: &RefCell<FormXObjectEffectAnalyzer>,
) -> Option<PageSequenceEdit<ConvertedPage>> {
    // Reset the analyzer's per-page refusal tally before this page's Form
    // demands begin, discarding any partial tally an earlier aborted page may
    // have left (this call never decodes, walks, or charges a budget).
    analyzer.borrow_mut().begin_page_refusal_tally();
    let decoded = sequence.bytes();
    // The page device-space policy replaces the earlier always-empty
    // environment: exact page device aliases resolve in graphics state, and
    // the per-family /Default* statuses gate the direct shortcut route.
    let policy = PageDeviceSpacePolicy::from_page_facts(facts);
    // The page-exact XObject colour-effect map: one deterministic build per
    // analysed page from the matched advisory report; a failed inspection or
    // identity join classifies every named `Do` as unknown, fail-closed. Exact
    // Form targets stay unresolved in this SAME sole semantic-name map until a
    // valid outer `Do` demands them, then resolve through the shared request
    // analyzer and cache only their fixed two-lane effect. Every Form byte stays
    // untouched and there is no separate demand scan.
    let xobject_policy = PageXObjectPolicy::analyzed(xobject_report, input, lookup, analyzer);
    // The page font policy maps the exact matched /Font and /ExtGState reports
    // into the effective FontEnv/ExtGStateEnv views plus the corroborated
    // ordinary-font admission set. A failed inspection or identity join makes
    // the environment unknown and refuses TextShow, never skipping the page.
    let font_policy = PageFontPolicy::new(font_report, extgstate_report);
    // Switch to the all-environments walk so `Tf`/`gs` resolve into the
    // effective font state that ordinary TextShow admission consumes. The seven
    // ExtGState safety parameters stay neutral; the page preflight remains the
    // controlling overprint/transparency gate.
    let program = PaintProgram::with_all_envs(
        decoded,
        sequence.records(),
        policy.color_space_env(),
        font_policy.extgstate_env(),
        font_policy.font_env(),
    );
    // The alias-epoch plan observes EVERY walked op before the colour-only
    // branch below. It is the sole producer of the structural alias-setter
    // tallies (unchanged T177 per-setter meaning) and privately retains
    // closed/refused epoch candidates; execution happens once after the walk.
    let mut plan = AliasEpochPlan::new(
        &policy,
        routing,
        &xobject_policy,
        &font_policy,
        target,
        page_index,
        sequence,
    );
    let mut tallies: Vec<PageTally> = (0..sequence.occurrence_count())
        .map(|_| PageTally {
            link_converted: vec![0; routing.links().len()],
            ..PageTally::default()
        })
        .collect();
    let mut occurrence_plans = vec![Vec::new(); sequence.occurrence_count()];
    // Retain only the immediately preceding shared snapshot, seeded with the
    // exact PDF page-default graphics state. This is the selected colour-space
    // state before the current operator and costs one `Rc` bump per record,
    // not a deep graphics-state clone or second walk.
    let mut previous_state = Rc::new(GraphicsStateSnapshot::page_default());

    for op in program.ops() {
        // A walk error refuses the ENTIRE page: all candidates are discarded.
        let op = op.ok()?;
        let state_before = std::mem::replace(&mut previous_state, Rc::clone(&op.state));
        let operator = decoded.get(op.operator_range.start()..op.operator_range.end())?;
        plan.observe(&op, operator, &state_before, sequence);
        let (PaintOpKind::SetStrokingColor { color } | PaintOpKind::SetNonstrokingColor { color }) =
            &op.kind
        else {
            continue;
        };
        // Exact shortcut bytes decide eligibility. A colour event whose
        // operator is not one of the six shortcuts is left verbatim; exact
        // alias setters were already retained by the plan above.
        let Some((space, stroking)) = classify_operator(operator) else {
            continue;
        };
        let operator_location = sequence.localize(op.operator_range)?;
        if operator_location.disposition != IndirectObjectEditDisposition::InPlaceMutation {
            continue;
        }
        let tally = &mut tallies[operator_location.occurrence_index];
        // The walker already guarantees the shortcut's operand count and finite
        // numbers (anything else refused the page above); the `[0,1]` range
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
        // Fail-closed /Default* interlock: raw device bytes are honest to read
        // and emit only when BOTH the source and the emitted destination
        // family are proven Absent or Identity. Runs after selector and route
        // (those keep their old counts) and before black preservation / LCMS.
        if !policy.route_is_raw_device(space, link.destination) {
            tally.skips.default_color_space_unsafe += 1;
            continue;
        }
        let range = op.record_range.into_byte_range();
        let location = sequence.localize(op.record_range)?;
        let Some(execution) = execute_components(operands, space, link, black_preservation) else {
            // Unreachable after the source-space route + walker validation
            // (channel count and range are already guaranteed); leave verbatim.
            continue;
        };
        let replacement = replacement_bytes(&execution.components, link.destination, stroking);
        match execution.attribution {
            ComponentAttribution::BlackPreserved => {
                tally.black_preserved += 1;
                if decoded.get(range.start..range.end) != Some(replacement.as_slice()) {
                    occurrence_plans[location.occurrence_index].push(LocalSplice {
                        range: location.local_range,
                        replacement,
                    });
                }
            }
            ComponentAttribution::Link(link_index) => {
                occurrence_plans[location.occurrence_index].push(LocalSplice {
                    range: location.local_range,
                    replacement,
                });
                tally.converted += 1;
                tally.link_converted[link_index] += 1;
            }
        }
    }

    // Close and consume the plan BEFORE physical reconciliation. This is one
    // ordered candidate pass over retained proof facts, not another content
    // interpretation or selector/resource replay.
    let alias_outcome = plan.finish();
    for (index, tally) in tallies.iter_mut().enumerate() {
        tally.alias_setters_eligible = alias_outcome.eligible_setters[index];
        tally.alias_setters_ineligible = alias_outcome.ineligible_setters[index];
    }
    let alias_tally = execute_alias_epochs(
        alias_outcome,
        sequence,
        routing,
        black_preservation,
        &mut occurrence_plans,
    )?;

    let plans = sequence.reconcile(occurrence_plans)?;
    let mut content_objects = Vec::new();
    let mut total = PageTally {
        link_converted: vec![0; routing.links().len()],
        ..PageTally::default()
    };
    let mut seen = BTreeSet::new();
    for (index, tally) in tallies.iter().enumerate() {
        let object = sequence.occurrence_object(index)?;
        if !seen.insert(object)
            || sequence.occurrence_disposition(index)?
                != IndirectObjectEditDisposition::InPlaceMutation
        {
            continue;
        }
        content_objects.push(object);
        total.converted += tally.converted;
        total.black_preserved += tally.black_preserved;
        total.alias_setters_eligible += tally.alias_setters_eligible;
        total.alias_setters_ineligible += tally.alias_setters_ineligible;
        add_operator_skips(&mut total.skips, &tally.skips);
        for (slot, count) in total.link_converted.iter_mut().zip(&tally.link_converted) {
            *slot += count;
        }
    }
    total.converted += alias_tally.operators_converted;
    total.black_preserved += alias_tally.black_preserved;
    for (slot, count) in total
        .link_converted
        .iter_mut()
        .zip(&alias_tally.link_converted)
    {
        *slot += count;
    }
    let form_xobject_refusal_counts = analyzer.borrow_mut().take_page_refusal_counts();
    Some(PageSequenceEdit {
        plans,
        metadata: ConvertedPage {
            page_index,
            content_objects,
            operators_converted: total.converted,
            black_preserved: total.black_preserved,
            resource_alias_setters_eligible: total.alias_setters_eligible,
            resource_alias_setters_ineligible: total.alias_setters_ineligible,
            resource_alias_candidates_converted: alias_tally.converted,
            resource_alias_candidates_refused: alias_tally.refused,
            operator_skips: total.skips,
            form_xobject_refusal_counts,
            links: link_counts(routing, &total.link_converted),
        },
    })
}

/// Dry-run every structurally executable alias root, propagate any
/// non-executability through shared physical records, then publish only whole
/// executable connected components into the existing occurrence plans.
#[allow(clippy::too_many_lines)]
fn execute_alias_epochs(
    outcome: AliasEpochOutcome,
    sequence: &PageContentSequence,
    routing: &LinkRouting,
    black_preservation: BlackPreservationPolicy,
    occurrence_plans: &mut [Vec<LocalSplice>],
) -> Option<AliasExecutionTally> {
    let epochs = outcome.epochs;
    let mut root_records = vec![BTreeSet::<PhysicalCandidateKey>::new(); epochs.len()];
    let mut record_roots = BTreeMap::<PhysicalCandidateKey, BTreeSet<usize>>::new();
    let mut root_failed = vec![false; epochs.len()];

    // Build the complete deterministic root-to-record relation before any
    // execution result can authorize a splice.
    for (root, epoch) in epochs.iter().enumerate() {
        for candidate in &epoch.candidates {
            let Some(object) = sequence.occurrence_object(candidate.occurrence_index) else {
                root_failed[root] = true;
                continue;
            };
            if candidate.local_range.start >= candidate.local_range.end {
                root_failed[root] = true;
                continue;
            }
            let key = (
                object,
                candidate.local_range.start,
                candidate.local_range.end,
            );
            root_records[root].insert(key);
            record_roots.entry(key).or_default().insert(root);
        }
    }

    let mut root_candidates: Vec<Option<Vec<AliasCandidateLocation>>> =
        epochs.iter().map(|_| None).collect();
    let mut record_executions =
        BTreeMap::<PhysicalCandidateKey, (Vec<u8>, ComponentAttribution, usize)>::new();
    for (root, epoch) in epochs.iter().enumerate() {
        let initially_executable = matches!(epoch.status, EpochStatus::Closed)
            && epoch.has_consumer
            && epoch.route.is_some();
        if !initially_executable || root_failed[root] || epoch.candidates.is_empty() {
            root_failed[root] = true;
            continue;
        }
        let Some(route) = epoch.route else {
            root_failed[root] = true;
            continue;
        };
        let Some(link) = routing.links().get(route.link_index) else {
            root_failed[root] = true;
            continue;
        };
        if link.index != route.link_index {
            root_failed[root] = true;
            continue;
        }
        if link.destination != route.destination {
            root_failed[root] = true;
            continue;
        }
        if routing.route(epoch.source).map(|item| item.index) != Some(route.link_index) {
            root_failed[root] = true;
            continue;
        }

        let stroking = epoch.side == LaneSide::Stroking;
        let mut temporary = Vec::with_capacity(epoch.candidates.len());
        let mut root_executions =
            BTreeMap::<PhysicalCandidateKey, (Vec<u8>, ComponentAttribution)>::new();
        for candidate in &epoch.candidates {
            let Some(object) = sequence.occurrence_object(candidate.occurrence_index) else {
                root_failed[root] = true;
                break;
            };
            if !candidate.selector_matched {
                root_failed[root] = true;
                break;
            }
            let Some(execution) = execute_components(
                &candidate.components,
                epoch.source,
                link,
                black_preservation,
            ) else {
                root_failed[root] = true;
                break;
            };
            let key = (
                object,
                candidate.local_range.start,
                candidate.local_range.end,
            );
            let replacement = replacement_bytes(&execution.components, route.destination, stroking);
            if let Some((known_replacement, known_attribution)) = root_executions.get(&key) {
                if known_replacement != &replacement || known_attribution != &execution.attribution
                {
                    root_failed[root] = true;
                    break;
                }
            } else {
                root_executions.insert(key, (replacement, execution.attribution));
            }
            temporary.push(AliasCandidateLocation {
                key,
                occurrence_index: candidate.occurrence_index,
                range: candidate.local_range,
            });
        }
        if root_failed[root] {
            continue;
        }
        for (key, (replacement, attribution)) in root_executions {
            if let Some((known_replacement, known_attribution, known_root)) =
                record_executions.get(&key)
            {
                if known_replacement != &replacement || known_attribution != &attribution {
                    root_failed[root] = true;
                    root_failed[*known_root] = true;
                }
            } else {
                record_executions.insert(key, (replacement, attribution, root));
            }
        }
        root_candidates[root] = Some(temporary);
    }

    // One queue traversal computes the fixed point across root-record-root
    // adjacency. Closed no-consumer roots seed non-executability but remain
    // silent when their retained records are unique.
    let mut queue: VecDeque<usize> = root_failed
        .iter()
        .enumerate()
        .filter_map(|(root, failed)| failed.then_some(root))
        .collect();
    while let Some(root) = queue.pop_front() {
        for key in &root_records[root] {
            let owners = record_roots.get(key)?;
            for &owner in owners {
                if !root_failed[owner] {
                    root_failed[owner] = true;
                    queue.push_back(owner);
                }
            }
        }
    }

    let mut tally = AliasExecutionTally {
        link_converted: vec![0; routing.links().len()],
        ..AliasExecutionTally::default()
    };
    let mut converted_keys = BTreeSet::new();
    let mut staged_occurrences = BTreeSet::new();
    for root in 0..epochs.len() {
        if root_failed[root] {
            continue;
        }
        let candidates = root_candidates[root].as_ref()?;
        for candidate in candidates {
            let (replacement, attribution, _) = record_executions.get(&candidate.key)?;
            let occurrence_key = (
                candidate.occurrence_index,
                candidate.range.start,
                candidate.range.end,
            );
            if staged_occurrences.insert(occurrence_key) {
                occurrence_plans
                    .get_mut(candidate.occurrence_index)?
                    .push(LocalSplice {
                        range: candidate.range,
                        replacement: replacement.clone(),
                    });
            }
            if converted_keys.insert(candidate.key) {
                tally.converted += 1;
                match *attribution {
                    ComponentAttribution::BlackPreserved => tally.black_preserved += 1,
                    ComponentAttribution::Link(link_index) => {
                        tally.operators_converted += 1;
                        *tally.link_converted.get_mut(link_index)? += 1;
                    }
                }
            }
        }
    }
    tally.refused = record_roots
        .iter()
        .filter(|(key, roots)| {
            !converted_keys.contains(key)
                && roots.iter().any(|&root| {
                    epochs[root].has_consumer
                        || matches!(epochs[root].status, EpochStatus::Refused { .. })
                })
        })
        .count();
    Some(tally)
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

/// Add one stream's operator-skip counts into a per-page aggregate.
const fn add_operator_skips(total: &mut OperatorSkipCounts, part: &OperatorSkipCounts) {
    total.no_matching_link += part.no_matching_link;
    total.wrong_operand_count += part.wrong_operand_count;
    total.non_number_operand += part.non_number_operand;
    total.operand_out_of_range += part.operand_out_of_range;
    total.selector_excluded += part.selector_excluded;
    total.default_color_space_unsafe += part.default_color_space_unsafe;
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
