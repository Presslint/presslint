//! Unit matrix for the page device-space policy: report identity join, alias
//! mapping, `/Default*` status precedence, and exact setter classification.

use presslint_paint::{PaintOpKind, PaintProgram};
use presslint_pdf::{
    ClassifiedColorSpaceDefinition, ClassifiedColorSpaceResource, ColorSpaceFamily,
    DefaultColorSpaceFact, DefaultColorSpaceKind, DictionaryEntryByteRange, DictionaryValueKind,
    DocumentPageColorSpaceResourcesInspection, DocumentPageDefaultColorSpacesInspection,
    IndirectRef, PageColorSpaceResourcesInspection, PageDefaultColorSpacesInspection,
    PdfName as PdfObjectName, SkippedColorSpaceResource, SkippedColorSpaceResourceReason,
    SkippedDefaultColorSpace, SkippedDefaultColorSpaceReason, SkippedPageXObjectResourceReason,
};
use presslint_syntax::{assemble_operators, tokenize};
use presslint_types::PdfName;

use crate::content_color_convert::DeviceColorSpace;
use crate::page_device_space_policy::{
    AliasSetterClass, AliasSetterEvent, DefaultStatus, PageColorFacts, PageColorFactsIndex,
    PageDeviceSpacePolicy,
};

const PAGE_REF: IndirectRef = IndirectRef {
    object_number: 3,
    generation: 0,
};
const PAGE_OFFSET: usize = 500;

fn definition(family: ColorSpaceFamily, count: usize) -> ClassifiedColorSpaceDefinition {
    ClassifiedColorSpaceDefinition {
        family,
        component_count: Some(count),
        spot_names: Vec::new(),
        alternate_space: None,
        base_space: None,
        indexed_hival: None,
        indexed_lookup: None,
        icc_profile_stream: None,
        icc_range_entry_count: None,
        icc_alternate_present: None,
    }
}

fn resource(name: &str, family: ColorSpaceFamily, count: usize) -> ClassifiedColorSpaceResource {
    ClassifiedColorSpaceResource {
        name: PdfObjectName(name.as_bytes().to_vec()),
        family,
        component_count: Some(count),
        spot_names: Vec::new(),
        alternate_space: None,
        base_space: None,
        indexed_hival: None,
        indexed_lookup: None,
        icc_profile_stream: None,
        icc_range_entry_count: None,
        icc_alternate_present: None,
    }
}

fn color_report(
    color_spaces: Vec<ClassifiedColorSpaceResource>,
) -> PageColorSpaceResourcesInspection {
    PageColorSpaceResourcesInspection {
        ordinal: 0,
        page_reference: PAGE_REF,
        page_object_byte_offset: PAGE_OFFSET,
        color_spaces,
        skipped: Vec::new(),
    }
}

fn defaults_report(
    defaults: Vec<DefaultColorSpaceFact>,
    skipped: Vec<SkippedDefaultColorSpace>,
) -> PageDefaultColorSpacesInspection {
    PageDefaultColorSpacesInspection {
        ordinal: 0,
        page_reference: PAGE_REF,
        page_object_byte_offset: PAGE_OFFSET,
        defaults,
        skipped,
    }
}

fn default_fact(kind: DefaultColorSpaceKind, family: ColorSpaceFamily) -> DefaultColorSpaceFact {
    DefaultColorSpaceFact {
        kind,
        color_space: definition(family, 3),
    }
}

fn missing_resources() -> SkippedDefaultColorSpace {
    SkippedDefaultColorSpace {
        object_byte_offset: PAGE_OFFSET,
        kind: None,
        reason: SkippedDefaultColorSpaceReason::MissingResources,
    }
}

fn resources_failure() -> SkippedDefaultColorSpace {
    SkippedDefaultColorSpace {
        object_byte_offset: PAGE_OFFSET,
        kind: None,
        reason: SkippedDefaultColorSpaceReason::Resources {
            resources_reason: SkippedPageXObjectResourceReason::UnsupportedResourcesValue {
                value_kind: DictionaryValueKind::Name,
            },
        },
    }
}

