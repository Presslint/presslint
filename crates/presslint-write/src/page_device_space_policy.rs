//! Page device-space policy: exact page device-colour alias facts plus
//! per-family `/Default*` colour-space status.
//!
//! This is the ONE abstraction the direct converter consults before trusting
//! raw device bytes. It is built once per analysed page from the two advisory
//! structural inspections (`/Resources /ColorSpace` entries and `/DefaultGray`
//! / `/DefaultRGB` / `/DefaultCMYK` facts), matched to the content page by
//! EXACT leaf page reference — never by compacted report vector position,
//! because the inspectors may omit failed leaves.
//!
//! The policy answers three questions, all read-only:
//!
//! - which page colour-space resource names are exact DeviceGray/RGB/CMYK
//!   aliases safe to resolve in the paint walk ([`ColorSpaceEnv`]);
//! - whether an exact numeric `sc`/`SC`/`scn`/`SCN` setter under such an alias
//!   is structurally eligible for a future closed conversion epoch;
//! - whether a routed direct source/destination device family is RAW-device
//!   safe: its matching `/Default*` is proven `Absent` or `Identity`.
//!
//! Everything is FAIL-CLOSED: a failed inspection, a missing/duplicate/
//! inconsistent page match, a general `/Resources` failure, or a
//! family-specific malformed default degrades to `Unknown`, which refuses
//! direct conversion for that family and keeps the alias out of the paint
//! environment. A genuine absence of effective `/Resources` or of the matching
//! `/Default*` key is safe (`Absent`): defaults can exist only inside an
//! effective `/Resources /ColorSpace` dictionary.

use std::collections::BTreeMap;

use presslint_paint::{ColorSpaceEnv, ColorSpaceResource, GraphicsColor};
use presslint_pdf::{
    ColorSpaceFamily, DefaultColorSpaceKind, DocumentPageColorSpaceResourcesInspection,
    DocumentPageDefaultColorSpacesInspection, IndirectRef, PageColorSpaceResourcesInspection,
    PageDefaultColorSpacesInspection, SkippedColorSpaceResourceReason,
    SkippedDefaultColorSpaceReason,
};
use presslint_syntax::{OperandRecord, OperatorRecord, Token, TokenKind};
use presslint_types::{ColorSpace, PdfName};

use crate::content_color_convert::DeviceColorSpace;

/// Per-family effective `/Default*` colour-space status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefaultStatus {
    /// A trustworthy effective resource environment has no matching default
    /// (including the genuinely missing-`/Resources` case).
    Absent,
    /// A present matching default classifies to the SAME device family.
    Identity,
    /// A present matching default classifies to a DIFFERENT family.
    Replaced,
    /// The resource/default inspection cannot prove the family; fail-closed.
    Unknown,
}

impl DefaultStatus {
    /// Whether raw device bytes of this family are honest to read and emit.
    #[must_use]
    pub const fn is_raw_device(self) -> bool {
        matches!(self, Self::Absent | Self::Identity)
    }
}

/// One name-sorted page device-alias fact retained for setter classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasDecision {
    /// Alias resource name (without the leading slash).
    pub name: PdfName,
    /// The alias's OWN classified terminal device family.
    pub space: DeviceColorSpace,
    /// Whether the alias entered the paint environment (exact component count
    /// and a raw-device-safe family status).
    pub eligible: bool,
}

/// Structural classification of one exact alias colour setter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasSetterClass {
    /// Every structural requirement for a future closed alias epoch holds.
    Eligible,
    /// The setter sits under a classified device alias but fails a structural
    /// requirement; it can never enter a closed epoch as written.
    Ineligible,
}

/// One `sc`/`SC`/`scn`/`SCN` paint event offered for alias classification.
pub struct AliasSetterEvent<'a> {
    /// Exact operator bytes at the event's operator range.
    pub operator: &'a [u8],
    /// Whether the event set the stroking colour.
    pub stroking: bool,
    /// Resource name selected on this side immediately before the setter.
    ///
    /// This comes from the existing pre-operator paint-state snapshot rather
    /// than the setter's resulting colour: `scn`/`SCN` may carry a distinct
    /// trailing Pattern name that replaces `GraphicsColor::resource_name`.
    pub selected_resource_name: Option<&'a PdfName>,
    /// The colour the setter established in graphics state.
    pub color: &'a GraphicsColor,
    /// The setter's assembled operator record.
    pub record: &'a OperatorRecord,
    /// The page sequence's token slice backing `record`.
    pub tokens: &'a [Token],
    /// Whether the event's record range localizes wholly to one physical
    /// occurrence of the page's `/Contents`.
    pub localized: bool,
}

