//! End-to-end Form caller-state inheritance suite.
//!
//! These tests prove the DUAL-PATH inheritance contract through the production
//! pipeline (`build_pdf_inventory` / `build_page_inventory_with_forms`): every
//! descended Form starts from the exact caller graphics state at its `Do`
//! invocation, the invocation-specific template is rebuilt from that same
//! state, caller and callee stay isolated, and resource lookup remains a
//! separate Form-local axis.
//!
//! CLAIM BOUNDARY. Only inheritance of the fields currently represented by
//! `GraphicsStateSnapshot` is claimed (CTM, both colours, text rendering mode,
//! font selection, classified `ExtGState` state). Form `/Matrix`
//! classification/concatenation, `/BBox` clipping, clip-path state,
//! transparency-group entry resets (ISO 32000-1 §11.6.6), and complete
//! CTM/appearance correctness remain deliberately deferred and are NOT tested
//! or claimed here.

#![allow(clippy::expect_used)]

use presslint_inventory::InventoryEntry;

use super::form_inventory::{
    CATALOG, PAGE_WITH_FORM, PAGES, classic_pdf, expand_first_page_with_context, form_xobject,
    page_with_xobjects_object, stream_object,
};
use crate::{
    ColorSpace, ContentScope, EditCapability, FormWalkContext, InvocationFrame, InvocationPath,
    ObjectKind, PageIndex, PdfName,
    actions::{
        Action, ConvertColor, MutationBoundary, Recipe, RecipeStep, SkipReason, plan_recipe,
    },
    build_pdf_inventory,
    selectors::{Predicate, Selector},
};

const MAX: usize = 4096;

fn form_scope(name: &[u8]) -> ContentScope {
    ContentScope::FormXObject {
        name: PdfName(name.to_vec()),
    }
}

fn scoped_vectors<'a>(
    entries: &'a [InventoryEntry],
    scope: &ContentScope,
) -> Vec<&'a InventoryEntry> {
    entries
        .iter()
        .filter(|entry| entry.provenance.scope == *scope && entry.kind == ObjectKind::Vector)
        .collect()
}

