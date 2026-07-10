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
//!
//! Raw byte callbacks retain their historical per-stream contract. The direct
//! converter uses a separate private page transaction: unique physical objects
//! decode once, exact occurrence bytes form one logical sequence, and all
//! physical rewrites publish only after global post-edit validation.

use std::collections::{BTreeMap, BTreeSet};

use presslint_actions::{
    IncrementalRevisionPlan, MutationBoundary, PlannedDirtyObject, PlannedValueProvenance,
};
use presslint_pdf::{
    ContentStreamDataExtentInspection, ContentStreamFilterClassification, DictionaryEntrySpan,
    DictionaryValueKind, DocumentAccessBackend, DocumentAccessError, DocumentAccessRejection,
    DocumentPageExtGStateResourcesInspection, DocumentPageExtGStateResourcesInspectionError,
    DocumentPageTransparencyGroupsInspection, DocumentPageTransparencyGroupsInspectionError,
    FlateDecodeParameters, FlateDecodeParametersResolution, IndirectObjectEditDecision,
    IndirectObjectEditDisposition, IndirectRef, ObjectLookup, PageExtGStateResourcesInspection,
    PageTransparencyGroupInspection, classify_content_stream_filter, content_stream_data_slice,
    decode_flate_stream, encode_flate_stream, inspect_document_access,
    inspect_document_page_content_extents_with_lookup,
    inspect_document_page_extgstate_resources_with_lookup,
    inspect_document_page_transparency_groups_with_lookup, inspect_indirect_object_dictionary,
    inspect_object_consumer_index, resolve_flate_decode_parameters,
};
use presslint_syntax::{serialize_tokens_unmodified, tokenize};
use presslint_types::{ByteRange, PageIndex};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError,
    content_object_ownership::ContentObjectOwnershipIndex,
    content_stream_plan::{
        LocatedContentStream, PageStreamsPlan, StreamMode, StreamOutcome, page_index_of,
        plan_page_streams,
    },
    page_content_sequence::{OccurrenceInput, PageContentSequence, PhysicalObjectPlan},
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

/// Result of editing selected pages through the content-stream pipeline.
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

/// Historical raw callback used by the source-compatible per-stream loop.
enum StreamEditCallback<'a> {
    /// Historical raw byte callback (public wrappers).
    Raw(&'a dyn Fn(PageIndex, &[u8]) -> EditedContent),
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
    edit_page_content_incremental_indexed_inner(input, pages, mode, &StreamEditCallback::Raw(&edit))
}

/// Converter-private result for one fully analysed logical page.
pub struct PageSequenceEdit<T> {
    pub plans: Vec<PhysicalObjectPlan>,
    pub metadata: T,
}

/// Converter-private page transaction output. Metadata is published only for
/// pages whose complete physical staging succeeded.
pub struct PageSequenceOutput<T> {
    pub bytes: Vec<u8>,
    pub pages: Vec<T>,
    pub skipped: Vec<PipelinePageSkip>,
}

struct PreparedSequenceObject<'a> {
    located: &'a LocatedContentStream<'a>,
    decoded: Vec<u8>,
    filter: PipelineFilterKind,
    length_value_range: ByteRange,
    dictionary_open: usize,
    dictionary_end: usize,
    occurrences: usize,
    decision: IndirectObjectEditDecision,
}

