use presslint_syntax::{OperatorRecord, TokenRef, assemble_operators, tokenize};
use presslint_types::{
    ByteRange, ColorSpace, ColorUsage, ContentScope, EditCapability, ObjectKind, PageIndex, PdfName,
};

mod bit_identity;
mod combined;
mod extgstate;
mod json;
mod resource_color;
mod serde_shape;
mod xobject;

use super::{
    GraphicsStateWalker, GraphicsWalkError, GraphicsWalkErrorKind, Inventory, PaintOpKind,
    PathPaintKind, TextRenderingMode, TextShowOperator, build_form_inventory,
    build_image_inventory, build_inventory, build_text_inventory, build_vector_inventory,
    walk_graphics_state,
};

fn walk(input: &[u8]) -> Result<Vec<super::PaintOp>, GraphicsWalkError> {
    let tokens = tokenize(input).map_err(|error| {
        GraphicsWalkError::new(GraphicsWalkErrorKind::InvalidSourceRange, error.range)
    })?;
    let assembled = assemble_operators(&tokens).map_err(|error| {
        let range = match error {
            presslint_syntax::AssembleError::InvalidTokenRange { range, .. }
            | presslint_syntax::AssembleError::TrailingOperands { range, .. }
            | presslint_syntax::AssembleError::UnmatchedArrayClose { range, .. }
            | presslint_syntax::AssembleError::UnmatchedDictionaryClose { range, .. }
            | presslint_syntax::AssembleError::MismatchedDelimiter { range, .. }
            | presslint_syntax::AssembleError::UnterminatedCompositeOperand { range, .. }
            | presslint_syntax::AssembleError::OperatorInsideCompositeOperand { range, .. }
            | presslint_syntax::AssembleError::UnexpectedKeyword { range, .. } => range,
        };
        GraphicsWalkError::new(GraphicsWalkErrorKind::InvalidSourceRange, range)
    })?;
    walk_graphics_state(input, &assembled.records)
}

fn vector_inventory(input: &[u8], scope: &ContentScope) -> Result<Inventory, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    build_vector_inventory(input, &assembled.records, PageIndex(2), scope)
        .map_err(|error| format!("{error:?}"))
}

fn text_inventory(input: &[u8], scope: &ContentScope) -> Result<Inventory, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    build_text_inventory(input, &assembled.records, PageIndex(2), scope)
        .map_err(|error| format!("{error:?}"))
}

fn image_inventory(
    input: &[u8],
    scope: &ContentScope,
    image_names: &[PdfName],
) -> Result<Inventory, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    build_image_inventory(input, &assembled.records, PageIndex(2), scope, image_names)
        .map_err(|error| format!("{error:?}"))
}

fn form_inventory(
    input: &[u8],
    scope: &ContentScope,
    form_names: &[PdfName],
) -> Result<Inventory, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    build_form_inventory(input, &assembled.records, PageIndex(2), scope, form_names)
        .map_err(|error| format!("{error:?}"))
}

fn combined_inventory(
    input: &[u8],
    scope: &ContentScope,
    image_names: &[PdfName],
    form_names: &[PdfName],
) -> Result<Inventory, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    build_inventory(
        input,
        &assembled.records,
        PageIndex(2),
        scope,
        image_names,
        form_names,
    )
    .map_err(|error| format!("{error:?}"))
}

fn assert_ctm_near(actual: [f64; 6], expected: [f64; 6]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!((actual - expected).abs() < 1e-12);
    }
}

#[test]
fn save_restore_returns_to_saved_colour_state() -> Result<(), String> {
    let events = walk(b"1 0 0 rg q 0.5 g Q S").map_err(|error| format!("{error:?}"))?;
    let final_event = events.last().ok_or("missing final event")?;

    assert_eq!(
        final_event.state.nonstroking_color.space,
        ColorSpace::DeviceRgb
    );
    assert_eq!(
        final_event.state.nonstroking_color.components,
        vec![1.0, 0.0, 0.0]
    );
    assert_eq!(
        final_event.kind,
        PaintOpKind::PathPaint {
            paint: PathPaintKind::Stroke,
        }
    );
    Ok(())
}

