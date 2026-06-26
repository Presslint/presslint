use presslint_core::{ByteRange, ContentScope, EditCapability, ObjectKind, PageIndex, PdfName};

use super::{form_inventory, image_inventory};

#[test]
fn form_inventory_includes_only_declared_form_xobject_names() -> Result<(), String> {
    let inventory = form_inventory(
        b"/Fm1 Do /Im1 Do /Fm2 Do",
        &ContentScope::Page,
        &[PdfName(b"Fm2".to_vec()), PdfName(b"Fm1".to_vec())],
    )?;

    assert_eq!(inventory.entries.len(), 2);
    assert_eq!(inventory.entries[0].kind, ObjectKind::FormXObject);
    assert_eq!(inventory.entries[0].id.sequence, 0);
    assert_eq!(inventory.entries[1].id.sequence, 1);
    let ranges = [
        inventory.entries[0].provenance.range,
        inventory.entries[1].provenance.range,
    ];
    assert_eq!(
        ranges,
        [
            Some(ByteRange { start: 0, end: 7 }),
            Some(ByteRange { start: 16, end: 23 })
        ]
    );
    Ok(())
}

#[test]
fn form_inventory_carries_do_provenance_and_read_only_capability() -> Result<(), String> {
    let scope = ContentScope::FormXObject {
        name: PdfName(b"Outer".to_vec()),
    };
    let inventory = form_inventory(b"q /Logo Do Q", &scope, &[PdfName(b"Logo".to_vec())])?;
    let entry = inventory.entries.first().ok_or("missing form entry")?;

    assert_eq!(entry.kind, ObjectKind::FormXObject);
    assert_eq!(entry.provenance.page, PageIndex(2));
    assert_eq!(entry.provenance.scope, scope);
    let range = Some(ByteRange { start: 2, end: 10 });
    assert_eq!(entry.provenance.range, range);
    assert_eq!(entry.bounds, None);
    assert!(entry.colors.is_empty());
    assert_eq!(entry.capabilities, vec![EditCapability::ReadOnly]);
    Ok(())
}

#[test]
fn form_and_image_inventory_filter_the_same_do_events_independently() -> Result<(), String> {
    let input = b"/Im1 Do /Fm1 Do /Other Do";
    let images = image_inventory(input, &ContentScope::Page, &[PdfName(b"Im1".to_vec())])?;
    let forms = form_inventory(input, &ContentScope::Page, &[PdfName(b"Fm1".to_vec())])?;

    assert_eq!(images.entries.len(), 1);
    assert_eq!(images.entries[0].kind, ObjectKind::Image);
    let image_range = Some(ByteRange { start: 0, end: 7 });
    assert_eq!(images.entries[0].provenance.range, image_range);

    assert_eq!(forms.entries.len(), 1);
    assert_eq!(forms.entries[0].kind, ObjectKind::FormXObject);
    let form_range = Some(ByteRange { start: 8, end: 15 });
    assert_eq!(forms.entries[0].provenance.range, form_range);
    Ok(())
}

#[test]
fn form_inventory_object_ids_are_deterministic() -> Result<(), String> {
    let names = [PdfName(b"Fm1".to_vec()), PdfName(b"Fm2".to_vec())];
    let first = form_inventory(b"/Fm1 Do /Image Do /Fm2 Do", &ContentScope::Page, &names)?;
    let second = form_inventory(b"/Fm1 Do /Image Do /Fm2 Do", &ContentScope::Page, &names)?;

    assert_eq!(first, second);
    assert_eq!(first.entries[0].id.page, PageIndex(2));
    assert_eq!(first.entries[0].id.sequence, 0);
    assert_eq!(first.entries[1].id.sequence, 1);
    assert_ne!(first.entries[0].id.digest, first.entries[1].id.digest);
    Ok(())
}
