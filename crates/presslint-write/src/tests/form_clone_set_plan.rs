//! Behaviour of the private reached-Form closure clone-set plan.
//!
//! Walk-level tests drive `walk_reached_form_closure` directly through a
//! classic xref table (or a hand-built xref-stream section for compressed
//! members), so frontier and budget behaviour is asserted without seeding.
//! Plan-level tests run the full `FormCloneSetPlan::build` over real document
//! access with a REAL document consumer index (or the synthetic
//! complete-but-empty index where a fixture needs dangling references without
//! poisoning index completeness). Serde shape locks for the public counts
//! projection live in the CLI report tests, which own a JSON serializer.

use crate::{
    ConvertContentColorsRequest, PageSelection,
    content_edit_pipeline::lookup_from_backend,
    convert_content_colors_incremental,
    form_clone_set_plan::{
        AncillaryKeyClass, CloneMemberLocator, CloneSetOutcome, CloneSetRefusal, FormCloneSet,
        FormCloneSetPlan, FormCloneSetPlanCounts, GraphEscapeClass, ReservationRefusal,
        StructuralFrontier,
        walk::{
            ClosureWalkOutcome, MAX_CLONE_SET_DECODE_WORK_BYTES, MAX_CLONE_SET_MEMBERS,
            MAX_CLONE_SET_REFERENCE_DEPTH, MAX_CLONE_SET_REFERENCE_OCCURRENCES,
            walk_reached_form_closure,
        },
    },
};
use presslint_pdf::{
    IndirectRef, MAX_OBJECT_BODY_REFERENCES, ObjectConsumerIndexInspection, ObjectLookup,
    ObjectStreamCacheReport, XrefStreamEntry, XrefStreamEntryRecord, XrefStreamSection,
    inspect_classic_xref_table, inspect_document_access, inspect_object_consumer_index,
};

use super::content_color_convert::{GRAY_TO_GRAY_LINK, one_link};

const fn reference(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

/// `count` generation-zero reference tokens starting at `first`, one string.
fn reference_list(first: usize, count: usize) -> String {
    use std::fmt::Write;
    (first..first + count).fold(String::new(), |mut refs, number| {
        let _ = write!(refs, "{number} 0 R ");
        refs
    })
}

/// One classic-xref object slot: an in-use body (without the `N 0 obj`
/// wrapper) or an explicit free entry.
enum Slot {
    InUse(Vec<u8>),
    Free,
}

fn in_use(body: &str) -> Slot {
    Slot::InUse(body.as_bytes().to_vec())
}

/// Assemble a classic-xref document from object slots (object numbers start
/// at 1). Free slots become free xref entries with no body bytes.
fn assemble(slots: &[Slot]) -> Vec<u8> {
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut entries = Vec::with_capacity(slots.len());
    for (index, slot) in slots.iter().enumerate() {
        match slot {
            Slot::InUse(body) => {
                entries.push(Some(buf.len()));
                buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
                buf.extend_from_slice(body);
                buf.extend_from_slice(b"\nendobj\n");
            }
            Slot::Free => entries.push(None),
        }
    }
    let xref_offset = buf.len();
    let size = slots.len() + 1;
    buf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for entry in &entries {
        match entry {
            Some(offset) => buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()),
            None => buf.extend_from_slice(b"0000000000 00001 f \n"),
        }
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF")
            .as_bytes(),
    );
    buf
}

/// The standard catalog + page-tree prefix: object 1 catalog, object 2 pages
/// root, remaining slots supplied by the caller starting at object 3.
fn document(kids: &str, count: usize, rest: &[Slot]) -> Vec<u8> {
    let mut slots = vec![
        in_use("<< /Type /Catalog /Pages 2 0 R >>"),
        in_use(&format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>")),
    ];
    slots.extend(rest.iter().map(|slot| match slot {
        Slot::InUse(body) => Slot::InUse(body.clone()),
        Slot::Free => Slot::Free,
    }));
    assemble(&slots)
}

/// A leaf page with a direct `/Resources /XObject` subdictionary.
fn page(xobject_entries: &str) -> Slot {
    in_use(&format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << {xobject_entries} >> >> >>"
    ))
}

/// A Form `XObject` stream with extra dictionary keys.
fn form(extra: &str) -> Slot {
    in_use(&format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1]{extra} /Length 0 >>\nstream\n\nendstream"
    ))
}

/// Walk one root through the document's final classic xref table.
fn walk(source: &[u8], root_object_number: u32) -> ClosureWalkOutcome {
    let xref_offset = final_startxref(source);
    let xref = inspect_classic_xref_table(source, xref_offset).expect("xref inspects");
    let consumers = empty_consumers(source.len());
    let frontier = StructuralFrontier::new(
        reference(1, 0),
        reference(2, 0),
        [reference(3, 0)],
        &consumers,
    );
    walk_reached_form_closure(
        source,
        ObjectLookup::ClassicXref(&xref),
        &frontier,
        reference(root_object_number, 0),
    )
}

fn final_startxref(source: &[u8]) -> usize {
    presslint_pdf::inspect_pdf_source(source)
        .expect("source inspects")
        .startxref
        .expect("startxref present")
        .byte_offset
}

/// Build the plan over real document access and the REAL consumer index.
fn build_plan(source: &[u8]) -> FormCloneSetPlan {
    let access = inspect_document_access(source).expect("document access inspects");
    let selected: Vec<usize> = (0..access.page_leaves.leaves.len()).collect();
    let consumers = inspect_object_consumer_index(source, &access);
    build_plan_selected_with_consumers(source, &access, &selected, &consumers)
}

