#![allow(clippy::expect_used, clippy::missing_errors_doc)]

mod json;

use presslint_inventory::InventoryEntry;
use presslint_types::{
    ColorObservation, ColorSpace, ColorUsage, ContentScope, EditCapability, ObjectId, ObjectKind,
    PageIndex, Provenance,
};
use serde::{Deserialize, Serialize};

use self::json::{Json, JsonSerializer};
use super::{PageMatcher, PageParity, Predicate, Selector, matches};

fn assert_selector_json(selector: &Selector, expected_json: Json) {
    let encoded = selector
        .serialize(JsonSerializer)
        .expect("serialize selector");
    assert_eq!(encoded, expected_json);

    let decoded = Selector::deserialize(expected_json).expect("deserialize selector fixture");
    assert_eq!(&decoded, selector);
}

fn assert_predicate_json(predicate: &Predicate, expected_json: Json) {
    let encoded = predicate
        .serialize(JsonSerializer)
        .expect("serialize predicate");
    assert_eq!(encoded, expected_json);

    let decoded = Predicate::deserialize(expected_json).expect("deserialize predicate fixture");
    assert_eq!(&decoded, predicate);
}

fn form_xobject_scope(name: &[u8]) -> ContentScope {
    ContentScope::FormXObject {
        name: presslint_types::PdfName(name.to_vec()),
    }
}

fn pdf_name_json(name: &[u8]) -> Json {
    Json::array(name.iter().copied().map(u32::from).map(Json::U32))
}

#[test]
fn selector_boolean_variants_have_stable_json_shape() {
    assert_selector_json(&Selector::All, Json::object([("op", Json::string("all"))]));
    assert_selector_json(
        &Selector::None,
        Json::object([("op", Json::string("none"))]),
    );
    assert_selector_json(
        &Selector::Not {
            expr: Box::new(Selector::All),
        },
        Json::object([
            ("op", Json::string("not")),
            ("expr", Json::object([("op", Json::string("all"))])),
        ]),
    );
    assert_selector_json(
        &Selector::And {
            exprs: vec![Selector::All, Selector::None],
        },
        Json::object([
            ("op", Json::string("and")),
            (
                "exprs",
                Json::array([
                    Json::object([("op", Json::string("all"))]),
                    Json::object([("op", Json::string("none"))]),
                ]),
            ),
        ]),
    );
    assert_selector_json(
        &Selector::Or {
            exprs: vec![Selector::None, Selector::All],
        },
        Json::object([
            ("op", Json::string("or")),
            (
                "exprs",
                Json::array([
                    Json::object([("op", Json::string("none"))]),
                    Json::object([("op", Json::string("all"))]),
                ]),
            ),
        ]),
    );
}

#[test]
fn predicate_variants_have_stable_json_shape() {
    assert_predicate_json(
        &Predicate::ObjectKind {
            object_kind: ObjectKind::Vector,
        },
        Json::object([
            ("kind", Json::string("object_kind")),
            ("object_kind", Json::string("vector")),
        ]),
    );
    assert_predicate_json(
        &Predicate::ColorSpace {
            space: ColorSpace::DeviceCmyk,
        },
        Json::object([
            ("kind", Json::string("color_space")),
            ("space", Json::string("device_cmyk")),
        ]),
    );
    assert_predicate_json(
        &Predicate::Page { page: PageIndex(3) },
        Json::object([("kind", Json::string("page")), ("page", Json::U32(3))]),
    );
    assert_predicate_json(
        &Predicate::Editable {
            capability: EditCapability::RewriteColorOperand,
        },
        Json::object([
            ("kind", Json::string("editable")),
            ("capability", Json::string("rewrite_color_operand")),
        ]),
    );
}

