use serde::{Deserialize, Serialize};

use crate::source_utils::{
    is_pdf_delimiter, skip_hex_string, skip_literal_string, skip_name, skip_scalar_token,
    skip_whitespace_and_comments,
};
use crate::{
    DictionaryEntryByteRange, IndirectRef, IndirectReferenceByteRange,
    IndirectReferenceInspectionRejection, PageTreeNodeInspection, PageTreeNodeInspectionRejection,
};

/// Direct top-level `/Kids` indirect references from a page-tree node.
///
/// This report stores only the delegated page-tree-node inspection, parsed
/// indirect-reference ids, reference byte ranges, and shallow skipped-entry
/// diagnostics. It does not retain or copy PDF bytes, array bytes, child object
/// bytes, page dictionaries, page-tree dictionaries, contents streams, or
/// referenced-object bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeKidsInspection {
    /// Delegated page-tree-node inspection that bounded the `/Kids` array.
    pub node: PageTreeNodeInspection,
    /// Direct top-level `N G R` references in source order.
    pub kids: Vec<PageTreeKidReference>,
    /// Top-level `/Kids` entries that were not reported as direct references.
    pub skipped: Vec<SkippedPageTreeKid>,
}

/// One direct top-level page-tree kid reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeKidReference {
    /// Parsed child indirect reference.
    pub reference: IndirectRef,
    /// Byte range covering the parsed `N G R` reference.
    pub reference_range: IndirectReferenceByteRange,
}

/// One top-level `/Kids` entry skipped by page-tree kid inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedPageTreeKid {
    /// Byte range covering the skipped top-level entry.
    pub entry_range: DictionaryEntryByteRange,
    /// Shallow skipped-entry family.
    pub kind: SkippedPageTreeKidKind,
}

/// Shallow family for a skipped top-level `/Kids` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SkippedPageTreeKidKind {
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

/// Error returned when page-tree kid references cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeKidsInspectionError {
    /// Caller-supplied byte offset where page-tree-node inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the resolved node object header begins, when it was
    /// located.
    pub node_header_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: PageTreeKidsInspectionRejection,
}

/// Structured page-tree kid inspection rejection reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageTreeKidsInspectionRejection {
    /// A delegated page-tree-node inspection failed.
    PageTreeNode {
        /// Underlying page-tree-node rejection reason.
        node_reason: PageTreeNodeInspectionRejection,
    },
}

/// Inspect direct top-level indirect references in a page-tree node's `/Kids`.
///
/// The helper composes [`crate::inspect_page_tree_node`] to locate and bound the
/// `/Kids` array, then scans only the bytes between that array's outer `[` and
/// matching `]`. Direct top-level `N G R` entries are parsed through
/// [`crate::parse_indirect_reference`]. Nested arrays, dictionaries, strings,
/// names, numbers, booleans, nulls, and other scalar entries are reported as
/// shallow skips and are not descended into or interpreted as child references.
///
/// # Errors
///
/// Returns [`PageTreeKidsInspectionError`] when the delegated page-tree-node
/// inspection fails. Malformed or unsupported direct `/Kids` entries are
/// reported in [`PageTreeKidsInspection::skipped`] rather than failing the whole
/// inspection.
pub fn inspect_page_tree_kids(
    input: &[u8],
    node_object_offset: usize,
) -> Result<PageTreeKidsInspection, PageTreeKidsInspectionError> {
    let node = crate::inspect_page_tree_node(input, node_object_offset).map_err(|error| {
        PageTreeKidsInspectionError {
            byte_offset: node_object_offset,
            byte_len: input.len(),
            node_header_byte_offset: error.node_header_byte_offset,
            error_byte_offset: error.error_byte_offset,
            reason: PageTreeKidsInspectionRejection::PageTreeNode {
                node_reason: error.reason,
            },
        }
    })?;

    Ok(inspect_page_tree_kids_from_node(input, node))
}

