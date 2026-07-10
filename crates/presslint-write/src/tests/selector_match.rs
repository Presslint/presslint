//! Unit and integration tests for operator-local selector targeting (F4-4).
//!
//! The unit half exercises [`crate::selector_match::collect_unsupported_leaves`],
//! the total precheck that must reject every unanswerable leaf BEFORE the
//! canonical-matcher adapter can run. The per-leaf truth-table coverage of the
//! adapter itself lives in the sibling `selector_match_paint_adapter` module.
//! The integration half drives the public `convert_content_colors_incremental`
//! action with a `target`, reusing the synthetic-DeviceLink fixtures from the
//! sibling `content_color_convert` test module.

use presslint_pdf::DocumentAccessBackend;
use presslint_selectors::{CompareOp, PageMatcher, PageParity, Predicate, Selector};
use presslint_types::{ColorSpace, ColorUsage, EditCapability, ObjectKind, PageIndex};

use crate::selector_match::{UnsupportedTargetLeaf, collect_unsupported_leaves};
use crate::{
    BlackPreservationPolicy, ConvertContentColorsError, ConvertContentColorsRequest, PageSelection,
    convert_content_colors_incremental,
};

use super::content_color_convert::{
    RGB_TO_CMYK_LINK, classic_raw_pdf, classic_two_page_pdf, contains, convert,
    convert_with_target, one_link, operands_of, page_decoded_stream, predicate, xref_stream_pdf,
};
use super::reopen;

#[test]
fn collect_unsupported_detects_object_kind_scope_editable() {
    let selector = Selector::And {
        exprs: vec![
            predicate(Predicate::ObjectKind {
                object_kind: ObjectKind::Text,
            }),
            Selector::Not {
                expr: Box::new(predicate(Predicate::Scope {
                    scope: presslint_types::ContentScope::Page,
                })),
            },
            predicate(Predicate::Editable {
                capability: EditCapability::RewriteColorOperand,
            }),
        ],
    };
    let unsupported = collect_unsupported_leaves(&selector);
    assert_eq!(
        unsupported,
        vec![
            UnsupportedTargetLeaf::ObjectKind {
                object_kind: ObjectKind::Text,
            },
            UnsupportedTargetLeaf::Scope {
                scope: presslint_types::ContentScope::Page,
            },
            UnsupportedTargetLeaf::Editable {
                capability: EditCapability::RewriteColorOperand,
            },
        ]
    );
}

#[test]
fn collect_unsupported_detects_non_device_and_image_usage() {
    let non_device = predicate(Predicate::ColorSpace {
        space: ColorSpace::IccBased,
    });
    assert_eq!(
        collect_unsupported_leaves(&non_device),
        vec![UnsupportedTargetLeaf::ColorSpace {
            space: ColorSpace::IccBased,
        }]
    );

    let image_usage = predicate(Predicate::ColorUsage {
        usage: ColorUsage::Image,
    });
    assert_eq!(
        collect_unsupported_leaves(&image_usage),
        vec![UnsupportedTargetLeaf::ColorUsage {
            usage: ColorUsage::Image,
        }]
    );

    let bad_components = predicate(Predicate::ColorComponents {
        space: ColorSpace::Separation,
        usage: Some(ColorUsage::Shading),
        components: vec![1.0],
        tolerance: None,
    });
    assert_eq!(
        collect_unsupported_leaves(&bad_components),
        vec![UnsupportedTargetLeaf::ColorComponents {
            space: ColorSpace::Separation,
            usage: Some(ColorUsage::Shading),
        }]
    );
}

#[test]
fn supported_selector_has_no_unsupported_leaves() {
    let selector = Selector::Or {
        exprs: vec![
            predicate(Predicate::ColorSpace {
                space: ColorSpace::DeviceCmyk,
            }),
            predicate(Predicate::PageMatch {
                matcher: PageMatcher::Parity {
                    parity: PageParity::Even,
                },
            }),
            predicate(Predicate::ColorComponents {
                space: ColorSpace::DeviceRgb,
                usage: Some(ColorUsage::Fill),
                components: vec![1.0, 0.0, 0.0],
                tolerance: Some(0.01),
            }),
        ],
    };
    assert!(collect_unsupported_leaves(&selector).is_empty());
}