#[test]
fn selector_predicate_fixtures_deserialize_to_expected_values() {
    assert_selector_json(
        &Selector::Predicate {
            predicate: Predicate::ObjectKind {
                object_kind: ObjectKind::Image,
            },
        },
        Json::object([
            ("op", Json::string("predicate")),
            (
                "predicate",
                Json::object([
                    ("kind", Json::string("object_kind")),
                    ("object_kind", Json::string("image")),
                ]),
            ),
        ]),
    );
    assert_selector_json(
        &Selector::Predicate {
            predicate: Predicate::ColorSpace {
                space: ColorSpace::IccBased,
            },
        },
        Json::object([
            ("op", Json::string("predicate")),
            (
                "predicate",
                Json::object([
                    ("kind", Json::string("color_space")),
                    ("space", Json::string("icc_based")),
                ]),
            ),
        ]),
    );
    assert_selector_json(
        &Selector::Predicate {
            predicate: Predicate::Page { page: PageIndex(0) },
        },
        Json::object([
            ("op", Json::string("predicate")),
            (
                "predicate",
                Json::object([("kind", Json::string("page")), ("page", Json::U32(0))]),
            ),
        ]),
    );
    assert_selector_json(
        &Selector::Predicate {
            predicate: Predicate::Editable {
                capability: EditCapability::AdjustStrokeWidth,
            },
        },
        Json::object([
            ("op", Json::string("predicate")),
            (
                "predicate",
                Json::object([
                    ("kind", Json::string("editable")),
                    ("capability", Json::string("adjust_stroke_width")),
                ]),
            ),
        ]),
    );
}

#[test]
fn scope_predicate_has_stable_json_shape() {
    assert_predicate_json(
        &Predicate::Scope {
            scope: ContentScope::Page,
        },
        Json::object([
            ("kind", Json::string("scope")),
            ("scope", Json::object([("kind", Json::string("page"))])),
        ]),
    );
    assert_predicate_json(
        &Predicate::Scope {
            scope: form_xobject_scope(b"Fm0"),
        },
        Json::object([
            ("kind", Json::string("scope")),
            (
                "scope",
                Json::object([
                    ("kind", Json::string("form_x_object")),
                    ("name", pdf_name_json(b"Fm0")),
                ]),
            ),
        ]),
    );
    assert_predicate_json(
        &Predicate::Scope {
            scope: ContentScope::AnnotationAppearance,
        },
        Json::object([
            ("kind", Json::string("scope")),
            (
                "scope",
                Json::object([("kind", Json::string("annotation_appearance"))]),
            ),
        ]),
    );
}

#[test]
fn color_usage_predicate_has_stable_json_shape() {
    assert_predicate_json(
        &Predicate::ColorUsage {
            usage: ColorUsage::Fill,
        },
        Json::object([
            ("kind", Json::string("color_usage")),
            ("usage", Json::string("fill")),
        ]),
    );
    assert_predicate_json(
        &Predicate::ColorUsage {
            usage: ColorUsage::Stroke,
        },
        Json::object([
            ("kind", Json::string("color_usage")),
            ("usage", Json::string("stroke")),
        ]),
    );
    assert_selector_json(
        &Selector::Predicate {
            predicate: Predicate::ColorUsage {
                usage: ColorUsage::Image,
            },
        },
        Json::object([
            ("op", Json::string("predicate")),
            (
                "predicate",
                Json::object([
                    ("kind", Json::string("color_usage")),
                    ("usage", Json::string("image")),
                ]),
            ),
        ]),
    );
}

#[test]
fn color_components_predicate_has_stable_json_shape() {
    assert_predicate_json(
        &Predicate::ColorComponents {
            space: ColorSpace::DeviceCmyk,
            usage: Some(ColorUsage::Stroke),
            components: vec![0.0, 0.0, 0.0, 1.0],
            tolerance: None,
        },
        Json::object([
            ("kind", Json::string("color_components")),
            ("space", Json::string("device_cmyk")),
            ("usage", Json::string("stroke")),
            (
                "components",
                Json::array([
                    Json::F64(0.0),
                    Json::F64(0.0),
                    Json::F64(0.0),
                    Json::F64(1.0),
                ]),
            ),
        ]),
    );
    assert_predicate_json(
        &Predicate::ColorComponents {
            space: ColorSpace::DeviceRgb,
            usage: None,
            components: vec![0.5, 0.25, 0.75],
            tolerance: Some(0.01),
        },
        Json::object([
            ("kind", Json::string("color_components")),
            ("space", Json::string("device_rgb")),
            (
                "components",
                Json::array([Json::F64(0.5), Json::F64(0.25), Json::F64(0.75)]),
            ),
            ("tolerance", Json::F64(0.01)),
        ]),
    );
}

