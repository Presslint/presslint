//! Matcher and serde shape-lock tests for the numeric component-compare
//! predicate ([`Predicate::ComponentCompare`]) and its [`CompareOp`].

use presslint_types::{ColorSpace, ColorUsage};
use serde::{Deserialize, Serialize};

use super::json::{Json, JsonSerializer};
use super::{assert_predicate_json, color_observation_with, entry_with_colors};
use crate::{CompareOp, Predicate, Selector, matches};

fn compare_selector(
    space: ColorSpace,
    usage: Option<ColorUsage>,
    component_index: usize,
    op: CompareOp,
    value: f64,
) -> Selector {
    Selector::Predicate {
        predicate: Predicate::ComponentCompare {
            space,
            usage,
            component_index,
            op,
            value,
        },
    }
}

/// A `DeviceCMYK` observation with the given K channel and usage.
fn cmyk_k(usage: ColorUsage, k: f64) -> Vec<presslint_types::ColorObservation> {
    vec![color_observation_with(
        usage,
        ColorSpace::DeviceCmyk,
        vec![0.0, 0.0, 0.0, k],
    )]
}

#[test]
fn each_compare_op_evaluates_against_the_k_channel() {
    // K = 0.85 exactly, so we can probe every operator around the boundary.
    let entry = entry_with_colors(cmyk_k(ColorUsage::Fill, 0.85));
    let at = |op, value| {
        matches(
            &compare_selector(ColorSpace::DeviceCmyk, None, 3, op, value),
            &entry,
        )
    };

    assert!(at(CompareOp::Ge, 0.85));
    assert!(!at(CompareOp::Ge, 0.86));
    assert!(at(CompareOp::Gt, 0.84));
    assert!(!at(CompareOp::Gt, 0.85));
    assert!(at(CompareOp::Le, 0.85));
    assert!(!at(CompareOp::Le, 0.84));
    assert!(at(CompareOp::Lt, 0.86));
    assert!(!at(CompareOp::Lt, 0.85));
    assert!(at(CompareOp::Eq, 0.85));
    assert!(!at(CompareOp::Eq, 0.84));
}

#[test]
fn threshold_matches_dark_black_only() {
    // `K >= 0.85`: a rich black passes, a mid grey does not.
    let selector = compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Ge, 0.85);
    assert!(matches(
        &selector,
        &entry_with_colors(cmyk_k(ColorUsage::Fill, 0.9))
    ));
    assert!(!matches(
        &selector,
        &entry_with_colors(cmyk_k(ColorUsage::Fill, 0.5))
    ));
}

#[test]
fn usage_gates_the_compared_observation() {
    let entry = entry_with_colors(cmyk_k(ColorUsage::Stroke, 0.9));

    // `None` usage matches any observation usage.
    assert!(matches(
        &compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Ge, 0.85),
        &entry
    ));
    // A matching usage still matches.
    let stroke = compare_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Stroke),
        3,
        CompareOp::Ge,
        0.85,
    );
    assert!(matches(&stroke, &entry));
    // A mismatched usage gates it out even though the K value would pass.
    let fill = compare_selector(
        ColorSpace::DeviceCmyk,
        Some(ColorUsage::Fill),
        3,
        CompareOp::Ge,
        0.85,
    );
    assert!(!matches(&fill, &entry));
}

#[test]
fn wrong_space_never_matches() {
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Fill,
        ColorSpace::DeviceRgb,
        vec![0.0, 0.0, 0.9],
    )]);
    let selector = compare_selector(ColorSpace::DeviceCmyk, None, 2, CompareOp::Ge, 0.5);
    assert!(!matches(&selector, &entry));
}

#[test]
fn out_of_range_index_is_a_non_match() {
    // A DeviceGray observation has one component; index 3 is out of range.
    let entry = entry_with_colors(vec![color_observation_with(
        ColorUsage::Fill,
        ColorSpace::DeviceGray,
        vec![0.5],
    )]);
    // Even a trivially-true threshold (`>= 0.0`) does not match a missing
    // component — it is a clean non-match, not a panic.
    let selector = compare_selector(ColorSpace::DeviceGray, None, 3, CompareOp::Ge, 0.0);
    assert!(!matches(&selector, &entry));
}

