//! Shallow classification of one page/form `/Group` attributes dictionary.
//!
//! This module records only Phase-1 transparency-group safety facts. It does
//! not composite, resolve resource colour spaces, or invent defaults for absent
//! `/CS`, `/I`, or `/K` entries.

use serde::{Deserialize, Serialize};

use crate::page_resource_inheritance::{ResolveReferenceError, resolve_reference, unique_entry};
use crate::{
    DictionaryEntryByteRange, DictionaryEntryInspectionError, DictionaryEntrySpan,
    DictionaryValueKind, IndirectObjectDictionaryInspectionError, IndirectRef,
    IndirectReferenceInspectionRejection, ObjectLookup, ObjectLookupLocation, PdfName,
    inspect_dictionary_entries, inspect_indirect_object_dictionary, parse_indirect_reference,
};

/// Classified top-level `/Group` attributes for one page or Form `XObject`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedTransparencyGroup {
    /// `/S /Transparency` was present and recognised.
    pub transparency: bool,
    /// Blend colour-space value shape when `/CS` is present.
    pub color_space: TransparencyGroupParamClass<TransparencyGroupColorSpace>,
    /// `/I` isolated flag as written.
    pub isolated: TransparencyGroupParamClass<bool>,
    /// `/K` knockout flag as written.
    pub knockout: TransparencyGroupParamClass<bool>,
    /// True when the group dictionary contains keys outside `/Type`, `/S`,
    /// `/CS`, `/I`, and `/K`.
    pub has_unclassified_keys: bool,
}

impl ClassifiedTransparencyGroup {
    /// True when any safety field is malformed or the dictionary carries keys
    /// outside this Phase-1 classifier.
    #[must_use]
    pub const fn has_unclassified_safety_field(&self) -> bool {
        matches!(
            self.color_space,
            TransparencyGroupParamClass::Malformed { .. }
                | TransparencyGroupParamClass::Duplicate { .. }
        ) || matches!(
            self.isolated,
            TransparencyGroupParamClass::Malformed { .. }
                | TransparencyGroupParamClass::Duplicate { .. }
        ) || matches!(
            self.knockout,
            TransparencyGroupParamClass::Malformed { .. }
                | TransparencyGroupParamClass::Duplicate { .. }
        ) || self.has_unclassified_keys
    }
}

/// Per-field group classification. `Unset` preserves absence exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum TransparencyGroupParamClass<T> {
    /// The key is absent from this group dictionary.
    #[default]
    Unset,
    /// The key is present and classified.
    Set {
        /// Classified value.
        value: T,
    },
    /// The key is present with a wrong shallow value type.
    Malformed {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The key is present more than once, so the effective safety value is
    /// ambiguous.
    Duplicate {
        /// First matching key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
}

/// Shallow `/CS` blend colour-space shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum TransparencyGroupColorSpace {
    /// Name-form colour space such as `/DeviceCMYK`.
    Name {
        /// Raw name bytes without the leading slash.
        raw_name: PdfName,
    },
    /// Array-form colour space. Resource resolution is deliberately out of
    /// scope for this phase.
    Array,
}

/// One page- or form-local `/Group` diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedTransparencyGroup {
    /// Resolved page/form object byte offset.
    pub object_byte_offset: usize,
    /// Structured diagnostic reason.
    pub reason: SkippedTransparencyGroupReason,
}

