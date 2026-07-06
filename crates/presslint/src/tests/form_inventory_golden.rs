//! Golden value locks for the form-expanded umbrella inventory path.
//!
//! ORACLE RATIONALE. The single-stream bit-identity lock in
//! `presslint-inventory` pins one content stream walked in one scope; it does
//! NOT guard the umbrella's recursive form-expansion path
//! (`build_page_inventory_with_forms` / `FormExpansion::expand`):
//! invocation-to-entry pairing, depth-first nested emission order, sequence
//! rebasing onto the page-global space, the collapsed innermost-name form
//! scope, and the structured skip records. Upcoming internal refactors will
//! migrate this manual recursion onto a paint-side call/return machine and MUST
//! keep this output byte-identical. These inline value locks pin, per fixture,
//! the full ordered entry identity (`id.sequence` AND `id.digest`), the kinds,
//! the scopes, and the ordered skip list, so any movement fails loudly even
//! when higher-level count/scope tests still pass.
//!
//! Under identity v3 the shared-form fixture documents invocation-aware
//! identity: each form-expanded entry is built born-final, folding its FINAL
//! page-global sequence and its ordered invocation path into the digest. Two
//! invocations of the same shared form therefore yield entries with DISTINCT
//! digests (distinct paths and sequences), asserted explicitly below — the
//! deliberate contract break these goldens were built to make visible.

use crate::{
    ContentScope, FormExpandedInventory, FormWalkContext, ObjectKind, PdfName,
    SkippedFormInventoryReason,
};

use super::form_inventory::{
    CATALOG, PAGE_WITH_FORM, PAGES, classic_pdf, expand_first_page_with_context,
    expand_first_page_with_extra_images, form_xobject, page_with_xobjects_object, stream_object,
};

