//! Differential truth table for the canonical-matcher selector adapter.
//!
//! `selector_matches_operator` evaluates a target selector through the
//! canonical `presslint_selectors::matches` over a private ephemeral
//! single-observation entry. Every case here asserts THREE agreeing values for
//! each accepted selector shape: the adapter's answer, the canonical matcher's
//! answer over a REAL-shaped single-observation entry (nonzero identity,
//! provenance range, edit capability — proving the adapter's inert sentinel
//! fields are unobservable), and the pinned truth vector carried over from the
//! retired write-local recursive evaluator.

use presslint_inventory::InventoryEntry;
use presslint_selectors::{CompareOp, PageMatcher, PageParity, Predicate, Selector, matches};
use presslint_types::{
    ByteRange, ColorObservation, ColorSpace, ColorUsage, ContentScope, EditCapability, ObjectId,
    ObjectKind, PageIndex, Provenance,
};

use crate::content_color_convert::DeviceColorSpace;
use crate::selector_match::selector_matches_operator;
use crate::{
    BlackPreservationPolicy, ConvertContentColorsError, ConvertContentColorsRequest, PageSelection,
    convert_content_colors_incremental,
};

use super::content_color_convert::{RGB_TO_CMYK_LINK, classic_raw_pdf, one_link, predicate};

const fn types_space(space: DeviceColorSpace) -> ColorSpace {
    match space {
        DeviceColorSpace::Gray => ColorSpace::DeviceGray,
        DeviceColorSpace::Rgb => ColorSpace::DeviceRgb,
        DeviceColorSpace::Cmyk => ColorSpace::DeviceCmyk,
    }
}

/// A REAL-shaped single-observation vector entry for the differential check:
/// every field the adapter fills with an inert sentinel carries a live value
/// here, so agreement proves the accepted subset never observes the sentinels.
fn realistic_entry(
    page: u32,
    space: DeviceColorSpace,
    usage: ColorUsage,
    components: &[f64],
) -> InventoryEntry {
    InventoryEntry {
        id: ObjectId {
            page: PageIndex(page),
            sequence: 7,
            digest: [0xAB; 32],
        },
        kind: ObjectKind::Vector,
        provenance: Provenance {
            page: PageIndex(page),
            scope: ContentScope::Page,
            range: Some(ByteRange { start: 10, end: 20 }),
            invocation: None,
        },
        bounds: None,
        colors: vec![ColorObservation {
            usage,
            space: types_space(space),
            components: components.to_vec(),
            spot_name: None,
            spot_names: Vec::new(),
            source: Some(ByteRange { start: 10, end: 18 }),
        }],
        capabilities: vec![EditCapability::RewriteColorOperand],
    }
}

/// Assert adapter == canonical-over-real-entry == pinned expectation.
fn assert_truth(
    selector: &Selector,
    page: u32,
    space: DeviceColorSpace,
    usage: ColorUsage,
    components: &[f64],
    expected: bool,
) {
    assert_eq!(
        selector_matches_operator(selector, PageIndex(page), space, usage, components),
        expected,
        "adapter truth for {selector:?}"
    );
    let entry = realistic_entry(page, space, usage, components);
    assert_eq!(
        matches(selector, &entry),
        expected,
        "canonical truth for {selector:?}"
    );
}

/// Shorthand for the most common context: page 0, RGB fill red.
fn assert_rgb_fill_red(selector: &Selector, expected: bool) {
    assert_truth(
        selector,
        0,
        DeviceColorSpace::Rgb,
        ColorUsage::Fill,
        &[1.0, 0.0, 0.0],
        expected,
    );
}

#[test]
fn all_matches_none_does_not() {
    assert_rgb_fill_red(&Selector::All, true);
    assert_rgb_fill_red(&Selector::None, false);
}