fn family_failure(kind: DefaultColorSpaceKind) -> SkippedDefaultColorSpace {
    SkippedDefaultColorSpace {
        object_byte_offset: PAGE_OFFSET,
        kind: Some(kind),
        reason: SkippedDefaultColorSpaceReason::ColorSpace {
            color_space_reason: SkippedColorSpaceResourceReason::MalformedColorSpaceOperand,
        },
    }
}

fn duplicate_default(kind: DefaultColorSpaceKind) -> SkippedDefaultColorSpace {
    let range = DictionaryEntryByteRange { start: 10, end: 20 };
    SkippedDefaultColorSpace {
        object_byte_offset: PAGE_OFFSET,
        kind: Some(kind),
        reason: SkippedDefaultColorSpaceReason::DuplicateDefault {
            first_key_range: range,
            duplicate_key_range: range,
        },
    }
}

fn policy(
    color_spaces: Option<&PageColorSpaceResourcesInspection>,
    defaults: Option<&PageDefaultColorSpacesInspection>,
) -> PageDeviceSpacePolicy {
    PageDeviceSpacePolicy::from_page_facts(&PageColorFacts {
        color_spaces,
        defaults,
    })
}

fn statuses(policy: &PageDeviceSpacePolicy) -> [DefaultStatus; 3] {
    [
        policy.default_status(DeviceColorSpace::Gray),
        policy.default_status(DeviceColorSpace::Rgb),
        policy.default_status(DeviceColorSpace::Cmyk),
    ]
}

/// Classify every `sc`/`SC`/`scn`/`SCN` event of `stream` under `policy`.
fn classify_setters(
    policy: &PageDeviceSpacePolicy,
    stream: &[u8],
    localized: bool,
) -> Vec<Option<AliasSetterClass>> {
    let tokens = tokenize(stream).expect("tokenize");
    let records = assemble_operators(&tokens).expect("assemble").records;
    let program = PaintProgram::new(stream, &records, policy.color_space_env());
    let mut classes = Vec::new();
    let mut previous_state = None;
    for op in program.ops() {
        let op = op.expect("walk succeeds");
        let state_before = previous_state.replace(op.state.clone());
        let stroking = matches!(op.kind, PaintOpKind::SetStrokingColor { .. });
        let (PaintOpKind::SetStrokingColor { color } | PaintOpKind::SetNonstrokingColor { color }) =
            &op.kind
        else {
            continue;
        };
        let operator = &stream[op.operator_range.start()..op.operator_range.end()];
        if !matches!(operator, b"sc" | b"SC" | b"scn" | b"SCN") {
            continue;
        }
        classes.push(policy.classify_alias_setter(&AliasSetterEvent {
            operator,
            stroking,
            selected_resource_name: state_before.as_ref().and_then(|state| {
                if stroking {
                    state.stroking_color.resource_name.as_ref()
                } else {
                    state.nonstroking_color.resource_name.as_ref()
                }
            }),
            color,
            record: &records[op.index],
            tokens: &tokens,
            localized,
        }));
    }
    classes
}

fn absent_defaults() -> PageDefaultColorSpacesInspection {
    defaults_report(Vec::new(), vec![missing_resources()])
}

fn three_aliases() -> PageColorSpaceResourcesInspection {
    color_report(vec![
        resource("CmykAlias", ColorSpaceFamily::DeviceCmyk, 4),
        resource("GrayAlias", ColorSpaceFamily::DeviceGray, 1),
        resource("RgbAlias", ColorSpaceFamily::DeviceRgb, 3),
    ])
}

// --- Default status matrix ---------------------------------------------------

#[test]
fn missing_report_is_unknown_for_every_family() {
    let policy = policy(None, None);
    assert_eq!(statuses(&policy), [DefaultStatus::Unknown; 3]);
    assert!(!policy.route_is_raw_device(DeviceColorSpace::Rgb, DeviceColorSpace::Cmyk));
}

#[test]
fn missing_resources_alone_proves_absence() {
    let defaults = absent_defaults();
    let policy = policy(None, Some(&defaults));
    assert_eq!(statuses(&policy), [DefaultStatus::Absent; 3]);
    assert!(policy.route_is_raw_device(DeviceColorSpace::Rgb, DeviceColorSpace::Cmyk));
}

