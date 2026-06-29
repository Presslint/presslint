use presslint_core::{ByteRange, ContentScope, EditCapability, ObjectKind, PageIndex};

use super::super::{Action, MutationBoundary, PlannedPatch};
use super::*;

fn content_boundary(page: u32, scope: ContentScope, start: usize, end: usize) -> MutationBoundary {
    MutationBoundary::ContentStream {
        page: PageIndex(page),
        scope,
        range: ByteRange { start, end },
    }
}

fn byte_range_json(start: u32, end: u32) -> Json {
    Json::object([("start", Json::U32(start)), ("end", Json::U32(end))])
}

fn page_scope_json() -> Json {
    Json::object([("kind", Json::string("page"))])
}

fn boundary_json(page: u32, start: u32, end: u32) -> Json {
    Json::object([
        ("kind", Json::string("content_stream")),
        ("page", Json::U32(page)),
        ("scope", page_scope_json()),
        ("range", byte_range_json(start, end)),
    ])
}

fn planned_patch_json(sequence: u32, page: u32, start: u32, end: u32) -> Json {
    Json::object([
        ("object", object_id_json(sequence)),
        ("capability", Json::string("rewrite_color_operand")),
        ("boundary", boundary_json(page, start, end)),
    ])
}

#[test]
fn convert_color_emits_patch_boundary_from_color_source_range() {
    let mut target = color_entry(
        1,
        ObjectKind::Vector,
        [EditCapability::RewriteColorOperand],
        [sourced_fill_color(ColorSpace::DeviceCmyk, 4, 12)],
    );
    target.provenance.page = PageIndex(7);
    target.provenance.scope = ContentScope::AnnotationAppearance;
    target.provenance.range = Some(ByteRange { start: 40, end: 52 });
    let inventory = Inventory {
        entries: vec![target],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert_eq!(step.targets, vec![object_id(1)]);
    assert_eq!(
        step.patches,
        vec![PlannedPatch {
            object: object_id(1),
            capability: EditCapability::RewriteColorOperand,
            boundary: content_boundary(7, ContentScope::AnnotationAppearance, 4, 12),
        }]
    );
    assert!(step.skipped.is_empty());
}

#[test]
fn convert_color_without_boundary_is_skipped_not_planned_for_patch() {
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
    assert!(step.patches.is_empty());
    assert_eq!(
        step.skipped,
        vec![SkippedTarget {
            object: object_id(1),
            reason: SkipReason::MissingColorSource,
        }]
    );
}

#[test]
fn convert_color_patch_boundaries_follow_inventory_order() {
    let inventory = Inventory {
        entries: vec![
            color_entry(
                3,
                ObjectKind::Vector,
                [EditCapability::RewriteColorOperand],
                [sourced_fill_color(ColorSpace::DeviceRgb, 30, 36)],
            ),
            color_entry(
                1,
                ObjectKind::Vector,
                [EditCapability::RewriteColorOperand],
                [sourced_fill_color(ColorSpace::DeviceGray, 10, 13)],
            ),
            color_entry(
                2,
                ObjectKind::Vector,
                [EditCapability::RewriteColorOperand],
                [sourced_fill_color(ColorSpace::DeviceCmyk, 20, 28)],
            ),
        ],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);
    let step = &plan.steps[0];

    assert_eq!(step.targets, vec![object_id(3), object_id(1), object_id(2)]);
    assert_eq!(
        step.patches
            .iter()
            .map(|patch| &patch.object)
            .collect::<Vec<_>>(),
        vec![&object_id(3), &object_id(1), &object_id(2)]
    );
    assert_eq!(
        step.patches
            .iter()
            .map(|patch| &patch.boundary)
            .collect::<Vec<_>>(),
        vec![
            &content_boundary(0, ContentScope::Page, 30, 36),
            &content_boundary(0, ContentScope::Page, 10, 13),
            &content_boundary(0, ContentScope::Page, 20, 28),
        ]
    );
}

#[test]
fn mutation_boundary_has_stable_json_shape() {
    assert_json_round_trip(
        &content_boundary(2, ContentScope::Page, 11, 19),
        boundary_json(2, 11, 19),
    );
}

#[test]
fn planned_patch_has_stable_json_shape() {
    assert_json_round_trip(
        &PlannedPatch {
            object: object_id(4),
            capability: EditCapability::RewriteColorOperand,
            boundary: content_boundary(0, ContentScope::Page, 3, 9),
        },
        planned_patch_json(4, 0, 3, 9),
    );
}

#[test]
fn action_plan_with_patch_boundary_has_stable_json_shape() {
    assert_json_round_trip(
        &ActionPlan {
            action: Action::ConvertColor(ConvertColor {
                target: "pso-coated-v3".to_owned(),
            }),
            targets: vec![object_id(1)],
            patches: vec![PlannedPatch {
                object: object_id(1),
                capability: EditCapability::RewriteColorOperand,
                boundary: content_boundary(0, ContentScope::Page, 5, 12),
            }],
            skipped: Vec::new(),
        },
        Json::object([
            (
                "action",
                Json::object([
                    ("action", Json::string("convert_color")),
                    ("target", Json::string("pso-coated-v3")),
                ]),
            ),
            ("targets", Json::array([object_id_json(1)])),
            ("patches", Json::array([planned_patch_json(1, 0, 5, 12)])),
            ("skipped", Json::array([])),
        ]),
    );
}