#[test]
fn cm_concatenates_current_transformation_matrix() -> Result<(), String> {
    let events = walk(b"1 0 0 1 10 0 cm 1 0 0 1 0 5 cm").map_err(|error| format!("{error:?}"))?;
    let final_event = events.last().ok_or("missing final event")?;

    assert_ctm_near(final_event.state.ctm, [1.0, 0.0, 0.0, 1.0, 10.0, 5.0]);
    Ok(())
}

#[test]
fn device_colour_observations_track_stroke_and_fill() -> Result<(), String> {
    let events =
        walk(b"0.1 0.2 0.3 RG 0.4 0.5 0.6 0.7 k B").map_err(|error| format!("{error:?}"))?;
    let final_event = events.last().ok_or("missing final event")?;
    let stroke = final_event.state.stroke_observation();
    let fill = final_event.state.fill_observation();

    assert_eq!(stroke.usage, ColorUsage::Stroke);
    assert_eq!(stroke.space, ColorSpace::DeviceRgb);
    assert_eq!(stroke.components, vec![0.1, 0.2, 0.3]);
    assert_eq!(fill.usage, ColorUsage::Fill);
    assert_eq!(fill.space, ColorSpace::DeviceCmyk);
    assert_eq!(fill.components, vec![0.4, 0.5, 0.6, 0.7]);
    Ok(())
}

#[test]
fn path_paint_event_carries_post_operator_snapshot_and_provenance() -> Result<(), String> {
    let events = walk(b"0.25 g 2 0 0 2 8 9 cm f*").map_err(|error| format!("{error:?}"))?;
    let event = events.last().ok_or("missing path event")?;

    assert_eq!(
        event.kind,
        PaintOpKind::PathPaint {
            paint: PathPaintKind::FillEvenOdd,
        }
    );
    assert_ctm_near(event.state.ctm, [2.0, 0.0, 0.0, 2.0, 8.0, 9.0]);
    assert_eq!(
        event.state.nonstroking_color,
        super::GraphicsColor {
            space: ColorSpace::DeviceGray,
            components: vec![0.25],
            resource_name: None,
            spot_name: None,
            source: Some(ByteRange { start: 0, end: 6 }),
        }
    );
    assert_eq!(event.record_range.start, 22);
    assert_eq!(event.operator_range.end, 24);
    Ok(())
}

#[test]
fn unsupported_operator_emits_noop_event() -> Result<(), String> {
    let events = walk(b"10 20 m").map_err(|error| format!("{error:?}"))?;

    assert_eq!(events[0].kind, PaintOpKind::NoOp);
    assert_eq!(
        events[0].state.as_ref(),
        &super::GraphicsStateSnapshot::page_default()
    );
    Ok(())
}

#[test]
fn do_operator_emits_xobject_invocation_event() -> Result<(), String> {
    let events = walk(b"/Im1 Do").map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        events[0].kind,
        PaintOpKind::XObjectInvoke {
            name: PdfName(b"Im1".to_vec()),
        }
    );
    assert_eq!(events[0].record_range, ByteRange { start: 0, end: 7 });
    assert_eq!(
        events[0].state.as_ref(),
        &super::GraphicsStateSnapshot::page_default()
    );
    Ok(())
}

#[test]
fn invalid_record_range_returns_structured_error() -> Result<(), String> {
    let mut walker = GraphicsStateWalker::new();
    let record = OperatorRecord {
        operator: TokenRef {
            token_index: 0,
            range: presslint_types::ByteRange { start: 0, end: 1 },
        },
        operands: Vec::new(),
        trivia: Vec::new(),
        range: presslint_types::ByteRange { start: 2, end: 1 },
    };

    let Err(err) = walker.step(b"m", 0, &record) else {
        return Err("invalid record range should fail".to_string());
    };

    assert_eq!(
        err,
        GraphicsWalkError::new(
            GraphicsWalkErrorKind::InvalidSourceRange,
            presslint_types::ByteRange { start: 2, end: 1 },
        )
    );
    Ok(())
}

