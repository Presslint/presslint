//! Walker interned-state identity tests (Phase 0a-5).
//!
//! These prove the `Rc`-interned graphics state is shared across no-state-change
//! ops and preserved across `q`/`Q` save/restore.

use std::rc::Rc;

use super::assemble;
use crate::{PaintOp, walk_graphics_state};

/// Walk `input` into materialized ops, mapping any walker/assemble error to a
/// `String` so the `Rc`-sharing tests can use `?`.
fn walk(input: &[u8]) -> Result<Vec<PaintOp>, String> {
    let records = assemble(input)?;
    walk_graphics_state(input, &records).map_err(|error| format!("{error:?}"))
}

#[test]
fn no_state_change_ops_share_the_same_interned_state() -> Result<(), String> {
    // These operators emit paint ops without mutating the graphics state.
    let ops = walk(b"n /Im1 Do /GS1 gs (Hi) Tj")?;
    assert_eq!(ops.len(), 4);
    for window in ops.windows(2) {
        assert!(
            Rc::ptr_eq(&window[0].state, &window[1].state),
            "no-state-change ops must share one interned state"
        );
    }
    Ok(())
}

#[test]
fn save_restore_preserves_interned_state_identity() -> Result<(), String> {
    let ops = walk(b"q 1 0 0 1 5 5 cm Q n")?;
    assert_eq!(ops.len(), 4);
    let saved = &ops[0].state;
    let concat = &ops[1].state;
    let restored = &ops[2].state;
    let after = &ops[3].state;

    assert!(
        Rc::ptr_eq(saved, restored),
        "post-`Q` state must be the exact saved pre-`cm` `Rc`"
    );
    assert!(
        Rc::ptr_eq(restored, after),
        "`n` must not disturb the restored interned state"
    );
    assert!(
        !Rc::ptr_eq(concat, saved),
        "`cm` must copy-on-write to a distinct snapshot"
    );
    Ok(())
}
