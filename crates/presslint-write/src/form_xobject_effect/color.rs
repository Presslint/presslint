//! Form-local Device colour authority, projection, and walk validation.
//!
//! This private submodule holds the cohesive T188 implementation moved
//! mechanically out of the parent analyzer: the decoded-name [`ColorAuthority`]
//! projected from one Form's OWN `/Resources /ColorSpace` facts (never page
//! fallback), the ephemeral [`ColorSpaceResource`] projection handed to the
//! single seeded walk, and the exact `CS`/`cs` selection and
//! `SC`/`SCN`/`sc`/`scn` setter validation that walk applies. Only small
//! `pub(super)` seams are exposed; the parent keeps analyzer orchestration,
//! stream/filter/budget checks, the raw grammar, the paint walk, and the
//! generic semantic-name/dictionary helpers.

use std::collections::{BTreeMap, BTreeSet};

use presslint_paint::{ColorSpaceEnv, ColorSpaceResource, DecodedRange, GraphicsColor};
use presslint_pdf::{
    ColorSpaceFamily, FormColorSpaceResourcesInspection, ObjectLookup,
    SkippedColorSpaceResourceReason, inspect_form_color_space_resources,
};
use presslint_syntax::OperatorRecord;
use presslint_types::{ColorSpace, PdfName};

use super::malformed_name_may_hide;
use crate::page_xobject_policy::decode_pdf_name;

/// Fixed cap on reported Form-local colour-space plus skip facts consulted for
/// one invocation's decoded-name authority. Beyond this, the whole projection is
/// poisoned Unknown before any writer-local map is built (excess is Unknown).
const MAX_COLOR_FACTS: usize = 256;

/// Fixed cap on distinct raw `CS`/`cs` operand spellings admitted to one
/// ephemeral [`ColorSpaceEnv`]. This separately bounds the projection even when
/// many escaped spellings decode to one semantic resource name.
const MAX_COLOR_OPERAND_SPELLINGS: usize = 256;

/// Return the family of a supported local Device colour: a
/// [`ColorSpace::DeviceGray`]/`DeviceRgb`/`DeviceCmyk` value selected through the
/// Form-local projection (so it carries a resource name). The inherited
/// sentinels and direct-device setters both carry no resource name and are
/// therefore never a local Device lane, so a named setter can never reinterpret
/// the artificial CMYK-shaped inherited sentinel as an admissible CMYK lane.
const fn supported_local_device_kind(color: &GraphicsColor) -> Option<DeviceKind> {
    if color.resource_name.is_none()
        || color.source.is_none()
        || color.spot_name.is_some()
        || !color.spot_names.is_empty()
    {
        return None;
    }
    DeviceKind::from_space(&color.space)
}

/// Corroborate the complete post-`CS`/`cs` lane against the exact projected
/// resource selected by this raw operand spelling. Family, spelling, ISO initial
/// components, and local source must all agree.
pub(super) fn valid_color_space_selection(
    post: &GraphicsColor,
    env: ColorSpaceEnv<'_>,
    record: &OperatorRecord,
    source: &[u8],
) -> bool {
    let Some(raw_name) = color_space_operand_name(record, source) else {
        return false;
    };
    let Some(name) = post.resource_name.as_ref() else {
        return false;
    };
    if name.0 != raw_name {
        return false;
    }
    let Some(resource) = env.resolve(name) else {
        return false;
    };
    let Some(kind) = DeviceKind::from_space(&resource.space) else {
        return false;
    };
    resource.component_count == Some(kind.arity())
        && resource.spot_names.is_empty()
        && post.space == resource.space
        && post.components.as_slice() == kind.initial_components()
        && post.spot_name.is_none()
        && post.spot_names.is_empty()
        && post.source == Some(DecodedRange::new(record.range))
}

/// Whether a named setter is admissible: the prior lane was a supported local
/// Device lane, not the source-less inherited sentinel, and the setter preserved
/// that lane's family/name while stamping its own exact source range and arity.
pub(super) fn valid_named_setter(
    prior: &GraphicsColor,
    post: &GraphicsColor,
    inherited: &GraphicsColor,
    record: &OperatorRecord,
) -> bool {
    if prior == inherited || prior.source.is_none() {
        return false;
    }
    let Some(kind) = supported_local_device_kind(prior) else {
        return false;
    };
    post.space == prior.space
        && post.resource_name == prior.resource_name
        && post.spot_name == prior.spot_name
        && post.spot_names == prior.spot_names
        && post.components.len() == kind.arity()
        && post.source == Some(DecodedRange::new(record.range))
}

/// Whether one record's operator token is `CS` or `cs`.
pub(super) fn is_color_space_operator(record: &OperatorRecord, source: &[u8]) -> bool {
    matches!(
        source.get(record.operator.range.start..record.operator.range.end),
        Some(b"CS" | b"cs")
    )
}

