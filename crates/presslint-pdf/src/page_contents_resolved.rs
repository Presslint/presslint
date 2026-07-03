use serde::{Deserialize, Serialize};

use crate::object_dictionary::resolved_dictionary_rejection_as_indirect;
use crate::source_utils::{
    skip_hex_string, skip_literal_string, skip_name, skip_scalar_token,
    skip_whitespace_and_comments,
};
use crate::{
    ArrayExtentInspectionRejection, DictionaryEntryByteRange, DictionaryEntrySpan,
    DictionaryValueKind, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionRejection, IndirectObjectHeaderByteRange, IndirectRef,
    IndirectReferenceByteRange, IndirectReferenceInspectionRejection, PageContentReference,
    PageContentsInspection, PageContentsValueShape, ResolvedObjectData,
    ResolvedObjectDictionaryInspection, inspect_array_extent, inspect_dictionary_extent,
    inspect_object_dictionary, parse_indirect_reference,
};

const CONTENTS_KEY: &[u8] = b"/Contents";

/// Provenance-neutral sentinel byte range for compressed-leaf `/Contents`
/// references.
///
/// RESOLVED-BODY PROVENANCE BOUNDARY: a compressed leaf's dictionary lives in
/// decoded `/ObjStm` bytes with no stable original-PDF source offset, so the
/// resolved `/Contents` inspector never surfaces a member-body-relative span as
/// if it were a source byte range. When the resolved references are adapted into
/// the shared [`PageContentsInspection`] shape for target/extent resolution, every
/// span field is filled with this zero sentinel instead of a buffer-relative
/// offset. Downstream target/extent resolution reads only the position-independent
/// object NUMBER from each reference, never the range, so the sentinel is inert.
const NEUTRAL_RANGE: IndirectReferenceByteRange = IndirectReferenceByteRange { start: 0, end: 0 };

/// Position-independent content-stream references read from a leaf page object's
/// top-level `/Contents` entry over a body-aware [`ResolvedObjectData`].
///
/// This report is deliberately PROVENANCE-CLEAN: it carries only the content
/// object REFERENCES (document-global object numbers), the single-vs-array value
/// shape, and a count of non-reference array entries. It exposes NO byte spans —
/// in particular it never surfaces the leaf-dict-side `/Contents` key/value spans,
/// which for a compressed leaf are relative to the decoded `/ObjStm` member body
/// and are meaningless as original-PDF source offsets. It retains or copies no PDF
/// bytes, object bodies, or decoded object-stream buffers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedPageContents {
    /// Whether the `/Contents` value was a single reference or an array.
    pub value_shape: PageContentsValueShape,
    /// Direct content-stream object references in source order.
    pub contents: Vec<IndirectRef>,
    /// Count of top-level array `/Contents` entries that were not direct
    /// references (nested arrays/dictionaries/strings/names/scalars). Always `0`
    /// for a single-reference value shape.
    pub skipped_non_reference_count: usize,
}

/// Error returned when a resolved leaf page object's `/Contents` cannot be read.
///
/// Like [`ResolvedPageContents`], this report is provenance-clean: it carries the
/// structured reason without any leaf-dict-side byte span.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ResolvedPageContentsError {
    /// The delegated resolved object-dictionary inspection failed.
    PageDictionary {
        /// Underlying object-dictionary rejection reason (compressed rejections
        /// are mapped into the uncompressed vocabulary).
        page_dictionary_reason: IndirectObjectDictionaryInspectionRejection,
    },
    /// The page dictionary has no exact top-level raw `/Contents` key.
    MissingContents,
    /// The page dictionary has more than one exact top-level raw `/Contents` key.
    DuplicateContents,
    /// The `/Contents` value is neither an indirect reference nor an array.
    NonReferenceOrArrayContentsValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The `/Contents` array value could not be bounded as a balanced extent.
    ContentsArrayExtent {
        /// Underlying array extent rejection reason.
        array_reason: ArrayExtentInspectionRejection,
    },
    /// The single-reference `/Contents` value did not parse as a complete `N G R`
    /// reference.
    MalformedContentsReference {
        /// Underlying indirect reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
}

