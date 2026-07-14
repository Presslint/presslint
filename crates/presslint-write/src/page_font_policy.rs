//! Private page font policy between the exact page `/Font` and `/ExtGState`
//! reports and the writer's `TextShow` alias admission.
//!
//! One [`PageFontPolicy`] is built per analysed page from the page's exact
//! identity-matched `/Font` report (or `None`) and exact identity-matched
//! `/ExtGState` report (or `None`). It owns only compact page-local data and
//! exposes three borrowed/scalar views:
//!
//! - [`PageFontPolicy::font_env`] — the effective [`FontEnv`] for the page font
//!   namespace: a completely known binding slice (including known-empty), or
//!   [`FontEnv::unknown`] when coverage is structurally incomplete.
//! - [`PageFontPolicy::extgstate_env`] — the [`ExtGStateEnv`] the paint walk
//!   consumes so `gs` resolves its mapped `/Font` directive. The seven safety
//!   parameters are neutral here: the unchanged page preflight
//!   ([`crate::extgstate_page_guard`]) remains the controlling overprint/
//!   transparency gate, so this policy never relaxes it.
//! - [`PageFontPolicy::admits`] — the exact ordinary-font admission query: a
//!   [`FontSelectionState::ResolvedIndirect`] whose `(object, generation,
//!   reached offset)` tuple is present and unpoisoned in the admitted set.
//!
//! The admitted set is CORROBORATION, never re-resolution: it is built once from
//! the same already-classified named `/Font` entries and structurally valid
//! direct `/ExtGState` `/Font` effects, keyed by exact object number,
//! generation, and reached object byte offset. Ordinary indirect `Type1`,
//! `MMType1`, `TrueType`, and `Type0` selections converge safe; Type3, CID
//! descendants,
//! inadmissible targets, and every reached tuple that is not provably ordinary
//! poison their tuple fail-closed. Named `Tf` bindings still represent Type3 as
//! a selectable root in the environment exactly as the environment permits, but Type3 never
//! enters the admitted set.
//!
//! `/ExtGState` resource names are matched SEMANTICALLY. Two mapped resources
//! decoding to the same PDF name (for example `/GS1` and `/GS#31`) cannot be
//! told apart by paint's raw-name lookup, so their `/Font` directives are BOTH
//! poisoned to [`ExtGStateFontDirective::Unknown`]: a `gs` on either spelling
//! then clears font certainty and the following `TextShow` refuses. This mirrors
//! the collision poisoning already applied by the safety preflight; the
//! resulting `Indeterminate` refusal is the deliberate safe false-negative
//! boundary and needs no `presslint-pdf`/`presslint-paint` change.

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use presslint_paint::{
    ExtGStateEnv, ExtGStateFontDirective, ExtGStateParams, ExtGStateResource, FontBinding,
    FontBindingTarget, FontEnv, FontSelectionState, ResolvedFont,
};
use presslint_pdf::{
    ClassifiedExtGStateResource, ClassifiedFontResource, ExtGStateFontEffect,
    FontDictionaryTypeFact, FontSubtypeClass, PageExtGStateResourcesInspection,
    PageFontResourcesInspection, SkippedExtGStateResource, SkippedFontResourceReason,
};
use presslint_types::PdfName;

use crate::page_xobject_policy::decode_pdf_name;

/// Exact indirect font identity keyed in the admitted set:
/// `(object_number, generation, reached_object_byte_offset)`.
type FontTuple = (u32, u16, usize);

/// Owned page-local font bindings, mapped `ExtGState` resources, and the exact
/// unpoisoned ordinary-font admitted tuple set.
pub struct PageFontPolicy {
    /// Decoded page font bindings, or `None` when namespace coverage is
    /// structurally unknown (a `None` report, a malformed name, or an
    /// incompleteness-inducing skip). `Some(vec![])` is the distinct
    /// known-empty namespace.
    bindings: Option<Vec<FontBinding>>,
    /// Mapped `ExtGState` resources carrying neutral safety parameters and the
    /// consumer `/Font` directive, with semantic-collision groups poisoned.
    extgstates: Vec<ExtGStateResource>,
    /// Exact ordinary-font tuples that no safe/unsafe/conflicting fact poisoned.
    admitted: BTreeSet<FontTuple>,
}