/// Build the plan for an explicit sorted, deduplicated page selection.
fn build_plan_selected(source: &[u8], selected: &[usize]) -> FormCloneSetPlan {
    let access = inspect_document_access(source).expect("document access inspects");
    let consumers = inspect_object_consumer_index(source, &access);
    build_plan_selected_with_consumers(source, &access, selected, &consumers)
}

fn build_plan_selected_with_consumers(
    source: &[u8],
    access: &presslint_pdf::DocumentAccess,
    selected: &[usize],
    consumers: &ObjectConsumerIndexInspection,
) -> FormCloneSetPlan {
    let lookup = lookup_from_backend(&access.backend);
    FormCloneSetPlan::build(
        source,
        lookup,
        access.root_reference,
        access.catalog_pages.pages_reference,
        access.page_tree_root.object_byte_offset,
        selected,
        consumers,
    )
}

/// Build the plan with a synthetic COMPLETE-but-empty consumer index, so
/// fixtures may carry dangling references without poisoning completeness
/// (every leaf-direct Form witness then qualifies with referrer count 0).
fn build_plan_with_empty_consumers(source: &[u8]) -> FormCloneSetPlan {
    let access = inspect_document_access(source).expect("document access inspects");
    let lookup = lookup_from_backend(&access.backend);
    let consumers = empty_consumers(source.len());
    let selected: Vec<usize> = (0..access.page_leaves.leaves.len()).collect();
    FormCloneSetPlan::build(
        source,
        lookup,
        access.root_reference,
        access.catalog_pages.pages_reference,
        access.page_tree_root.object_byte_offset,
        &selected,
        &consumers,
    )
}

const fn empty_consumers(byte_len: usize) -> ObjectConsumerIndexInspection {
    ObjectConsumerIndexInspection {
        byte_len,
        entries: Vec::new(),
        unresolved_edges: Vec::new(),
        skipped: Vec::new(),
        truncations: Vec::new(),
        expanded_node_count: 0,
        recorded_pair_count: 0,
        object_stream_cache: ObjectStreamCacheReport {
            budget_bytes: 0,
            cached_container_count: 0,
            cached_byte_count: 0,
            dropped_over_budget: false,
        },
        unreferenced: Vec::new(),
    }
}

fn member_sources(set: &FormCloneSet) -> Vec<u32> {
    match &set.outcome {
        CloneSetOutcome::Planned { members, .. } => members
            .iter()
            .map(|member| member.source.object_number)
            .collect(),
        CloneSetOutcome::Refused { .. } => panic!("expected a planned set: {set:?}"),
    }
}

fn set_refusal(set: &FormCloneSet) -> &CloneSetRefusal {
    match &set.outcome {
        CloneSetOutcome::Refused { refusal } => refusal,
        CloneSetOutcome::Planned { .. } => panic!("expected a refused set"),
    }
}

fn walk_refusal(outcome: &ClosureWalkOutcome) -> &CloneSetRefusal {
    outcome
        .result
        .as_ref()
        .expect_err("expected a walk refusal")
}

// ---------------------------------------------------------------------------
// Closure walk: membership, ordering, sharing, cycles, null equivalents.
// ---------------------------------------------------------------------------

#[test]
fn closure_reaches_nested_members_once_in_reference_order() {
    // Form 4 -> nested form 6 (deliberately out of order) and font 5; the
    // nested form shares font 5 and points BACK at form 4 (a cycle).
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            form(" /Resources << /XObject << /X 6 0 R >> /Font << /A 5 0 R /B 5 0 R >> >>"),
            in_use("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
            form(" /Resources << /XObject << /Back 4 0 R >> /Font << /A 5 0 R >> >>"),
        ],
    );
    let outcome = walk(&source, 4);
    let closure = outcome.result.expect("closure completes");

    let sources: Vec<u32> = closure
        .members
        .iter()
        .map(|member| member.source.object_number)
        .collect();
    assert_eq!(sources, vec![4, 5, 6], "deterministic by source reference");
    assert!(closure.null_equivalents.is_empty());
    // Duplicate-preserving outgoing list: form 4 names font 5 twice.
    let root = &closure.members[0];
    assert_eq!(
        root.outgoing
            .iter()
            .filter(|target| target.object_number == 5)
            .count(),
        2,
    );
    assert!(matches!(
        root.locator,
        CloneMemberLocator::Uncompressed { .. }
    ));
    assert_eq!(outcome.budget.max_depth_reached, 1);
    assert_eq!(outcome.budget.unique_members, 3);
}

#[test]
fn self_referential_indirect_length_is_a_terminated_cycle_not_a_cut() {
    // pypdf #3112 class: `/Length 4 0 R` points at the stream object itself.
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 4 0 R >>\nstream\n\nendstream",
            ),
        ],
    );
    let closure = walk(&source, 4).result.expect("closure completes");
    assert_eq!(closure.members.len(), 1);
    assert_eq!(closure.members[0].outgoing, vec![reference(4, 0)]);
}

#[test]
fn indirect_length_is_an_ordinary_closure_member() {
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 5 0 R >>\nstream\n\nendstream",
            ),
            in_use("0"),
        ],
    );
    let closure = walk(&source, 4).result.expect("closure completes");
    let sources: Vec<u32> = closure
        .members
        .iter()
        .map(|member| member.source.object_number)
        .collect();
    assert_eq!(sources, vec![4, 5], "the /Length integer is a member");
}

