#![allow(clippy::expect_used)]

use presslint_inventory::{
    ColorSpaceEnv, ExtGStateEnv, ExtGStateResource, GraphicsStateWalker, GsParam, PaintOp,
};
use presslint_pdf::{
    DocumentAccessBackend, ObjectLookup, inspect_document_access,
    inspect_document_page_content_extents_with_lookup,
    inspect_document_page_xobject_resources_with_lookup, inspect_form_extgstate_resources,
};
use presslint_syntax::{assemble_operators, tokenize};
use presslint_types::PageIndex;

use crate::document_inventory::{extgstate_env_resources, inventory_names};
use crate::{
    ColorSpace, ContentScope, FormExpandedInventory, FormWalkContext, InvocationFrame,
    InvocationPath, ObjectKind, PdfInventorySkip, PdfName, SkippedFormInventoryReason,
    build_classic_pdf_inventory, build_page_inventory_with_forms, build_pdf_inventory,
};

const MAX: usize = 4096;

/// Build a classic-xref PDF from object bodies numbered `1..=objects.len()`.
pub(super) fn classic_pdf(objects: &[&[u8]]) -> Vec<u8> {
    let mut source = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for object in objects {
        offsets.push(source.len());
        source.extend_from_slice(object);
    }

    let xref_offset = source.len();
    let object_count = objects.len() + 1;
    source.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
    source.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    source.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );
    source
}

/// Build one `N 0 obj` stream object whose `/Length` matches `data` exactly.
pub(super) fn stream_object(number: u32, dict_extra: &str, data: &[u8]) -> Vec<u8> {
    let mut object = format!(
        "{number} 0 obj\n<< /Length {}{} >>\nstream\n",
        data.len(),
        dict_extra
    )
    .into_bytes();
    object.extend_from_slice(data);
    object.extend_from_slice(b"\nendstream\nendobj\n");
    object
}

pub(super) const CATALOG: &[u8] = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
pub(super) const PAGES: &[u8] = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
pub(super) const PAGE_WITH_FORM: &[u8] = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm 4 0 R >> >> /Contents 5 0 R >>\nendobj\n";

pub(super) fn page_with_xobjects_object(xobjects: &str, contents: u32) -> Vec<u8> {
    format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << {xobjects} >> >> /Contents {contents} 0 R >>\nendobj\n"
    )
    .into_bytes()
}

pub(super) fn form_xobject(number: u32, xobjects: &str, content: &[u8]) -> Vec<u8> {
    let resources = if xobjects.is_empty() {
        String::new()
    } else {
        format!(" /Resources << /XObject << {xobjects} >> >>")
    };
    stream_object(
        number,
        &format!(" /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ]{resources}"),
        content,
    )
}

/// Single page that invokes form `/Fm` (object 4), whose own content is `form`.
fn page_with_form_pdf(page_content: &[u8], form_dict_extra: &str, form: &[u8]) -> Vec<u8> {
    let form_object = stream_object(4, form_dict_extra, form);
    let page_content_object = stream_object(5, "", page_content);
    classic_pdf(&[
        CATALOG,
        PAGES,
        PAGE_WITH_FORM,
        &form_object,
        &page_content_object,
    ])
}

/// Run the neutral document pipeline and expand the first page's forms directly,
/// exposing the per-form skip diagnostics that the report bridges do not surface.
fn expand_first_page(source: &[u8]) -> FormExpandedInventory {
    expand_first_page_with_context(source, FormWalkContext::one_level())
}

pub(super) fn expand_first_page_with_context(
    source: &[u8],
    context: FormWalkContext,
) -> FormExpandedInventory {
    expand_first_page_with_extra_images(source, context, &[])
}

/// Same pipeline, with `extra_image_names` appended to the page's image-name
/// list — lets a test force an image/form name conflict that a single
/// `/XObject` dictionary cannot produce naturally.
pub(super) fn expand_first_page_with_extra_images(
    source: &[u8],
    context: FormWalkContext,
    extra_image_names: &[PdfName],
) -> FormExpandedInventory {
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
    let root = access.page_tree_root.object_byte_offset;
    let extents = inspect_document_page_content_extents_with_lookup(source, lookup, root)
        .expect("page content extents");
    let resources = inspect_document_page_xobject_resources_with_lookup(source, lookup, root)
        .expect("page xobject resources");
    let page = &extents.pages[0];
    let page_resources = &resources.pages[0];
    let mut image_names = inventory_names(&page_resources.image_xobject_names);
    image_names.extend_from_slice(extra_image_names);
    let form_names = inventory_names(&page_resources.form_xobject_names);
    build_page_inventory_with_forms(
        source,
        lookup,
        page,
        PageIndex(0),
        MAX,
        &image_names,
        &form_names,
        &page_resources.form_xobjects,
        &[],
        &[],
        context,
    )
    .expect("first page inventory")
}

