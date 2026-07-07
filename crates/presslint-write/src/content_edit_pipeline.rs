//! Shared page-content decode -> edit -> encode -> whole-stream write pipeline.
//!
//! T136 makes this pipeline MULTI-content-stream aware: a page's `/Contents` may
//! name more than one content-stream object, and each object is located,
//! ownership-gated, decoded, edited, re-encoded, and emitted as its own
//! [`PlannedDirtyObject`] — N dirty objects in ONE incremental revision. The
//! per-stream LOCATION step lives in [`crate::content_stream_plan`]; this module
//! owns the per-stream EDIT step and the revision assembly.
//!
//! Two stream modes are exposed. [`edit_page_content_incremental`] keeps the
//! legacy single-stream taxonomy (a >1-stream page is skipped whole) for callers
//! that have not adopted the per-stream model. The page-aware and re-encode
//! callers drive [`StreamMode::MultiStream`] and edit every stream object.

use std::collections::{BTreeMap, BTreeSet};

use presslint_actions::{
    IncrementalRevisionPlan, MutationBoundary, PlannedDirtyObject, PlannedValueProvenance,
};
use presslint_pdf::{
    ContentStreamDataExtentInspection, ContentStreamFilterClassification, DictionaryEntrySpan,
    DictionaryValueKind, DocumentAccessBackend, DocumentAccessError, DocumentAccessRejection,
    DocumentPageContentExtentInspection, DocumentPageContentExtentResult,
    DocumentPageExtGStateResourcesInspection, DocumentPageExtGStateResourcesInspectionError,
    DocumentPageTransparencyGroupsInspection, DocumentPageTransparencyGroupsInspectionError,
    FlateDecodeParameters, FlateDecodeParametersResolution, IndirectObjectEditDecision,
    IndirectObjectEditDisposition, IndirectRef, ObjectLookup, PageExtGStateResourcesInspection,
    PageTransparencyGroupInspection, classify_content_stream_filter, content_stream_data_slice,
    decide_indirect_object_edit, decode_flate_stream, encode_flate_stream, inspect_document_access,
    inspect_document_page_content_extents_with_lookup,
    inspect_document_page_extgstate_resources_with_lookup,
    inspect_document_page_transparency_groups_with_lookup, inspect_indirect_object_dictionary,
    resolve_flate_decode_parameters,
};
use presslint_syntax::{serialize_tokens_unmodified, tokenize};
use presslint_types::{ByteRange, PageIndex};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError,
    content_stream_plan::{
        LocatedContentStream, PageStreamsPlan, StreamMode, StreamOutcome, page_index_of,
        plan_page_streams,
    },
    stream_object_body::build_stream_object_body,
    write_incremental_revision, write_incremental_revision_plan,
};

const LENGTH_KEY: &[u8] = b"/Length";

/// Upper bound on decoded and re-encoded content-stream buffers.
const MAX_CONTENT_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// Which pages a content-stream edit should visit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "pages", rename_all = "snake_case")]
pub enum PageSelection {
    /// Every enumerated document page.
    All,
    /// A specific set of zero-based document page indexes.
    Indices(Vec<PageIndex>),
}

/// Filter path used to re-encode an edited page's content stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineFilterKind {
    Raw,
    Flate,
}

/// Result of an edit callback over decoded content bytes.
pub enum EditedContent {
    Unchanged,
    Rejected(PipelineSkipReason),
    Rewritten { decoded: Vec<u8>, edit_count: usize },
}

/// Report for one edited content-stream OBJECT (one stream of a page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineEditedPage {
    pub page_index: PageIndex,
    /// Zero-based source-order ordinal of this stream within the page's
    /// `/Contents` (always `0` for a single-stream page).
    pub stream_ordinal: usize,
    pub content_object: IndirectRef,
    pub filter_kind: PipelineFilterKind,
    pub edit_count: usize,
}