#[test]
fn free_and_not_found_targets_are_terminal_null_equivalents() {
    // Object 5 is an explicit free entry; object 40 is absent from the table.
    let source = document(
        "3 0 R",
        1,
        &[page("/F1 4 0 R"), form(" /A 5 0 R /B 40 0 R"), Slot::Free],
    );
    let closure = walk(&source, 4).result.expect("closure completes");

    assert_eq!(closure.members.len(), 1, "only the root is allocated");
    assert_eq!(closure.null_equivalents.len(), 2);
    let free = &closure.null_equivalents[0];
    assert_eq!(free.reference, reference(5, 0));
    assert!(free.free_entry);
    let absent = &closure.null_equivalents[1];
    assert_eq!(absent.reference, reference(40, 0));
    assert!(!absent.free_entry);
}

#[test]
fn generation_mismatch_refuses_instead_of_nulling_out() {
    let source = document(
        "3 0 R",
        1,
        &[page("/F1 4 0 R"), form(" /A 5 1 R"), in_use("<< >>")],
    );
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::ResolutionFailed { member } if *member == reference(5, 1)
    ));
}

#[test]
fn incomplete_body_scan_refuses_the_whole_set() {
    // An out-of-range reference shape (object number over u32) is a skipped
    // reference the scanner cannot prove complete.
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            form(" /A 5 0 R"),
            in_use("<< /X 99999999999 0 R >>"),
        ],
    );
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::BodyScanIncomplete { member } if *member == reference(5, 0)
    ));
}

#[test]
fn per_body_scanner_truncation_refuses_before_occurrence_accumulation() {
    let refs = reference_list(100, MAX_OBJECT_BODY_REFERENCES + 1);
    let source = document(
        "3 0 R",
        1,
        &[page("/F1 4 0 R"), form(&format!(" /Deps [{refs}]"))],
    );
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::BodyScanIncomplete { member } if *member == reference(4, 0)
    ));
    assert_eq!(outcome.budget.reference_occurrences, 0);
}

// ---------------------------------------------------------------------------
// Frontier preflight: graph escapes, ancillary keys, decoded-name evasion.
// ---------------------------------------------------------------------------

fn frontier_document(member_body: &str) -> Vec<u8> {
    document(
        "3 0 R",
        1,
        &[page("/F1 4 0 R"), form(" /A 5 0 R"), in_use(member_body)],
    )
}

#[test]
fn type_spoofed_page_tree_and_structure_members_refuse_as_graph_escapes() {
    let cases: &[(&str, GraphEscapeClass)] = &[
        ("<< /Type /Catalog >>", GraphEscapeClass::Catalog),
        ("<< /Type /Pages /Kids [] >>", GraphEscapeClass::Pages),
        ("<< /Type /Page >>", GraphEscapeClass::Page),
        (
            "<< /Type /StructTreeRoot >>",
            GraphEscapeClass::StructTreeRoot,
        ),
        ("<< /Type /StructElem >>", GraphEscapeClass::StructElem),
        ("<< /Type /OCG >>", GraphEscapeClass::OptionalContentGroup),
        (
            "<< /Type /OCMD >>",
            GraphEscapeClass::OptionalContentMembership,
        ),
    ];
    for (body, expected) in cases {
        let source = frontier_document(body);
        let outcome = walk(&source, 4);
        assert!(
            matches!(
                walk_refusal(&outcome),
                CloneSetRefusal::GraphEscape { member, escape }
                    if *member == reference(5, 0) && escape == expected
            ),
            "body {body} must refuse as {expected:?}",
        );
    }
}

#[test]
fn known_catalog_pages_root_and_leaf_refuse_by_identity_when_type_is_spoofed() {
    for (target, expected) in [
        (1, GraphEscapeClass::Catalog),
        (2, GraphEscapeClass::Pages),
        (3, GraphEscapeClass::Page),
    ] {
        // The structural objects deliberately claim `/Type /Font`; the
        // exact document identities, not type sniffing, must still refuse.
        let source = assemble(&[
            in_use("<< /Type /Font >>"),
            in_use("<< /Type /Font >>"),
            in_use("<< /Type /Font >>"),
            form(&format!(" /Escape {target} 0 R")),
        ]);
        let outcome = walk(&source, 4);
        assert!(
            matches!(
                walk_refusal(&outcome),
                CloneSetRefusal::GraphEscape { member, escape }
                    if *member == reference(target, 0) && *escape == expected
            ),
            "known structural target {target} must refuse as {expected:?}",
        );
    }
}

#[test]
fn generation_mismatch_to_a_known_page_is_resolution_failure_not_page_identity() {
    let source = assemble(&[
        in_use("<< >>"),
        in_use("<< >>"),
        in_use("<< /Type /Font >>"),
        form(" /Escape 3 1 R"),
    ]);
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::ResolutionFailed { member } if *member == reference(3, 1)
    ));
}

#[test]
fn escaped_type_name_value_still_escapes() {
    // `/P#61ge` decodes to `Page`: the value name is decoded before matching.
    let source = frontier_document("<< /Type /P#61ge >>");
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::GraphEscape {
            escape: GraphEscapeClass::Page,
            ..
        }
    ));
}

#[test]
fn ancillary_companion_keys_refuse_on_any_member() {
    let cases: &[(&str, AncillaryKeyClass)] = &[
        ("<< /OC 6 0 R >>", AncillaryKeyClass::OptionalContent),
        ("<< /Ref << >> >>", AncillaryKeyClass::ReferenceXObject),
        ("<< /F (ext.pdf) >>", AncillaryKeyClass::ExternalFile),
        ("<< /OPI << >> >>", AncillaryKeyClass::Opi),
        ("<< /PieceInfo << >> >>", AncillaryKeyClass::PieceInfo),
        ("<< /StructParent 3 >>", AncillaryKeyClass::StructParent),
        ("<< /StructParents 0 >>", AncillaryKeyClass::StructParents),
    ];
    for (body, expected) in cases {
        let source = frontier_document(body);
        let outcome = walk(&source, 4);
        assert!(
            matches!(
                walk_refusal(&outcome),
                CloneSetRefusal::AncillaryKey { member, key }
                    if *member == reference(5, 0) && key == expected
            ),
            "body {body} must refuse as {expected:?}",
        );
    }
}