#[test]
fn colorspace_leaf_matches_the_declared_space() {
    let rgb = predicate(Predicate::ColorSpace {
        space: ColorSpace::DeviceRgb,
    });
    assert_rgb_fill_red(&rgb, true);
    assert_truth(
        &rgb,
        0,
        DeviceColorSpace::Cmyk,
        ColorUsage::Fill,
        &[0.0, 0.0, 0.0, 1.0],
        false,
    );
    let gray = predicate(Predicate::ColorSpace {
        space: ColorSpace::DeviceGray,
    });
    assert_truth(
        &gray,
        0,
        DeviceColorSpace::Gray,
        ColorUsage::Fill,
        &[0.5],
        true,
    );
}

#[test]
fn page_and_page_match_leaves() {
    let gray = |selector: &Selector, page: u32, expected: bool| {
        assert_truth(
            selector,
            page,
            DeviceColorSpace::Gray,
            ColorUsage::Fill,
            &[0.5],
            expected,
        );
    };

    let exact = predicate(Predicate::Page { page: PageIndex(2) });
    gray(&exact, 2, true);
    gray(&exact, 3, false);

    // Parity is on the one-based page number: index 0 (page 1) is odd.
    let odd = predicate(Predicate::PageMatch {
        matcher: PageMatcher::Parity {
            parity: PageParity::Odd,
        },
    });
    gray(&odd, 0, true);
    gray(&odd, 1, false);

    let range = predicate(Predicate::PageMatch {
        matcher: PageMatcher::Range {
            start: PageIndex(1),
            end: PageIndex(3),
        },
    });
    gray(&range, 0, false);
    gray(&range, 2, true);
    gray(&range, 4, false);

    let set = predicate(Predicate::PageMatch {
        matcher: PageMatcher::Set {
            pages: vec![PageIndex(0), PageIndex(4)],
        },
    });
    gray(&set, 4, true);
    gray(&set, 2, false);
}

#[test]
fn color_usage_leaf_gates_fill_and_stroke() {
    let stroke = predicate(Predicate::ColorUsage {
        usage: ColorUsage::Stroke,
    });
    assert_rgb_fill_red(&stroke, false);
    assert_truth(
        &stroke,
        0,
        DeviceColorSpace::Rgb,
        ColorUsage::Stroke,
        &[1.0, 0.0, 0.0],
        true,
    );
}

#[test]
fn color_components_leaf_with_and_without_tolerance() {
    let exact = predicate(Predicate::ColorComponents {
        space: ColorSpace::DeviceRgb,
        usage: None,
        components: vec![1.0, 0.0, 0.0],
        tolerance: None,
    });
    assert_rgb_fill_red(&exact, true);
    assert_truth(
        &exact,
        0,
        DeviceColorSpace::Rgb,
        ColorUsage::Fill,
        &[0.99, 0.0, 0.0],
        false,
    );

    let toleranced = predicate(Predicate::ColorComponents {
        space: ColorSpace::DeviceRgb,
        usage: Some(ColorUsage::Fill),
        components: vec![1.0, 0.0, 0.0],
        tolerance: Some(0.05),
    });
    assert_truth(
        &toleranced,
        0,
        DeviceColorSpace::Rgb,
        ColorUsage::Fill,
        &[0.99, 0.0, 0.0],
        true,
    );

    // Usage on the predicate must also match the operator usage.
    let stroke_only = predicate(Predicate::ColorComponents {
        space: ColorSpace::DeviceRgb,
        usage: Some(ColorUsage::Stroke),
        components: vec![1.0, 0.0, 0.0],
        tolerance: None,
    });
    assert_rgb_fill_red(&stroke_only, false);
}

#[test]
fn color_components_invalid_tolerance_is_a_clean_non_match() {
    for tolerance in [-0.1, f64::NAN, f64::INFINITY] {
        let selector = predicate(Predicate::ColorComponents {
            space: ColorSpace::DeviceRgb,
            usage: None,
            components: vec![1.0, 0.0, 0.0],
            tolerance: Some(tolerance),
        });
        // Negative tolerance never matches; non-finite tolerance never matches
        // (infinite tolerance is not treated as match-everything).
        assert_rgb_fill_red(&selector, false);
    }
}

