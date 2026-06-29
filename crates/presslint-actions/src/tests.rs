#![allow(clippy::expect_used, clippy::missing_errors_doc)]

mod json;
mod patch_boundary;

use std::fmt::Debug;

use presslint_core::{
    ByteRange, ColorObservation, ColorSpace, ColorUsage, ContentScope, EditCapability, ObjectId,
    ObjectKind, PageIndex, Provenance,
};
use presslint_inventory::{Inventory, InventoryEntry};
use presslint_selectors::{Predicate, Selector};
use serde::{Serialize, de::DeserializeOwned};

use self::json::{Json, JsonSerializer};
use super::{
    Action, ActionPlan, ConvertColor, MinimumStrokeWidth, PatchPlan, PatchPlanMode, Recipe,
    RecipeStep, SkipReason, SkippedTarget, SpreadText, plan_recipe,
};

fn object_id(sequence: u32) -> ObjectId {
    let mut digest = [0; 32];
    digest[0] = u8::try_from(sequence).unwrap_or(u8::MAX);

    ObjectId {
        page: PageIndex(0),
        sequence,
        digest,
    }
}

fn entry(
    sequence: u32,
    kind: ObjectKind,
    capabilities: impl IntoIterator<Item = EditCapability>,
) -> InventoryEntry {
    InventoryEntry {
        id: object_id(sequence),
        kind,
        provenance: Provenance {
            page: PageIndex(0),
            scope: ContentScope::Page,
            range: None,
        },
        bounds: None,
        colors: Vec::new(),
        capabilities: capabilities.into_iter().collect(),
    }
}

fn color_entry(
    sequence: u32,
    kind: ObjectKind,
    capabilities: impl IntoIterator<Item = EditCapability>,
    colors: impl IntoIterator<Item = ColorObservation>,
) -> InventoryEntry {
    InventoryEntry {
        colors: colors.into_iter().collect(),
        ..entry(sequence, kind, capabilities)
    }
}

fn fill_color(space: ColorSpace) -> ColorObservation {
    fill_color_with_source(space, None)
}

fn sourced_fill_color(space: ColorSpace, start: usize, end: usize) -> ColorObservation {
    fill_color_with_source(space, Some(ByteRange { start, end }))
}

fn fill_color_with_source(space: ColorSpace, source: Option<ByteRange>) -> ColorObservation {
    ColorObservation {
        usage: ColorUsage::Fill,
        space,
        components: Vec::new(),
        spot_name: None,
        source,
    }
}

fn convert_color_step(select: Selector) -> RecipeStep {
    recipe_step(
        select,
        Action::ConvertColor(ConvertColor {
            target: "pso-coated-v3".to_owned(),
        }),
    )
}

fn recipe_step(select: Selector, action: Action) -> RecipeStep {
    RecipeStep { select, action }
}

#[test]
fn plan_recipe_preserves_selector_matches_in_inventory_order() {
    let inventory = Inventory {
        entries: vec![
            color_entry(
                2,
                ObjectKind::Vector,
                [EditCapability::RewriteColorOperand],
                [sourced_fill_color(ColorSpace::DeviceCmyk, 0, 8)],
            ),
            entry(1, ObjectKind::Text, [EditCapability::AddTextSpreadStroke]),
            color_entry(
                3,
                ObjectKind::Vector,
                [EditCapability::RewriteColorOperand],
                [sourced_fill_color(ColorSpace::DeviceRgb, 20, 29)],
            ),
        ],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::Predicate {
            predicate: Predicate::ObjectKind {
                object_kind: ObjectKind::Vector,
            },
        })],
    };

    let plan = plan_recipe(&recipe, &inventory);

    assert_eq!(plan.mode, PatchPlanMode::NoOp);
    assert!(plan.is_no_op());
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].targets, vec![object_id(2), object_id(3)]);
    assert!(plan.steps[0].skipped.is_empty());
}