#[test]
fn stack_underflow_returns_structured_error() -> Result<(), String> {
    let Err(err) = walk(b"Q") else {
        return Err("Q without q should fail".to_string());
    };

    assert_eq!(
        err,
        GraphicsWalkError::new(
            GraphicsWalkErrorKind::GraphicsStateStackUnderflow,
            presslint_types::ByteRange { start: 0, end: 1 },
        )
    );
    Ok(())
}

#[test]
fn malformed_operand_count_returns_structured_error() -> Result<(), String> {
    let Err(err) = walk(b"1 2 RG") else {
        return Err("RG with two operands should fail".to_string());
    };

    assert_eq!(
        err.kind,
        GraphicsWalkErrorKind::MalformedOperandCount {
            operator: b"RG".to_vec(),
            expected: 3,
            got: 2,
        }
    );
    Ok(())
}

#[test]
fn malformed_numeric_operand_returns_structured_error() -> Result<(), String> {
    let Err(err) = walk(b"/Name g") else {
        return Err("name operand should fail".to_string());
    };

    assert_eq!(
        err.kind,
        GraphicsWalkErrorKind::MalformedNumericOperand {
            operator: b"g".to_vec(),
            operand_index: 0,
        }
    );
    Ok(())
}

#[test]
fn malformed_do_operand_count_returns_structured_error() -> Result<(), String> {
    let Err(err) = walk(b"/Im1 /Im2 Do") else {
        return Err("Do with two operands should fail".to_string());
    };

    assert_eq!(
        err.kind,
        GraphicsWalkErrorKind::MalformedOperandCount {
            operator: b"Do".to_vec(),
            expected: 1,
            got: 2,
        }
    );
    Ok(())
}

#[test]
fn malformed_do_name_operand_returns_structured_error() -> Result<(), String> {
    let Err(err) = walk(b"42 Do") else {
        return Err("Do with non-name operand should fail".to_string());
    };

    assert_eq!(
        err.kind,
        GraphicsWalkErrorKind::MalformedNameOperand {
            operator: b"Do".to_vec(),
            operand_index: 0,
        }
    );
    assert_eq!(err.range, ByteRange { start: 0, end: 2 });
    Ok(())
}