/// Exact page device-alias facts plus per-family `/Default*` status.
pub struct PageDeviceSpacePolicy {
    /// Aliases safe to resolve in the paint walk, in report (name) order.
    paint_resources: Vec<ColorSpaceResource>,
    /// Name-sorted device-alias facts, including ineligible ones.
    alias_decisions: Vec<AliasDecision>,
    /// Per-family default status, indexed Gray / Rgb / Cmyk.
    defaults: [DefaultStatus; 3],
}

impl PageDeviceSpacePolicy {
    /// Build the policy from one page's advisory matched inspection facts.
    ///
    /// A `None` fact (failed inspection or failed page match) degrades to an
    /// empty alias environment and all-`Unknown` default statuses.
    #[must_use]
    pub fn from_page_facts(facts: &PageColorFacts<'_>) -> Self {
        let defaults = default_statuses(facts.defaults);
        let (paint_resources, alias_decisions) = alias_facts(facts.color_spaces, defaults);
        Self {
            paint_resources,
            alias_decisions,
            defaults,
        }
    }

    /// Borrow the page-alias paint environment for the single paint walk.
    #[must_use]
    pub fn color_space_env(&self) -> ColorSpaceEnv<'_> {
        ColorSpaceEnv::new(&self.paint_resources)
    }

    /// The effective `/Default*` status of one device family.
    #[must_use]
    pub const fn default_status(&self, space: DeviceColorSpace) -> DefaultStatus {
        self.defaults[family_index(space)]
    }

    /// Whether a routed direct conversion may trust raw device bytes: both the
    /// source and the emitted destination family must be proven `Absent` or
    /// `Identity`. A same-family link naturally checks one status.
    #[must_use]
    pub const fn route_is_raw_device(
        &self,
        source: DeviceColorSpace,
        destination: DeviceColorSpace,
    ) -> bool {
        self.default_status(source).is_raw_device()
            && self.default_status(destination).is_raw_device()
    }

    /// Classify one exact `sc`/`SC`/`scn`/`SCN` event under a page alias.
    ///
    /// Returns `None` when the event is not an alias setter at all: wrong
    /// operator bytes, no selecting resource name, or a name that is not a
    /// classified page device alias (built-in names, `/Default*` keys, and
    /// non-device families never enter the decisions). Such events stay
    /// byte-verbatim and uncounted, exactly as before.
    #[must_use]
    pub fn classify_alias_setter(&self, event: &AliasSetterEvent<'_>) -> Option<AliasSetterClass> {
        let uppercase = match event.operator {
            b"sc" | b"scn" => false,
            b"SC" | b"SCN" => true,
            _ => return None,
        };
        let name = event.selected_resource_name?;
        let decision = self
            .alias_decisions
            .iter()
            .find(|decision| &decision.name == name)?;
        let expected = family_component_count(decision.space);
        let structurally_eligible = decision.eligible
            && uppercase == event.stroking
            && event.color.space == paint_color_space(decision.space)
            && event.color.components.len() == expected
            && event.record.operands.len() == expected
            && event
                .record
                .operands
                .iter()
                .all(|operand| operand_is_single_number(operand, event.tokens))
            && event
                .color
                .components
                .iter()
                .all(|value| value.is_finite() && (0.0..=1.0).contains(value))
            && event.localized;
        Some(if structurally_eligible {
            AliasSetterClass::Eligible
        } else {
            AliasSetterClass::Ineligible
        })
    }
}

/// One page's advisory matched colour resource / default inspection facts.
///
/// `None` means UNKNOWN (failed inspection or failed identity match), never
/// "no resources": genuine absence is reported inside a present report.
pub struct PageColorFacts<'a> {
    /// Matched page `/Resources /ColorSpace` classification report.
    pub color_spaces: Option<&'a PageColorSpaceResourcesInspection>,
    /// Matched page `/Default*` colour-space report.
    pub defaults: Option<&'a PageDefaultColorSpacesInspection>,
}

/// Deterministic per-request join from leaf page references to report pages.
///
/// Built once per conversion request from the two advisory document
/// inspections. Lookups are ordered (`BTreeMap`) by exact [`IndirectRef`]; a
/// reference reported more than once is poisoned to a failed match.
pub struct PageColorFactsIndex<'a> {
    color_spaces: BTreeMap<IndirectRef, Option<&'a PageColorSpaceResourcesInspection>>,
    defaults: BTreeMap<IndirectRef, Option<&'a PageDefaultColorSpacesInspection>>,
}

