//! End-to-end `ExtGState` wiring tests (Phase 1-3).
//!
//! These prove the umbrella bridge feeds real classified `ExtGState`
//! environments into the paint walk: a page whose content executes `gs` against
//! a classified resource carries the classified params on the walked snapshot; a
//! `gs` on a MISSING resource in a non-empty env goes all-`Unresolved`; a page
//! without `/ExtGState` keeps the T156 legacy no-op; and identity is unchanged
//! because the snapshot never feeds the digest.
//!
//! The env is always built from a REAL synthetic PDF through the same pdf-side
//! inspector and umbrella mapping the production bridges use; the behavioural
//! effect is then observed with a direct paint walk (the machine's flat ops are
//! not surfaced through the inventory report, and the snapshot deliberately does
//! not reach an `InventoryEntry`).

#![allow(clippy::expect_used)]

use presslint_inventory::{
    AlphaClass, BlendModeClass, ColorSpaceEnv, ExtGStateEnv, ExtGStateResource,
    GraphicsStateWalker, GsParam, OverprintMode, PaintOp, SoftMaskClass,
};
use presslint_pdf::{
    DocumentAccessBackend, ObjectLookup, inspect_document_access,
    inspect_document_page_extgstate_resources_with_lookup,
};
use presslint_syntax::{assemble_operators, tokenize};

use crate::document_inventory::page_extgstate_env;
use crate::{ColorSpace, ObjectKind, build_pdf_inventory};

use super::form_inventory::{CATALOG, PAGES, classic_pdf, stream_object};

const MAX: usize = 4096;

/// A single-page classic PDF whose page `/Resources /ExtGState` is `dict` and
/// whose one raw content stream is `content`.
fn page_with_extgstate_pdf(dict: &str, content: &[u8]) -> Vec<u8> {
    let page = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /ExtGState << {dict} >> >> /Contents 4 0 R >>\nendobj\n"
    )
    .into_bytes();
    let content_object = stream_object(4, "", content);
    classic_pdf(&[CATALOG, PAGES, &page, &content_object])
}

/// A single-page classic PDF with NO `/ExtGState` resources at all.
fn page_without_extgstate_pdf(content: &[u8]) -> Vec<u8> {
    let page: &[u8] = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n";
    let content_object = stream_object(4, "", content);
    classic_pdf(&[CATALOG, PAGES, page, &content_object])
}

/// Build the page-scope `ExtGState` env exactly as the production bridges do:
/// through the real pdf-side inspector and the umbrella `page_extgstate_env`
/// mapping.
fn page_env(source: &[u8]) -> Vec<ExtGStateResource> {
    let access = inspect_document_access(source).expect("document access");
    let lookup = match &access.backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    };
    let report = inspect_document_page_extgstate_resources_with_lookup(
        source,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .expect("extgstate report");
    page_extgstate_env(&report.pages[0])
}

/// Directly walk `content` against a borrowed `ExtGState` env, materialising the
/// paint op for every operator.
fn walk(content: &[u8], extgstates: &[ExtGStateResource]) -> Vec<PaintOp> {
    let tokens = tokenize(content).expect("tokenize");
    let assembled = assemble_operators(&tokens).expect("assemble");
    let mut walker =
        GraphicsStateWalker::with_envs(ColorSpaceEnv::empty(), ExtGStateEnv::new(extgstates));
    assembled
        .records
        .iter()
        .enumerate()
        .map(|(index, record)| walker.step(content, index, record).expect("walk step"))
        .collect()
}

fn find(env: &[ExtGStateResource], name: &[u8]) -> ExtGStateResource {
    env.iter()
        .find(|resource| resource.name.0 == name)
        .expect("resource present in env")
        .clone()
}