#[test]
fn empty_effective_color_space_dictionary_is_absent() {
    // A trustworthy resource environment with no matching default keys.
    let defaults = defaults_report(Vec::new(), Vec::new());
    let policy = policy(None, Some(&defaults));
    assert_eq!(statuses(&policy), [DefaultStatus::Absent; 3]);
}

#[test]
fn general_resources_failure_poisons_all_families_over_missing_resources() {
    let defaults = defaults_report(Vec::new(), vec![resources_failure(), missing_resources()]);
    let policy = policy(None, Some(&defaults));
    assert_eq!(statuses(&policy), [DefaultStatus::Unknown; 3]);
}

#[test]
fn kindless_color_space_failure_poisons_all_families() {
    let skip = SkippedDefaultColorSpace {
        object_byte_offset: PAGE_OFFSET,
        kind: None,
        reason: SkippedDefaultColorSpaceReason::ColorSpace {
            color_space_reason: SkippedColorSpaceResourceReason::MalformedColorSpaceOperand,
        },
    };
    let defaults = defaults_report(Vec::new(), vec![skip]);
    let policy = policy(None, Some(&defaults));
    assert_eq!(statuses(&policy), [DefaultStatus::Unknown; 3]);
}

#[test]
fn identity_default_classifies_to_the_same_family() {
    let defaults = defaults_report(
        vec![default_fact(
            DefaultColorSpaceKind::DefaultRgb,
            ColorSpaceFamily::DeviceRgb,
        )],
        Vec::new(),
    );
    let policy = policy(None, Some(&defaults));
    assert_eq!(
        statuses(&policy),
        [
            DefaultStatus::Absent,
            DefaultStatus::Identity,
            DefaultStatus::Absent
        ]
    );
    assert!(policy.route_is_raw_device(DeviceColorSpace::Rgb, DeviceColorSpace::Rgb));
}

#[test]
fn replaced_default_classifies_to_a_different_family() {
    for family in [ColorSpaceFamily::DeviceCmyk, ColorSpaceFamily::IccBased] {
        let defaults = defaults_report(
            vec![default_fact(DefaultColorSpaceKind::DefaultRgb, family)],
            Vec::new(),
        );
        let policy = policy(None, Some(&defaults));
        assert_eq!(
            policy.default_status(DeviceColorSpace::Rgb),
            DefaultStatus::Replaced
        );
        assert!(!policy.route_is_raw_device(DeviceColorSpace::Rgb, DeviceColorSpace::Cmyk));
        assert!(!policy.route_is_raw_device(DeviceColorSpace::Cmyk, DeviceColorSpace::Rgb));
        assert!(policy.route_is_raw_device(DeviceColorSpace::Gray, DeviceColorSpace::Cmyk));
    }
}

#[test]
fn family_specific_failure_poisons_only_that_family() {
    for skip in [
        family_failure(DefaultColorSpaceKind::DefaultCmyk),
        duplicate_default(DefaultColorSpaceKind::DefaultCmyk),
    ] {
        let defaults = defaults_report(Vec::new(), vec![skip]);
        let policy = policy(None, Some(&defaults));
        assert_eq!(
            statuses(&policy),
            [
                DefaultStatus::Absent,
                DefaultStatus::Absent,
                DefaultStatus::Unknown
            ]
        );
        assert!(!policy.route_is_raw_device(DeviceColorSpace::Rgb, DeviceColorSpace::Cmyk));
        assert!(policy.route_is_raw_device(DeviceColorSpace::Rgb, DeviceColorSpace::Rgb));
    }
}

// --- Alias mapping into the paint environment --------------------------------

#[test]
fn exact_device_aliases_enter_the_environment_when_family_is_safe() {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    let env = policy.color_space_env();
    for name in ["GrayAlias", "RgbAlias", "CmykAlias"] {
        assert!(
            env.resolve(&PdfName(name.as_bytes().to_vec())).is_some(),
            "{name} resolves"
        );
    }
}

