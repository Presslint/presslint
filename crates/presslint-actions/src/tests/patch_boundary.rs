use presslint_pdf::{
    IndirectObjectEditDecision, IndirectObjectEditDisposition, IndirectObjectOwnership,
    IndirectRef, PageRectangle,
};
use presslint_types::{ByteRange, ContentScope, EditCapability, ObjectKind, PageIndex, PdfName};

use super::super::{
    Action, DictionaryEntryOp, DictionaryValueLocator, MutationBoundary, PlannedObjectAllocation,
    PlannedPatch, PlannedValueProvenance, SetPageBox, plan_set_page_box_boundaries,
};
use super::*;

fn content_boundary(page: u32, scope: ContentScope, start: usize, end: usize) -> MutationBoundary {
    MutationBoundary::ContentStreamOperand {
        page: PageIndex(page),
        scope,
        record_range: ByteRange { start, end },
        operand_range: None,
        operator_range: None,
        ownership: None,
        value_provenance: PlannedValueProvenance::ActionGenerated {
            action: convert_color_action(),
        },
    }
}

fn convert_color_action() -> Action {
    Action::ConvertColor(ConvertColor {
        target: "pso-coated-v3".to_owned(),
    })
}

fn byte_range_json(start: u32, end: u32) -> Json {
    Json::object([("start", Json::U32(start)), ("end", Json::U32(end))])
}

fn page_scope_json() -> Json {
    Json::object([("kind", Json::string("page"))])
}

fn boundary_json(page: u32, start: u32, end: u32) -> Json {
    Json::object([
        ("kind", Json::string("content_stream_operand")),
        ("page", Json::U32(page)),
        ("scope", page_scope_json()),
        ("record_range", byte_range_json(start, end)),
        ("operand_range", Json::Null),
        ("operator_range", Json::Null),
        ("ownership", Json::Null),
        ("value_provenance", action_generated_json()),
    ])
}

fn action_generated_json() -> Json {
    Json::object([
        ("kind", Json::string("action_generated")),
        ("action", action_json()),
    ])
}

fn action_json() -> Json {
    Json::object([
        ("action", Json::string("convert_color")),
        ("target", Json::string("pso-coated-v3")),
    ])
}

fn planned_patch_json(sequence: u32, page: u32, start: u32, end: u32) -> Json {
    Json::object([
        ("object", object_id_json(sequence)),
        ("capability", Json::string("rewrite_color_operand")),
        ("boundary", boundary_json(page, start, end)),
    ])
}

fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

fn indirect_ref_json(object_number: u32, generation: u32) -> Json {
    Json::object([
        ("object_number", Json::U32(object_number)),
        ("generation", Json::U32(generation)),
    ])
}

fn single_use_ownership(target: IndirectRef, owner: IndirectRef) -> IndirectObjectEditDecision {
    IndirectObjectEditDecision {
        target,
        ownership: IndirectObjectOwnership::ProvenSingleUse { owner },
        disposition: IndirectObjectEditDisposition::InPlaceMutation,
    }
}

fn shared_ownership(
    target: IndirectRef,
    consumers: Vec<IndirectRef>,
) -> IndirectObjectEditDecision {
    IndirectObjectEditDecision {
        target,
        ownership: IndirectObjectOwnership::Shared { consumers },
        disposition: IndirectObjectEditDisposition::PrivateCopy,
    }
}

fn unproven_ownership(target: IndirectRef) -> IndirectObjectEditDecision {
    IndirectObjectEditDecision {
        target,
        ownership: IndirectObjectOwnership::Unproven,
        disposition: IndirectObjectEditDisposition::PrivateCopy,
    }
}

fn single_use_ownership_json(target: (u32, u32), owner: (u32, u32)) -> Json {
    Json::object([
        ("target", indirect_ref_json(target.0, target.1)),
        (
            "ownership",
            Json::object([
                ("status", Json::string("proven_single_use")),
                ("owner", indirect_ref_json(owner.0, owner.1)),
            ]),
        ),
        ("disposition", Json::string("in_place_mutation")),
    ])
}

fn shared_ownership_json(
    target: (u32, u32),
    consumers: impl IntoIterator<Item = (u32, u32)>,
) -> Json {
    Json::object([
        ("target", indirect_ref_json(target.0, target.1)),
        (
            "ownership",
            Json::object([
                ("status", Json::string("shared")),
                (
                    "consumers",
                    Json::array(
                        consumers
                            .into_iter()
                            .map(|consumer| indirect_ref_json(consumer.0, consumer.1)),
                    ),
                ),
            ]),
        ),
        ("disposition", Json::string("private_copy")),
    ])
}

