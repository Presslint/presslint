//! Structural classification of one `/Resources /ColorSpace` entry.
//!
//! Split out of `page_color_space_resources.rs` (which owns the page-tree walk,
//! report types, and skip taxonomy) to keep each module under the file-size
//! gate. This module classifies a single colour-space definition — a bare name,
//! a `[ … ]` array, or an indirect definition — into a [`ColorSpaceFamily`]
//! plus its shallow component count, spot colorant names, and alternate family.
//! It resolves resource SHAPES only: no paint semantics, colourimetry, tint
//! transform evaluation, or profile parsing beyond the shallow `/N`.

use crate::page_resource_inheritance::{ResolveReferenceError, resolve_reference, unique_entry};
use crate::source_utils::{
    parse_usize_decimal, skip_name, skip_scalar_token, skip_whitespace_and_comments,
};
use crate::{
    DictionaryEntrySpan, DictionaryValueKind, IndirectObjectBodyLeadingTokenKind, IndirectRef,
    ObjectLookup, PdfName, inspect_array_extent, inspect_indirect_object_body_token,
    inspect_indirect_object_dictionary, inspect_indirect_object_header, parse_indirect_reference,
};

use super::{
    ClassifiedColorSpaceDefinition, ClassifiedColorSpaceResource, ColorSpaceFamily,
    SkippedColorSpaceResourceReason,
};
use crate::page_color_space_resources::IndexedLookupDescriptor;

/// Classify one `/ColorSpace` dictionary entry into a structural family.
pub fn classify_color_space_entry(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    name: &PdfName,
    entry: DictionaryEntrySpan,
) -> Result<ClassifiedColorSpaceResource, SkippedColorSpaceResourceReason> {
    let definition = classify_color_space_definition_entry(input, lookup, entry)?;
    Ok(ClassifiedColorSpaceResource {
        name: name.clone(),
        family: definition.family,
        component_count: definition.component_count,
        spot_names: definition.spot_names,
        alternate_space: definition.alternate_space,
        base_space: definition.base_space,
        indexed_hival: definition.indexed_hival,
        indexed_lookup: definition.indexed_lookup,
    })
}

/// Classify one colour-space definition entry into a structural definition,
/// without assigning or implying a selectable resource name.
pub fn classify_color_space_definition_entry(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    entry: DictionaryEntrySpan,
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    match entry.value_kind {
        DictionaryValueKind::Name => {
            classify_name_family(&input[entry.value_range.start..entry.value_range.end])
        }
        DictionaryValueKind::Array => classify_array(input, lookup, entry.value_range.start),
        DictionaryValueKind::IndirectReferenceLike => {
            let reference = parse_indirect_reference(input, entry.value_range.start)
                .map_err(|_| SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
            let (_, object_byte_offset) = resolve_reference(lookup, reference.reference)
                .map_err(|error| unresolved_reference(&error))?;
            classify_indirect_definition(input, lookup, object_byte_offset)
        }
        _ => Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand),
    }
}

