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
//! The shared-form fixture additionally documents today's flat-model behaviour
//! ON PURPOSE: expansion rebases `entry.id.sequence` onto the page-global
//! space but never recomputes `entry.id.digest`, which was computed inside the
//! form's OWN walk from its form-local sequence. Two invocations of the same
//! form therefore yield entries with IDENTICAL digests but DIFFERENT rebased
//! sequences. A future, deliberate invocation-identity change must alter that
//! golden VISIBLY, not silently.

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
    // Pinned flat-model contradiction (INTENTIONAL, do not "fix" in tests):
    // the two expansions of the same form carry IDENTICAL digests (computed in
    // the form's own walk from its LOCAL sequence, never recomputed on rebase)
    // yet DIFFERENT page-global sequences.
    assert_eq!(
        expanded.inventory.entries[1].id.digest, expanded.inventory.entries[3].id.digest,
        "double invocation must share one digest under the flat model"
    );
    assert_ne!(
        expanded.inventory.entries[1].id.sequence, expanded.inventory.entries[3].id.sequence,
        "double invocation must carry distinct rebased sequences"
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
        6, 196, 35, 247, 162, 217, 31, 52, 54, 170, 207, 80, 147, 98, 232, 194, 157, 184, 30, 169,
        121, 31, 174, 189, 195, 86, 176, 0, 92, 116, 35, 146,
    ],
    [
        4, 75, 84, 166, 242, 134, 245, 52, 229, 90, 215, 110, 249, 211, 210, 45, 246, 45, 193, 212,
        254, 10, 83, 196, 208, 42, 83, 29, 133, 115, 58, 126,
    ],
    [
        164, 201, 155, 96, 215, 54, 95, 92, 251, 127, 148, 237, 8, 38, 67, 90, 223, 31, 222, 0,
        242, 197, 33, 185, 62, 25, 36, 245, 157, 213, 100, 124,
    ],
    [
        4, 75, 84, 166, 242, 134, 245, 52, 229, 90, 215, 110, 249, 211, 210, 45, 246, 45, 193, 212,
        254, 10, 83, 196, 208, 42, 83, 29, 133, 115, 58, 126,
    ],
];
const NESTED_DIGESTS: &[[u8; 32]] = &[
    [
        6, 196, 35, 247, 162, 217, 31, 52, 54, 170, 207, 80, 147, 98, 232, 194, 157, 184, 30, 169,
        121, 31, 174, 189, 195, 86, 176, 0, 92, 116, 35, 146,
    ],
    [
        4, 75, 84, 166, 242, 134, 245, 52, 229, 90, 215, 110, 249, 211, 210, 45, 246, 45, 193, 212,
        254, 10, 83, 196, 208, 42, 83, 29, 133, 115, 58, 126,
    ],
    [
        232, 227, 175, 10, 162, 15, 165, 186, 161, 252, 218, 249, 156, 214, 200, 9, 228, 201, 197,
        231, 14, 174, 239, 6, 130, 119, 191, 153, 187, 121, 195, 201,
    ],
    [
        32, 110, 6, 102, 73, 157, 242, 60, 2, 245, 143, 43, 240, 12, 206, 70, 70, 169, 199, 227,
        221, 56, 186, 71, 244, 221, 250, 178, 192, 21, 157, 224,
    ],
];
const CYCLE_DIGESTS: &[[u8; 32]] = &[
    [
        8, 94, 111, 216, 69, 39, 94, 103, 1, 230, 161, 149, 144, 130, 195, 93, 11, 12, 9, 135, 37,
        13, 24, 36, 223, 83, 124, 32, 45, 179, 199, 243,
    ],
    [
        253, 37, 86, 62, 38, 215, 127, 109, 166, 42, 102, 52, 17, 89, 5, 205, 215, 23, 39, 123, 23,
        154, 18, 248, 96, 18, 41, 218, 3, 217, 74, 72,
    ],
    [
        173, 82, 179, 212, 12, 249, 235, 27, 188, 231, 119, 87, 226, 45, 239, 252, 114, 147, 204,
        223, 198, 124, 28, 63, 204, 40, 108, 49, 81, 166, 156, 232,
    ],
];
const MAX_DEPTH_DIGESTS: &[[u8; 32]] = &[
    [
        6, 196, 35, 247, 162, 217, 31, 52, 54, 170, 207, 80, 147, 98, 232, 194, 157, 184, 30, 169,
        121, 31, 174, 189, 195, 86, 176, 0, 92, 116, 35, 146,
    ],
    [
        104, 227, 100, 66, 14, 206, 211, 27, 172, 8, 62, 243, 254, 111, 20, 61, 204, 153, 107, 240,
        101, 72, 89, 183, 38, 5, 105, 33, 149, 120, 12, 160,
    ],
    [
        169, 168, 66, 17, 96, 89, 189, 102, 76, 82, 197, 62, 61, 20, 129, 239, 122, 179, 2, 92, 94,
        52, 238, 67, 185, 197, 190, 247, 95, 58, 233, 96,
    ],
];
const BUDGET_DIGESTS: &[[u8; 32]] = &[
    [
        6, 196, 35, 247, 162, 217, 31, 52, 54, 170, 207, 80, 147, 98, 232, 194, 157, 184, 30, 169,
        121, 31, 174, 189, 195, 86, 176, 0, 92, 116, 35, 146,
    ],
    [
        23, 224, 100, 66, 14, 207, 211, 27, 251, 25, 62, 243, 254, 112, 20, 61, 147, 136, 107, 240,
        101, 71, 89, 183, 149, 11, 105, 33, 149, 119, 12, 160,
    ],
    [
        223, 50, 239, 205, 109, 131, 117, 109, 189, 61, 129, 147, 138, 48, 176, 26, 43, 92, 25, 44,
        93, 153, 83, 49, 1, 54, 211, 4, 44, 144, 100, 206,
    ],
    [
        241, 54, 154, 96, 215, 55, 95, 92, 174, 124, 148, 237, 8, 39, 67, 90, 230, 23, 222, 0, 242,
        194, 33, 185, 203, 24, 36, 245, 157, 214, 100, 124,
    ],
    [
        169, 168, 66, 17, 96, 89, 189, 102, 76, 82, 197, 62, 61, 20, 129, 239, 122, 179, 2, 92, 94,
        52, 238, 67, 185, 197, 190, 247, 95, 58, 233, 96,
    ],
];
const CONFLICT_DIGESTS: &[[u8; 32]] = &[[
    142, 59, 207, 44, 222, 47, 54, 152, 67, 231, 26, 135, 91, 152, 66, 114, 54, 223, 58, 193, 218,
    202, 196, 108, 197, 164, 254, 80, 244, 93, 86, 59,
]];