#[test]
fn component_compare_all_ops_over_device_cmyk() {
    let compare = |op: CompareOp, value: f64| {
        predicate(Predicate::ComponentCompare {
            space: ColorSpace::DeviceCmyk,
            usage: None,
            component_index: 3,
            op,
            value,
        })
    };
    let cmyk = |selector: &Selector, k: f64, expected: bool| {
        assert_truth(
            selector,
            0,
            DeviceColorSpace::Cmyk,
            ColorUsage::Fill,
            &[0.0, 0.0, 0.0, k],
            expected,
        );
    };

    let ge = compare(CompareOp::Ge, 0.85);
    cmyk(&ge, 0.9, true);
    cmyk(&ge, 0.5, false);

    let lt = compare(CompareOp::Lt, 0.85);
    cmyk(&lt, 0.9, false);
    cmyk(&lt, 0.5, true);

    // 0.9 is not strictly greater than 0.9; 0.5 is not either.
    let gt = compare(CompareOp::Gt, 0.9);
    cmyk(&gt, 0.9, false);
    cmyk(&gt, 0.5, false);

    // 0.9 <= 0.5 is false; 0.5 <= 0.5 is true (boundary inclusive).
    let le = compare(CompareOp::Le, 0.5);
    cmyk(&le, 0.9, false);
    cmyk(&le, 0.5, true);

    // Exact float equality (no tolerance).
    let eq = compare(CompareOp::Eq, 0.9);
    cmyk(&eq, 0.9, true);
    cmyk(&eq, 0.5, false);
}

#[test]
fn component_compare_non_finite_value_or_component_is_non_match() {
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        for op in [
            CompareOp::Ge,
            CompareOp::Gt,
            CompareOp::Le,
            CompareOp::Lt,
            CompareOp::Eq,
        ] {
            let selector = predicate(Predicate::ComponentCompare {
                space: ColorSpace::DeviceCmyk,
                usage: None,
                component_index: 3,
                op,
                value,
            });
            assert_truth(
                &selector,
                0,
                DeviceColorSpace::Cmyk,
                ColorUsage::Fill,
                &[0.0, 0.0, 0.0, 0.9],
                false,
            );
        }
    }

    // A non-finite actual component never matches a finite threshold.
    let selector = predicate(Predicate::ComponentCompare {
        space: ColorSpace::DeviceCmyk,
        usage: None,
        component_index: 3,
        op: CompareOp::Ge,
        value: 0.5,
    });
    assert_truth(
        &selector,
        0,
        DeviceColorSpace::Cmyk,
        ColorUsage::Fill,
        &[0.0, 0.0, 0.0, f64::NAN],
        false,
    );
}

#[test]
fn component_compare_usage_gates_the_operator() {
    let fill_only = predicate(Predicate::ComponentCompare {
        space: ColorSpace::DeviceCmyk,
        usage: Some(ColorUsage::Fill),
        component_index: 3,
        op: CompareOp::Ge,
        value: 0.85,
    });
    let stroke_only = predicate(Predicate::ComponentCompare {
        space: ColorSpace::DeviceCmyk,
        usage: Some(ColorUsage::Stroke),
        component_index: 3,
        op: CompareOp::Ge,
        value: 0.85,
    });
    let dark = [0.0, 0.0, 0.0, 0.9];
    assert_truth(
        &fill_only,
        0,
        DeviceColorSpace::Cmyk,
        ColorUsage::Stroke,
        &dark,
        false,
    );
    assert_truth(
        &stroke_only,
        0,
        DeviceColorSpace::Cmyk,
        ColorUsage::Stroke,
        &dark,
        true,
    );
}

