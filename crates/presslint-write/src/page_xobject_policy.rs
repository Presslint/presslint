//! Page `XObject` colour-effect policy between the advisory structural page
//! `/Resources /XObject` report and the alias-epoch proof's named-`Do`
//! classification.
//!
//! The policy is built once per analysed page from the page's exact
//! identity-matched [`PageXObjectResourcesInspection`] (or `None` when the
//! inspection or the page-identity join failed) and answers exactly one
//! question per invoked name: what does painting this named `XObject` do to
//! the CURRENT graphics-state colour?
//!
//! - [`PageXObjectEffect::OrdinaryImage`]: a `/Subtype /Image` target whose
//!   `/ImageMask` fact is `Missing` or explicit `False`. An ordinary image
//!   paints its own samples (ISO 32000-1 §8.9.5) and never reads the current
//!   colour, so it is colour-NEUTRAL for alias proof.
//! - [`PageXObjectEffect::Stencil`]: a structurally valid `/ImageMask true`
//!   stencil — positive `/Width` and `/Height` values, `/BitsPerComponent`
//!   absent or exactly `1`, and NO `/ColorSpace` entry (§8.9.6.2 forbids one).
//!   Painting a stencil CONSUMES the current nonstroking colour.
//! - [`PageXObjectEffect::Form`]: a `/Subtype /Form` target that this policy
//!   refuses without descent — either the structural constructor (which never
//!   analyzes) or a demanded Form whose exact analysis was Unknown. Forms
//!   inherit both colour lanes, so a refused Form keeps the historical
//!   fail-closed `Do` refusal.
//! - [`PageXObjectEffect::AnalyzedForm`]: a `/Subtype /Form` target demanded by
//!   a page `Do` whose exact read-only analysis proved a bounded inherited-lane
//!   colour effect. It carries whether painting inside the Form consumes the
//!   caller's inherited stroking and/or nonstroking colour; a neutral Form
//!   consumes neither lane and leaves alias roots live.
//! - [`PageXObjectEffect::Unknown`]: everything else, fail-closed — an
//!   uninspectable page, a name with no classified target, a named structural
//!   skip, an invalid stencil declaration, a malformed name escape, or a
//!   decoded-name collision.
//!
//! Matching is SEMANTIC: PDF name `#xx` escapes (§7.3.5) are decoded for both
//! report keys and `Do` operands, so `/Im#31` and `/Im1` are one name. Names
//! remain case-sensitive byte sequences after decoding. Two report entries
//! decoding to the same semantic name poison that name to `Unknown` (the raw
//! spellings cannot be told apart at invocation time), and a report key whose
//! escape is malformed poisons its literal byte spelling — a permissive viewer
//! may read the broken escape literally, so an operand decoding to those bytes
//! can never be proven distinct. A named skip always overrides a same-name
//! classified target; unrelated named skips poison nothing else; a page-scoped
//! skip (no resource name) makes EVERY name `Unknown`.
//!
//! Cost: one deterministic `BTreeMap` built per analysed page from the
//! already-produced report, and one O(log R) lookup per named `Do`. Form entries
//! in the production policy begin as exact unresolved targets and are analyzed
//! lazily on their first valid outer invocation; the entry is then replaced by
//! its two-bit effect or fail-closed refusal. Name keys are owned once during
//! map construction; unescaped `Do` operands borrow their bytes for lookup,
//! while escaped operands allocate at most one name-length buffer. No second
//! page walk or owned invocation-name set is created, and no Form bytes are
//! retained in the map.

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

use presslint_pdf::{
    ImageColorSpaceMetadata, ImageIntegerMetadata, ImageMaskMetadata, ImageXObjectMetadata,
    ObjectLookup, PageXObjectResourcesInspection,
};
use presslint_types::PdfName;

use crate::form_xobject_effect::FormXObjectEffectAnalyzer;

/// Colour effect of invoking one named page `XObject` with `Do`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageXObjectEffect {
    /// A non-stencil image: neutral to the current graphics-state colour.
    OrdinaryImage,
    /// A structurally valid stencil mask: consumes the nonstroking colour.
    Stencil,
    /// A form `XObject` refused without descent: inherits both lanes.
    Form,
    /// A demanded form `XObject` whose exact analysis proved its inherited-lane
    /// colour effect (a neutral form carries two `false` lanes).
    AnalyzedForm {
        /// Painting inside the form consumes the caller's inherited stroking
        /// colour.
        consumes_stroking: bool,
        /// Painting inside the form consumes the caller's inherited nonstroking
        /// colour.
        consumes_nonstroking: bool,
    },
    /// Anything unproven; fail-closed refusal.
    Unknown,
}

