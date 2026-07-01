use serde::{Deserialize, Serialize};

use crate::{
    ArrayExtentInspection, ArrayExtentInspectionRejection, DictionaryEntryByteRange,
    DictionaryValueKind, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionRejection, ResolvedObjectData,
    ResolvedObjectDictionaryInspection, inspect_object_dictionary,
    object_dictionary::{
        compressed_dictionary_as_indirect_object_dictionary,
        resolved_dictionary_rejection_as_indirect,
    },
};

const KIDS_KEY: &[u8] = b"/Kids";
const COUNT_KEY: &[u8] = b"/Count";

/// Bounded `/Kids` array extent and `/Count` value span of a page-tree node.
///
/// This report stores only structural metadata. It does not retain or copy PDF
/// bytes, object bodies, stream bodies, page dictionaries, contents streams,
/// `/Kids` array elements, or referenced-object bytes; the `/Kids` array is
/// reported as offsets plus a nesting-depth scalar, and `/Count` as byte ranges
/// only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeNodeInspection {
    /// Delegated page-tree-node object dictionary inspection.
    pub node_dictionary: IndirectObjectDictionaryInspection,
    /// Byte range covering the exact top-level raw `/Kids` key.
    pub kids_key_range: DictionaryEntryByteRange,
    /// Byte range covering the `/Kids` value span.
    pub kids_value_range: DictionaryEntryByteRange,
    /// Balanced extent of the `/Kids` array value.
    pub kids_array_extent: ArrayExtentInspection,
    /// Byte range covering the exact top-level raw `/Count` key.
    pub count_key_range: DictionaryEntryByteRange,
    /// Byte range covering the `/Count` value span.
    pub count_value_range: DictionaryEntryByteRange,
}

/// Error returned when a page-tree node's `/Kids`/`/Count` cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeNodeInspectionError {
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
    pub reason: PageTreeNodeInspectionRejection,
}

