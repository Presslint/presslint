//! Semantic **no-op** content-stream re-encode over single-content-stream pages.
//!
//! This is the second whole-stream slice of the content-stream pivot. It proves
//! the whole "replace an indirect stream object's body" pipeline —
//! locate -> decode -> re-serialize byte-identically -> re-encode ->
//! `WholeStream`-replace -> reopen — without altering any decoded content, so the
//! first real operand rewrite only has to add the "edit the decoded bytes" step
//! onto a proven substrate.
//!
//! For a `/FlateDecode` content stream the decoded bytes are re-serialized (which
//! must round-trip byte-identically) and re-compressed with
//! [`presslint_pdf::encode_flate_stream`]: the stream bytes may differ but
//! `decode(reopened) == decode(original)`. For a raw/uncompressed content stream
//! the re-serialized decoded bytes equal the original stream data, so the
//! reopened stream is byte-identical (a true byte no-op). Every unrelated stream
//! dictionary byte — including `/Filter` and `/DecodeParms` — is preserved
//! verbatim; only the single direct `/Length` value is rewritten to the new data
//! length.
//!
//! Structural facts are read through `presslint-pdf` and never reparsed here: the
//! per-page single content stream is located through
//! [`inspect_document_page_content_extents_with_lookup`], its dictionary through
//! [`inspect_indirect_object_dictionary`], its data through
//! [`content_stream_data_slice`], and its filter/decode-parameters through
//! [`classify_content_stream_filter`]/[`resolve_flate_decode_parameters`].
//! Ownership is decided with [`decide_indirect_object_edit`]; only a content
//! object referenced by exactly one page is rewritten in place. Edits are routed
//! through [`write_incremental_revision_plan`] as one `WholeStream` boundary each.

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
    PlannedWriteError, WriteError, write_incremental_revision, write_incremental_revision_plan,
};

const LENGTH_KEY: &[u8] = b"/Length";

/// Upper bound on the decoded and re-encoded content-stream buffers.
///
/// One decoded `Vec<u8>` and one re-encoded `Vec<u8>` are held per edited page;
/// this bound keeps that per-page memory explicit without limiting realistic
/// content streams.
const MAX_CONTENT_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// Which pages to re-encode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "pages", rename_all = "snake_case")]
pub enum PageSelection {
    /// Every enumerated document page.
    All,
    /// A specific set of zero-based document page indexes.
    Indices(Vec<PageIndex>),
}

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
    /// The content stream had no filter; re-serialized decoded bytes are written
    /// verbatim (a true byte no-op).
    Raw,
    /// The content stream used a single `/FlateDecode`; decoded bytes are
    /// re-compressed (a semantic no-op).
    Flate,
}

/// Report for one re-encoded page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReencodedPage {
    /// Zero-based document page index.
    pub page_index: PageIndex,
    /// Indirect reference of the rewritten content-stream object.
    pub content_object: IndirectRef,
    /// Filter path used for the re-encode.
    pub filter_kind: ReencodeFilterKind,
}

/// One requested page skipped before any byte writing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReencodePageSkip {
    /// Requested zero-based document page index.
    pub page_index: PageIndex,
    /// Content-stream object reference when it was located.
    pub content_object: Option<IndirectRef>,
    /// Structured skip reason.
    pub reason: ReencodePageSkipReason,
}