/// Classify a colour space defined as its own indirect object (its body is a
/// name or an array).
fn classify_indirect_definition(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    object_byte_offset: usize,
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    let header = inspect_indirect_object_header(input, object_byte_offset)
        .map_err(|_| SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
    let token = inspect_indirect_object_body_token(input, header.after_obj_keyword_offset)
        .map_err(|_| SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
    match token.token_kind {
        IndirectObjectBodyLeadingTokenKind::Name => {
            let end = skip_name(input, token.first_token_byte_offset, input.len());
            classify_name_family(&input[token.first_token_byte_offset..end])
        }
        IndirectObjectBodyLeadingTokenKind::ArrayOpen => {
            classify_array(input, lookup, token.first_token_byte_offset)
        }
        _ => Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand),
    }
}

/// Classify a bare colour-space name (device families; anything else skips).
fn classify_name_family(
    name_bytes: &[u8],
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    let family = match name_bytes {
        b"/DeviceGray" | b"/G" => (ColorSpaceFamily::DeviceGray, 1usize),
        b"/DeviceRGB" | b"/RGB" => (ColorSpaceFamily::DeviceRgb, 3),
        b"/DeviceCMYK" | b"/CMYK" => (ColorSpaceFamily::DeviceCmyk, 4),
        b"/Pattern" => return Err(SkippedColorSpaceResourceReason::UnsupportedPatternColor),
        other => {
            return Err(SkippedColorSpaceResourceReason::UnknownColorSpaceName {
                name: other.to_vec(),
            });
        }
    };
    Ok(ClassifiedColorSpaceDefinition {
        family: family.0,
        component_count: Some(family.1),
        spot_names: Vec::new(),
        alternate_space: None,
        base_space: None,
        indexed_hival: None,
        indexed_lookup: None,
    })
}

/// Classify a `[ … ]` colour-space array at its opening `[`.
fn classify_array(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    open_offset: usize,
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    let elements = array_elements(input, open_offset)?;
    let Some(head) = elements.first() else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let ArrayElementKind::Name = head.kind else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    match &input[head.start..head.end] {
        b"/ICCBased" => classify_icc_based(input, lookup, &elements),
        b"/Separation" => classify_separation(input, lookup, &elements),
        b"/DeviceN" => classify_device_n(input, lookup, &elements),
        b"/Indexed" | b"/I" => classify_indexed(input, lookup, &elements),
        b"/Pattern" => Err(SkippedColorSpaceResourceReason::UnsupportedPatternColor),
        b"/Lab" | b"/CalGray" | b"/CalRGB" => {
            Err(SkippedColorSpaceResourceReason::UnsupportedLabOrCalSpace)
        }
        b"/DeviceGray" | b"/DeviceRGB" | b"/DeviceCMYK" => {
            classify_name_family(&input[head.start..head.end])
        }
        other => Err(SkippedColorSpaceResourceReason::UnknownColorSpaceName {
            name: other.to_vec(),
        }),
    }
}

fn classify_icc_based(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    elements: &[ArrayElement],
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    let Some(stream) = elements.get(1) else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let ArrayElementKind::Reference(reference) = stream.kind else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let (_, object_byte_offset) =
        resolve_reference(lookup, reference).map_err(|error| unresolved_reference(&error))?;
    let dictionary = inspect_indirect_object_dictionary(input, object_byte_offset)
        .map_err(|_| SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
    let component_count = dictionary_usize(input, &dictionary.entries, b"/N");
    let alternate_space = dictionary_alternate(input, lookup, &dictionary.entries);
    Ok(ClassifiedColorSpaceDefinition {
        family: ColorSpaceFamily::IccBased,
        component_count,
        spot_names: Vec::new(),
        alternate_space,
        base_space: None,
        indexed_hival: None,
        indexed_lookup: None,
    })
}

fn classify_separation(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    elements: &[ArrayElement],
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    let Some(colorant) = elements.get(1) else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let ArrayElementKind::Name = colorant.kind else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let spot = PdfName(input[colorant.start + 1..colorant.end].to_vec());
    let alternate_space = elements
        .get(2)
        .and_then(|element| classify_element_family(input, lookup, element));
    Ok(ClassifiedColorSpaceDefinition {
        family: ColorSpaceFamily::Separation,
        component_count: Some(1),
        spot_names: vec![spot],
        alternate_space,
        base_space: None,
        indexed_hival: None,
        indexed_lookup: None,
    })
}

fn classify_device_n(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    elements: &[ArrayElement],
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    let Some(names) = elements.get(1) else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let ArrayElementKind::Array = names.kind else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let colorants = array_elements(input, names.start)?;
    let mut spot_names = Vec::with_capacity(colorants.len());
    for colorant in &colorants {
        let ArrayElementKind::Name = colorant.kind else {
            return Err(SkippedColorSpaceResourceReason::UnsupportedTintTransform);
        };
        spot_names.push(PdfName(input[colorant.start + 1..colorant.end].to_vec()));
    }
    if spot_names.is_empty() {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    }
    let alternate_space = elements
        .get(2)
        .and_then(|element| classify_element_family(input, lookup, element));
    let component_count = Some(spot_names.len());
    Ok(ClassifiedColorSpaceDefinition {
        family: ColorSpaceFamily::DeviceN,
        component_count,
        spot_names,
        alternate_space,
        base_space: None,
        indexed_hival: None,
        indexed_lookup: None,
    })
}

/// Classify a shallow `[/Indexed base hival lookup]` definition into a
/// structural fact: family `Indexed`, one index paint operand, the shallowly
/// classified base family, the direct integer `hival`, and a descriptor-only
/// lookup shape. No palette expansion, lookup decoding, or byte retention.
fn classify_indexed(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    elements: &[ArrayElement],
) -> Result<ClassifiedColorSpaceDefinition, SkippedColorSpaceResourceReason> {
    let (Some(base), Some(hival), Some(lookup_operand)) =
        (elements.get(1), elements.get(2), elements.get(3))
    else {
        return Err(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand);
    };
    let indexed_hival = non_negative_integer_element(input, hival)
        .ok_or(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
    let base_space = classify_indexed_base_family(input, lookup, base)?;
    Ok(ClassifiedColorSpaceDefinition {
        family: ColorSpaceFamily::Indexed,
        // Painting an Indexed colour takes exactly one index operand.
        component_count: Some(1),
        spot_names: Vec::new(),
        alternate_space: None,
        base_space,
        indexed_hival: Some(indexed_hival),
        indexed_lookup: Some(indexed_lookup_descriptor(input, lookup_operand)),
    })
}

fn classify_indexed_base_family(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    base: &ArrayElement,
) -> Result<Option<ColorSpaceFamily>, SkippedColorSpaceResourceReason> {
    // An indirect base that does not resolve stays an explicit skip; any other
    // shallowly unclassifiable base is honestly recorded as `None`.
    if let ArrayElementKind::Reference(reference) = base.kind {
        let (_, object_byte_offset) =
            resolve_reference(lookup, reference).map_err(|error| unresolved_reference(&error))?;
        return Ok(classify_indirect_name_family(input, object_byte_offset));
    }
    Ok(classify_element_family(input, lookup, base))
}

/// Read a shallow non-negative direct integer array element (`hival`).
///
/// ISO 32000-1 §7.3.3 integer syntax permits an optional sign, so an
/// explicitly positive token such as `+15` is a valid non-negative integer.
/// Negative-signed tokens stay unaccepted.
fn non_negative_integer_element(input: &[u8], element: &ArrayElement) -> Option<usize> {
    let ArrayElementKind::Other = element.kind else {
        return None;
    };
    let bytes = input.get(element.start..element.end)?;
    let digits = bytes.strip_prefix(b"+").unwrap_or(bytes);
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    parse_usize_decimal(digits)
}

/// Describe the shallow syntactic shape of an Indexed lookup operand without
/// resolving, reading, decoding, or retaining any lookup bytes.
fn indexed_lookup_descriptor(input: &[u8], element: &ArrayElement) -> IndexedLookupDescriptor {
    match element.kind {
        ArrayElementKind::Reference(reference) => IndexedLookupDescriptor::Reference {
            object_number: reference.object_number,
            generation: reference.generation,
        },
        ArrayElementKind::Other => match input.get(element.start) {
            Some(b'(') => IndexedLookupDescriptor::LiteralString,
            Some(b'<') if input.get(element.start + 1) != Some(&b'<') => {
                IndexedLookupDescriptor::HexString {
                    byte_len: hex_string_byte_len(&input[element.start..element.end]),
                }
            }
            _ => IndexedLookupDescriptor::Unknown,
        },
        ArrayElementKind::Name | ArrayElementKind::Array => IndexedLookupDescriptor::Unknown,
    }
}

/// Decoded byte length of a hex string span `<…>`: two hex digits per byte,
/// with a trailing odd digit implying a final zero (ISO 32000-1 §7.3.4.3).
/// Only digit COUNTING happens here; no bytes are decoded or retained.
fn hex_string_byte_len(span: &[u8]) -> usize {
    let digits = span.iter().filter(|byte| byte.is_ascii_hexdigit()).count();
    digits.div_ceil(2)
}

/// Shallow family classification of an alternate-space or Indexed-base array
/// element (name, nested array head, or a reference to a bare name). Returns
/// `None` when it cannot be determined shallowly.
fn classify_element_family(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    element: &ArrayElement,
) -> Option<ColorSpaceFamily> {
    match element.kind {
        ArrayElementKind::Name => name_family(&input[element.start..element.end]),
        ArrayElementKind::Array => {
            let inner = array_elements(input, element.start).ok()?;
            let head = inner.first()?;
            match head.kind {
                ArrayElementKind::Name => array_head_family(&input[head.start..head.end]),
                _ => None,
            }
        }
        ArrayElementKind::Reference(reference) => {
            let (_, object_byte_offset) = resolve_reference(lookup, reference).ok()?;
            classify_indirect_name_family(input, object_byte_offset)
        }
        ArrayElementKind::Other => None,
    }
}

fn classify_indirect_name_family(
    input: &[u8],
    object_byte_offset: usize,
) -> Option<ColorSpaceFamily> {
    let header = inspect_indirect_object_header(input, object_byte_offset).ok()?;
    let token = inspect_indirect_object_body_token(input, header.after_obj_keyword_offset).ok()?;
    match token.token_kind {
        IndirectObjectBodyLeadingTokenKind::Name => {
            let end = skip_name(input, token.first_token_byte_offset, input.len());
            name_family(&input[token.first_token_byte_offset..end])
        }
        _ => None,
    }
}

const fn name_family(bytes: &[u8]) -> Option<ColorSpaceFamily> {
    match bytes {
        b"/DeviceGray" | b"/G" => Some(ColorSpaceFamily::DeviceGray),
        b"/DeviceRGB" | b"/RGB" => Some(ColorSpaceFamily::DeviceRgb),
        b"/DeviceCMYK" | b"/CMYK" => Some(ColorSpaceFamily::DeviceCmyk),
        _ => None,
    }
}

const fn array_head_family(bytes: &[u8]) -> Option<ColorSpaceFamily> {
    match bytes {
        b"/ICCBased" => Some(ColorSpaceFamily::IccBased),
        b"/Separation" => Some(ColorSpaceFamily::Separation),
        b"/DeviceN" => Some(ColorSpaceFamily::DeviceN),
        _ => None,
    }
}

const fn unresolved_reference(error: &ResolveReferenceError) -> SkippedColorSpaceResourceReason {
    match error {
        ResolveReferenceError::Unresolved {
            reference,
            location,
        } => SkippedColorSpaceResourceReason::UnresolvedResourceReference {
            reference: *reference,
            location: Some(*location),
        },
        ResolveReferenceError::GenerationMismatch { reference, .. } => {
            SkippedColorSpaceResourceReason::UnresolvedResourceReference {
                reference: *reference,
                location: None,
            }
        }
    }
}

/// Read a shallow non-negative integer dictionary value.
fn dictionary_usize(input: &[u8], entries: &[DictionaryEntrySpan], key: &[u8]) -> Option<usize> {
    let entry = unique_entry(input, entries, key).ok().flatten()?;
    if entry.value_kind != DictionaryValueKind::NumberLike {
        return None;
    }
    let bytes = input.get(entry.value_range.start..entry.value_range.end)?;
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    parse_usize_decimal(bytes)
}

/// Shallow `/Alternate` family classification for an `ICCBased` dictionary.
fn dictionary_alternate(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    entries: &[DictionaryEntrySpan],
) -> Option<ColorSpaceFamily> {
    let entry = unique_entry(input, entries, b"/Alternate").ok().flatten()?;
    match entry.value_kind {
        DictionaryValueKind::Name => {
            name_family(&input[entry.value_range.start..entry.value_range.end])
        }
        DictionaryValueKind::Array => {
            let inner = array_elements(input, entry.value_range.start).ok()?;
            let head = inner.first()?;
            match head.kind {
                ArrayElementKind::Name => array_head_family(&input[head.start..head.end]),
                _ => None,
            }
        }
        DictionaryValueKind::IndirectReferenceLike => {
            let reference = parse_indirect_reference(input, entry.value_range.start).ok()?;
            let element = ArrayElement {
                start: entry.value_range.start,
                end: entry.value_range.end,
                kind: ArrayElementKind::Reference(reference.reference),
            };
            classify_element_family(input, lookup, &element)
        }
        _ => None,
    }
}

/// Kind of a shallow colour-space array element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayElementKind {
    Name,
    Array,
    Reference(IndirectRef),
    Other,
}

/// One shallow colour-space array element byte span plus its kind.
#[derive(Debug, Clone, Copy)]
struct ArrayElement {
    start: usize,
    end: usize,
    kind: ArrayElementKind,
}

/// Maximum number of colour-space array elements scanned before bailing.
const MAX_ARRAY_ELEMENTS: usize = 64;

/// Scan the shallow elements of a `[ … ]` array at its opening offset.
///
/// Uses the balanced array extent to bound the scan, then walks significant
/// tokens by first byte. Nested arrays and dictionaries are treated as opaque
/// balanced spans; indirect references (`N G R`) are recognized as one element.
fn array_elements(
    input: &[u8],
    open_offset: usize,
) -> Result<Vec<ArrayElement>, SkippedColorSpaceResourceReason> {
    let extent = inspect_array_extent(input, open_offset)
        .map_err(|_| SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
    let limit = extent.close_byte_offset;
    let mut cursor = extent.open_byte_offset + 1;
    let mut elements = Vec::new();
    while elements.len() < MAX_ARRAY_ELEMENTS {
        cursor = skip_whitespace_and_comments(input, cursor, limit);
        if cursor >= limit {
            break;
        }
        let byte = input[cursor];
        let element = if byte == b'/' {
            let end = skip_name(input, cursor, limit);
            ArrayElement {
                start: cursor,
                end,
                kind: ArrayElementKind::Name,
            }
        } else if byte == b'[' {
            let nested = inspect_array_extent(input, cursor)
                .map_err(|_| SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
            ArrayElement {
                start: cursor,
                end: nested.after_close_byte_offset,
                kind: ArrayElementKind::Array,
            }
        } else if byte == b'<' || byte == b'(' {
            // A dictionary, hex string, or literal string operand: opaque for the
            // shapes this classifier models. Skip via a balanced/quoted scan.
            let end = skip_opaque(input, cursor, limit)
                .ok_or(SkippedColorSpaceResourceReason::MalformedColorSpaceOperand)?;
            ArrayElement {
                start: cursor,
                end,
                kind: ArrayElementKind::Other,
            }
        } else if let Some(element) = try_reference(input, cursor, limit) {
            element
        } else {
            let end = skip_scalar_token(input, cursor, limit);
            ArrayElement {
                start: cursor,
                end,
                kind: ArrayElementKind::Other,
            }
        };
        cursor = element.end;
        elements.push(element);
    }
    Ok(elements)
}

/// Try to parse an `N G R` indirect reference element at `cursor`.
fn try_reference(input: &[u8], cursor: usize, limit: usize) -> Option<ArrayElement> {
    if !input.get(cursor).is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let reference = parse_indirect_reference(input, cursor).ok()?;
    let end = reference.reference_range.end.min(limit);
    Some(ArrayElement {
        start: cursor,
        end,
        kind: ArrayElementKind::Reference(reference.reference),
    })
}

/// Skip a dictionary, hex string, or literal string opaque span.
fn skip_opaque(input: &[u8], cursor: usize, limit: usize) -> Option<usize> {
    match input.get(cursor)? {
        b'(' => crate::source_utils::skip_literal_string(input, cursor).map(|end| end.min(limit)),
        b'<' if input.get(cursor + 1) == Some(&b'<') => {
            crate::inspect_dictionary_extent(input, cursor)
                .ok()
                .map(|extent| extent.after_close_byte_offset.min(limit))
        }
        b'<' => crate::source_utils::skip_hex_string(input, cursor).map(|end| end.min(limit)),
        _ => None,
    }
}