#[test]
fn root_form_ancillary_keys_refuse_before_any_descent() {
    // The Table-95 integer keys sit on the ROOT Form dictionary itself.
    let source = document("3 0 R", 1, &[page("/F1 4 0 R"), form(" /StructParents 0")]);
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::AncillaryKey {
            key: AncillaryKeyClass::StructParents,
            ..
        }
    ));
}

#[test]
fn decoded_name_evasion_cannot_bypass_the_preflight() {
    // `/O#43` decodes to `OC`; `/StructP#61rent` decodes to `StructParent`.
    for (body, expected) in [
        ("<< /O#43 6 0 R >>", AncillaryKeyClass::OptionalContent),
        ("<< /StructP#61rent 3 >>", AncillaryKeyClass::StructParent),
    ] {
        let source = frontier_document(body);
        let outcome = walk(&source, 4);
        assert!(
            matches!(
                walk_refusal(&outcome),
                CloneSetRefusal::AncillaryKey { key, .. } if *key == expected
            ),
            "body {body} must refuse as {expected:?}",
        );
    }
}

#[test]
fn duplicate_decoded_keys_are_ambiguous_and_refuse() {
    // `/N#61me` decodes to `Name`, colliding with the literal spelling.
    let source = frontier_document("<< /Name 1 /N#61me 2 >>");
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::MalformedStructuralKeys { member } if *member == reference(5, 0)
    ));
}

#[test]
fn malformed_key_escape_and_non_name_type_refuse() {
    for body in ["<< /Bad#zz 1 >>", "<< /Type (Page) >>", "<< /Type 6 0 R >>"] {
        let source = frontier_document(body);
        let outcome = walk(&source, 4);
        assert!(
            matches!(
                walk_refusal(&outcome),
                CloneSetRefusal::MalformedStructuralKeys { .. }
            ),
            "body {body} must refuse as malformed/ambiguous",
        );
    }
}

#[test]
fn non_dictionary_members_need_no_preflight() {
    let source = frontier_document("[1 2 3]");
    let closure = walk(&source, 4).result.expect("array member is ordinary");
    assert_eq!(closure.members.len(), 2);
}

#[test]
fn fifo_order_refuses_a_sibling_before_its_earlier_siblings_child() {
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            form(" /First 5 0 R /Second 6 0 R"),
            in_use("<< /Child 7 0 R >>"),
            in_use("<< /Type /Page >>"),
            in_use("<< /Type /Catalog >>"),
        ],
    );
    let outcome = walk(&source, 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::GraphEscape {
            member,
            escape: GraphEscapeClass::Page,
        } if *member == reference(6, 0)
    ));
}

#[test]
fn catalog_companion_graph_membership_refuses_without_type_markers() {
    for (key, expected) in [
        ("AcroForm", GraphEscapeClass::AcroFormGraph),
        ("AcroF#6frm", GraphEscapeClass::AcroFormGraph),
        ("StructTreeRoot", GraphEscapeClass::StructureGraph),
        (
            "OCProperties",
            GraphEscapeClass::OptionalContentPropertiesGraph,
        ),
    ] {
        let source = assemble(&[
            in_use(&format!("<< /Type /Catalog /Pages 2 0 R /{key} 6 0 R >>")),
            in_use("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
            page("/F1 5 0 R"),
            page("/F2 5 0 R"),
            form(" /Companion 6 0 R"),
            in_use("<< >>"),
        ]);
        let plan = build_plan(&source);
        assert_eq!(plan.sets.len(), 2);
        for set in &plan.sets {
            assert!(
                matches!(
                    set_refusal(set),
                    CloneSetRefusal::GraphEscape { member, escape }
                        if *member == reference(6, 0) && *escape == expected
                ),
                "catalog /{key} graph must refuse without relying on /Type",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Budgets: depth, unique members, occurrences, decode work.
// ---------------------------------------------------------------------------

/// A chain fixture: page, root form (object 4), then `links` chained plain
/// dictionaries, each referencing the next. The deepest node sits at
/// reference depth `links`.
fn chain_document(links: usize) -> Vec<u8> {
    let mut rest = vec![page("/F1 4 0 R")];
    if links == 0 {
        rest.push(form(""));
    } else {
        rest.push(form(" /Next 5 0 R"));
        for step in 1..links {
            rest.push(in_use(&format!("<< /Next {} 0 R >>", 5 + step)));
        }
        rest.push(in_use("<< >>"));
    }
    document("3 0 R", 1, &rest)
}

#[test]
fn depth_sixty_four_is_admitted_and_sixty_five_refuses() {
    let closure = walk(&chain_document(MAX_CLONE_SET_REFERENCE_DEPTH), 4)
        .result
        .expect("a 64-edge chain is within the depth budget");
    assert_eq!(closure.members.len(), MAX_CLONE_SET_REFERENCE_DEPTH + 1);

    let outcome = walk(&chain_document(MAX_CLONE_SET_REFERENCE_DEPTH + 1), 4);
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::DepthBudgetExhausted { max_depth }
            if *max_depth == MAX_CLONE_SET_REFERENCE_DEPTH
    ));
    assert_eq!(
        outcome.budget.max_depth_reached, MAX_CLONE_SET_REFERENCE_DEPTH,
        "the depth budget reads exhausted on refusal",
    );
}

/// A root form whose dictionary carries `count` references to absent objects.
fn fan_out_document(count: usize) -> Vec<u8> {
    let refs = reference_list(100, count);
    document(
        "3 0 R",
        1,
        &[page("/F1 4 0 R"), form(&format!(" /Deps [{refs}]"))],
    )
}

/// A root form whose dictionary carries `count` references to distinct
/// resolvable empty dictionaries.
fn resolvable_fan_out_document(count: usize) -> Vec<u8> {
    let refs = reference_list(5, count);
    let mut rest = vec![page("/F1 4 0 R"), form(&format!(" /Deps [{refs}]"))];
    rest.extend((0..count).map(|_| in_use("<< >>")));
    document("3 0 R", 1, &rest)
}

#[test]
fn unique_member_budget_refuses_and_reads_exhausted() {
    // 4096 accepted occurrences pass the occurrence budget exactly, but the
    // root plus 4096 resolved children exceed the unique-member budget.
    let outcome = walk(
        &resolvable_fan_out_document(MAX_CLONE_SET_REFERENCE_OCCURRENCES),
        4,
    );
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::MemberBudgetExhausted { max_members }
            if *max_members == MAX_CLONE_SET_MEMBERS
    ));
    assert_eq!(outcome.budget.unique_members, MAX_CLONE_SET_MEMBERS);
    assert_eq!(
        outcome.budget.reference_occurrences,
        MAX_CLONE_SET_REFERENCE_OCCURRENCES,
    );
}

#[test]
fn null_equivalents_do_not_spend_the_unique_member_budget() {
    let outcome = walk(&fan_out_document(32), 4);
    assert_eq!(outcome.budget.unique_members, 1);
    let closure = outcome
        .result
        .expect("null-equivalent fan-out stays within the reference budget");
    assert_eq!(closure.members.len(), 1);
    assert_eq!(closure.null_equivalents.len(), 32);
}

#[test]
fn cumulative_reference_occurrence_budget_refuses_and_reads_exhausted() {
    let outcome = walk(
        &fan_out_document(MAX_CLONE_SET_REFERENCE_OCCURRENCES + 1),
        4,
    );
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::ReferenceBudgetExhausted { max_occurrences }
            if *max_occurrences == MAX_CLONE_SET_REFERENCE_OCCURRENCES
    ));
    assert_eq!(
        outcome.budget.reference_occurrences, MAX_CLONE_SET_REFERENCE_OCCURRENCES,
        "the residual is exhausted before refusing",
    );
}