#[test]
fn rgb_inside_page_level_form_surfaces_as_form_scope_marking_entry() {
    let source = page_with_form_pdf(
        b"q\n/Fm Do\nQ",
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ]",
        b"1 0 0 rg\n0 0 50 50 re\nf",
    );

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    // Page-level form invocation entry, then the form's own content entry.
    assert_eq!(report.inventory.len(), 2);
    let invocation = &report.inventory.entries[0];
    assert_eq!(invocation.kind, ObjectKind::FormXObject);
    assert_eq!(invocation.provenance.scope, ContentScope::Page);

    let form_marking = &report.inventory.entries[1];
    assert_eq!(form_marking.kind, ObjectKind::Vector);
    assert_eq!(
        form_marking.provenance.scope,
        ContentScope::FormXObject {
            name: PdfName(b"Fm".to_vec()),
        }
    );
    assert!(
        form_marking
            .colors
            .iter()
            .any(|color| color.space == ColorSpace::DeviceRgb)
    );
}

#[test]
fn form_entries_carry_invoking_page_index_and_page_global_sequence() {
    let source = page_with_form_pdf(
        b"q\n/Fm Do\nQ",
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ]",
        b"1 0 0 rg\n0 0 50 50 re\nf",
    );

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    let invocation = &report.inventory.entries[0];
    let form_marking = &report.inventory.entries[1];
    // Nested entry is stamped with the ORIGINAL invoking page index.
    assert_eq!(form_marking.id.page, PageIndex(0));
    assert_eq!(form_marking.provenance.page, PageIndex(0));
    // Sequence is page-global and continues after the page space; it never
    // restarts at 0.
    assert_eq!(invocation.id.sequence, 0);
    assert_eq!(form_marking.id.sequence, 1);
    assert!(form_marking.id.sequence > invocation.id.sequence);
}

#[test]
fn self_referential_form_is_a_skip_not_a_page_failure() {
    // Object 4 is a form whose own `/Resources /XObject /Fm` points back at
    // itself and whose content re-invokes `/Fm`.
    let form_object = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /XObject << /Fm 4 0 R >> >>",
        b"1 0 0 rg\n0 0 50 50 re\nf\n/Fm Do",
    );
    let page_content = stream_object(5, "", b"q\n/Fm Do\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, PAGE_WITH_FORM, &form_object, &page_content]);

    let expanded = expand_first_page(&source);

    // The page's own invocation entry plus the form's own content survive.
    assert!(!expanded.inventory.is_empty());
    assert!(
        expanded
            .inventory
            .entries
            .iter()
            .any(|entry| entry.provenance.scope == ContentScope::Page)
    );
    // The re-invocation is reported as a cycle, not descended into forever.
    assert_eq!(expanded.form_skipped.len(), 1);
    assert_eq!(expanded.form_skipped[0].name, PdfName(b"Fm".to_vec()));
    assert_eq!(
        expanded.form_skipped[0].reason,
        SkippedFormInventoryReason::Cycle
    );
}

#[test]
fn unsupported_filter_form_is_a_skip_with_page_inventory_intact() {
    // The page paints its own CMYK vector, then invokes a form whose stream uses
    // a filter this bridge does not decode.
    let source = page_with_form_pdf(
        b"q\n0 0 0 1 k\n0 0 10 10 re\nf\n/Fm Do\nQ",
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Filter /ASCIIHexDecode",
        b"00",
    );

    let expanded = expand_first_page(&source);

    // Page's own vector inventory is still produced.
    assert!(
        expanded
            .inventory
            .entries
            .iter()
            .any(|entry| entry.kind == ObjectKind::Vector
                && entry.provenance.scope == ContentScope::Page)
    );
    assert_eq!(expanded.form_skipped.len(), 1);
    assert!(matches!(
        expanded.form_skipped[0].reason,
        SkippedFormInventoryReason::Content {
            skip: PdfInventorySkip::UnsupportedFilter { .. }
        }
    ));
}