#[test]
fn page_scope_gs_hit_carries_classified_op_true() {
    // The page classifies `/GS0` with `op true`; walking its `gs` must set the
    // non-stroking overprint flag on the snapshot.
    let source = page_with_extgstate_pdf("/GS0 << /op true >>", b"/GS0 gs");

    let env = page_env(&source);
    assert_eq!(find(&env, b"GS0").params.overprint_fill, GsParam::Set(true));

    let ops = walk(b"/GS0 gs", &env);
    assert_eq!(ops[0].state.extgstate.overprint_fill, GsParam::Set(true));
    // A key the dictionary does not carry stays at the page default.
    assert_eq!(ops[0].state.extgstate.overprint_stroke, GsParam::Default);
}

#[test]
fn set_unset_alpha_opm_and_smask_all_map_through() {
    let source = page_with_extgstate_pdf(
        "/GS0 << /OP true /CA 0.5 /OPM 1 /SMask /None >>",
        b"/GS0 gs",
    );

    let ops = walk(b"/GS0 gs", &page_env(&source));
    let after = &ops[0].state.extgstate;
    assert_eq!(after.overprint_stroke, GsParam::Set(true));
    assert_eq!(after.stroke_alpha, GsParam::Set(AlphaClass::NonOpaque));
    assert_eq!(after.overprint_mode, GsParam::Set(OverprintMode::One));
    assert_eq!(after.soft_mask, GsParam::Set(SoftMaskClass::None));
    // `/op`/`/ca`/`/BM` were not written, so they stay at the page default.
    assert_eq!(after.overprint_fill, GsParam::Default);
    assert_eq!(after.fill_alpha, GsParam::Default);
    assert_eq!(after.blend_mode, GsParam::Default);
}

#[test]
fn opaque_alpha_maps_to_opaque() {
    let source = page_with_extgstate_pdf("/GS0 << /ca 1.0 >>", b"/GS0 gs");
    let ops = walk(b"/GS0 gs", &page_env(&source));
    assert_eq!(
        ops[0].state.extgstate.fill_alpha,
        GsParam::Set(AlphaClass::Opaque)
    );
}

#[test]
fn malformed_value_maps_to_unclassified() {
    // `/op` must be a boolean; a numeric value is a malformed VALUE, which maps
    // to `Unclassified` for that single parameter (not the whole resource).
    let source = page_with_extgstate_pdf("/GS0 << /op 3 >>", b"/GS0 gs");

    let env = page_env(&source);
    assert_eq!(
        find(&env, b"GS0").params.overprint_fill,
        GsParam::Unclassified
    );

    let ops = walk(b"/GS0 gs", &env);
    assert_eq!(ops[0].state.extgstate.overprint_fill, GsParam::Unclassified);
}

#[test]
fn blend_mode_names_classify_per_iso_32000() {
    let source = page_with_extgstate_pdf(
        "/GNormal << /BM /Normal >> /GMul << /BM /Multiply >> /GComp << /BM /Compatible >> \
         /GOther << /BM /Frobnicate >> /GArray << /BM [ /Multiply /Normal ] >>",
        b"/GNormal gs",
    );

    let env = page_env(&source);
    assert_eq!(
        find(&env, b"GNormal").params.blend_mode,
        GsParam::Set(BlendModeClass::Normal)
    );
    // The standard separable/non-separable names are the real non-Normal set.
    assert_eq!(
        find(&env, b"GMul").params.blend_mode,
        GsParam::Set(BlendModeClass::NonNormal)
    );
    // `Compatible` is the deprecated `/Normal` alias (ISO 32000-1 §11.3.5).
    assert_eq!(
        find(&env, b"GComp").params.blend_mode,
        GsParam::Set(BlendModeClass::Normal)
    );
    // An unrecognised name and an array-form list are both present-but-other.
    assert_eq!(
        find(&env, b"GOther").params.blend_mode,
        GsParam::Set(BlendModeClass::OtherNamed)
    );
    assert_eq!(
        find(&env, b"GArray").params.blend_mode,
        GsParam::Set(BlendModeClass::OtherNamed)
    );
}