/// The raw operand spelling (without the leading slash) of a `CS`/`cs` record,
/// or `None` when the sole operand is not a well-formed single name.
fn color_space_operand_name<'a>(record: &OperatorRecord, source: &'a [u8]) -> Option<&'a [u8]> {
    let [operand] = record.operands.as_slice() else {
        return None;
    };
    source
        .get(operand.range.start..operand.range.end)
        .and_then(|bytes| bytes.strip_prefix(b"/"))
        .filter(|name| !name.is_empty())
}

/// Build the Form-local ephemeral device colour environment for one paint walk.
///
/// The decoded-name authority is derived once from
/// [`inspect_form_color_space_resources`] (never page fallback). Each DISTINCT
/// `CS`/`cs` operand spelling actually used is resolved through that authority;
/// only a proven supported Device family produces a [`ColorSpaceResource`] whose
/// raw name matches the operand spelling for the walker's raw-name lookup. Any
/// unresolved spelling is simply absent, so the walk observes a `Resource(name)`
/// lane and refuses the Form.
pub(super) fn build_device_projection(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reached_offset: usize,
    records: &[OperatorRecord],
    decoded: &[u8],
) -> Option<Vec<ColorSpaceResource>> {
    let authority = ColorAuthority::from_inspection(&inspect_form_color_space_resources(
        input,
        lookup,
        reached_offset,
    ))?;
    let mut resources = Vec::new();
    let mut seen: BTreeSet<&[u8]> = BTreeSet::new();
    for record in records {
        if !is_color_space_operator(record, decoded) {
            continue;
        }
        let Some(raw) = color_space_operand_name(record, decoded) else {
            continue;
        };
        if seen.contains(raw) {
            continue;
        }
        if seen.len() == MAX_COLOR_OPERAND_SPELLINGS {
            return None;
        }
        seen.insert(raw);
        if let Some(kind) = authority.resolve(raw) {
            resources.push(ColorSpaceResource {
                name: PdfName(raw.to_vec()),
                space: kind.color_space(),
                component_count: Some(kind.arity()),
                spot_names: Vec::new(),
            });
        }
    }
    Some(resources)
}

/// A supported Form-local Device colour family.
#[derive(Clone, Copy)]
enum DeviceKind {
    Gray,
    Rgb,
    Cmyk,
}

impl DeviceKind {
    /// Family index into a `[gray, rgb, cmyk]` lane array.
    const fn index(self) -> usize {
        match self {
            Self::Gray => 0,
            Self::Rgb => 1,
            Self::Cmyk => 2,
        }
    }

    /// The inventory colour space this family selects.
    const fn color_space(self) -> ColorSpace {
        match self {
            Self::Gray => ColorSpace::DeviceGray,
            Self::Rgb => ColorSpace::DeviceRgb,
            Self::Cmyk => ColorSpace::DeviceCmyk,
        }
    }

    /// Exact component count of this family.
    const fn arity(self) -> usize {
        match self {
            Self::Gray => 1,
            Self::Rgb => 3,
            Self::Cmyk => 4,
        }
    }

    /// The reserved direct operand name of this family (`DeviceGray`, …). These
    /// built-in selectors cannot be shadowed by same-named resource keys.
    fn from_reserved(name: &[u8]) -> Option<Self> {
        match name {
            b"DeviceGray" => Some(Self::Gray),
            b"DeviceRGB" => Some(Self::Rgb),
            b"DeviceCMYK" => Some(Self::Cmyk),
            _ => None,
        }
    }

    /// The Device family of one classified resource, or `None` for every
    /// unsupported family (ICC/Cal/Lab/Indexed/Separation/DeviceN/Pattern).
    const fn from_family(family: ColorSpaceFamily) -> Option<Self> {
        match family {
            ColorSpaceFamily::DeviceGray => Some(Self::Gray),
            ColorSpaceFamily::DeviceRgb => Some(Self::Rgb),
            ColorSpaceFamily::DeviceCmyk => Some(Self::Cmyk),
            _ => None,
        }
    }

    /// Device kind represented by one paint-layer colour space.
    const fn from_space(space: &ColorSpace) -> Option<Self> {
        match space {
            ColorSpace::DeviceGray => Some(Self::Gray),
            ColorSpace::DeviceRgb => Some(Self::Rgb),
            ColorSpace::DeviceCmyk => Some(Self::Cmyk),
            _ => None,
        }
    }

    /// Exact ISO initial components established by `CS`/`cs`.
    const fn initial_components(self) -> &'static [f64] {
        match self {
            Self::Gray => &[0.0],
            Self::Rgb => &[0.0, 0.0, 0.0],
            Self::Cmyk => &[0.0, 0.0, 0.0, 1.0],
        }
    }
}

/// The relevant `/Default*` lane index of a decoded name, when it is one.
fn default_index(name: &[u8]) -> Option<usize> {
    match name {
        b"DefaultGray" => Some(0),
        b"DefaultRGB" => Some(1),
        b"DefaultCMYK" => Some(2),
        _ => None,
    }
}