#[test]
fn page_match_predicate_has_stable_json_shape() {
    assert_predicate_json(
        &Predicate::PageMatch {
            matcher: PageMatcher::Parity {
                parity: PageParity::Odd,
            },
        },
        Json::object([
            ("kind", Json::string("page_match")),
            (
                "matcher",
                Json::object([
                    ("match", Json::string("parity")),
                    ("parity", Json::string("odd")),
                ]),
            ),
        ]),
    );
    assert_predicate_json(
        &Predicate::PageMatch {
            matcher: PageMatcher::Parity {
                parity: PageParity::Even,
            },
        },
        Json::object([
            ("kind", Json::string("page_match")),
            (
                "matcher",
                Json::object([
                    ("match", Json::string("parity")),
                    ("parity", Json::string("even")),
                ]),
            ),
        ]),
    );
    assert_predicate_json(
        &Predicate::PageMatch {
            matcher: PageMatcher::Range {
                start: PageIndex(4),
                end: PageIndex(9),
            },
        },
        Json::object([
            ("kind", Json::string("page_match")),
            (
                "matcher",
                Json::object([
                    ("match", Json::string("range")),
                    ("start", Json::U32(4)),
                    ("end", Json::U32(9)),
                ]),
            ),
        ]),
    );
    assert_predicate_json(
        &Predicate::PageMatch {
            matcher: PageMatcher::Set {
                pages: vec![PageIndex(1), PageIndex(5), PageIndex(12)],
            },
        },
        Json::object([
            ("kind", Json::string("page_match")),
            (
                "matcher",
                Json::object([
                    ("match", Json::string("set")),
                    (
                        "pages",
                        Json::array([Json::U32(1), Json::U32(5), Json::U32(12)]),
                    ),
                ]),
            ),
        ]),
    );
}

#[test]
fn page_match_predicate_round_trips_through_selector() {
    assert_selector_json(
        &Selector::Predicate {
            predicate: Predicate::PageMatch {
                matcher: PageMatcher::Range {
                    start: PageIndex(2),
                    end: PageIndex(2),
                },
            },
        },
        Json::object([
            ("op", Json::string("predicate")),
            (
                "predicate",
                Json::object([
                    ("kind", Json::string("page_match")),
                    (
                        "matcher",
                        Json::object([
                            ("match", Json::string("range")),
                            ("start", Json::U32(2)),
                            ("end", Json::U32(2)),
                        ]),
                    ),
                ]),
            ),
        ]),
    );
}

fn color_observation(usage: ColorUsage) -> ColorObservation {
    color_observation_with(usage, ColorSpace::DeviceCmyk, Vec::new())
}

fn color_observation_with(
    usage: ColorUsage,
    space: ColorSpace,
    components: Vec<f64>,
) -> ColorObservation {
    ColorObservation {
        usage,
        space,
        components,
        spot_name: None,
        source: None,
    }
}

fn inventory_entry(scope: ContentScope, colors: Vec<ColorObservation>) -> InventoryEntry {
    InventoryEntry {
        id: ObjectId {
            page: PageIndex(0),
            sequence: 0,
            digest: [0u8; 32],
        },
        kind: ObjectKind::Vector,
        provenance: Provenance {
            page: PageIndex(0),
            scope,
            range: None,
        },
        bounds: None,
        colors,
        capabilities: Vec::new(),
    }
}

fn entry_with_colors(colors: Vec<ColorObservation>) -> InventoryEntry {
    inventory_entry(ContentScope::Page, colors)
}

fn color_usage_selector(usage: ColorUsage) -> Selector {
    Selector::Predicate {
        predicate: Predicate::ColorUsage { usage },
    }
}

fn color_components_selector(
    space: ColorSpace,
    usage: Option<ColorUsage>,
    components: Vec<f64>,
    tolerance: Option<f64>,
) -> Selector {
    Selector::Predicate {
        predicate: Predicate::ColorComponents {
            space,
            usage,
            components,
            tolerance,
        },
    }
}

#[test]
fn color_usage_predicate_matches_single_matching_observation() {
    let entry = entry_with_colors(vec![color_observation(ColorUsage::Fill)]);
    assert!(matches(&color_usage_selector(ColorUsage::Fill), &entry));
}

#[test]
fn color_usage_predicate_does_not_match_without_usage() {
    let entry = entry_with_colors(vec![color_observation(ColorUsage::Fill)]);
    assert!(!matches(&color_usage_selector(ColorUsage::Stroke), &entry));
}