#[test]
fn classic_bridge_expands_page_level_form_content() {
    let source = page_with_form_pdf(
        b"q\n/Fm Do\nQ",
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ]",
        b"1 0 0 rg\n0 0 50 50 re\nf",
    );

    let report = build_classic_pdf_inventory(&source, MAX).expect("classic inventory should build");

    assert!(report.inventory.entries.iter().any(|entry| {
        entry.kind == ObjectKind::Vector
            && entry.provenance.scope
                == ContentScope::FormXObject {
                    name: PdfName(b"Fm".to_vec()),
                }
            && entry
                .colors
                .iter()
                .any(|c| c.space == ColorSpace::DeviceRgb)
    }));
}

#[test]
fn page_without_form_invocations_is_unchanged() {
    let source = classic_pdf(&[
        CATALOG,
        PAGES,
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>\nendobj\n",
        &stream_object(4, "", b"q\n0 0 1 rg\n12 12 80 80 re\nf\nQ"),
    ]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    // Exactly the page-only vector entry, in `Page` scope, sequence 0: no form
    // machinery altered the page-only output.
    assert_eq!(report.inventory.len(), 1);
    let entry = &report.inventory.entries[0];
    assert_eq!(entry.kind, ObjectKind::Vector);
    assert_eq!(entry.provenance.scope, ContentScope::Page);
    assert_eq!(entry.id.sequence, 0);
    assert_eq!(entry.id.page, PageIndex(0));
}

#[test]
fn bounded_default_walks_rgb_inside_nested_form() {
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"/B Do");
    let form_b = form_xobject(5, "", b"1 0 0 rg\n0 0 50 50 re\nf");
    let page_content = stream_object(6, "", b"/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    assert!(report.pages.iter().all(|page| match &page.result {
        crate::PdfInventoryPageResult::Inventoried { form_skipped, .. } => {
            form_skipped.is_empty()
        }
        crate::PdfInventoryPageResult::Skipped { .. } => false,
    }));
    assert!(report.inventory.entries.iter().any(|entry| {
        entry.provenance.scope
            == ContentScope::FormXObject {
                name: PdfName(b"B".to_vec()),
            }
            && entry
                .colors
                .iter()
                .any(|color| color.space == ColorSpace::DeviceRgb)
    }));
}

#[test]
fn nested_form_entries_carry_invoking_page_index() {
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"/B Do");
    let form_b = form_xobject(5, "", b"0 0 0 1 k\n0 0 50 50 re\nf");
    let page_content = stream_object(6, "", b"/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");
    let nested = report
        .inventory
        .entries
        .iter()
        .find(|entry| {
            entry.provenance.scope
                == ContentScope::FormXObject {
                    name: PdfName(b"B".to_vec()),
                }
        })
        .expect("nested form entry");

    assert_eq!(nested.id.page, PageIndex(0));
    assert_eq!(nested.provenance.page, PageIndex(0));
}