#[test]
fn unsafe_family_alias_is_excluded_from_the_environment_but_still_ineligible() {
    let color_spaces = color_report(vec![resource("RgbAlias", ColorSpaceFamily::DeviceRgb, 3)]);
    let defaults = defaults_report(
        vec![default_fact(
            DefaultColorSpaceKind::DefaultRgb,
            ColorSpaceFamily::DeviceCmyk,
        )],
        Vec::new(),
    );
    let policy = policy(Some(&color_spaces), Some(&defaults));
    assert!(
        policy
            .color_space_env()
            .resolve(&PdfName(b"RgbAlias".to_vec()))
            .is_none()
    );
    // The setter under the excluded alias still classifies deterministically.
    assert_eq!(
        classify_setters(&policy, b"/RgbAlias cs 0 0.5 1 scn\n", true),
        vec![Some(AliasSetterClass::Ineligible)]
    );
}

#[test]
fn non_device_families_and_reserved_names_never_become_aliases() {
    let color_spaces = color_report(vec![
        resource("DefaultRGB", ColorSpaceFamily::DeviceRgb, 3),
        resource("DeviceRGB", ColorSpaceFamily::DeviceCmyk, 4),
        resource("Icc", ColorSpaceFamily::IccBased, 3),
        resource("Idx", ColorSpaceFamily::Indexed, 1),
        resource("Sep", ColorSpaceFamily::Separation, 1),
    ]);
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    for name in ["DefaultRGB", "DeviceRGB", "Icc", "Idx", "Sep"] {
        assert!(
            policy
                .color_space_env()
                .resolve(&PdfName(name.as_bytes().to_vec()))
                .is_none(),
            "{name} must not resolve"
        );
    }
    // Setters under them are not alias setters at all: uncounted.
    assert_eq!(
        classify_setters(&policy, b"/Icc cs 0 0 0 sc\n/Sep CS 1 SCN\n", true),
        vec![None, None]
    );
}

#[test]
fn wrong_shallow_component_count_never_yields_a_false_eligible() {
    let mut broken = resource("GrayAlias", ColorSpaceFamily::DeviceGray, 1);
    broken.component_count = None;
    let color_spaces = color_report(vec![broken]);
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    assert!(
        policy
            .color_space_env()
            .resolve(&PdfName(b"GrayAlias".to_vec()))
            .is_none()
    );
    assert_eq!(
        classify_setters(&policy, b"/GrayAlias cs 0.5 sc\n", true),
        vec![Some(AliasSetterClass::Ineligible)]
    );
}

#[test]
fn duplicate_device_alias_is_retained_only_as_ineligible() {
    let range = DictionaryEntryByteRange { start: 10, end: 20 };
    let mut color_spaces =
        color_report(vec![resource("GrayAlias", ColorSpaceFamily::DeviceGray, 1)]);
    color_spaces.skipped.push(SkippedColorSpaceResource {
        page_object_byte_offset: PAGE_OFFSET,
        resource_name: Some(PdfObjectName(b"GrayAlias".to_vec())),
        reason: SkippedColorSpaceResourceReason::DuplicateColorSpaceName {
            first_key_range: range,
            duplicate_key_range: range,
        },
    });
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));

    assert!(
        policy
            .color_space_env()
            .resolve(&PdfName(b"GrayAlias".to_vec()))
            .is_none()
    );
    assert_eq!(
        classify_setters(&policy, b"/GrayAlias cs 0.5 sc\n", true),
        vec![Some(AliasSetterClass::Ineligible)]
    );
}

// --- Exact setter classification ----------------------------------------------

#[test]
fn exact_numeric_setters_are_eligible_per_family_case_and_operator() {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    let stream = b"/GrayAlias cs 0.5 sc\n/GrayAlias CS 1 SC\n/RgbAlias cs 0 .5 1 scn\n/CmykAlias CS 0 0 0 1 SCN\n";
    assert_eq!(
        classify_setters(&policy, stream, true),
        vec![Some(AliasSetterClass::Eligible); 4]
    );
}

#[test]
fn setter_without_an_alias_selection_is_uncounted() {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    // Default DeviceGray state: no selecting resource name.
    assert_eq!(classify_setters(&policy, b"0.5 sc\n", true), vec![None]);
    // Unresolved selection name: honest coverage gap, uncounted.
    assert_eq!(
        classify_setters(&policy, b"/Nope cs 0.5 sc\n", true),
        vec![None]
    );
}