#[test]
fn shared_form_invoked_twice_identity_is_golden_locked() {
    // One form resource `/A`, invoked TWICE by the page (`/A Do\n/A Do`).
    let page = page_with_xobjects_object("/A 4 0 R", 5);
    let form_a = form_xobject(4, "", b"0 0 0 1 k\n0 0 10 10 re\nf");
    let page_content = stream_object(5, "", b"/A Do\n/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    // Emission order interleaves each expansion after its invocation entry,
    // while rebased sequences continue after the PAGE sequence space (0, 1):
    // the first expansion gets 2, the second gets 3.
    assert_form_golden(
        "shared form invoked twice",
        &expanded,
        &[
            (0, ObjectKind::FormXObject, page_scope()),
            (2, ObjectKind::Vector, form_scope(b"A")),
            (1, ObjectKind::FormXObject, page_scope()),
            (3, ObjectKind::Vector, form_scope(b"A")),
        ],
        SHARED_TWICE_DIGESTS,
        &[],
    );
    // Identity v3: the two expansions of the same form now carry DISTINCT digests
    // (each folds its own invocation path `[(0,/A)]` vs `[(1,/A)]` and its own
    // final page-global sequence), AND distinct sequences. This is the deliberate
    // contract break the T147 golden was built to make visible.
    assert_ne!(
        expanded.inventory.entries[1].id.digest, expanded.inventory.entries[3].id.digest,
        "distinct invocations of one shared form must now carry distinct digests"
    );
    assert_ne!(
        expanded.inventory.entries[1].id.sequence, expanded.inventory.entries[3].id.sequence,
        "double invocation must carry distinct page-global sequences"
    );
}

#[test]
fn nested_form_identity_is_golden_locked() {
    // Page invokes `/A`; `/A` paints its own vector then invokes `/B`.
    let page = page_with_xobjects_object("/A 4 0 R", 6);
    let form_a = form_xobject(4, "/B 5 0 R", b"0 0 0 1 k\n0 0 10 10 re\nf\n/B Do");
    let form_b = form_xobject(5, "", b"1 0 0 rg\n0 0 50 50 re\nf");
    let page_content = stream_object(6, "", b"/A Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form_a, &form_b, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    // Depth-first emission: `/B`'s entry sits IMMEDIATELY after its invocation
    // entry inside `/A`, all in one monotonic sequence space seeded at the page
    // inventory length (1). Scopes collapse to the INNERMOST invoking name.
    assert_form_golden(
        "nested form",
        &expanded,
        &[
            (0, ObjectKind::FormXObject, page_scope()),
            (1, ObjectKind::Vector, form_scope(b"A")),
            (2, ObjectKind::FormXObject, form_scope(b"A")),
            (3, ObjectKind::Vector, form_scope(b"B")),
        ],
        NESTED_DIGESTS,
        &[],
    );
}

#[test]
fn self_referential_cycle_identity_is_golden_locked() {
    // `/Fm` re-invokes itself: the refused descent is a `Cycle` skip and emits
    // NO entries; the form's own content (vector + the re-invocation entry)
    // still surfaces exactly once.
    let form = stream_object(
        4,
        " /Type /XObject /Subtype /Form /BBox [ 0 0 100 100 ] /Resources << /XObject << /Fm 4 0 R >> >>",
        b"1 0 0 rg\n0 0 50 50 re\nf\n/Fm Do",
    );
    let page_content = stream_object(5, "", b"q\n/Fm Do\nQ");
    let source = classic_pdf(&[CATALOG, PAGES, PAGE_WITH_FORM, &form, &page_content]);

    let expanded = expand_first_page_with_context(&source, FormWalkContext::bounded_default());

    assert_form_golden(
        "self-referential cycle",
        &expanded,
        &[
            (0, ObjectKind::FormXObject, page_scope()),
            (1, ObjectKind::Vector, form_scope(b"Fm")),
            (2, ObjectKind::FormXObject, form_scope(b"Fm")),
        ],
        CYCLE_DIGESTS,
        &[(PdfName(b"Fm".to_vec()), SkippedFormInventoryReason::Cycle)],
    );
}

#[test]
fn max_depth_identity_is_golden_locked() {
    // Chain `/A` -> `/B` -> `/C` with a depth budget of 2: `/C` is invoked (its
    // invocation entry exists inside `/B`) but the descent into its content is
    // refused as a `MaxDepth` skip, so no `/C`-scope entries are emitted.
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

    assert_form_golden(
        "max depth",
        &expanded,
        &[
            (0, ObjectKind::FormXObject, page_scope()),
            (1, ObjectKind::FormXObject, form_scope(b"A")),
            (2, ObjectKind::FormXObject, form_scope(b"B")),
        ],
        MAX_DEPTH_DIGESTS,
        &[(
            PdfName(b"C".to_vec()),
            SkippedFormInventoryReason::MaxDepth { max_depth: 2 },
        )],
    );
}

#[test]
fn budget_exhaustion_identity_is_golden_locked() {
    // Shared `/C` reachable via `/A` and `/B` with a total expansion budget of
    // 3: the walk spends A, C-under-A, B, then refuses C-under-B with a
    // `BudgetExhausted` skip, so `/C`'s content is emitted exactly once.
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

    assert_form_golden(
        "budget exhaustion",
        &expanded,
        &[
            (0, ObjectKind::FormXObject, page_scope()),
            (2, ObjectKind::FormXObject, form_scope(b"A")),
            (3, ObjectKind::Vector, form_scope(b"C")),
            (1, ObjectKind::FormXObject, page_scope()),
            (4, ObjectKind::FormXObject, form_scope(b"B")),
        ],
        BUDGET_DIGESTS,
        &[(
            PdfName(b"C".to_vec()),
            SkippedFormInventoryReason::BudgetExhausted { max_expansions: 3 },
        )],
    );
}

#[test]
fn image_form_name_conflict_identity_is_golden_locked() {
    // `/Dup` resolves to a form object, but the same name is ALSO forced into
    // the page's image-name list (the conflict shape a single `/XObject`
    // dictionary cannot produce naturally): image classification wins, the
    // invocation is an Image entry, and NO form expansion (and no skip)
    // happens.
    let page = page_with_xobjects_object("/Dup 4 0 R", 5);
    let form = form_xobject(4, "", b"0 0 0 1 k\n0 0 10 10 re\nf");
    let page_content = stream_object(5, "", b"/Dup Do");
    let source = classic_pdf(&[CATALOG, PAGES, &page, &form, &page_content]);

    let expanded = expand_first_page_with_extra_images(
        &source,
        FormWalkContext::bounded_default(),
        &[PdfName(b"Dup".to_vec())],
    );

    assert_form_golden(
        "image/form name conflict",
        &expanded,
        &[(0, ObjectKind::Image, page_scope())],
        CONFLICT_DIGESTS,
        &[],
    );
}

/// Assert the full golden identity of one expanded page: ordered
/// `(sequence, kind, scope)` triples, the pinned digest sequence, and the
/// ordered `(name, reason)` skip list.
fn assert_form_golden(
    name: &str,
    expanded: &FormExpandedInventory,
    expected_entries: &[(u32, ObjectKind, ContentScope)],
    expected_digests: &[[u8; 32]],
    expected_skips: &[(PdfName, SkippedFormInventoryReason)],
) {
    let sequences: Vec<u32> = expanded
        .inventory
        .entries
        .iter()
        .map(|entry| entry.id.sequence)
        .collect();
    let kinds: Vec<ObjectKind> = expanded
        .inventory
        .entries
        .iter()
        .map(|entry| entry.kind)
        .collect();
    let scopes: Vec<ContentScope> = expanded
        .inventory
        .entries
        .iter()
        .map(|entry| entry.provenance.scope.clone())
        .collect();
    let digests: Vec<[u8; 32]> = expanded
        .inventory
        .entries
        .iter()
        .map(|entry| entry.id.digest)
        .collect();
    let skips: Vec<(PdfName, SkippedFormInventoryReason)> = expanded
        .form_skipped
        .iter()
        .map(|skip| (skip.name.clone(), skip.reason.clone()))
        .collect();

    let expected_sequences: Vec<u32> = expected_entries.iter().map(|(seq, _, _)| *seq).collect();
    let expected_kinds: Vec<ObjectKind> =
        expected_entries.iter().map(|(_, kind, _)| *kind).collect();
    let expected_scopes: Vec<ContentScope> = expected_entries
        .iter()
        .map(|(_, _, scope)| scope.clone())
        .collect();

    assert_eq!(
        expanded.inventory.len(),
        expected_entries.len(),
        "{name}: entry count"
    );
    assert_eq!(sequences, expected_sequences, "{name}: sequences");
    assert_eq!(kinds, expected_kinds, "{name}: kinds");
    assert_eq!(scopes, expected_scopes, "{name}: scopes");
    assert_eq!(digests, expected_digests, "{name}: digests");
    assert_eq!(skips, expected_skips, "{name}: skip list");
}

const fn page_scope() -> ContentScope {
    ContentScope::Page
}

fn form_scope(name: &[u8]) -> ContentScope {
    ContentScope::FormXObject {
        name: PdfName(name.to_vec()),
    }
}

const SHARED_TWICE_DIGESTS: &[[u8; 32]] = &[
    [
        98, 200, 86, 150, 165, 0, 128, 59, 178, 235, 26, 63, 131, 193, 66, 249, 199, 132, 179, 215,
        111, 65, 228, 48, 3, 125, 109, 114, 71, 28, 180, 173,
    ],
    [
        117, 240, 44, 21, 184, 120, 157, 216, 245, 60, 52, 35, 7, 200, 127, 16, 80, 148, 211, 230,
        235, 227, 170, 24, 43, 252, 222, 153, 156, 88, 37, 189,
    ],
    [
        90, 148, 211, 24, 179, 226, 25, 22, 47, 7, 234, 22, 58, 198, 127, 121, 60, 115, 79, 55,
        243, 8, 129, 113, 239, 127, 202, 253, 250, 120, 91, 110,
    ],
    [
        195, 224, 48, 220, 45, 17, 4, 158, 10, 2, 11, 227, 56, 85, 100, 61, 122, 84, 218, 185, 174,
        19, 162, 164, 227, 132, 21, 110, 32, 10, 116, 29,
    ],
];
const NESTED_DIGESTS: &[[u8; 32]] = &[
    [
        98, 200, 86, 150, 165, 0, 128, 59, 178, 235, 26, 63, 131, 193, 66, 249, 199, 132, 179, 215,
        111, 65, 228, 48, 3, 125, 109, 114, 71, 28, 180, 173,
    ],
    [
        6, 141, 61, 64, 233, 91, 24, 158, 74, 48, 209, 39, 67, 72, 94, 154, 204, 176, 124, 109, 12,
        115, 155, 1, 66, 174, 21, 23, 133, 22, 121, 148,
    ],
    [
        191, 204, 38, 32, 201, 53, 173, 122, 234, 226, 165, 135, 41, 105, 190, 200, 187, 101, 84,
        149, 2, 182, 161, 82, 195, 102, 239, 27, 17, 191, 136, 153,
    ],
    [
        196, 114, 190, 137, 96, 41, 40, 209, 241, 204, 158, 18, 187, 50, 240, 225, 104, 26, 109,
        51, 242, 57, 7, 213, 121, 133, 181, 16, 195, 62, 179, 244,
    ],
];
const CYCLE_DIGESTS: &[[u8; 32]] = &[
    [
        21, 28, 118, 33, 72, 114, 106, 114, 199, 166, 174, 70, 193, 5, 18, 168, 97, 127, 174, 59,
        169, 203, 145, 83, 87, 208, 11, 198, 87, 196, 188, 85,
    ],
    [
        50, 58, 6, 3, 136, 14, 125, 231, 19, 80, 229, 82, 238, 233, 176, 166, 138, 166, 252, 64,
        83, 191, 90, 237, 108, 112, 19, 33, 145, 13, 45, 208,
    ],
    [
        92, 51, 67, 149, 94, 51, 69, 182, 136, 185, 124, 226, 204, 105, 120, 110, 181, 21, 237, 27,
        83, 9, 69, 46, 82, 167, 225, 45, 77, 63, 97, 212,
    ],
];
const MAX_DEPTH_DIGESTS: &[[u8; 32]] = &[
    [
        98, 200, 86, 150, 165, 0, 128, 59, 178, 235, 26, 63, 131, 193, 66, 249, 199, 132, 179, 215,
        111, 65, 228, 48, 3, 125, 109, 114, 71, 28, 180, 173,
    ],
    [
        23, 196, 193, 157, 220, 60, 159, 60, 169, 139, 14, 73, 20, 19, 136, 236, 28, 220, 141, 66,
        53, 87, 126, 39, 177, 184, 188, 244, 242, 164, 176, 24,
    ],
    [
        98, 4, 172, 176, 176, 126, 120, 250, 101, 158, 92, 221, 23, 190, 193, 172, 132, 127, 3,
        128, 151, 26, 6, 4, 240, 141, 94, 187, 133, 31, 168, 183,
    ],
];
const BUDGET_DIGESTS: &[[u8; 32]] = &[
    [
        98, 200, 86, 150, 165, 0, 128, 59, 178, 235, 26, 63, 131, 193, 66, 249, 199, 132, 179, 215,
        111, 65, 228, 48, 3, 125, 109, 114, 71, 28, 180, 173,
    ],
    [
        173, 70, 246, 122, 233, 249, 36, 179, 38, 240, 181, 5, 233, 63, 177, 183, 21, 171, 54, 217,
        156, 106, 9, 210, 161, 15, 53, 135, 145, 223, 224, 59,
    ],
    [
        90, 235, 203, 66, 143, 132, 34, 208, 243, 214, 68, 203, 104, 58, 234, 93, 29, 100, 33, 60,
        239, 202, 232, 232, 108, 49, 131, 189, 46, 225, 58, 110,
    ],
    [
        99, 174, 211, 24, 179, 223, 25, 22, 198, 120, 234, 22, 58, 195, 127, 121, 113, 116, 79, 55,
        243, 9, 129, 113, 6, 116, 202, 253, 250, 117, 91, 110,
    ],
    [
        137, 55, 41, 203, 72, 230, 124, 201, 56, 158, 64, 223, 180, 248, 92, 212, 72, 20, 230, 149,
        253, 80, 233, 138, 232, 39, 4, 219, 40, 163, 162, 123,
    ],
];
const CONFLICT_DIGESTS: &[[u8; 32]] = &[[
    53, 84, 99, 139, 162, 73, 102, 168, 244, 192, 204, 235, 182, 203, 116, 171, 56, 99, 132, 143,
    62, 204, 167, 226, 85, 185, 73, 38, 245, 125, 113, 110,
]];