/// Structured skip for one content-stream slot (or a whole non-editable page).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelinePageSkip {
    pub page_index: PageIndex,
    /// Zero-based source-order ordinal of the skipped slot (`0` for a whole-page
    /// skip that carries no per-stream detail).
    pub stream_ordinal: usize,
    pub content_object: Option<IndirectRef>,
    pub reason: PipelineSkipReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineSkipReason {
    MultipleContentStreams {
        count: usize,
    },
    NoContentStream,
    CompressedContentObject {
        object_stream_number: usize,
        index_within_object_stream: usize,
    },
    IndirectLength,
    MissingOrDuplicateLength,
    NonDirectNumericLength {
        value_kind: DictionaryValueKind,
    },
    UnsupportedFilter,
    PredictorFlate {
        predictor: u16,
    },
    ContentRoundTripMismatch,
    OwnershipNotInPlace {
        occurrences: usize,
        disposition: IndirectObjectEditDisposition,
    },
    /// A page-level preflight hook poisoned the WHOLE page: one or more of its
    /// decodable content streams contains a `gs` (`ExtGState` set) operator, so the
    /// page is left byte-verbatim (the interim overprint/transparency guard, T140).
    #[allow(dead_code)]
    ExtGStatePresent,
    /// A page-level preflight hook found active or unknowable `ExtGState` safety
    /// parameters in `gs` resources used by the page content. Deprecated
    /// `ExtGStatePresent` is retained for compatibility but the converter now
    /// emits this precision reason.
    ExtGStateUnsafe {
        overprint: bool,
        transparency: bool,
        unresolved: bool,
        unclassified: bool,
        gs_count: u32,
    },
    TransparencyGroupUnsafe {
        transparency: bool,
        unresolved: bool,
        unclassified: bool,
    },
    Unchanged,
}

