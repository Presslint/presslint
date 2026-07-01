use serde::{Deserialize, Serialize};

use crate::{
    DictionaryEntryByteRange, DictionaryValueKind, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionRejection, ResolvedObjectData,
    ResolvedObjectDictionaryInspection, inspect_object_dictionary,
    object_dictionary::{
        compressed_dictionary_as_indirect_object_dictionary,
        resolved_dictionary_rejection_as_indirect,
    },
};

const TYPE_KEY: &[u8] = b"/Type";
const PAGES_NAME: &[u8] = b"/Pages";
const PAGE_NAME: &[u8] = b"/Page";

/// Structural node kind classified from a page-tree object's `/Type` name value.
///
/// The classification compares the exact raw `/Type` name value bytes. It
/// decodes no PDF name escapes, so an escaped form such as `/Page#73` is
/// reported as [`PageTreeNodeType::Other`], not as [`PageTreeNodeType::Page`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageTreeNodeType {
    /// An intermediate page-tree node (`/Type /Pages`).
    Pages,
    /// A leaf page object (`/Type /Page`).
    Page,
    /// Any other exact raw name value; read the bytes from the value range.
    Other,
}

/// Classified `/Type` node kind of an already-located page-tree object.
///
/// This report stores only structural metadata. It does not retain or copy PDF
/// bytes, object bodies, stream bodies, page dictionaries, contents streams,
/// `/Type` name bytes, or referenced-object bytes; the `/Type` key and value are
/// reported as byte ranges only, and the node kind as a small enum. An
/// [`PageTreeNodeType::Other`] value stays addressed by `type_value_range`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeNodeTypeInspection {
    /// Delegated page-tree object dictionary inspection.
    pub object_dictionary: IndirectObjectDictionaryInspection,
    /// Byte range covering the exact top-level raw `/Type` key.
    pub type_key_range: DictionaryEntryByteRange,
    /// Byte range covering the `/Type` value span.
    pub type_value_range: DictionaryEntryByteRange,
    /// Classified structural node kind.
    pub node_type: PageTreeNodeType,
}

/// Error returned when a page-tree object's `/Type` node kind cannot be
/// classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageTreeNodeTypeInspectionError {
    /// Caller-supplied byte offset where classification began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the resolved object header begins, when it was located.
    pub object_header_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: PageTreeNodeTypeInspectionRejection,
}

