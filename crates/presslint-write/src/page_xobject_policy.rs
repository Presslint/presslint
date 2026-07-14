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
//! - [`PageXObjectEffect::Form`]: a `/Subtype /Form` target; forms inherit
//!   both colour lanes and stay refused because this policy does not descend
//!   into their content streams.
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
//! already-produced report, and one O(log R) lookup per named `Do`. Name
//! keys are owned once during map construction; unescaped `Do` operands borrow
//! their bytes for lookup, while escaped operands allocate at most one
//! name-length buffer. No dictionaries, streams, or pixels are retained or
//! decoded here.

use std::{borrow::Cow, collections::BTreeMap};

use presslint_pdf::{
    ImageColorSpaceMetadata, ImageIntegerMetadata, ImageMaskMetadata, ImageXObjectMetadata,
    PageXObjectResourcesInspection,
};
use presslint_types::PdfName;

/// Colour effect of invoking one named page `XObject` with `Do`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageXObjectEffect {
    /// A non-stencil image: neutral to the current graphics-state colour.
    OrdinaryImage,
    /// A structurally valid stencil mask: consumes the nonstroking colour.
    Stencil,
    /// A form `XObject`: inherits both lanes, refused without descent.
    Form,
    /// Anything unproven; fail-closed refusal.
    Unknown,
}

/// Deterministic semantic-name map from one matched page `XObject` report.
pub struct PageXObjectPolicy {
    /// Decoded-name effects in deterministic byte order, or `None` when a
    /// page-scoped structural gap hides the complete name universe.
    effects: Option<BTreeMap<Vec<u8>, PageXObjectEffect>>,
}

impl PageXObjectPolicy {
    /// Build the policy from the page's exact identity-matched report.
    ///
    /// `None` means the document inspection or the exact page-identity join
    /// failed: every invoked name classifies `Unknown`.
    #[must_use]
    pub fn new(report: Option<&PageXObjectResourcesInspection>) -> Self {
        let Some(report) = report else {
            return Self { effects: None };
        };
        // Any skip WITHOUT a resource name is page-scoped (missing/duplicate/
        // malformed `/Resources` or `/XObject` shapes): the report cannot even
        // enumerate the names, so nothing on this page is provable.
        if report
            .skipped
            .iter()
            .any(|skip| skip.resource_name.is_none())
        {
            return Self { effects: None };
        }
        let mut effects = BTreeMap::new();
        for target in &report.image_xobjects {
            insert_target(
                &mut effects,
                &target.name.0,
                classify_image(target.image_metadata.as_ref()),
            );
        }
        for target in &report.form_xobjects {
            insert_target(&mut effects, &target.name.0, PageXObjectEffect::Form);
        }
        // Named skips run LAST and unconditionally: a same-name skip overrides
        // any prior classified target, and only that semantic name.
        for skip in &report.skipped {
            if let Some(name) = &skip.resource_name {
                effects.insert(semantic_key(&name.0), PageXObjectEffect::Unknown);
            }
        }
        Self {
            effects: Some(effects),
        }
    }

    /// Classify one raw `Do` operand name (without the leading slash).
    #[must_use]
    pub fn effect_of(&self, name: &PdfName) -> PageXObjectEffect {
        let Some(effects) = &self.effects else {
            return PageXObjectEffect::Unknown;
        };
        let Some(decoded) = decode_pdf_name(&name.0) else {
            return PageXObjectEffect::Unknown;
        };
        effects
            .get(decoded.as_ref())
            .copied()
            .unwrap_or(PageXObjectEffect::Unknown)
    }
}

/// Insert one classified target under its semantic name; a second entry for
/// the same semantic name is a collision and poisons it. A report key whose
/// escape is malformed keeps NO classified effect: it becomes an `Unknown`
/// poison entry under its literal byte spelling.
fn insert_target(
    effects: &mut BTreeMap<Vec<u8>, PageXObjectEffect>,
    raw_name: &[u8],
    effect: PageXObjectEffect,
) {
    let (key, effect) = decode_pdf_name(raw_name).map_or_else(
        || (raw_name.to_vec(), PageXObjectEffect::Unknown),
        |decoded| (decoded.into_owned(), effect),
    );
    effects
        .entry(key)
        .and_modify(|slot| *slot = PageXObjectEffect::Unknown)
        .or_insert(effect);
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