impl<'a> PageColorFactsIndex<'a> {
    /// Index both advisory document reports by exact page reference.
    #[must_use]
    pub fn new(
        color_spaces: Option<&'a DocumentPageColorSpaceResourcesInspection>,
        defaults: Option<&'a DocumentPageDefaultColorSpacesInspection>,
    ) -> Self {
        Self {
            color_spaces: color_spaces.map_or_else(BTreeMap::new, |document| {
                index_by_reference(&document.pages, |page| page.page_reference)
            }),
            defaults: defaults.map_or_else(BTreeMap::new, |document| {
                index_by_reference(&document.pages, |page| page.page_reference)
            }),
        }
    }

    /// Match one content page's leaf identity to its advisory report pages.
    ///
    /// Each report is matched independently by exact reference and corroborated
    /// by page object byte offset and document ordinal. A missing, duplicate,
    /// or inconsistent match yields `None` for that report (fail-closed).
    #[must_use]
    pub fn facts_for(
        &self,
        reference: IndirectRef,
        object_byte_offset: usize,
        ordinal: usize,
    ) -> PageColorFacts<'a> {
        PageColorFacts {
            color_spaces: matched_page(
                &self.color_spaces,
                reference,
                object_byte_offset,
                ordinal,
                |page| (page.page_object_byte_offset, page.ordinal),
            ),
            defaults: matched_page(
                &self.defaults,
                reference,
                object_byte_offset,
                ordinal,
                |page| (page.page_object_byte_offset, page.ordinal),
            ),
        }
    }
}

/// Index report pages by reference; a repeated reference poisons its slot.
fn index_by_reference<T>(
    pages: &[T],
    reference_of: impl Fn(&T) -> IndirectRef,
) -> BTreeMap<IndirectRef, Option<&T>> {
    let mut slots = BTreeMap::new();
    for page in pages {
        slots
            .entry(reference_of(page))
            .and_modify(|slot| *slot = None)
            .or_insert(Some(page));
    }
    slots
}

/// Resolve one uniquely matched, identity-corroborated report page.
fn matched_page<'a, T>(
    slots: &BTreeMap<IndirectRef, Option<&'a T>>,
    reference: IndirectRef,
    object_byte_offset: usize,
    ordinal: usize,
    identity_of: impl Fn(&T) -> (usize, usize),
) -> Option<&'a T> {
    let page = slots.get(&reference).copied().flatten()?;
    (identity_of(page) == (object_byte_offset, ordinal)).then_some(page)
}

/// Resource-dictionary names that may never act as page device aliases: the
/// built-in device/pattern selectors, their inline-image abbreviations, and
/// the `/Default*` policy keys.
const RESERVED_ALIAS_NAMES: [&[u8]; 11] = [
    b"CMYK",
    b"DefaultCMYK",
    b"DefaultGray",
    b"DefaultRGB",
    b"DeviceCMYK",
    b"DeviceGray",
    b"DeviceRGB",
    b"G",
    b"I",
    b"Pattern",
    b"RGB",
];

/// Derive the per-family default statuses from one matched defaults report.
fn default_statuses(report: Option<&PageDefaultColorSpacesInspection>) -> [DefaultStatus; 3] {
    let Some(report) = report else {
        return [DefaultStatus::Unknown; 3];
    };
    // A general /Resources failure, or a kind-less /ColorSpace failure that
    // hides whether defaults exist, poisons ALL families and takes precedence
    // over a simultaneous MissingResources fact.
    let all_poisoned = report.skipped.iter().any(|skip| match &skip.reason {
        SkippedDefaultColorSpaceReason::Resources { .. } => true,
        SkippedDefaultColorSpaceReason::ColorSpace { .. } => skip.kind.is_none(),
        SkippedDefaultColorSpaceReason::MissingResources
        | SkippedDefaultColorSpaceReason::DuplicateDefault { .. } => false,
    });
    if all_poisoned {
        return [DefaultStatus::Unknown; 3];
    }
    // MissingResources ALONE proves absence: defaults can exist only inside an
    // effective /Resources /ColorSpace, so every family starts Absent.
    let mut statuses = [DefaultStatus::Absent; 3];
    for fact in &report.defaults {
        statuses[default_kind_index(fact.kind)] =
            if fact.color_space.family == identity_family(fact.kind) {
                DefaultStatus::Identity
            } else {
                DefaultStatus::Replaced
            };
    }
    // Family-specific malformed/duplicate/unresolved/unclassified defaults
    // poison ONLY their family, and win over any classified fact.
    for skip in &report.skipped {
        if let Some(kind) = skip.kind {
            statuses[default_kind_index(kind)] = DefaultStatus::Unknown;
        }
    }
    statuses
}