/// A buffer + hand-built xref-stream section where the root form (object 4)
/// references compressed members inside raw (unfiltered) object streams of
/// the given body sizes. Member `i` is object `10 + i` inside its own
/// container `20 + i`.
fn compressed_member_fixture(container_body_sizes: &[usize]) -> (Vec<u8>, XrefStreamSection) {
    let mut buf = b"%PDF-1.5\n".to_vec();
    let mut entries = Vec::new();

    let member_refs = reference_list(10, container_body_sizes.len());
    let root_offset = buf.len();
    buf.extend_from_slice(
        format!(
            "4 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Deps [{member_refs}] /Length 0 >>\nstream\n\nendstream\nendobj\n"
        )
        .as_bytes(),
    );
    entries.push(XrefStreamEntry {
        object_number: 4,
        record: XrefStreamEntryRecord::Uncompressed {
            byte_offset: root_offset,
            generation: 0,
        },
    });

    for (index, body_size) in container_body_sizes.iter().enumerate() {
        let member_number = 10 + index;
        let container_number = 20 + index;
        let header = format!("{member_number} 0 ");
        let mut body = header.into_bytes();
        let first = body.len();
        body.extend_from_slice(b"<< >>");
        assert!(*body_size >= body.len(), "container body size too small");
        body.resize(*body_size, b' ');

        let container_offset = buf.len();
        buf.extend_from_slice(
            format!(
                "{container_number} 0 obj\n<< /Type /ObjStm /N 1 /First {first} /Length {} >>\nstream\n",
                body.len()
            )
            .as_bytes(),
        );
        buf.extend_from_slice(&body);
        buf.extend_from_slice(b"\nendstream\nendobj\n");

        entries.push(XrefStreamEntry {
            object_number: member_number,
            record: XrefStreamEntryRecord::Compressed {
                object_stream_number: container_number,
                index_within_object_stream: 0,
            },
        });
        entries.push(XrefStreamEntry {
            object_number: container_number,
            record: XrefStreamEntryRecord::Uncompressed {
                byte_offset: container_offset,
                generation: 0,
            },
        });
    }

    entries.sort_by_key(|entry| entry.object_number);
    let section = XrefStreamSection {
        object_byte_offset: 0,
        widths: [1, 2, 1],
        size: 100,
        index_subsections: Vec::new(),
        root_reference: reference(1, 0),
        prev_byte_offset: None,
        entries,
    };
    (buf, section)
}

#[test]
fn compressed_members_charge_actual_decoded_bytes() {
    let (buf, section) = compressed_member_fixture(&[1000]);
    let outcome = walk_reached_form_closure(
        &buf,
        ObjectLookup::XrefStreamSection(&section),
        &StructuralFrontier::default(),
        reference(4, 0),
    );
    let closure = outcome.result.expect("compressed member is ordinary");

    assert_eq!(closure.members.len(), 2);
    assert!(matches!(
        closure.members[1].locator,
        CloneMemberLocator::Compressed {
            object_stream_number: 20,
            index_within_object_stream: 0,
        }
    ));
    assert_eq!(
        outcome.budget.decode_work_bytes, 1000,
        "the ACTUAL decoded container length is charged",
    );
}