/// Structured reason a `/Group` entry was not classified as a transparency
/// group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedTransparencyGroupReason {
    /// The top-level page/form dictionary could not be inspected.
    ObjectDictionaryFailed {
        /// Delegated indirect-object dictionary failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// More than one `/Group` key was present.
    DuplicateGroup {
        /// First `/Group` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Group` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The top-level `/Group` value was not a direct dictionary or indirect
    /// reference.
    NonDictionaryGroup {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// An indirect `/Group` value looked like a reference but was malformed.
    MalformedGroupReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// An indirect `/Group` reference did not resolve.
    UnresolvedGroupReference {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number, when available.
        location: Option<ObjectLookupLocation>,
    },
    /// An indirect `/Group` resolved to a non-dictionary object.
    GroupDictionaryFailed {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Delegated dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// A direct group attributes dictionary could not be scanned.
    DirectGroupDictionaryFailed {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// `/S` was absent.
    MissingSubtype,
    /// `/S` was present but was not a name.
    MalformedSubtype {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// `/S` named a group subtype other than `/Transparency`.
    NonTransparencySubtype {
        /// Raw subtype name bytes without the leading slash.
        raw_name: PdfName,
    },
}

/// Classify the unique top-level `/Group` entry of a page/form dictionary.
///
/// Returns `Ok(None)` when `/Group` is absent.
///
/// # Errors
///
/// Returns a structured diagnostic when `/Group` is malformed, unresolved,
/// non-dictionary, or is not a `/S /Transparency` group.
pub fn classify_transparency_group_entry(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    entries: &[DictionaryEntrySpan],
) -> Result<Option<ClassifiedTransparencyGroup>, SkippedTransparencyGroup> {
    let entry = match unique_entry(input, entries, b"/Group") {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            return Err(skipped_group(
                object_byte_offset,
                SkippedTransparencyGroupReason::DuplicateGroup {
                    first_key_range,
                    duplicate_key_range,
                },
            ));
        }
    };
    let Some(entry) = entry else {
        return Ok(None);
    };

    let group_entries = match entry.value_kind {
        DictionaryValueKind::Dictionary => {
            inspect_dictionary_entries(input, entry.value_range.start)
                .map_err(|error| {
                    skipped_group(
                        object_byte_offset,
                        SkippedTransparencyGroupReason::DirectGroupDictionaryFailed { error },
                    )
                })?
                .entries
        }
        DictionaryValueKind::IndirectReferenceLike => {
            let reference =
                parse_indirect_reference(input, entry.value_range.start).map_err(|error| {
                    skipped_group(
                        object_byte_offset,
                        SkippedTransparencyGroupReason::MalformedGroupReference {
                            reference_reason: error.reason,
                        },
                    )
                })?;
            let (_, group_object_byte_offset) = resolve_reference(lookup, reference.reference)
                .map_err(|error| skipped_group(object_byte_offset, unresolved_reference(&error)))?;
            inspect_indirect_object_dictionary(input, group_object_byte_offset)
                .map_err(|error| {
                    skipped_group(
                        object_byte_offset,
                        SkippedTransparencyGroupReason::GroupDictionaryFailed {
                            reference: reference.reference,
                            object_byte_offset: group_object_byte_offset,
                            error,
                        },
                    )
                })?
                .entries
        }
        value_kind => {
            return Err(skipped_group(
                object_byte_offset,
                SkippedTransparencyGroupReason::NonDictionaryGroup { value_kind },
            ));
        }
    };

    classify_group_dictionary(input, object_byte_offset, &group_entries).map(Some)
}

fn classify_group_dictionary(
    input: &[u8],
    object_byte_offset: usize,
    entries: &[DictionaryEntrySpan],
) -> Result<ClassifiedTransparencyGroup, SkippedTransparencyGroup> {
    let subtype =
        unique_entry(input, entries, b"/S").map_err(|(first_key_range, duplicate_key_range)| {
            skipped_group(
                object_byte_offset,
                SkippedTransparencyGroupReason::DuplicateGroup {
                    first_key_range,
                    duplicate_key_range,
                },
            )
        })?;
    let Some(subtype) = subtype else {
        return Err(skipped_group(
            object_byte_offset,
            SkippedTransparencyGroupReason::MissingSubtype,
        ));
    };
    if subtype.value_kind != DictionaryValueKind::Name {
        return Err(skipped_group(
            object_byte_offset,
            SkippedTransparencyGroupReason::MalformedSubtype {
                value_kind: subtype.value_kind,
            },
        ));
    }
    let raw_name = PdfName(input[subtype.value_range.start + 1..subtype.value_range.end].to_vec());
    if raw_name.0 != b"Transparency" {
        return Err(skipped_group(
            object_byte_offset,
            SkippedTransparencyGroupReason::NonTransparencySubtype { raw_name },
        ));
    }

    let mut group = ClassifiedTransparencyGroup {
        transparency: true,
        color_space: TransparencyGroupParamClass::Unset,
        isolated: TransparencyGroupParamClass::Unset,
        knockout: TransparencyGroupParamClass::Unset,
        has_unclassified_keys: false,
    };
    let mut color_space_key_range = None;
    let mut isolated_key_range = None;
    let mut knockout_key_range = None;
    for entry in entries {
        match &input[entry.key_range.start..entry.key_range.end] {
            b"/Type" | b"/S" => {}
            b"/CS" => classify_unique_param(
                input,
                entry,
                &mut color_space_key_range,
                &mut group.color_space,
                classify_color_space,
            ),
            b"/I" => classify_unique_param(
                input,
                entry,
                &mut isolated_key_range,
                &mut group.isolated,
                classify_bool,
            ),
            b"/K" => classify_unique_param(
                input,
                entry,
                &mut knockout_key_range,
                &mut group.knockout,
                classify_bool,
            ),
            _ => group.has_unclassified_keys = true,
        }
    }
    Ok(group)
}

fn classify_unique_param<T>(
    input: &[u8],
    entry: &DictionaryEntrySpan,
    first_key_range: &mut Option<DictionaryEntryByteRange>,
    target: &mut TransparencyGroupParamClass<T>,
    classify: impl FnOnce(&[u8], &DictionaryEntrySpan) -> TransparencyGroupParamClass<T>,
) {
    if let Some(first_key_range) = *first_key_range {
        *target = TransparencyGroupParamClass::Duplicate {
            first_key_range,
            duplicate_key_range: entry.key_range,
        };
        return;
    }

    *first_key_range = Some(entry.key_range);
    *target = classify(input, entry);
}

fn classify_color_space(
    input: &[u8],
    entry: &DictionaryEntrySpan,
) -> TransparencyGroupParamClass<TransparencyGroupColorSpace> {
    let value = match entry.value_kind {
        DictionaryValueKind::Name => TransparencyGroupColorSpace::Name {
            raw_name: PdfName(input[entry.value_range.start + 1..entry.value_range.end].to_vec()),
        },
        DictionaryValueKind::Array => TransparencyGroupColorSpace::Array,
        value_kind => return TransparencyGroupParamClass::Malformed { value_kind },
    };
    TransparencyGroupParamClass::Set { value }
}

fn classify_bool(input: &[u8], entry: &DictionaryEntrySpan) -> TransparencyGroupParamClass<bool> {
    if entry.value_kind != DictionaryValueKind::Boolean {
        return TransparencyGroupParamClass::Malformed {
            value_kind: entry.value_kind,
        };
    }
    TransparencyGroupParamClass::Set {
        value: &input[entry.value_range.start..entry.value_range.end] == b"true",
    }
}

#[must_use]
pub const fn skipped_group(
    object_byte_offset: usize,
    reason: SkippedTransparencyGroupReason,
) -> SkippedTransparencyGroup {
    SkippedTransparencyGroup {
        object_byte_offset,
        reason,
    }
}

const fn unresolved_reference(error: &ResolveReferenceError) -> SkippedTransparencyGroupReason {
    match error {
        ResolveReferenceError::Unresolved {
            reference,
            location,
        } => SkippedTransparencyGroupReason::UnresolvedGroupReference {
            reference: *reference,
            location: Some(*location),
        },
        ResolveReferenceError::GenerationMismatch { reference, .. } => {
            SkippedTransparencyGroupReason::UnresolvedGroupReference {
                reference: *reference,
                location: None,
            }
        }
    }
}
