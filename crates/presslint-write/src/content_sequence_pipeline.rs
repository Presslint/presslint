//! Logical page-sequence decode -> analyse -> validate -> encode -> publish
//! pipeline for the direct-device / alias-epoch colour converter.
//!
//! This is the DESTINATION half of the mechanical split from
//! [`crate::content_edit_pipeline`]: the raw/indexed per-stream pipeline and the
//! low-level write/encode helpers stay in the source module (reused here behind
//! the smallest `pub(crate)` seams), while the whole-page transaction — exact
//! decoded `/Contents` concatenation, one tokenize/assemble/paint walk, physical
//! occurrence mapping, page-atomic staging, ownership disposition, and the
//! advisory report joins — lives here.
//!
//! Every advisory structural inspection (page colour spaces, `/Default*`,
//! `/XObject`, `/Font`) runs ONCE per request through the same object lookup and
//! is matched to a content page by EXACT page identity — ordinal, leaf page
//! reference, and page object byte offset — never by compacted report vector
//! position. A duplicate reference poisons its slot; a failed inspection or a
//! failed identity join degrades that page's fact to unknown and never adds a
//! page skip. The page `/Font` and `/ExtGState` joins replace the earlier
//! vector-index assumption; the `/ExtGState` report is inspected once here and
//! reused for BOTH the safety preflight and the font-policy edit fact.

use std::collections::BTreeMap;

use presslint_actions::PlannedDirtyObject;
use presslint_pdf::{
    DocumentAccessError, DocumentAccessRejection, DocumentPageTransparencyGroupsInspection,
    DocumentPageTransparencyGroupsInspectionError, FlateDecodeParameters,
    IndirectObjectEditDecision, IndirectObjectEditDisposition, IndirectRef, ObjectLookup,
    PageExtGStateResourcesInspection, PageFontResourcesInspection, PageTransparencyGroupInspection,
    PageXObjectResourcesInspection, content_stream_data_slice, decode_flate_stream,
    encode_flate_stream, inspect_document_access,
    inspect_document_page_color_space_resources_with_lookup,
    inspect_document_page_content_extents_with_lookup,
    inspect_document_page_default_color_spaces_with_lookup,
    inspect_document_page_extgstate_resources_with_lookup,
    inspect_document_page_font_resources_with_lookup,
    inspect_document_page_transparency_groups_with_lookup,
    inspect_document_page_xobject_resources_with_lookup, inspect_indirect_object_dictionary,
    inspect_object_consumer_index,
};
use presslint_types::{ByteRange, PageIndex};