#[test]
fn gs_miss_on_non_empty_env_is_all_unresolved() {
    // The env is non-empty (it classifies `/GS0`) but the content invokes `/GS9`,
    // which it does not carry: every param goes `Unresolved`.
    let source = page_with_extgstate_pdf("/GS0 << /op true >>", b"/GS9 gs");

    let env = page_env(&source);
    assert!(!env.is_empty());

    let ops = walk(b"/GS9 gs", &env);
    let after = &ops[0].state.extgstate;
    assert_eq!(after.overprint_stroke, GsParam::Unresolved);
    assert_eq!(after.overprint_fill, GsParam::Unresolved);
    assert_eq!(after.overprint_mode, GsParam::Unresolved);
    assert_eq!(after.stroke_alpha, GsParam::Unresolved);
    assert_eq!(after.fill_alpha, GsParam::Unresolved);
    assert_eq!(after.blend_mode, GsParam::Unresolved);
    assert_eq!(after.soft_mask, GsParam::Unresolved);
}

#[test]
fn unresolved_entry_surfaces_in_env_as_all_unresolved() {
    // `/GS0` is an indirect reference to a missing object: the pdf side records a
    // skip, and the umbrella surfaces it in the env with every param
    // `Unresolved`, so a `gs` on that name is honestly unknown, not swallowed.
    let source = page_with_extgstate_pdf("/GS0 99 0 R", b"/GS0 gs");

    let env = page_env(&source);
    let gs0 = find(&env, b"GS0");
    assert_eq!(gs0.params.overprint_fill, GsParam::Unresolved);
    assert_eq!(gs0.params.blend_mode, GsParam::Unresolved);

    let ops = walk(b"/GS0 gs", &env);
    assert_eq!(ops[0].state.extgstate.overprint_fill, GsParam::Unresolved);
    assert_eq!(ops[0].state.extgstate.soft_mask, GsParam::Unresolved);
}

#[test]
fn page_without_extgstate_keeps_legacy_no_op() {
    // No `/ExtGState` resources: the env is empty, so `gs` mutates nothing and the
    // snapshot stays at the page default (T156 legacy rule).
    let source = page_without_extgstate_pdf(b"/GS0 gs");

    let env = page_env(&source);
    assert!(env.is_empty());

    let ops = walk(b"/GS0 gs", &env);
    let after = &ops[0].state.extgstate;
    assert_eq!(after.overprint_fill, GsParam::Default);
    assert_eq!(after.overprint_stroke, GsParam::Default);
    assert_eq!(after.blend_mode, GsParam::Default);
}

#[test]
fn populated_env_leaves_page_inventory_identity_untouched() {
    // Two pages paint the identical vector through a `gs`; one classifies `/GS0`
    // (populated env), the other does not (empty env). The snapshot never feeds an
    // `InventoryEntry`, so the two inventories — kinds, colours, and digests — are
    // byte-for-byte equal.
    let content: &[u8] = b"/GS0 gs\n0 0 1 rg\n0 0 10 10 re\nf";
    let with_gs = page_with_extgstate_pdf("/GS0 << /op true /BM /Multiply >>", content);
    let without_gs = page_without_extgstate_pdf(content);

    let populated = build_pdf_inventory(&with_gs, MAX).expect("inventory should build");
    let empty = build_pdf_inventory(&without_gs, MAX).expect("inventory should build");

    // The env is genuinely populated for the first document.
    assert!(!page_env(&with_gs).is_empty());
    assert!(page_env(&without_gs).is_empty());

    // Identity untouched: same entries, same digests.
    assert_eq!(populated.inventory.entries, empty.inventory.entries);
    // And the content really did paint the expected classified vector.
    assert!(populated.inventory.entries.iter().any(|entry| {
        entry.kind == ObjectKind::Vector
            && entry
                .colors
                .iter()
                .any(|color| color.space == ColorSpace::DeviceRgb)
    }));
}