#[test]
fn shared_form_invoked_twice_entries_carry_distinct_invocation_ordinals() {
    let page = page_with_xobjects_object("/A 4 0 R", 5);
    let form_a = form_xobject(4, "", b"0 0 0 1 k\n0 0 10 10 re\nf");
    let page_content = stream_object(5, "", b"/A Do\n/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    assert_eq!(expanded.inventory.entries.len(), 4);
    assert_eq!(expanded.inventory.entries[0].provenance.invocation, None);
    assert_eq!(
        expanded.inventory.entries[1].provenance.invocation,
        Some(invocation_path(&[(0, b"A")]))
    );
    assert_eq!(expanded.inventory.entries[2].provenance.invocation, None);
    assert_eq!(
        expanded.inventory.entries[3].provenance.invocation,
        Some(invocation_path(&[(1, b"A")]))
    );
    assert_eq!(
        expanded.inventory.entries[1].provenance.scope,
        expanded.inventory.entries[3].provenance.scope
    );
}

#[test]
fn form_expanded_identity_is_deterministic_and_unique_across_invocations() {
    let page = page_with_xobjects_object("/A 4 0 R", 5);
    let form_a = form_xobject(4, "", b"0 0 0 1 k\n0 0 10 10 re\nf");
    let page_content = stream_object(5, "", b"/A Do\n/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &page_content]);

    let first = expand_first_page_with_context(&source, FormWalkContext::bounded_default());
    let second = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    // Determinism: the same input walked twice yields byte-identical identities,
    // digests included.
    assert_eq!(first.inventory.entries, second.inventory.entries);
    // Uniqueness: the two invocations of the shared form now carry distinct
    // digests (each folds its own invocation path and final sequence).
    assert_ne!(
        first.inventory.entries[1].id.digest,
        first.inventory.entries[3].id.digest
    );
}

#[test]
fn nested_form_entries_carry_outer_and_inner_invocation_chains() {
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"0 0 0 1 k\n0 0 10 10 re\nf\n/B Do");
    let form_b = form_xobject(5, "", b"1 0 0 rg\n0 0 50 50 re\nf");
    let page_content = stream_object(6, "", b"/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    assert_eq!(expanded.inventory.entries.len(), 4);
    assert_eq!(expanded.inventory.entries[0].provenance.invocation, None);
    assert_eq!(
        expanded.inventory.entries[1].provenance.invocation,
        Some(invocation_path(&[(0, b"A")]))
    );
    assert_eq!(
        expanded.inventory.entries[2].provenance.invocation,
        Some(invocation_path(&[(0, b"A")]))
    );
    assert_eq!(
        expanded.inventory.entries[3].provenance.invocation,
        Some(invocation_path(&[(0, b"A"), (0, b"B")]))
    );
}

#[test]
fn form_cycle_a_b_a_terminates_with_cycle_skip() {
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"0 0 0 1 k\n0 0 10 10 re\nf\n/B Do");
    let form_b = form_xobject(5, "/A 4 0 R", b"/A Do");
    let page_content = stream_object(6, "", b"/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    assert!(!expanded.inventory.is_empty());
    assert_eq!(expanded.form_skipped.len(), 1);
    assert_eq!(
        expanded.form_skipped[0].reason,
        SkippedFormInventoryReason::Cycle
    );
    assert_eq!(expanded.form_skipped[0].name, PdfName(b"A".to_vec()));
}

fn invocation_path(frames: &[(u32, &[u8])]) -> InvocationPath {
    InvocationPath {
        frames: frames
            .iter()
            .map(|(ordinal, name)| InvocationFrame {
                ordinal: *ordinal,
                name: PdfName((*name).to_vec()),
            })
            .collect(),
    }
}

#[test]
fn form_beyond_max_depth_is_reported_as_max_depth_skip() {
    let page = page_with_xobjects_object("/A 4 0 R", 7);
    let form_a = form_xobject(4, "/B 5 0 R", b"/B Do");
    let form_b = form_xobject(5, "/C 6 0 R", b"/C Do");
    let form_c = form_xobject(6, "", b"0 0 0 1 k\n0 0 50 50 re\nf");
    let page_content = stream_object(7, "", b"/A Do");
    let source = classic_pdf(&[
        CATALOG,
        PAGES,
        &page,
        &form_a,
        &form_b,
        &form_c,
        &page_content,
    ]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::new(2));

    assert_eq!(expanded.form_skipped.len(), 1);
    assert_eq!(expanded.form_skipped[0].name, PdfName(b"C".to_vec()));
    assert_eq!(
        expanded.form_skipped[0].reason,
        SkippedFormInventoryReason::MaxDepth { max_depth: 2 }
    );
}

#[test]
fn shared_form_reached_by_two_non_cyclic_branches_is_walked_twice() {
    let page = page_with_xobjects_object("/A 4 0 R /B 5 0 R", 7);
    let form_a = form_xobject(4, "/C 6 0 R", b"/C Do");
    let form_b = form_xobject(5, "/C 6 0 R", b"/C Do");
    let form_c = form_xobject(6, "", b"0 0 0 1 k\n0 0 50 50 re\nf");
    let page_content = stream_object(7, "", b"/A Do\n/B Do");
    let source = classic_pdf(&[
        CATALOG,
        PAGES,
        &page,
        &form_a,
        &form_b,
        &form_c,
        &page_content,
    ]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    assert!(expanded.form_skipped.is_empty());
    let shared_markings = expanded
        .inventory
        .entries
        .iter()
        .filter(|entry| {
            entry.provenance.scope
                == ContentScope::FormXObject {
                    name: PdfName(b"C".to_vec()),
                }
                && entry.kind == ObjectKind::Vector
        })
        .count();
    assert_eq!(shared_markings, 2);
}

#[test]
fn repeated_non_cyclic_form_invocations_stop_at_total_budget() {
    let page = page_with_xobjects_object("/A 4 0 R /B 5 0 R", 7);
    let form_a = form_xobject(4, "/C 6 0 R", b"/C Do");
    let form_b = form_xobject(5, "/C 6 0 R", b"/C Do");
    let form_c = form_xobject(6, "", b"0 0 0 1 k\n0 0 50 50 re\nf");
    let page_content = stream_object(7, "", b"/A Do\n/B Do");
    let source = classic_pdf(&[
        CATALOG,
        PAGES,
        &page,
        &form_a,
        &form_b,
        &form_c,
        &page_content,
    ]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::with_budget(8, 3));

    let walked_shared_markings = expanded
        .inventory
        .entries
        .iter()
        .filter(|entry| {
            entry.provenance.scope
                == ContentScope::FormXObject {
                    name: PdfName(b"C".to_vec()),
                }
                && entry.kind == ObjectKind::Vector
        })
        .count();
    assert_eq!(walked_shared_markings, 1);
    assert_eq!(expanded.form_skipped.len(), 1);
    assert_eq!(expanded.form_skipped[0].name, PdfName(b"C".to_vec()));
    assert_eq!(
        expanded.form_skipped[0].reason,
        SkippedFormInventoryReason::BudgetExhausted { max_expansions: 3 }
    );
}

/// A `/N 4` ICC-profile stream object (object 6) for `[ /ICCBased 6 0 R ]`.
const ICC_N4: &[u8] = b"6 0 obj\n<< /N 4 /Length 1 >>\nstream\nx\nendstream\nendobj\n";
/// A shallow tint-transform function stream (object 7) for `/Separation`.
const TINT_FN: &[u8] =
    b"7 0 obj\n<< /FunctionType 2 /Domain [ 0 1 ] /N 1 /Length 0 >>\nstream\n\nendstream\nendobj\n";

#[test]
fn form_scope_resource_color_spaces_resolve_to_icc_and_separation() {
    // The page does nothing but invoke `/Fm`; the form declares its OWN
    // `/ColorSpace` and paints an ICC fill then a Separation fill.
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /ColorSpace << /CS0 [ /ICCBased 6 0 R ] /CS1 [ /Separation /PANTONE /DeviceCMYK 7 0 R ] >> >>",
        b"/CS0 cs 0.1 0.2 0.3 0.4 scn 0 0 50 50 re f\n/CS1 cs 0.5 scn 0 0 50 50 re f",
    );
    let page_content = stream_object(5, "", b"q\n/Fm Do\nQ");
    let source = classic_pdf(&[
        CATALOG,
        PAGES,
        PAGE_WITH_FORM,
        &form,
        &page_content,
        ICC_N4,
        TINT_FN,
    ]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    let form_scope = ContentScope::FormXObject {
        name: PdfName(b"Fm".to_vec()),
    };
    // The form's `cs`/`scn` resolve against the FORM's own resources: the real
    // `IccBased` and `Separation` families, not unresolved `Resource(/CS…)`.
    let icc = report
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.provenance.scope == form_scope)
        .flat_map(|entry| entry.colors.iter())
        .find(|color| color.space == ColorSpace::IccBased)
        .expect("form ICC fill resolves to IccBased");
    assert_eq!(icc.components, vec![0.1, 0.2, 0.3, 0.4]);

    let separation = report
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.provenance.scope == form_scope)
        .flat_map(|entry| entry.colors.iter())
        .find(|color| color.space == ColorSpace::Separation)
        .expect("form Separation fill resolves to Separation");
    assert_eq!(separation.spot_name, Some(PdfName(b"PANTONE".to_vec())));
    // No colour observation stayed unresolved as a `Resource(name)`.
    assert!(
        report
            .inventory
            .entries
            .iter()
            .filter(|entry| entry.provenance.scope == form_scope)
            .flat_map(|entry| entry.colors.iter())
            .all(|color| !matches!(color.space, ColorSpace::Resource(_)))
    );
}

#[test]
fn form_without_color_space_does_not_inherit_page_color_space() {
    // The PAGE declares `/CS0`, but the form omits `/ColorSpace` and uses
    // `/CS0 cs`. Forms do not inherit page colour spaces (ISO 32000-1 §7.8.3 +
    // §8.10.2 Table 95), so the form's `cs CS0` stays `Resource(CS0)`.
    let page: &[u8] = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm 4 0 R >> /ColorSpace << /CS0 [ /ICCBased 6 0 R ] >> >> /Contents 5 0 R >>\nendobj\n";
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ]",
        b"/CS0 cs 0.5 scn 0 0 50 50 re f",
    );
    let page_content = stream_object(5, "", b"q\n/Fm Do\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, page, &form, &page_content, ICC_N4]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    let form_scope = ContentScope::FormXObject {
        name: PdfName(b"Fm".to_vec()),
    };
    let color = report
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.provenance.scope == form_scope)
        .flat_map(|entry| entry.colors.iter())
        .next()
        .expect("form paints one colour");
    // Honest KEEP: the form's env is empty, so `/CS0` is unresolved, NOT the
    // page's `IccBased`.
    assert_eq!(color.space, ColorSpace::Resource(PdfName(b"CS0".to_vec())));
    assert_ne!(color.space, ColorSpace::IccBased);
}