/// Derive the paint environment and the name-sorted alias decisions.
///
/// Only aliases whose OWN classified terminal family is exactly
/// DeviceGray/RGB/CMYK become decisions; the paint environment additionally
/// requires the exact component count and a raw-device-safe family status.
/// Alternate/base/ICC metadata never narrows a special space to a device one.
fn alias_facts(
    report: Option<&PageColorSpaceResourcesInspection>,
    defaults: [DefaultStatus; 3],
) -> (Vec<ColorSpaceResource>, Vec<AliasDecision>) {
    let Some(report) = report else {
        return (Vec::new(), Vec::new());
    };
    let mut paint_resources = Vec::new();
    let mut alias_decisions = Vec::new();
    for resource in &report.color_spaces {
        if RESERVED_ALIAS_NAMES.contains(&resource.name.0.as_slice()) {
            continue;
        }
        let Some(space) = device_family(resource.family) else {
            continue;
        };
        let count = family_component_count(space);
        let duplicated = report.skipped.iter().any(|skip| {
            skip.resource_name.as_ref() == Some(&resource.name)
                && matches!(
                    &skip.reason,
                    SkippedColorSpaceResourceReason::DuplicateColorSpaceName { .. }
                )
        });
        let eligible = !duplicated
            && resource.component_count == Some(count)
            && defaults[family_index(space)].is_raw_device();
        // The one deliberate copy: report names cross from the PDF name type
        // into the paint name type once per selected page.
        let name = PdfName(resource.name.0.clone());
        if eligible {
            paint_resources.push(ColorSpaceResource {
                name: name.clone(),
                space: paint_color_space(space),
                component_count: Some(count),
                spot_names: Vec::new(),
            });
        }
        alias_decisions.push(AliasDecision {
            name,
            space,
            eligible,
        });
    }
    (paint_resources, alias_decisions)
}

/// Whether one operand is exactly one numeric token (no composite operand and
/// no trailing name/Pattern operand can ever satisfy this).
fn operand_is_single_number(operand: &OperandRecord, tokens: &[Token]) -> bool {
    let [token_ref] = operand.tokens.as_slice() else {
        return false;
    };
    tokens
        .get(token_ref.token_index)
        .is_some_and(|token| matches!(token.kind, TokenKind::Number(_)))
}

/// Narrow a classified structural family to a direct device space.
const fn device_family(family: ColorSpaceFamily) -> Option<DeviceColorSpace> {
    match family {
        ColorSpaceFamily::DeviceGray => Some(DeviceColorSpace::Gray),
        ColorSpaceFamily::DeviceRgb => Some(DeviceColorSpace::Rgb),
        ColorSpaceFamily::DeviceCmyk => Some(DeviceColorSpace::Cmyk),
        ColorSpaceFamily::IccBased
        | ColorSpaceFamily::Separation
        | ColorSpaceFamily::DeviceN
        | ColorSpaceFamily::Indexed => None,
    }
}

/// Fixed per-family status index: Gray, Rgb, Cmyk.
const fn family_index(space: DeviceColorSpace) -> usize {
    match space {
        DeviceColorSpace::Gray => 0,
        DeviceColorSpace::Rgb => 1,
        DeviceColorSpace::Cmyk => 2,
    }
}

/// Exact per-family component count.
const fn family_component_count(space: DeviceColorSpace) -> usize {
    match space {
        DeviceColorSpace::Gray => 1,
        DeviceColorSpace::Rgb => 3,
        DeviceColorSpace::Cmyk => 4,
    }
}

/// The paint colour space a device alias resolves to.
const fn paint_color_space(space: DeviceColorSpace) -> ColorSpace {
    match space {
        DeviceColorSpace::Gray => ColorSpace::DeviceGray,
        DeviceColorSpace::Rgb => ColorSpace::DeviceRgb,
        DeviceColorSpace::Cmyk => ColorSpace::DeviceCmyk,
    }
}

/// Fixed per-default-kind status index: Gray, Rgb, Cmyk.
const fn default_kind_index(kind: DefaultColorSpaceKind) -> usize {
    match kind {
        DefaultColorSpaceKind::DefaultGray => 0,
        DefaultColorSpaceKind::DefaultRgb => 1,
        DefaultColorSpaceKind::DefaultCmyk => 2,
    }
}

/// The device family a `/Default*` key must classify to for `Identity`.
const fn identity_family(kind: DefaultColorSpaceKind) -> ColorSpaceFamily {
    match kind {
        DefaultColorSpaceKind::DefaultGray => ColorSpaceFamily::DeviceGray,
        DefaultColorSpaceKind::DefaultRgb => ColorSpaceFamily::DeviceRgb,
        DefaultColorSpaceKind::DefaultCmyk => ColorSpaceFamily::DeviceCmyk,
    }
}