/// Analyzer-private decoded-name authority over one Form's own
/// `/Resources /ColorSpace` facts. It is a scoped interpreter input built once
/// per invocation, not a second page policy: `PageXObjectPolicy` remains the sole
/// page XObject-name authority.
struct ColorAuthority {
    /// Decoded alias name -> supported Device family. Semantic duplicates and
    /// named skips are already removed, so a present entry is unambiguous.
    aliases: BTreeMap<Vec<u8>, DeviceKind>,
    /// Whether each `[gray, rgb, cmyk]` family's `/Default*` binding is proven
    /// absent; presence, a skip, or uncertainty leaves it `false`.
    default_absent: [bool; 3],
    /// Literal spellings of undecodable classified/skipped resource names.
    /// A decoded operand equal to one of these spellings is poisoned, while an
    /// unrelated malformed name remains isolated.
    literal_poison: BTreeSet<Vec<u8>>,
    /// A nameless uncertain skip poisons every resource-name and reserved
    /// selection to Unknown.
    poison_all: bool,
}

impl ColorAuthority {
    /// Project one Form's classified `/Resources /ColorSpace` inspection into the
    /// decoded-name authority. Return `None` before allocating authority maps
    /// when the consulted fact count exceeds the fixed cap.
    fn from_inspection(inspection: &FormColorSpaceResourcesInspection) -> Option<Self> {
        if inspection.color_spaces.len() + inspection.skipped.len() > MAX_COLOR_FACTS {
            return None;
        }
        let mut aliases: BTreeMap<Vec<u8>, DeviceKind> = BTreeMap::new();
        let mut seen: BTreeSet<Vec<u8>> = BTreeSet::new();
        let mut default_absent = [true; 3];
        let mut literal_poison = BTreeSet::new();
        let mut poison_all = false;

        for resource in &inspection.color_spaces {
            let Some(decoded) = decode_pdf_name(&resource.name.0) else {
                poison_possible_default(&resource.name.0, &mut default_absent);
                literal_poison.insert(resource.name.0.clone());
                continue;
            };
            let name = decoded.into_owned();
            if let Some(index) = default_index(&name) {
                default_absent[index] = false;
            } else if DeviceKind::from_reserved(&name).is_some() {
                // A reserved-named key cannot shadow the built-in selector.
            } else if !seen.insert(name.clone()) {
                // A semantic duplicate poisons the name for this invocation.
                aliases.remove(&name);
            } else if let Some(kind) = DeviceKind::from_family(resource.family) {
                aliases.insert(name, kind);
            }
        }

        for skip in &inspection.skipped {
            match skip.reason {
                // A proven-absent `/ColorSpace` (or `/Resources`) is not
                // uncertainty: it confirms every `/Default*` absent and poisons
                // no name. An alias selection still finds no resource key.
                SkippedColorSpaceResourceReason::MissingColorSpaceResources
                | SkippedColorSpaceResourceReason::MissingColorSpace => continue,
                _ => {}
            }
            match &skip.resource_name {
                None => poison_all = true,
                Some(name) => {
                    if let Some(decoded) = decode_pdf_name(&name.0) {
                        let name = decoded.as_ref();
                        if let Some(index) = default_index(name) {
                            default_absent[index] = false;
                        } else {
                            aliases.remove(name);
                        }
                    } else {
                        poison_possible_default(&name.0, &mut default_absent);
                        literal_poison.insert(name.0.clone());
                    }
                }
            }
        }

        Some(Self {
            aliases,
            default_absent,
            literal_poison,
            poison_all,
        })
    }

    /// Resolve one raw `CS`/`cs` operand spelling to a supported Device family.
    ///
    /// Reserved direct names win first and cannot be shadowed; every other
    /// spelling must decode to a unique, unpoisoned Device alias. Either way the
    /// matching family's `/Default*` binding must be proven absent.
    fn resolve(&self, raw: &[u8]) -> Option<DeviceKind> {
        let decoded = decode_pdf_name(raw)?;
        let name = decoded.as_ref();
        if self.literal_poison.contains(name) {
            return None;
        }
        if let Some(kind) = DeviceKind::from_reserved(name) {
            return (!self.poison_all && self.default_absent[kind.index()]).then_some(kind);
        }
        if self.poison_all {
            return None;
        }
        let kind = *self.aliases.get(name)?;
        self.default_absent[kind.index()].then_some(kind)
    }
}

/// A malformed resource name poisons only the `/Default*` families whose known
/// decoded prefix it could hide. Thus `/Default#GG...` is uncertainty for the
/// matching candidates, while an unrelated `/Other#GG` remains isolated.
fn poison_possible_default(raw: &[u8], default_absent: &mut [bool; 3]) {
    for (index, candidate) in [b"DefaultGray".as_slice(), b"DefaultRGB", b"DefaultCMYK"]
        .into_iter()
        .enumerate()
    {
        if malformed_name_may_hide(raw, candidate) {
            default_absent[index] = false;
        }
    }
}