#[test]
fn nested_form_resolves_its_own_color_space_independently() {
    // Page invokes form `/A` (no colour), which invokes nested form `/B` whose
    // OWN `/ColorSpace` declares `/CS0`. The nested form resolves it in the same
    // recursive `expand` step, without seeing `/A`'s (absent) spaces.
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"/B Do");
    let form_b = stream_object(
        5,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /ColorSpace << /CS0 [ /ICCBased 7 0 R ] >> >>",
        b"/CS0 cs 0.2 0.4 0.6 scn 0 0 50 50 re f",
    );
    let page_content = stream_object(6, "", b"/A Do");
    let icc_n3 = b"7 0 obj\n<< /N 3 /Length 1 >>\nstream\nx\nendstream\nendobj\n";
    let source = classic_pdf(&[
        CATALOG,
        PAGES,
        &page,
        &form_a,
        &form_b,
        &page_content,
        icc_n3,
    ]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    let nested_scope = ContentScope::FormXObject {
        name: PdfName(b"B".to_vec()),
    };
    let color = report
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.provenance.scope == nested_scope)
        .flat_map(|entry| entry.colors.iter())
        .find(|color| color.space == ColorSpace::IccBased)
        .expect("nested form ICC fill resolves to IccBased");
    assert_eq!(color.components, vec![0.2, 0.4, 0.6]);
}