#[test]
fn cumulative_decode_work_budget_passes_the_residual_into_resolution() {
    // Two ~700 KiB containers: the first decodes inside the budget, the
    // second is cut off by the RESIDUAL bound passed into resolve_object.
    let size = 700 * 1024;
    let (buf, section) = compressed_member_fixture(&[size, size]);
    let outcome = walk_reached_form_closure(
        &buf,
        ObjectLookup::XrefStreamSection(&section),
        &StructuralFrontier::default(),
        reference(4, 0),
    );
    assert!(matches!(
        walk_refusal(&outcome),
        CloneSetRefusal::DecodeWorkBudgetExhausted { max_decoded_bytes }
            if *max_decoded_bytes == MAX_CLONE_SET_DECODE_WORK_BYTES
    ));
    assert_eq!(
        outcome.budget.decode_work_bytes, MAX_CLONE_SET_DECODE_WORK_BYTES,
        "the decode budget reads exhausted on refusal",
    );
}

// ---------------------------------------------------------------------------
// Seed qualification and per-page counts.
// ---------------------------------------------------------------------------

/// Two leaf pages (objects 3 and 4) sharing one Form (object 5).
fn shared_form_document(form_extra: &str) -> Vec<u8> {
    document(
        "3 0 R 4 0 R",
        2,
        &[page("/F1 5 0 R"), page("/F2 5 0 R"), form(form_extra)],
    )
}

#[test]
fn shared_form_across_two_pages_plans_two_sets_with_distinct_identities() {
    let source = shared_form_document("");
    let plan = build_plan(&source);

    assert!(plan.globally_complete);
    assert_eq!(plan.input_byte_len, source.len());
    assert_eq!(plan.sets.len(), 2);
    assert_eq!(plan.in_place_candidates, 0);

    let mut fresh_identities = Vec::new();
    for (index, set) in plan.sets.iter().enumerate() {
        assert_eq!(set.page.ordinal, index, "plan order is page-ordinal order");
        assert_eq!(set.root, reference(5, 0));
        assert_eq!(member_sources(set), vec![5]);
        assert_eq!(set.retarget_sites.len(), 1);
        assert_eq!(set.retarget_sites[0].expected_target, reference(5, 0));
        let CloneSetOutcome::Planned {
            source_to_fresh, ..
        } = &set.outcome
        else {
            panic!("expected a planned set");
        };
        assert_eq!(source_to_fresh.len(), 1);
        assert_eq!(source_to_fresh[0].0, reference(5, 0));
        fresh_identities.push(source_to_fresh[0].1);
    }
    // Distinct contiguous ascending generation-zero identities: the same
    // source on different pages never shares a fresh identity.
    assert_ne!(fresh_identities[0], fresh_identities[1]);
    assert_eq!(
        fresh_identities[1].object_number,
        fresh_identities[0].object_number + 1,
    );
    assert!(
        fresh_identities
            .iter()
            .all(|fresh| { fresh.generation == 0 && fresh.object_number > 5 })
    );

    let access = inspect_document_access(&source).expect("access");
    for (ordinal, leaf) in access.page_leaves.leaves.iter().enumerate() {
        let counts = plan.page_counts(leaf.reference, leaf.object_byte_offset, ordinal);
        assert_eq!(
            counts,
            FormCloneSetPlanCounts {
                candidate_sets: 1,
                planned_sets: 1,
                planned_objects: 1,
                ..FormCloneSetPlanCounts::new()
            },
        );
        // A failed identity join is the empty counts, fail-closed.
        assert!(
            plan.page_counts(leaf.reference, leaf.object_byte_offset + 1, ordinal)
                .is_empty()
        );
        assert!(
            plan.page_counts(leaf.reference, leaf.object_byte_offset, ordinal + 7)
                .is_empty()
        );
    }
}

#[test]
fn unselected_page_witnesses_do_not_enter_the_request_plan_or_reservation() {
    let source = shared_form_document("");
    let plan = build_plan_selected(&source, &[0]);

    assert_eq!(plan.sets.len(), 1);
    assert_eq!(plan.sets[0].page.ordinal, 0);
    assert_eq!(plan.sets[0].root, reference(5, 0));

    let access = inspect_document_access(&source).expect("access");
    let selected = &access.page_leaves.leaves[0];
    assert_eq!(
        plan.page_counts(selected.reference, selected.object_byte_offset, 0),
        FormCloneSetPlanCounts {
            candidate_sets: 1,
            planned_sets: 1,
            planned_objects: 1,
            ..FormCloneSetPlanCounts::default()
        }
    );
    let unselected = &access.page_leaves.leaves[1];
    assert!(
        plan.page_counts(unselected.reference, unselected.object_byte_offset, 1)
            .is_empty()
    );
}

#[test]
fn clone_sets_are_ordered_by_page_then_root_reference() {
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            page("/Z 6 0 R /A 5 0 R"),
            page("/Y 5 0 R /B 6 0 R"),
            form(""),
            form(""),
        ],
    );
    let plan = build_plan(&source);
    let order: Vec<(usize, u32)> = plan
        .sets
        .iter()
        .map(|set| (set.page.ordinal, set.root.object_number))
        .collect();
    assert_eq!(order, vec![(0, 5), (0, 6), (1, 5), (1, 6)]);
}

#[test]
fn multiple_names_binding_one_target_share_one_set() {
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[page("/A 5 0 R /B 5 0 R"), page("/C 5 0 R"), form("")],
    );
    let plan = build_plan(&source);

    assert_eq!(plan.sets.len(), 2);
    let first = &plan.sets[0];
    assert_eq!(first.retarget_sites.len(), 2, "both bindings are retained");
    assert_eq!(first.retarget_sites[0].name.0, b"A".to_vec());
    assert_eq!(first.retarget_sites[1].name.0, b"B".to_vec());

    let access = inspect_document_access(&source).expect("access");
    let leaf = &access.page_leaves.leaves[0];
    let counts = plan.page_counts(leaf.reference, leaf.object_byte_offset, 0);
    assert_eq!(counts.candidate_sets, 1, "one set per (page, target)");
}

