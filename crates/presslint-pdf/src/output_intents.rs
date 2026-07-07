use serde::{Deserialize, Serialize};

use crate::page_resource_inheritance::unique_entry;
use crate::source_utils::{
    is_pdf_whitespace, skip_name, skip_scalar_token, skip_whitespace_and_comments,
};
use crate::{
    DictionaryEntryByteRange, DictionaryEntryInspectionError, DictionaryEntrySpan,
    DictionaryValueKind, DocumentAccessError, IndirectRef, ObjectLookup, ObjectResolutionError,
    ResolvedObjectPosition, inspect_array_extent, inspect_dictionary_entries,
    inspect_document_access, inspect_indirect_object_dictionary, parse_indirect_reference,
    resolve_xref_object_offset,
};

/// Catalog `/OutputIntents` structural observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputIntentsInspection {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Resolved catalog object byte offset for uncompressed catalogs.
    pub catalog_object_byte_offset: Option<usize>,
    /// Classified output-intent identity facts.
    pub output_intents: Vec<PdfOutputIntentFact>,
    /// Structured diagnostics for malformed or unsupported entries.
    pub skipped: Vec<SkippedOutputIntent>,
}

/// Output intent subtype names supported by the neutral PDF observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PdfOutputIntentSubtype {
    /// `/GTS_PDFX`.
    GtsPdfx,
    /// `/GTS_PDFA1`.
    GtsPdfa1,
    /// `/ISO_PDFE1`.
    IsoPdfe1,
}

/// One fully classified output-intent identity fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfOutputIntentFact {
    /// Zero-based index inside the catalog `/OutputIntents` array.
    pub index: usize,
    /// Supported `/S` subtype.
    pub subtype: PdfOutputIntentSubtype,
    /// Decoded simple ASCII `/OutputConditionIdentifier` text string.
    pub output_condition_identifier: String,
    /// Structural `/DestOutputProfile` presence/reference fact.
    pub dest_output_profile: DestOutputProfileFact,
}

/// Structural fact for an output intent's `/DestOutputProfile` entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DestOutputProfileFact {
    /// Whether the key was present.
    pub present: bool,
    /// Indirect stream reference when the value was shaped as `N G R`.
    pub reference: Option<IndirectRef>,
    /// Shallow value kind when the key was present.
    pub value_kind: Option<DictionaryValueKind>,
}

/// One output-intent observer diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedOutputIntent {
    /// Zero-based array index when the diagnostic concerns one array entry.
    pub index: Option<usize>,
    /// Structured skip reason.
    pub reason: SkippedOutputIntentReason,
}