/// Read a leaf page object's top-level `/Contents` content references from a
/// body-aware [`ResolvedObjectData`] (uncompressed OR compressed).
///
/// The leaf dictionary is read through the existing resolved-aware
/// [`inspect_object_dictionary`] — the same precedent `page_boxes.rs` uses to read
/// boxes from resolved leaves — so no new dictionary parser is introduced. For an
/// [`ResolvedObjectData::Uncompressed`] leaf the entry byte ranges are relative to
/// `input`; for a [`ResolvedObjectData::Compressed`] leaf they are relative to the
/// decoded `/ObjStm` member body. The `/Contents` value is parsed against the
/// matching buffer, and only the position-independent content object REFERENCES
/// (plus the single/array shape) are reported. No byte span is returned: this is
/// the RESOLVED-BODY PROVENANCE BOUNDARY (a compressed member body has no stable
/// source offset).
///
/// A single-reference value is parsed with [`parse_indirect_reference`]; an array
/// value is bounded with [`inspect_array_extent`] and scanned for direct top-level
/// `N G R` references in source order, counting non-reference entries as shallow
/// skips rather than dropping them silently. It resolves, fetches, decodes, or
/// concatenates no referenced content streams, and retains or copies no PDF bytes.
///
/// # Errors
///
/// Returns [`ResolvedPageContentsError`] for a delegated dictionary failure, a
/// missing or duplicate exact `/Contents` key, a non-reference/non-array value, a
/// malformed single reference, or an array value that cannot be bounded.
pub fn inspect_page_contents_resolved(
    input: &[u8],
    resolved: &ResolvedObjectData,
) -> Result<ResolvedPageContents, ResolvedPageContentsError> {
    let dictionary = inspect_object_dictionary(input, resolved).map_err(|error| {
        ResolvedPageContentsError::PageDictionary {
            page_dictionary_reason: resolved_dictionary_rejection_as_indirect(error.reason),
        }
    })?;
    let entries = dictionary_entries(&dictionary);
    let buffer = value_buffer(input, resolved);

    let contents_entry = unique_contents_entry(buffer, entries)?;
    match contents_entry.value_kind {
        DictionaryValueKind::Array => read_array_contents(buffer, contents_entry),
        DictionaryValueKind::IndirectReferenceLike | DictionaryValueKind::OtherScalar => {
            read_single_contents(buffer, contents_entry)
        }
        value_kind => {
            Err(ResolvedPageContentsError::NonReferenceOrArrayContentsValue { value_kind })
        }
    }
}

/// Adapt provenance-clean resolved references into the shared
/// [`PageContentsInspection`] shape expected by
/// [`crate::inspect_page_content_targets_with_lookup`].
///
/// Every byte span in the returned inspection is a provenance-neutral zero
/// sentinel ([`NEUTRAL_RANGE`] and its dictionary equivalents): the target/extent
/// inspectors read only the object NUMBER from each [`PageContentReference`], so
/// the sentinel spans are never used as source offsets and the compressed leaf's
/// member-body spans are never presented as original-PDF source byte ranges. The
/// resulting `targets`/`extents` therefore carry ONLY source-valid offsets that
/// come from resolving the content object numbers through the backend.
#[must_use]
pub fn page_contents_inspection_from_resolved(
    resolved: &ResolvedPageContents,
) -> PageContentsInspection {
    let contents = resolved
        .contents
        .iter()
        .map(|&reference| PageContentReference {
            reference,
            reference_range: NEUTRAL_RANGE,
        })
        .collect();
    PageContentsInspection {
        page_dictionary: neutral_dictionary(),
        contents_key_range: DictionaryEntryByteRange { start: 0, end: 0 },
        contents_value_range: DictionaryEntryByteRange { start: 0, end: 0 },
        value_shape: resolved.value_shape,
        contents,
        skipped: Vec::new(),
    }
}

/// Borrow the entry spans of a resolved-object dictionary inspection.
///
/// Uncompressed entries are relative to `input`; compressed entries are relative
/// to the decoded member body. The caller pairs this with [`value_buffer`].
fn dictionary_entries(dictionary: &ResolvedObjectDictionaryInspection) -> &[DictionaryEntrySpan] {
    match dictionary {
        ResolvedObjectDictionaryInspection::Uncompressed(inspection) => &inspection.entries,
        ResolvedObjectDictionaryInspection::Compressed(inspection) => &inspection.entries,
    }
}

/// Select the buffer whose offsets the resolved dictionary entry ranges address.
///
/// For an uncompressed object that is the original `input`; for a compressed
/// member it is the extracted member-body slice inside the decoded object stream.
fn value_buffer<'a>(input: &'a [u8], resolved: &'a ResolvedObjectData) -> &'a [u8] {
    match resolved {
        ResolvedObjectData::Uncompressed { .. } => input,
        ResolvedObjectData::Compressed {
            decoded_object_stream,
            object_body_span,
            ..
        } => decoded_object_stream
            .get(object_body_span.start..object_body_span.end)
            .unwrap_or(&[]),
    }
}