/// One entry in the sole semantic-name map. The unresolved form tuple is an
/// implementation detail, replaced in-place on its first valid outer `Do`.
#[derive(Clone, Copy)]
enum PageXObjectTarget {
    Resolved(PageXObjectEffect),
    UnresolvedForm {
        reference: presslint_pdf::IndirectRef,
        object_byte_offset: usize,
    },
}

/// Deterministic semantic-name map from one matched page `XObject` report.
pub struct PageXObjectPolicy<'analysis> {
    /// Decoded-name targets in deterministic byte order, or `None` when a
    /// page-scoped structural gap hides the complete name universe. `Cell`
    /// permits one unresolved Form entry to be replaced during an otherwise
    /// read-only alias-plan observation without widening existing callers.
    effects: Option<BTreeMap<Vec<u8>, Cell<PageXObjectTarget>>>,
    /// Production-only exact analysis context. Structural policies keep the
    /// whole tuple absent and resolve every Form directly to the historical
    /// refusal.
    analysis: Option<(
        &'analysis [u8],
        ObjectLookup<'analysis>,
        &'analysis RefCell<FormXObjectEffectAnalyzer>,
    )>,
}

impl<'analysis> PageXObjectPolicy<'analysis> {
    /// Build the policy from the page's exact identity-matched report.
    ///
    /// `None` means the document inspection or the exact page-identity join
    /// failed: every invoked name classifies `Unknown`. Forms always classify
    /// `Form` (structural refusal); the production path uses [`Self::analyzed`]
    /// to install lazily resolved exact targets. Retained for focused tests and
    /// fail-closed callers.
    #[must_use]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(report: Option<&PageXObjectResourcesInspection>) -> Self {
        Self {
            effects: build_effects(report, false),
            analysis: None,
        }
    }

    /// Build the production policy with lazy exact Form targets in the same sole
    /// semantic-name/collision/skip map.
    ///
    /// This mirrors [`Self::new`] for images, page-scoped skips, named skips and
    /// collision poisoning, but stores each unpoisoned Form as its exact
    /// `(reference, reached object offset)` target. [`Self::effect_of`] resolves
    /// that target only when the alias planner reaches a valid outer `Do`, then
    /// replaces it with [`PageXObjectEffect::AnalyzedForm`] or the fail-closed
    /// [`PageXObjectEffect::Form`]. A Form no page `Do` invokes is never analyzed.
    /// Every target still passes through [`insert_target`], so collision or skip
    /// poisoning wins before any analysis and can never be overridden.
    #[must_use]
    pub fn analyzed(
        report: Option<&PageXObjectResourcesInspection>,
        input: &'analysis [u8],
        lookup: ObjectLookup<'analysis>,
        analyzer: &'analysis RefCell<FormXObjectEffectAnalyzer>,
    ) -> Self {
        Self {
            effects: build_effects(report, true),
            analysis: Some((input, lookup, analyzer)),
        }
    }

    /// Classify one raw `Do` operand name (without the leading slash), lazily
    /// resolving and replacing an exact Form target when necessary.
    #[must_use]
    pub fn effect_of(&self, name: &PdfName) -> PageXObjectEffect {
        let Some(effects) = &self.effects else {
            return PageXObjectEffect::Unknown;
        };
        let Some(decoded) = decode_pdf_name(&name.0) else {
            return PageXObjectEffect::Unknown;
        };
        let Some(slot) = effects.get(decoded.as_ref()) else {
            return PageXObjectEffect::Unknown;
        };
        match slot.get() {
            PageXObjectTarget::Resolved(effect) => effect,
            PageXObjectTarget::UnresolvedForm {
                reference,
                object_byte_offset,
            } => {
                let effect = match self.analysis {
                    Some((input, lookup, analyzer)) => match analyzer.borrow_mut().analyze(
                        input,
                        lookup,
                        reference,
                        object_byte_offset,
                    ) {
                        Some([consumes_stroking, consumes_nonstroking]) => {
                            PageXObjectEffect::AnalyzedForm {
                                consumes_stroking,
                                consumes_nonstroking,
                            }
                        }
                        None => PageXObjectEffect::Form,
                    },
                    None => PageXObjectEffect::Form,
                };
                slot.set(PageXObjectTarget::Resolved(effect));
                effect
            }
        }
    }
}