/// Decode, analyse, validate, encode, and publish each selected page atomically
/// in exact logical `/Contents` order.
#[allow(clippy::too_many_lines)]
pub fn edit_page_content_incremental_sequence<T, P, F>(
    input: &[u8],
    pages: &PageSelection,
    preflight: P,
    edit: F,
) -> Result<PageSequenceOutput<T>, EditPageContentError>
where
    P: Fn(
        PageIndex,
        Option<&PageExtGStateResourcesInspection>,
        Option<&PageTransparencyGroupInspection>,
        &PageContentSequence,
    ) -> Option<PipelineSkipReason>,
    F: Fn(PageIndex, &PageContentSequence) -> Option<PageSequenceEdit<T>>,
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
    let selected = select_indices(pages, document.pages.len())?;
    let ownership = ContentObjectOwnershipIndex::new(
        &document.pages,
        inspect_object_consumer_index(input, &access),
    );
    let extgstate_document = inspect_extgstate_document_for_preflight(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
        true,
    );
    let group_document = inspect_transparency_group_document_for_preflight(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
        true,
    );
    let mut published = Vec::new();
    let mut skipped = Vec::new();
    let mut dirty_objects = Vec::new();

    for index in selected {
        let page = &document.pages[index];
        let page_index = page_index_of(page);
        let outcomes = match plan_page_streams(page, StreamMode::LogicalSequence) {
            PageStreamsPlan::Streams(outcomes) => outcomes,
            PageStreamsPlan::PageSkip {
                content_object,
                reason,
            } => {
                skipped.push(PipelinePageSkip {
                    page_index,
                    stream_ordinal: 0,
                    content_object,
                    reason,
                });
                continue;
            }
        };

        let mut prepared = Vec::new();
        let mut object_indexes: BTreeMap<IndirectRef, usize> = BTreeMap::new();
        let mut occurrence_indexes = Vec::with_capacity(outcomes.len());
        let mut failure = None;
        for outcome in &outcomes {
            let StreamOutcome::Located(located) = outcome else {
                if let StreamOutcome::Skip { reason, .. } = outcome {
                    failure = Some(*reason);
                }
                break;
            };
            if let Some(object_index) = object_indexes.get(&located.content_object) {
                occurrence_indexes.push(*object_index);
                continue;
            }
            match prepare_sequence_object(input, located, &ownership) {
                Ok(object) => {
                    let object_index = prepared.len();
                    object_indexes.insert(located.content_object, object_index);
                    occurrence_indexes.push(object_index);
                    prepared.push(object);
                }
                Err(reason) => {
                    failure = Some(reason);
                    break;
                }
            }
        }
        if let Some(reason) = failure {
            skipped.push(whole_page_skip(page_index, reason));
            continue;
        }

        let inputs: Vec<_> = outcomes
            .iter()
            .zip(&occurrence_indexes)
            .filter_map(|(outcome, object_index)| {
                let StreamOutcome::Located(located) = outcome else {
                    return None;
                };
                let object = &prepared[*object_index];
                Some(OccurrenceInput {
                    stream_ordinal: located.stream_ordinal,
                    content_object: located.content_object,
                    decoded: &object.decoded,
                    disposition: object.decision.disposition,
                })
            })
            .collect();
        let Some(sequence) = PageContentSequence::new(&inputs, MAX_CONTENT_STREAM_BYTES) else {
            skipped.push(whole_page_skip(
                page_index,
                PipelineSkipReason::ContentRoundTripMismatch,
            ));
            continue;
        };
        let extgstate_page = match extgstate_page_for_preflight(&extgstate_document, index) {
            Ok(value) => value,
            Err(reason) => {
                skipped.push(whole_page_skip(page_index, reason));
                continue;
            }
        };
        let group_page = match transparency_group_page_for_preflight(&group_document, index) {
            Ok(value) => value,
            Err(reason) => {
                skipped.push(whole_page_skip(page_index, reason));
                continue;
            }
        };
        if let Some(reason) = preflight(page_index, extgstate_page, group_page, &sequence) {
            skipped.push(whole_page_skip(page_index, reason));
            continue;
        }
        let Some(page_edit) = edit(page_index, &sequence) else {
            skipped.push(whole_page_skip(
                page_index,
                PipelineSkipReason::ContentRoundTripMismatch,
            ));
            continue;
        };

        let mut edited_by_object = BTreeMap::new();
        let mut stage_failed = false;
        for plan in &page_edit.plans {
            let Some(object_index) = object_indexes.get(&plan.content_object).copied() else {
                stage_failed = true;
                break;
            };
            let object = &prepared[object_index];
            if object.decision.disposition != IndirectObjectEditDisposition::InPlaceMutation
                && !plan.splices.is_empty()
            {
                stage_failed = true;
                break;
            }
            if plan.splices.is_empty() {
                continue;
            }
            let mut decoded = object.decoded.clone();
            for splice in plan.splices.iter().rev() {
                if splice.range.end > decoded.len() {
                    stage_failed = true;
                    break;
                }
                decoded.splice(
                    splice.range.start..splice.range.end,
                    splice.replacement.iter().copied(),
                );
            }
            edited_by_object.insert(plan.content_object, decoded);
        }
        let decoded_by_object: BTreeMap<_, _> = prepared
            .iter()
            .map(|object| {
                let content_object = object.located.content_object;
                let decoded = edited_by_object
                    .get(&content_object)
                    .map_or(object.decoded.as_slice(), Vec::as_slice);
                (content_object, decoded)
            })
            .collect();
        if stage_failed || !sequence.validate_edited(&decoded_by_object, MAX_CONTENT_STREAM_BYTES) {
            skipped.push(whole_page_skip(
                page_index,
                PipelineSkipReason::ContentRoundTripMismatch,
            ));
            continue;
        }
        drop(decoded_by_object);
        let mut staged_dirty = Vec::new();
        for plan in &page_edit.plans {
            if plan.splices.is_empty() {
                continue;
            }
            let object = &prepared[object_indexes[&plan.content_object]];
            let Some(decoded) = edited_by_object.remove(&plan.content_object) else {
                stage_failed = true;
                break;
            };
            let encoded = match object.filter {
                PipelineFilterKind::Raw => decoded,
                PipelineFilterKind::Flate => {
                    let Ok(encoded) = encode_flate_stream(&decoded, MAX_CONTENT_STREAM_BYTES)
                    else {
                        stage_failed = true;
                        break;
                    };
                    encoded
                }
            };
            let body = build_stream_object_body(
                input,
                object.dictionary_open,
                object.dictionary_end,
                object.length_value_range,
                &encoded,
            );
            staged_dirty.push(PlannedDirtyObject {
                reference: plan.content_object,
                boundaries: vec![whole_stream_boundary(
                    plan.content_object,
                    object.located.extent,
                    &object.decision,
                )],
                body_bytes: body,
            });
        }
        if stage_failed {
            skipped.push(whole_page_skip(
                page_index,
                PipelineSkipReason::ContentRoundTripMismatch,
            ));
            continue;
        }
        for object in &prepared {
            if object.decision.disposition != IndirectObjectEditDisposition::InPlaceMutation {
                skipped.push(PipelinePageSkip {
                    page_index,
                    stream_ordinal: object.located.stream_ordinal,
                    content_object: Some(object.located.content_object),
                    reason: PipelineSkipReason::OwnershipNotInPlace {
                        occurrences: object.occurrences,
                        disposition: object.decision.disposition,
                    },
                });
            }
        }
        dirty_objects.extend(staged_dirty);
        published.push(page_edit.metadata);
    }
    merge_duplicate_dirty_objects(&mut dirty_objects);
    let bytes = write_dirty_objects(input, dirty_objects)?;
    Ok(PageSequenceOutput {
        bytes,
        pages: published,
        skipped,
    })
}