impl PageFontPolicy {
    /// Build the policy from the page's exact identity-matched reports.
    #[must_use]
    pub fn new(
        font_report: Option<&PageFontResourcesInspection>,
        extgstate_report: Option<&PageExtGStateResourcesInspection>,
    ) -> Self {
        let bindings = font_report.and_then(build_bindings);
        // The mapped resources reuse only the `/Font` directive; the seven
        // safety parameters stay neutral because the page preflight is the
        // controlling safety gate. Semantic collisions poison the directive so
        // an ambiguous `gs` cannot select a font by raw-name first-win.
        let mut extgstates = extgstate_report.map_or_else(Vec::new, |report| {
            report.extgstates.iter().map(mapped_resource).collect()
        });
        poison_extgstate_ambiguity(
            &mut extgstates,
            extgstate_report.map_or(&[], |report| report.skipped.as_slice()),
        );

        let mut admission: BTreeMap<FontTuple, bool> = BTreeMap::new();
        if let Some(report) = font_report {
            for resource in &report.fonts {
                if let Some((tuple, safe)) = font_resource_fact(resource) {
                    record_admission(&mut admission, tuple, safe);
                }
            }
        }
        if let Some(report) = extgstate_report {
            for resource in &report.extgstates {
                if let Some((tuple, safe)) = extgstate_font_fact(&resource.font_effect) {
                    record_admission(&mut admission, tuple, safe);
                }
            }
        }
        let admitted = admission
            .into_iter()
            .filter_map(|(tuple, poisoned)| (!poisoned).then_some(tuple))
            .collect();

        Self {
            bindings,
            extgstates,
            admitted,
        }
    }

    /// Borrow the page font namespace as a [`FontEnv`].
    #[must_use]
    pub fn font_env(&self) -> FontEnv<'_> {
        self.bindings
            .as_deref()
            .map_or_else(FontEnv::unknown, FontEnv::known)
    }

    /// Borrow the mapped `ExtGState` resources as the paint walk environment.
    #[must_use]
    pub fn extgstate_env(&self) -> ExtGStateEnv<'_> {
        ExtGStateEnv::new(&self.extgstates)
    }

    /// Whether the effective font selection is an admitted exact ordinary
    /// indirect font. Only a [`FontSelectionState::ResolvedIndirect`] whose
    /// tuple is present and unpoisoned admits; raw `Selected`, `Unset`, and
    /// `Indeterminate` all refuse fail-closed.
    #[must_use]
    pub fn admits(&self, selection: &FontSelectionState) -> bool {
        match selection {
            FontSelectionState::ResolvedIndirect {
                object_number,
                generation,
                object_byte_offset,
                ..
            } => self
                .admitted
                .contains(&(*object_number, *generation, *object_byte_offset)),
            FontSelectionState::Unset
            | FontSelectionState::Selected { .. }
            | FontSelectionState::Indeterminate => false,
        }
    }
}

