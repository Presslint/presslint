use presslint_syntax::{assemble_operators, tokenize};
use presslint_types::{ByteRange, ContentScope, ObjectKind, PageIndex, PdfName};

use super::{combined_inventory, text_inventory, vector_inventory};
use crate::{build_inventory, inventory_from_graphics_events, walk_graphics_state};

#[test]
fn combined_inventory_merges_kinds_in_content_order() -> Result<(), String> {
    let inventory = combined_inventory(
        b"0.4 g f /Im1 Do (Hi) Tj /Fm1 Do",
        &ContentScope::Page,
        &[PdfName(b"Im1".to_vec())],
        &[PdfName(b"Fm1".to_vec())],
    )?;

    let kinds: Vec<ObjectKind> = inventory.entries.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![
            ObjectKind::Vector,
            ObjectKind::Image,
            ObjectKind::Text,
            ObjectKind::FormXObject,
        ]
    );
    Ok(())
}

#[test]
fn combined_inventory_assigns_one_monotonic_sequence_across_kinds() -> Result<(), String> {
    let inventory = combined_inventory(
        b"0.4 g f /Im1 Do (Hi) Tj /Fm1 Do",
        &ContentScope::Page,
        &[PdfName(b"Im1".to_vec())],
        &[PdfName(b"Fm1".to_vec())],
    )?;

    let sequences: Vec<u32> = inventory
        .entries
        .iter()
        .map(|entry| entry.id.sequence)
        .collect();
    assert_eq!(sequences, vec![0, 1, 2, 3]);
    for entry in &inventory.entries {
        assert_eq!(entry.id.page, PageIndex(2));
    }
    Ok(())
}

#[test]
fn combined_inventory_filters_image_and_form_names_independently() -> Result<(), String> {
    let inventory = combined_inventory(
        b"/Im1 Do /Fm1 Do /Other Do",
        &ContentScope::Page,
        &[PdfName(b"Im1".to_vec())],
        &[PdfName(b"Fm1".to_vec())],
    )?;

    assert_eq!(inventory.entries.len(), 2);
    assert_eq!(inventory.entries[0].kind, ObjectKind::Image);
    assert_eq!(inventory.entries[0].id.sequence, 0);
    assert_eq!(
        inventory.entries[0].provenance.range,
        Some(ByteRange { start: 0, end: 7 })
    );
    assert_eq!(inventory.entries[1].kind, ObjectKind::FormXObject);
    assert_eq!(inventory.entries[1].id.sequence, 1);
    assert_eq!(
        inventory.entries[1].provenance.range,
        Some(ByteRange { start: 8, end: 15 })
    );
    Ok(())
}

#[test]
fn combined_inventory_classifies_shared_name_as_image() -> Result<(), String> {
    let inventory = combined_inventory(
        b"/Dup Do",
        &ContentScope::Page,
        &[PdfName(b"Dup".to_vec())],
        &[PdfName(b"Dup".to_vec())],
    )?;

    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].kind, ObjectKind::Image);
    Ok(())
}

#[test]
fn combined_inventory_is_empty_when_no_objects_are_painted() -> Result<(), String> {
    let inventory = combined_inventory(b"q 10 20 m n Q", &ContentScope::Page, &[], &[])?;

    assert!(inventory.is_empty());
    Ok(())
}

