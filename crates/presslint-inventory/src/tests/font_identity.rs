//! Text-v5 identity locks for raw and resolved effective font selection.

use std::rc::Rc;

use presslint_types::{ByteRange, ContentScope, InvocationPath, ObjectKind, PageIndex, PdfName};

use crate::{
    DecodedRange, FontSelectionState, GraphicsStateSnapshot, Inventory, PaintOp, PaintOpKind,
    PathPaintKind, TextRenderingMode, TextShowOperator, expanded_entry_identity,
    inventory_from_graphics_events, text_inventory_from_graphics_events,
};

fn name(bytes: &[u8]) -> PdfName {
    PdfName(bytes.to_vec())
}

fn selected(name_bytes: &[u8], size: f64) -> FontSelectionState {
    FontSelectionState::Selected {
        name: name(name_bytes),
        size,
    }
}

fn resolved(object_number: u32, generation: u16, offset: usize, size: f64) -> FontSelectionState {
    FontSelectionState::ResolvedIndirect {
        object_number,
        generation,
        object_byte_offset: offset,
        size,
    }
}

fn event(kind: PaintOpKind, font_selection: FontSelectionState) -> PaintOp {
    let mut state = GraphicsStateSnapshot::page_default();
    state.font_selection = font_selection;
    PaintOp {
        index: 3,
        operator_range: DecodedRange::new(ByteRange { start: 40, end: 42 }),
        record_range: DecodedRange::new(ByteRange { start: 35, end: 42 }),
        kind,
        state: Rc::new(state),
    }
}

fn text_event(font_selection: FontSelectionState) -> PaintOp {
    event(
        PaintOpKind::TextShow {
            operator: TextShowOperator::ShowText,
            rendering_mode: TextRenderingMode::Fill,
        },
        font_selection,
    )
}

fn text_inventory(event: &PaintOp) -> Inventory {
    text_inventory_from_graphics_events(
        PageIndex(2),
        &ContentScope::Page,
        std::slice::from_ref(event),
    )
}

fn text_digest(font_selection: FontSelectionState) -> [u8; 32] {
    text_inventory(&text_event(font_selection)).entries[0]
        .id
        .digest
}

#[test]
fn text_v5_distinguishes_all_font_state_discriminators() {
    let unset = text_digest(FontSelectionState::Unset);
    let selected = text_digest(selected(b"F1", 12.0));
    let resolved = text_digest(resolved(9, 2, 401, 12.0));
    let indeterminate = text_digest(FontSelectionState::Indeterminate);

    assert_ne!(unset, selected);
    assert_ne!(unset, resolved);
    assert_ne!(unset, indeterminate);
    assert_ne!(selected, resolved);
    assert_ne!(selected, indeterminate);
    assert_ne!(resolved, indeterminate);
}

#[test]
fn text_v5_hashes_legacy_raw_name_and_exact_size_bits() {
    assert_ne!(
        text_digest(selected(b"F1", 12.0)),
        text_digest(selected(b"F#31", 12.0)),
        "raw names are not escape-decoded"
    );
    assert_ne!(
        text_digest(selected(b"F1", 12.0)),
        text_digest(selected(b"F1", 12.25))
    );
    assert_ne!(
        text_digest(selected(b"F1", 0.0)),
        text_digest(selected(b"F1", -0.0)),
        "the size sign bit is identity input"
    );
}

#[test]
fn text_v5_resolved_identity_ignores_alias_but_hashes_reached_tuple_and_size_bits() {
    let alias_a = text_digest(resolved(9, 2, 401, 12.0));
    let alias_b = text_digest(resolved(9, 2, 401, 12.0));
    assert_eq!(alias_a, alias_b, "resolved state carries no resource name");
    assert_ne!(alias_a, text_digest(resolved(10, 2, 401, 12.0)));
    assert_ne!(alias_a, text_digest(resolved(9, 3, 401, 12.0)));
    assert_ne!(alias_a, text_digest(resolved(9, 2, 907, 12.0)));
    assert_ne!(alias_a, text_digest(resolved(9, 2, 401, 12.25)));
    assert_ne!(
        text_digest(resolved(9, 2, 401, 0.0)),
        text_digest(resolved(9, 2, 401, -0.0))
    );
}

#[test]
fn repeated_walks_with_selected_font_are_deterministic() -> Result<(), String> {
    let first = super::text_inventory(b"/F1 -0.0 Tf (A) Tj", &ContentScope::Page)?;
    let second = super::text_inventory(b"/F1 -0.0 Tf (A) Tj", &ContentScope::Page)?;
    assert_eq!(first, second);
    Ok(())
}

#[test]
fn expanded_and_single_stream_identity_borrow_the_same_snapshot_font() {
    let event = text_event(selected(b"F1", 12.0));
    let template = text_inventory(&event).entries.remove(0);
    let empty_path = InvocationPath { frames: Vec::new() };
    let expanded = expanded_entry_identity(&template, 0, &empty_path, &event);
    assert_eq!(expanded.id, template.id);

    let uncertain_event = text_event(FontSelectionState::Indeterminate);
    let uncertain = expanded_entry_identity(&template, 0, &empty_path, &uncertain_event);
    assert_ne!(uncertain.id.digest, template.id.digest);
}

#[test]
fn font_selection_changes_text_identity_only() {
    let states = [FontSelectionState::Unset, selected(b"F1", 12.0)];

    let vector_ids: Vec<_> = states
        .iter()
        .cloned()
        .map(|state| {
            let event = event(
                PaintOpKind::PathPaint {
                    paint: PathPaintKind::FillNonzero,
                },
                state,
            );
            inventory_from_graphics_events(PageIndex(2), &ContentScope::Page, &[event], &[], &[])
                .entries[0]
                .id
                .clone()
        })
        .collect();
    assert_eq!(vector_ids[0], vector_ids[1]);

    for (kind, images, forms, expected_kind) in [
        (
            PaintOpKind::XObjectInvoke { name: name(b"Im") },
            vec![name(b"Im")],
            Vec::new(),
            ObjectKind::Image,
        ),
        (
            PaintOpKind::XObjectInvoke { name: name(b"Fm") },
            Vec::new(),
            vec![name(b"Fm")],
            ObjectKind::FormXObject,
        ),
    ] {
        let ids: Vec<_> = states
            .iter()
            .cloned()
            .map(|state| {
                let event = event(kind.clone(), state);
                let entry = inventory_from_graphics_events(
                    PageIndex(2),
                    &ContentScope::Page,
                    &[event],
                    &images,
                    &forms,
                )
                .entries
                .remove(0);
                assert_eq!(entry.kind, expected_kind);
                entry.id
            })
            .collect();
        assert_eq!(ids[0], ids[1]);
    }

    assert_ne!(
        text_digest(FontSelectionState::Unset),
        text_digest(selected(b"F1", 12.0))
    );
}