/// Whole-page decision returned by a pipeline preflight hook, evaluated BEFORE any
/// dirty object is emitted for a page.
///
/// The hook is GENERIC and OPT-IN: the pipeline decodes a page's editable content
/// streams once and hands them to the caller-supplied closure, which either lets
/// the normal per-stream edit loop run ([`PagePreflight::Continue`]) or poisons
/// the ENTIRE page ([`PagePreflight::SkipPage`], emit one whole-page skip and
/// convert nothing). The pipeline itself stays scanner-agnostic; the converter
/// supplies the `gs`-presence scanner.
pub enum PagePreflight {
    /// Run the normal per-stream edit loop for this page.
    Continue,
    /// Poison the whole page: emit one [`PipelinePageSkip`] with this reason and
    /// edit none of the page's streams.
    SkipPage(PipelineSkipReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPageContentOutput {
    pub bytes: Vec<u8>,
    /// Edited content-stream objects, ordered by `(page_index, stream_ordinal)`.
    pub edited: Vec<PipelineEditedPage>,
    /// Skipped content-stream slots and whole-page skips, in `(page_index,
    /// stream_ordinal)` source order.
    pub skipped: Vec<PipelinePageSkip>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditPageContentError {
    EmptyRequest,
    Open {
        error: Box<DocumentAccessError>,
    },
    PageIndexOutOfRange {
        page_index: PageIndex,
        page_count: usize,
    },
    Write {
        error: Box<WriteError>,
    },
    Plan {
        error: Box<PlannedWriteError>,
    },
}

/// Single-content-stream content edit: a page with more than one content stream
/// is skipped whole as [`PipelineSkipReason::MultipleContentStreams`].
///
/// Kept so [`crate::content_color_rewrite`] (whose skip taxonomy pins the
/// single-stream shape) stays byte-for-byte unchanged. Callers that want
/// per-stream editing drive [`edit_page_content_incremental_indexed`] with
/// [`StreamMode::MultiStream`].
pub fn edit_page_content_incremental<F>(
    input: &[u8],
    pages: &PageSelection,
    edit: F,
) -> Result<EditPageContentOutput, EditPageContentError>
where
    F: Fn(&[u8]) -> EditedContent,
{
    edit_page_content_incremental_indexed(input, pages, StreamMode::SingleOnly, |_page, decoded| {
        edit(decoded)
    })
}

/// Page-aware, MULTI-stream-capable content edit: the edit closure receives the
/// zero-based document [`PageIndex`] of the page whose decoded content-stream
/// bytes it is editing (the same page index is passed once per content-stream
/// object of that page), so callers such as the selector-targeted colour
/// converter can evaluate per-page predicates.
///
/// Each content-stream object of every selected page is located
/// ([`plan_page_streams`]), ownership-gated
/// ([`decide_indirect_object_edit`]) per object, decoded / edited / re-encoded
/// independently, and emitted as one [`PlannedDirtyObject`]. All resulting dirty
/// objects are written in ONE appended incremental revision; a content object
/// reached more than once is edited and reported once and never yields two dirty
/// objects with the same number.
pub fn edit_page_content_incremental_indexed<F>(
    input: &[u8],
    pages: &PageSelection,
    mode: StreamMode,
    edit: F,
) -> Result<EditPageContentOutput, EditPageContentError>
where
    F: Fn(PageIndex, &[u8]) -> EditedContent,
{
    // Existing callers stay behaviourally identical: a no-op preflight that never
    // poisons a page and does not pay the preflight decode pass.
    edit_page_content_incremental_indexed_inner(input, pages, mode, None, edit)
}

/// Page-aware, MULTI-stream-capable content edit with a GENERIC, OPT-IN page-level
/// PREFLIGHT hook: before any dirty object is emitted for a selected page, the
/// pipeline decodes that page's editable content streams once and calls
/// `preflight(page_index, &decoded_streams)`. On [`PagePreflight::SkipPage`] the
/// whole page is left byte-verbatim and reported as one [`PipelinePageSkip`]
/// (`stream_ordinal: 0`, `content_object: None`); on [`PagePreflight::Continue`]
/// the existing per-stream edit loop runs unchanged.
///
/// The preflight is deliberately whole-page (ISO 32000 §7.8.2: a page's content
/// streams share graphics state), so a poisoned page converts NONE of its streams
/// even when only one carries the poison operator. The decoded-stream slice passed
/// to the hook contains only the streams that decode successfully (an undecodable
/// stream is never edited either); this is the interim double-decode (preflight
/// scan then edit decode) accepted for T140, bounded by [`MAX_CONTENT_STREAM_BYTES`].
///
/// # Errors
///
/// Returns [`EditPageContentError`] under the same conditions as
/// [`edit_page_content_incremental_indexed`].
pub fn edit_page_content_incremental_indexed_with_preflight<P, F>(
    input: &[u8],
    pages: &PageSelection,
    mode: StreamMode,
    preflight: P,
    edit: F,
) -> Result<EditPageContentOutput, EditPageContentError>
where
    P: Fn(
        PageIndex,
        Option<&PageExtGStateResourcesInspection>,
        Option<&PageTransparencyGroupInspection>,
        &[Vec<u8>],
    ) -> PagePreflight,
    F: Fn(PageIndex, &[u8]) -> EditedContent,
{
    edit_page_content_incremental_indexed_inner(input, pages, mode, Some(&preflight), edit)
}

fn edit_page_content_incremental_indexed_inner<F>(
    input: &[u8],
    pages: &PageSelection,
    mode: StreamMode,
    preflight: Option<&PagePreflightCallback<'_>>,
    edit: F,
) -> Result<EditPageContentOutput, EditPageContentError>
where
    F: Fn(PageIndex, &[u8]) -> EditedContent,
{
    let access = inspect_document_access(input).map_err(|error| EditPageContentError::Open {
        error: Box::new(error),
    })?;
    let lookup = lookup_from_backend(&access.backend);
    let document = inspect_document_page_content_extents_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .map_err(|error| EditPageContentError::Open {
        error: Box::new(DocumentAccessError {
            byte_len: input.len(),
            reason: DocumentAccessRejection::PageTreeLeaves { error: error.error },
        }),
    })?;
    let page_count = document.pages.len();
    let selected = select_indices(pages, page_count)?;
    let owners = content_object_owners(&document.pages);
    let extgstate_document = inspect_extgstate_document_for_preflight(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
        preflight.is_some(),
    );
    let group_document = inspect_transparency_group_document_for_preflight(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
        preflight.is_some(),
    );

    let mut edited = Vec::new();
    let mut skipped = Vec::new();
    let mut dirty_objects: Vec<PlannedDirtyObject> = Vec::new();

    for index in selected {
        let page = &document.pages[index];
        match plan_page_streams(page, mode) {
            PageStreamsPlan::PageSkip {
                content_object,
                reason,
            } => skipped.push(PipelinePageSkip {
                page_index: page_index_of(page),
                stream_ordinal: 0,
                content_object,
                reason,
            }),
            PageStreamsPlan::Streams(outcomes) => {
                if let Some(preflight) = &preflight {
                    if let Some(skip) = preflight_skip_for_page(
                        input,
                        index,
                        page,
                        &outcomes,
                        *preflight,
                        &extgstate_document,
                        &group_document,
                    ) {
                        skipped.push(skip);
                        continue;
                    }
                }
                for outcome in outcomes {
                    match outcome {
                        StreamOutcome::Skip {
                            stream_ordinal,
                            content_object,
                            reason,
                        } => skipped.push(PipelinePageSkip {
                            page_index: page_index_of(page),
                            stream_ordinal,
                            content_object,
                            reason,
                        }),
                        StreamOutcome::Located(located) => {
                            match plan_stream(input, &located, &owners, &edit) {
                                StreamPlan::Edit { report, planned } => {
                                    edited.push(report);
                                    dirty_objects.push(planned);
                                }
                                StreamPlan::Skip(skip) => skipped.push(skip),
                            }
                        }
                    }
                }
            }
        }
    }

    // DUPLICATE-DIRTY-OBJECT SAFETY: merge any dirty objects that share an object
    // number down to one before plan building (the plan rejects duplicates). The
    // per-page location dedup already guarantees uniqueness within a page and the
    // ownership gate blocks cross-page sharing, so this is a defensive net that in
    // practice removes nothing; keeping the first is exact because same-object
    // edits are identical.
    merge_duplicate_dirty_objects(&mut dirty_objects);

    let bytes = write_dirty_objects(input, dirty_objects)?;

    edited.sort_by_key(|report| (report.page_index.0, report.stream_ordinal));

    Ok(EditPageContentOutput {
        bytes,
        edited,
        skipped,
    })
}

fn write_dirty_objects(
    input: &[u8],
    dirty_objects: Vec<PlannedDirtyObject>,
) -> Result<Vec<u8>, EditPageContentError> {
    if dirty_objects.is_empty() {
        return write_incremental_revision(input, &[]).map_err(|error| {
            EditPageContentError::Write {
                error: Box::new(error),
            }
        });
    }
    let plan = IncrementalRevisionPlan { dirty_objects };
    write_incremental_revision_plan(input, &plan).map_err(|error| EditPageContentError::Plan {
        error: Box::new(error),
    })
}

type ExtGStateDocumentPreflight = Option<
    Result<DocumentPageExtGStateResourcesInspection, DocumentPageExtGStateResourcesInspectionError>,
>;

type TransparencyGroupDocumentPreflight = Option<
    Result<DocumentPageTransparencyGroupsInspection, DocumentPageTransparencyGroupsInspectionError>,
>;

type PagePreflightCallback<'a> = dyn Fn(
        PageIndex,
        Option<&PageExtGStateResourcesInspection>,
        Option<&PageTransparencyGroupInspection>,
        &[Vec<u8>],
    ) -> PagePreflight
    + 'a;

fn inspect_extgstate_document_for_preflight(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
    enabled: bool,
) -> ExtGStateDocumentPreflight {
    enabled.then(|| {
        inspect_document_page_extgstate_resources_with_lookup(
            input,
            lookup,
            root_node_object_offset,
        )
    })
}

fn extgstate_page_for_preflight(
    document: &ExtGStateDocumentPreflight,
    index: usize,
) -> Result<Option<&PageExtGStateResourcesInspection>, PipelineSkipReason> {
    match document {
        Some(Ok(document)) => Ok(document.pages.get(index)),
        Some(Err(_)) => Err(PipelineSkipReason::ExtGStateUnsafe {
            overprint: false,
            transparency: false,
            unresolved: true,
            unclassified: true,
            gs_count: 0,
        }),
        None => Ok(None),
    }
}

fn inspect_transparency_group_document_for_preflight(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_node_object_offset: usize,
    enabled: bool,
) -> TransparencyGroupDocumentPreflight {
    enabled.then(|| {
        inspect_document_page_transparency_groups_with_lookup(
            input,
            lookup,
            root_node_object_offset,
        )
    })
}

fn transparency_group_page_for_preflight(
    document: &TransparencyGroupDocumentPreflight,
    index: usize,
) -> Result<Option<&PageTransparencyGroupInspection>, PipelineSkipReason> {
    match document {
        Some(Ok(document)) => Ok(document.pages.get(index)),
        Some(Err(_)) => Err(PipelineSkipReason::TransparencyGroupUnsafe {
            transparency: false,
            unresolved: true,
            unclassified: true,
        }),
        None => Ok(None),
    }
}

const fn whole_page_skip(page_index: PageIndex, reason: PipelineSkipReason) -> PipelinePageSkip {
    PipelinePageSkip {
        page_index,
        stream_ordinal: 0,
        content_object: None,
        reason,
    }
}

fn preflight_skip_for_page(
    input: &[u8],
    index: usize,
    page: &DocumentPageContentExtentInspection,
    outcomes: &[StreamOutcome<'_>],
    preflight: &PagePreflightCallback<'_>,
    extgstate_document: &ExtGStateDocumentPreflight,
    group_document: &TransparencyGroupDocumentPreflight,
) -> Option<PipelinePageSkip> {
    let page_index = page_index_of(page);
    let extgstate_page = match extgstate_page_for_preflight(extgstate_document, index) {
        Ok(page) => page,
        Err(reason) => return Some(whole_page_skip(page_index, reason)),
    };
    let group_page = match transparency_group_page_for_preflight(group_document, index) {
        Ok(page) => page,
        Err(reason) => return Some(whole_page_skip(page_index, reason)),
    };
    run_page_preflight(
        input,
        page_index,
        extgstate_page,
        group_page,
        outcomes,
        preflight,
    )
}

fn run_page_preflight(
    input: &[u8],
    page_index: PageIndex,
    extgstate_page: Option<&PageExtGStateResourcesInspection>,
    group_page: Option<&PageTransparencyGroupInspection>,
    outcomes: &[StreamOutcome<'_>],
    preflight: &PagePreflightCallback<'_>,
) -> Option<PipelinePageSkip> {
    // PAGE-LEVEL PREFLIGHT: decode this page's located (editable) streams ONCE
    // and let the hook poison the whole page before any dirty object is emitted.
    // Undecodable/skipped slots are excluded from the scan (they are never
    // edited); per-stream skips are still reported by the edit loop on Continue.
    let decoded_streams: Vec<Vec<u8>> = outcomes
        .iter()
        .filter_map(|outcome| match outcome {
            StreamOutcome::Located(located) => decode_located_stream_for_preflight(input, located),
            StreamOutcome::Skip { .. } => None,
        })
        .collect();
    if let PagePreflight::SkipPage(reason) =
        preflight(page_index, extgstate_page, group_page, &decoded_streams)
    {
        Some(PipelinePageSkip {
            page_index,
            stream_ordinal: 0,
            content_object: None,
            reason,
        })
    } else {
        None
    }
}

const fn lookup_from_backend(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
    match backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    }
}

fn select_indices(
    pages: &PageSelection,
    page_count: usize,
) -> Result<Vec<usize>, EditPageContentError> {
    match pages {
        PageSelection::All => Ok((0..page_count).collect()),
        PageSelection::Indices(indices) => {
            if indices.is_empty() {
                return Err(EditPageContentError::EmptyRequest);
            }
            let mut ordered: Vec<usize> = Vec::new();
            for page_index in indices {
                let index = page_index.0 as usize;
                if index >= page_count {
                    return Err(EditPageContentError::PageIndexOutOfRange {
                        page_index: *page_index,
                        page_count,
                    });
                }
                if !ordered.contains(&index) {
                    ordered.push(index);
                }
            }
            ordered.sort_unstable();
            Ok(ordered)
        }
    }
}

/// Build the per-content-object owner map from every enumerated page's
/// `/Contents` references.
///
/// The key is the content-stream object; the value lists one owning leaf per
/// direct `/Contents` occurrence (a page naming an object twice pushes it twice,
/// so the ownership decision — which deduplicates — still proves single use).
fn content_object_owners(
    pages: &[DocumentPageContentExtentInspection],
) -> BTreeMap<IndirectRef, Vec<IndirectRef>> {
    let mut owners: BTreeMap<IndirectRef, Vec<IndirectRef>> = BTreeMap::new();
    for page in pages {
        // Only `Inspected` pages contribute `/Contents` owners here. Contents-failed
        // and both compressed-leaf variants do not: the write pipeline never edits a
        // compressed leaf (its dictionary has no editable source offset), so a
        // `CompressedLeafInspected` page's resolved references are intentionally not
        // threaded into ownership decisions by this READ-ONLY slice.
        if let DocumentPageContentExtentResult::Inspected { contents, .. } = &page.result {
            for reference in &contents.contents {
                owners
                    .entry(reference.reference)
                    .or_default()
                    .push(page.leaf.reference);
            }
        }
    }
    owners
}

/// Drop dirty objects that repeat an object number already planned, keeping the
/// first. Same-object edits are identical, so this is a merge, not a loss.
fn merge_duplicate_dirty_objects(dirty_objects: &mut Vec<PlannedDirtyObject>) {
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    dirty_objects.retain(|dirty| seen.insert(dirty.reference.object_number));
}

/// Plan for one located content-stream object.
enum StreamPlan {
    Edit {
        report: PipelineEditedPage,
        planned: PlannedDirtyObject,
    },
    Skip(PipelinePageSkip),
}

/// Ownership-gate, decode, edit, re-encode, and build one located stream object.
fn plan_stream<F>(
    input: &[u8],
    located: &LocatedContentStream<'_>,
    owners: &BTreeMap<IndirectRef, Vec<IndirectRef>>,
    edit: &F,
) -> StreamPlan
where
    F: Fn(PageIndex, &[u8]) -> EditedContent,
{
    let page_index = located.page_index;
    let stream_ordinal = located.stream_ordinal;
    let content_object = located.content_object;
    let skip = |reason: PipelineSkipReason| {
        StreamPlan::Skip(PipelinePageSkip {
            page_index,
            stream_ordinal,
            content_object: Some(content_object),
            reason,
        })
    };

    // OWNERSHIP unit = the content-stream OBJECT: an object referenced by more than
    // one page (or twice with distinct owners) is not a single-use in-place
    // mutation, so it is skipped (no private-copy yet).
    let consumers = owners
        .get(&content_object)
        .map_or([].as_slice(), Vec::as_slice);
    let occurrences = consumers.len();
    let decision = decide_indirect_object_edit(content_object, consumers.iter().copied());
    if decision.disposition != IndirectObjectEditDisposition::InPlaceMutation {
        return skip(PipelineSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition: decision.disposition,
        });
    }

    let Ok(dictionary) = inspect_indirect_object_dictionary(input, located.object_byte_offset)
    else {
        return skip(PipelineSkipReason::NoContentStream);
    };
    let length_value_range = match find_direct_length(input, &dictionary.entries) {
        Ok(range) => range,
        Err(reason) => return skip(reason),
    };

    let filter = match classify_filter(input, located.object_byte_offset) {
        Ok(filter) => filter,
        Err(reason) => return skip(reason),
    };
    let Ok(stream_data) = content_stream_data_slice(input, located.extent) else {
        return skip(PipelineSkipReason::NoContentStream);
    };

    let new_data = match edit_stream_data(page_index, stream_data, filter, edit) {
        Ok(Some(new_data)) => new_data,
        Ok(None) => return skip(PipelineSkipReason::Unchanged),
        Err(reason) => return skip(reason),
    };

    let body = build_stream_object_body(
        input,
        dictionary.dictionary_open_byte_offset,
        dictionary.after_dictionary_close_byte_offset,
        length_value_range,
        &new_data.encoded,
    );
    let boundary = whole_stream_boundary(content_object, located.extent, &decision);

    StreamPlan::Edit {
        report: PipelineEditedPage {
            page_index,
            stream_ordinal,
            content_object,
            filter_kind: filter,
            edit_count: new_data.edit_count,
        },
        planned: PlannedDirtyObject {
            reference: content_object,
            boundaries: vec![boundary],
            body_bytes: body,
        },
    }
}

fn find_direct_length(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
) -> Result<ByteRange, PipelineSkipReason> {
    let mut found: Option<&DictionaryEntrySpan> = None;
    for entry in entries {
        if input.get(entry.key_range.start..entry.key_range.end) == Some(LENGTH_KEY) {
            if found.is_some() {
                return Err(PipelineSkipReason::MissingOrDuplicateLength);
            }
            found = Some(entry);
        }
    }
    let entry = found.ok_or(PipelineSkipReason::MissingOrDuplicateLength)?;
    match entry.value_kind {
        DictionaryValueKind::NumberLike => Ok(ByteRange {
            start: entry.value_range.start,
            end: entry.value_range.end,
        }),
        DictionaryValueKind::IndirectReferenceLike => Err(PipelineSkipReason::IndirectLength),
        value_kind => Err(PipelineSkipReason::NonDirectNumericLength { value_kind }),
    }
}

fn classify_filter(
    input: &[u8],
    object_byte_offset: usize,
) -> Result<PipelineFilterKind, PipelineSkipReason> {
    match classify_content_stream_filter(input, object_byte_offset) {
        Ok(ContentStreamFilterClassification::Uncompressed) => Ok(PipelineFilterKind::Raw),
        Ok(ContentStreamFilterClassification::Flate) => {
            match resolve_flate_decode_parameters(input, object_byte_offset) {
                Ok(FlateDecodeParametersResolution::Resolved { parameters, .. }) => {
                    if parameters.predictor == FlateDecodeParameters::default().predictor {
                        Ok(PipelineFilterKind::Flate)
                    } else {
                        Err(PipelineSkipReason::PredictorFlate {
                            predictor: parameters.predictor,
                        })
                    }
                }
                Ok(FlateDecodeParametersResolution::UnsupportedArrayParms { .. }) | Err(_) => {
                    Err(PipelineSkipReason::UnsupportedFilter)
                }
            }
        }
        Ok(
            ContentStreamFilterClassification::UnsupportedFilter { .. }
            | ContentStreamFilterClassification::UnsupportedFilterChain { .. },
        )
        | Err(_) => Err(PipelineSkipReason::UnsupportedFilter),
    }
}

/// Best-effort decode of one located content stream for the page-level preflight
/// scan. Returns the decoded bytes, or `None` when the stream's filter is
/// unsupported, its data slice is unavailable, or `FlateDecode` fails — such a
/// stream is never converted by the per-stream edit loop either, so excluding it
/// keeps the preflight scan aligned with what the loop would actually edit.
fn decode_located_stream_for_preflight(
    input: &[u8],
    located: &LocatedContentStream<'_>,
) -> Option<Vec<u8>> {
    let filter = classify_filter(input, located.object_byte_offset).ok()?;
    let stream_data = content_stream_data_slice(input, located.extent).ok()?;
    match filter {
        PipelineFilterKind::Raw => Some(stream_data.to_vec()),
        PipelineFilterKind::Flate => decode_flate_stream(
            stream_data,
            FlateDecodeParameters::default(),
            MAX_CONTENT_STREAM_BYTES,
        )
        .ok(),
    }
}

struct EditedStreamData {
    encoded: Vec<u8>,
    edit_count: usize,
}

fn edit_stream_data<F>(
    page_index: PageIndex,
    stream_data: &[u8],
    filter: PipelineFilterKind,
    edit: &F,
) -> Result<Option<EditedStreamData>, PipelineSkipReason>
where
    F: Fn(PageIndex, &[u8]) -> EditedContent,
{
    let decoded = match filter {
        PipelineFilterKind::Raw => stream_data.to_vec(),
        PipelineFilterKind::Flate => decode_flate_stream(
            stream_data,
            FlateDecodeParameters::default(),
            MAX_CONTENT_STREAM_BYTES,
        )
        .map_err(|_| PipelineSkipReason::ContentRoundTripMismatch)?,
    };

    require_round_trip(&decoded)?;
    let (edited, edit_count) = match edit(page_index, &decoded) {
        EditedContent::Unchanged => return Ok(None),
        EditedContent::Rejected(reason) => return Err(reason),
        EditedContent::Rewritten {
            decoded,
            edit_count,
        } => (decoded, edit_count),
    };
    require_round_trip(&edited)?;

    let encoded = match filter {
        PipelineFilterKind::Raw => edited,
        PipelineFilterKind::Flate => encode_flate_stream(&edited, MAX_CONTENT_STREAM_BYTES)
            .map_err(|_| PipelineSkipReason::ContentRoundTripMismatch)?,
    };
    Ok(Some(EditedStreamData {
        encoded,
        edit_count,
    }))
}

fn require_round_trip(decoded: &[u8]) -> Result<(), PipelineSkipReason> {
    let tokens = tokenize(decoded).map_err(|_| PipelineSkipReason::ContentRoundTripMismatch)?;
    let serialized = serialize_tokens_unmodified(decoded, &tokens)
        .map_err(|_| PipelineSkipReason::ContentRoundTripMismatch)?;
    if serialized == decoded {
        Ok(())
    } else {
        Err(PipelineSkipReason::ContentRoundTripMismatch)
    }
}

fn whole_stream_boundary(
    content_object: IndirectRef,
    extent: &ContentStreamDataExtentInspection,
    ownership: &IndirectObjectEditDecision,
) -> MutationBoundary {
    MutationBoundary::WholeStream {
        target: content_object,
        stream_data_range: Some(ByteRange {
            start: extent.stream_data_start_byte_offset(),
            end: extent.stream_data_end_byte_offset(),
        }),
        ownership: ownership.clone(),
        value_provenance: PlannedValueProvenance::DerivedFromObject {
            object: content_object,
        },
    }
}