fn unique_contents_entry(
    buffer: &[u8],
    entries: &[DictionaryEntrySpan],
) -> Result<DictionaryEntrySpan, ResolvedPageContentsError> {
    let mut found: Option<DictionaryEntrySpan> = None;
    for entry in entries {
        if buffer.get(entry.key_range.start..entry.key_range.end) != Some(CONTENTS_KEY) {
            continue;
        }
        if found.is_some() {
            return Err(ResolvedPageContentsError::DuplicateContents);
        }
        found = Some(*entry);
    }
    found.ok_or(ResolvedPageContentsError::MissingContents)
}

fn read_single_contents(
    buffer: &[u8],
    contents_entry: DictionaryEntrySpan,
) -> Result<ResolvedPageContents, ResolvedPageContentsError> {
    let reference =
        parse_indirect_reference(buffer, contents_entry.value_range.start).map_err(|error| {
            ResolvedPageContentsError::MalformedContentsReference {
                reference_reason: error.reason,
            }
        })?;
    if reference.after_keyword_offset != contents_entry.value_range.end {
        return Err(ResolvedPageContentsError::MalformedContentsReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        });
    }
    Ok(ResolvedPageContents {
        value_shape: PageContentsValueShape::SingleReference,
        contents: vec![reference.reference],
        skipped_non_reference_count: 0,
    })
}

fn read_array_contents(
    buffer: &[u8],
    contents_entry: DictionaryEntrySpan,
) -> Result<ResolvedPageContents, ResolvedPageContentsError> {
    let array =
        inspect_array_extent(buffer, contents_entry.value_range.start).map_err(|error| {
            ResolvedPageContentsError::ContentsArrayExtent {
                array_reason: error.reason,
            }
        })?;

    let mut contents = Vec::new();
    let mut skipped_non_reference_count = 0usize;
    let mut cursor = array.open_byte_offset + 1;
    let body_end = array.close_byte_offset;

    while cursor < body_end {
        cursor = skip_whitespace_and_comments(buffer, cursor, body_end);
        if cursor >= body_end {
            break;
        }
        match parse_indirect_reference(buffer, cursor) {
            Ok(reference) if reference.after_keyword_offset <= body_end => {
                contents.push(reference.reference);
                cursor = reference.after_keyword_offset;
            }
            _ => {
                skipped_non_reference_count += 1;
                cursor = skip_array_value(buffer, cursor, body_end);
            }
        }
    }

    Ok(ResolvedPageContents {
        value_shape: PageContentsValueShape::Array,
        contents,
        skipped_non_reference_count,
    })
}

/// Advance past one non-reference array value, guaranteeing forward progress.
fn skip_array_value(buffer: &[u8], start: usize, limit: usize) -> usize {
    let end = match buffer[start] {
        b'[' => {
            inspect_array_extent(buffer, start).map_or(limit, |array| array.after_close_byte_offset)
        }
        b'<' if buffer.get(start + 1) == Some(&b'<') => inspect_dictionary_extent(buffer, start)
            .map_or(limit, |dictionary| dictionary.after_close_byte_offset),
        b'(' => skip_literal_string(buffer, start).unwrap_or(limit),
        b'<' => skip_hex_string(buffer, start).unwrap_or(limit),
        b'/' => skip_name(buffer, start, limit),
        _ => skip_scalar_token(buffer, start, limit),
    };
    end.clamp(start + 1, limit)
}

/// Build a provenance-neutral, all-zero [`IndirectObjectDictionaryInspection`].
///
/// This is only a placeholder for the `page_dictionary` field of the synthetic
/// [`PageContentsInspection`] used to reach the shared target/extent inspectors; it
/// is never surfaced in the resolved content-extents report and carries no source
/// bytes or meaningful offsets.
const fn neutral_dictionary() -> IndirectObjectDictionaryInspection {
    IndirectObjectDictionaryInspection {
        reference: IndirectRef {
            object_number: 0,
            generation: 0,
        },
        header_range: IndirectObjectHeaderByteRange { start: 0, end: 0 },
        dictionary_open_byte_offset: 0,
        dictionary_close_byte_offset: 0,
        after_dictionary_close_byte_offset: 0,
        max_observed_dictionary_depth: 0,
        entries: Vec::new(),
    }
}