/// Structured reason a requested page's content stream was not re-encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ReencodePageSkipReason {
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
    /// The stream `/Length` is an indirect reference; only a direct integer is
    /// rewritten.
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
    /// The content stream is `/FlateDecode` with a `/DecodeParms` predictor,
    /// which this slice does not re-encode.
    PredictorFlate {
        /// The unsupported predictor value.
        predictor: u16,
    },
    /// The decoded content did not re-serialize byte-identically through
    /// `tokenize` + `serialize_tokens_unmodified`, or could not be decoded, so no
    /// page whose syntax does not round-trip is written.
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
        page_index: PageIndex,
        /// Number of enumerated document pages.
        page_count: usize,
    },
    /// The append writer rejected the input or the assembled revision. Used for
    /// the all-skipped case, which delegates to the append writer directly.
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
/// revision that rewrites only the edited content-stream objects. For a
/// `/FlateDecode` stream the decoded content is unchanged
/// (`decode(reopened) == decode(original)`); for a raw stream the reopened data
/// is byte-identical. Pages whose content stream shape is unsupported (multiple
/// or zero streams, compressed object, indirect/missing/duplicate/non-numeric
/// `/Length`, unsupported filter, predictor Flate, round-trip mismatch, or
/// unproven ownership) are reported as structured skips and the remaining
/// editable pages are still written.
///
/// # Errors
///
/// Returns [`ReencodePageContentError`] when the request is empty, the input
/// cannot be opened, a requested page index is out of range, or the append
/// writer / plan bridge rejects the input (for example an encrypted or hybrid
/// document).
pub fn reencode_page_content_incremental(
    input: &[u8],
    request: &ReencodePageContentRequest,
) -> Result<ReencodePageContentOutput, ReencodePageContentError> {
    let access =
        inspect_document_access(input).map_err(|error| ReencodePageContentError::Open {
            error: Box::new(error),
        })?;
    let lookup = lookup_from_backend(&access.backend);
    let document = inspect_document_page_content_extents_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .map_err(|error| ReencodePageContentError::Open {
        error: Box::new(DocumentAccessError {
            byte_len: input.len(),
            reason: DocumentAccessRejection::PageTreeLeaves { error: error.error },
        }),
    })?;

    let page_count = document.pages.len();
    let selected = select_indices(&request.pages, page_count)?;
    let owners = content_object_owners(&document.pages);

    let mut reencoded = Vec::new();
    let mut skipped = Vec::new();
    let mut dirty_objects: Vec<PlannedDirtyObject> = Vec::new();

    for index in selected {
        // `select_indices` guarantees `index < page_count`.
        match plan_page(input, &document.pages[index], &owners) {
            PagePlan::Edit { report, planned } => {
                reencoded.push(report);
                dirty_objects.push(planned);
            }
            PagePlan::Skip(skip) => skipped.push(skip),
        }
    }

    let bytes = if dirty_objects.is_empty() {
        // Every selected page was skipped: the plan would be empty, which the
        // bridge rejects by contract, so delegate the no-op revision straight to
        // the append writer (this is also where an encrypted/hybrid input is
        // rejected on the all-skipped path).
        write_incremental_revision(input, &[]).map_err(|error| ReencodePageContentError::Write {
            error: Box::new(error),
        })?
    } else {
        let plan = IncrementalRevisionPlan { dirty_objects };
        write_incremental_revision_plan(input, &plan).map_err(|error| {
            ReencodePageContentError::Plan {
                error: Box::new(error),
            }
        })?
    };

    reencoded.sort_by_key(|report| report.page_index.0);

    Ok(ReencodePageContentOutput {
        bytes,
        reencoded,
        skipped,
    })
}

/// Map a document-access backend to the borrowed object-lookup view it exposes.
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