#[test]
fn plan_recipe_reports_empty_matches_without_skips() {
    let inventory = Inventory {
        entries: vec![entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![recipe_step(
            Selector::None,
            Action::MinimumStrokeWidth(MinimumStrokeWidth { width_pt: 0.25 }),
        )],
    };

    let plan = plan_recipe(&recipe, &inventory);

    assert_eq!(plan.mode, PatchPlanMode::NoOp);
    assert!(plan.steps[0].targets.is_empty());
    assert!(plan.steps[0].skipped.is_empty());
}

#[test]
fn plan_recipe_reports_unsupported_capability_skips_in_inventory_order() {
    let inventory = Inventory {
        entries: vec![
            entry(1, ObjectKind::Text, [EditCapability::ReadOnly]),
            entry(2, ObjectKind::Text, [EditCapability::AddTextSpreadStroke]),
            entry(3, ObjectKind::Text, [EditCapability::RewriteColorOperand]),
        ],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![recipe_step(
            Selector::All,
            Action::SpreadText(SpreadText {
                amount_pt: 0.1,
                overprint: true,
            }),
        )],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert_eq!(step.targets, vec![object_id(2)]);
    assert_eq!(
        step.skipped,
        vec![
            SkippedTarget {
                object: object_id(1),
                reason: SkipReason::UnsupportedCapability {
                    required: EditCapability::AddTextSpreadStroke,
                },
            },
            SkippedTarget {
                object: object_id(3),
                reason: SkipReason::UnsupportedCapability {
                    required: EditCapability::AddTextSpreadStroke,
                },
            },
        ]
    );
}

#[test]
fn convert_color_accepts_sourced_process_device_fill_target() {
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [sourced_fill_color(ColorSpace::DeviceCmyk, 0, 8)],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert_eq!(step.targets, vec![object_id(1)]);
    assert!(step.skipped.is_empty());
}

#[test]
fn convert_color_skips_multiple_sourced_process_device_colors() {
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [
                sourced_fill_color(ColorSpace::DeviceCmyk, 0, 8),
                sourced_fill_color(ColorSpace::DeviceRgb, 20, 29),
            ],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert!(step.targets.is_empty());
    assert_eq!(
        step.skipped,
        vec![SkippedTarget {
            object: object_id(1),
            reason: SkipReason::AmbiguousColorSource,
        }]
    );
}

#[test]
fn convert_color_skips_process_device_color_without_source() {
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [fill_color(ColorSpace::DeviceRgb)],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert!(step.targets.is_empty());
    assert_eq!(
        step.skipped,
        vec![SkippedTarget {
            object: object_id(1),
            reason: SkipReason::MissingColorSource,
        }]
    );
}

#[test]
fn convert_color_skips_mixed_sourced_and_unsourced_process_device_color_as_missing_source() {
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [
                sourced_fill_color(ColorSpace::DeviceCmyk, 0, 8),
                fill_color(ColorSpace::DeviceRgb),
            ],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert!(step.targets.is_empty());
    assert_eq!(
        step.skipped,
        vec![SkippedTarget {
            object: object_id(1),
            reason: SkipReason::MissingColorSource,
        }]
    );
}

#[test]
fn convert_color_skips_spot_separation_as_non_process() {
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [fill_color(ColorSpace::Separation)],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert!(step.targets.is_empty());
    assert_eq!(
        step.skipped,
        vec![SkippedTarget {
            object: object_id(1),
            reason: SkipReason::NonProcessColor,
        }]
    );
}

#[test]
fn convert_color_accepts_mixed_observation_entry() {
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [
                fill_color(ColorSpace::Separation),
                sourced_fill_color(ColorSpace::DeviceGray, 10, 13),
            ],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert_eq!(step.targets, vec![object_id(1)]);
    assert!(step.skipped.is_empty());
}

#[test]
fn convert_color_skips_entry_without_color_observations() {
    let inventory = Inventory {
        entries: vec![entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert!(step.targets.is_empty());
    assert_eq!(
        step.skipped,
        vec![SkippedTarget {
            object: object_id(1),
            reason: SkipReason::NonProcessColor,
        }]
    );
}

#[test]
fn convert_color_unsupported_capability_precedes_color_source_checks() {
    let inventory = Inventory {
        entries: vec![
            color_entry(
                1,
                ObjectKind::Vector,
                [EditCapability::ReadOnly],
                [fill_color(ColorSpace::Separation)],
            ),
            color_entry(
                2,
                ObjectKind::Vector,
                [EditCapability::ReadOnly],
                [fill_color(ColorSpace::DeviceRgb)],
            ),
            color_entry(
                3,
                ObjectKind::Vector,
                [EditCapability::ReadOnly],
                [
                    sourced_fill_color(ColorSpace::DeviceCmyk, 0, 8),
                    sourced_fill_color(ColorSpace::DeviceRgb, 20, 29),
                ],
            ),
        ],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert!(step.targets.is_empty());
    assert_eq!(
        step.skipped,
        vec![
            SkippedTarget {
                object: object_id(1),
                reason: SkipReason::UnsupportedCapability {
                    required: EditCapability::RewriteColorOperand,
                },
            },
            SkippedTarget {
                object: object_id(2),
                reason: SkipReason::UnsupportedCapability {
                    required: EditCapability::RewriteColorOperand,
                },
            },
            SkippedTarget {
                object: object_id(3),
                reason: SkipReason::UnsupportedCapability {
                    required: EditCapability::RewriteColorOperand,
                },
            }
        ]
    );
}

#[test]
fn non_color_actions_ignore_process_color_eligibility() {
    let inventory = Inventory {
        entries: vec![
            entry(1, ObjectKind::Text, [EditCapability::AddTextSpreadStroke]),
            entry(2, ObjectKind::Vector, [EditCapability::AdjustStrokeWidth]),
        ],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![
            recipe_step(
                Selector::Predicate {
                    predicate: Predicate::ObjectKind {
                        object_kind: ObjectKind::Text,
                    },
                },
                Action::SpreadText(SpreadText {
                    amount_pt: 0.1,
                    overprint: false,
                }),
            ),
            recipe_step(
                Selector::Predicate {
                    predicate: Predicate::ObjectKind {
                        object_kind: ObjectKind::Vector,
                    },
                },
                Action::MinimumStrokeWidth(MinimumStrokeWidth { width_pt: 0.25 }),
            ),
        ],
    };

    let plan = plan_recipe(&recipe, &inventory);

    assert_eq!(plan.steps[0].targets, vec![object_id(1)]);
    assert!(plan.steps[0].skipped.is_empty());
    assert_eq!(plan.steps[1].targets, vec![object_id(2)]);
    assert!(plan.steps[1].skipped.is_empty());
}

// --- Serde shape tests -------------------------------------------------------
//
// These lock the public JSON encoding of the recipe, action, and patch-plan
// data contracts. Each fixture asserts a full round-trip: the value serializes
// to the locked `Json` tree and that tree deserializes back to the equal value.
// The fixtures assert the externally-tagged `action`/`reason` field names and
// `snake_case` variant names exactly as the current `#[serde(...)]` attributes
// emit them; if a fixture and the code disagree, the fixture is wrong.

/// Assert that `value` serializes to `expected` and `expected` deserializes
/// back to `value`.
fn assert_json_round_trip<T>(value: &T, expected: Json)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let encoded = value.serialize(JsonSerializer).expect("serialize value");
    assert_eq!(encoded, expected);

    let decoded = T::deserialize(expected).expect("deserialize fixture");
    assert_eq!(&decoded, value);
}

/// Locked JSON encoding of an [`ObjectId`] built by the `object_id` helper:
/// page `0`, the given `sequence`, and a digest whose first byte mirrors the
/// sequence with all other bytes zero.
fn object_id_json(sequence: u32) -> Json {
    let mut digest: Vec<Json> = (0..32).map(|_| Json::U32(0)).collect();
    digest[0] = Json::U32(u32::from(u8::try_from(sequence).unwrap_or(u8::MAX)));

    Json::object([
        ("page", Json::U32(0)),
        ("sequence", Json::U32(sequence)),
        ("digest", Json::Array(digest)),
    ])
}

#[test]
fn action_variants_have_stable_json_shape() {
    assert_json_round_trip(
        &Action::ConvertColor(ConvertColor {
            target: "pso-coated-v3".to_owned(),
        }),
        Json::object([
            ("action", Json::string("convert_color")),
            ("target", Json::string("pso-coated-v3")),
        ]),
    );
    assert_json_round_trip(
        &Action::SpreadText(SpreadText {
            amount_pt: 0.25,
            overprint: true,
        }),
        Json::object([
            ("action", Json::string("spread_text")),
            ("amount_pt", Json::F64(0.25)),
            ("overprint", Json::Bool(true)),
        ]),
    );
    assert_json_round_trip(
        &Action::MinimumStrokeWidth(MinimumStrokeWidth { width_pt: 0.5 }),
        Json::object([
            ("action", Json::string("minimum_stroke_width")),
            ("width_pt", Json::F64(0.5)),
        ]),
    );
}

#[test]
fn recipe_and_step_have_stable_json_shape() {
    let step = RecipeStep {
        select: Selector::All,
        action: Action::ConvertColor(ConvertColor {
            target: "pso-coated-v3".to_owned(),
        }),
    };
    let step_json = Json::object([
        ("select", Json::object([("op", Json::string("all"))])),
        (
            "action",
            Json::object([
                ("action", Json::string("convert_color")),
                ("target", Json::string("pso-coated-v3")),
            ]),
        ),
    ]);

    assert_json_round_trip(&step, step_json.clone());

    assert_json_round_trip(
        &Recipe {
            schema_version: 1,
            steps: vec![step],
        },
        Json::object([
            ("schema_version", Json::U32(1)),
            ("steps", Json::array([step_json])),
        ]),
    );
}

#[test]
fn patch_plan_mode_has_stable_json_shape() {
    assert_json_round_trip(&PatchPlanMode::NoOp, Json::string("no_op"));
}

#[test]
fn action_plan_has_stable_json_shape() {
    assert_json_round_trip(
        &ActionPlan {
            action: Action::SpreadText(SpreadText {
                amount_pt: 0.25,
                overprint: false,
            }),
            targets: vec![object_id(1)],
            patches: Vec::new(),
            skipped: vec![SkippedTarget {
                object: object_id(2),
                reason: SkipReason::NonProcessColor,
            }],
        },
        Json::object([
            (
                "action",
                Json::object([
                    ("action", Json::string("spread_text")),
                    ("amount_pt", Json::F64(0.25)),
                    ("overprint", Json::Bool(false)),
                ]),
            ),
            ("targets", Json::array([object_id_json(1)])),
            ("patches", Json::array([])),
            (
                "skipped",
                Json::array([Json::object([
                    ("object", object_id_json(2)),
                    (
                        "reason",
                        Json::object([("reason", Json::string("non_process_color"))]),
                    ),
                ])]),
            ),
        ]),
    );
}

#[test]
fn patch_plan_has_stable_json_shape() {
    assert_json_round_trip(
        &PatchPlan {
            mode: PatchPlanMode::NoOp,
            steps: vec![ActionPlan {
                action: Action::MinimumStrokeWidth(MinimumStrokeWidth { width_pt: 0.25 }),
                targets: vec![object_id(1)],
                patches: Vec::new(),
                skipped: Vec::new(),
            }],
        },
        Json::object([
            ("mode", Json::string("no_op")),
            (
                "steps",
                Json::array([Json::object([
                    (
                        "action",
                        Json::object([
                            ("action", Json::string("minimum_stroke_width")),
                            ("width_pt", Json::F64(0.25)),
                        ]),
                    ),
                    ("targets", Json::array([object_id_json(1)])),
                    ("patches", Json::array([])),
                    ("skipped", Json::array([])),
                ])]),
            ),
        ]),
    );
}

#[test]
fn skipped_target_has_stable_json_shape() {
    assert_json_round_trip(
        &SkippedTarget {
            object: object_id(7),
            reason: SkipReason::UnsupportedCapability {
                required: EditCapability::AddTextSpreadStroke,
            },
        },
        Json::object([
            ("object", object_id_json(7)),
            (
                "reason",
                Json::object([
                    ("reason", Json::string("unsupported_capability")),
                    ("required", Json::string("add_text_spread_stroke")),
                ]),
            ),
        ]),
    );
}

#[test]
fn skip_reason_variants_have_stable_json_shape() {
    assert_json_round_trip(
        &SkipReason::UnsupportedCapability {
            required: EditCapability::RewriteColorOperand,
        },
        Json::object([
            ("reason", Json::string("unsupported_capability")),
            ("required", Json::string("rewrite_color_operand")),
        ]),
    );
    assert_json_round_trip(
        &SkipReason::NonProcessColor,
        Json::object([("reason", Json::string("non_process_color"))]),
    );
    assert_json_round_trip(
        &SkipReason::MissingColorSource,
        Json::object([("reason", Json::string("missing_color_source"))]),
    );
    assert_json_round_trip(
        &SkipReason::AmbiguousColorSource,
        Json::object([("reason", Json::string("ambiguous_color_source"))]),
    );
}
