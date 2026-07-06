//! `gs` snapshot-classification tests (Phase 1-2).
//!
//! These prove the tri-level `gs` rule: an empty environment is state-identical
//! to the pre-classification walker (`gs` mutates nothing), a miss on a NON-empty
//! environment sets every param to `Unresolved`, and a hit layers the resource's
//! set params over the current state without disturbing the params it leaves
//! unset. They also pin `q`/`Q` save-restore of the classified state and the
//! `Rc`-sharing boundary around the mutation.

use std::rc::Rc;

use super::{assemble, name};
use crate::{
    AlphaClass, BlendModeClass, ColorSpaceEnv, ExtGStateEnv, ExtGStateParams, ExtGStateResource,
    GraphicsExtGStateSnapshot, GraphicsStateWalker, GsParam, PaintOp,
};

/// Walk `input` with a borrowed `ExtGState` environment, materializing every op.
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
                .map_err(|e| format!("{e:?}"))
        })
        .collect()
}

/// A resource selected by `gs_name` whose params are built from `set`.
fn resource(gs_name: &[u8], set: ExtGStateParams) -> ExtGStateResource {
    ExtGStateResource {
        name: name(gs_name),
        params: set,
        has_unclassified_keys: false,
    }
}

#[test]
fn gs_hit_layers_set_params_and_leaves_unset_params_untouched() -> Result<(), String> {
    // GS0 sets both overprint flags; GS1 sets only the blend mode. After GS1 the
    // overprint flags must keep their GS0 classification (gs layers, it does not
    // reset absent keys).
    let resources = [
        resource(
            b"GS0",
            ExtGStateParams {
                overprint_stroke: GsParam::Set(true),
                overprint_fill: GsParam::Set(true),
                ..ExtGStateParams::empty()
            },
        ),
        resource(
            b"GS1",
            ExtGStateParams {
                blend_mode: GsParam::Set(BlendModeClass::NonNormal),
                ..ExtGStateParams::empty()
            },
        ),
    ];
    let ops = walk_with_env(b"/GS0 gs /GS1 gs", &resources)?;

    let after = &ops[1].state.extgstate;
    assert_eq!(after.overprint_stroke, GsParam::Set(true));
    assert_eq!(after.overprint_fill, GsParam::Set(true));
    assert_eq!(after.blend_mode, GsParam::Set(BlendModeClass::NonNormal));
    // Genuinely-unset params stay at the page default.
    assert_eq!(after.overprint_mode, GsParam::Default);
    assert_eq!(after.stroke_alpha, GsParam::Default);
    assert_eq!(after.fill_alpha, GsParam::Default);
    assert_eq!(after.soft_mask, GsParam::Default);
    Ok(())
}

#[test]
fn gs_miss_on_non_empty_env_sets_all_params_unresolved() -> Result<(), String> {
    // The env is non-empty (it classifies GS0) but the stream invokes GS9, which
    // is not in it: nothing is known about that gs, so every param goes Unresolved.
    let resources = [resource(
        b"GS0",
        ExtGStateParams {
            overprint_stroke: GsParam::Set(true),
            ..ExtGStateParams::empty()
        },
    )];
    let ops = walk_with_env(b"/GS9 gs", &resources)?;

    let after = &ops[0].state.extgstate;
    assert_eq!(after.overprint_stroke, GsParam::Unresolved);
    assert_eq!(after.overprint_fill, GsParam::Unresolved);
    assert_eq!(after.overprint_mode, GsParam::Unresolved);
    assert_eq!(after.stroke_alpha, GsParam::Unresolved);
    assert_eq!(after.fill_alpha, GsParam::Unresolved);
    assert_eq!(after.blend_mode, GsParam::Unresolved);
    assert_eq!(after.soft_mask, GsParam::Unresolved);
    Ok(())
}

#[test]
fn gs_on_empty_env_mutates_nothing() -> Result<(), String> {
    // The empty env is the feature-off path: `gs` must leave the snapshot exactly
    // at the page default, byte-and-state-identical to the pre-classification
    // walker. (`walk_with_env` with an empty slice takes the `is_empty()` branch.)
    let ops = walk_with_env(b"/GS1 gs", &[])?;

    assert_eq!(
        ops[0].state.extgstate,
        GraphicsExtGStateSnapshot::page_default()
    );
    // The op still surfaces; only the state stayed inert.
    assert_eq!(ops.len(), 1);
    Ok(())
}