/// Map a completely classified page `/Font` report into cached semantic
/// bindings, mirroring the umbrella font mapping exactly. `None` means namespace
/// coverage is structurally unknown; an empty vector is the distinct
/// known-empty namespace.
fn build_bindings(report: &PageFontResourcesInspection) -> Option<Vec<FontBinding>> {
    let mut bindings = Vec::with_capacity(report.fonts.len());
    for resource in &report.fonts {
        let target = match (resource.reference, resource.object_byte_offset) {
            (Some(reference), Some(object_byte_offset)) if has_selectable_font_type(resource) => {
                FontBindingTarget::Resolved(ResolvedFont {
                    object_number: reference.object_number,
                    generation: reference.generation,
                    object_byte_offset,
                })
            }
            _ => FontBindingTarget::Unresolved,
        };
        bindings.push(FontBinding::from_pdf_name_bytes(&resource.name.0, target)?);
    }
    for skip in &report.skipped {
        let Some(name) = &skip.resource_name else {
            // A nameless skip that is not a benign missing-`/Font` fact makes
            // the whole namespace unknown.
            if !matches!(skip.reason, SkippedFontResourceReason::MissingFont) {
                return None;
            }
            continue;
        };
        let poison = FontBinding::from_pdf_name_bytes(&name.0, FontBindingTarget::Unresolved)?;
        if let Some(existing) = bindings
            .iter_mut()
            .find(|binding| binding.semantic_name() == poison.semantic_name())
        {
            *existing = poison;
        } else {
            bindings.push(poison);
        }
    }
    Some(bindings)
}

/// The selectable-root test used to build the environment binding (Type3
/// included). This is environment representation, not writer admission.
const fn has_selectable_font_type(resource: &ClassifiedFontResource) -> bool {
    matches!(resource.dictionary_type, FontDictionaryTypeFact::Font)
        && matches!(
            resource.subtype,
            FontSubtypeClass::Type1
                | FontSubtypeClass::MmType1
                | FontSubtypeClass::TrueType
                | FontSubtypeClass::Type0
                | FontSubtypeClass::Type3
        )
}

/// The exact ordinary writer-admission test: `/Type /Font` and one of the four
/// ordinary direct subtypes. Type3 is a legal current font but is never
/// writer-admitted here.
const fn is_admissible_ordinary_type(
    dictionary_type: &FontDictionaryTypeFact,
    subtype: &FontSubtypeClass,
) -> bool {
    matches!(dictionary_type, FontDictionaryTypeFact::Font)
        && matches!(
            subtype,
            FontSubtypeClass::Type1
                | FontSubtypeClass::MmType1
                | FontSubtypeClass::TrueType
                | FontSubtypeClass::Type0
        )
}

/// Extract one admission fact from a named `/Font` resource: any resource that
/// reached an exact indirect tuple contributes, safe only when it is provably
/// an ordinary `/Type /Font` subtype. A direct dictionary (no reached tuple)
/// contributes nothing.
fn font_resource_fact(resource: &ClassifiedFontResource) -> Option<(FontTuple, bool)> {
    let reference = resource.reference?;
    let object_byte_offset = resource.object_byte_offset?;
    let tuple = (
        reference.object_number,
        reference.generation,
        object_byte_offset,
    );
    Some((
        tuple,
        is_admissible_ordinary_type(&resource.dictionary_type, &resource.subtype),
    ))
}

/// Extract one admission fact from a direct `ExtGState` `/Font` effect. A
/// structurally valid effect is safe only for an ordinary subtype; a Type3
/// structurally valid effect and an inadmissible target both poison their
/// reached tuple. Every effect without a reached tuple contributes nothing.
const fn extgstate_font_fact(effect: &ExtGStateFontEffect) -> Option<(FontTuple, bool)> {
    match effect {
        ExtGStateFontEffect::StructurallyValid {
            reference,
            object_byte_offset,
            dictionary_type,
            subtype,
            ..
        } => Some((
            (
                reference.object_number,
                reference.generation,
                *object_byte_offset,
            ),
            is_admissible_ordinary_type(dictionary_type, subtype),
        )),
        ExtGStateFontEffect::InadmissibleTarget {
            reference,
            object_byte_offset,
            ..
        } => Some((
            (
                reference.object_number,
                reference.generation,
                *object_byte_offset,
            ),
            false,
        )),
        _ => None,
    }
}

/// Fold one fact into the admission map: a safe fact seeds an admittable tuple
/// without un-poisoning an existing one; any unsafe/conflicting fact poisons it
/// fail-closed. Repeated consistent safe facts converge.
fn record_admission(admission: &mut BTreeMap<FontTuple, bool>, tuple: FontTuple, safe: bool) {
    if safe {
        admission.entry(tuple).or_insert(false);
    } else {
        admission.insert(tuple, true);
    }
}