#[test]
fn color_usage_predicate_matches_one_of_multiple_observations() {
    let entry = entry_with_colors(vec![
        color_observation(ColorUsage::Fill),
        color_observation(ColorUsage::Stroke),
    ]);
    assert!(matches(&color_usage_selector(ColorUsage::Stroke), &entry));
}

#[test]
fn color_usage_predicate_does_not_match_entry_without_observations() {
    let entry = entry_with_colors(Vec::new());
    assert!(!matches(&color_usage_selector(ColorUsage::Fill), &entry));
}

#[test]
fn color_components_predicate_matches_exact_device_cmyk_k100() {
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Stroke,
        ColorSpace::DeviceCmyk,
        vec![0.0, 0.0, 0.0, 1.0],
    )]);
    let selector = color_components_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        vec![0.0, 0.0, 0.0, 1.0],
        None,
    );
    assert!(matches(&selector, &entry));

    let selector =
        color_components_selector(ColorSpace::DeviceCmyk, None, vec![0.0, 0.0, 0.0, 1.0], None);
    assert!(matches(&selector, &entry));
}

#[test]
fn color_components_predicate_matches_with_absolute_tolerance() {
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Fill,
        ColorSpace::DeviceRgb,
        vec![0.49, 0.25, 0.751],
    )]);
    let selector = color_components_selector(
        ColorSpace::DeviceRgb,
        Some(ColorUsage::Fill),
        vec![0.5, 0.25, 0.75],
        Some(0.011),
    );
    assert!(matches(&selector, &entry));

    let selector = color_components_selector(
        ColorSpace::DeviceRgb,
        Some(ColorUsage::Fill),
        vec![0.5, 0.25, 0.75],
        Some(0.009),
    );
    assert!(!matches(&selector, &entry));
}

#[test]
fn color_components_predicate_does_not_match_wrong_space() {
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Stroke,
        ColorSpace::DeviceRgb,
        vec![0.0, 0.0, 0.0, 1.0],
    )]);
    let selector = color_components_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        vec![0.0, 0.0, 0.0, 1.0],
        None,
    );
    assert!(!matches(&selector, &entry));
}

#[test]
fn color_components_predicate_does_not_match_wrong_usage() {
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Fill,
        ColorSpace::DeviceCmyk,
        vec![0.0, 0.0, 0.0, 1.0],
    )]);
    let selector = color_components_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        vec![0.0, 0.0, 0.0, 1.0],
        None,
    );
    assert!(!matches(&selector, &entry));
}

#[test]
fn color_components_predicate_does_not_match_wrong_component_length() {
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Stroke,
        ColorSpace::DeviceCmyk,
        vec![0.0, 0.0, 1.0],
    )]);
    let selector = color_components_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        vec![0.0, 0.0, 0.0, 1.0],
        Some(1.0),
    );
    assert!(!matches(&selector, &entry));
}

#[test]
fn color_components_predicate_does_not_cross_match_multiple_observations() {
    let entry = entry_with_colors(vec![
        color_observation_with(
            ColorUsage::Stroke,
            ColorSpace::DeviceRgb,
            vec![0.0, 0.0, 0.0],
        ),
        color_observation_with(
            ColorUsage::Fill,
            ColorSpace::DeviceCmyk,
            vec![0.0, 0.0, 0.0, 1.0],
        ),
    ]);
    let selector = color_components_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        vec![0.0, 0.0, 0.0, 1.0],
        None,
    );
    assert!(!matches(&selector, &entry));
}

#[test]
fn color_components_predicate_rejects_non_finite_predicate_values() {
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Stroke,
        ColorSpace::DeviceCmyk,
        vec![0.0, 0.0, 0.0, 1.0],
    )]);
    let selector = color_components_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        vec![0.0, f64::NAN, 0.0, 1.0],
        None,
    );
    assert!(!matches(&selector, &entry));

    let selector = color_components_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        vec![0.0, 0.0, 0.0, 1.0],
        Some(f64::INFINITY),
    );
    assert!(!matches(&selector, &entry));
}

fn entry_with_scope(scope: ContentScope) -> InventoryEntry {
    inventory_entry(scope, Vec::new())
}

fn scope_selector(scope: ContentScope) -> Selector {
    Selector::Predicate {
        predicate: Predicate::Scope { scope },
    }
}

