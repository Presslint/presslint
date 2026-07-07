//! Shallow classification of one `/Resources /ExtGState` entry.
//!
//! This module records only Phase-1 safety parameters from an extended graphics
//! state dictionary. It deliberately does not simulate graphics-state defaults:
//! ISO 32000-1 says an absent `op` takes `OP`'s value when `OP` is set, but this
//! read model preserves absence as [`ExtGStateParamClass::Unset`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::page_resource_inheritance::{ResolveReferenceError, ResourceContext, resolve_reference};
use crate::{
    DictionaryEntryByteRange, DictionaryEntryInspectionError, DictionaryEntrySpan,
    DictionaryValueKind, IndirectObjectDictionaryInspectionError, IndirectRef,
    IndirectReferenceInspectionRejection, ObjectLookup, ObjectLookupLocation, PdfName,
    SkippedPageXObjectResourceReason, inspect_dictionary_entries,
    inspect_indirect_object_dictionary, parse_indirect_reference,
};

/// One classified `/Resources /ExtGState` resource entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedExtGStateResource {
    /// Resource name (without the leading slash) selected by the `gs` operator.
    pub name: PdfName,
    /// Strokes overprint flag (`OP`) as written, without applying defaults.
    pub op_stroking: ExtGStateParamClass<bool>,
    /// Non-stroking overprint flag (`op`) as written, without inheriting `OP`.
    pub op_nonstroking: ExtGStateParamClass<bool>,
    /// Overprint mode (`OPM`), classified as 0, 1, or another numeric token.
    pub overprint_mode: ExtGStateParamClass<ExtGStateOverprintMode>,
    /// Stroking alpha constant (`CA`); exact numeric 1.0 is opaque.
    pub stroking_alpha: ExtGStateParamClass<ExtGStateAlpha>,
    /// Non-stroking alpha constant (`ca`); exact numeric 1.0 is opaque.
    pub nonstroking_alpha: ExtGStateParamClass<ExtGStateAlpha>,
    /// Blend mode (`BM`) shape and raw name when it is a name.
    pub blend_mode: ExtGStateParamClass<ExtGStateBlendMode>,
    /// Soft mask (`SMask`) presence classification.
    pub soft_mask: ExtGStateParamClass<ExtGStateSoftMask>,
    /// True when the dictionary contains keys outside the Phase-1 safety set
    /// plus `/Type`.
    pub has_unclassified_keys: bool,
}

impl ClassifiedExtGStateResource {
    /// True when `OP` or `op` is explicitly `true`, or when `OPM` is set to any
    /// value. This deliberately does not apply the PDF `op`-defaults-to-`OP`
    /// rule; callers only use values written in this resource.
    #[must_use]
    pub const fn is_overprint_active(&self) -> bool {
        param_bool_true(&self.op_stroking)
            || param_bool_true(&self.op_nonstroking)
            || matches!(self.overprint_mode, ExtGStateParamClass::Set { .. })
    }

    /// True when alpha, blend mode, or soft mask parameters activate
    /// non-default transparency behaviour.
    #[must_use]
    pub const fn is_transparency_active(&self) -> bool {
        param_alpha_non_opaque(&self.stroking_alpha)
            || param_alpha_non_opaque(&self.nonstroking_alpha)
            || param_blend_mode_non_normal(&self.blend_mode)
            || param_soft_mask_present(&self.soft_mask)
    }

    /// True when any Phase-1 safety parameter is present but malformed or
    /// classified into an unknown/unsupported safety value.
    #[must_use]
    pub const fn has_unresolved_or_unclassified_safety_param(&self) -> bool {
        param_malformed(&self.op_stroking)
            || param_malformed(&self.op_nonstroking)
            || param_opm_unknown(&self.overprint_mode)
            || param_malformed(&self.stroking_alpha)
            || param_malformed(&self.nonstroking_alpha)
            || param_blend_mode_unclassified(&self.blend_mode)
            || param_malformed(&self.soft_mask)
    }
}