#[test]
fn save_restore_restores_the_pre_gs_extgstate() -> Result<(), String> {
    // GS0 sets overprint before the `q`; GS1 sets the blend mode inside the block.
    // `Q` must restore the pre-`q` extgstate (GS0's overprint kept, GS1's blend
    // mode gone), not page-default it.
    let resources = [
        resource(
            b"GS0",
            ExtGStateParams {
                overprint_stroke: GsParam::Set(true),
                ..ExtGStateParams::empty()
            },
        ),
        resource(
            b"GS1",
            ExtGStateParams {
                blend_mode: GsParam::Set(BlendModeClass::NonNormal),
                ..ExtGStateParams::empty()
            },
        ),
    ];
    // Events: gs GS0(0), q(1), gs GS1(2), Q(3), f(4).
    let ops = walk_with_env(b"/GS0 gs q /GS1 gs Q f", &resources)?;

    let inside = &ops[2].state.extgstate;
    assert_eq!(inside.overprint_stroke, GsParam::Set(true));
    assert_eq!(inside.blend_mode, GsParam::Set(BlendModeClass::NonNormal));

    let restored = &ops[4].state.extgstate;
    assert_eq!(restored.overprint_stroke, GsParam::Set(true));
    assert_eq!(restored.blend_mode, GsParam::Default);
    Ok(())
}

#[test]
fn two_sequential_gs_layer_and_override() -> Result<(), String> {
    // GS_A sets OP=true and CA=NonOpaque; GS_B sets BM and OVERRIDES OP=false.
    // After both: OP is B's value, CA is A's (layered through), BM is B's.
    let resources = [
        resource(
            b"GA",
            ExtGStateParams {
                overprint_stroke: GsParam::Set(true),
                stroke_alpha: GsParam::Set(AlphaClass::NonOpaque),
                ..ExtGStateParams::empty()
            },
        ),
        resource(
            b"GB",
            ExtGStateParams {
                overprint_stroke: GsParam::Set(false),
                blend_mode: GsParam::Set(BlendModeClass::NonNormal),
                ..ExtGStateParams::empty()
            },
        ),
    ];
    let ops = walk_with_env(b"/GA gs /GB gs", &resources)?;

    let after = &ops[1].state.extgstate;
    assert_eq!(after.overprint_stroke, GsParam::Set(false));
    assert_eq!(after.stroke_alpha, GsParam::Set(AlphaClass::NonOpaque));
    assert_eq!(after.blend_mode, GsParam::Set(BlendModeClass::NonNormal));
    Ok(())
}

#[test]
fn empty_env_gs_keeps_interned_state_sharing_and_a_hit_breaks_it() -> Result<(), String> {
    // Empty env: `gs` mutates nothing, so it stays inside the run of no-state-
    // change ops that share one interned `Rc` (extends the walker.rs identity
    // test with the classified snapshot in place).
    let inert = walk_with_env(b"n /GS1 gs (Hi) Tj", &[])?;
    assert_eq!(inert.len(), 3);
    for window in inert.windows(2) {
        assert!(
            Rc::ptr_eq(&window[0].state, &window[1].state),
            "empty-env gs must not perturb the shared interned state"
        );
    }

    // A hitting gs DOES mutate, so it copies-on-write to a distinct snapshot while
    // the following no-op shares the mutated state.
    let resources = [resource(
        b"GS1",
        ExtGStateParams {
            blend_mode: GsParam::Set(BlendModeClass::NonNormal),
            ..ExtGStateParams::empty()
        },
    )];
    let hit = walk_with_env(b"n /GS1 gs n", &resources)?;
    assert!(
        !Rc::ptr_eq(&hit[0].state, &hit[1].state),
        "a hitting gs must copy-on-write away from the pre-gs state"
    );
    assert!(
        Rc::ptr_eq(&hit[1].state, &hit[2].state),
        "the no-op after a hitting gs shares the mutated state"
    );
    Ok(())
}