/// Build the sole semantic-name map. A page-scoped skip makes the whole map
/// unavailable; named skips run last and poison only their semantic name.
fn build_effects(
    report: Option<&PageXObjectResourcesInspection>,
    unresolved_forms: bool,
) -> Option<BTreeMap<Vec<u8>, Cell<PageXObjectTarget>>> {
    let report = report?;
    if report
        .skipped
        .iter()
        .any(|skip| skip.resource_name.is_none())
    {
        return None;
    }
    let mut effects = BTreeMap::new();
    for target in &report.image_xobjects {
        insert_target(
            &mut effects,
            &target.name.0,
            PageXObjectTarget::Resolved(classify_image(target.image_metadata.as_ref())),
        );
    }
    for target in &report.form_xobjects {
        let form = if unresolved_forms {
            PageXObjectTarget::UnresolvedForm {
                reference: target.reference,
                object_byte_offset: target.object_byte_offset,
            }
        } else {
            PageXObjectTarget::Resolved(PageXObjectEffect::Form)
        };
        insert_target(&mut effects, &target.name.0, form);
    }
    for skip in &report.skipped {
        if let Some(name) = &skip.resource_name {
            effects.insert(
                semantic_key(&name.0),
                Cell::new(PageXObjectTarget::Resolved(PageXObjectEffect::Unknown)),
            );
        }
    }
    Some(effects)
}

/// Insert one classified target under its semantic name; a second entry for
/// the same semantic name is a collision and poisons it. A report key whose
/// escape is malformed keeps NO classified effect: it becomes an `Unknown`
/// poison entry under its literal byte spelling.
fn insert_target(
    effects: &mut BTreeMap<Vec<u8>, Cell<PageXObjectTarget>>,
    raw_name: &[u8],
    target: PageXObjectTarget,
) {
    let (key, target) = decode_pdf_name(raw_name).map_or_else(
        || {
            (
                raw_name.to_vec(),
                PageXObjectTarget::Resolved(PageXObjectEffect::Unknown),
            )
        },
        |decoded| (decoded.into_owned(), target),
    );
    effects
        .entry(key)
        .and_modify(|slot| {
            slot.set(PageXObjectTarget::Resolved(PageXObjectEffect::Unknown));
        })
        .or_insert_with(|| Cell::new(target));
}

/// The semantic map key of one raw report name: its decoded bytes, or — for a
/// malformed escape — its literal byte spelling, which then acts as a poison
/// entry against any operand decoding to those bytes.
fn semantic_key(raw_name: &[u8]) -> Vec<u8> {
    decode_pdf_name(raw_name).map_or_else(|| raw_name.to_vec(), Cow::into_owned)
}

/// Decode PDF name `#xx` escapes (ISO 32000-1 §7.3.5). `None` for a malformed
/// escape: a bare trailing `#`, a non-hex digit, or `#00` (NUL is not
/// permitted in a name).
///
/// This bounded writer-local decoder is shared crate-wide (the `ExtGState`
/// safety preflight and the page font policy reuse it for decoded-name
/// equality); its XObject-policy behaviour is unchanged. Unescaped names use
/// the borrowed fast path; an escaped name allocates at most one bounded
/// name-length buffer.
pub fn decode_pdf_name(raw: &[u8]) -> Option<Cow<'_, [u8]>> {
    if !raw.contains(&b'#') {
        return Some(Cow::Borrowed(raw));
    }
    let mut decoded = Vec::with_capacity(raw.len());
    let mut index = 0;
    while index < raw.len() {
        let byte = raw[index];
        if byte == b'#' {
            let high = hex_digit(*raw.get(index + 1)?)?;
            let low = hex_digit(*raw.get(index + 2)?)?;
            let value = high * 16 + low;
            if value == 0 {
                return None;
            }
            decoded.push(value);
            index += 3;
        } else {
            decoded.push(byte);
            index += 1;
        }
    }
    Some(Cow::Owned(decoded))
}

const fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Map one image target's structural metadata to its colour effect.
///
/// `Missing` and explicit `False` masks are BOTH ordinary images — the public
/// metadata keeps them distinct, the colour effect does not. A `True` mask is
/// a stencil only when the whole §8.9.6.2 shape holds; any other mask fact or
/// an absent metadata record is `Unknown`, fail-closed.
fn classify_image(metadata: Option<&ImageXObjectMetadata>) -> PageXObjectEffect {
    let Some(metadata) = metadata else {
        return PageXObjectEffect::Unknown;
    };
    match metadata.image_mask {
        ImageMaskMetadata::Missing | ImageMaskMetadata::False => PageXObjectEffect::OrdinaryImage,
        ImageMaskMetadata::True => {
            let valid = positive_dimension(metadata.width)
                && positive_dimension(metadata.height)
                && matches!(
                    metadata.bits_per_component,
                    ImageIntegerMetadata::Missing | ImageIntegerMetadata::Value { value: 1 }
                )
                && metadata.color_space == ImageColorSpaceMetadata::Missing;
            if valid {
                PageXObjectEffect::Stencil
            } else {
                PageXObjectEffect::Unknown
            }
        }
        ImageMaskMetadata::Duplicate { .. } | ImageMaskMetadata::Unsupported { .. } => {
            PageXObjectEffect::Unknown
        }
    }
}

const fn positive_dimension(value: ImageIntegerMetadata) -> bool {
    matches!(value, ImageIntegerMetadata::Value { value } if value > 0)
}
