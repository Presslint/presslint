use serde::{Deserialize, Serialize};

use crate::{
    DictionaryEntryByteRange, DictionaryValueKind, IndirectObjectDictionaryInspection,
    IndirectObjectDictionaryInspectionRejection, IndirectRef, IndirectReferenceInspectionRejection,
    ResolvedObjectData, ResolvedObjectDictionaryInspection, inspect_object_dictionary,
    object_dictionary::{
        compressed_dictionary_as_indirect_object_dictionary,
        resolved_dictionary_rejection_as_indirect,
    },
};

const PAGES_KEY: &[u8] = b"/Pages";

/// Parsed `/Pages` indirect reference from a catalog dictionary object.
///
/// This report stores only structural metadata. It does not retain or copy PDF
/// bytes, object bodies, stream bodies, page-tree dictionaries, page
/// dictionaries, contents streams, or referenced-object bytes, and it does not
/// resolve the parsed reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPagesInspection {
    /// Delegated catalog-shaped object dictionary inspection.
    pub catalog_dictionary: IndirectObjectDictionaryInspection,
    /// Byte range covering the exact top-level raw `/Pages` key.
    pub pages_key_range: DictionaryEntryByteRange,
    /// Byte range covering the `/Pages` value span.
    pub pages_value_range: DictionaryEntryByteRange,
    /// Parsed page-tree root indirect reference.
    pub pages_reference: IndirectRef,
}

/// Error returned when a catalog `/Pages` reference cannot be inspected.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPagesInspectionError {
    /// Caller-supplied byte offset where catalog object inspection began.
    pub byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Byte offset where the resolved catalog object header begins, when it was
    /// located.
    pub catalog_header_byte_offset: Option<usize>,
    /// Byte offset where the malformed or unsupported construct was found, when
    /// available.
    pub error_byte_offset: Option<usize>,
    /// Structured failure reason.
    pub reason: CatalogPagesInspectionRejection,
}

