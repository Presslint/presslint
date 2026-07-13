//! End-to-end page/Form font-environment and text-v5 wiring tests.

#![allow(clippy::expect_used)]

use presslint_inventory::{FontBindingTarget, FontSelectionState};
use presslint_pdf::{
    ObjectLookup, inspect_classic_document_access, inspect_document_page_content_extents,
    inspect_document_page_font_resources, inspect_form_font_resources,
};

use crate::document_inventory::{font_env_resources, page_font_bindings_at};
use crate::{ContentScope, ObjectKind, build_classic_pdf_inventory, build_pdf_inventory};

use super::form_inventory::{classic_pdf, stream_object};

const CATALOG: &[u8] = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
const CATALOG_PADDED: &[u8] = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R        >>\nendobj\n";
const PAGES: &[u8] = b"2 0 obj\n<< /Type /Pages /Kids [ 4 0 R ] /Count 1 >>\nendobj\n";
const FONT: &[u8] = b"3 0 obj\n<< /Type /Font /Subtype /Type1 >>\nendobj\n";
const MAX: usize = 4096;

fn tf_pdf(catalog: &[u8], binding_name: &str, content_name: &str) -> Vec<u8> {
    let page = format!(
        "4 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /{binding_name} 3 0 R >> >> /Contents 5 0 R >>\nendobj\n"
    )
    .into_bytes();
    let content = stream_object(5, "", format!("/{content_name} 12 Tf (A) Tj").as_bytes());
    classic_pdf(&[catalog, PAGES, FONT, &page, &content])
}

fn direct_gs_pdf() -> Vec<u8> {
    let page: &[u8] = b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ExtGState << /GS000 6 0 R >> >> /Contents 5 0 R >>\nendobj\n";
    let content = stream_object(5, "", b"/GS000 gs (A) Tj");
    let extgstate: &[u8] = b"6 0 obj\n<< /Type /ExtGState /Font [ 3 0 R 12 ] >>\nendobj\n";
    classic_pdf(&[CATALOG, PAGES, FONT, page, &content, extgstate])
}

fn text_digest(source: &[u8]) -> [u8; 32] {
    let report = build_pdf_inventory(source, MAX).expect("inventory should build");
    let texts: Vec<_> = report
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.kind == ObjectKind::Text)
        .collect();
    assert_eq!(texts.len(), 1);
    texts[0].id.digest
}

fn page_bindings(source: &[u8]) -> Vec<presslint_inventory::FontBinding> {
    let access = inspect_classic_document_access(source).expect("classic access");
    let report = inspect_document_page_font_resources(
        source,
        &access.xref_table,
        access.page_tree_root.object_byte_offset,
    )
    .expect("font report");
    font_env_resources(&report.pages[0].fonts, &report.pages[0].skipped)
        .expect("known font namespace")
}

#[test]
fn aliases_and_direct_gs_converge_on_the_same_resolved_text_v5_component() {
    let f1 = tf_pdf(CATALOG, "F1", "F1");
    let f2 = tf_pdf(CATALOG, "F2", "F2");
    let escaped = tf_pdf(CATALOG, "F#31", "F1");
    let gs = direct_gs_pdf();

    let expected = text_digest(&f1);
    assert_eq!(text_digest(&f2), expected);
    assert_eq!(text_digest(&escaped), expected);
    assert_eq!(text_digest(&gs), expected);

    let newer_offset = tf_pdf(CATALOG_PADDED, "F1", "F1");
    assert_ne!(text_digest(&newer_offset), expected);
}

#[test]
fn classic_and_neutral_bridges_produce_identical_resolved_font_identity() {
    let source = tf_pdf(CATALOG, "F#31", "F1");
    let classic = build_classic_pdf_inventory(&source, MAX).expect("classic inventory");
    let neutral = build_pdf_inventory(&source, MAX).expect("neutral inventory");
    assert_eq!(classic.inventory, neutral.inventory);

    let again = build_pdf_inventory(&source, MAX).expect("deterministic rebuild");
    assert_eq!(neutral.inventory, again.inventory);
}

