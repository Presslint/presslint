//! Shared page-content decode -> edit -> encode -> whole-stream write pipeline.

use std::collections::BTreeMap;

use presslint_actions::{
    IncrementalRevisionPlan, MutationBoundary, PlannedDirtyObject, PlannedValueProvenance,
};
use presslint_pdf::{
    ContentStreamDataExtentInspection, ContentStreamDataExtentInspectionRejection,
    ContentStreamFilterClassification, DictionaryEntrySpan, DictionaryValueKind,
    DocumentAccessBackend, DocumentAccessError, DocumentAccessRejection,
    DocumentPageContentExtentInspection, DocumentPageContentExtentResult, FlateDecodeParameters,
    FlateDecodeParametersResolution, IndirectObjectEditDecision, IndirectObjectEditDisposition,
    IndirectRef, ObjectLookup, ObjectLookupLocation, PageContentExtentInspection,
    SkippedPageContentTargetReason, classify_content_stream_filter, content_stream_data_slice,
    decide_indirect_object_edit, decode_flate_stream, encode_flate_stream, inspect_document_access,
    inspect_document_page_content_extents_with_lookup, inspect_indirect_object_dictionary,
    resolve_flate_decode_parameters,
};
use presslint_syntax::{serialize_tokens_unmodified, tokenize};
use presslint_types::{ByteRange, PageIndex};
use serde::{Deserialize, Serialize};

use crate::{
    PlannedWriteError, WriteError, stream_object_body::build_stream_object_body,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelineEditedPage {
    pub page_index: PageIndex,
    pub content_object: IndirectRef,
    pub filter_kind: PipelineFilterKind,
    pub edit_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelinePageSkip {
    pub page_index: PageIndex,
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
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditPageContentOutput {
    pub bytes: Vec<u8>,
    pub edited: Vec<PipelineEditedPage>,
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

pub fn edit_page_content_incremental<F>(
    input: &[u8],
    pages: &PageSelection,
    edit: F,
) -> Result<EditPageContentOutput, EditPageContentError>
where
    F: Fn(&[u8]) -> EditedContent,
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

    let mut edited = Vec::new();
    let mut skipped = Vec::new();
    let mut dirty_objects: Vec<PlannedDirtyObject> = Vec::new();

    for index in selected {
        match plan_page(input, &document.pages[index], &owners, &edit) {
            PagePlan::Edit { report, planned } => {
                edited.push(report);
                dirty_objects.push(planned);
            }
            PagePlan::Skip(skip) => skipped.push(skip),
        }
    }

    let bytes = if dirty_objects.is_empty() {
        write_incremental_revision(input, &[]).map_err(|error| EditPageContentError::Write {
            error: Box::new(error),
        })?
    } else {
        let plan = IncrementalRevisionPlan { dirty_objects };
        write_incremental_revision_plan(input, &plan).map_err(|error| {
            EditPageContentError::Plan {
                error: Box::new(error),
            }
        })?
    };

    edited.sort_by_key(|report| report.page_index.0);

    Ok(EditPageContentOutput {
        bytes,
        edited,
        skipped,
    })
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

fn content_object_owners(
    pages: &[DocumentPageContentExtentInspection],
) -> BTreeMap<IndirectRef, Vec<IndirectRef>> {
    let mut owners: BTreeMap<IndirectRef, Vec<IndirectRef>> = BTreeMap::new();
    for page in pages {
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

enum PagePlan {
    Edit {
        report: PipelineEditedPage,
        planned: PlannedDirtyObject,
    },
    Skip(PipelinePageSkip),
}

struct LocatedStream<'a> {
    content_object: IndirectRef,
    object_byte_offset: usize,
    extent: &'a ContentStreamDataExtentInspection,
}

fn plan_page<F>(
    input: &[u8],
    page: &DocumentPageContentExtentInspection,
    owners: &BTreeMap<IndirectRef, Vec<IndirectRef>>,
    edit: &F,
) -> PagePlan
where
    F: Fn(&[u8]) -> EditedContent,
{
    let page_index = page_index_of(page);
    let located = match locate_single_stream(page) {
        Ok(located) => located,
        Err((content_object, reason)) => {
            return PagePlan::Skip(PipelinePageSkip {
                page_index,
                content_object,
                reason,
            });
        }
    };
    let content_object = located.content_object;
    let skip = |reason: PipelineSkipReason| {
        PagePlan::Skip(PipelinePageSkip {
            page_index,
            content_object: Some(content_object),
            reason,
        })
    };

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

    let new_data = match edit_stream_data(stream_data, filter, edit) {
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

    PagePlan::Edit {
        report: PipelineEditedPage {
            page_index,
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

fn locate_single_stream(
    page: &DocumentPageContentExtentInspection,
) -> Result<LocatedStream<'_>, (Option<IndirectRef>, PipelineSkipReason)> {
    let DocumentPageContentExtentResult::Inspected {
        contents, extents, ..
    } = &page.result
    else {
        return Err((None, PipelineSkipReason::NoContentStream));
    };

    let count = contents.contents.len();
    if count == 0 {
        return Err((None, PipelineSkipReason::NoContentStream));
    }
    if count > 1 || !contents.skipped.is_empty() {
        return Err((None, PipelineSkipReason::MultipleContentStreams { count }));
    }

    match extents.entries.first() {
        Some(PageContentExtentInspection::Located {
            content_reference,
            object_byte_offset,
            extent,
        }) => {
            if matches!(extent, ContentStreamDataExtentInspection::IndirectLength(_)) {
                return Err((
                    Some(content_reference.reference),
                    PipelineSkipReason::IndirectLength,
                ));
            }
            Ok(LocatedStream {
                content_object: content_reference.reference,
                object_byte_offset: *object_byte_offset,
                extent,
            })
        }
        Some(PageContentExtentInspection::Skipped {
            content_reference,
            reason,
        }) => Err((
            Some(content_reference.reference),
            skip_reason_from_target(reason),
        )),
        Some(PageContentExtentInspection::Failed {
            content_reference,
            error,
            ..
        }) => Err((
            Some(content_reference.reference),
            skip_reason_from_extent_failure(&error.reason),
        )),
        None => Err((None, PipelineSkipReason::NoContentStream)),
    }
}

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

struct EditedStreamData {
    encoded: Vec<u8>,
    edit_count: usize,
}

fn edit_stream_data<F>(
    stream_data: &[u8],
    filter: PipelineFilterKind,
    edit: &F,
) -> Result<Option<EditedStreamData>, PipelineSkipReason>
where
    F: Fn(&[u8]) -> EditedContent,
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
    let (edited, edit_count) = match edit(&decoded) {
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

fn page_index_of(page: &DocumentPageContentExtentInspection) -> PageIndex {
    PageIndex(u32::try_from(page.ordinal).unwrap_or(u32::MAX))
}