/// Map one classified `ExtGState` resource into the walker model with neutral
/// safety parameters and the consumer `/Font` directive.
fn mapped_resource(resource: &ClassifiedExtGStateResource) -> ExtGStateResource {
    ExtGStateResource {
        name: PdfName(resource.name.0.clone()),
        params: ExtGStateParams::empty(),
        has_unclassified_keys: resource.has_unclassified_keys,
        font: mapped_font_directive(resource),
    }
}

/// Map the classified `/Font` effect into the paint directive, mirroring the effective font semantics:
/// a structurally valid effect selects the exact indirect font+size, a proven
/// `Unset` (with no unclassified keys) leaves the current selection, and every
/// other effect clears certainty.
const fn mapped_font_directive(resource: &ClassifiedExtGStateResource) -> ExtGStateFontDirective {
    match &resource.font_effect {
        ExtGStateFontEffect::StructurallyValid {
            reference,
            object_byte_offset,
            size_bits,
            ..
        } => ExtGStateFontDirective::Select {
            font: ResolvedFont {
                object_number: reference.object_number,
                generation: reference.generation,
                object_byte_offset: *object_byte_offset,
            },
            size_bits: *size_bits,
        },
        ExtGStateFontEffect::Unset if !resource.has_unclassified_keys => {
            ExtGStateFontDirective::LeaveUnchanged
        }
        _ => ExtGStateFontDirective::Unknown,
    }
}

/// Poison every mapped `/Font` directive implicated by an `ExtGState`
/// structural skip or semantic-name collision.
///
/// A named skip is a same-semantic-name collision with a surviving classified
/// resource (for example a `DuplicateExtGStateName`): both spellings poison, so
/// a `gs` on either clears font certainty. A nameless skip is namespace-level
/// incompleteness (for example a `/Resources` inheritance diagnostic that still
/// left a partial classified set): any `gs` may name an unclassified resource,
/// so no mapped directive is trustworthy and every one poisons fail-closed.
/// Mapped resource names are decoded once for both checks; malformed names keep
/// their literal spelling, preserving the permissive-reader poison boundary.
fn poison_extgstate_ambiguity(
    resources: &mut [ExtGStateResource],
    skipped: &[SkippedExtGStateResource],
) {
    if skipped.iter().any(|skip| skip.resource_name.is_none()) {
        for resource in resources {
            resource.font = ExtGStateFontDirective::Unknown;
        }
        return;
    }

    let resource_keys: Vec<_> = resources
        .iter()
        .map(|resource| semantic_key(&resource.name.0))
        .collect();
    let skipped_keys: Vec<_> = skipped
        .iter()
        .filter_map(|skip| skip.resource_name.as_ref())
        .map(|name| semantic_key(&name.0))
        .collect();
    let mut counts: BTreeMap<&[u8], usize> = BTreeMap::new();
    for key in &resource_keys {
        *counts.entry(key.as_ref()).or_insert(0) += 1;
    }
    let poisoned: Vec<_> = resource_keys
        .iter()
        .map(|key| {
            counts.get(key.as_ref()).is_some_and(|count| *count > 1)
                || skipped_keys
                    .iter()
                    .any(|skipped| skipped.as_ref() == key.as_ref())
        })
        .collect();
    for (resource, poisoned) in resources.iter_mut().zip(poisoned) {
        if poisoned {
            resource.font = ExtGStateFontDirective::Unknown;
        }
    }
}

/// The semantic grouping key for one raw resource name: its decoded bytes, or
/// its literal spelling when the escape is malformed.
fn semantic_key(raw: &[u8]) -> Cow<'_, [u8]> {
    decode_pdf_name(raw).unwrap_or(Cow::Borrowed(raw))
}
