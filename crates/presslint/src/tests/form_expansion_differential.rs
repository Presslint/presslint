//! Differential proof for the parallel machine-driven form expansion path.

use crate::{FormExpandedInventory, FormWalkContext, PdfName};

use super::form_inventory::{
    CATALOG, PAGE_WITH_FORM, PAGES, classic_pdf, expand_first_page_with_context,
    expand_first_page_with_context_machine, expand_first_page_with_extra_images,
    expand_first_page_with_extra_images_machine, form_xobject, page_with_xobjects_object,
    stream_object,
};

fn assert_machine_matches_old(fixture: &str, source: &[u8], context: FormWalkContext) {
    let old = expand_first_page_with_context(source, context.clone());
    let machine = expand_first_page_with_context_machine(source, context);
    assert_equal(fixture, &old, &machine);
}

fn assert_machine_matches_old_with_images(
    fixture: &str,
    source: &[u8],
    context: FormWalkContext,
    extra_image_names: &[PdfName],
) {
    let old = expand_first_page_with_extra_images(source, context.clone(), extra_image_names);
    let machine = expand_first_page_with_extra_images_machine(source, context, extra_image_names);
    assert_equal(fixture, &old, &machine);
}

fn assert_equal(fixture: &str, old: &FormExpandedInventory, machine: &FormExpandedInventory) {
    assert_eq!(machine, old, "{fixture}: machine expansion diverged");
}

#[test]
fn machine_matches_old_on_shared_form_invoked_twice() {
    let page = page_with_xobjects_object("/A 4 0 R", 5);
    let form_a = form_xobject(4, "", b"0 0 0 1 k\n0 0 10 10 re\nf");
    let page_content = stream_object(5, "", b"/A Do\n/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &page_content]);

    assert_machine_matches_old(
        "shared form invoked twice",
        &source,
        FormWalkContext::bounded_default(),
    );
}

#[test]
fn machine_matches_old_on_nested_form() {
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"0 0 0 1 k\n0 0 10 10 re\nf\n/B Do");
    let form_b = form_xobject(5, "", b"1 0 0 rg\n0 0 50 50 re\nf");
    let page_content = stream_object(6, "", b"/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    assert_machine_matches_old("nested form", &source, FormWalkContext::bounded_default());
}

#[test]
fn machine_matches_old_on_self_referential_cycle() {
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /XObject << /Fm 4 0 R >> >>",
        b"1 0 0 rg\n0 0 50 50 re\nf\n/Fm Do",
    );
    let page_content = stream_object(5, "", b"q\n/Fm Do\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, PAGE_WITH_FORM, &form, &page_content]);

    assert_machine_matches_old(
        "self-referential cycle",
        &source,
        FormWalkContext::bounded_default(),
    );
}

#[test]
fn machine_matches_old_on_max_depth() {
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

    assert_machine_matches_old("max depth", &source, FormWalkContext::new(2));
}

#[test]
fn machine_matches_old_on_budget_exhaustion() {
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

    assert_machine_matches_old(
        "budget exhaustion",
        &source,
        FormWalkContext::with_budget(8, 3),
    );
}

#[test]
fn machine_matches_old_on_image_form_name_conflict() {
    let page = page_with_xobjects_object("/Dup 4 0 R", 5);
    let form = form_xobject(4, "", b"0 0 0 1 k\n0 0 10 10 re\nf");
    let page_content = stream_object(5, "", b"/Dup Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form, &page_content]);
    let extra_images = [PdfName(b"Dup".to_vec())];

    assert_machine_matches_old_with_images(
        "image/form name conflict",
        &source,
        FormWalkContext::bounded_default(),
        &extra_images,
    );
}