/// Structured catalog `/Pages` inspection rejection reasons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum CatalogPagesInspectionRejection {
    /// A delegated catalog dictionary inspection failed.
    CatalogDictionary {
        /// Underlying object dictionary rejection reason.
        catalog_dictionary_reason: IndirectObjectDictionaryInspectionRejection,
    },
    /// The catalog dictionary has no exact top-level raw `/Pages` key.
    MissingPages,
    /// The catalog dictionary has more than one exact top-level raw `/Pages`
    /// key.
    DuplicatePages {
        /// First `/Pages` key range observed in source order.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Pages` key range observed in source order.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Pages` value is not shaped as an indirect reference value span.
    NonReferencePagesValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The `/Pages` value was shaped as an indirect reference but did not
    /// parse.
    MalformedPagesReference {
        /// Underlying indirect reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
}

/// Inspect the top-level `/Pages` indirect reference from a catalog object.
///
/// The helper composes existing bounded inspectors: it reads the
/// catalog-shaped object with [`crate::inspect_indirect_object_dictionary`],
/// matches only the exact raw top-level key bytes `/Pages`, and parses that
/// value through [`crate::parse_indirect_reference`].
///
/// It does not decode PDF name escapes, interpret nested dictionaries, validate
/// `/Type /Catalog`, resolve the parsed page-tree reference, or inspect page
/// tree/page dictionaries or contents streams.
///
/// # Errors
///
/// Returns [`CatalogPagesInspectionError`] for a delegated catalog dictionary
/// inspection failure, a missing or duplicate exact `/Pages` key, a
/// non-reference `/Pages` value, or a malformed `/Pages` reference.
pub fn inspect_catalog_pages(
    input: &[u8],
    catalog_object_offset: usize,
) -> Result<CatalogPagesInspection, CatalogPagesInspectionError> {
    let catalog_dictionary =
        crate::inspect_indirect_object_dictionary(input, catalog_object_offset).map_err(
            |error| {
                catalog_pages_error(
                    input,
                    catalog_object_offset,
                    error.header_byte_offset,
                    CatalogPagesInspectionRejection::CatalogDictionary {
                        catalog_dictionary_reason: error.reason,
                    },
                    error.error_byte_offset,
                )
            },
        )?;

    catalog_pages_from_entries(
        input,
        input,
        catalog_object_offset,
        Some(catalog_dictionary.header_range.start),
        catalog_dictionary,
    )
}

/// Inspect catalog `/Pages` from body-aware resolved object data.
///
/// Uncompressed objects delegate to [`inspect_catalog_pages`]. Compressed
/// objects scan only the extracted member body and keep all dictionary entry
/// ranges member-body-relative.
///
/// # Errors
///
/// Returns [`CatalogPagesInspectionError`] for the same catalog dictionary and
/// `/Pages` shape failures as [`inspect_catalog_pages`].
pub fn inspect_catalog_pages_resolved(
    input: &[u8],
    resolved: &ResolvedObjectData,
) -> Result<CatalogPagesInspection, CatalogPagesInspectionError> {
    match resolved {
        ResolvedObjectData::Uncompressed { resolved } => {
            inspect_catalog_pages(input, resolved.object_byte_offset)
        }
        ResolvedObjectData::Compressed {
            decoded_object_stream,
            object_body_span,
            ..
        } => {
            let dictionary = inspect_object_dictionary(input, resolved).map_err(|error| {
                catalog_pages_error(
                    input,
                    0,
                    None,
                    CatalogPagesInspectionRejection::CatalogDictionary {
                        catalog_dictionary_reason: resolved_dictionary_rejection_as_indirect(
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
            catalog_pages_from_entries(
                input,
                body,
                0,
                None,
                compressed_dictionary_as_indirect_object_dictionary(compressed),
            )
        }
    }
}

fn catalog_pages_from_entries(
    input: &[u8],
    dictionary_source: &[u8],
    catalog_object_offset: usize,
    catalog_header_byte_offset: Option<usize>,
    catalog_dictionary: IndirectObjectDictionaryInspection,
) -> Result<CatalogPagesInspection, CatalogPagesInspectionError> {
    let mut pages_entry: Option<crate::DictionaryEntrySpan> = None;
    for entry in &catalog_dictionary.entries {
        if !is_exact_pages_key(dictionary_source, entry.key_range) {
            continue;
        }

        if let Some(first) = pages_entry {
            return Err(catalog_pages_error(
                input,
                catalog_object_offset,
                catalog_header_byte_offset,
                CatalogPagesInspectionRejection::DuplicatePages {
                    first_key_range: first.key_range,
                    duplicate_key_range: entry.key_range,
                },
                Some(entry.key_range.start),
            ));
        }

        pages_entry = Some(*entry);
    }

    let pages_entry = pages_entry.ok_or_else(|| {
        catalog_pages_error(
            input,
            catalog_object_offset,
            catalog_header_byte_offset,
            CatalogPagesInspectionRejection::MissingPages,
            Some(catalog_dictionary.dictionary_close_byte_offset),
        )
    })?;

    if !matches!(
        pages_entry.value_kind,
        DictionaryValueKind::IndirectReferenceLike | DictionaryValueKind::OtherScalar
    ) {
        return Err(catalog_pages_error(
            input,
            catalog_object_offset,
            catalog_header_byte_offset,
            CatalogPagesInspectionRejection::NonReferencePagesValue {
                value_kind: pages_entry.value_kind,
            },
            Some(pages_entry.value_range.start),
        ));
    }

    let pages_reference =
        crate::parse_indirect_reference(dictionary_source, pages_entry.value_range.start).map_err(
            |error| {
                catalog_pages_error(
                    input,
                    catalog_object_offset,
                    catalog_header_byte_offset,
                    CatalogPagesInspectionRejection::MalformedPagesReference {
                        reference_reason: error.reason,
                    },
                    error.error_byte_offset,
                )
            },
        )?;

    if pages_reference.after_keyword_offset != pages_entry.value_range.end {
        return Err(catalog_pages_error(
            input,
            catalog_object_offset,
            catalog_header_byte_offset,
            CatalogPagesInspectionRejection::MalformedPagesReference {
                reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
            },
            Some(pages_reference.after_keyword_offset),
        ));
    }

    Ok(CatalogPagesInspection {
        catalog_dictionary,
        pages_key_range: pages_entry.key_range,
        pages_value_range: pages_entry.value_range,
        pages_reference: pages_reference.reference,
    })
}

fn is_exact_pages_key(input: &[u8], key_range: DictionaryEntryByteRange) -> bool {
    input.get(key_range.start..key_range.end) == Some(PAGES_KEY)
}

const fn catalog_pages_error(
    input: &[u8],
    byte_offset: usize,
    catalog_header_byte_offset: Option<usize>,
    reason: CatalogPagesInspectionRejection,
    error_byte_offset: Option<usize>,
) -> CatalogPagesInspectionError {
    CatalogPagesInspectionError {
        byte_offset,
        byte_len: input.len(),
        catalog_header_byte_offset,
        error_byte_offset,
        reason,
    }
}
