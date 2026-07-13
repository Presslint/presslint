//! Focused raw `Tf` font-selection state tests.

use std::rc::Rc;

use presslint_types::ByteRange;

use super::{assemble, mini_json, name};
use crate::DecodedRange;
use crate::{
    ColorSpaceEnv, ExtGStateEnv, ExtGStateFontDirective, ExtGStateParams, ExtGStateResource,
    FontSelectionState, GraphicsStateWalker, GraphicsWalkErrorKind, GsParam, PaintOp, PaintOpKind,
    PaintProgram, TextRenderingMode, TextShowOperator, walk_graphics_state,
};

fn walk(input: &[u8]) -> Result<Vec<PaintOp>, String> {
    let records = assemble(input)?;
    walk_graphics_state(input, &records).map_err(|error| format!("{error:?}"))
}

fn walk_with_env(input: &[u8], resources: &[ExtGStateResource]) -> Result<Vec<PaintOp>, String> {
    let records = assemble(input)?;
    let mut walker =
        GraphicsStateWalker::with_envs(ColorSpaceEnv::empty(), ExtGStateEnv::new(resources));
    records
        .iter()
        .enumerate()
        .map(|(index, record)| {
            walker
                .step(input, index, record)
                .map_err(|error| format!("{error:?}"))
        })
        .collect()
}

fn selected(name_bytes: &[u8], size: f64) -> FontSelectionState {
    FontSelectionState::Selected {
        name: name(name_bytes),
        size,
    }
}

#[test]
fn initial_state_and_text_without_tf_are_unset() -> Result<(), String> {
    assert_eq!(
        crate::GraphicsStateSnapshot::page_default().font_selection,
        FontSelectionState::Unset
    );
    let ops = walk(b"(Hi) Tj")?;
    assert_eq!(ops[0].state.font_selection, FontSelectionState::Unset);
    Ok(())
}

#[test]
fn tf_emits_set_font_and_all_text_show_forms_observe_shared_selection() -> Result<(), String> {
    let ops = walk(b"/F#31 12 Tf (A) Tj [(B)] TJ (C) ' 1 2 (D) \"")?;
    assert_eq!(
        ops[0].kind,
        PaintOpKind::SetFont {
            name: name(b"F#31"),
            size: 12.0,
        }
    );
    assert_eq!(ops[0].state.font_selection, selected(b"F#31", 12.0));

    let expected = [
        TextShowOperator::ShowText,
        TextShowOperator::ShowTextAdjusted,
        TextShowOperator::MoveNextLineAndShowText,
        TextShowOperator::SetSpacingMoveNextLineAndShowText,
    ];
    for (op, operator) in ops[1..].iter().zip(expected) {
        assert_eq!(
            op.kind,
            PaintOpKind::TextShow {
                operator,
                rendering_mode: TextRenderingMode::Fill,
            }
        );
        assert_eq!(op.state.font_selection, selected(b"F#31", 12.0));
        assert!(Rc::ptr_eq(&ops[0].state, &op.state));
    }
    Ok(())
}

#[test]
fn tf_accepts_every_finite_size_and_preserves_negative_zero_bits() -> Result<(), String> {
    for (lexeme, expected) in [
        ("0", 0.0f64),
        ("-3", -3.0),
        ("12.25", 12.25),
        ("-0.0", -0.0),
    ] {
        let input = format!("/F {lexeme} Tf");
        let ops = walk(input.as_bytes())?;
        let FontSelectionState::Selected { size, .. } = &ops[0].state.font_selection else {
            return Err("Tf did not install Selected".to_string());
        };
        assert_eq!(size.to_bits(), expected.to_bits(), "{lexeme}");
        let PaintOpKind::SetFont { size, .. } = &ops[0].kind else {
            return Err("Tf did not emit SetFont".to_string());
        };
        assert_eq!(size.to_bits(), expected.to_bits(), "{lexeme}");
    }
    Ok(())
}