fn unproven_ownership_json(target: (u32, u32)) -> Json {
    Json::object([
        ("target", indirect_ref_json(target.0, target.1)),
        (
            "ownership",
            Json::object([("status", Json::string("unproven"))]),
        ),
        ("disposition", Json::string("private_copy")),
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
fn convert_color_content_stream_operand_carries_action_provenance_and_no_fake_ownership() {
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [sourced_fill_color(ColorSpace::DeviceRgb, 9, 16)],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };

    let plan = plan_recipe(&recipe, &inventory);

    assert_eq!(
        plan.steps[0].patches[0].boundary,
        MutationBoundary::ContentStreamOperand {
            page: PageIndex(0),
            scope: ContentScope::Page,
            record_range: ByteRange { start: 9, end: 16 },
            operand_range: None,
            operator_range: None,
            ownership: None,
            value_provenance: PlannedValueProvenance::ActionGenerated {
                action: convert_color_action(),
            },
        }
    );
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
fn dictionary_entry_boundary_has_stable_json_shape() {
    let target = indirect_ref(12, 0);
    let owner = indirect_ref(3, 0);
    let value = MutationBoundary::DictionaryEntry {
        target,
        key: PdfName(b"ColorSpace".to_vec()),
        op: DictionaryEntryOp::Replace,
        value_locator: DictionaryValueLocator::ExistingValue {
            key_range: ByteRange { start: 20, end: 31 },
            value_range: ByteRange { start: 32, end: 45 },
        },
        ownership: single_use_ownership(target, owner),
        value_provenance: PlannedValueProvenance::DerivedFromObject {
            object: indirect_ref(44, 2),
        },
    };

    assert_json_round_trip(
        &value,
        Json::object([
            ("kind", Json::string("dictionary_entry")),
            ("target", indirect_ref_json(12, 0)),
            (
                "key",
                Json::array(
                    b"ColorSpace"
                        .iter()
                        .copied()
                        .map(|byte| Json::U32(u32::from(byte))),
                ),
            ),
            ("op", Json::string("replace")),
            (
                "value_locator",
                Json::object([
                    ("kind", Json::string("existing_value")),
                    ("key_range", byte_range_json(20, 31)),
                    ("value_range", byte_range_json(32, 45)),
                ]),
            ),
            ("ownership", single_use_ownership_json((12, 0), (3, 0))),
            (
                "value_provenance",
                Json::object([
                    ("kind", Json::string("derived_from_object")),
                    ("object", indirect_ref_json(44, 2)),
                ]),
            ),
        ]),
    );
}

#[test]
fn whole_stream_boundary_has_stable_json_shape() {
    let target = indirect_ref(20, 0);
    let value = MutationBoundary::WholeStream {
        target,
        stream_data_range: Some(ByteRange {
            start: 100,
            end: 180,
        }),
        ownership: shared_ownership(target, vec![indirect_ref(2, 0), indirect_ref(5, 0)]),
        value_provenance: PlannedValueProvenance::ExternalPolicy {
            name: "output-intent".to_owned(),
        },
    };

    assert_json_round_trip(
        &value,
        Json::object([
            ("kind", Json::string("whole_stream")),
            ("target", indirect_ref_json(20, 0)),
            ("stream_data_range", byte_range_json(100, 180)),
            (
                "ownership",
                shared_ownership_json((20, 0), [(2, 0), (5, 0)]),
            ),
            (
                "value_provenance",
                Json::object([
                    ("kind", Json::string("external_policy")),
                    ("name", Json::string("output-intent")),
                ]),
            ),
        ]),
    );
}

#[test]
fn indirect_object_clone_boundary_has_stable_json_shape() {
    let source = indirect_ref(30, 0);
    let consumer = indirect_ref(7, 0);
    let reference_patch = MutationBoundary::DictionaryEntry {
        target: consumer,
        key: PdfName(b"XObject".to_vec()),
        op: DictionaryEntryOp::Insert,
        value_locator: DictionaryValueLocator::InsertionPoint {
            dictionary_range: ByteRange {
                start: 200,
                end: 260,
            },
        },
        ownership: unproven_ownership(consumer),
        value_provenance: PlannedValueProvenance::ExternalPolicy {
            name: "private-resource-reference".to_owned(),
        },
    };
    let value = MutationBoundary::IndirectObjectClone {
        source,
        consumer,
        new_object: PlannedObjectAllocation::AppendNew { object_number: 91 },
        reference_patch: Box::new(reference_patch),
        ownership: shared_ownership(source, vec![consumer, indirect_ref(8, 0)]),
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: source },
    };

    assert_json_round_trip(
        &value,
        Json::object([
            ("kind", Json::string("indirect_object_clone")),
            ("source", indirect_ref_json(30, 0)),
            ("consumer", indirect_ref_json(7, 0)),
            (
                "new_object",
                Json::object([
                    ("kind", Json::string("append_new")),
                    ("object_number", Json::U32(91)),
                ]),
            ),
            (
                "reference_patch",
                Json::object([
                    ("kind", Json::string("dictionary_entry")),
                    ("target", indirect_ref_json(7, 0)),
                    (
                        "key",
                        Json::array(
                            b"XObject"
                                .iter()
                                .copied()
                                .map(|byte| Json::U32(u32::from(byte))),
                        ),
                    ),
                    ("op", Json::string("insert")),
                    (
                        "value_locator",
                        Json::object([
                            ("kind", Json::string("insertion_point")),
                            ("dictionary_range", byte_range_json(200, 260)),
                        ]),
                    ),
                    ("ownership", unproven_ownership_json((7, 0))),
                    (
                        "value_provenance",
                        Json::object([
                            ("kind", Json::string("external_policy")),
                            ("name", Json::string("private-resource-reference")),
                        ]),
                    ),
                ]),
            ),
            (
                "ownership",
                shared_ownership_json((30, 0), [(7, 0), (8, 0)]),
            ),
            (
                "value_provenance",
                Json::object([
                    ("kind", Json::string("derived_from_object")),
                    ("object", indirect_ref_json(30, 0)),
                ]),
            ),
        ]),
    );
}

#[test]
fn supporting_boundary_enums_have_stable_json_shape() {
    assert_json_round_trip(&DictionaryEntryOp::Insert, Json::string("insert"));
    assert_json_round_trip(
        &DictionaryValueLocator::InsertionPoint {
            dictionary_range: ByteRange { start: 1, end: 9 },
        },
        Json::object([
            ("kind", Json::string("insertion_point")),
            ("dictionary_range", byte_range_json(1, 9)),
        ]),
    );
    assert_json_round_trip(
        &PlannedValueProvenance::ExternalPolicy {
            name: "policy".to_owned(),
        },
        Json::object([
            ("kind", Json::string("external_policy")),
            ("name", Json::string("policy")),
        ]),
    );
    assert_json_round_trip(
        &PlannedObjectAllocation::Deferred,
        Json::object([("kind", Json::string("deferred"))]),
    );
}

#[test]
fn patch_boundary_report_retains_no_source_bytes() {
    let mut secret_color = sourced_fill_color(ColorSpace::DeviceCmyk, 4, 18);
    secret_color.spot_name = Some(PdfName(b"/Secret".to_vec()));
    let inventory = Inventory {
        entries: vec![color_entry(
            1,
            ObjectKind::Vector,
            [EditCapability::RewriteColorOperand],
            [secret_color],
        )],
    };
    let recipe = Recipe {
        schema_version: 1,
        steps: vec![convert_color_step(Selector::All)],
    };
    let plan = plan_recipe(&recipe, &inventory);

    let encoded = plan
        .serialize(JsonSerializer)
        .expect("serialize patch plan with boundary");

    assert!(!json_contains_string(&encoded, "/Secret"));
    assert!(!json_contains_byte_sequence(&encoded, b"/Secret"));
}

fn json_contains_string(value: &Json, needle: &str) -> bool {
    match value {
        Json::Object(fields) => fields
            .iter()
            .any(|(key, value)| key.contains(needle) || json_contains_string(value, needle)),
        Json::Array(values) => values
            .iter()
            .any(|value| json_contains_string(value, needle)),
        Json::String(value) => value.contains(needle),
        Json::U32(_) | Json::F64(_) | Json::Bool(_) | Json::Null => false,
    }
}

fn json_contains_byte_sequence(value: &Json, needle: &[u8]) -> bool {
    match value {
        Json::Object(fields) => fields
            .iter()
            .any(|(_, value)| json_contains_byte_sequence(value, needle)),
        Json::Array(values) => {
            let bytes = values
                .iter()
                .map(|value| match value {
                    Json::U32(value) => u8::try_from(*value).ok(),
                    _ => None,
                })
                .collect::<Option<Vec<_>>>();

            bytes.as_deref().is_some_and(|bytes| {
                needle.is_empty() || bytes.windows(needle.len()).any(|window| window == needle)
            }) || values
                .iter()
                .any(|value| json_contains_byte_sequence(value, needle))
        }
        Json::String(_) | Json::U32(_) | Json::F64(_) | Json::Bool(_) | Json::Null => false,
    }
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
            ("action", action_json()),
            ("targets", Json::array([object_id_json(1)])),
            ("patches", Json::array([planned_patch_json(1, 0, 5, 12)])),
            ("skipped", Json::array([])),
        ]),
    );
}