/// Resolve the request into a deterministic, deduplicated, in-range set of
/// document page indexes.
fn select_indices(
    pages: &PageSelection,
    page_count: usize,
) -> Result<Vec<usize>, ReencodePageContentError> {
    match pages {
        PageSelection::All => Ok((0..page_count).collect()),
        PageSelection::Indices(indices) => {
            if indices.is_empty() {
                return Err(ReencodePageContentError::EmptyRequest);
            }
            // Deduplicate and order so a repeated index never produces two dirty
            // objects for the same content stream, and the output is independent
            // of request order.
            let mut ordered: Vec<usize> = Vec::new();
            for page_index in indices {
                let index = page_index.0 as usize;
                if index >= page_count {
                    return Err(ReencodePageContentError::PageIndexOutOfRange {
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

/// Count the owning leaf pages of every content object referenced across the
/// document, so shared content streams can be proven not single-use.
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

/// Outcome of planning one requested page.
enum PagePlan {
    Edit {
        report: ReencodedPage,
        planned: PlannedDirtyObject,
    },
    Skip(ReencodePageSkip),
}

/// The located, single, uncompressed, direct-`/Length` content stream of a page.
struct LocatedStream<'a> {
    content_object: IndirectRef,
    object_byte_offset: usize,
    extent: &'a ContentStreamDataExtentInspection,
}

fn plan_page(
    input: &[u8],
    page: &DocumentPageContentExtentInspection,
    owners: &BTreeMap<IndirectRef, Vec<IndirectRef>>,
) -> PagePlan {
    let page_index = page_index_of(page);
    let located = match locate_single_stream(page) {
        Ok(located) => located,
        Err((content_object, reason)) => {
            return PagePlan::Skip(ReencodePageSkip {
                page_index,
                content_object,
                reason,
            });
        }
    };
    let content_object = located.content_object;

    let skip = |reason: ReencodePageSkipReason| {
        PagePlan::Skip(ReencodePageSkip {
            page_index,
            content_object: Some(content_object),
            reason,
        })
    };

    // Prove single-use ownership of the content object before rewriting it.
    let consumers = owners
        .get(&content_object)
        .map_or([].as_slice(), Vec::as_slice);
    let occurrences = consumers.len();
    let decision = decide_indirect_object_edit(content_object, consumers.iter().copied());
    if decision.disposition != IndirectObjectEditDisposition::InPlaceMutation {
        return skip(ReencodePageSkipReason::OwnershipNotInPlace {
            occurrences,
            disposition: decision.disposition,
        });
    }

    // Read the stream dictionary and locate the single direct `/Length` value.
    let Ok(dictionary) = inspect_indirect_object_dictionary(input, located.object_byte_offset)
    else {
        return skip(ReencodePageSkipReason::NoContentStream);
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
        return skip(ReencodePageSkipReason::NoContentStream);
    };

    let new_data = match reencode_stream_data(stream_data, filter) {
        Ok(new_data) => new_data,
        Err(reason) => return skip(reason),
    };

    let body = build_stream_object_body(
        input,
        dictionary.dictionary_open_byte_offset,
        dictionary.after_dictionary_close_byte_offset,
        length_value_range,
        &new_data,
    );

    let boundary = whole_stream_boundary(content_object, located.extent, &decision);

    PagePlan::Edit {
        report: ReencodedPage {
            page_index,
            content_object,
            filter_kind: filter,
        },
        planned: PlannedDirtyObject {
            reference: content_object,
            boundaries: vec![boundary],
            body_bytes: body,
        },
    }
}

/// Locate a page's single uncompressed content stream with a direct `/Length`.
///
/// On failure returns the content object reference (when one was resolvable) and
/// the structured skip reason.
fn locate_single_stream(
    page: &DocumentPageContentExtentInspection,
) -> Result<LocatedStream<'_>, (Option<IndirectRef>, ReencodePageSkipReason)> {
    let DocumentPageContentExtentResult::Inspected {
        contents, extents, ..
    } = &page.result
    else {
        return Err((None, ReencodePageSkipReason::NoContentStream));
    };

    let count = contents.contents.len();
    if count == 0 {
        return Err((None, ReencodePageSkipReason::NoContentStream));
    }
    if count > 1 || !contents.skipped.is_empty() {
        return Err((
            None,
            ReencodePageSkipReason::MultipleContentStreams { count },
        ));
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
                    ReencodePageSkipReason::IndirectLength,
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
        None => Err((None, ReencodePageSkipReason::NoContentStream)),
    }
}

/// Map an unresolved content-target skip to a re-encode skip reason.
///
/// A type-2 compressed object stream member surfaces the containing object
/// stream; every other unresolved lookup is treated as "no content stream".
const fn skip_reason_from_target(
    reason: &SkippedPageContentTargetReason,
) -> ReencodePageSkipReason {
    if let SkippedPageContentTargetReason::UnresolvedLookupLocation {
        location:
            ObjectLookupLocation::XrefStreamCompressed {
                object_stream_number,
                index_within_object_stream,
                ..
            },
    } = reason
    {
        return ReencodePageSkipReason::CompressedContentObject {
            object_stream_number: *object_stream_number,
            index_within_object_stream: *index_within_object_stream,
        };
    }
    ReencodePageSkipReason::NoContentStream
}

/// Map a content-extent location failure to a re-encode skip reason.
const fn skip_reason_from_extent_failure(
    reason: &ContentStreamDataExtentInspectionRejection,
) -> ReencodePageSkipReason {
    match reason {
        ContentStreamDataExtentInspectionRejection::MissingLength
        | ContentStreamDataExtentInspectionRejection::DuplicateLength { .. } => {
            ReencodePageSkipReason::MissingOrDuplicateLength
        }
        ContentStreamDataExtentInspectionRejection::UnsupportedLengthValueKind { value_kind } => {
            ReencodePageSkipReason::NonDirectNumericLength {
                value_kind: *value_kind,
            }
        }
        ContentStreamDataExtentInspectionRejection::IndirectLengthRequiresXrefTable
        | ContentStreamDataExtentInspectionRejection::IndirectLength { .. }
        | ContentStreamDataExtentInspectionRejection::LookupIndirectLength { .. } => {
            ReencodePageSkipReason::IndirectLength
        }
        ContentStreamDataExtentInspectionRejection::StreamStart { .. }
        | ContentStreamDataExtentInspectionRejection::DirectLength { .. } => {
            ReencodePageSkipReason::NoContentStream
        }
    }
}

/// Find the single top-level direct integer `/Length` value span.
fn find_direct_length(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
) -> Result<ByteRange, ReencodePageSkipReason> {
    let mut found: Option<&DictionaryEntrySpan> = None;
    for entry in entries {
        if input.get(entry.key_range.start..entry.key_range.end) == Some(LENGTH_KEY) {
            if found.is_some() {
                return Err(ReencodePageSkipReason::MissingOrDuplicateLength);
            }
            found = Some(entry);
        }
    }
    let entry = found.ok_or(ReencodePageSkipReason::MissingOrDuplicateLength)?;
    match entry.value_kind {
        DictionaryValueKind::NumberLike => Ok(ByteRange {
            start: entry.value_range.start,
            end: entry.value_range.end,
        }),
        DictionaryValueKind::IndirectReferenceLike => Err(ReencodePageSkipReason::IndirectLength),
        value_kind => Err(ReencodePageSkipReason::NonDirectNumericLength { value_kind }),
    }
}

/// Classify the content stream's filter into a supported re-encode path.
fn classify_filter(
    input: &[u8],
    object_byte_offset: usize,
) -> Result<ReencodeFilterKind, ReencodePageSkipReason> {
    match classify_content_stream_filter(input, object_byte_offset) {
        Ok(ContentStreamFilterClassification::Uncompressed) => Ok(ReencodeFilterKind::Raw),
        Ok(ContentStreamFilterClassification::Flate) => {
            match resolve_flate_decode_parameters(input, object_byte_offset) {
                Ok(FlateDecodeParametersResolution::Resolved { parameters, .. }) => {
                    if parameters.predictor == FlateDecodeParameters::default().predictor {
                        Ok(ReencodeFilterKind::Flate)
                    } else {
                        Err(ReencodePageSkipReason::PredictorFlate {
                            predictor: parameters.predictor,
                        })
                    }
                }
                // An array `/DecodeParms` is the multi-filter shape this
                // single-filter slice does not re-encode.
                Ok(FlateDecodeParametersResolution::UnsupportedArrayParms { .. }) | Err(_) => {
                    Err(ReencodePageSkipReason::UnsupportedFilter)
                }
            }
        }
        Ok(
            ContentStreamFilterClassification::UnsupportedFilter { .. }
            | ContentStreamFilterClassification::UnsupportedFilterChain { .. },
        )
        | Err(_) => Err(ReencodePageSkipReason::UnsupportedFilter),
    }
}

/// Decode, prove byte-identical re-serialization, and re-encode the stream data.
///
/// The decoded content must re-serialize byte-identically through
/// [`tokenize`] + [`serialize_tokens_unmodified`] (else the page is a
/// round-trip-mismatch skip, so no page whose syntax does not round-trip is
/// written). For [`ReencodeFilterKind::Raw`] the re-serialized bytes equal the
/// original stream data (a true byte no-op); for [`ReencodeFilterKind::Flate`]
/// they are re-compressed (a semantic no-op).
fn reencode_stream_data(
    stream_data: &[u8],
    filter: ReencodeFilterKind,
) -> Result<Vec<u8>, ReencodePageSkipReason> {
    match filter {
        ReencodeFilterKind::Raw => reserialize_byte_identical(stream_data),
        ReencodeFilterKind::Flate => {
            let decoded = decode_flate_stream(
                stream_data,
                FlateDecodeParameters::default(),
                MAX_CONTENT_STREAM_BYTES,
            )
            .map_err(|_| ReencodePageSkipReason::ContentRoundTripMismatch)?;
            let serialized = reserialize_byte_identical(&decoded)?;
            encode_flate_stream(&serialized, MAX_CONTENT_STREAM_BYTES)
                .map_err(|_| ReencodePageSkipReason::ContentRoundTripMismatch)
        }
    }
}

/// Re-serialize decoded content and require it to equal the input byte-for-byte.
///
/// `serialize_unmodified` is currently an identity placeholder, so this uses the
/// real lexical round trip: content that does not tokenize (for example an
/// unterminated string) or does not reconstruct exactly is a round-trip-mismatch
/// skip rather than a silently rewritten page.
fn reserialize_byte_identical(decoded: &[u8]) -> Result<Vec<u8>, ReencodePageSkipReason> {
    let tokens = tokenize(decoded).map_err(|_| ReencodePageSkipReason::ContentRoundTripMismatch)?;
    let serialized = serialize_tokens_unmodified(decoded, &tokens)
        .map_err(|_| ReencodePageSkipReason::ContentRoundTripMismatch)?;
    if serialized == decoded {
        Ok(serialized)
    } else {
        Err(ReencodePageSkipReason::ContentRoundTripMismatch)
    }
}

/// Rebuild a stream object's body with the one direct `/Length` value replaced.
///
/// The result is `<< preserved-dict-with-/Length-replaced >>\nstream\n<data>\n
/// endstream`. Exactly the one direct `/Length` value span is rewritten to the
/// new data length; every other dictionary byte (including `/Filter` and
/// `/DecodeParms`) is preserved verbatim. LF is used for the synthesized
/// `stream`/`endstream` separators; the dictionary is not normalized.
fn build_stream_object_body(
    input: &[u8],
    dictionary_open_byte_offset: usize,
    after_dictionary_close_byte_offset: usize,
    length_value_range: ByteRange,
    new_stream_data: &[u8],
) -> Vec<u8> {
    let dictionary = &input[dictionary_open_byte_offset..after_dictionary_close_byte_offset];
    let relative_start = length_value_range.start - dictionary_open_byte_offset;
    let relative_end = length_value_range.end - dictionary_open_byte_offset;
    let new_length = new_stream_data.len().to_string();

    let mut body = Vec::with_capacity(
        dictionary.len()
            + new_length.len()
            + new_stream_data.len()
            + b"\nstream\n\nendstream".len(),
    );
    body.extend_from_slice(&dictionary[..relative_start]);
    body.extend_from_slice(new_length.as_bytes());
    body.extend_from_slice(&dictionary[relative_end..]);
    body.extend_from_slice(b"\nstream\n");
    body.extend_from_slice(new_stream_data);
    body.extend_from_slice(b"\nendstream");
    body
}

/// Build the `WholeStream` boundary describing this in-place object rewrite.
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

/// The document-order ordinal of a page as a [`PageIndex`], saturating to `u32`.
fn page_index_of(page: &DocumentPageContentExtentInspection) -> PageIndex {
    PageIndex(u32::try_from(page.ordinal).unwrap_or(u32::MAX))
}