#[test]
fn exclusive_form_is_an_in_place_candidate_and_never_walked() {
    // One page, one form, complete real consumer index: ProvenPageLocal.
    let source = document("3 0 R", 1, &[page("/F1 4 0 R"), form("")]);
    let plan = build_plan(&source);

    assert!(plan.sets.is_empty());
    assert_eq!(plan.in_place_candidates, 1);
    assert_eq!(plan.disqualified_witnesses, 0);
    let access = inspect_document_access(&source).expect("access");
    let leaf = &access.page_leaves.leaves[0];
    assert!(
        plan.page_counts(leaf.reference, leaf.object_byte_offset, 0)
            .is_empty()
    );
}

#[test]
fn wrong_subtype_witnesses_are_disqualified_not_walked() {
    // A shared IMAGE XObject would satisfy the exclusivity shape but the
    // subtype gate disqualifies it.
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            page("/I1 5 0 R"),
            page("/I2 5 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /Length 0 >>\nstream\n\nendstream",
            ),
        ],
    );
    let plan = build_plan(&source);
    assert!(plan.sets.is_empty());
    assert_eq!(plan.disqualified_witnesses, 2);
}

#[test]
fn indirect_resources_paths_are_disqualified_not_walked() {
    // Both pages share one INDIRECT /Resources dictionary: the first blocking
    // check is the indirect-resources locality, never target exclusivity.
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            in_use("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources 6 0 R >>"),
            in_use("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources 6 0 R >>"),
            form(""),
            in_use("<< /XObject << /F1 5 0 R >> >>"),
        ],
    );
    let plan = build_plan(&source);
    assert!(plan.sets.is_empty());
    assert_eq!(plan.disqualified_witnesses, 2);
}

#[test]
fn incomplete_consumer_index_disqualifies_every_witness() {
    let source = shared_form_document("");
    let access = inspect_document_access(&source).expect("access");
    let lookup = lookup_from_backend(&access.backend);
    let mut consumers = inspect_object_consumer_index(&source, &access);
    consumers
        .truncations
        .push(presslint_pdf::ObjectConsumerIndexTruncation {
            referrer: None,
            target: None,
            limit: presslint_pdf::ObjectConsumerIndexLimit::MaxTraversalDepth { max_depth: 0 },
        });
    let plan = FormCloneSetPlan::build(
        &source,
        lookup,
        access.root_reference,
        access.catalog_pages.pages_reference,
        access.page_tree_root.object_byte_offset,
        &[0, 1],
        &consumers,
    );
    assert!(plan.sets.is_empty());
    assert_eq!(plan.disqualified_witnesses, 2);
}

#[test]
fn incomplete_page_tree_walk_seeds_no_clone_set() {
    // A /Kids child that is neither /Pages nor /Page is a completeness-
    // relevant traversal skip: nothing qualifies, fail-closed.
    let source = document(
        "3 0 R 4 0 R 6 0 R",
        2,
        &[
            page("/F1 5 0 R"),
            page("/F2 5 0 R"),
            form(""),
            in_use("<< /Type /NotAPage >>"),
        ],
    );
    let plan = build_plan(&source);

    assert!(!plan.globally_complete);
    assert!(plan.sets.is_empty());
    let access = inspect_document_access(&source).expect("access");
    let leaf = &access.page_leaves.leaves[0];
    assert!(
        plan.page_counts(leaf.reference, leaf.object_byte_offset, 0)
            .is_empty()
    );
}

#[test]
fn duplicate_leaf_references_poison_the_page_counts_slot() {
    let source = document("3 0 R 3 0 R", 2, &[page("/F1 4 0 R"), form("")]);
    let plan = build_plan(&source);
    let access = inspect_document_access(&source).expect("access");
    for (ordinal, leaf) in access.page_leaves.leaves.iter().enumerate() {
        assert!(
            plan.page_counts(leaf.reference, leaf.object_byte_offset, ordinal)
                .is_empty(),
            "a duplicated leaf reference must fail the identity join closed",
        );
    }
}

#[test]
fn null_equivalents_are_counted_per_page() {
    // The synthetic complete-but-empty index lets the fixture carry a
    // dangling reference without poisoning index completeness.
    let source = document("3 0 R", 1, &[page("/F1 4 0 R"), form(" /Gone 40 0 R")]);
    let plan = build_plan_with_empty_consumers(&source);

    assert_eq!(plan.sets.len(), 1);
    let access = inspect_document_access(&source).expect("access");
    let leaf = &access.page_leaves.leaves[0];
    let counts = plan.page_counts(leaf.reference, leaf.object_byte_offset, 0);
    assert_eq!(counts.planned_sets, 1);
    assert_eq!(counts.planned_objects, 1);
    assert_eq!(counts.null_equivalents, 1);
}

#[test]
fn refused_set_keeps_its_first_refusal_and_counts_as_refused() {
    let source = shared_form_document(" /StructParents 0");
    let plan = build_plan(&source);

    assert_eq!(plan.sets.len(), 2);
    for set in &plan.sets {
        assert!(matches!(
            set_refusal(set),
            CloneSetRefusal::AncillaryKey {
                key: AncillaryKeyClass::StructParents,
                ..
            }
        ));
    }
    let access = inspect_document_access(&source).expect("access");
    let leaf = &access.page_leaves.leaves[0];
    let counts = plan.page_counts(leaf.reference, leaf.object_byte_offset, 0);
    assert_eq!(counts.candidate_sets, 1);
    assert_eq!(counts.refused_sets, 1);
    assert_eq!(counts.planned_sets, 0);
    assert_eq!(counts.planned_objects, 0);
}