#[test]
fn component_compare_band_via_and() {
    // `K >= 0.2 AND K < 0.8` via boolean `And` — no dedicated band variant.
    let band = Selector::And {
        exprs: vec![
            predicate(Predicate::ComponentCompare {
                space: ColorSpace::DeviceCmyk,
                usage: None,
                component_index: 3,
                op: CompareOp::Ge,
                value: 0.2,
            }),
            predicate(Predicate::ComponentCompare {
                space: ColorSpace::DeviceCmyk,
                usage: None,
                component_index: 3,
                op: CompareOp::Lt,
                value: 0.8,
            }),
        ],
    };
    assert_truth(
        &band,
        0,
        DeviceColorSpace::Cmyk,
        ColorUsage::Fill,
        &[0.0, 0.0, 0.0, 0.5],
        true,
    );
    assert_truth(
        &band,
        0,
        DeviceColorSpace::Cmyk,
        ColorUsage::Fill,
        &[0.0, 0.0, 0.0, 0.9],
        false,
    );
}

#[test]
fn component_compare_out_of_range_index_is_non_match() {
    // A DeviceGray operator has one component; index 3 is out of range.
    let selector = predicate(Predicate::ComponentCompare {
        space: ColorSpace::DeviceGray,
        usage: None,
        component_index: 3,
        op: CompareOp::Ge,
        value: 0.0,
    });
    assert_truth(
        &selector,
        0,
        DeviceColorSpace::Gray,
        ColorUsage::Fill,
        &[0.5],
        false,
    );
}

#[test]
fn nested_boolean_composition() {
    let and = Selector::And {
        exprs: vec![
            predicate(Predicate::ColorSpace {
                space: ColorSpace::DeviceRgb,
            }),
            predicate(Predicate::ColorUsage {
                usage: ColorUsage::Fill,
            }),
        ],
    };
    assert_rgb_fill_red(&and, true);
    assert_truth(
        &and,
        0,
        DeviceColorSpace::Rgb,
        ColorUsage::Stroke,
        &[1.0, 0.0, 0.0],
        false,
    );

    let or = Selector::Or {
        exprs: vec![
            predicate(Predicate::ColorUsage {
                usage: ColorUsage::Stroke,
            }),
            predicate(Predicate::Page { page: PageIndex(0) }),
        ],
    };
    assert_rgb_fill_red(&or, true);

    let not = Selector::Not {
        expr: Box::new(predicate(Predicate::ColorUsage {
            usage: ColorUsage::Fill,
        })),
    };
    assert_rgb_fill_red(&not, false);

    // Not(Or(Not(page 1), stroke)) — deep nesting stays consistent.
    let deep = Selector::Not {
        expr: Box::new(Selector::Or {
            exprs: vec![
                Selector::Not {
                    expr: Box::new(predicate(Predicate::Page { page: PageIndex(0) })),
                },
                predicate(Predicate::ColorUsage {
                    usage: ColorUsage::Stroke,
                }),
            ],
        }),
    };
    assert_rgb_fill_red(&deep, true);
    assert_truth(
        &deep,
        1,
        DeviceColorSpace::Rgb,
        ColorUsage::Fill,
        &[1.0, 0.0, 0.0],
        false,
    );
}

#[test]
fn unsupported_leaf_nested_under_not_and_or_is_rejected_up_front() {
    // The synthetic entry's sentinel capability list would answer `Editable`
    // observably, so the total precheck must reject it in ANY nesting before
    // the adapter can ever run.
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let nested = Selector::Not {
        expr: Box::new(Selector::Or {
            exprs: vec![
                predicate(Predicate::ColorUsage {
                    usage: ColorUsage::Fill,
                }),
                Selector::And {
                    exprs: vec![Selector::Not {
                        expr: Box::new(predicate(Predicate::Editable {
                            capability: EditCapability::RewriteColorOperand,
                        })),
                    }],
                },
            ],
        }),
    };
    let error = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: Some(nested),
        },
    )
    .expect_err("nested unsupported leaf is rejected");
    assert!(matches!(
        error,
        ConvertContentColorsError::UnsupportedTargetSelector { .. }
    ));
}