use crate::{
    content_edit_pipeline::{
        EditPageContentError, MAX_CONTENT_STREAM_BYTES, PageSelection, PipelineFilterKind,
        PipelinePageSkip, PipelineSkipReason, classify_filter, find_direct_length,
        lookup_from_backend, merge_duplicate_dirty_objects, select_indices, whole_stream_boundary,
        write_dirty_objects,
    },
    content_object_ownership::ContentObjectOwnershipIndex,
    content_stream_plan::{
        LocatedContentStream, PageStreamsPlan, StreamMode, StreamOutcome, page_index_of,
        plan_page_streams,
    },
    form_clone_set_plan::{
        FormCloneSetPlan, FormCloneSetPlanCounts, commit::build_clone_commit_batch,
    },
    page_content_sequence::{OccurrenceInput, PageContentSequence, PhysicalObjectPlan},
    page_device_space_policy::{PageColorFacts, PageColorFactsIndex},
    stream_object_body::build_stream_object_body,
};

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
///
/// The edit closure receives, for the analysed page: the parsed logical
/// sequence; the page's advisory colour facts (`/Resources /ColorSpace` and
/// `/Default*`, matched by exact leaf page identity); the exact identity-matched
/// page `/XObject` report (or `None` when the inspection failed or the identity
/// join did not match); the exact identity-matched page `/Font` report (advisory
/// — a `None` fact makes the caller's font policy unknown and never skips the
/// page); and the exact identity-matched page `/ExtGState` report (the SAME
/// report used by the safety preflight, never inspected twice); the page's
/// identity-matched observe-only [`FormCloneSetPlanCounts`] from the ONE
/// request-scoped clone-set plan (empty counts on a failed identity join,
/// never a page skip); and the request
/// `ObjectLookup` (Copy), so an `FnMut` callback can resolve exact demanded
/// objects (for example root Form colour-effect analysis) through the
/// already-open backend without reopening the document. Each `None` advisory
/// fact degrades the callback's knowledge, never the page itself.
#[allow(clippy::too_many_lines)]
pub fn edit_page_content_incremental_sequence<T, P, F>(
    input: &[u8],
    pages: &PageSelection,
    preflight: P,
    mut edit: F,
) -> Result<PageSequenceOutput<T>, EditPageContentError>
where
    P: Fn(
        PageIndex,
        Option<&PageExtGStateResourcesInspection>,
        Option<&PageTransparencyGroupInspection>,
        &PageContentSequence,
    ) -> Option<PipelineSkipReason>,
    F: FnMut(
        PageIndex,
        &PageContentSequence,
        &PageColorFacts<'_>,
        Option<&PageXObjectResourcesInspection>,
        Option<&PageFontResourcesInspection>,
        Option<&PageExtGStateResourcesInspection>,
        FormCloneSetPlanCounts,
        ObjectLookup<'_>,
    ) -> Option<PageSequenceEdit<T>>,
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
    // The consumer index is inspected ONCE: it is first BORROWED by the
    // request-scoped Form clone-set plan (binding witnesses + closure walk +
    // single reservation, observe-only), then MOVED into the ownership
    // adapter. No second index is built.
    let consumers = inspect_object_consumer_index(input, &access);
    let mut form_clone_set_plan = FormCloneSetPlan::build(
        input,
        lookup,
        access.root_reference,
        access.catalog_pages.pages_reference,
        access.page_tree_root.object_byte_offset,
        &selected,
        &consumers,
    );
    // Staged/validate-only clone-body export: every planned set is
    // materialized and fully validated immediately after plan construction,
    // BEFORE the per-page counts are consumed, and the staged counters are
    // committed atomically. A successful staged batch is then materialized
    // into the full request-atomic clone commit transaction — the moved
    // staged fresh bodies PLUS one corroborated planned dirty page object
    // per affected page — and deliberately DROPPED: no fresh object or
    // page-retarget object reaches any production writer, so emitted product
    // bytes stay byte-identical to the plan-only behaviour.
    if let Ok(staged) = form_clone_set_plan.stage_export(input, lookup) {
        drop(build_clone_commit_batch(
            input,
            &form_clone_set_plan,
            staged,
        ));
    }
    let ownership = ContentObjectOwnershipIndex::new(&document.pages, consumers);
    // The `ExtGState` report is inspected ONCE and reused for both the safety
    // preflight and the font-policy edit fact. An inspection failure poisons
    // every selected page fail-closed (unresolved/unclassified), exactly as the
    // pre-split preflight did.
    let extgstate_document = inspect_document_page_extgstate_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    );
    let extgstate_pages = extgstate_document
        .as_ref()
        .ok()
        .map(|document| document.pages.as_slice());
    let extgstate_index = PageReportIndex::new(extgstate_pages);
    let group_document = inspect_document_page_transparency_groups_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    );
    // Advisory colour facts: both structural inspections run ONCE per request
    // through the already-open lookup; a failed inspection degrades to unknown
    // facts for every page rather than a new pipeline skip.
    let color_space_document = inspect_document_page_color_space_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .ok();
    let default_document = inspect_document_page_default_color_spaces_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .ok();
    let facts_index =
        PageColorFactsIndex::new(color_space_document.as_ref(), default_document.as_ref());
    // Advisory page-XObject facts: ONE bounded shallow document inspection per
    // request; a failure degrades every page to a `None` fact, never a skip.
    let xobject_document = inspect_document_page_xobject_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .ok();
    let xobject_index = PageReportIndex::new(
        xobject_document
            .as_ref()
            .map(|document| document.pages.as_slice()),
    );
    // Advisory page-Font facts: ONE bounded document inspection per request. A
    // failed inspection or a failed identity join is advisory only — it makes
    // the caller's font policy unknown (TextShow refuses) but never skips a page
    // whose walked content has no font dependency.
    let font_document = inspect_document_page_font_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .ok();
    let font_index = PageReportIndex::new(
        font_document
            .as_ref()
            .map(|document| document.pages.as_slice()),
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
        if extgstate_document.is_err() {
            skipped.push(whole_page_skip(
                page_index,
                PipelineSkipReason::ExtGStateUnsafe {
                    overprint: false,
                    transparency: false,
                    unresolved: true,
                    unclassified: true,
                    gs_count: 0,
                },
            ));
            continue;
        }
        let extgstate_page = extgstate_index.matched(
            page.leaf.reference,
            page.leaf.object_byte_offset,
            page.ordinal,
        );
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
        let facts = facts_index.facts_for(
            page.leaf.reference,
            page.leaf.object_byte_offset,
            page.ordinal,
        );
        let xobject_page = xobject_index.matched(
            page.leaf.reference,
            page.leaf.object_byte_offset,
            page.ordinal,
        );
        let font_page = font_index.matched(
            page.leaf.reference,
            page.leaf.object_byte_offset,
            page.ordinal,
        );
        // Observe-only clone-set plan counts, joined by the SAME exact page
        // identity triple as every other advisory report.
        let form_clone_counts = form_clone_set_plan.page_counts(
            page.leaf.reference,
            page.leaf.object_byte_offset,
            page.ordinal,
        );
        let Some(page_edit) = edit(
            page_index,
            &sequence,
            &facts,
            xobject_page,
            font_page,
            extgstate_page,
            form_clone_counts,
            lookup,
        ) else {
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

/// Structural facts every advisory page report exposes for the exact page
/// identity join: document ordinal, leaf page reference, and page object byte
/// offset.
trait PageIdentityReport {
    fn identity(&self) -> (IndirectRef, usize, usize);
}

impl PageIdentityReport for PageXObjectResourcesInspection {
    fn identity(&self) -> (IndirectRef, usize, usize) {
        (
            self.page_reference,
            self.page_object_byte_offset,
            self.ordinal,
        )
    }
}

impl PageIdentityReport for PageFontResourcesInspection {
    fn identity(&self) -> (IndirectRef, usize, usize) {
        (
            self.page_reference,
            self.page_object_byte_offset,
            self.ordinal,
        )
    }
}

impl PageIdentityReport for PageExtGStateResourcesInspection {
    fn identity(&self) -> (IndirectRef, usize, usize) {
        (
            self.page_reference,
            self.page_object_byte_offset,
            self.ordinal,
        )
    }
}

/// Request-local exact join from leaf page references to one advisory page
/// report of type `R`.
///
/// Lookups are ordered (`BTreeMap`) by exact [`IndirectRef`]; a reference
/// reported more than once is poisoned to a failed match, and every match is
/// corroborated by page object byte offset and document ordinal, so a missing,
/// duplicate, or inconsistent report is fail-closed `None`. This is the single
/// mechanical join used by the page `/XObject`, `/Font`, and `/ExtGState`
/// facts; the exact `XObject` join behaviour is preserved unchanged.
struct PageReportIndex<'a, R> {
    slots: BTreeMap<IndirectRef, Option<&'a R>>,
}

impl<'a, R: PageIdentityReport> PageReportIndex<'a, R> {
    fn new(pages: Option<&'a [R]>) -> Self {
        let mut slots = BTreeMap::new();
        if let Some(pages) = pages {
            for page in pages {
                let (reference, _, _) = page.identity();
                slots
                    .entry(reference)
                    .and_modify(|slot| *slot = None)
                    .or_insert(Some(page));
            }
        }
        Self { slots }
    }

    /// Resolve one uniquely matched, identity-corroborated report page; any
    /// missing, duplicate, or inconsistent match is `None`, fail-closed.
    fn matched(
        &self,
        reference: IndirectRef,
        object_byte_offset: usize,
        ordinal: usize,
    ) -> Option<&'a R> {
        let page = self.slots.get(&reference).copied().flatten()?;
        (page.identity() == (reference, object_byte_offset, ordinal)).then_some(page)
    }
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

fn transparency_group_page_for_preflight(
    document: &Result<
        DocumentPageTransparencyGroupsInspection,
        DocumentPageTransparencyGroupsInspectionError,
    >,
    index: usize,
) -> Result<Option<&PageTransparencyGroupInspection>, PipelineSkipReason> {
    document.as_ref().map_or(
        Err(PipelineSkipReason::TransparencyGroupUnsafe {
            transparency: false,
            unresolved: true,
            unclassified: true,
        }),
        |document| Ok(document.pages.get(index)),
    )
}

const fn whole_page_skip(page_index: PageIndex, reason: PipelineSkipReason) -> PipelinePageSkip {
    PipelinePageSkip {
        page_index,
        stream_ordinal: 0,
        content_object: None,
        reason,
    }
}

#[cfg(test)]
mod page_report_index_tests {
    use super::*;

    fn page(
        ordinal: usize,
        object_number: u32,
        page_object_byte_offset: usize,
    ) -> PageXObjectResourcesInspection {
        PageXObjectResourcesInspection {
            ordinal,
            page_reference: IndirectRef {
                object_number,
                generation: 0,
            },
            page_object_byte_offset,
            image_xobjects: Vec::new(),
            form_xobjects: Vec::new(),
            image_xobject_names: Vec::new(),
            form_xobject_names: Vec::new(),
            skipped: Vec::new(),
        }
    }

    #[test]
    fn exact_reference_match_requires_offset_and_ordinal_corroboration() {
        let pages = vec![page(2, 7, 70)];
        let index = PageReportIndex::new(Some(pages.as_slice()));
        let reference = IndirectRef {
            object_number: 7,
            generation: 0,
        };

        assert!(index.matched(reference, 70, 2).is_some());
        assert!(index.matched(reference, 71, 2).is_none());
        assert!(index.matched(reference, 70, 1).is_none());
        assert!(
            index
                .matched(
                    IndirectRef {
                        object_number: 7,
                        generation: 1,
                    },
                    70,
                    2,
                )
                .is_none()
        );
    }

    #[test]
    fn duplicate_page_references_and_missing_documents_are_poisoned() {
        let pages = vec![page(0, 3, 30), page(1, 3, 30)];
        let reference = IndirectRef {
            object_number: 3,
            generation: 0,
        };

        assert!(
            PageReportIndex::new(Some(pages.as_slice()))
                .matched(reference, 30, 0)
                .is_none()
        );
        assert!(
            PageReportIndex::<PageXObjectResourcesInspection>::new(None)
                .matched(reference, 30, 0)
                .is_none()
        );
    }
}