#[test]
fn vector_inventory_attaches_color_observations_by_paint_usage() -> Result<(), String> {
    let inventory = vector_inventory(
        b"0.1 0.2 0.3 RG S 0.4 g f 0 0 0 1 K 0.5 0.6 0.7 rg B",
        &ContentScope::Page,
    )?;

    assert_eq!(inventory.entries.len(), 3);
    assert_eq!(inventory.entries[0].kind, ObjectKind::Vector);
    assert_eq!(inventory.entries[0].colors.len(), 1);
    assert_eq!(inventory.entries[0].colors[0].usage, ColorUsage::Stroke);
    assert_eq!(inventory.entries[0].colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(
        inventory.entries[0].colors[0].components,
        vec![0.1, 0.2, 0.3]
    );

    assert_eq!(inventory.entries[1].colors.len(), 1);
    assert_eq!(inventory.entries[1].colors[0].usage, ColorUsage::Fill);
    assert_eq!(inventory.entries[1].colors[0].space, ColorSpace::DeviceGray);
    assert_eq!(inventory.entries[1].colors[0].components, vec![0.4]);

    assert_eq!(inventory.entries[2].colors.len(), 2);
    assert_eq!(inventory.entries[2].colors[0].usage, ColorUsage::Stroke);
    assert_eq!(inventory.entries[2].colors[0].space, ColorSpace::DeviceCmyk);
    assert_eq!(
        inventory.entries[2].colors[0].components,
        vec![0.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(inventory.entries[2].colors[1].usage, ColorUsage::Fill);
    assert_eq!(inventory.entries[2].colors[1].space, ColorSpace::DeviceRgb);
    assert_eq!(
        inventory.entries[2].colors[1].components,
        vec![0.5, 0.6, 0.7]
    );
    Ok(())
}

#[test]
fn vector_inventory_carries_provenance_and_edit_capability() -> Result<(), String> {
    let scope = ContentScope::FormXObject {
        name: PdfName(b"Logo".to_vec()),
    };
    let inventory = vector_inventory(b"0.25 g f", &scope)?;
    let entry = inventory.entries.first().ok_or("missing vector entry")?;

    assert_eq!(entry.provenance.page, PageIndex(2));
    assert_eq!(entry.provenance.scope, scope);
    assert_eq!(entry.provenance.range, Some(ByteRange { start: 7, end: 8 }));
    assert_eq!(entry.bounds, None);
    assert_eq!(
        entry.capabilities,
        vec![EditCapability::RewriteColorOperand]
    );
    Ok(())
}

#[test]
fn vector_inventory_object_ids_are_deterministic() -> Result<(), String> {
    let first = vector_inventory(b"S f B", &ContentScope::Page)?;
    let second = vector_inventory(b"S f B", &ContentScope::Page)?;

    assert_eq!(first, second);
    assert_eq!(first.entries[0].id.page, PageIndex(2));
    assert_eq!(first.entries[0].id.sequence, 0);
    assert_eq!(first.entries[1].id.sequence, 1);
    assert_eq!(first.entries[2].id.sequence, 2);
    assert_ne!(first.entries[0].id.digest, first.entries[1].id.digest);
    assert_ne!(first.entries[1].id.digest, first.entries[2].id.digest);
    Ok(())
}

#[test]
fn vector_inventory_skips_noop_and_end_path_events() -> Result<(), String> {
    let inventory = vector_inventory(b"10 20 m n", &ContentScope::Page)?;

    assert!(inventory.is_empty());
    Ok(())
}

#[test]
fn default_filled_text_inventory_uses_nonstroking_color() -> Result<(), String> {
    let scope = ContentScope::FormXObject {
        name: PdfName(b"Body".to_vec()),
    };
    let inventory = text_inventory(b"0.2 0.3 0.4 rg (Hello) Tj", &scope)?;
    let entry = inventory.entries.first().ok_or("missing text entry")?;

    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(entry.kind, ObjectKind::Text);
    assert_eq!(entry.provenance.page, PageIndex(2));
    assert_eq!(entry.provenance.scope, scope);
    assert_eq!(
        entry.provenance.range,
        Some(ByteRange { start: 15, end: 25 })
    );
    assert_eq!(entry.bounds, None);
    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Fill);
    assert_eq!(entry.colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(entry.colors[0].components, vec![0.2, 0.3, 0.4]);
    assert_eq!(
        entry.capabilities,
        vec![
            EditCapability::RewriteColorOperand,
            EditCapability::AddTextSpreadStroke,
        ]
    );
    Ok(())
}

#[test]
fn stroked_text_rendering_mode_uses_stroking_color() -> Result<(), String> {
    let inventory = text_inventory(b"0.7 G 1 Tr (Outline) Tj", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing text entry")?;

    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Stroke);
    assert_eq!(entry.colors[0].space, ColorSpace::DeviceGray);
    assert_eq!(entry.colors[0].components, vec![0.7]);
    assert_eq!(
        entry.capabilities,
        vec![
            EditCapability::RewriteColorOperand,
            EditCapability::AddTextSpreadStroke,
        ]
    );
    Ok(())
}

#[test]
fn fill_and_stroke_text_rendering_mode_uses_both_colors() -> Result<(), String> {
    let inventory = text_inventory(
        b"0.1 0.2 0.3 RG 0.4 0.5 0.6 rg 2 Tr [(Hi) 20 (There)] TJ",
        &ContentScope::Page,
    )?;
    let entry = inventory.entries.first().ok_or("missing text entry")?;

    assert_eq!(entry.colors.len(), 2);
    assert_eq!(entry.colors[0].usage, ColorUsage::Stroke);
    assert_eq!(entry.colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(entry.colors[0].components, vec![0.1, 0.2, 0.3]);
    assert_eq!(entry.colors[1].usage, ColorUsage::Fill);
    assert_eq!(entry.colors[1].space, ColorSpace::DeviceRgb);
    assert_eq!(entry.colors[1].components, vec![0.4, 0.5, 0.6]);
    Ok(())
}

#[test]
fn invisible_text_is_represented_without_color_edit_capability() -> Result<(), String> {
    let inventory = text_inventory(b"3 Tr (Hidden) Tj", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing text entry")?;

    assert_eq!(entry.kind, ObjectKind::Text);
    assert!(entry.colors.is_empty());
    assert!(entry.capabilities.is_empty());
    Ok(())
}

#[test]
fn unsupported_text_rendering_mode_is_conservative() -> Result<(), String> {
    let events = walk(b"4 Tr (ClipFill) Tj").map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        events[0].kind,
        PaintOpKind::SetTextRenderingMode {
            mode: TextRenderingMode::Unsupported { value: 4 },
        }
    );
    assert_eq!(
        events[1].kind,
        PaintOpKind::TextShow {
            operator: TextShowOperator::ShowText,
            rendering_mode: TextRenderingMode::Unsupported { value: 4 },
        }
    );

    let inventory = text_inventory(b"4 Tr (ClipFill) Tj", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing text entry")?;

    assert_eq!(entry.kind, ObjectKind::Text);
    assert!(entry.colors.is_empty());
    assert!(entry.capabilities.is_empty());
    Ok(())
}

#[test]
fn quoted_text_showing_operators_are_inventoried() -> Result<(), String> {
    let inventory = text_inventory(b"(Next) ' 4 2 (Spaced) \"", &ContentScope::Page)?;

    assert_eq!(inventory.entries.len(), 2);
    assert_eq!(inventory.entries[0].id.sequence, 0);
    assert_eq!(
        inventory.entries[0].provenance.range,
        Some(ByteRange { start: 0, end: 8 })
    );
    assert_eq!(inventory.entries[1].id.sequence, 1);
    assert_eq!(
        inventory.entries[1].provenance.range,
        Some(ByteRange { start: 9, end: 23 })
    );
    Ok(())
}

#[test]
fn text_inventory_object_ids_are_deterministic() -> Result<(), String> {
    let first = text_inventory(b"(A) Tj 1 Tr (B) Tj 2 Tr [(C)] TJ", &ContentScope::Page)?;
    let second = text_inventory(b"(A) Tj 1 Tr (B) Tj 2 Tr [(C)] TJ", &ContentScope::Page)?;

    assert_eq!(first, second);
    assert_eq!(first.entries[0].id.page, PageIndex(2));
    assert_eq!(first.entries[0].id.sequence, 0);
    assert_eq!(first.entries[1].id.sequence, 1);
    assert_eq!(first.entries[2].id.sequence, 2);
    assert_ne!(first.entries[0].id.digest, first.entries[1].id.digest);
    assert_ne!(first.entries[1].id.digest, first.entries[2].id.digest);
    Ok(())
}

#[test]
fn image_inventory_includes_only_declared_image_xobject_names() -> Result<(), String> {
    let inventory = image_inventory(
        b"/Im1 Do /Fm1 Do /Im2 Do",
        &ContentScope::Page,
        &[
            PdfName(b"Im2".to_vec()),
            PdfName(b"Missing".to_vec()),
            PdfName(b"Im1".to_vec()),
        ],
    )?;

    assert_eq!(inventory.entries.len(), 2);
    assert_eq!(inventory.entries[0].kind, ObjectKind::Image);
    assert_eq!(inventory.entries[0].id.sequence, 0);
    assert_eq!(
        inventory.entries[0].provenance.range,
        Some(ByteRange { start: 0, end: 7 })
    );
    assert_eq!(inventory.entries[1].id.sequence, 1);
    assert_eq!(
        inventory.entries[1].provenance.range,
        Some(ByteRange { start: 16, end: 23 })
    );
    Ok(())
}

#[test]
fn image_inventory_carries_conservative_observation_and_read_only_capability() -> Result<(), String>
{
    let scope = ContentScope::AnnotationAppearance;
    let inventory = image_inventory(b"q /Photo Do Q", &scope, &[PdfName(b"Photo".to_vec())])?;
    let entry = inventory.entries.first().ok_or("missing image entry")?;

    assert_eq!(entry.provenance.page, PageIndex(2));
    assert_eq!(entry.provenance.scope, scope);
    assert_eq!(
        entry.provenance.range,
        Some(ByteRange { start: 2, end: 11 })
    );
    assert_eq!(entry.bounds, None);
    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Image);
    assert_eq!(entry.colors[0].space, ColorSpace::Unknown);
    assert!(entry.colors[0].components.is_empty());
    assert_eq!(entry.colors[0].spot_name, None);
    assert_eq!(entry.capabilities, vec![EditCapability::ReadOnly]);
    Ok(())
}

#[test]
fn non_image_xobject_invocations_are_skipped_by_image_inventory() -> Result<(), String> {
    let inventory = image_inventory(
        b"/Form Do /PostScript Do",
        &ContentScope::Page,
        &[PdfName(b"Image".to_vec())],
    )?;

    assert!(inventory.is_empty());
    Ok(())
}

#[test]
fn image_inventory_object_ids_are_deterministic() -> Result<(), String> {
    let names = [PdfName(b"Im1".to_vec()), PdfName(b"Im2".to_vec())];
    let first = image_inventory(b"/Im1 Do /Other Do /Im2 Do", &ContentScope::Page, &names)?;
    let second = image_inventory(b"/Im1 Do /Other Do /Im2 Do", &ContentScope::Page, &names)?;

    assert_eq!(first, second);
    assert_eq!(first.entries[0].id.page, PageIndex(2));
    assert_eq!(first.entries[0].id.sequence, 0);
    assert_eq!(first.entries[1].id.sequence, 1);
    assert_ne!(first.entries[0].id.digest, first.entries[1].id.digest);
    Ok(())
}

#[test]
fn fill_color_observation_carries_color_operator_source_not_paint_source() -> Result<(), String> {
    let inventory = vector_inventory(b"1 0 0 rg f", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing vector entry")?;

    // The paint operator `f` sits at bytes 9..10; the color source must point
    // at the `rg` color-setting record (bytes 0..8), not the paint record.
    assert_eq!(
        entry.provenance.range,
        Some(ByteRange { start: 9, end: 10 })
    );
    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Fill);
    assert_eq!(entry.colors[0].source, Some(ByteRange { start: 0, end: 8 }));
    Ok(())
}

#[test]
fn default_color_observation_has_no_source() -> Result<(), String> {
    let inventory = vector_inventory(b"f", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing vector entry")?;

    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Fill);
    assert_eq!(entry.colors[0].source, None);
    Ok(())
}

#[test]
fn color_source_is_restored_after_save_restore() -> Result<(), String> {
    // `1 0 0 rg` (bytes 0..8) sets the fill source; `0 g` (bytes 11..14)
    // overrides it inside the `q`...`Q` block; after `Q` the source must be
    // restored to the `rg` record range, and the `f` paint observes it.
    let inventory = vector_inventory(b"1 0 0 rg q 0 g Q f", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing vector entry")?;

    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Fill);
    assert_eq!(entry.colors[0].source, Some(ByteRange { start: 0, end: 8 }));
    Ok(())
}

#[test]
fn color_source_inside_save_restore_points_at_inner_operator() -> Result<(), String> {
    // While the `q`...`Q` block is open, the active fill source must point at
    // the inner `0 g` record (bytes 11..14), not the outer `rg` record.
    let inventory = vector_inventory(b"1 0 0 rg q 0 g f Q", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing vector entry")?;

    assert_eq!(entry.colors.len(), 1);
    assert_eq!(
        entry.colors[0].source,
        Some(ByteRange { start: 11, end: 14 })
    );
    Ok(())
}

#[test]
fn synthesized_image_color_observation_has_no_source() -> Result<(), String> {
    let inventory = image_inventory(
        b"/Photo Do",
        &ContentScope::Page,
        &[PdfName(b"Photo".to_vec())],
    )?;
    let entry = inventory.entries.first().ok_or("missing image entry")?;

    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Image);
    assert_eq!(entry.colors[0].source, None);
    Ok(())
}

#[test]
fn stroke_color_observation_carries_color_operator_source() -> Result<(), String> {
    // `0.1 0.2 0.3 RG` occupies bytes 0..14; the `S` paint observes that range
    // as the stroke color source.
    let inventory = vector_inventory(b"0.1 0.2 0.3 RG S", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing vector entry")?;

    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Stroke);
    assert_eq!(
        entry.colors[0].source,
        Some(ByteRange { start: 0, end: 14 })
    );
    Ok(())
}

#[test]
fn text_color_observation_carries_color_operator_source() -> Result<(), String> {
    // `0.2 0.3 0.4 rg` occupies bytes 0..14; the `(Hello) Tj` text-showing
    // operator observes that range as the fill color source.
    let inventory = text_inventory(b"0.2 0.3 0.4 rg (Hello) Tj", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing text entry")?;

    assert_eq!(entry.colors.len(), 1);
    assert_eq!(entry.colors[0].usage, ColorUsage::Fill);
    assert_eq!(
        entry.colors[0].source,
        Some(ByteRange { start: 0, end: 14 })
    );
    Ok(())
}

#[test]
fn color_source_changes_object_digest() -> Result<(), String> {
    // The same paint and color components, but one fill is page-default
    // (source `None`) and the other was set by `rg` (source `Some`). The
    // source provenance must change the object digest.
    let defaulted = vector_inventory(b"f", &ContentScope::Page)?;
    let sourced = vector_inventory(b"0 0 0 rg f", &ContentScope::Page)?;

    let defaulted_entry = defaulted.entries.first().ok_or("missing default entry")?;
    let sourced_entry = sourced.entries.first().ok_or("missing sourced entry")?;

    assert_eq!(defaulted_entry.colors[0].source, None);
    assert_eq!(
        sourced_entry.colors[0].source,
        Some(ByteRange { start: 0, end: 8 })
    );
    assert_ne!(defaulted_entry.id.digest, sourced_entry.id.digest);
    Ok(())
}

#[test]
fn vector_object_digest_is_locked() -> Result<(), String> {
    let inventory = vector_inventory(b"1 0 0 rg f", &ContentScope::Page)?;
    let entry = inventory.entries.first().ok_or("missing vector entry")?;

    assert_eq!(entry.id.digest, VECTOR_DIGEST_RG_FILL);
    Ok(())
}

// Locks the `presslint.vector.v2` digest for `1 0 0 rg f` on page 2, sequence 0,
// page scope, with the fill color source pointing at the `rg` record (0..8).
const VECTOR_DIGEST_RG_FILL: [u8; 32] = [
    217, 142, 65, 91, 110, 170, 75, 230, 252, 240, 215, 175, 209, 215, 240, 59, 219, 114, 104, 58,
    55, 44, 112, 184, 238, 244, 97, 190, 129, 253, 98, 6,
];