#[test]
fn page_report_join_uses_full_identity_and_semantic_collision_poisons() {
    let source = tf_pdf(CATALOG, "F#31", "F1");
    let access = inspect_classic_document_access(&source).expect("classic access");
    let report = inspect_document_page_font_resources(
        &source,
        &access.xref_table,
        access.page_tree_root.object_byte_offset,
    )
    .expect("font report");
    let extents = inspect_document_page_content_extents(
        &source,
        &access.xref_table,
        access.page_tree_root.object_byte_offset,
    )
    .expect("content extents");
    let joined =
        page_font_bindings_at(Some(&report.pages), &extents.pages[0]).expect("exact report join");
    assert!(matches!(
        joined[0].target(),
        FontBindingTarget::Resolved(font) if font.object_number == 3
    ));

    for mismatch in [0, 1, 2] {
        let mut pages = report.pages.clone();
        match mismatch {
            0 => pages[0].ordinal += 1,
            1 => pages[0].page_reference.object_number += 1,
            _ => pages[0].page_object_byte_offset += 1,
        }
        assert!(page_font_bindings_at(Some(&pages), &extents.pages[0]).is_none());
    }

    let page: &[u8] = b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 3 0 R /F#31 3 0 R >> >> /Contents 5 0 R >>\nendobj\n";
    let content = stream_object(5, "", b"/F1 12 Tf (A) Tj");
    let collision = classic_pdf(&[CATALOG, PAGES, FONT, page, &content]);
    let bindings = page_bindings(&collision);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].target(), FontBindingTarget::Unresolved);
}

#[test]
fn direct_dictionary_is_present_but_unresolved_without_fabricated_identity() {
    let page: &[u8] = b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 >> >> >> /Contents 5 0 R >>\nendobj\n";
    let content = stream_object(5, "", b"/F1 12 Tf (A) Tj");
    let source = classic_pdf(&[CATALOG, PAGES, FONT, page, &content]);
    let bindings = page_bindings(&source);
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].target(), FontBindingTarget::Unresolved);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    assert_eq!(report.inventory.entries[0].kind, ObjectKind::Text);
}

#[test]
fn form_own_scope_rebinds_same_name_without_page_fallback() {
    let page: &[u8] = b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /A 6 0 R >> /Font << /F1 3 0 R >> >> /Contents 5 0 R >>\nendobj\n";
    let page_content = stream_object(5, "", b"/F1 9 Tf /A Do (P) Tj");
    let form = stream_object(
        6,
        " /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Resources << /Font << /F1 7 0 R >> >>",
        b"/F1 12 Tf (A) Tj",
    );
    let form_font: &[u8] = b"7 0 obj\n<< /Type /Font /Subtype /TrueType >>\nendobj\n";
    let source = classic_pdf(&[CATALOG, PAGES, FONT, page, &page_content, &form, form_font]);

    let access = inspect_classic_document_access(&source).expect("classic access");
    let lookup = ObjectLookup::ClassicXref(&access.xref_table);
    let form_report = inspect_form_font_resources(
        &source,
        lookup,
        source
            .windows(b"6 0 obj".len())
            .position(|window| window == b"6 0 obj")
            .expect("form offset"),
    );
    let form_bindings =
        font_env_resources(&form_report.fonts, &form_report.skipped).expect("known form fonts");
    let page_target = page_bindings(&source)[0].target();
    assert!(matches!(page_target, FontBindingTarget::Resolved(_)));
    let FontBindingTarget::Resolved(page_font) = page_target else {
        return;
    };
    let form_target = form_bindings[0].target();
    assert!(matches!(form_target, FontBindingTarget::Resolved(_)));
    let FontBindingTarget::Resolved(form_font) = form_target else {
        return;
    };
    assert_eq!(page_font.object_number, 3);
    assert_eq!(form_font.object_number, 7);

    let inventory = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let scopes: Vec<_> = inventory
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.kind == ObjectKind::Text)
        .map(|entry| entry.provenance.scope.clone())
        .collect();
    assert!(scopes.contains(&ContentScope::Page));
    assert!(scopes.contains(&ContentScope::FormXObject {
        name: crate::PdfName(b"A".to_vec()),
    }));
}

#[test]
fn resolved_state_shape_remains_additive_to_public_inventory_types() {
    let state = FontSelectionState::ResolvedIndirect {
        object_number: 3,
        generation: 0,
        object_byte_offset: 99,
        size: -0.0,
    };
    assert!(matches!(state, FontSelectionState::ResolvedIndirect { .. }));
}