fn prepare_sequence_object<'a>(
    input: &[u8],
    located: &'a LocatedContentStream<'a>,
    ownership: &ContentObjectOwnershipIndex,
) -> Result<PreparedSequenceObject<'a>, PipelineSkipReason> {
    let dictionary = inspect_indirect_object_dictionary(input, located.object_byte_offset)
        .map_err(|_| PipelineSkipReason::NoContentStream)?;
    let length_value_range = find_direct_length(input, &dictionary.entries)?;
    let filter = classify_filter(input, located.object_byte_offset)?;
    let stream_data = content_stream_data_slice(input, located.extent)
        .map_err(|_| PipelineSkipReason::NoContentStream)?;
    let decoded = match filter {
        PipelineFilterKind::Raw => stream_data.to_vec(),
        PipelineFilterKind::Flate => decode_flate_stream(
            stream_data,
            FlateDecodeParameters::default(),
            MAX_CONTENT_STREAM_BYTES,
        )
        .map_err(|_| PipelineSkipReason::ContentRoundTripMismatch)?,
    };
    let (occurrences, decision) = ownership.decide(located.content_object);
    Ok(PreparedSequenceObject {
        located,
        decoded,
        filter,
        length_value_range,
        dictionary_open: dictionary.dictionary_open_byte_offset,
        dictionary_end: dictionary.after_dictionary_close_byte_offset,
        occurrences,
        decision,
    })
}

fn edit_page_content_incremental_indexed_inner(
    input: &[u8],
    pages: &PageSelection,
    mode: StreamMode,
    edit: &StreamEditCallback<'_>,
) -> Result<EditPageContentOutput, EditPageContentError> {
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
    // One bounded document-wide traversal per request, from the same immutable
    // input/access snapshot used by all subsequent ownership decisions.
    let consumer_inspection = inspect_object_consumer_index(input, &access);
    let ownership = ContentObjectOwnershipIndex::new(&document.pages, consumer_inspection);
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
                            match plan_stream(input, &located, &ownership, edit) {
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
fn plan_stream(
    input: &[u8],
    located: &LocatedContentStream<'_>,
    ownership: &ContentObjectOwnershipIndex,
    edit: &StreamEditCallback<'_>,
) -> StreamPlan {
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
    let (occurrences, decision) = ownership.decide(content_object);
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

/// Encoded replacement bytes and the number of edits applied to one stream.
struct EditedStreamData {
    encoded: Vec<u8>,
    edit_count: usize,
}

fn edit_stream_data(
    page_index: PageIndex,
    stream_data: &[u8],
    filter: PipelineFilterKind,
    edit: &StreamEditCallback<'_>,
) -> Result<Option<EditedStreamData>, PipelineSkipReason> {
    let decoded = match filter {
        PipelineFilterKind::Raw => stream_data.to_vec(),
        PipelineFilterKind::Flate => decode_flate_stream(
            stream_data,
            FlateDecodeParameters::default(),
            MAX_CONTENT_STREAM_BYTES,
        )
        .map_err(|_| PipelineSkipReason::ContentRoundTripMismatch)?,
    };

    let outcome = match edit {
        StreamEditCallback::Raw(edit) => {
            require_round_trip(&decoded)?;
            edit(page_index, &decoded)
        }
    };
    let (edited, edit_count) = match outcome {
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