#[test]
fn malformed_setter_shapes_are_ineligible_never_falsely_eligible() {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    let cases: [(&[u8], &str); 4] = [
        (b"/GrayAlias cs 0.5 0.5 sc\n", "wrong component count"),
        (b"/GrayAlias cs 1.5 sc\n", "operand above 1"),
        (b"/GrayAlias cs -0.1 sc\n", "operand below 0"),
        (
            b"/GrayAlias cs 0.5 /PatternPaint scn\n",
            "distinct trailing Pattern name operand",
        ),
    ];
    for (stream, label) in cases {
        assert_eq!(
            classify_setters(&policy, stream, true),
            vec![Some(AliasSetterClass::Ineligible)],
            "{label}"
        );
    }
}

#[test]
fn setter_whose_record_does_not_localize_is_ineligible() {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    assert_eq!(
        classify_setters(&policy, b"/GrayAlias cs 0.5 sc\n", false),
        vec![Some(AliasSetterClass::Ineligible)]
    );
}

#[test]
fn restore_drops_the_alias_selection_read_only() {
    let color_spaces = three_aliases();
    let defaults = absent_defaults();
    let policy = policy(Some(&color_spaces), Some(&defaults));
    // After Q the colour reverts to the page-default DeviceGray (no name).
    assert_eq!(
        classify_setters(&policy, b"q /GrayAlias cs Q 0.5 sc\n", true),
        vec![None]
    );
    // Inside q..Q the selection is live.
    assert_eq!(
        classify_setters(&policy, b"/GrayAlias cs q 0.5 sc Q\n", true),
        vec![Some(AliasSetterClass::Eligible)]
    );
}

// --- Exact page-report identity join ------------------------------------------

fn color_document(
    pages: Vec<PageColorSpaceResourcesInspection>,
) -> DocumentPageColorSpaceResourcesInspection {
    DocumentPageColorSpaceResourcesInspection {
        byte_len: 1000,
        pages,
        page_tree_skipped: Vec::new(),
        visited_node_count: 1,
        truncated: None,
    }
}

fn defaults_document(
    pages: Vec<PageDefaultColorSpacesInspection>,
) -> DocumentPageDefaultColorSpacesInspection {
    DocumentPageDefaultColorSpacesInspection {
        byte_len: 1000,
        pages,
        page_tree_skipped: Vec::new(),
        visited_node_count: 1,
        truncated: None,
    }
}

#[test]
fn exact_reference_match_returns_both_reports() {
    let color = color_document(vec![color_report(Vec::new())]);
    let defaults = defaults_document(vec![absent_defaults()]);
    let index = PageColorFactsIndex::new(Some(&color), Some(&defaults));
    let facts = index.facts_for(PAGE_REF, PAGE_OFFSET, 0);
    assert!(facts.color_spaces.is_some());
    assert!(facts.defaults.is_some());
}

#[test]
fn missing_reference_and_failed_document_are_unknown() {
    let color = color_document(Vec::new());
    let index = PageColorFactsIndex::new(Some(&color), None);
    let facts = index.facts_for(PAGE_REF, PAGE_OFFSET, 0);
    assert!(facts.color_spaces.is_none());
    assert!(facts.defaults.is_none());
}

#[test]
fn duplicate_reference_in_a_report_poisons_the_match() {
    let defaults = defaults_document(vec![absent_defaults(), absent_defaults()]);
    let index = PageColorFactsIndex::new(None, Some(&defaults));
    assert!(index.facts_for(PAGE_REF, PAGE_OFFSET, 0).defaults.is_none());
}

#[test]
fn corroboration_rejects_offset_or_ordinal_mismatch() {
    let defaults = defaults_document(vec![absent_defaults()]);
    let index = PageColorFactsIndex::new(None, Some(&defaults));
    // Same reference, different resolved object offset.
    assert!(
        index
            .facts_for(PAGE_REF, PAGE_OFFSET + 1, 0)
            .defaults
            .is_none()
    );
    // Same reference and offset, shifted document ordinal (an inspector that
    // omitted a failed leaf must not silently re-associate).
    assert!(index.facts_for(PAGE_REF, PAGE_OFFSET, 1).defaults.is_none());
    // Different reference entirely.
    let other = IndirectRef {
        object_number: 9,
        generation: 0,
    };
    assert!(index.facts_for(other, PAGE_OFFSET, 0).defaults.is_none());
}