/// Per-parameter classification. `Unset` means the key is absent; no PDF
/// default or inherited graphics-state value has been invented.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "class", rename_all = "snake_case")]
pub enum ExtGStateParamClass<T> {
    /// The parameter key is absent from this dictionary.
    #[default]
    Unset,
    /// The parameter key is present and classified.
    Set {
        /// Classified parameter value.
        value: T,
    },
    /// The parameter key is present with a wrong shallow value type.
    Malformed {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// Overprint mode (`OPM`) classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ExtGStateOverprintMode {
    /// Numeric `0`.
    Zero,
    /// Numeric `1`.
    One,
    /// Any other numeric token, retained as source bytes.
    Other {
        /// Raw numeric token bytes.
        raw: Vec<u8>,
    },
}

/// Alpha constant classification. Exact numeric 1.0 is opaque; every other
/// number is non-opaque. No tolerance is applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "alpha", rename_all = "snake_case")]
pub enum ExtGStateAlpha {
    /// Numeric value exactly equal to 1.0.
    Opaque,
    /// Any numeric value other than 1.0, retained as source bytes.
    NonOpaque {
        /// Raw numeric token bytes.
        raw: Vec<u8>,
    },
}

/// Blend mode (`BM`) shape classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "blend_mode", rename_all = "snake_case")]
pub enum ExtGStateBlendMode {
    /// `/Normal` blend mode, or `/Compatible` which PDF renderers treat as
    /// equivalent to `/Normal`.
    Normal {
        /// Raw name bytes without the leading slash.
        raw_name: PdfName,
    },
    /// A non-`/Normal` blend mode name.
    NonNormal {
        /// Raw name bytes without the leading slash.
        raw_name: PdfName,
    },
    /// An array-form blend mode list.
    Array,
    /// Any other shallow value shape.
    Other {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// Soft mask (`SMask`) presence classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtGStateSoftMask {
    /// The value is the name `/None`.
    None,
    /// Any other value shape is a present soft mask for this Phase-1 report.
    Present,
}

const fn param_bool_true(param: &ExtGStateParamClass<bool>) -> bool {
    matches!(param, ExtGStateParamClass::Set { value: true })
}

const fn param_malformed<T>(param: &ExtGStateParamClass<T>) -> bool {
    matches!(param, ExtGStateParamClass::Malformed { .. })
}

const fn param_opm_unknown(param: &ExtGStateParamClass<ExtGStateOverprintMode>) -> bool {
    matches!(
        param,
        ExtGStateParamClass::Malformed { .. }
            | ExtGStateParamClass::Set {
                value: ExtGStateOverprintMode::Other { .. },
            }
    )
}

const fn param_alpha_non_opaque(param: &ExtGStateParamClass<ExtGStateAlpha>) -> bool {
    matches!(
        param,
        ExtGStateParamClass::Set {
            value: ExtGStateAlpha::NonOpaque { .. },
        }
    )
}

const fn param_blend_mode_non_normal(param: &ExtGStateParamClass<ExtGStateBlendMode>) -> bool {
    matches!(
        param,
        ExtGStateParamClass::Set {
            value: ExtGStateBlendMode::NonNormal { .. },
        }
    )
}

const fn param_blend_mode_unclassified(param: &ExtGStateParamClass<ExtGStateBlendMode>) -> bool {
    matches!(
        param,
        ExtGStateParamClass::Malformed { .. }
            | ExtGStateParamClass::Set {
                value: ExtGStateBlendMode::Array | ExtGStateBlendMode::Other { .. },
            }
    )
}

const fn param_soft_mask_present(param: &ExtGStateParamClass<ExtGStateSoftMask>) -> bool {
    matches!(
        param,
        ExtGStateParamClass::Set {
            value: ExtGStateSoftMask::Present,
        }
    )
}

/// One page- or form-local `/ExtGState` resource diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedExtGStateResource {
    /// Resolved page/form object byte offset.
    pub object_byte_offset: usize,
    /// Resource name when the diagnostic concerns one `/ExtGState` entry.
    pub resource_name: Option<PdfName>,
    /// Structured skip reason.
    pub reason: SkippedExtGStateResourceReason,
}

/// Structured reason an `/ExtGState` resource was not classified.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedExtGStateResourceReason {
    /// A `/Resources` inheritance-level diagnostic (delegated vocabulary).
    Resources {
        /// Delegated `/Resources` resolution/inheritance failure.
        resources_reason: crate::SkippedPageXObjectResourceReason,
    },
    /// No effective `/Resources` dictionary was available.
    MissingExtGStateResources,
    /// No `/ExtGState` sub-dictionary was present in the effective resources.
    MissingExtGState,
    /// An effective `/ExtGState` key occurred more than once.
    DuplicateExtGState {
        /// First `/ExtGState` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/ExtGState` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The effective `/ExtGState` value was not a direct dictionary.
    NonDictionaryExtGState {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The direct `/ExtGState` dictionary could not be scanned.
    ExtGStateDictionaryFailed {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
    /// A direct `/ExtGState` dictionary repeated the same resource name.
    DuplicateExtGStateName {
        /// First matching resource-name key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching resource-name key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// One `/ExtGState` entry was not a direct dictionary or indirect reference.
    NonDictionaryEntry {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// An indirect `/ExtGState` entry was shaped like a reference but malformed.
    MalformedResourceReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// An indirect `/ExtGState` entry reference did not resolve.
    UnresolvedResourceReference {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number, when available.
        location: Option<ObjectLookupLocation>,
    },
    /// An indirect `/ExtGState` entry resolved to a non-dictionary object.
    ResourceDictionaryFailed {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
}

/// Classify one `/ExtGState` dictionary entry.
///
/// # Errors
///
/// Returns a structured skip reason when the entry is not a dictionary, an
/// indirect entry is malformed or unresolved, or a dictionary cannot be scanned.
pub fn classify_extgstate_entry(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    name: &PdfName,
    entry: DictionaryEntrySpan,
) -> Result<ClassifiedExtGStateResource, SkippedExtGStateResourceReason> {
    match entry.value_kind {
        DictionaryValueKind::Dictionary => {
            let entries =
                inspect_dictionary_entries(input, entry.value_range.start).map_err(|error| {
                    SkippedExtGStateResourceReason::ExtGStateDictionaryFailed { error }
                })?;
            Ok(classify_dictionary(input, name.clone(), &entries.entries))
        }
        DictionaryValueKind::IndirectReferenceLike => {
            let reference =
                parse_indirect_reference(input, entry.value_range.start).map_err(|error| {
                    SkippedExtGStateResourceReason::MalformedResourceReference {
                        reference_reason: error.reason,
                    }
                })?;
            let (_, object_byte_offset) = resolve_reference(lookup, reference.reference)
                .map_err(|error| unresolved_reference(&error))?;
            let dictionary = inspect_indirect_object_dictionary(input, object_byte_offset)
                .map_err(
                    |error| SkippedExtGStateResourceReason::ResourceDictionaryFailed {
                        reference: reference.reference,
                        object_byte_offset,
                        error,
                    },
                )?;
            Ok(classify_dictionary(
                input,
                name.clone(),
                &dictionary.entries,
            ))
        }
        value_kind => Err(SkippedExtGStateResourceReason::NonDictionaryEntry { value_kind }),
    }
}

/// Classify all entries in a direct `/ExtGState` sub-dictionary.
pub fn classify_extgstate_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    entries: Vec<DictionaryEntrySpan>,
    skipped: &mut Vec<SkippedExtGStateResource>,
) -> Vec<ClassifiedExtGStateResource> {
    let mut classified = Vec::new();
    let mut seen_names = BTreeMap::new();
    for entry in entries {
        let name = PdfName(input[entry.key_range.start + 1..entry.key_range.end].to_vec());
        if let Some(first_key_range) = seen_names.get(&name) {
            skipped.push(skipped_entry(
                object_byte_offset,
                Some(name),
                SkippedExtGStateResourceReason::DuplicateExtGStateName {
                    first_key_range: *first_key_range,
                    duplicate_key_range: entry.key_range,
                },
            ));
            continue;
        }
        seen_names.insert(name.clone(), entry.key_range);
        match classify_extgstate_entry(input, lookup, &name, entry) {
            Ok(resource) => classified.push(resource),
            Err(reason) => skipped.push(skipped_entry(object_byte_offset, Some(name), reason)),
        }
    }
    classified
}

pub struct EffectiveExtGStateResources {
    pub extgstates: Vec<ClassifiedExtGStateResource>,
    pub skipped: Vec<SkippedExtGStateResource>,
}

pub fn inspect_effective_extgstate_resource_entries(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
    context: &ResourceContext,
) -> EffectiveExtGStateResources {
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
            SkippedExtGStateResourceReason::MissingExtGStateResources,
        ));
        return effective_resources(Vec::new(), skipped);
    };

    let Some(gs_entry) = (match crate::page_resource_inheritance::unique_entry(
        input,
        &resources.entries,
        b"/ExtGState",
    ) {
        Ok(entry) => entry,
        Err((first_key_range, duplicate_key_range)) => {
            skipped.push(skipped_entry(
                object_byte_offset,
                None,
                SkippedExtGStateResourceReason::DuplicateExtGState {
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
            SkippedExtGStateResourceReason::MissingExtGState,
        ));
        return effective_resources(Vec::new(), skipped);
    };

    if gs_entry.value_kind != DictionaryValueKind::Dictionary {
        skipped.push(skipped_entry(
            object_byte_offset,
            None,
            SkippedExtGStateResourceReason::NonDictionaryExtGState {
                value_kind: gs_entry.value_kind,
            },
        ));
        return effective_resources(Vec::new(), skipped);
    }

    let entries = match inspect_dictionary_entries(input, gs_entry.value_range.start) {
        Ok(entries) => entries,
        Err(error) => {
            skipped.push(skipped_entry(
                object_byte_offset,
                None,
                SkippedExtGStateResourceReason::ExtGStateDictionaryFailed { error },
            ));
            return effective_resources(Vec::new(), skipped);
        }
    };

    let extgstates = classify_extgstate_entries(
        input,
        lookup,
        object_byte_offset,
        entries.entries,
        &mut skipped,
    );
    effective_resources(extgstates, skipped)
}

fn effective_resources(
    mut extgstates: Vec<ClassifiedExtGStateResource>,
    skipped: Vec<SkippedExtGStateResource>,
) -> EffectiveExtGStateResources {
    extgstates.sort_by(|left, right| left.name.cmp(&right.name));
    EffectiveExtGStateResources {
        extgstates,
        skipped,
    }
}

#[must_use]
pub const fn skipped_entry(
    object_byte_offset: usize,
    resource_name: Option<PdfName>,
    reason: SkippedExtGStateResourceReason,
) -> SkippedExtGStateResource {
    SkippedExtGStateResource {
        object_byte_offset,
        resource_name,
        reason,
    }
}

const fn resources_skip(
    reason: SkippedPageXObjectResourceReason,
) -> SkippedExtGStateResourceReason {
    SkippedExtGStateResourceReason::Resources {
        resources_reason: reason,
    }
}

fn classify_dictionary(
    input: &[u8],
    name: PdfName,
    entries: &[DictionaryEntrySpan],
) -> ClassifiedExtGStateResource {
    let mut resource = ClassifiedExtGStateResource {
        name,
        op_stroking: ExtGStateParamClass::Unset,
        op_nonstroking: ExtGStateParamClass::Unset,
        overprint_mode: ExtGStateParamClass::Unset,
        stroking_alpha: ExtGStateParamClass::Unset,
        nonstroking_alpha: ExtGStateParamClass::Unset,
        blend_mode: ExtGStateParamClass::Unset,
        soft_mask: ExtGStateParamClass::Unset,
        has_unclassified_keys: false,
    };

    for entry in entries {
        match &input[entry.key_range.start..entry.key_range.end] {
            b"/Type" => {}
            b"/OP" => resource.op_stroking = classify_bool(input, entry),
            b"/op" => resource.op_nonstroking = classify_bool(input, entry),
            b"/OPM" => resource.overprint_mode = classify_opm(input, entry),
            b"/CA" => resource.stroking_alpha = classify_alpha(input, entry),
            b"/ca" => resource.nonstroking_alpha = classify_alpha(input, entry),
            b"/BM" => resource.blend_mode = classify_blend_mode(input, entry),
            b"/SMask" => resource.soft_mask = classify_soft_mask(input, entry),
            _ => resource.has_unclassified_keys = true,
        }
    }
    resource
}

fn classify_bool(input: &[u8], entry: &DictionaryEntrySpan) -> ExtGStateParamClass<bool> {
    if entry.value_kind != DictionaryValueKind::Boolean {
        return ExtGStateParamClass::Malformed {
            value_kind: entry.value_kind,
        };
    }
    ExtGStateParamClass::Set {
        value: &input[entry.value_range.start..entry.value_range.end] == b"true",
    }
}

fn classify_opm(
    input: &[u8],
    entry: &DictionaryEntrySpan,
) -> ExtGStateParamClass<ExtGStateOverprintMode> {
    if entry.value_kind != DictionaryValueKind::NumberLike {
        return ExtGStateParamClass::Malformed {
            value_kind: entry.value_kind,
        };
    }
    let raw = &input[entry.value_range.start..entry.value_range.end];
    let value = match raw {
        b"0" => ExtGStateOverprintMode::Zero,
        b"1" => ExtGStateOverprintMode::One,
        _ => ExtGStateOverprintMode::Other { raw: raw.to_vec() },
    };
    ExtGStateParamClass::Set { value }
}

fn classify_alpha(
    input: &[u8],
    entry: &DictionaryEntrySpan,
) -> ExtGStateParamClass<ExtGStateAlpha> {
    if entry.value_kind != DictionaryValueKind::NumberLike {
        return ExtGStateParamClass::Malformed {
            value_kind: entry.value_kind,
        };
    }
    let raw = &input[entry.value_range.start..entry.value_range.end];
    let value = if is_exact_decimal_one(raw) {
        ExtGStateAlpha::Opaque
    } else {
        ExtGStateAlpha::NonOpaque { raw: raw.to_vec() }
    };
    ExtGStateParamClass::Set { value }
}

fn classify_blend_mode(
    input: &[u8],
    entry: &DictionaryEntrySpan,
) -> ExtGStateParamClass<ExtGStateBlendMode> {
    let value = match entry.value_kind {
        DictionaryValueKind::Name => {
            let raw_name =
                PdfName(input[entry.value_range.start + 1..entry.value_range.end].to_vec());
            if raw_name.0 == b"Normal" || raw_name.0 == b"Compatible" {
                ExtGStateBlendMode::Normal { raw_name }
            } else {
                ExtGStateBlendMode::NonNormal { raw_name }
            }
        }
        DictionaryValueKind::Array => ExtGStateBlendMode::Array,
        value_kind => ExtGStateBlendMode::Other { value_kind },
    };
    ExtGStateParamClass::Set { value }
}

fn classify_soft_mask(
    input: &[u8],
    entry: &DictionaryEntrySpan,
) -> ExtGStateParamClass<ExtGStateSoftMask> {
    let value = if entry.value_kind == DictionaryValueKind::Name
        && &input[entry.value_range.start..entry.value_range.end] == b"/None"
    {
        ExtGStateSoftMask::None
    } else {
        ExtGStateSoftMask::Present
    };
    ExtGStateParamClass::Set { value }
}

fn is_exact_decimal_one(raw: &[u8]) -> bool {
    let raw = raw.strip_prefix(b"+").unwrap_or(raw);
    let Some(dot) = raw.iter().position(|byte| *byte == b'.') else {
        return decimal_integer_is_one(raw);
    };
    let integer = &raw[..dot];
    let fraction = &raw[dot + 1..];
    decimal_integer_is_one(integer)
        && !fraction.is_empty()
        && fraction.iter().all(|byte| *byte == b'0')
}

fn decimal_integer_is_one(bytes: &[u8]) -> bool {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let without_leading_zeroes = bytes
        .iter()
        .position(|byte| *byte != b'0')
        .map_or(&b""[..], |index| &bytes[index..]);
    without_leading_zeroes == b"1"
}

const fn unresolved_reference(error: &ResolveReferenceError) -> SkippedExtGStateResourceReason {
    match error {
        ResolveReferenceError::Unresolved {
            reference,
            location,
        } => SkippedExtGStateResourceReason::UnresolvedResourceReference {
            reference: *reference,
            location: Some(*location),
        },
        ResolveReferenceError::GenerationMismatch { reference, .. } => {
            SkippedExtGStateResourceReason::UnresolvedResourceReference {
                reference: *reference,
                location: None,
            }
        }
    }
}
