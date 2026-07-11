use serde::{Deserialize, Serialize};

use crate::page_resource_inheritance::unique_entry;
use crate::{DictionaryEntryByteRange, DictionaryEntrySpan, DictionaryValueKind};

/// Structural dictionary-level metadata read from a resolved `/Subtype /Image`
/// `XObject`.
///
/// This report is intentionally structural and copy-cheap: it carries only the
/// five scalar image dictionary entries `/Width`, `/Height`,
/// `/BitsPerComponent`, `/ColorSpace`, and `/ImageMask`, each mapped to a value
/// or an explicit unsupported shape. It never decodes image samples and never
/// retains PDF bytes, object bodies, stream bodies, resource dictionaries,
/// decoded image data, or ICC/profile bytes; the only owned bytes are a copied
/// non-device `/ColorSpace` name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageXObjectMetadata {
    /// Direct `/Width` image dimension.
    pub width: ImageIntegerMetadata,
    /// Direct `/Height` image dimension.
    pub height: ImageIntegerMetadata,
    /// Direct `/BitsPerComponent` sample depth.
    pub bits_per_component: ImageIntegerMetadata,
    /// Direct `/ColorSpace` device family or explicit unsupported shape.
    pub color_space: ImageColorSpaceMetadata,
    /// Direct `/ImageMask` stencil-mask declaration (ISO 32000-1 §8.9.6.2).
    ///
    /// The default [`ImageMaskMetadata::Missing`] is omitted from serialized
    /// reports and absent in legacy reports, so pre-existing JSON shapes stay
    /// byte-identical and deserialize unchanged. `Missing` and `False` are
    /// deliberately distinct structural facts and are never collapsed here.
    #[serde(default, skip_serializing_if = "ImageMaskMetadata::is_missing")]
    pub image_mask: ImageMaskMetadata,
}