#[test]
fn malformed_tf_uses_existing_errors_and_does_not_partially_mutate() -> Result<(), String> {
    for (input, expected, range) in [
        (
            b"Tf".as_slice(),
            GraphicsWalkErrorKind::MalformedOperandCount {
                operator: b"Tf".to_vec(),
                expected: 2,
                got: 0,
            },
            ByteRange { start: 0, end: 2 },
        ),
        (
            b"/F 12 13 Tf".as_slice(),
            GraphicsWalkErrorKind::MalformedOperandCount {
                operator: b"Tf".to_vec(),
                expected: 2,
                got: 3,
            },
            ByteRange { start: 0, end: 11 },
        ),
        (
            b"12 9 Tf".as_slice(),
            GraphicsWalkErrorKind::MalformedNameOperand {
                operator: b"Tf".to_vec(),
                operand_index: 0,
            },
            ByteRange { start: 0, end: 2 },
        ),
        (
            b"/ 9 Tf".as_slice(),
            GraphicsWalkErrorKind::MalformedNameOperand {
                operator: b"Tf".to_vec(),
                operand_index: 0,
            },
            ByteRange { start: 0, end: 1 },
        ),
        (
            b"/F [9] Tf".as_slice(),
            GraphicsWalkErrorKind::MalformedNumericOperand {
                operator: b"Tf".to_vec(),
                operand_index: 1,
            },
            ByteRange { start: 3, end: 6 },
        ),
    ] {
        let records = assemble(input)?;
        let error = walk_graphics_state(input, &records)
            .err()
            .ok_or("malformed Tf should fail")?;
        assert_eq!(error.kind, expected, "{}", String::from_utf8_lossy(input));
        assert_eq!(error.range, DecodedRange::new(range));
    }

    let input = b"/F0 8 Tf /F1 [9] Tf";
    let records = assemble(input)?;
    let mut walker = GraphicsStateWalker::new();
    walker
        .step(input, 0, &records[0])
        .map_err(|error| format!("{error:?}"))?;
    let error = walker
        .step(input, 1, &records[1])
        .err()
        .ok_or("second Tf should fail")?;
    assert_eq!(
        error.kind,
        GraphicsWalkErrorKind::MalformedNumericOperand {
            operator: b"Tf".to_vec(),
            operand_index: 1,
        }
    );
    assert_eq!(error.range, DecodedRange::new(records[1].operands[1].range));
    assert_eq!(walker.state().font_selection, selected(b"F0", 8.0));
    Ok(())
}

#[test]
fn nonfinite_tf_size_is_structured_and_fuses_paint_program() -> Result<(), String> {
    let huge = "9".repeat(400);
    let input = format!("/F0 8 Tf /F1 {huge} Tf /F2 9 Tf");
    let records = assemble(input.as_bytes())?;
    let program = PaintProgram::new(input.as_bytes(), &records, ColorSpaceEnv::empty());
    let mut ops = program.ops();
    assert!(matches!(
        ops.next(),
        Some(Ok(PaintOp {
            kind: PaintOpKind::SetFont { .. },
            ..
        }))
    ));
    let error = ops
        .next()
        .ok_or("missing nonfinite result")?
        .err()
        .ok_or("nonfinite Tf should fail")?;
    assert_eq!(
        error.kind,
        GraphicsWalkErrorKind::NonFiniteNumericOperand {
            operator: b"Tf".to_vec(),
            operand_index: 1,
        }
    );
    assert_eq!(error.range, DecodedRange::new(records[1].operands[1].range));
    assert!(ops.next().is_none());
    assert!(ops.next().is_none());
    Ok(())
}

#[test]
fn q_q_restore_exact_font_and_bt_et_do_not_reset_it() -> Result<(), String> {
    let ops = walk(b"/F0 -0.0 Tf q /F2 9 Tf Q BT (A) Tj ET BT (B) Tj ET")?;
    assert_eq!(ops[2].state.font_selection, selected(b"F2", 9.0));
    assert_eq!(ops[3].state.font_selection, selected(b"F0", -0.0));
    assert!(Rc::ptr_eq(&ops[0].state, &ops[3].state));
    for show in [&ops[5], &ops[8]] {
        let FontSelectionState::Selected { size, .. } = &show.state.font_selection else {
            return Err("show did not retain selected font".to_string());
        };
        assert_eq!(size.to_bits(), (-0.0f64).to_bits());
    }
    assert!(Rc::ptr_eq(&ops[3].state, &ops[5].state));
    assert!(Rc::ptr_eq(&ops[5].state, &ops[8].state));
    Ok(())
}