#[test]
fn component_compare_non_device_or_image_usage_is_unsupported() {
    let non_device = predicate(Predicate::ComponentCompare {
        space: ColorSpace::IccBased,
        usage: None,
        component_index: 0,
        op: CompareOp::Ge,
        value: 0.5,
    });
    assert_eq!(
        collect_unsupported_leaves(&non_device),
        vec![UnsupportedTargetLeaf::ComponentCompare {
            space: ColorSpace::IccBased,
            usage: None,
        }]
    );

    let image_usage = predicate(Predicate::ComponentCompare {
        space: ColorSpace::DeviceCmyk,
        usage: Some(ColorUsage::Shading),
        component_index: 3,
        op: CompareOp::Ge,
        value: 0.5,
    });
    assert_eq!(
        collect_unsupported_leaves(&image_usage),
        vec![UnsupportedTargetLeaf::ComponentCompare {
            space: ColorSpace::DeviceCmyk,
            usage: Some(ColorUsage::Shading),
        }]
    );

    // Device space + Fill/Stroke usage is supported (no unsupported leaf).
    let supported = predicate(Predicate::ComponentCompare {
        space: ColorSpace::DeviceCmyk,
        usage: Some(ColorUsage::Fill),
        component_index: 3,
        op: CompareOp::Ge,
        value: 0.85,
    });
    assert!(collect_unsupported_leaves(&supported).is_empty());
}

// --- F4-4 integration: selector-targeted conversion over real PDFs ---------

#[test]
fn all_selector_matches_target_none_behaviour() {
    let input = classic_raw_pdf(b"q 1 0 0 rg 0 0 1 RG Q\n");
    let none = convert(&input, RGB_TO_CMYK_LINK);
    let all = convert_with_target(&input, RGB_TO_CMYK_LINK, Selector::All);

    assert_eq!(all.bytes, none.bytes);
    assert_eq!(all.converted[0].operators_converted, 2);
    assert_eq!(all.converted[0].operator_skips.selector_excluded, 0);
}

#[test]
fn colorspace_rgb_selector_converts_matching_source_operator() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::ColorSpace {
            space: ColorSpace::DeviceRgb,
        }),
    );

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 0);
    assert!(!contains(&page_decoded_stream(&output.bytes, false), b"rg"));
}

#[test]
fn colorspace_cmyk_selector_excludes_rgb_operator() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::ColorSpace {
            space: ColorSpace::DeviceCmyk,
        }),
    );

    // The rg operator is a source-space match but not a selector match: left
    // byte-verbatim and counted as selector_excluded.
    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].operators_converted, 0);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 1);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"1 0 0 rg"
    ));
}

#[test]
fn selector_precedes_route_lookup_for_offspace_operators() {
    // F4-5 order: operand validation → selector → route lookup. Under an
    // `RGB`-only selector the off-space g/k operators fail the selector FIRST, so
    // they are counted `selector_excluded` and never reach the route lookup
    // (`no_matching_link` is reserved for selector-included coverage gaps).
    let input = classic_raw_pdf(b"0.5 g\n0 0 0 1 k\n1 0 0 rg\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::ColorSpace {
            space: ColorSpace::DeviceRgb,
        }),
    );

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 2);
    assert_eq!(output.converted[0].operator_skips.no_matching_link, 0);
}

#[test]
fn color_usage_stroke_selector_converts_stroke_excludes_fill() {
    let input = classic_raw_pdf(b"1 0 0 rg\n0 0 1 RG\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::ColorUsage {
            usage: ColorUsage::Stroke,
        }),
    );

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    // The stroking RG became K; the non-stroking rg is untouched.
    assert!(contains(&decoded, b"1 0 0 rg"));
    assert!(!contains(&decoded, b"RG"));
    assert_eq!(operands_of(&decoded, b"K").len(), 4);
}

#[test]
fn page_parity_selector_converts_matching_pages_only() {
    // Both pages carry the same rg operator; an Odd (one-based) parity selector
    // converts page index 0 (page 1) and excludes page index 1 (page 2).
    let input = classic_two_page_pdf(b"1 0 0 rg\n", b"0 0 1 rg\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::PageMatch {
            matcher: PageMatcher::Parity {
                parity: PageParity::Odd,
            },
        }),
    );

    assert_eq!(output.converted.len(), 2);
    let page0 = output
        .converted
        .iter()
        .find(|page| page.page_index == PageIndex(0))
        .expect("page 0");
    let page1 = output
        .converted
        .iter()
        .find(|page| page.page_index == PageIndex(1))
        .expect("page 1");
    assert_eq!(page0.operators_converted, 1);
    assert_eq!(page0.operator_skips.selector_excluded, 0);
    assert_eq!(page1.operators_converted, 0);
    assert_eq!(page1.operator_skips.selector_excluded, 1);
    // Page 2's content is preserved verbatim (no revision object for it).
    assert!(contains(&output.bytes, b"0 0 1 rg"));
}