/// Structured reason an output-intent fact was not classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedOutputIntentReason {
    /// Catalog object was compressed; this report-only observer does not retain
    /// decoded object-stream bytes after document access.
    UnsupportedCompressedCatalog,
    /// Catalog dictionary inspection failed.
    CatalogDictionaryFailed {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// `/OutputIntents` key occurred more than once.
    DuplicateOutputIntents {
        /// First `/OutputIntents` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/OutputIntents` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// `/OutputIntents` was present but not an array.
    NonArrayOutputIntents {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The `/OutputIntents` array could not be scanned.
    MalformedOutputIntentsArray,
    /// An array entry was not a direct or indirect dictionary.
    NonDictionaryOutputIntent {
        /// Shallow element value kind.
        value_kind: OutputIntentArrayEntryKind,
    },
    /// An indirect output-intent dictionary reference did not resolve.
    UnresolvedOutputIntentReference {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Delegated object resolution failure.
        error: ObjectResolutionError,
    },
    /// An indirect output-intent object was not a dictionary.
    OutputIntentDictionaryFailed {
        /// Delegated dictionary inspection failure.
        error: crate::IndirectObjectDictionaryInspectionError,
    },
    /// `/S` was missing.
    MissingSubtype,
    /// `/S` occurred more than once.
    DuplicateSubtype {
        /// First `/S` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/S` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// `/S` was not a name.
    NonNameSubtype {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// `/S` was a name outside the supported observer mapping.
    UnsupportedSubtype {
        /// Raw name bytes including the leading slash.
        name: Vec<u8>,
    },
    /// `/OutputConditionIdentifier` was missing.
    MissingOutputConditionIdentifier,
    /// `/OutputConditionIdentifier` occurred more than once.
    DuplicateOutputConditionIdentifier {
        /// First key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// `/OutputConditionIdentifier` was not a string.
    NonStringOutputConditionIdentifier {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The text string uses a form outside this slice's minimal decoder.
    UnsupportedOutputConditionIdentifierString,
}

/// Shallow kind of one `/OutputIntents` array element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputIntentArrayEntryKind {
    /// Direct `<< ... >>` dictionary.
    Dictionary,
    /// Indirect `N G R` reference.
    IndirectReference,
    /// Other array element shape.
    Other,
}

/// Error returned when the document-access spine cannot find the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputIntentsInspectionError {
    /// Total source length.
    pub byte_len: usize,
    /// Delegated document-access failure.
    pub error: Box<DocumentAccessError>,
}

/// Inspect catalog-level `/OutputIntents` using the existing document-access
/// spine.
///
/// # Errors
///
/// Returns an error only when the document-access spine cannot resolve the
/// catalog. Malformed or unsupported `/OutputIntents` entries are reported as
/// structured diagnostics in a successful inspection.
pub fn inspect_document_output_intents(
    input: &[u8],
) -> Result<OutputIntentsInspection, OutputIntentsInspectionError> {
    let access = inspect_document_access(input).map_err(|error| OutputIntentsInspectionError {
        byte_len: input.len(),
        error: Box::new(error),
    })?;

    let lookup = match &access.backend {
        crate::DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        crate::DocumentAccessBackend::ClassicXrefChain { chain } => {
            ObjectLookup::ClassicXrefChain(chain)
        }
        crate::DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        crate::DocumentAccessBackend::XrefStreamChain { chain } => {
            ObjectLookup::XrefStreamChain(chain)
        }
    };

    match access.catalog.position {
        ResolvedObjectPosition::Uncompressed {
            object_byte_offset, ..
        } => Ok(inspect_catalog_output_intents(
            input,
            lookup,
            object_byte_offset,
        )),
        ResolvedObjectPosition::Compressed { .. } => Ok(OutputIntentsInspection {
            byte_len: input.len(),
            catalog_object_byte_offset: None,
            output_intents: Vec::new(),
            skipped: vec![SkippedOutputIntent {
                index: None,
                reason: SkippedOutputIntentReason::UnsupportedCompressedCatalog,
            }],
        }),
    }
}

/// Inspect `/OutputIntents` from a resolved uncompressed catalog dictionary.
#[must_use]
pub fn inspect_catalog_output_intents(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    catalog_object_byte_offset: usize,
) -> OutputIntentsInspection {
    let catalog = match inspect_indirect_object_dictionary(input, catalog_object_byte_offset) {
        Ok(catalog) => catalog,
        Err(error) => {
            return catalog_report(
                input,
                catalog_object_byte_offset,
                Vec::new(),
                vec![SkippedOutputIntent {
                    index: None,
                    reason: SkippedOutputIntentReason::OutputIntentDictionaryFailed { error },
                }],
            );
        }
    };

    let mut skipped = Vec::new();
    let Some(entry) = (match unique_entry(input, &catalog.entries, b"/OutputIntents") {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(SkippedOutputIntent {
                index: None,
                reason: SkippedOutputIntentReason::DuplicateOutputIntents {
                    first_key_range,
                    duplicate_key_range,
                },
            });
            None
        }
    }) else {
        return catalog_report(input, catalog_object_byte_offset, Vec::new(), skipped);
    };

    if entry.value_kind != DictionaryValueKind::Array {
        skipped.push(SkippedOutputIntent {
            index: None,
            reason: SkippedOutputIntentReason::NonArrayOutputIntents {
                value_kind: entry.value_kind,
            },
        });
        return catalog_report(input, catalog_object_byte_offset, Vec::new(), skipped);
    }

    let Ok(entries) = array_entries(input, entry.value_range.start) else {
        skipped.push(SkippedOutputIntent {
            index: None,
            reason: SkippedOutputIntentReason::MalformedOutputIntentsArray,
        });
        return catalog_report(input, catalog_object_byte_offset, Vec::new(), skipped);
    };

    let mut output_intents = Vec::new();
    for (index, array_entry) in entries.into_iter().enumerate() {
        match classify_output_intent(input, lookup, index, array_entry) {
            Ok(fact) => output_intents.push(fact),
            Err(reason) => skipped.push(SkippedOutputIntent {
                index: Some(index),
                reason,
            }),
        }
    }

    catalog_report(input, catalog_object_byte_offset, output_intents, skipped)
}

const fn catalog_report(
    input: &[u8],
    catalog_object_byte_offset: usize,
    output_intents: Vec<PdfOutputIntentFact>,
    skipped: Vec<SkippedOutputIntent>,
) -> OutputIntentsInspection {
    OutputIntentsInspection {
        byte_len: input.len(),
        catalog_object_byte_offset: Some(catalog_object_byte_offset),
        output_intents,
        skipped,
    }
}

fn classify_output_intent(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    index: usize,
    entry: ArrayEntry,
) -> Result<PdfOutputIntentFact, SkippedOutputIntentReason> {
    let entries = match entry.kind {
        OutputIntentArrayEntryKind::Dictionary => {
            inspect_dictionary_entries(input, entry.start)
                .map_err(|error| SkippedOutputIntentReason::CatalogDictionaryFailed { error })?
                .entries
        }
        OutputIntentArrayEntryKind::IndirectReference => {
            let reference = parse_indirect_reference(input, entry.start)
                .map_err(|_| SkippedOutputIntentReason::NonDictionaryOutputIntent {
                    value_kind: entry.kind,
                })?
                .reference;
            let resolved =
                resolve_xref_object_offset(input, lookup, reference).map_err(|error| {
                    SkippedOutputIntentReason::UnresolvedOutputIntentReference { reference, error }
                })?;
            inspect_indirect_object_dictionary(input, resolved.object_byte_offset)
                .map_err(|error| SkippedOutputIntentReason::OutputIntentDictionaryFailed { error })?
                .entries
        }
        OutputIntentArrayEntryKind::Other => {
            return Err(SkippedOutputIntentReason::NonDictionaryOutputIntent {
                value_kind: entry.kind,
            });
        }
    };

    let subtype = classify_subtype(input, &entries)?;
    let output_condition_identifier = classify_identifier(input, &entries)?;
    let dest_output_profile = classify_dest_output_profile(input, &entries);

    Ok(PdfOutputIntentFact {
        index,
        subtype,
        output_condition_identifier,
        dest_output_profile,
    })
}

fn classify_subtype(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
) -> Result<PdfOutputIntentSubtype, SkippedOutputIntentReason> {
    let entry =
        unique_entry(input, entries, b"/S").map_err(|(first_key_range, duplicate_key_range)| {
            SkippedOutputIntentReason::DuplicateSubtype {
                first_key_range,
                duplicate_key_range,
            }
        })?;
    let Some(entry) = entry else {
        return Err(SkippedOutputIntentReason::MissingSubtype);
    };
    if entry.value_kind != DictionaryValueKind::Name {
        return Err(SkippedOutputIntentReason::NonNameSubtype {
            value_kind: entry.value_kind,
        });
    }
    match &input[entry.value_range.start..entry.value_range.end] {
        b"/GTS_PDFX" => Ok(PdfOutputIntentSubtype::GtsPdfx),
        b"/GTS_PDFA1" => Ok(PdfOutputIntentSubtype::GtsPdfa1),
        b"/ISO_PDFE1" => Ok(PdfOutputIntentSubtype::IsoPdfe1),
        other => Err(SkippedOutputIntentReason::UnsupportedSubtype {
            name: other.to_vec(),
        }),
    }
}

fn classify_identifier(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
) -> Result<String, SkippedOutputIntentReason> {
    let entry = unique_entry(input, entries, b"/OutputConditionIdentifier").map_err(
        |(first_key_range, duplicate_key_range)| {
            SkippedOutputIntentReason::DuplicateOutputConditionIdentifier {
                first_key_range,
                duplicate_key_range,
            }
        },
    )?;
    let Some(entry) = entry else {
        return Err(SkippedOutputIntentReason::MissingOutputConditionIdentifier);
    };
    if entry.value_kind != DictionaryValueKind::String {
        return Err(
            SkippedOutputIntentReason::NonStringOutputConditionIdentifier {
                value_kind: entry.value_kind,
            },
        );
    }
    decode_simple_pdf_string(&input[entry.value_range.start..entry.value_range.end])
        .ok_or(SkippedOutputIntentReason::UnsupportedOutputConditionIdentifierString)
}

fn classify_dest_output_profile(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
) -> DestOutputProfileFact {
    let Some(entry) = unique_entry(input, entries, b"/DestOutputProfile")
        .ok()
        .flatten()
    else {
        return DestOutputProfileFact {
            present: false,
            reference: None,
            value_kind: None,
        };
    };
    let reference = if entry.value_kind == DictionaryValueKind::IndirectReferenceLike {
        parse_indirect_reference(input, entry.value_range.start)
            .ok()
            .map(|parsed| parsed.reference)
    } else {
        None
    };
    DestOutputProfileFact {
        present: true,
        reference,
        value_kind: Some(entry.value_kind),
    }
}

#[derive(Debug, Clone, Copy)]
struct ArrayEntry {
    start: usize,
    end: usize,
    kind: OutputIntentArrayEntryKind,
}

const MAX_OUTPUT_INTENT_ARRAY_ENTRIES: usize = 256;

fn array_entries(input: &[u8], open_offset: usize) -> Result<Vec<ArrayEntry>, ()> {
    let extent = inspect_array_extent(input, open_offset).map_err(|_| ())?;
    let mut cursor = extent.open_byte_offset + 1;
    let limit = extent.close_byte_offset;
    let mut entries = Vec::new();
    while entries.len() < MAX_OUTPUT_INTENT_ARRAY_ENTRIES {
        cursor = skip_whitespace_and_comments(input, cursor, limit);
        if cursor >= limit {
            break;
        }
        let entry = if input[cursor] == b'<' && input.get(cursor + 1) == Some(&b'<') {
            let extent = crate::inspect_dictionary_extent(input, cursor).map_err(|_| ())?;
            ArrayEntry {
                start: cursor,
                end: extent.after_close_byte_offset,
                kind: OutputIntentArrayEntryKind::Dictionary,
            }
        } else if let Ok(reference) = parse_indirect_reference(input, cursor) {
            ArrayEntry {
                start: cursor,
                end: reference.reference_range.end,
                kind: OutputIntentArrayEntryKind::IndirectReference,
            }
        } else {
            let end = if input[cursor] == b'/' {
                skip_name(input, cursor, limit)
            } else {
                skip_scalar_token(input, cursor, limit)
            };
            ArrayEntry {
                start: cursor,
                end,
                kind: OutputIntentArrayEntryKind::Other,
            }
        };
        cursor = entry.end;
        entries.push(entry);
    }
    Ok(entries)
}

fn decode_simple_pdf_string(bytes: &[u8]) -> Option<String> {
    match bytes.first()? {
        b'(' => decode_simple_literal(bytes),
        b'<' => decode_simple_hex(bytes),
        _ => None,
    }
}

fn decode_simple_literal(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || bytes.first() != Some(&b'(') || bytes.last() != Some(&b')') {
        return None;
    }
    let mut out = String::new();
    let mut cursor = 1;
    while cursor + 1 < bytes.len() {
        let byte = bytes[cursor];
        if byte == b'\\' {
            cursor += 1;
            let escaped = *bytes.get(cursor)?;
            let mapped = match escaped {
                b'n' => b'\n',
                b'r' => b'\r',
                b't' => b'\t',
                b'b' => 0x08,
                b'f' => 0x0c,
                b'(' | b')' | b'\\' => escaped,
                _ => return None,
            };
            out.push(char::from(mapped));
        } else if byte.is_ascii() && !byte.is_ascii_control() {
            out.push(char::from(byte));
        } else {
            return None;
        }
        cursor += 1;
    }
    Some(out)
}

fn decode_simple_hex(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 2 || bytes.first() != Some(&b'<') || bytes.last() != Some(&b'>') {
        return None;
    }
    let mut nibbles = Vec::new();
    for byte in &bytes[1..bytes.len() - 1] {
        if is_pdf_whitespace(*byte) {
            continue;
        }
        nibbles.push(hex_value(*byte)?);
    }
    if nibbles.len() % 2 != 0 {
        nibbles.push(0);
    }
    let mut out = String::new();
    for pair in nibbles.chunks(2) {
        let byte = (pair[0] << 4) | pair[1];
        if byte.is_ascii() && !byte.is_ascii_control() {
            out.push(char::from(byte));
        } else {
            return None;
        }
    }
    Some(out)
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