#[test]
fn scope_predicate_matches_page_content_entry() {
    let entry = entry_with_scope(ContentScope::Page);
    assert!(matches(&scope_selector(ContentScope::Page), &entry));
}

#[test]
fn scope_predicate_matches_named_form_xobject_entry() {
    let entry = entry_with_scope(form_xobject_scope(b"Fm0"));
    assert!(matches(&scope_selector(form_xobject_scope(b"Fm0")), &entry));
}

#[test]
fn scope_predicate_does_not_match_different_form_name() {
    let entry = entry_with_scope(form_xobject_scope(b"Fm0"));
    assert!(!matches(
        &scope_selector(form_xobject_scope(b"Fm1")),
        &entry
    ));
}

#[test]
fn scope_predicate_does_not_match_across_scope_kind() {
    let entry = entry_with_scope(ContentScope::Page);
    assert!(!matches(
        &scope_selector(form_xobject_scope(b"Fm0")),
        &entry
    ));
}

fn entry_on_page(page: u32) -> InventoryEntry {
    let mut entry = inventory_entry(ContentScope::Page, Vec::new());
    entry.id.page = PageIndex(page);
    entry
}

fn page_match_selector(matcher: PageMatcher) -> Selector {
    Selector::Predicate {
        predicate: Predicate::PageMatch { matcher },
    }
}

fn parity_selector(parity: PageParity) -> Selector {
    page_match_selector(PageMatcher::Parity { parity })
}

#[test]
fn parity_odd_matches_first_third_fifth_pages() {
    // One-based page numbers 1, 3, 5 are zero-based indices 0, 2, 4.
    for index in [0, 2, 4] {
        assert!(matches(
            &parity_selector(PageParity::Odd),
            &entry_on_page(index)
        ));
    }
    for index in [1, 3, 5] {
        assert!(!matches(
            &parity_selector(PageParity::Odd),
            &entry_on_page(index)
        ));
    }
}

#[test]
fn parity_even_matches_second_fourth_sixth_pages() {
    // One-based page numbers 2, 4, 6 are zero-based indices 1, 3, 5.
    for index in [1, 3, 5] {
        assert!(matches(
            &parity_selector(PageParity::Even),
            &entry_on_page(index)
        ));
    }
    for index in [0, 2, 4] {
        assert!(!matches(
            &parity_selector(PageParity::Even),
            &entry_on_page(index)
        ));
    }
}

#[test]
fn range_matches_inclusive_on_both_ends() {
    let selector = page_match_selector(PageMatcher::Range {
        start: PageIndex(4),
        end: PageIndex(9),
    });
    assert!(matches(&selector, &entry_on_page(4)));
    assert!(matches(&selector, &entry_on_page(9)));
    assert!(matches(&selector, &entry_on_page(6)));
    assert!(!matches(&selector, &entry_on_page(3)));
    assert!(!matches(&selector, &entry_on_page(10)));
}

#[test]
fn range_with_equal_ends_matches_only_that_page() {
    let selector = page_match_selector(PageMatcher::Range {
        start: PageIndex(7),
        end: PageIndex(7),
    });
    assert!(matches(&selector, &entry_on_page(7)));
    assert!(!matches(&selector, &entry_on_page(6)));
    assert!(!matches(&selector, &entry_on_page(8)));
}

#[test]
fn range_matches_nothing_when_start_is_greater_than_end() {
    let selector = page_match_selector(PageMatcher::Range {
        start: PageIndex(9),
        end: PageIndex(4),
    });
    for index in [3, 4, 6, 9, 10] {
        assert!(!matches(&selector, &entry_on_page(index)));
    }
}

#[test]
fn set_matches_membership_independent_of_order_and_duplicates() {
    let selector = page_match_selector(PageMatcher::Set {
        pages: vec![PageIndex(12), PageIndex(1), PageIndex(5), PageIndex(1)],
    });
    for index in [1, 5, 12] {
        assert!(matches(&selector, &entry_on_page(index)));
    }
    for index in [0, 2, 11, 13] {
        assert!(!matches(&selector, &entry_on_page(index)));
    }
}

#[test]
fn empty_set_matches_nothing() {
    let selector = page_match_selector(PageMatcher::Set { pages: Vec::new() });
    for index in [0, 1, 5, 12] {
        assert!(!matches(&selector, &entry_on_page(index)));
    }
}