#[test]
fn color_components_selector_converts_only_matching_colour() {
    let input = classic_raw_pdf(b"1 0 0 rg\n0 1 0 rg\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::ColorComponents {
            space: ColorSpace::DeviceRgb,
            usage: None,
            components: vec![1.0, 0.0, 0.0],
            tolerance: None,
        }),
    );

    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    // The pure-red operand converted; the green operand is left verbatim.
    assert!(contains(&decoded, b"0 1 0 rg"));
    assert_eq!(operands_of(&decoded, b"k").len(), 4);
}

#[test]
fn and_composition_requires_both_leaves() {
    let input = classic_raw_pdf(b"1 0 0 rg\n0 0 1 RG\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        Selector::And {
            exprs: vec![
                predicate(Predicate::ColorSpace {
                    space: ColorSpace::DeviceRgb,
                }),
                predicate(Predicate::ColorUsage {
                    usage: ColorUsage::Fill,
                }),
            ],
        },
    );

    // Only the fill rg satisfies both leaves.
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(!contains(&decoded, b"rg"));
    assert!(contains(&decoded, b"0 0 1 RG"));
}

#[test]
fn or_composition_matches_either_leaf() {
    let input = classic_raw_pdf(b"1 0 0 rg\n0 0 1 RG\n0 1 0 rg\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        Selector::Or {
            exprs: vec![
                predicate(Predicate::ColorUsage {
                    usage: ColorUsage::Stroke,
                }),
                predicate(Predicate::ColorComponents {
                    space: ColorSpace::DeviceRgb,
                    usage: Some(ColorUsage::Fill),
                    components: vec![0.0, 1.0, 0.0],
                    tolerance: None,
                }),
            ],
        },
    );

    assert_eq!(output.converted[0].operators_converted, 2);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"1 0 0 rg"));
    assert!(!contains(&decoded, b"RG"));
    assert_eq!(operands_of(&decoded, b"K").len(), 4);
    assert_eq!(operands_of(&decoded, b"k").len(), 4);
}

#[test]
fn not_composition_inverts_the_leaf() {
    let input = classic_raw_pdf(b"1 0 0 rg\n0 0 1 RG\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        Selector::Not {
            expr: Box::new(predicate(Predicate::ColorUsage {
                usage: ColorUsage::Fill,
            })),
        },
    );

    // Not(Fill) keeps only the stroking operator.
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(output.converted[0].operator_skips.selector_excluded, 1);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"1 0 0 rg"));
    assert!(!contains(&decoded, b"RG"));
}

#[test]
fn xref_stream_selector_targeting_converts_and_reopens() {
    let input = xref_stream_pdf(b"1 0 0 rg\n");
    let output = convert_with_target(
        &input,
        RGB_TO_CMYK_LINK,
        predicate(Predicate::ColorUsage {
            usage: ColorUsage::Fill,
        }),
    );

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert!(matches!(
        reopen(&output.bytes).backend,
        DocumentAccessBackend::XrefStreamChain { .. }
    ));
    assert_eq!(output.converted[0].operators_converted, 1);
    assert_eq!(
        operands_of(&page_decoded_stream(&output.bytes, false), b"k").len(),
        4
    );
}

#[test]
fn object_kind_selector_is_rejected_up_front() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: Some(predicate(Predicate::ObjectKind {
                object_kind: ObjectKind::Text,
            })),
        },
    )
    .expect_err("object-kind target is rejected");
    let ConvertContentColorsError::UnsupportedTargetSelector { unsupported } = error else {
        panic!("expected UnsupportedTargetSelector, got {error:?}");
    };
    assert_eq!(
        unsupported,
        vec![UnsupportedTargetLeaf::ObjectKind {
            object_kind: ObjectKind::Text,
        }]
    );
}

#[test]
fn scope_selector_is_rejected_up_front() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: Some(predicate(Predicate::Scope {
                scope: presslint_types::ContentScope::Page,
            })),
        },
    )
    .expect_err("scope target is rejected");
    assert!(matches!(
        error,
        ConvertContentColorsError::UnsupportedTargetSelector { .. }
    ));
}

#[test]
fn editable_selector_is_rejected_up_front() {
    let input = classic_raw_pdf(b"1 0 0 rg\n");
    let error = convert_content_colors_incremental(
        &input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(RGB_TO_CMYK_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: Some(predicate(Predicate::Editable {
                capability: EditCapability::RewriteColorOperand,
            })),
        },
    )
    .expect_err("editable target is rejected");
    assert!(matches!(
        error,
        ConvertContentColorsError::UnsupportedTargetSelector { .. }
    ));
}