pub fn inspect_page_tree_kids_from_node(
    dictionary_source: &[u8],
    node: PageTreeNodeInspection,
) -> PageTreeKidsInspection {
    let mut kids = Vec::new();
    let mut skipped = Vec::new();
    let mut cursor = node.kids_array_extent.open_byte_offset + 1;
    let body_end = node.kids_array_extent.close_byte_offset;

    while cursor < body_end {
        cursor = skip_whitespace_and_comments(dictionary_source, cursor, body_end);
        if cursor >= body_end {
            break;
        }

        let entry = scan_kid_entry(dictionary_source, cursor, body_end);
        cursor = entry.after_entry;
        match entry.outcome {
            KidEntryOutcome::Reference(kid) => kids.push(kid),
            KidEntryOutcome::Skipped(skip) => skipped.push(skip),
        }
    }

    PageTreeKidsInspection {
        node,
        kids,
        skipped,
    }
}

struct ScannedKidEntry {
    after_entry: usize,
    outcome: KidEntryOutcome,
}

enum KidEntryOutcome {
    Reference(PageTreeKidReference),
    Skipped(SkippedPageTreeKid),
}

fn scan_kid_entry(input: &[u8], start: usize, limit: usize) -> ScannedKidEntry {
    match input[start] {
        b'[' => match crate::inspect_array_extent(input, start) {
            Ok(array) if array.after_close_byte_offset <= limit => skipped_entry(
                start,
                array.after_close_byte_offset,
                SkippedPageTreeKidKind::Array,
            ),
            _ => skipped_entry(start, limit, SkippedPageTreeKidKind::Array),
        },
        b'<' if input.get(start + 1) == Some(&b'<') => {
            match crate::inspect_dictionary_extent(input, start) {
                Ok(dictionary) if dictionary.after_close_byte_offset <= limit => skipped_entry(
                    start,
                    dictionary.after_close_byte_offset,
                    SkippedPageTreeKidKind::Dictionary,
                ),
                _ => skipped_entry(start, limit, SkippedPageTreeKidKind::Dictionary),
            }
        }
        b'(' => {
            let end = skip_literal_string(input, start)
                .unwrap_or(limit)
                .min(limit);
            skipped_entry(start, end, SkippedPageTreeKidKind::String)
        }
        b'<' => {
            let end = skip_hex_string(input, start).unwrap_or(limit).min(limit);
            skipped_entry(start, end, SkippedPageTreeKidKind::String)
        }
        b'/' => skipped_entry(
            start,
            skip_name(input, start, limit),
            SkippedPageTreeKidKind::Name,
        ),
        _ => scan_scalar_kid_entry(input, start, limit),
    }
}

fn scan_scalar_kid_entry(input: &[u8], start: usize, limit: usize) -> ScannedKidEntry {
    if looks_like_reference_candidate(input, start, limit) {
        return match crate::parse_indirect_reference(input, start) {
            Ok(reference) if reference.after_keyword_offset <= limit => ScannedKidEntry {
                after_entry: reference.after_keyword_offset,
                outcome: KidEntryOutcome::Reference(PageTreeKidReference {
                    reference: reference.reference,
                    reference_range: reference.reference_range,
                }),
            },
            Ok(_) => skipped_entry(
                start,
                limit,
                SkippedPageTreeKidKind::MalformedIndirectReference {
                    reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
                },
            ),
            Err(error) => {
                let end = reference_candidate_end(input, start, limit);
                skipped_entry(
                    start,
                    end,
                    SkippedPageTreeKidKind::MalformedIndirectReference {
                        reference_reason: error.reason,
                    },
                )
            }
        };
    }

    let end = skip_scalar_token(input, start, limit);
    skipped_entry(start, end, classify_scalar(input, start, end))
}

const fn skipped_entry(start: usize, end: usize, kind: SkippedPageTreeKidKind) -> ScannedKidEntry {
    ScannedKidEntry {
        after_entry: end,
        outcome: KidEntryOutcome::Skipped(SkippedPageTreeKid {
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

fn classify_scalar(input: &[u8], start: usize, end: usize) -> SkippedPageTreeKidKind {
    match &input[start..end] {
        b"true" | b"false" => SkippedPageTreeKidKind::Boolean,
        b"null" => SkippedPageTreeKidKind::Null,
        bytes if token_is_number_like(bytes) => SkippedPageTreeKidKind::NumberLike,
        _ => SkippedPageTreeKidKind::OtherScalar,
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