// ---------------------------------------------------------------------------
// Reservation: all-or-nothing, floor budget honesty, Annex C.
// ---------------------------------------------------------------------------

#[test]
fn reservation_floor_budget_refuses_a_tiny_closure_as_reservation_budget() {
    // An unrelated catalog-referenced object carries 4097 references, so the
    // whole-document reservation floor proof exceeds ITS cumulative budget
    // while the clone closure itself stays tiny. The refusal must read as
    // reservation budget, never as a closure fact.
    let aux_refs: String = (0..4097).map(|_| "1 0 R ").collect();
    let source = assemble(&[
        in_use("<< /Type /Catalog /Pages 2 0 R /PressAux 6 0 R >>"),
        in_use("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
        page("/F1 5 0 R"),
        page("/F2 5 0 R"),
        form(""),
        in_use(&format!("[{aux_refs}]")),
    ]);
    let plan = build_plan(&source);

    assert_eq!(plan.sets.len(), 2);
    for set in &plan.sets {
        assert!(matches!(
            set_refusal(set),
            CloneSetRefusal::ReservationRefused {
                reason: ReservationRefusal::FloorProof { .. },
            }
        ));
    }
    let access = inspect_document_access(&source).expect("access");
    let leaf = &access.page_leaves.leaves[0];
    let counts = plan.page_counts(leaf.reference, leaf.object_byte_offset, 0);
    assert_eq!(counts.candidate_sets, 1);
    assert_eq!(counts.refused_sets, 1);
    assert_eq!(counts.planned_sets, 0);
}

#[test]
fn annex_c_object_limit_refuses_before_the_plan_is_ready() {
    // A second xref subsection defines object 8388607 (the Annex C Table C.1
    // limit), so the first fresh identity would exceed it.
    let slots = [
        in_use("<< /Type /Catalog /Pages 2 0 R /PressAux 8388607 0 R >>"),
        in_use("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
        page("/F1 5 0 R"),
        page("/F2 5 0 R"),
        form(""),
    ];
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, slot) in slots.iter().enumerate() {
        let Slot::InUse(body) = slot else {
            panic!("fixture slots are in use")
        };
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let high_offset = buf.len();
    buf.extend_from_slice(b"8388607 0 obj\n<< >>\nendobj\n");
    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(format!("8388607 1\n{high_offset:010} 00000 n \n").as_bytes());
    buf.extend_from_slice(
        format!("trailer\n<< /Size 8388608 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF")
            .as_bytes(),
    );

    let plan = build_plan(&buf);
    assert_eq!(plan.sets.len(), 2);
    for set in &plan.sets {
        assert!(matches!(
            set_refusal(set),
            CloneSetRefusal::ReservationRefused {
                reason: ReservationRefusal::AnnexCObjectLimitExceeded { .. },
            }
        ));
    }
}

// ---------------------------------------------------------------------------
// Public wiring: the counts ride the converter's per-page report.
// ---------------------------------------------------------------------------

#[test]
fn converted_pages_carry_identity_matched_clone_set_plan_counts() {
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            in_use(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 6 0 R /Resources << /XObject << /F1 5 0 R >> >> >>",
            ),
            in_use(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 7 0 R /Resources << /XObject << /F2 5 0 R >> >> >>",
            ),
            form(""),
            in_use("<< /Length 3 >>\nstream\n0 g\nendstream"),
            in_use("<< /Length 3 >>\nstream\n0 g\nendstream"),
        ],
    );
    let output = convert_content_colors_incremental(
        &source,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(GRAY_TO_GRAY_LINK),
            black_preservation: crate::BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("conversion succeeds");

    assert_eq!(output.converted.len(), 2);
    // The staged export runs in the pipeline, so the identity-matched counts
    // carry the staged counters too; the staged body is the form's exact
    // dictionary-through-`endstream` extent.
    let form_body_len =
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\n\nendstream".len();
    for page in &output.converted {
        assert_eq!(
            page.form_clone_set_plan_counts,
            FormCloneSetPlanCounts {
                candidate_sets: 1,
                planned_sets: 1,
                planned_objects: 1,
                staged_sets: 1,
                staged_objects: 1,
                staged_body_bytes: form_body_len,
                ..FormCloneSetPlanCounts::new()
            },
            "page {:?} must carry its identity-matched plan counts",
            page.page_index,
        );
    }
    // Observe-only: the emitted bytes still start with the verbatim input.
    assert_eq!(&output.bytes[..source.len()], source.as_slice());
}

#[test]
fn every_nonzero_counter_defeats_the_omit_when_empty_predicate() {
    assert!(FormCloneSetPlanCounts::new().is_empty());
    assert_eq!(
        FormCloneSetPlanCounts::new(),
        FormCloneSetPlanCounts::default()
    );
    for field in 0..9 {
        let mut counts = FormCloneSetPlanCounts::new();
        match field {
            0 => counts.candidate_sets = 1,
            1 => counts.planned_sets = 1,
            2 => counts.refused_sets = 1,
            3 => counts.planned_objects = 1,
            4 => counts.null_equivalents = 1,
            5 => counts.staged_sets = 1,
            6 => counts.staged_objects = 1,
            7 => counts.staged_body_bytes = 1,
            _ => counts.export_refused_sets = 1,
        }
        assert!(!counts.is_empty(), "field {field} must defeat is_empty");
    }
}
