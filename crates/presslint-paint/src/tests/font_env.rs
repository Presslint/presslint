//! Enabled font-environment resolution tests.

use presslint_types::PdfName;

use super::{assemble, name};
use crate::{
    ColorSpaceEnv, ExtGStateEnv, ExtGStateFontDirective, ExtGStateParams, ExtGStateResource,
    FontBinding, FontBindingTarget, FontEnv, FontSelectionState, PaintOp, PaintProgram,
    ResolvedFont,
};

const FONT_A: ResolvedFont = ResolvedFont {
    object_number: 9,
    generation: 2,
    object_byte_offset: 401,
};

const FONT_A_NEWER: ResolvedFont = ResolvedFont {
    object_number: 9,
    generation: 2,
    object_byte_offset: 907,
};

fn binding(raw_name: &[u8], target: FontBindingTarget) -> Result<FontBinding, String> {
    FontBinding::from_pdf_name(&name(raw_name), target)
        .ok_or_else(|| "test binding name should decode".to_string())
}

fn resource(name_bytes: &[u8], font: ExtGStateFontDirective) -> ExtGStateResource {
    ExtGStateResource {
        name: name(name_bytes),
        params: ExtGStateParams::empty(),
        has_unclassified_keys: false,
        font,
    }
}

fn walk_all(
    input: &[u8],
    bindings: &[FontBinding],
    extgstates: &[ExtGStateResource],
) -> Result<Vec<PaintOp>, String> {
    let records = assemble(input)?;
    PaintProgram::with_all_envs(
        input,
        &records,
        ColorSpaceEnv::empty(),
        ExtGStateEnv::new(extgstates),
        FontEnv::known(bindings),
    )
    .ops()
    .collect::<Result<_, _>>()
    .map_err(|error| format!("{error:?}"))
}

fn resolved(font: ResolvedFont, size: f64) -> FontSelectionState {
    FontSelectionState::ResolvedIndirect {
        object_number: font.object_number,
        generation: font.generation,
        object_byte_offset: font.object_byte_offset,
        size,
    }
}

#[test]
fn coverage_states_and_semantic_name_lookup_are_distinct() -> Result<(), String> {
    let bindings = [binding(b"F#31", FontBindingTarget::Resolved(FONT_A))?];
    assert_eq!(bindings[0].semantic_name(), &name(b"F1"));

    let input = b"/F1 12 Tf";
    let records = assemble(input)?;
    let enabled = PaintProgram::with_all_envs(
        input,
        &records,
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
        FontEnv::known(&bindings),
    )
    .ops()
    .next()
    .ok_or("missing enabled op")?
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(enabled.state.font_selection, resolved(FONT_A, 12.0));

    for env in [FontEnv::known(&[]), FontEnv::unknown()] {
        let op = PaintProgram::with_all_envs(
            input,
            &records,
            ColorSpaceEnv::empty(),
            ExtGStateEnv::empty(),
            env,
        )
        .ops()
        .next()
        .ok_or("missing fail-closed op")?
        .map_err(|error| format!("{error:?}"))?;
        assert_eq!(op.state.font_selection, FontSelectionState::Indeterminate);
    }

    let legacy = PaintProgram::with_all_envs(
        input,
        &records,
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
        FontEnv::disabled(),
    )
    .ops()
    .next()
    .ok_or("missing legacy op")?
    .map_err(|error| format!("{error:?}"))?;
    assert_eq!(
        legacy.state.font_selection,
        FontSelectionState::Selected {
            name: name(b"F1"),
            size: 12.0,
        }
    );
    Ok(())
}

