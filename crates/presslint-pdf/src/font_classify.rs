//! Shallow classification of one `/Resources /Font` entry.
//!
//! "Font resource" here means one entry of a page's or form's `/Font`
//! sub-dictionary — the dictionary a `Tf` operand name selects. This module
//! records only structural facts about a reached font dictionary: the exact
//! `/Type` dictionary fact and the exact `/Subtype` class. It never resolves
//! or inspects the `/FontDescriptor` key, descendant fonts, encodings,
//! `CMap`s,
//! widths, `ToUnicode` maps, `CharProcs`, or font program streams, and it
//! makes no safety or admissibility judgement.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::page_resource_inheritance::{
    ResolveReferenceError, ResourceContext, resolve_reference, unique_entry,
};
use crate::{
    DictionaryEntryByteRange, DictionaryEntryInspectionError, DictionaryEntrySpan,
    DictionaryValueKind, IndirectObjectDictionaryInspectionError, IndirectRef,
    IndirectReferenceInspectionRejection, ObjectLookup, ObjectLookupLocation, PdfName,
    SkippedPageXObjectResourceReason, inspect_dictionary_entries,
    inspect_indirect_object_dictionary, parse_indirect_reference,
};

/// One classified `/Resources /Font` resource entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedFontResource {
    /// Resource name (without the leading slash) selected by the `Tf`
    /// operator. Raw source bytes are preserved; `#xx` escapes are not
    /// decoded and no collision semantics are applied.
    pub name: PdfName,
    /// Exact `/Type` dictionary fact, recorded without guessing.
    pub dictionary_type: FontDictionaryTypeFact,
    /// Exact `/Subtype` classification.
    pub subtype: FontSubtypeClass,
    /// Resolved indirect reference when the entry value was a reference.
    pub reference: Option<IndirectRef>,
    /// Resolved font object byte offset when the entry value was a reference.
    pub object_byte_offset: Option<usize>,
}