/// Structured page-tree node inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageTreeNodeInspectionRejection {
    /// A delegated page-tree-node object dictionary inspection failed.
    NodeDictionary {
        /// Underlying object dictionary rejection reason.
        node_dictionary_reason: IndirectObjectDictionaryInspectionRejection,
    },
    /// The node dictionary has no exact top-level raw `/Kids` key.
    MissingKids,
    /// The node dictionary has more than one exact top-level raw `/Kids` key.
    DuplicateKids {
        /// First `/Kids` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Kids` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Kids` value is not shaped as a `[ ... ]` array value.
    NonArrayKidsValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The `/Kids` array value could not be bounded as a balanced extent.
    KidsArrayExtent {
        /// Underlying array extent rejection reason.
        array_reason: ArrayExtentInspectionRejection,
    },
    /// The node dictionary has no exact top-level raw `/Count` key.
    MissingCount,
    /// The node dictionary has more than one exact top-level raw `/Count` key.
    DuplicateCount {
        /// First `/Count` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Count` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Count` value is not a shallow number-like scalar.
    NonNumberCountValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// Inspect a page-tree node's top-level `/Kids` array extent and `/Count` span.
///
/// The helper composes existing bounded inspectors only: it reads the
/// page-tree-node object with [`crate::inspect_indirect_object_dictionary`],
/// matches the exact raw top-level key bytes `/Kids` and `/Count`, and bounds
/// the `/Kids` array value with [`crate::inspect_array_extent`].
///
/// It matches only the exact raw key bytes, without decoding PDF name escapes or
/// interpreting nested dictionaries/arrays. It does not scan `/Kids` array
/// elements, parse the `/Count` integer, descend into child page-tree nodes or
/// page objects, resolve `/Kids` references, follow `/Parent`, or require a
/// `/Type /Pages` entry.
///
/// # Errors
///
/// Returns [`PageTreeNodeInspectionError`] for a delegated object-dictionary
/// inspection failure, a missing or duplicate exact `/Kids` key, a non-array
/// `/Kids` value, a delegated `/Kids` array-extent failure, a missing or
/// duplicate exact `/Count` key, or a non-number-like `/Count` value.
pub fn inspect_page_tree_node(
    input: &[u8],
    node_object_offset: usize,
) -> Result<PageTreeNodeInspection, PageTreeNodeInspectionError> {
    let node_dictionary = crate::inspect_indirect_object_dictionary(input, node_object_offset)
        .map_err(|error| {
            page_tree_node_error(
                input,
                node_object_offset,
                error.header_byte_offset,
                PageTreeNodeInspectionRejection::NodeDictionary {
                    node_dictionary_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;
    page_tree_node_from_entries(
        input,
        input,
        node_object_offset,
        Some(node_dictionary.header_range.start),
        node_dictionary,
    )
}

/// Inspect a page-tree node from body-aware resolved object data.
///
/// # Errors
///
/// Returns [`PageTreeNodeInspectionError`] for the same node dictionary,
/// `/Kids`, and `/Count` shape failures as [`inspect_page_tree_node`].
pub fn inspect_page_tree_node_resolved(
    input: &[u8],
    resolved: &ResolvedObjectData,
) -> Result<PageTreeNodeInspection, PageTreeNodeInspectionError> {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => {
            inspect_page_tree_node(input, resolved.object_byte_offset)
        }
        ResolvedObjectData::Compressed {
            decoded_object_stream,
            object_body_span,
            ..
        } => {
            let dictionary = inspect_object_dictionary(input, resolved).map_err(|error| {
                page_tree_node_error(
                    input,
                    0,
                    None,
                    PageTreeNodeInspectionRejection::NodeDictionary {
                        node_dictionary_reason: resolved_dictionary_rejection_as_indirect(
                            error.reason,
                        ),
                    },
                    error.error_byte_offset,
                )
            })?;
            let body = decoded_object_stream
                .get(object_body_span.start..object_body_span.end)
                .unwrap_or(&[]);
            let ResolvedObjectDictionaryInspection::Compressed(compressed) = dictionary else {
                unreachable!("compressed resolved object must inspect as compressed")
            };
            page_tree_node_from_entries(
                input,
                body,
                0,
                None,
                compressed_dictionary_as_indirect_object_dictionary(compressed),
            )
        }
    }
}

fn page_tree_node_from_entries(
    input: &[u8],
    dictionary_source: &[u8],
    node_object_offset: usize,
    node_header_byte_offset: Option<usize>,
    node_dictionary: IndirectObjectDictionaryInspection,
) -> Result<PageTreeNodeInspection, PageTreeNodeInspectionError> {
    let kids_entry = find_unique_entry(dictionary_source, &node_dictionary.entries, KIDS_KEY)
        .map_err(|(first, duplicate)| {
            page_tree_node_error(
                input,
                node_object_offset,
                node_header_byte_offset,
                PageTreeNodeInspectionRejection::DuplicateKids {
                    first_key_range: first,
                    duplicate_key_range: duplicate,
                },
                Some(duplicate.start),
            )
        })?
        .ok_or_else(|| {
            page_tree_node_error(
                input,
                node_object_offset,
                node_header_byte_offset,
                PageTreeNodeInspectionRejection::MissingKids,
                Some(node_dictionary.dictionary_close_byte_offset),
            )
        })?;

    if kids_entry.value_kind != DictionaryValueKind::Array {
        return Err(page_tree_node_error(
            input,
            node_object_offset,
            node_header_byte_offset,
            PageTreeNodeInspectionRejection::NonArrayKidsValue {
                value_kind: kids_entry.value_kind,
            },
            Some(kids_entry.value_range.start),
        ));
    }

    let kids_array_extent =
        crate::inspect_array_extent(dictionary_source, kids_entry.value_range.start).map_err(
            |error| {
                page_tree_node_error(
                    input,
                    node_object_offset,
                    node_header_byte_offset,
                    PageTreeNodeInspectionRejection::KidsArrayExtent {
                        array_reason: error.reason,
                    },
                    error.error_byte_offset,
                )
            },
        )?;

    let count_entry = find_unique_entry(dictionary_source, &node_dictionary.entries, COUNT_KEY)
        .map_err(|(first, duplicate)| {
            page_tree_node_error(
                input,
                node_object_offset,
                node_header_byte_offset,
                PageTreeNodeInspectionRejection::DuplicateCount {
                    first_key_range: first,
                    duplicate_key_range: duplicate,
                },
                Some(duplicate.start),
            )
        })?
        .ok_or_else(|| {
            page_tree_node_error(
                input,
                node_object_offset,
                node_header_byte_offset,
                PageTreeNodeInspectionRejection::MissingCount,
                Some(node_dictionary.dictionary_close_byte_offset),
            )
        })?;

    if count_entry.value_kind != DictionaryValueKind::NumberLike {
        return Err(page_tree_node_error(
            input,
            node_object_offset,
            node_header_byte_offset,
            PageTreeNodeInspectionRejection::NonNumberCountValue {
                value_kind: count_entry.value_kind,
            },
            Some(count_entry.value_range.start),
        ));
    }

    Ok(PageTreeNodeInspection {
        node_dictionary,
        kids_key_range: kids_entry.key_range,
        kids_value_range: kids_entry.value_range,
        kids_array_extent,
        count_key_range: count_entry.key_range,
        count_value_range: count_entry.value_range,
    })
}

/// Locate the single top-level entry whose key matches `key` exactly.
///
/// Returns `Ok(None)` when no entry matches, `Ok(Some(entry))` for exactly one
/// match, and `Err((first, duplicate))` carrying the first and duplicate key
/// ranges when more than one entry matches.
fn find_unique_entry(
    input: &[u8],
    entries: &[crate::DictionaryEntrySpan],
    key: &[u8],
) -> Result<Option<crate::DictionaryEntrySpan>, (DictionaryEntryByteRange, DictionaryEntryByteRange)>
{
    let mut found: Option<crate::DictionaryEntrySpan> = None;
    for entry in entries {
        if input.get(entry.key_range.start..entry.key_range.end) != Some(key) {
            continue;
        }
        if let Some(first) = found {
            return Err((first.key_range, entry.key_range));
        }
        found = Some(*entry);
    }
    Ok(found)
}

const fn page_tree_node_error(
    input: &[u8],
    byte_offset: usize,
    node_header_byte_offset: Option<usize>,
    reason: PageTreeNodeInspectionRejection,
    error_byte_offset: Option<usize>,
) -> PageTreeNodeInspectionError {
    PageTreeNodeInspectionError {
        byte_offset,
        byte_len: input.len(),
        node_header_byte_offset,
        error_byte_offset,
        reason,
    }
}