/// Structured page-tree node-type classification rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum PageTreeNodeTypeInspectionRejection {
    /// A delegated object dictionary inspection failed.
    ObjectDictionary {
        /// Underlying object dictionary rejection reason.
        object_dictionary_reason: IndirectObjectDictionaryInspectionRejection,
    },
    /// The object dictionary has no exact top-level raw `/Type` key.
    MissingType,
    /// The object dictionary has more than one exact top-level raw `/Type` key.
    DuplicateType {
        /// First `/Type` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Type` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Type` value is not shaped as a `/Name` value.
    NonNameTypeValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// Classify an already-located page-tree object's `/Type` node kind.
///
/// The helper composes existing bounded inspectors only: it reads the object's
/// top-level entries with [`crate::inspect_indirect_object_dictionary`], matches
/// the single exact raw top-level key bytes `/Type`, validates the value kind is
/// [`DictionaryValueKind::Name`], and classifies the value's exact raw bytes
/// against `/Pages` and `/Page`. Any other name value is
/// [`PageTreeNodeType::Other`].
///
/// It matches only the exact raw key and value bytes, without decoding PDF name
/// escapes, so an escaped form such as `/Page#73` classifies as
/// [`PageTreeNodeType::Other`]. It never resolves references, follows `/Kids`,
/// `/Parent`, or `/Contents`, or descends into child page-tree nodes or page
/// dictionaries.
///
/// # Errors
///
/// Returns [`PageTreeNodeTypeInspectionError`] for a delegated object-dictionary
/// inspection failure, a missing or duplicate exact `/Type` key, or a non-name
/// `/Type` value.
pub fn inspect_page_tree_node_type(
    input: &[u8],
    object_offset: usize,
) -> Result<PageTreeNodeTypeInspection, PageTreeNodeTypeInspectionError> {
    let object_dictionary = crate::inspect_indirect_object_dictionary(input, object_offset)
        .map_err(|error| {
            page_tree_node_type_error(
                input,
                object_offset,
                error.header_byte_offset,
                PageTreeNodeTypeInspectionRejection::ObjectDictionary {
                    object_dictionary_reason: error.reason,
                },
                error.error_byte_offset,
            )
        })?;
    page_tree_node_type_from_entries(
        input,
        input,
        object_offset,
        Some(object_dictionary.header_range.start),
        object_dictionary,
    )
}

/// Classify a page-tree object's `/Type` from body-aware resolved object data.
///
/// # Errors
///
/// Returns [`PageTreeNodeTypeInspectionError`] for the same object dictionary
/// and `/Type` shape failures as [`inspect_page_tree_node_type`].
pub fn inspect_page_tree_node_type_resolved(
    input: &[u8],
    resolved: &ResolvedObjectData,
) -> Result<PageTreeNodeTypeInspection, PageTreeNodeTypeInspectionError> {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => {
            inspect_page_tree_node_type(input, resolved.object_byte_offset)
        }
        ResolvedObjectData::Compressed {
            decoded_object_stream,
            object_body_span,
            ..
        } => {
            let dictionary = inspect_object_dictionary(input, resolved).map_err(|error| {
                page_tree_node_type_error(
                    input,
                    0,
                    None,
                    PageTreeNodeTypeInspectionRejection::ObjectDictionary {
                        object_dictionary_reason: resolved_dictionary_rejection_as_indirect(
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
            page_tree_node_type_from_entries(
                input,
                body,
                0,
                None,
                compressed_dictionary_as_indirect_object_dictionary(compressed),
            )
        }
    }
}

fn page_tree_node_type_from_entries(
    input: &[u8],
    dictionary_source: &[u8],
    object_offset: usize,
    object_header_byte_offset: Option<usize>,
    object_dictionary: IndirectObjectDictionaryInspection,
) -> Result<PageTreeNodeTypeInspection, PageTreeNodeTypeInspectionError> {
    let type_entry = find_unique_entry(dictionary_source, &object_dictionary.entries, TYPE_KEY)
        .map_err(|(first, duplicate)| {
            page_tree_node_type_error(
                input,
                object_offset,
                object_header_byte_offset,
                PageTreeNodeTypeInspectionRejection::DuplicateType {
                    first_key_range: first,
                    duplicate_key_range: duplicate,
                },
                Some(duplicate.start),
            )
        })?
        .ok_or_else(|| {
            page_tree_node_type_error(
                input,
                object_offset,
                object_header_byte_offset,
                PageTreeNodeTypeInspectionRejection::MissingType,
                Some(object_dictionary.dictionary_close_byte_offset),
            )
        })?;

    if type_entry.value_kind != DictionaryValueKind::Name {
        return Err(page_tree_node_type_error(
            input,
            object_offset,
            object_header_byte_offset,
            PageTreeNodeTypeInspectionRejection::NonNameTypeValue {
                value_kind: type_entry.value_kind,
            },
            Some(type_entry.value_range.start),
        ));
    }

    let node_type = classify_type_name(dictionary_source, type_entry.value_range);

    Ok(PageTreeNodeTypeInspection {
        object_dictionary,
        type_key_range: type_entry.key_range,
        type_value_range: type_entry.value_range,
        node_type,
    })
}

/// Classify the exact raw `/Type` value name bytes without decoding escapes.
fn classify_type_name(input: &[u8], value_range: DictionaryEntryByteRange) -> PageTreeNodeType {
    match input.get(value_range.start..value_range.end) {
        Some(PAGES_NAME) => PageTreeNodeType::Pages,
        Some(PAGE_NAME) => PageTreeNodeType::Page,
        _ => PageTreeNodeType::Other,
    }
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

const fn page_tree_node_type_error(
    input: &[u8],
    byte_offset: usize,
    object_header_byte_offset: Option<usize>,
    reason: PageTreeNodeTypeInspectionRejection,
    error_byte_offset: Option<usize>,
) -> PageTreeNodeTypeInspectionError {
    PageTreeNodeTypeInspectionError {
        byte_offset,
        byte_len: input.len(),
        object_header_byte_offset,
        error_byte_offset,
        reason,
    }
}