fn page_rect(llx: f64, lly: f64, urx: f64, ury: f64) -> PageRectangle {
    PageRectangle { llx, lly, urx, ury }
}

#[test]
fn set_page_box_plans_replace_and_insert_dictionary_entries() {
    let target = indirect_ref(4, 0);
    let ownership = single_use_ownership(target, indirect_ref(2, 0));
    let action = SetPageBox {
        media_box: Some(page_rect(0.0, 0.0, 612.0, 792.0)),
        crop_box: Some(page_rect(10.0, 10.0, 400.0, 400.0)),
    };
    let media_locator = DictionaryValueLocator::ExistingValue {
        key_range: ByteRange { start: 10, end: 19 },
        value_range: ByteRange { start: 20, end: 42 },
    };
    let crop_locator = DictionaryValueLocator::InsertionPoint {
        dictionary_range: ByteRange { start: 8, end: 60 },
    };

    let boundaries = plan_set_page_box_boundaries(
        &action,
        target,
        &ownership,
        Some(media_locator),
        Some(crop_locator),
    );

    // MediaBox precedes CropBox; the op is derived from the locator variant.
    assert_eq!(
        boundaries,
        vec![
            MutationBoundary::DictionaryEntry {
                target,
                key: PdfName(b"MediaBox".to_vec()),
                op: DictionaryEntryOp::Replace,
                value_locator: media_locator,
                ownership: ownership.clone(),
                value_provenance: PlannedValueProvenance::ActionGenerated {
                    action: Action::SetPageBox(action),
                },
            },
            MutationBoundary::DictionaryEntry {
                target,
                key: PdfName(b"CropBox".to_vec()),
                op: DictionaryEntryOp::Insert,
                value_locator: crop_locator,
                ownership,
                value_provenance: PlannedValueProvenance::ActionGenerated {
                    action: Action::SetPageBox(action),
                },
            },
        ]
    );
}