#[test]
fn aliases_converge_but_reached_offset_and_unresolved_bindings_do_not() -> Result<(), String> {
    let bindings = [
        binding(b"F1", FontBindingTarget::Resolved(FONT_A))?,
        binding(b"Alias", FontBindingTarget::Resolved(FONT_A))?,
        binding(b"New", FontBindingTarget::Resolved(FONT_A_NEWER))?,
        binding(b"Direct", FontBindingTarget::Unresolved)?,
    ];
    let ops = walk_all(
        b"/F1 8 Tf /Alias 8 Tf /New 8 Tf /Direct 8 Tf /Missing 8 Tf /F# 8 Tf",
        &bindings,
        &[],
    )?;
    assert_eq!(ops[0].state.font_selection, resolved(FONT_A, 8.0));
    assert_eq!(ops[1].state.font_selection, resolved(FONT_A, 8.0));
    assert_eq!(ops[2].state.font_selection, resolved(FONT_A_NEWER, 8.0));
    for op in &ops[3..] {
        assert_eq!(op.state.font_selection, FontSelectionState::Indeterminate);
    }
    Ok(())
}

#[test]
fn tf_and_gs_are_atomic_last_writer_wins_and_absent_font_preserves() -> Result<(), String> {
    let bindings = [binding(b"F1", FontBindingTarget::Resolved(FONT_A))?];
    let resources = [
        resource(b"Keep", ExtGStateFontDirective::LeaveUnchanged),
        resource(
            b"Direct",
            ExtGStateFontDirective::Select {
                font: FONT_A_NEWER,
                size_bits: (-0.0f64).to_bits(),
            },
        ),
        resource(b"Unknown", ExtGStateFontDirective::Unknown),
    ];
    let ops = walk_all(
        b"/F1 7 Tf /Keep gs /Direct gs /F1 9 Tf /Unknown gs /F1 11 Tf /Missing gs",
        &bindings,
        &resources,
    )?;
    assert_eq!(ops[0].state.font_selection, resolved(FONT_A, 7.0));
    assert_eq!(ops[1].state.font_selection, resolved(FONT_A, 7.0));
    let FontSelectionState::ResolvedIndirect { size, .. } = &ops[2].state.font_selection else {
        return Err("direct gs did not install a resolved selection".to_string());
    };
    assert_eq!(size.to_bits(), (-0.0f64).to_bits());
    assert_eq!(ops[2].state.font_selection, resolved(FONT_A_NEWER, -0.0));
    assert_eq!(ops[3].state.font_selection, resolved(FONT_A, 9.0));
    assert_eq!(
        ops[4].state.font_selection,
        FontSelectionState::Indeterminate
    );
    assert_eq!(ops[5].state.font_selection, resolved(FONT_A, 11.0));
    assert_eq!(
        ops[6].state.font_selection,
        FontSelectionState::Indeterminate
    );
    Ok(())
}

#[test]
fn save_restore_and_text_objects_preserve_exact_resolved_selection() -> Result<(), String> {
    let bindings = [binding(b"F1", FontBindingTarget::Resolved(FONT_A))?];
    let resources = [resource(
        b"Direct",
        ExtGStateFontDirective::Select {
            font: FONT_A_NEWER,
            size_bits: 4.0f64.to_bits(),
        },
    )];
    let ops = walk_all(
        b"/F1 -0.0 Tf q /Direct gs Q BT (A) Tj ET BT (B) Tj ET",
        &bindings,
        &resources,
    )?;
    assert_eq!(ops[2].state.font_selection, resolved(FONT_A_NEWER, 4.0));
    assert_eq!(ops[3].state.font_selection, resolved(FONT_A, -0.0));
    for index in [5, 8] {
        let FontSelectionState::ResolvedIndirect { size, .. } = &ops[index].state.font_selection
        else {
            return Err("text show did not retain a resolved selection".to_string());
        };
        assert_eq!(size.to_bits(), (-0.0f64).to_bits());
    }
    Ok(())
}

#[test]
fn malformed_binding_names_are_rejected_before_environment_construction() {
    for raw in [
        b"F#".as_slice(),
        b"F#1".as_slice(),
        b"F#gg".as_slice(),
        b"F#00".as_slice(),
        b"F\0".as_slice(),
    ] {
        assert!(
            FontBinding::from_pdf_name(&PdfName(raw.to_vec()), FontBindingTarget::Unresolved)
                .is_none()
        );
    }
}