/// A scalar integer image dictionary entry (`/Width`, `/Height`, or
/// `/BitsPerComponent`), mapped to a value or an explicit unsupported shape.
///
/// Every non-value variant keeps the shape explicit rather than guessed, so a
/// downstream consumer can tell "absent" from "present but malformed" from
/// "present but not an integer".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageIntegerMetadata {
    /// A direct non-negative 32-bit integer value.
    Value {
        /// Parsed non-negative integer.
        value: u32,
    },
    /// The key was absent from the image dictionary.
    Missing,
    /// The key occurred more than once.
    Duplicate {
        /// First matching key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate matching key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The value was present but not a number-shaped scalar (a name, array,
    /// dictionary, string, indirect reference, and so on).
    Unsupported {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
    /// The value was number-shaped but not a non-negative 32-bit integer
    /// (a real, a signed value, or an out-of-range magnitude).
    Malformed,
}

/// The `/ColorSpace` image dictionary entry, mapped to a device colour family
/// when it is a direct device name, or to an explicit unsupported shape.
///
/// Only the three direct device names are mapped. Any other direct name, and
/// every non-name value (an array such as `[/ICCBased ...]`/`[/Indexed ...]`, an
/// indirect reference, a dictionary, and so on) stays explicit rather than
/// guessed, preserving fail-closed handling for unsupported, malformed,
/// indirect, or unresolved colour-space shapes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageColorSpaceMetadata {
    /// Direct `/DeviceGray`.
    DeviceGray,
    /// Direct `/DeviceRGB`.
    DeviceRgb,
    /// Direct `/DeviceCMYK`.
    DeviceCmyk,
    /// The `/ColorSpace` key was absent.
    Missing,
    /// The `/ColorSpace` key occurred more than once.
    Duplicate {
        /// First `/ColorSpace` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/ColorSpace` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// A direct `/Name` other than the three device families (for example a
    /// resource-alias colour space). Raw name bytes including the leading
    /// slash.
    OtherName {
        /// Raw name bytes including the leading slash.
        name: Vec<u8>,
    },
    /// The value was not a direct name: an array colour space
    /// (`ICCBased`, `Indexed`, `CalRGB`, `CalGray`, `Lab`, `Separation`,
    /// `DeviceN`), an indirect reference, a dictionary, or any other shape.
    /// Deliberately not resolved in this structural slice.
    Unsupported {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

/// The `/ImageMask` image dictionary entry, mapped to an exact boolean fact or
/// an explicit unsupported shape.
///
/// The scanner reports `True`/`False` only for the exact scalar keywords
/// `true`/`false`; every other value shape (a number, name, string, array,
/// dictionary, or an indirect reference — deliberately not resolved in this
/// structural slice) stays an explicit `Unsupported` fact rather than a guess.
/// There is no separate malformed variant: booleans are exact by construction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ImageMaskMetadata {
    /// The `/ImageMask` key was absent (the PDF default: not a stencil mask).
    #[default]
    Missing,
    /// An explicit direct `false` scalar.
    False,
    /// An explicit direct `true` scalar: the image declares itself a stencil
    /// mask whose painted samples take the current nonstroking colour.
    True,
    /// The `/ImageMask` key occurred more than once.
    Duplicate {
        /// First `/ImageMask` key range observed.
        first_key_range: DictionaryEntryByteRange,
        /// Duplicate `/ImageMask` key range observed.
        duplicate_key_range: DictionaryEntryByteRange,
    },
    /// The value was present but not an exact `true`/`false` scalar.
    Unsupported {
        /// Shallow value kind reported by dictionary entry inspection.
        value_kind: DictionaryValueKind,
    },
}

impl ImageMaskMetadata {
    /// Whether this is the default absent fact (the serde omission gate).
    #[must_use]
    pub const fn is_missing(&self) -> bool {
        matches!(self, Self::Missing)
    }
}

/// Inspect an already-resolved image `XObject` dictionary's shallow entries for
/// structural `/Width`, `/Height`, `/BitsPerComponent`, `/ColorSpace`, and
/// `/ImageMask` metadata.
///
/// The caller supplies the `entries` from
/// [`crate::inspect_indirect_object_dictionary`] over the resolved
/// `/Subtype /Image` target, so this performs one bounded scan per entry family
/// over data already inspected during subtype classification. It resolves no
/// sub-references, decodes no samples, and copies only small scalar metadata
/// plus a non-device `/ColorSpace` name.
#[must_use]
pub fn inspect_image_xobject_metadata(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
) -> ImageXObjectMetadata {
    ImageXObjectMetadata {
        width: integer_metadata(input, entries, b"/Width"),
        height: integer_metadata(input, entries, b"/Height"),
        bits_per_component: integer_metadata(input, entries, b"/BitsPerComponent"),
        color_space: color_space_metadata(input, entries),
        image_mask: image_mask_metadata(input, entries),
    }
}

fn integer_metadata(
    input: &[u8],
    entries: &[DictionaryEntrySpan],
    key: &[u8],
) -> ImageIntegerMetadata {
    let entry = match unique_entry(input, entries, key) {
        Ok(Some(entry)) => entry,
        Ok(None) => return ImageIntegerMetadata::Missing,
        Err((first_key_range, duplicate_key_range)) => {
            return ImageIntegerMetadata::Duplicate {
                first_key_range,
                duplicate_key_range,
            };
        }
    };

    if entry.value_kind != DictionaryValueKind::NumberLike {
        return ImageIntegerMetadata::Unsupported {
            value_kind: entry.value_kind,
        };
    }

    parse_non_negative_u32(&input[entry.value_range.start..entry.value_range.end])
        .map_or(ImageIntegerMetadata::Malformed, |value| {
            ImageIntegerMetadata::Value { value }
        })
}

/// Parse a bare non-negative decimal integer. A number-shaped scalar carrying a
/// sign, a decimal point, or an out-of-range magnitude yields `None` so the
/// caller reports it as an explicit malformed shape rather than guessing.
fn parse_non_negative_u32(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: u32 = 0;
    for &byte in bytes {
        value = value.checked_mul(10)?.checked_add(u32::from(byte - b'0'))?;
    }
    Some(value)
}

fn image_mask_metadata(input: &[u8], entries: &[DictionaryEntrySpan]) -> ImageMaskMetadata {
    let entry = match unique_entry(input, entries, b"/ImageMask") {
        Ok(Some(entry)) => entry,
        Ok(None) => return ImageMaskMetadata::Missing,
        Err((first_key_range, duplicate_key_range)) => {
            return ImageMaskMetadata::Duplicate {
                first_key_range,
                duplicate_key_range,
            };
        }
    };

    // Dictionary inspection classifies exactly the `true`/`false` keywords as
    // `Boolean`, so the byte match below is total; the fallthrough only guards
    // a future classifier drift and stays an explicit unsupported fact.
    if entry.value_kind == DictionaryValueKind::Boolean {
        match &input[entry.value_range.start..entry.value_range.end] {
            b"true" => return ImageMaskMetadata::True,
            b"false" => return ImageMaskMetadata::False,
            _ => {}
        }
    }
    ImageMaskMetadata::Unsupported {
        value_kind: entry.value_kind,
    }
}

fn color_space_metadata(input: &[u8], entries: &[DictionaryEntrySpan]) -> ImageColorSpaceMetadata {
    let entry = match unique_entry(input, entries, b"/ColorSpace") {
        Ok(Some(entry)) => entry,
        Ok(None) => return ImageColorSpaceMetadata::Missing,
        Err((first_key_range, duplicate_key_range)) => {
            return ImageColorSpaceMetadata::Duplicate {
                first_key_range,
                duplicate_key_range,
            };
        }
    };

    if entry.value_kind != DictionaryValueKind::Name {
        return ImageColorSpaceMetadata::Unsupported {
            value_kind: entry.value_kind,
        };
    }

    match &input[entry.value_range.start..entry.value_range.end] {
        b"/DeviceGray" => ImageColorSpaceMetadata::DeviceGray,
        b"/DeviceRGB" => ImageColorSpaceMetadata::DeviceRgb,
        b"/DeviceCMYK" => ImageColorSpaceMetadata::DeviceCmyk,
        other => ImageColorSpaceMetadata::OtherName {
            name: other.to_vec(),
        },
    }
}