#[test]
fn set_page_box_emits_boundary_only_when_requested_and_located() {
    let target = indirect_ref(4, 0);
    let ownership = single_use_ownership(target, indirect_ref(2, 0));
    let media_locator = DictionaryValueLocator::ExistingValue {
        key_range: ByteRange { start: 10, end: 19 },
        value_range: ByteRange { start: 20, end: 42 },
    };
    let crop_locator = DictionaryValueLocator::InsertionPoint {
        dictionary_range: ByteRange { start: 8, end: 60 },
    };

    // Media requested but not located: no boundary. Crop located but not
    // requested: no boundary. Result is empty.
    let media_only = SetPageBox {
        media_box: Some(page_rect(0.0, 0.0, 612.0, 792.0)),
        crop_box: None,
    };
    assert!(
        plan_set_page_box_boundaries(&media_only, target, &ownership, None, Some(crop_locator))
            .is_empty()
    );

    // Media requested and located; crop neither requested nor located.
    let boundaries =
        plan_set_page_box_boundaries(&media_only, target, &ownership, Some(media_locator), None);
    assert_eq!(boundaries.len(), 1);
    assert!(matches!(
        &boundaries[0],
        MutationBoundary::DictionaryEntry {
            key,
            op: DictionaryEntryOp::Replace,
            ..
        } if *key == PdfName(b"MediaBox".to_vec())
    ));
}

#[test]
fn set_page_box_carries_shared_ownership_for_report_only_planning() {
    let target = indirect_ref(9, 0);
    let ownership = shared_ownership(target, vec![indirect_ref(2, 0), indirect_ref(5, 0)]);
    let action = SetPageBox {
        media_box: None,
        crop_box: Some(page_rect(0.0, 0.0, 100.0, 100.0)),
    };
    let crop_locator = DictionaryValueLocator::InsertionPoint {
        dictionary_range: ByteRange { start: 8, end: 60 },
    };

    let boundaries =
        plan_set_page_box_boundaries(&action, target, &ownership, None, Some(crop_locator));

    // The planner records the ownership decision verbatim; it does not gate on
    // the disposition (that is the writer's responsibility) and it never writes.
    assert_eq!(
        boundaries,
        vec![MutationBoundary::DictionaryEntry {
            target,
            key: PdfName(b"CropBox".to_vec()),
            op: DictionaryEntryOp::Insert,
            value_locator: crop_locator,
            ownership,
            value_provenance: PlannedValueProvenance::ActionGenerated {
                action: Action::SetPageBox(action),
            },
        }]
    );
}