/// Exact `/Type` dictionary fact for one reached font dictionary.
///
/// The fact is recorded independently of the `/Subtype` class and never
/// upgrades or downgrades it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FontDictionaryTypeFact {
    /// The exact direct name `/Font`.
    Font,
    /// The `/Type` key is absent.
    Missing,
    /// Another direct name value, retained as raw bytes.
    OtherName {
        /// Raw name bytes without the leading slash.
        name: PdfName,
    },
    /// The `/Type` key occurred more than once.
    Duplicate {
        /// First `/Type` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Type` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Type` value was not a direct name. An indirect reference value
    /// is deliberately never resolved.
    NonName {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// Exact `/Subtype` classification for one reached font dictionary.
///
/// Classification uses exact byte equality on a direct name only. A reached
/// dictionary with a bad or absent `/Subtype` stays classified with an
/// explicit fail-closed class, never a guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FontSubtypeClass {
    /// `/Type1` (ISO 32000-1 Table 110).
    Type1,
    /// `/MMType1`, kept distinct from [`FontSubtypeClass::Type1`].
    MmType1,
    /// `/TrueType`.
    TrueType,
    /// `/Type0` composite font.
    Type0,
    /// `/Type3`.
    Type3,
    /// `/CIDFontType0`. CID fonts are descendants of Type 0 fonts and are not
    /// valid direct `Tf` operands (ISO 32000-1 §9.7.4.1); this class is never
    /// mapped to [`FontSubtypeClass::Type0`].
    CidFontType0,
    /// `/CIDFontType2`. Same invalid-direct-`Tf`-operand fact as
    /// [`FontSubtypeClass::CidFontType0`].
    CidFontType2,
    /// Any other direct name (for example `/Type1C`), retained as raw bytes.
    OtherName {
        /// Raw name bytes without the leading slash.
        name: PdfName,
    },
    /// The `/Subtype` key is absent.
    Missing,
    /// The `/Subtype` key occurred more than once.
    Duplicate {
        /// First `/Subtype` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Subtype` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Subtype` value was not a direct name. An indirect reference
    /// value is deliberately never resolved.
    NonName {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// One page- or form-local `/Font` resource diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedFontResource {
    /// Resolved page/form object byte offset.
    pub object_byte_offset: usize,
    /// Resource name when the diagnostic concerns one `/Font` entry.
    pub resource_name: Option<PdfName>,
    /// Structured skip reason.
    pub reason: SkippedFontResourceReason,
}

/// Structured reason a `/Font` resource was not classified.
///
/// This vocabulary covers failures that prevent obtaining or scanning the
/// effective resources or a font entry's dictionary. A REACHED dictionary
/// with a bad or absent `/Type` or `/Subtype` is not a skip: it stays a
/// classified resource with explicit fail-closed classes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedFontResourceReason {
    /// A `/Resources` inheritance-level diagnostic (delegated vocabulary).
    Resources {
        /// Delegated `/Resources` resolution/inheritance failure.
        resources_reason: SkippedPageXObjectResourceReason,
    },
    /// No effective `/Resources` dictionary was available.
    MissingFontResources,
    /// No `/Font` sub-dictionary was present in the effective resources.
    MissingFont,
    /// An effective `/Font` key occurred more than once.
    DuplicateFont {
        /// First `/Font` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Font` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The effective `/Font` value was not a direct dictionary.
    NonDictionaryFont {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The direct `/Font` dictionary could not be scanned.
    FontDictionaryFailed {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// A direct `/Font` dictionary repeated the same resource name.
    DuplicateFontName {
        /// First matching resource-name key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching resource-name key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// One `/Font` entry was not a direct dictionary or indirect reference.
    NonDictionaryEntry {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// An indirect `/Font` entry was shaped like a reference but malformed.
    MalformedResourceReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// An indirect `/Font` entry reference did not resolve.
    UnresolvedResourceReference {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number, when available.
        location: Option<ObjectLookupLocation>,
    },
    /// An indirect `/Font` entry resolved to a non-dictionary object.
    ResourceDictionaryFailed {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
}

/// Classify one `/Font` dictionary entry.
///
/// # Errors
///
/// Returns a structured skip reason when the entry is not a dictionary, an
/// indirect entry is malformed or unresolved, or a dictionary cannot be
/// scanned.
pub fn classify_font_entry(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    name: &PdfName,
    entry: DictionaryEntrySpan,
) -> Result<ClassifiedFontResource, SkippedFontResourceReason> {
    match entry.value_kind {
        DictionaryValueKind::Dictionary => {
            let entries = inspect_dictionary_entries(input, entry.value_range.start)
                .map_err(|error| SkippedFontResourceReason::FontDictionaryFailed { error })?;
            Ok(classify_dictionary(
                input,
                name.clone(),
                &entries.entries,
                None,
                None,
            ))
        }
        DictionaryValueKind::IndirectReferenceLike => {
            let reference =
                parse_indirect_reference(input, entry.value_range.start).map_err(|error| {
                    SkippedFontResourceReason::MalformedResourceReference {
                        reference_reason: error.reason,
                    }
                })?;
            let (_, object_byte_offset) = resolve_reference(lookup, reference.reference)
                .map_err(|error| unresolved_reference(&error))?;
            let dictionary = inspect_indirect_object_dictionary(input, object_byte_offset)
                .map_err(
                    |error| SkippedFontResourceReason::ResourceDictionaryFailed {
                        reference: reference.reference,
                        object_byte_offset,
                        error,
                    },
                )?;
            Ok(classify_dictionary(
                input,
                name.clone(),
                &dictionary.entries,
                Some(reference.reference),
                Some(object_byte_offset),
            ))
        }
        value_kind => Err(SkippedFontResourceReason::NonDictionaryEntry { value_kind }),
    }
}

/// Classify all entries in a direct `/Font` sub-dictionary.
pub fn classify_font_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    entries: Vec<DictionaryEntrySpan>,
    skipped: &mut Vec<SkippedFontResource>,
) -> Vec<ClassifiedFontResource> {
    let mut classified = Vec::new();
    let mut seen_names = BTreeMap::new();
    for entry in entries {
        let name = PdfName(input[entry.key_range.start + 1..entry.key_range.end].to_vec());
        if let Some(first_key_range) = seen_names.get(&name) {
            skipped.push(skipped_entry(
                object_byte_offset,
                Some(name),
                SkippedFontResourceReason::DuplicateFontName {
                    first_key_range: *first_key_range,
                    duplicate_key_range: entry.key_range,
                },
            ));
            continue;
        }
        seen_names.insert(name.clone(), entry.key_range);
        match classify_font_entry(input, lookup, &name, entry) {
            Ok(resource) => classified.push(resource),
            Err(reason) => skipped.push(skipped_entry(object_byte_offset, Some(name), reason)),
        }
    }
    classified
}

pub struct EffectiveFontResources {
    pub fonts: Vec<ClassifiedFontResource>,
    pub skipped: Vec<SkippedFontResource>,
}

pub fn inspect_effective_font_resource_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    context: &ResourceContext,
) -> EffectiveFontResources {
    let mut skipped = context
        .skips
        .iter()
        .cloned()
        .map(|reason| skipped_entry(object_byte_offset, None, resources_skip(reason)))
        .collect::<Vec<_>>();
    let Some(resources) = &context.resources else {
        skipped.push(skipped_entry(
            object_byte_offset,
            None,
            SkippedFontResourceReason::MissingFontResources,
        ));
        return effective_resources(Vec::new(), skipped);
    };

    let Some(font_entry) = (match unique_entry(input, &resources.entries, b"/Font") {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(skipped_entry(
                object_byte_offset,
                None,
                SkippedFontResourceReason::DuplicateFont {
                    first_key_range,
                    duplicate_key_range,
                },
            ));
            return effective_resources(Vec::new(), skipped);
        }
    }) else {
        skipped.push(skipped_entry(
            object_byte_offset,
            None,
            SkippedFontResourceReason::MissingFont,
        ));
        return effective_resources(Vec::new(), skipped);
    };

    if font_entry.value_kind != DictionaryValueKind::Dictionary {
        skipped.push(skipped_entry(
            object_byte_offset,
            None,
            SkippedFontResourceReason::NonDictionaryFont {
                value_kind: font_entry.value_kind,
            },
        ));
        return effective_resources(Vec::new(), skipped);
    }

    let entries = match inspect_dictionary_entries(input, font_entry.value_range.start) {
        Ok(entries) => entries,
        Err(error) => {
            skipped.push(skipped_entry(
                object_byte_offset,
                None,
                SkippedFontResourceReason::FontDictionaryFailed { error },
            ));
            return effective_resources(Vec::new(), skipped);
        }
    };

    let fonts = classify_font_entries(
        input,
        lookup,
        object_byte_offset,
        entries.entries,
        &mut skipped,
    );
    effective_resources(fonts, skipped)
}

fn effective_resources(
    mut fonts: Vec<ClassifiedFontResource>,
    skipped: Vec<SkippedFontResource>,
) -> EffectiveFontResources {
    fonts.sort_by(|left, right| left.name.cmp(&right.name));
    EffectiveFontResources { fonts, skipped }
}

#[must_use]
pub const fn skipped_entry(
    object_byte_offset: usize,
    resource_name: Option<PdfName>,
    reason: SkippedFontResourceReason,
) -> SkippedFontResource {
    SkippedFontResource {
        object_byte_offset,
        resource_name,
        reason,
    }
}

const fn resources_skip(reason: SkippedPageXObjectResourceReason) -> SkippedFontResourceReason {
    SkippedFontResourceReason::Resources {
        resources_reason: reason,
    }
}

fn classify_dictionary(
    input: &[u8],
    name: PdfName,
    entries: &[DictionaryEntrySpan],
    reference: Option<IndirectRef>,
    object_byte_offset: Option<usize>,
) -> ClassifiedFontResource {
    let (dictionary_type, subtype) = classify_font_dictionary_facts(input, entries);
    ClassifiedFontResource {
        name,
        dictionary_type,
        subtype,
        reference,
        object_byte_offset,
    }
}

/// Derive the exact `/Type` fact and `/Subtype` class from already-inspected
/// dictionary entries.
///
/// This is the single taxonomy source shared by the page/form `/Font`
/// resource classifier and the `ExtGState` `/Font` effect classifier, so the
/// two paths can never drift apart. It performs no resolution and no scan of
/// its own: callers hand it the entry spans of a dictionary they already
/// inspected. The enclosing module is private, so this stays crate-internal
/// and is not part of the public surface.
pub fn classify_font_dictionary_facts(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
) -> (FontDictionaryTypeFact, FontSubtypeClass) {
    (
        classify_type_fact(input, entries),
        classify_subtype(input, entries),
    )
}

fn classify_type_fact(input: &[u8], entries: &[DictionaryEntrySpan]) -> FontDictionaryTypeFact {
    match unique_entry(input, entries, b"/Type") {
        Err((first_key_range, duplicate_key_range)) => FontDictionaryTypeFact::Duplicate {
            first_key_range,
            duplicate_key_range,
        },
        Ok(None) => FontDictionaryTypeFact::Missing,
        Ok(Some(entry)) => match entry.value_kind {
            DictionaryValueKind::Name => {
                let name = value_name(input, entry);
                if name.0 == b"Font" {
                    FontDictionaryTypeFact::Font
                } else {
                    FontDictionaryTypeFact::OtherName { name }
                }
            }
            value_kind => FontDictionaryTypeFact::NonName { value_kind },
        },
    }
}

fn classify_subtype(input: &[u8], entries: &[DictionaryEntrySpan]) -> FontSubtypeClass {
    match unique_entry(input, entries, b"/Subtype") {
        Err((first_key_range, duplicate_key_range)) => FontSubtypeClass::Duplicate {
            first_key_range,
            duplicate_key_range,
        },
        Ok(None) => FontSubtypeClass::Missing,
        Ok(Some(entry)) => match entry.value_kind {
            DictionaryValueKind::Name => {
                let name = value_name(input, entry);
                match name.0.as_slice() {
                    b"Type1" => FontSubtypeClass::Type1,
                    b"MMType1" => FontSubtypeClass::MmType1,
                    b"TrueType" => FontSubtypeClass::TrueType,
                    b"Type0" => FontSubtypeClass::Type0,
                    b"Type3" => FontSubtypeClass::Type3,
                    b"CIDFontType0" => FontSubtypeClass::CidFontType0,
                    b"CIDFontType2" => FontSubtypeClass::CidFontType2,
                    _ => FontSubtypeClass::OtherName { name },
                }
            }
            value_kind => FontSubtypeClass::NonName { value_kind },
        },
    }
}

fn value_name(input: &[u8], entry: DictionaryEntrySpan) -> PdfName {
    PdfName(input[entry.value_range.start + 1..entry.value_range.end].to_vec())
}

const fn unresolved_reference(error: &ResolveReferenceError) -> SkippedFontResourceReason {
    match error {
        ResolveReferenceError::Unresolved {
            reference,
            location,
        } => SkippedFontResourceReason::UnresolvedResourceReference {
            reference: *reference,
            location: Some(*location),
        },
        ResolveReferenceError::GenerationMismatch { reference, .. } => {
            SkippedFontResourceReason::UnresolvedResourceReference {
                reference: *reference,
                location: None,
            }
        }
    }
}