#[test]
fn page_and_form_indexed_resources_resolve_to_indexed_index_operands() {
    // The PAGE declares its own Indexed space and paints `7 scn`; the form
    // declares a DIFFERENT Indexed space (CMYK base) and paints `3 scn`. Both
    // resolve scope-locally to `ColorSpace::Indexed`, and every observation
    // keeps the raw INDEX operand — no palette expansion into base components.
    let page: &[u8] = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm 4 0 R >> /ColorSpace << /P0 [ /Indexed /DeviceRGB 255 <000102> ] >> >> /Contents 5 0 R >>\nendobj\n";
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /ColorSpace << /F0 [ /I /DeviceCMYK 15 <00010203> ] >> >>",
        b"/F0 cs 3 scn 0 0 50 50 re f",
    );
    let page_content = stream_object(5, "", b"/P0 cs 7 scn 0 0 50 50 re f\n/Fm Do");
    let source = classic_pdf(&[CATALOG, PAGES, page, &form, &page_content]);

    let report = build_pdf_inventory(&source, MAX).expect("inventory should build");

    let page_color = report
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.provenance.scope == ContentScope::Page)
        .flat_map(|entry| entry.colors.iter())
        .find(|color| color.space == ColorSpace::Indexed)
        .expect("page Indexed fill resolves to Indexed");
    assert_eq!(page_color.components, vec![7.0]);
    assert_eq!(page_color.spot_name, None);

    let form_scope = ContentScope::FormXObject {
        name: PdfName(b"Fm".to_vec()),
    };
    let form_color = report
        .inventory
        .entries
        .iter()
        .filter(|entry| entry.provenance.scope == form_scope)
        .flat_map(|entry| entry.colors.iter())
        .find(|color| color.space == ColorSpace::Indexed)
        .expect("form Indexed fill resolves to Indexed");
    assert_eq!(form_color.components, vec![3.0]);
    // No colour observation stayed unresolved as a `Resource(name)`.
    assert!(
        report
            .inventory
            .entries
            .iter()
            .flat_map(|entry| entry.colors.iter())
            .all(|color| !matches!(color.space, ColorSpace::Resource(_)))
    );
}