#[test]
fn combined_inventory_entries_match_per_kind_builders_except_sequence() -> Result<(), String> {
    let input = b"0.4 g f (A) Tj 0.5 g f (B) Tj";
    let scope = ContentScope::Page;
    let combined = combined_inventory(input, &scope, &[], &[])?;
    let vectors = vector_inventory(input, &scope)?;
    let texts = text_inventory(input, &scope)?;

    let combined_kinds: Vec<ObjectKind> = combined.entries.iter().map(|entry| entry.kind).collect();
    assert_eq!(
        combined_kinds,
        vec![
            ObjectKind::Vector,
            ObjectKind::Text,
            ObjectKind::Vector,
            ObjectKind::Text,
        ]
    );

    let pairs = [
        (&combined.entries[0], &vectors.entries[0]),
        (&combined.entries[1], &texts.entries[0]),
        (&combined.entries[2], &vectors.entries[1]),
        (&combined.entries[3], &texts.entries[1]),
    ];

    for (combined_entry, per_kind_entry) in pairs {
        assert_eq!(combined_entry.kind, per_kind_entry.kind);
        assert_eq!(combined_entry.provenance, per_kind_entry.provenance);
        assert_eq!(combined_entry.bounds, per_kind_entry.bounds);
        assert_eq!(combined_entry.colors, per_kind_entry.colors);
        assert_eq!(combined_entry.capabilities, per_kind_entry.capabilities);
    }

    assert_eq!(combined.entries[0].id, vectors.entries[0].id);
    assert_eq!(combined.entries[1].id.sequence, 1);
    assert_eq!(texts.entries[0].id.sequence, 0);
    assert_ne!(combined.entries[1].id.digest, texts.entries[0].id.digest);
    assert_eq!(combined.entries[2].id.sequence, 2);
    assert_eq!(vectors.entries[1].id.sequence, 1);
    assert_ne!(combined.entries[2].id.digest, vectors.entries[1].id.digest);
    assert_eq!(combined.entries[3].id.sequence, 3);
    assert_eq!(texts.entries[1].id.sequence, 1);
    assert_ne!(combined.entries[3].id.digest, texts.entries[1].id.digest);
    Ok(())
}

#[test]
fn combined_inventory_object_ids_are_deterministic() -> Result<(), String> {
    let input = b"0.4 g f /Im1 Do (Hi) Tj /Fm1 Do";
    let images = [PdfName(b"Im1".to_vec())];
    let forms = [PdfName(b"Fm1".to_vec())];
    let first = combined_inventory(input, &ContentScope::Page, &images, &forms)?;
    let second = combined_inventory(input, &ContentScope::Page, &images, &forms)?;

    assert_eq!(first, second);
    Ok(())
}

#[test]
fn streaming_build_inventory_equals_events_path_for_mixed_stream() -> Result<(), String> {
    // Many non-entry / path-construction operators (cm, m, l, h, W, n, gs, BT,
    // semantic Tf, ET, re) surround only four entry-producing operators (f, Tj,
    // /Im1 Do, /Fm1 Do): the many-no-op/few-entry case the streaming driver targets.
    let input: &[u8] = b"q 1 0 0 1 5 5 cm 10 20 m 30 40 l 50 60 l h W n /GS1 gs \
        0.4 g f BT /F1 12 Tf (Hi) Tj ET /Im1 Do 5 5 10 10 re n /Fm1 Do Q";
    let images = [PdfName(b"Im1".to_vec())];
    let forms = [PdfName(b"Fm1".to_vec())];
    let page = PageIndex(2);
    let scope = ContentScope::Page;

    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;

    let streamed = build_inventory(input, &assembled.records, page, &scope, &images, &forms)
        .map_err(|error| format!("{error:?}"))?;

    let events =
        walk_graphics_state(input, &assembled.records).map_err(|error| format!("{error:?}"))?;
    let materialized = inventory_from_graphics_events(page, &scope, &events, &images, &forms);

    // Few entries, so the case is genuinely many-no-op/few-entry, and the
    // streaming path is bit-identical to feeding the full event slice.
    assert_eq!(streamed.len(), 4);
    assert_eq!(streamed, materialized);
    Ok(())
}

#[test]
fn streaming_build_inventory_surfaces_walk_error_after_last_entry() -> Result<(), String> {
    // `0.4 g f` produces the only inventory entry; the malformed `1 2 RG`
    // (three operands expected, two given) sits after it. The streaming driver
    // must keep walking past the last entry-producing operator and surface the
    // exact `GraphicsWalkError` (kind and range) the materializing walk returns.
    let input: &[u8] = b"0.4 g f 1 2 RG";
    let page = PageIndex(2);
    let scope = ContentScope::Page;

    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;

    let streamed_err = build_inventory(input, &assembled.records, page, &scope, &[], &[])
        .err()
        .ok_or("malformed RG after last entry should fail the streaming build")?;
    let events_err = walk_graphics_state(input, &assembled.records)
        .err()
        .ok_or("malformed RG should fail the materializing walk")?;

    assert_eq!(streamed_err, events_err);
    Ok(())
}