#[test]
fn every_gs_case_is_indeterminate_and_later_tf_recovers() -> Result<(), String> {
    let resources = [
        ExtGStateResource {
            name: name(b"Synthetic"),
            params: ExtGStateParams::empty(),
            has_unclassified_keys: false,
            font: ExtGStateFontDirective::Unknown,
        },
        ExtGStateResource {
            name: name(b"Classified"),
            params: ExtGStateParams {
                overprint_stroke: GsParam::Set(true),
                ..ExtGStateParams::empty()
            },
            has_unclassified_keys: false,
            font: ExtGStateFontDirective::Unknown,
        },
    ];
    for (gs_name, env) in [
        (b"Empty".as_slice(), &[][..]),
        (b"Miss".as_slice(), &resources[..]),
        (b"Synthetic".as_slice(), &resources[..]),
        (b"Classified".as_slice(), &resources[..]),
    ] {
        let mut input = b"/F0 8 Tf /".to_vec();
        input.extend_from_slice(gs_name);
        input.extend_from_slice(b" gs /F1 9 Tf (x) Tj");
        let ops = walk_with_env(&input, env)?;
        assert_eq!(
            ops[1].state.font_selection,
            FontSelectionState::Indeterminate,
            "{}",
            String::from_utf8_lossy(gs_name)
        );
        assert_eq!(ops[2].state.font_selection, selected(b"F1", 9.0));
        assert_eq!(ops[3].state.font_selection, selected(b"F1", 9.0));
    }
    Ok(())
}

#[test]
fn q_q_scopes_gs_font_invalidation() -> Result<(), String> {
    let ops = walk(b"/F0 8 Tf q /GS gs /F1 9 Tf Q (x) Tj")?;
    assert_eq!(
        ops[2].state.font_selection,
        FontSelectionState::Indeterminate
    );
    assert_eq!(ops[3].state.font_selection, selected(b"F1", 9.0));
    assert_eq!(ops[4].state.font_selection, selected(b"F0", 8.0));
    assert_eq!(ops[5].state.font_selection, selected(b"F0", 8.0));
    Ok(())
}

#[test]
fn font_state_setter_and_text_show_serde_shapes_are_locked() -> Result<(), mini_json::JsonError> {
    assert_eq!(
        mini_json::to_json(&FontSelectionState::Unset)?,
        r#"{"kind":"unset"}"#
    );
    assert_eq!(
        mini_json::to_json(&selected(b"F1", 12.0))?,
        r#"{"kind":"selected","name":[70,49],"size":12}"#
    );
    assert_eq!(
        mini_json::to_json(&FontSelectionState::Indeterminate)?,
        r#"{"kind":"indeterminate"}"#
    );
    assert_eq!(
        mini_json::to_json(&FontSelectionState::ResolvedIndirect {
            object_number: 17,
            generation: 3,
            object_byte_offset: 901,
            size: -0.0,
        })?,
        r#"{"kind":"resolved_indirect","object_number":17,"generation":3,"object_byte_offset":901,"size":-0}"#
    );
    assert_eq!(
        mini_json::to_json(&PaintOpKind::SetFont {
            name: name(b"F1"),
            size: 12.0,
        })?,
        r#"{"kind":"set_font","name":[70,49],"size":12}"#
    );
    assert_eq!(
        mini_json::to_json(&PaintOpKind::TextShow {
            operator: TextShowOperator::ShowText,
            rendering_mode: TextRenderingMode::Fill,
        })?,
        r#"{"kind":"text_show","operator":"show_text","rendering_mode":"fill"}"#
    );
    Ok(())
}
