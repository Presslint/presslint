//! Shallow classification of one `/Resources /ExtGState` entry.
//!
//! This module records only Phase-1 safety parameters from an extended graphics
//! state dictionary. It deliberately does not simulate graphics-state defaults:
//! ISO 32000-1 says an absent `op` takes `OP`'s value when `OP` is set, but this
//! read model preserves absence as [`ExtGStateParamClass::Unset`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::font_classify::classify_font_dictionary_facts;
use crate::page_resource_inheritance::{ResolveReferenceError, ResourceContext, resolve_reference};
use crate::source_utils::{
    decode_pdf_name, is_pdf_delimiter, is_pdf_whitespace, parse_u64_decimal, skip_scalar_token,
    skip_whitespace_and_comments,
};
use crate::{
    DictionaryEntryByteRange, DictionaryEntryInspectionError, DictionaryEntrySpan,
    DictionaryValueKind, FontDictionaryTypeFact, FontSubtypeClass,
    IndirectObjectDictionaryInspectionError, IndirectObjectDictionaryInspectionRejection,
    IndirectRef, IndirectReferenceInspectionRejection, ObjectLookup, ObjectLookupLocation, PdfName,
    SkippedPageXObjectResourceReason, inspect_dictionary_entries,
    inspect_indirect_object_dictionary, inspect_indirect_object_header, parse_indirect_reference,
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
    /// plus `/Type`. A semantic `/Font` key still counts as unclassified even
    /// though [`ClassifiedExtGStateResource::font_effect`] records its exact
    /// classification. Existing safety findings still treat this aggregate
    /// flag fail-closed; font consumers may use a classified present effect,
    /// but [`ExtGStateFontEffect::Unset`] proves absence only when this flag is
    /// false.
    pub has_unclassified_keys: bool,
    /// Exact classified `/Font` effect (ISO 32000-1 Table 58), recorded
    /// without changing any of the seven safety parameter classifications.
    #[serde(default)]
    pub font_effect: ExtGStateFontEffect,
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

    /// True when any Phase-1 safety parameter is present but malformed,
    /// repeated under a semantic PDF-name spelling, or classified into an
    /// unknown/unsupported safety value.
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
    /// The parameter key is present with a wrong shallow value type, or the
    /// same semantic key occurs more than once. Duplicate safety keys never
    /// first- or last-win; they reuse this existing fail-closed vocabulary.
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

/// Exact classified `/Font` effect of one `ExtGState` dictionary.
///
/// ISO 32000-1 Table 58 defines the value as a two-element array `[font
/// size]` where `font` shall be an indirect reference to a font dictionary
/// and `size` a number in text space units — the direct-reference equivalent
/// of the `Tf` operator's name-plus-size operands.
///
/// [`ExtGStateFontEffect::StructurallyValid`] proves structural selection
/// eligibility only: the target is an inspectable dictionary with `/Type`
/// exactly `/Font` and a legal direct current-font subtype. It makes no
/// writer-safety or admissibility claim; Type3 in particular is a legal
/// current font but is not writer-safe. Every other variant is an explicit
/// fail-closed fact and never collapses into absence or a fabricated binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "effect", rename_all = "snake_case")]
pub enum ExtGStateFontEffect {
    /// No well-formed semantic `/Font` key is present in this dictionary. While
    /// [`ClassifiedExtGStateResource::has_unclassified_keys`] is true this is
    /// not proof of absence: an unrelated or malformed key remains outside
    /// this classifier's vocabulary.
    #[default]
    Unset,
    /// The semantic `/Font` key occurred more than once. No first- or last-wins
    /// recovery is applied.
    DuplicateKey {
        /// First `/Font` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/Font` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The `/Font` value was not an array.
    NonArrayValue {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The first array element was absent or not a well-formed `N G R`
    /// indirect reference. Direct dictionaries and names never become valid
    /// font references (Table 58 requires an indirect reference).
    MalformedFontReference {
        /// Underlying indirect-reference rejection reason.
        reference_reason: IndirectReferenceInspectionRejection,
    },
    /// The second array element was absent (empty range) or not a parseable
    /// number.
    MalformedSize {
        /// Byte range of the offending size span; empty when the element is
        /// absent.
        size_range: DictionaryEntryByteRange,
    },
    /// The size parsed as a number but is not finite (for example numeric
    /// overflow).
    NonFiniteSize {
        /// Byte range of the non-finite size token.
        size_range: DictionaryEntryByteRange,
    },
    /// The size is an indirect numeric object, which the bounded current
    /// scalar path does not inspect. This is an uninspected fact, not a
    /// malformed one. Once a complete indirect reference is recognized, this
    /// outcome takes precedence over any trailing array content because the
    /// referenced value itself is not inspected.
    IndirectSizeUnsupported {
        /// The uninspected indirect size reference.
        reference: IndirectRef,
    },
    /// The array carried content after the two expected elements.
    ExtraArrayElements {
        /// Byte range of the unexpected trailing content up to the array
        /// close.
        trailing_range: DictionaryEntryByteRange,
    },
    /// The font reference did not resolve to a usable in-use object
    /// (undefined, free, or generation-mismatched).
    UnresolvedTarget {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Locate-only result for the requested object number, when
        /// available. `None` marks a generation mismatch: against an in-use
        /// entry, or a nonzero-generation reference to a compressed object
        /// whose generation is implicitly zero (ISO 32000-1 §7.5.8.3).
        location: Option<ObjectLookupLocation>,
    },
    /// The generation-zero font reference resolved to an xref-stream
    /// compressed object. That is a valid possible PDF object, but the
    /// offset-based resource path cannot inspect it standalone, so its
    /// structural location is retained uninspected — neither valid nor
    /// malformed, and never with a fabricated byte offset.
    CompressedTargetUninspected {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Structural object-stream/member location from the xref lookup.
        location: ObjectLookupLocation,
    },
    /// The resolved target could not be inspected as a dictionary.
    TargetDictionaryFailed {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Delegated object-dictionary inspection failure.
        error: IndirectObjectDictionaryInspectionError,
    },
    /// The indirect object header at the resolved byte offset identifies a
    /// different object than the requested reference, so the xref binding
    /// resolves to no usable font. Header identity is checked before any body
    /// inspection, so the mismatched body is never classified.
    TargetHeaderMismatch {
        /// Requested indirect reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Reference parsed from the object header at that offset.
        header_reference: IndirectRef,
    },
    /// The target dictionary was reached and inspected but its `/Type` and
    /// `/Subtype` facts do not prove a legal direct current font. CID
    /// descendant subtypes stay their distinct facts and are never folded
    /// into Type0; missing/duplicate/non-name facts stay exact.
    InadmissibleTarget {
        /// Resolved indirect reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Exact parsed size as `f64::to_bits()`.
        size_bits: u64,
        /// Exact `/Type` dictionary fact (shared font-resource taxonomy).
        dictionary_type: FontDictionaryTypeFact,
        /// Exact `/Subtype` classification (shared font-resource taxonomy).
        subtype: FontSubtypeClass,
    },
    /// Structurally valid font selection: an inspectable `/Type /Font`
    /// dictionary with a legal direct current-font subtype
    /// (Type1/MMType1/TrueType/Type0/Type3) and a finite size. This is not a
    /// writer-safety claim.
    StructurallyValid {
        /// Resolved indirect reference.
        reference: IndirectRef,
        /// Resolved in-use object byte offset.
        object_byte_offset: usize,
        /// Exact parsed size as `f64::to_bits()`; sign of zero, negative
        /// values, and fractions are preserved bit-exactly.
        size_bits: u64,
        /// Exact `/Type` dictionary fact (always the `/Font` fact here).
        dictionary_type: FontDictionaryTypeFact,
        /// Exact `/Subtype` classification (one of the five legal direct
        /// subtypes here).
        subtype: FontSubtypeClass,
    },
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
            Ok(classify_dictionary(
                input,
                lookup,
                name.clone(),
                &entries.entries,
            ))
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
                lookup,
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
    lookup: ObjectLookup<'_>,
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
        font_effect: ExtGStateFontEffect::Unset,
    };

    let mut font_entry: Option<DictionaryEntrySpan> = None;
    for entry in entries {
        let raw_key = &input[entry.key_range.start + 1..entry.key_range.end];
        let Some(key) = decode_pdf_name(raw_key) else {
            resource.has_unclassified_keys = true;
            continue;
        };
        match key.as_ref() {
            b"Type" => {}
            b"Font" => {
                // `/Font` stays an unclassified key in this slice: the
                // aggregate flag is what existing findings fail closed on.
                resource.has_unclassified_keys = true;
                match font_entry {
                    None => font_entry = Some(*entry),
                    Some(first) => {
                        if !matches!(
                            resource.font_effect,
                            ExtGStateFontEffect::DuplicateKey { .. }
                        ) {
                            resource.font_effect = ExtGStateFontEffect::DuplicateKey {
                                first_key_range: first.key_range,
                                duplicate_key_range: entry.key_range,
                            };
                        }
                    }
                }
            }
            b"OP" => {
                set_unique_safety_param(&mut resource.op_stroking, input, entry, classify_bool);
            }
            b"op" => {
                set_unique_safety_param(&mut resource.op_nonstroking, input, entry, classify_bool);
            }
            b"OPM" => {
                set_unique_safety_param(&mut resource.overprint_mode, input, entry, classify_opm);
            }
            b"CA" => {
                set_unique_safety_param(&mut resource.stroking_alpha, input, entry, classify_alpha);
            }
            b"ca" => set_unique_safety_param(
                &mut resource.nonstroking_alpha,
                input,
                entry,
                classify_alpha,
            ),
            b"BM" => {
                set_unique_safety_param(
                    &mut resource.blend_mode,
                    input,
                    entry,
                    classify_blend_mode,
                );
            }
            b"SMask" => {
                set_unique_safety_param(&mut resource.soft_mask, input, entry, classify_soft_mask);
            }
            _ => resource.has_unclassified_keys = true,
        }
    }
    if !matches!(
        resource.font_effect,
        ExtGStateFontEffect::DuplicateKey { .. }
    ) && let Some(entry) = font_entry
    {
        resource.font_effect = classify_font_effect(input, lookup, entry);
    }
    resource
}

/// Classify one semantically unique safety parameter. A second decoded-equal
/// key replaces neither value: it becomes the existing fail-closed malformed
/// fact consumed by `has_unresolved_or_unclassified_safety_param`.
fn set_unique_safety_param<T>(
    target: &mut ExtGStateParamClass<T>,
    input: &[u8],
    entry: &DictionaryEntrySpan,
    classify: fn(&[u8], &DictionaryEntrySpan) -> ExtGStateParamClass<T>,
) {
    if matches!(target, ExtGStateParamClass::Unset) {
        *target = classify(input, entry);
    } else {
        *target = ExtGStateParamClass::Malformed {
            value_kind: entry.value_kind,
        };
    }
}

/// Classify one unique `/Font` entry value as its Table-58 effect.
fn classify_font_effect(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    entry: DictionaryEntrySpan,
) -> ExtGStateFontEffect {
    if entry.value_kind != DictionaryValueKind::Array {
        return ExtGStateFontEffect::NonArrayValue {
            value_kind: entry.value_kind,
        };
    }
    // The dictionary scanner already bounded this balanced array, so the
    // closing `]` sits at `value_range.end - 1`.
    let body_end = entry.value_range.end - 1;
    let reference = match parse_font_effect_reference(input, entry.value_range.start + 1, body_end)
    {
        Ok(reference) => reference,
        Err(reference_reason) => {
            return ExtGStateFontEffect::MalformedFontReference { reference_reason };
        }
    };

    let size_bits = match classify_font_size(input, reference.after_keyword_offset, body_end) {
        Ok(size_bits) => size_bits,
        Err(effect) => return effect,
    };
    classify_font_target(input, lookup, reference.reference, size_bits)
}

const FONT_EFFECT_REFERENCE_SCAN_LIMIT: usize = 128;

/// Minimal comment-aware `N G R` result for the two `/Font` array operands.
struct FontEffectReference {
    reference: IndirectRef,
    after_keyword_offset: usize,
}

/// Parse the `/Font` effect's direct font reference or indirect size.
///
/// PDF comments are token separators equivalent to whitespace, including
/// between `N`, `G`, and `R`. This local parser intentionally leaves the
/// shared whitespace-only indirect-reference parser unchanged. Its scan is
/// capped to the same 128-byte leading window, it allocates nothing, retains
/// the shared object/generation range limits, and validates the `R` keyword
/// boundary against the containing array.
fn parse_font_effect_reference(
    input: &[u8],
    byte_offset: usize,
    array_body_end: usize,
) -> Result<FontEffectReference, IndirectReferenceInspectionRejection> {
    if byte_offset >= input.len() {
        return Err(IndirectReferenceInspectionRejection::OffsetOutOfBounds);
    }
    let scan_end = byte_offset
        .saturating_add(FONT_EFFECT_REFERENCE_SCAN_LIMIT)
        .min(array_body_end)
        .min(input.len());
    let reference_start = skip_whitespace_and_comments(input, byte_offset, scan_end);
    let object_end = scan_decimal_digits(input, reference_start, scan_end);
    if object_end == reference_start {
        return Err(IndirectReferenceInspectionRejection::MalformedReference);
    }

    let generation_start = skip_whitespace_and_comments(input, object_end, scan_end);
    if generation_start == object_end {
        return Err(IndirectReferenceInspectionRejection::MalformedReference);
    }
    let generation_end = scan_decimal_digits(input, generation_start, scan_end);
    if generation_end == generation_start {
        return Err(IndirectReferenceInspectionRejection::MalformedReference);
    }

    let keyword_offset = skip_whitespace_and_comments(input, generation_end, scan_end);
    if keyword_offset == generation_end || input.get(keyword_offset) != Some(&b'R') {
        return Err(IndirectReferenceInspectionRejection::MalformedReference);
    }
    let after_keyword_offset = keyword_offset + 1;
    if after_keyword_offset < array_body_end
        && input
            .get(after_keyword_offset)
            .is_some_and(|byte| !is_pdf_whitespace(*byte) && !is_pdf_delimiter(*byte))
    {
        return Err(IndirectReferenceInspectionRejection::MalformedReference);
    }

    let object_number = parse_u64_decimal(&input[reference_start..object_end])
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(IndirectReferenceInspectionRejection::ObjectNumberOutOfRange)?;
    let generation = parse_u64_decimal(&input[generation_start..generation_end])
        .and_then(|value| u16::try_from(value).ok())
        .ok_or(IndirectReferenceInspectionRejection::GenerationOutOfRange)?;

    Ok(FontEffectReference {
        reference: IndirectRef {
            object_number,
            generation,
        },
        after_keyword_offset,
    })
}

fn scan_decimal_digits(input: &[u8], mut cursor: usize, limit: usize) -> usize {
    while cursor < limit && input[cursor].is_ascii_digit() {
        cursor += 1;
    }
    cursor
}

/// Parse the second Table-58 array element as an exact finite size.
///
/// The token must satisfy the PDF number grammar (ISO 32000-1 §7.3.3) before
/// it reaches Rust's `f64` parser: PDF has no exponential notation, so a
/// lexeme such as `1e2` is malformed, never a value. The grammar gate plus
/// lexeme-to-`f64` conversion mirror the paint `Tf` operand path (tokenizer
/// number classification, then `str::parse::<f64>`), so identical source
/// lexemes always yield identical bits.
fn classify_font_size(
    input: &[u8],
    after_reference: usize,
    body_end: usize,
) -> Result<u64, ExtGStateFontEffect> {
    let cursor = skip_whitespace_and_comments(input, after_reference, body_end);
    if cursor >= body_end {
        return Err(ExtGStateFontEffect::MalformedSize {
            size_range: DictionaryEntryByteRange {
                start: cursor,
                end: cursor,
            },
        });
    }
    if let Ok(size_reference) = parse_font_effect_reference(input, cursor, body_end) {
        return Err(ExtGStateFontEffect::IndirectSizeUnsupported {
            reference: size_reference.reference,
        });
    }
    let token_end = skip_scalar_token(input, cursor, body_end);
    let size_range = DictionaryEntryByteRange {
        start: cursor,
        end: token_end,
    };
    let token = &input[cursor..token_end];
    if !is_pdf_number_lexeme(token) {
        return Err(ExtGStateFontEffect::MalformedSize { size_range });
    }
    let Some(size) = core::str::from_utf8(token)
        .ok()
        .and_then(|text| text.parse::<f64>().ok())
    else {
        return Err(ExtGStateFontEffect::MalformedSize { size_range });
    };
    if !size.is_finite() {
        return Err(ExtGStateFontEffect::NonFiniteSize { size_range });
    }
    let trailing = skip_whitespace_and_comments(input, token_end, body_end);
    if trailing < body_end {
        return Err(ExtGStateFontEffect::ExtraArrayElements {
            trailing_range: DictionaryEntryByteRange {
                start: trailing,
                end: body_end,
            },
        });
    }
    Ok(size.to_bits())
}

/// True when the token satisfies the PDF number grammar (ISO 32000-1 §7.3.3):
/// an optional leading sign, then decimal digits with at most one period and
/// at least one digit. Exponential notation is not PDF number syntax.
fn is_pdf_number_lexeme(token: &[u8]) -> bool {
    let digits = match token.first() {
        Some(b'+' | b'-') => &token[1..],
        _ => token,
    };
    let mut saw_digit = false;
    let mut saw_period = false;
    for byte in digits {
        match byte {
            b'0'..=b'9' => saw_digit = true,
            b'.' if !saw_period => saw_period = true,
            _ => return false,
        }
    }
    saw_digit
}

/// Resolve and classify the Table-58 font-dictionary target.
fn classify_font_target(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reference: IndirectRef,
    size_bits: u64,
) -> ExtGStateFontEffect {
    let object_byte_offset = match resolve_reference(lookup, reference) {
        Ok((_, object_byte_offset)) => object_byte_offset,
        Err(ResolveReferenceError::Unresolved {
            reference,
            location,
        }) => match (location, reference.generation) {
            (location @ ObjectLookupLocation::XrefStreamCompressed { .. }, 0) => {
                return ExtGStateFontEffect::CompressedTargetUninspected {
                    reference,
                    location,
                };
            }
            // Compressed objects have implicit generation zero
            // (ISO 32000-1 §7.5.8.3), so any other generation resolves to
            // no usable font, exactly like an in-use generation mismatch.
            (ObjectLookupLocation::XrefStreamCompressed { .. }, _) => {
                return ExtGStateFontEffect::UnresolvedTarget {
                    reference,
                    location: None,
                };
            }
            (location, _) => {
                return ExtGStateFontEffect::UnresolvedTarget {
                    reference,
                    location: Some(location),
                };
            }
        },
        Err(ResolveReferenceError::GenerationMismatch { reference, .. }) => {
            return ExtGStateFontEffect::UnresolvedTarget {
                reference,
                location: None,
            };
        }
    };
    let header = match inspect_indirect_object_header(input, object_byte_offset) {
        Ok(header) => header,
        Err(error) => {
            return ExtGStateFontEffect::TargetDictionaryFailed {
                reference,
                object_byte_offset,
                error: IndirectObjectDictionaryInspectionError {
                    byte_offset: object_byte_offset,
                    byte_len: input.len(),
                    header_byte_offset: None,
                    error_byte_offset: error.error_byte_offset,
                    reason: IndirectObjectDictionaryInspectionRejection::Header {
                        header_reason: error.reason,
                    },
                },
            };
        }
    };
    if header.reference != reference {
        return ExtGStateFontEffect::TargetHeaderMismatch {
            reference,
            object_byte_offset,
            header_reference: header.reference,
        };
    }

    let dictionary = match inspect_indirect_object_dictionary(input, object_byte_offset) {
        Ok(dictionary) => dictionary,
        Err(error) => {
            return ExtGStateFontEffect::TargetDictionaryFailed {
                reference,
                object_byte_offset,
                error,
            };
        }
    };
    let (dictionary_type, subtype) = classify_font_dictionary_facts(input, &dictionary.entries);
    if dictionary_type == FontDictionaryTypeFact::Font && is_legal_direct_subtype(&subtype) {
        ExtGStateFontEffect::StructurallyValid {
            reference,
            object_byte_offset,
            size_bits,
            dictionary_type,
            subtype,
        }
    } else {
        ExtGStateFontEffect::InadmissibleTarget {
            reference,
            object_byte_offset,
            size_bits,
            dictionary_type,
            subtype,
        }
    }
}

/// The five legal direct current-font subtypes (ISO 32000-1 Table 110).
/// CID descendants and every other/unproven fact stay inadmissible.
const fn is_legal_direct_subtype(subtype: &FontSubtypeClass) -> bool {
    matches!(
        subtype,
        FontSubtypeClass::Type1
            | FontSubtypeClass::MmType1
            | FontSubtypeClass::TrueType
            | FontSubtypeClass::Type0
            | FontSubtypeClass::Type3
    )
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
