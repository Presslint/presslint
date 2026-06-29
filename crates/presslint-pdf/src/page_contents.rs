use serde::{Deserialize, Serialize};

use crate::source_utils::{
    is_pdf_delimiter, skip_hex_string, skip_literal_string, skip_name, skip_scalar_token,
    skip_whitespace_and_comments,
};
use crate::{
    ArrayExtentInspectionRejection, DictionaryEntryByteRange, DictionaryEntrySpan,
    DictionaryValueKind, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionRejection, IndirectRef, IndirectReferenceByteRange,
    IndirectReferenceInspectionRejection,
};

const CONTENTS_KEY: &[u8] = b"/Contents";

/// Direct content-stream indirect reference(s) from a leaf page object's
/// top-level `/Contents` entry.
///
/// This report stores only the delegated page-object dictionary inspection,
/// the `/Contents` key/value byte ranges, a small value-shape marker, parsed
/// content references with their byte ranges, and shallow skipped-entry
/// diagnostics. It does not retain or copy PDF bytes, object bodies, array
/// bytes, content-stream bodies, or referenced-object bytes, and it does not
/// resolve, fetch, decode, or concatenate the referenced content streams.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageContentsInspection {
    /// Delegated leaf page-object dictionary inspection.
    pub page_dictionary: IndirectObjectDictionaryInspection,
    /// Byte range covering the exact top-level raw `/Contents` key.
    pub contents_key_range: DictionaryEntryByteRange,
    /// Byte range covering the `/Contents` value span.
    pub contents_value_range: DictionaryEntryByteRange,
    /// Whether the `/Contents` value was a single reference or an array.
    pub value_shape: PageContentsValueShape,
    /// Direct content-stream references in source order.
    pub contents: Vec<PageContentReference>,
    /// Top-level array `/Contents` entries that were not direct references.
    ///
    /// Always empty for a single-reference value shape.
    pub skipped: Vec<SkippedPageContentEntry>,
}

/// Shallow shape of a leaf page object's `/Contents` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageContentsValueShape {
    /// A single `N G R` indirect reference value.
    SingleReference,
    /// A `[ ... ]` array of references value.
    Array,
}

/// One direct content-stream indirect reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageContentReference {
    /// Parsed content-stream indirect reference.
    pub reference: IndirectRef,
    /// Byte range covering the parsed `N G R` reference.
    pub reference_range: IndirectReferenceByteRange,
}

/// One top-level array `/Contents` entry skipped by page-contents inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPageContentEntry {
    /// Byte range covering the skipped top-level entry.
    pub entry_range: DictionaryEntryByteRange,
    /// Shallow skipped-entry family.
    pub kind: SkippedPageContentEntryKind,
}

/// Shallow family for a skipped top-level array `/Contents` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkippedPageContentEntryKind {
    /// A nested `[ ... ]` array entry.
    Array,
    /// A nested `<< ... >>` dictionary entry.
    Dictionary,
    /// A literal string `( ... )` or hex string `< ... >` entry.
    String,
    /// A `/Name` entry.
    Name,
    /// A number-shaped scalar entry.
    NumberLike,
    /// A `true` or `false` scalar entry.
    Boolean,
    /// A `null` scalar entry.
    Null,
    /// Any other shallow scalar entry.
    OtherScalar,
    /// A direct scalar candidate shaped like an indirect reference but rejected
    /// by the shared indirect-reference parser.
    MalformedIndirectReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
}

/// Error returned when a leaf page object's `/Contents` cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageContentsInspectionError {
    /// Caller-supplied byte offset where page-object inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the resolved page object header begins, when it was
    /// located.
    pub page_header_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: PageContentsInspectionRejection,
}