#[test]
fn nested_resource_classification_skips_surface_as_form_skips() {
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/Bad 99 0 R /B 5 0 R", b"/B Do");
    let form_b = form_xobject(5, "", b"0 0 0 1 k\n0 0 50 50 re\nf");
    let page_content = stream_object(6, "", b"/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    assert!(expanded.inventory.entries.iter().any(|entry| {
        entry.provenance.scope
            == ContentScope::FormXObject {
                name: PdfName(b"B".to_vec()),
            }
    }));
    assert_eq!(expanded.form_skipped.len(), 1);
    assert_eq!(expanded.form_skipped[0].name, PdfName(b"A".to_vec()));
    assert!(matches!(
        expanded.form_skipped[0].reason,
        SkippedFormInventoryReason::Resource { .. }
    ));
}

fn backend_lookup(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
    match backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    }
}

/// Build the FORM-local `ExtGState` env for the first page's first form, through
/// the real pdf-side form inspector and the umbrella mapping — the same path the
/// machine's `prepare_form_object` uses. No page resources are inherited.
fn first_form_extgstate_env(source: &[u8]) -> Vec<ExtGStateResource> {
    let access = inspect_document_access(source).expect("document access");
    let lookup = backend_lookup(&access.backend);
    let root = access.page_tree_root.object_byte_offset;
    let resources = inspect_document_page_xobject_resources_with_lookup(source, lookup, root)
        .expect("page xobject resources");
    let form_target = &resources.pages[0].form_xobjects[0];
    let report = inspect_form_extgstate_resources(source, lookup, form_target.object_byte_offset);
    extgstate_env_resources(&report.extgstates, &report.skipped)
}

/// Directly walk `content` against a borrowed form-local `ExtGState` env.
fn walk_form(content: &[u8], env: &[ExtGStateResource]) -> Vec<PaintOp> {
    let tokens = tokenize(content).expect("tokenize");
    let assembled = assemble_operators(&tokens).expect("assemble");
    let mut walker = GraphicsStateWalker::with_envs(ColorSpaceEnv::empty(), ExtGStateEnv::new(env));
    assembled
        .records
        .iter()
        .enumerate()
        .map(|(index, record)| walker.step(content, index, record).expect("walk step"))
        .collect()
}

#[test]
fn form_scope_gs_applies_form_local_extgstate() {
    // The form declares its OWN `/ExtGState /GS0` with `op true` and its content
    // applies it. The form-local env must classify `/GS0` and the walk must carry
    // it onto the snapshot.
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /ExtGState << /GS0 << /op true >> >> >>",
        b"/GS0 gs\n0 0 1 rg\n0 0 50 50 re\nf",
    );
    let page_content = stream_object(5, "", b"q\n/Fm Do\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, PAGE_WITH_FORM, &form, &page_content]);

    let env = first_form_extgstate_env(&source);
    assert_eq!(
        env.iter()
            .find(|resource| resource.name.0 == b"GS0")
            .expect("form-local GS0")
            .params
            .overprint_fill,
        GsParam::Set(true)
    );

    let ops = walk_form(b"/GS0 gs", &env);
    assert_eq!(ops[0].state.extgstate.overprint_fill, GsParam::Set(true));
}

#[test]
fn form_does_not_inherit_page_extgstate() {
    // The PAGE declares `/ExtGState /GS0`, but the form omits `/ExtGState` and
    // still uses `/GS0 gs`. Forms paint against their OWN resources only (same
    // rule as colour spaces), so the form-local env is empty and the form's
    // `/GS0 gs` is an honest legacy no-op, not the page's classification.
    let page: &[u8] = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Resources << /XObject << /Fm 4 0 R >> /ExtGState << /GS0 << /op true >> >> >> /Contents 5 0 R >>\nendobj\n";
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ]",
        b"/GS0 gs\n0 0 1 rg\n0 0 50 50 re\nf",
    );
    let page_content = stream_object(5, "", b"q\n/Fm Do\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, page, &form, &page_content]);

    let env = first_form_extgstate_env(&source);
    assert!(env.is_empty());

    let ops = walk_form(b"/GS0 gs", &env);
    assert_eq!(ops[0].state.extgstate.overprint_fill, GsParam::Default);
}