#[test]
fn non_finite_value_or_component_is_a_non_match() {
    // Non-finite predicate value: never matches.
    let entry = entry_with_colors(cmyk_k(ColorUsage::Fill, 0.9));
    let nan_value = compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Ge, f64::NAN);
    assert!(!matches(&nan_value, &entry));
    let inf_value = compare_selector(
        ColorSpace::DeviceCmyk,
        None,
        3,
        CompareOp::Le,
        f64::INFINITY,
    );
    assert!(!matches(&inf_value, &entry));

    // Non-finite observed component: never matches.
    let nan_entry = entry_with_colors(cmyk_k(ColorUsage::Fill, f64::NAN));
    let finite = compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Ge, 0.5);
    assert!(!matches(&finite, &nan_entry));
}

#[test]
fn band_via_and_of_two_compares() {
    // A band `K >= 0.2 AND K < 0.8` is expressed with the boolean `And`, with no
    // dedicated band variant.
    let band = Selector::And {
        exprs: vec![
            compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Ge, 0.2),
            compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Lt, 0.8),
        ],
    };
    assert!(matches(
        &band,
        &entry_with_colors(cmyk_k(ColorUsage::Fill, 0.5))
    ));
    // Below the band.
    assert!(!matches(
        &band,
        &entry_with_colors(cmyk_k(ColorUsage::Fill, 0.1))
    ));
    // At the open upper bound.
    assert!(!matches(
        &band,
        &entry_with_colors(cmyk_k(ColorUsage::Fill, 0.8))
    ));
}

#[test]
fn matches_any_observation_on_the_entry() {
    let entry = entry_with_colors(vec![
        color_observation_with(
            ColorUsage::Fill,
            ColorSpace::DeviceCmyk,
            vec![0.0, 0.0, 0.0, 0.1],
        ),
        color_observation_with(
            ColorUsage::Stroke,
            ColorSpace::DeviceCmyk,
            vec![0.0, 0.0, 0.0, 0.9],
        ),
    ]);
    let selector = compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Ge, 0.85);
    assert!(matches(&selector, &entry));
}

fn assert_compare_op_json(op: CompareOp, expected_json: Json) {
    let encoded = op.serialize(JsonSerializer).expect("serialize compare op");
    assert_eq!(encoded, expected_json);
    let decoded = CompareOp::deserialize(expected_json).expect("deserialize compare op fixture");
    assert_eq!(decoded, op);
}

#[test]
fn compare_op_has_stable_json_shape() {
    assert_compare_op_json(CompareOp::Ge, Json::string("ge"));
    assert_compare_op_json(CompareOp::Gt, Json::string("gt"));
    assert_compare_op_json(CompareOp::Le, Json::string("le"));
    assert_compare_op_json(CompareOp::Lt, Json::string("lt"));
    assert_compare_op_json(CompareOp::Eq, Json::string("eq"));
}

#[test]
fn component_compare_predicate_has_stable_json_shape() {
    assert_predicate_json(
        &Predicate::ComponentCompare {
            space: ColorSpace::DeviceCmyk,
            usage: Some(ColorUsage::Fill),
            component_index: 3,
            op: CompareOp::Ge,
            value: 0.85,
        },
        Json::object([
            ("kind", Json::string("component_compare")),
            ("space", Json::string("device_cmyk")),
            ("usage", Json::string("fill")),
            ("component_index", Json::U32(3)),
            ("op", Json::string("ge")),
            ("value", Json::F64(0.85)),
        ]),
    );
    // Usage is omitted when `None`.
    assert_predicate_json(
        &Predicate::ComponentCompare {
            space: ColorSpace::DeviceRgb,
            usage: None,
            component_index: 0,
            op: CompareOp::Lt,
            value: 0.5,
        },
        Json::object([
            ("kind", Json::string("component_compare")),
            ("space", Json::string("device_rgb")),
            ("component_index", Json::U32(0)),
            ("op", Json::string("lt")),
            ("value", Json::F64(0.5)),
        ]),
    );
}

#[test]
fn component_compare_predicate_round_trips_through_selector() {
    let selector = compare_selector(ColorSpace::DeviceCmyk, None, 3, CompareOp::Ge, 0.85);
    let encoded = selector
        .serialize(JsonSerializer)
        .expect("serialize selector");
    let decoded = Selector::deserialize(encoded).expect("deserialize selector");
    assert_eq!(decoded, selector);
}
