//! Replay + walk-agreement tests for the [`PaintProgram`](crate::PaintProgram)
//! iterator (Phase 0a-2), plus the seeded-replay seam.
//!
//! These prove the paint-program abstraction is a faithful re-expression of the
//! walker: it REPLAYS (iterating the same program twice yields identical op
//! sequences) and it AGREES with `walk_graphics_state` (both the success case
//! and the error-fusing short-circuit). The seeded tests prove
//! `ops_with_initial_state` transports EVERY tracked snapshot field (CTM, both
//! colours, rendering mode, font selection, classified `ExtGState` state),
//! stays deterministic across replays, and leaves the default `.ops()` path at
//! page default. Form `/Matrix` concatenation, `/BBox` clipping, and
//! transparency-group entry resets are NOT modelled or tested here.

use std::rc::Rc;

use presslint_types::ColorSpace;

use super::{assemble, name};
use crate::{
    BlendModeClass, ColorSpaceEnv, FontSelectionState, GraphicsColor, GraphicsExtGStateSnapshot,
    GraphicsStateSnapshot, GraphicsWalkError, GsParam, PaintOp, PaintProgram, TextRenderingMode,
    walk_graphics_state,
};

/// Collect the program's ops as raw per-record results (no short-circuit), the
/// way a caller that wants every yielded item would.
fn raw_ops(program: PaintProgram<'_>) -> Vec<Result<PaintOp, GraphicsWalkError>> {
    program.into_iter().collect()
}

#[test]
fn paint_program_replays_identical_op_sequences() -> Result<(), String> {
    // A mixed, well-formed stream exercising save/restore, cm, colour, path
    // paint, text show, and both XObject/ExtGState invocations.
    let input: &[u8] = b"q 1 0 0 1 5 5 cm 0.4 g f BT (Hi) Tj ET /Im1 Do /GS1 gs Q";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());

    // Replay: two independent walks of the same descriptor are identical.
    let first = raw_ops(program);
    let second = raw_ops(program);
    assert_eq!(first, second);
    // The descriptor is Copy, so it is unconsumed and re-iterable a third time.
    assert_eq!(raw_ops(program), first);
    Ok(())
}

#[test]
fn paint_program_ops_equal_walk_graphics_state() -> Result<(), String> {
    let input: &[u8] = b"q 1 0 0 1 5 5 cm 0.4 g f BT (Hi) Tj ET /Im1 Do /GS1 gs Q";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());

    // Collecting Result items short-circuits to Result<Vec, _> exactly like the
    // materializing `walk_graphics_state`, so the two must be equal.
    let collected: Result<Vec<_>, _> = program.into_iter().collect();
    let walked = walk_graphics_state(input, &records);
    assert_eq!(collected, walked);
    Ok(())
}

#[test]
fn paint_program_fuses_on_first_error_matching_walk() -> Result<(), String> {
    // `0.4 g f` is well-formed; the malformed `1 2 RG` (three operands expected,
    // two given) sits after it. The program must yield ops up to and including
    // the Err, then fuse to None forever.
    let input: &[u8] = b"0.4 g f 1 2 RG";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());

    let mut ops = program.into_iter();
    let mut yielded = Vec::new();
    for item in ops.by_ref() {
        let is_err = item.is_err();
        yielded.push(item);
        if is_err {
            break;
        }
    }

    // The last yielded item is the Err, and it matches what the materializing
    // walk surfaces for the same malformed record.
    let last = yielded.last().ok_or("at least one op should be yielded")?;
    assert!(last.is_err());
    let walked_err = walk_graphics_state(input, &records)
        .err()
        .ok_or("walk should fail on the malformed record")?;
    assert_eq!(last.as_ref().err(), Some(&walked_err));

    // Fused: every subsequent poll is None, forever.
    assert!(ops.next().is_none());
    assert!(ops.next().is_none());

    // And the short-circuiting collect agrees byte-for-byte with the walk.
    let collected: Result<Vec<_>, _> = program.into_iter().collect();
    assert_eq!(collected, walk_graphics_state(input, &records));
    Ok(())
}

/// A snapshot where EVERY tracked field differs from `page_default()`: CTM,
/// both colours, text rendering mode, font selection, and two classified
/// `ExtGState` parameters.
pub(super) fn non_default_snapshot() -> GraphicsStateSnapshot {
    let mut extgstate = GraphicsExtGStateSnapshot::page_default();
    extgstate.overprint_fill = GsParam::Set(true);
    extgstate.blend_mode = GsParam::Set(BlendModeClass::NonNormal);
    GraphicsStateSnapshot {
        ctm: [2.0, 0.0, 0.0, 2.0, 10.0, 20.0],
        stroking_color: GraphicsColor::new(ColorSpace::DeviceRgb, vec![1.0, 0.0, 0.0]),
        nonstroking_color: GraphicsColor::new(ColorSpace::DeviceCmyk, vec![0.0, 0.0, 0.0, 1.0]),
        text_rendering_mode: TextRenderingMode::FillThenStroke,
        font_selection: FontSelectionState::Selected {
            name: name(b"F9"),
            size: 9.5,
        },
        extgstate,
    }
}

#[test]
// Exact CTM transport is part of the inheritance contract, so the array
// comparison is deliberately strict.
#[allow(clippy::float_cmp)]
fn seeded_replay_starts_from_exact_seed_and_replays_deterministically() -> Result<(), String> {
    // First op `n` does not mutate state, so it must expose the seed itself;
    // the later `0.5 g` mutates and must copy-on-write away from it.
    let input: &[u8] = b"n 0.5 g f";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());
    let seed = Rc::new(non_default_snapshot());

    let first: Vec<_> = program
        .ops_with_initial_state(Rc::clone(&seed))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{error:?}"))?;

    // Full tracked-state transport: the non-mutating first op carries the seed
    // by POINTER (a refcount bump, not a copy), so every field matches exactly.
    assert!(Rc::ptr_eq(&first[0].state, &seed));
    assert_eq!(*first[0].state, non_default_snapshot());

    // The mutation copied-on-write: the caller-held seed is untouched, while the
    // walk's own state moved on (only the nonstroking colour changed).
    assert_eq!(*seed, non_default_snapshot());
    assert!(!Rc::ptr_eq(&first[2].state, &seed));
    assert_eq!(
        first[2].state.nonstroking_color.space,
        ColorSpace::DeviceGray
    );
    assert_eq!(first[2].state.ctm, non_default_snapshot().ctm);
    assert_eq!(
        first[2].state.font_selection,
        non_default_snapshot().font_selection
    );

    // Replaying the same program from an equal seed is deterministic.
    let second: Vec<_> = program
        .ops_with_initial_state(Rc::new(non_default_snapshot()))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(first, second);
    Ok(())
}

#[test]
fn default_ops_still_start_at_page_default_and_equal_seeded_page_default() -> Result<(), String> {
    let input: &[u8] = b"n 0.5 g f";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());

    let default_ops: Vec<_> = program
        .ops()
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(*default_ops[0].state, GraphicsStateSnapshot::page_default());

    // `.ops()` is exactly `ops_with_initial_state(page_default)`.
    let seeded_default: Vec<_> = program
        .ops_with_initial_state(Rc::new(GraphicsStateSnapshot::page_default()))
        .collect::<Result<_, _>>()
        .map_err(|error| format!("{error:?}"))?;
    assert_eq!(default_ops, seeded_default);
    Ok(())
}
