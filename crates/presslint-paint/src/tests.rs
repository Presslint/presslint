//! Focused tests for the replayable [`PaintProgram`](crate::PaintProgram) stream.
//!
//! These prove the two invariants the paint-program abstraction must hold to be a
//! faithful re-expression of the walker: it REPLAYS (iterating the same program
//! twice yields identical op sequences) and it AGREES with `walk_graphics_state`
//! (both the success case and the error-fusing short-circuit).

use presslint_syntax::{OperatorRecord, assemble_operators, tokenize};

use crate::{ColorSpaceEnv, GraphicsWalkError, PaintOp, PaintProgram, walk_graphics_state};

/// Tokenize + assemble a content stream into owned operator records for testing.
fn assemble(input: &[u8]) -> Result<Vec<OperatorRecord>, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    Ok(assembled.records)
}

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