#[test]
fn shared_form_inherits_each_callers_do_state_with_distinct_identities() {
    // One shared form `/A` with NO local colour operator, invoked after two
    // DIFFERENT caller colours: each invocation's expanded entry must observe
    // its own caller state, carry its own invocation path, and stay
    // deterministic across rebuilds.
    let page = page_with_xobjects_object("/A 4 0 R", 5);
    let form_a = form_xobject(4, "", b"0 0 30 30 re f");
    let page_content = stream_object(5, "", b"1 0 0 rg\n/A Do\n0 0 1 rg\n/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let entries = &report.inventory.entries;

    // Emission order: Do#1 invocation, A's red fill, Do#2 invocation, A's blue fill.
    assert_eq!(entries.len(), 4);
    assert_eq!(entries[0].kind, ObjectKind::FormXObject);
    assert_eq!(entries[2].kind, ObjectKind::FormXObject);

    let first = &entries[1];
    let second = &entries[3];
    assert_eq!(first.provenance.scope, form_scope(b"A"));
    assert_eq!(first.colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(first.colors[0].components, vec![1.0, 0.0, 0.0]);
    assert_eq!(second.colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(second.colors[0].components, vec![0.0, 0.0, 1.0]);

    // Invocation paths are preserved per invocation and the state-dependent
    // identities are distinct.
    let path = |ordinal| {
        Some(InvocationPath {
            frames: vec![InvocationFrame {
                ordinal,
                name: PdfName(b"A".to_vec()),
            }],
        })
    };
    assert_eq!(first.provenance.invocation, path(0));
    assert_eq!(second.provenance.invocation, path(1));
    assert_ne!(first.id.digest, second.id.digest);

    // Deterministic: rebuilding yields byte-identical entries.
    let again = build_pdf_inventory(&source, MAX).expect("inventory should rebuild");
    assert_eq!(report.inventory, again.inventory);
}

#[test]
fn shared_form_same_caller_state_stays_distinct_by_invocation_path() {
    // Two invocations under the SAME caller state still get distinct digests
    // through their distinct invocation paths and sequences.
    let page = page_with_xobjects_object("/A 4 0 R", 5);
    let form_a = form_xobject(4, "", b"0 0 30 30 re f");
    let page_content = stream_object(5, "", b"1 0 0 rg\n/A Do\n/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let markings = scoped_vectors(&report.inventory.entries, &form_scope(b"A"));

    assert_eq!(markings.len(), 2);
    assert_eq!(markings[0].colors, markings[1].colors);
    assert_eq!(markings[0].colors[0].components, vec![1.0, 0.0, 0.0]);
    assert_ne!(markings[0].id.digest, markings[1].id.digest);
    assert_ne!(
        markings[0].provenance.invocation,
        markings[1].provenance.invocation
    );
}

#[test]
fn nested_form_inherits_from_immediate_caller_not_page_or_default() {
    // Page sets 0.3 g and invokes A; A sets 0.7 g and invokes B. B paints with
    // NO local colour: it must observe A's 0.7, not the page's 0.3 and not the
    // page-default black.
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"0.7 g\n/B Do");
    let form_b = form_xobject(5, "", b"0 0 10 10 re f");
    let page_content = stream_object(6, "", b"0.3 g\n/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());
    let b_markings = scoped_vectors(&expanded.inventory.entries, &form_scope(b"B"));

    assert_eq!(b_markings.len(), 1);
    assert_eq!(b_markings[0].colors[0].space, ColorSpace::DeviceGray);
    assert_eq!(b_markings[0].colors[0].components, vec![0.7]);
    assert_eq!(b_markings[0].id.page, PageIndex(0));
    assert!(expanded.form_skipped.is_empty());
}

#[test]
fn form_mutations_do_not_leak_to_caller_or_sibling_invocation() {
    // `/A` mutates colour and text rendering mode; the caller paint AFTER the
    // return and the SIBLING form `/B` must both keep the caller's red state.
    let page = page_with_xobjects_object("/A 4 0 R /B 5 0 R", 6);
    let form_a = form_xobject(4, "", b"0 1 0 rg\n2 Tr\n0 0 10 10 re f");
    let form_b = form_xobject(5, "", b"0 0 10 10 re f");
    let page_content = stream_object(6, "", b"1 0 0 rg\n/A Do\n5 5 20 20 re f\n/B Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let entries = &report.inventory.entries;

    // A's own marking sees A's local mutation.
    let a_markings = scoped_vectors(entries, &form_scope(b"A"));
    assert_eq!(a_markings[0].colors[0].components, vec![0.0, 1.0, 0.0]);

    // The caller paint after A's return still sees the caller's red.
    let page_markings = scoped_vectors(entries, &ContentScope::Page);
    assert_eq!(page_markings.len(), 1);
    assert_eq!(page_markings[0].colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(page_markings[0].colors[0].components, vec![1.0, 0.0, 0.0]);

    // The sibling B inherits from ITS OWN `Do` state: red, not A's green.
    let b_markings = scoped_vectors(entries, &form_scope(b"B"));
    assert_eq!(b_markings[0].colors[0].components, vec![1.0, 0.0, 0.0]);
}

#[test]
fn q_scoped_state_seeds_the_form_and_caller_restores_after_q() {
    // q / state change / Do / Q: the form starts from the INNER (0.8) state and
    // the caller paint after `Q` sees the pre-`q` (0.2) state.
    let form = form_xobject(4, "", b"0 0 10 10 re f");
    let page_content = stream_object(5, "", b"0.2 g\nq\n0.8 g\n/Fm Do\nQ\n0 0 10 10 re f");
    let source = classic_pdf(&[CATALOG, PAGES, PAGE_WITH_FORM, &form, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let entries = &report.inventory.entries;

    let form_markings = scoped_vectors(entries, &form_scope(b"Fm"));
    assert_eq!(form_markings[0].colors[0].components, vec![0.8]);

    let page_markings = scoped_vectors(entries, &ContentScope::Page);
    assert_eq!(page_markings.len(), 1);
    assert_eq!(page_markings[0].colors[0].components, vec![0.2]);
}

/// A `/N 4` ICC-profile stream object (object 6) for `[ /ICCBased 6 0 R ]`.
const ICC_N4: &[u8] = b"6 0 obj\n<< /N 4 /Length 1 >>\nstream\nx\nendstream\nendobj\n";
/// A shallow tint-transform function stream (object 7) for `/Separation`.
const TINT_FN: &[u8] =
    b"7 0 obj\n<< /FunctionType 2 /Domain [ 0 1 ] /N 1 /Length 0 >>\nstream\n\nendstream\nendobj\n";

#[test]
fn inherited_color_persists_until_form_local_cs_resolves_form_env_never_page_env() {
    // Resource lookup stays a SEPARATE axis from state inheritance: the page
    // and the form both spell `/CS0`, with DIFFERENT classifications. The
    // form's first paint (before any local colour operator) observes the
    // caller's inherited effective red; its `/CS0 cs 0.5 scn` then resolves
    // against the FORM-local environment (Separation), never the page's
    // ICCBased `/CS0`.
    let page: &[u8] = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm 4 0 R >> /ColorSpace << /CS0 [ /ICCBased 6 0 R ] >> >> /Contents 5 0 R >>\nendobj\n";
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /ColorSpace << /CS0 [ /Separation /PANTONE /DeviceCMYK 7 0 R ] >> >>",
        b"0 0 20 20 re f\n/CS0 cs 0.5 scn\n0 0 20 20 re f",
    );
    let page_content = stream_object(5, "", b"1 0 0 rg\n/Fm Do");
    let source = classic_pdf(&[CATALOG, PAGES, page, &form, &page_content, ICC_N4, TINT_FN]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let markings = scoped_vectors(&report.inventory.entries, &form_scope(b"Fm"));

    assert_eq!(markings.len(), 2);
    // Inherited caller-effective colour before any Form-local colour operator.
    assert_eq!(markings[0].colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(markings[0].colors[0].components, vec![1.0, 0.0, 0.0]);
    // Form-local `/CS0` resolves to the FORM's Separation, not the page's ICC.
    assert_eq!(markings[1].colors[0].space, ColorSpace::Separation);
    assert_eq!(
        markings[1].colors[0].spot_name,
        Some(PdfName(b"PANTONE".to_vec()))
    );
    assert_eq!(markings[1].colors[0].components, vec![0.5]);
    assert!(
        markings
            .iter()
            .flat_map(|entry| entry.colors.iter())
            .all(|color| color.space != ColorSpace::IccBased)
    );
}

#[test]
fn gs_inside_form_keeps_template_and_descended_interpretation_coherent() {
    // The form applies its OWN `/ExtGState /GS0` before painting. Template and
    // descended walk now share BOTH the caller seed and the Form-local
    // `ExtGState` env, so the `gs` cannot make them diverge: the expanded entry
    // still carries the inherited caller colour and its identity is stable
    // across rebuilds (classified `ExtGState` facts stay digest-neutral).
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /ExtGState << /GS0 << /op true >> >> >>",
        b"/GS0 gs\n0 0 10 10 re f",
    );
    let page_content = stream_object(5, "", b"1 0 0 rg\n/Fm Do");
    let source = classic_pdf(&[CATALOG, PAGES, PAGE_WITH_FORM, &form, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let markings = scoped_vectors(&report.inventory.entries, &form_scope(b"Fm"));

    assert_eq!(markings.len(), 1);
    assert_eq!(markings[0].colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(markings[0].colors[0].components, vec![1.0, 0.0, 0.0]);

    let again = build_pdf_inventory(&source, MAX).expect("inventory should rebuild");
    assert_eq!(report.inventory, again.inventory);
}

#[test]
fn inherited_caller_source_never_plans_a_form_local_color_boundary() {
    // The caller's `rg` source is inherited by the Form's first fill, but its
    // bare decoded range does not identify the owning stream. Inventory must
    // therefore withhold rewrite capability for the Form entry. The unchanged
    // planner reports UnsupportedCapability and publishes no Form-local
    // ContentStreamOperand boundary, while a later page-local fill using that
    // same sourced page colour remains eligible exactly as before.
    let form = form_xobject(4, "", b"0 0 10 10 re f");
    let page_content = stream_object(5, "", b"1 0 0 rg\n/Fm Do\n0 0 10 10 re f");
    let source = classic_pdf(&[CATALOG, PAGES, PAGE_WITH_FORM, &form, &page_content]);
    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let form_marking = scoped_vectors(&report.inventory.entries, &form_scope(b"Fm"))[0];
    let page_marking = scoped_vectors(&report.inventory.entries, &ContentScope::Page)[0];

    assert!(form_marking.colors[0].source.is_some());
    assert!(
        !form_marking
            .capabilities
            .contains(&EditCapability::RewriteColorOperand)
    );
    assert!(
        page_marking
            .capabilities
            .contains(&EditCapability::RewriteColorOperand)
    );

    let recipe = Recipe {
        schema_version: 1,
        steps: vec![RecipeStep {
            select: Selector::Predicate {
                predicate: Predicate::ObjectKind {
                    object_kind: ObjectKind::Vector,
                },
            },
            action: Action::ConvertColor(ConvertColor {
                target: "test-output".to_owned(),
            }),
        }],
    };
    let plan = plan_recipe(&recipe, &report.inventory);
    let step = &plan.steps[0];

    assert_eq!(step.targets, vec![page_marking.id.clone()]);
    assert_eq!(step.patches.len(), 1);
    assert!(matches!(
        &step.patches[0].boundary,
        MutationBoundary::ContentStreamOperand {
            scope: ContentScope::Page,
            ..
        }
    ));
    assert_eq!(step.skipped.len(), 1);
    assert_eq!(step.skipped[0].object, form_marking.id);
    assert_eq!(
        step.skipped[0].reason,
        SkipReason::UnsupportedCapability {
            required: EditCapability::RewriteColorOperand,
        }
    );
    assert!(step.patches.iter().all(|patch| !matches!(
        &patch.boundary,
        MutationBoundary::ContentStreamOperand {
            scope: ContentScope::FormXObject { .. },
            ..
        }
    )));
}