/// Structured page-contents inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageContentsInspectionRejection {
    /// A delegated page-object dictionary inspection failed.
    PageDictionary {
        /// Underlying object dictionary rejection reason.
        page_dictionary_reason: IndirectObjectDictionaryInspectionRejection,
    },
    /// The page dictionary has no exact top-level raw `/Contents` key.
    MissingContents,
    /// The page dictionary has more than one exact top-level raw `/Contents`
    /// key.
    DuplicateContents {
        /// First `/Contents` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Contents` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Contents` value is neither an indirect reference nor an array.
    NonReferenceOrArrayContentsValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The `/Contents` array value could not be bounded as a balanced extent.
    ///
    /// An array-classified value was already bounded by the delegated
    /// dictionary-entry scan, so this is the dedicated channel for the bounding
    /// call rather than a reachable failure for a well-formed array value.
    ContentsArrayExtent {
        /// Underlying array extent rejection reason.
        array_reason: ArrayExtentInspectionRejection,
    },
    /// The single-reference `/Contents` value was shaped as a scalar but did not
    /// parse as a complete `N G R` reference.
    MalformedContentsReference {
        /// Underlying indirect reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
}

/// Inspect a leaf page object's top-level `/Contents` content references.
///
/// The helper composes existing bounded inspectors: it reads the page object
/// with [`crate::inspect_indirect_object_dictionary`], matches only the exact
/// raw top-level key bytes `/Contents`, and reports the direct content-stream
/// indirect reference(s) a future page-content reader will resolve.
///
/// A single-reference value (`N G R`) is parsed through
/// [`crate::parse_indirect_reference`] and reported as one content reference. An
/// array value is bounded with [`crate::inspect_array_extent`] and scanned for
/// direct top-level `N G R` references in source order, mirroring
/// [`crate::inspect_page_tree_kids`]: nested arrays, dictionaries, strings,
/// names, numbers, booleans, nulls, and other scalar entries are reported as
/// shallow skips and never silently dropped.
///
/// It does not resolve, fetch, decode, or concatenate the referenced content
/// streams, parse `stream`/`endstream`/`/Length`, validate `/Type /Page`,
/// inspect `/Resources`/page boxes/`/Annots`/inherited attributes, recurse into
/// `/Kids`, or decode PDF name escapes. An absent `/Contents` is reported as a
/// structured rejection rather than treated as a valid empty page.
///
/// # Errors
///
/// Returns [`PageContentsInspectionError`] for a delegated page-object
/// dictionary failure, a missing or duplicate exact `/Contents` key, a
/// non-reference/non-array `/Contents` value, or a malformed single-reference
/// value. Non-reference array entries are reported in
/// [`PageContentsInspection::skipped`] rather than failing the whole inspection.
pub fn inspect_page_contents(
    input: &[u8],
    page_object_offset: usize,
) -> Result<PageContentsInspection, PageContentsInspectionError> {
    let page_dictionary = crate::inspect_indirect_object_dictionary(input, page_object_offset)
        .map_err(|error| {
            page_contents_error(
                input,
                page_object_offset,
                error.header_byte_offset,
                PageContentsInspectionRejection::PageDictionary {
                    page_dictionary_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;
    let page_header_byte_offset = Some(page_dictionary.header_range.start);
    let dictionary_close = page_dictionary.dictionary_close_byte_offset;

    let mut contents_entry: Option<DictionaryEntrySpan> = None;
    for entry in &page_dictionary.entries {
        if !is_exact_contents_key(input, entry.key_range) {
            continue;
        }

        if let Some(first) = contents_entry {
            return Err(page_contents_error(
                input,
                page_object_offset,
                page_header_byte_offset,
                PageContentsInspectionRejection::DuplicateContents {
                    first_key_range: first.key_range,
                    duplicate_key_range: entry.key_range,
                },
                Some(entry.key_range.start),
            ));
        }

        contents_entry = Some(*entry);
    }

    let contents_entry = contents_entry.ok_or_else(|| {
        page_contents_error(
            input,
            page_object_offset,
            page_header_byte_offset,
            PageContentsInspectionRejection::MissingContents,
            Some(dictionary_close),
        )
    })?;

    match contents_entry.value_kind {
        DictionaryValueKind::Array => {
            inspect_array_contents(input, page_object_offset, page_dictionary, contents_entry)
        }
        DictionaryValueKind::IndirectReferenceLike | DictionaryValueKind::OtherScalar => {
            inspect_single_contents(input, page_object_offset, page_dictionary, contents_entry)
        }
        other => Err(page_contents_error(
            input,
            page_object_offset,
            page_header_byte_offset,
            PageContentsInspectionRejection::NonReferenceOrArrayContentsValue { value_kind: other },
            Some(contents_entry.value_range.start),
        )),
    }
}

fn inspect_single_contents(
    input: &[u8],
    page_object_offset: usize,
    page_dictionary: IndirectObjectDictionaryInspection,
    contents_entry: DictionaryEntrySpan,
) -> Result<PageContentsInspection, PageContentsInspectionError> {
    let page_header_byte_offset = Some(page_dictionary.header_range.start);

    let reference = crate::parse_indirect_reference(input, contents_entry.value_range.start)
        .map_err(|error| {
            page_contents_error(
                input,
                page_object_offset,
                page_header_byte_offset,
                PageContentsInspectionRejection::MalformedContentsReference {
                    reference_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;

    if reference.after_keyword_offset != contents_entry.value_range.end {
        return Err(page_contents_error(
            input,
            page_object_offset,
            page_header_byte_offset,
            PageContentsInspectionRejection::MalformedContentsReference {
                reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
            },
            Some(reference.after_keyword_offset),
        ));
    }

    Ok(PageContentsInspection {
        page_dictionary,
        contents_key_range: contents_entry.key_range,
        contents_value_range: contents_entry.value_range,
        value_shape: PageContentsValueShape::SingleReference,
        contents: vec![PageContentReference {
            reference: reference.reference,
            reference_range: reference.reference_range,
        }],
        skipped: Vec::new(),
    })
}

fn inspect_array_contents(
    input: &[u8],
    page_object_offset: usize,
    page_dictionary: IndirectObjectDictionaryInspection,
    contents_entry: DictionaryEntrySpan,
) -> Result<PageContentsInspection, PageContentsInspectionError> {
    let page_header_byte_offset = Some(page_dictionary.header_range.start);

    let array =
        crate::inspect_array_extent(input, contents_entry.value_range.start).map_err(|error| {
            page_contents_error(
                input,
                page_object_offset,
                page_header_byte_offset,
                PageContentsInspectionRejection::ContentsArrayExtent {
                    array_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;

    let mut contents = Vec::new();
    let mut skipped = Vec::new();
    let mut cursor = array.open_byte_offset + 1;
    let body_end = array.close_byte_offset;

    while cursor < body_end {
        cursor = skip_whitespace_and_comments(input, cursor, body_end);
        if cursor >= body_end {
            break;
        }

        let entry = scan_content_entry(input, cursor, body_end);
        cursor = entry.after_entry;
        match entry.outcome {
            ContentEntryOutcome::Reference(reference) => contents.push(reference),
            ContentEntryOutcome::Skipped(skip) => skipped.push(skip),
        }
    }

    Ok(PageContentsInspection {
        page_dictionary,
        contents_key_range: contents_entry.key_range,
        contents_value_range: contents_entry.value_range,
        value_shape: PageContentsValueShape::Array,
        contents,
        skipped,
    })
}

struct ScannedContentEntry {
    after_entry: usize,
    outcome: ContentEntryOutcome,
}

enum ContentEntryOutcome {
    Reference(PageContentReference),
    Skipped(SkippedPageContentEntry),
}

fn scan_content_entry(input: &[u8], start: usize, limit: usize) -> ScannedContentEntry {
    match input[start] {
        b'[' => match crate::inspect_array_extent(input, start) {
            Ok(array) if array.after_close_byte_offset <= limit => skipped_content_entry(
                start,
                array.after_close_byte_offset,
                SkippedPageContentEntryKind::Array,
            ),
            _ => skipped_content_entry(start, limit, SkippedPageContentEntryKind::Array),
        },
        b'<' if input.get(start + 1) == Some(&b'<') => {
            match crate::inspect_dictionary_extent(input, start) {
                Ok(dictionary) if dictionary.after_close_byte_offset <= limit => {
                    skipped_content_entry(
                        start,
                        dictionary.after_close_byte_offset,
                        SkippedPageContentEntryKind::Dictionary,
                    )
                }
                _ => skipped_content_entry(start, limit, SkippedPageContentEntryKind::Dictionary),
            }
        }
        b'(' => {
            let end = skip_literal_string(input, start)
                .unwrap_or(limit)
                .min(limit);
            skipped_content_entry(start, end, SkippedPageContentEntryKind::String)
        }
        b'<' => {
            let end = skip_hex_string(input, start).unwrap_or(limit).min(limit);
            skipped_content_entry(start, end, SkippedPageContentEntryKind::String)
        }
        b'/' => skipped_content_entry(
            start,
            skip_name(input, start, limit),
            SkippedPageContentEntryKind::Name,
        ),
        _ => scan_scalar_content_entry(input, start, limit),
    }
}

fn scan_scalar_content_entry(input: &[u8], start: usize, limit: usize) -> ScannedContentEntry {
    if looks_like_reference_candidate(input, start, limit) {
        return match crate::parse_indirect_reference(input, start) {
            Ok(reference) if reference.after_keyword_offset <= limit => ScannedContentEntry {
                after_entry: reference.after_keyword_offset,
                outcome: ContentEntryOutcome::Reference(PageContentReference {
                    reference: reference.reference,
                    reference_range: reference.reference_range,
                }),
            },
            Ok(_) => skipped_content_entry(
                start,
                limit,
                SkippedPageContentEntryKind::MalformedIndirectReference {
                    reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
                },
            ),
            Err(error) => {
                let end = reference_candidate_end(input, start, limit);
                skipped_content_entry(
                    start,
                    end,
                    SkippedPageContentEntryKind::MalformedIndirectReference {
                        reference_reason: error.reason,
                    },
                )
            }
        };
    }

    let end = skip_scalar_token(input, start, limit);
    skipped_content_entry(start, end, classify_scalar(input, start, end))
}

const fn skipped_content_entry(
    start: usize,
    end: usize,
    kind: SkippedPageContentEntryKind,
) -> ScannedContentEntry {
    ScannedContentEntry {
        after_entry: end,
        outcome: ContentEntryOutcome::Skipped(SkippedPageContentEntry {
            entry_range: DictionaryEntryByteRange { start, end },
            kind,
        }),
    }
}

fn looks_like_reference_candidate(input: &[u8], start: usize, limit: usize) -> bool {
    let first_end = skip_scalar_token(input, start, limit);
    if !token_is_unsigned_integer(&input[start..first_end]) {
        return false;
    }

    let second_start = skip_whitespace_and_comments(input, first_end, limit);
    if second_start >= limit || is_pdf_delimiter(input[second_start]) {
        return false;
    }
    let second_end = skip_scalar_token(input, second_start, limit);
    token_is_unsigned_integer(&input[second_start..second_end])
}

fn reference_candidate_end(input: &[u8], start: usize, limit: usize) -> usize {
    let first_end = skip_scalar_token(input, start, limit);
    let second_start = skip_whitespace_and_comments(input, first_end, limit);
    if second_start >= limit {
        return first_end;
    }
    let second_end = skip_scalar_token(input, second_start, limit);
    let third_start = skip_whitespace_and_comments(input, second_end, limit);
    if third_start >= limit || is_pdf_delimiter(input[third_start]) {
        return second_end;
    }
    skip_scalar_token(input, third_start, limit)
}

fn classify_scalar(input: &[u8], start: usize, end: usize) -> SkippedPageContentEntryKind {
    match &input[start..end] {
        b"true" | b"false" => SkippedPageContentEntryKind::Boolean,
        b"null" => SkippedPageContentEntryKind::Null,
        bytes if token_is_number_like(bytes) => SkippedPageContentEntryKind::NumberLike,
        _ => SkippedPageContentEntryKind::OtherScalar,
    }
}

fn token_is_unsigned_integer(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.iter().all(u8::is_ascii_digit)
}

fn token_is_number_like(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .any(|byte| byte.is_ascii_digit() || matches!(*byte, b'+' | b'-' | b'.'))
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(*byte, b'+' | b'-' | b'.'))
}

fn is_exact_contents_key(input: &[u8], key_range: DictionaryEntryByteRange) -> bool {
    input.get(key_range.start..key_range.end) == Some(CONTENTS_KEY)
}

const fn page_contents_error(
    input: &[u8],
    byte_offset: usize,
    page_header_byte_offset: Option<usize>,
    reason: PageContentsInspectionRejection,
    error_byte_offset: Option<usize>,
) -> PageContentsInspectionError {
    PageContentsInspectionError {
        byte_offset,
        byte_len: input.len(),
        page_header_byte_offset,
        error_byte_offset,
        reason,
    }
}
