//! Borrowed font-resource environment and compact mapped font effects.
//!
//! This is the paint-native seam between structural PDF inspection and the
//! graphics-state walker. It contains no PDF object types: exact indirect font
//! identity is represented by scalar object/generation/offset fields, while
//! resource names are cached in their decoded PDF-name form.

use std::borrow::Cow;

use presslint_types::PdfName;

/// Exact reached identity of one eligible indirect font dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResolvedFont {
    /// PDF indirect object number.
    pub object_number: u32,
    /// PDF indirect object generation.
    pub generation: u16,
    /// Byte offset of the reached current indirect-object body.
    pub object_byte_offset: usize,
}

/// Result retained for one named font-resource binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontBindingTarget {
    /// Exact eligible indirect font identity.
    Resolved(ResolvedFont),
    /// The name is present, but no eligible indirect identity was proved.
    Unresolved,
}

/// One decoded named binding in a known font namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontBinding {
    semantic_name: PdfName,
    target: FontBindingTarget,
}

impl FontBinding {
    /// Decode and cache one raw PDF resource name.
    ///
    /// Returns `None` for a malformed/truncated/non-hexadecimal escape, an
    /// escape that decodes to null, or a literal null byte. Unescaped names use
    /// the borrowed decoder fast path and are copied once into this compact
    /// per-scope binding; escaped names allocate one bounded decoded buffer.
    #[must_use]
    pub fn from_pdf_name(name: &PdfName, target: FontBindingTarget) -> Option<Self> {
        Self::from_pdf_name_bytes(&name.0, target)
    }

    /// Decode and cache raw PDF-name bytes without requiring a model wrapper.
    #[must_use]
    pub fn from_pdf_name_bytes(name: &[u8], target: FontBindingTarget) -> Option<Self> {
        let semantic_name = PdfName(decode_pdf_name(name)?.into_owned());
        Some(Self {
            semantic_name,
            target,
        })
    }

    /// Cached decoded resource name.
    #[must_use]
    pub const fn semantic_name(&self) -> &PdfName {
        &self.semantic_name
    }

    /// Mapped target fact for this name.
    #[must_use]
    pub const fn target(&self) -> FontBindingTarget {
        self.target
    }
}

/// Mapped `/Font` directive on one `ExtGState` resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtGStateFontDirective {
    /// The dictionary is proven not to carry `/Font`; preserve current state.
    LeaveUnchanged,
    /// Atomically replace font identity and size.
    Select {
        /// Exact eligible indirect font identity.
        font: ResolvedFont,
        /// Exact finite size encoded as `f64::to_bits()`.
        size_bits: u64,
    },
    /// The font effect is absent-but-unproven, malformed, ambiguous, skipped,
    /// unresolved, compressed/uninspected, header-mismatched, or inadmissible.
    Unknown,
}

/// Borrowed coverage state for one page or Form font namespace.
///
/// `Known(&[])` is intentionally distinct from `Disabled`: the former enables
/// semantic resolution and therefore makes any named `Tf` indeterminate,
/// whereas the latter preserves the legacy raw-name paint behavior.
#[derive(Debug, Clone, Copy)]
pub enum FontEnv<'a> {
    /// Legacy compatibility mode: retain raw `Tf` and invalidate on every `gs`.
    Disabled,
    /// A completely known namespace, including the known-empty case.
    Known(&'a [FontBinding]),
    /// The namespace structure or report join is not trustworthy.
    Unknown,
}

impl<'a> FontEnv<'a> {
    /// Legacy compatibility environment.
    #[must_use]
    pub const fn disabled() -> Self {
        Self::Disabled
    }

    /// Completely known bindings; an empty slice means known-empty.
    #[must_use]
    pub const fn known(bindings: &'a [FontBinding]) -> Self {
        Self::Known(bindings)
    }

    /// Structurally unknown namespace.
    #[must_use]
    pub const fn unknown() -> Self {
        Self::Unknown
    }

    /// Whether legacy font behavior is selected.
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        matches!(self, Self::Disabled)
    }

    /// Resolve one raw `Tf` operand through decoded PDF-name equality.
    ///
    /// The operand is decoded at most once and the normally small binding slice
    /// is scanned linearly. `None` covers unknown coverage, malformed names,
    /// and a missing binding; enabled walker semantics fail closed for all three.
    #[must_use]
    pub fn resolve(self, raw_name: &PdfName) -> Option<FontBindingTarget> {
        let Self::Known(bindings) = self else {
            return None;
        };
        let semantic_name = decode_pdf_name(&raw_name.0)?;
        bindings
            .iter()
            .find(|binding| binding.semantic_name.0.as_slice() == semantic_name.as_ref())
            .map(FontBinding::target)
    }
}

impl Default for FontEnv<'_> {
    fn default() -> Self {
        Self::disabled()
    }
}

fn decode_pdf_name(name: &[u8]) -> Option<Cow<'_, [u8]>> {
    if !name.contains(&b'#') {
        return (!name.contains(&0)).then_some(Cow::Borrowed(name));
    }
    let mut decoded = Vec::with_capacity(name.len());
    let mut cursor = 0;
    while cursor < name.len() {
        if name[cursor] != b'#' {
            if name[cursor] == 0 {
                return None;
            }
            decoded.push(name[cursor]);
            cursor += 1;
            continue;
        }
        let high = hex_value(*name.get(cursor + 1)?)?;
        let low = hex_value(*name.get(cursor + 2)?)?;
        let byte = (high << 4) | low;
        if byte == 0 {
            return None;
        }
        decoded.push(byte);
        cursor += 3;
    }
    Some(Cow::Owned(decoded))
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
