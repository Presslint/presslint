//! Behaviour of the staged clone-body export (validate-only, no writer
//! hand-off in production).
//!
//! Real-plan tests run `FormCloneSetPlan::build` + `stage_export` over real
//! document access, proving the pipeline-visible behaviour: staged batches,
//! atomic counters, all-or-nothing suppression, and the byte-identity guard
//! locking the validate-only contract. Hand-built-plan tests drive `build_staged_export` directly with
//! synthetic sets to cover the corroboration matrix and the object-stream
//! member paths without oversized fixtures. Integration tests hand the built
//! batch to the public fresh-object writer to prove end-to-end viability —
//! the one hand-off production deliberately withholds.

use presslint_pdf::{
    IndirectRef, ObjectConsumerIndexInspection, ObjectLookup, ObjectStreamCacheReport,
    ResolvedObjectData, XrefStreamEntry, XrefStreamEntryRecord, XrefStreamSection,
    inspect_classic_xref_table, inspect_document_access, inspect_object_body_references,
    inspect_object_consumer_index, resolve_object,
};

use crate::{
    BlackPreservationPolicy, ConvertContentColorsRequest, FreshObjectBytes, PageSelection,
    content_edit_pipeline::lookup_from_backend,
    convert_content_colors_incremental,
    form_clone_set_plan::{
        CloneMemberLocator, CloneSetMember, CloneSetOutcome, CloneSetPageIdentity, FormCloneSet,
        FormCloneSetPlan, FormCloneSetPlanCounts,
        export::{
            CloneSetExportRefusal, MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES, build_staged_export,
        },
        walk::{CloneSetBudgetUsage, MAX_CLONE_SET_DECODE_WORK_BYTES},
    },
    write_incremental_revision, write_incremental_revision_with_fresh_objects,
};

use super::content_color_convert::{GRAY_TO_GRAY_LINK, one_link};
use super::{last_trailer_size, reopen};

pub(super) const fn reference(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

/// The default form slot's exact body extent (dictionary through the end of
/// the `endstream` keyword).
pub(super) const FORM_BODY: &str =
    "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\n\nendstream";

pub(super) type StageResult = Result<Vec<FreshObjectBytes>, CloneSetExportRefusal>;

// ---------------------------------------------------------------------------
// Classic-document fixtures (self-contained copies of the plan-test builders).
// ---------------------------------------------------------------------------

pub(super) fn in_use(body: &str) -> Vec<u8> {
    body.as_bytes().to_vec()
}

/// Assemble a classic-xref document from in-use object bodies (object numbers
/// start at 1), returning the buffer plus each object's byte offset.
pub(super) fn assemble_with_offsets(slots: &[Vec<u8>]) -> (Vec<u8>, Vec<usize>) {
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(slots.len());
    for (index, body) in slots.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = buf.len();
    let size = slots.len() + 1;
    buf.extend_from_slice(format!("xref\n0 {size}\n0000000000 65535 f \n").as_bytes());
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF")
            .as_bytes(),
    );
    (buf, offsets)
}

pub(super) fn assemble(slots: &[Vec<u8>]) -> Vec<u8> {
    assemble_with_offsets(slots).0
}

/// The standard catalog + page-tree prefix: object 1 catalog, object 2 pages
/// root, remaining slots supplied by the caller starting at object 3.
pub(super) fn document(kids: &str, count: usize, rest: &[Vec<u8>]) -> Vec<u8> {
    let mut slots = vec![
        in_use("<< /Type /Catalog /Pages 2 0 R >>"),
        in_use(&format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>")),
    ];
    slots.extend(rest.iter().cloned());
    assemble(&slots)
}

pub(super) fn page(xobject_entries: &str) -> Vec<u8> {
    in_use(&format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << {xobject_entries} >> >> >>"
    ))
}

pub(super) fn form(extra: &str) -> Vec<u8> {
    in_use(&format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1]{extra} /Length 0 >>\nstream\n\nendstream"
    ))
}

/// Two leaf pages (objects 3 and 4) sharing one Form (object 5).
pub(super) fn shared_form_document() -> Vec<u8> {
    document(
        "3 0 R 4 0 R",
        2,
        &[page("/F1 5 0 R"), page("/F2 5 0 R"), form("")],
    )
}

pub(super) fn final_startxref(source: &[u8]) -> usize {
    presslint_pdf::inspect_pdf_source(source)
        .expect("source inspects")
        .startxref
        .expect("startxref present")
        .byte_offset
}

pub(super) const fn empty_consumers(byte_len: usize) -> ObjectConsumerIndexInspection {
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

/// Build the plan over real document access and stage the export.
pub(super) fn stage(source: &[u8]) -> (FormCloneSetPlan, StageResult) {
    let access = inspect_document_access(source).expect("document access inspects");
    let lookup = lookup_from_backend(&access.backend);
    let consumers = inspect_object_consumer_index(source, &access);
    let selected: Vec<usize> = (0..access.page_leaves.leaves.len()).collect();
    let mut plan = FormCloneSetPlan::build(
        source,
        lookup,
        access.root_reference,
        access.catalog_pages.pages_reference,
        access.page_tree_root.object_byte_offset,
        &selected,
        &consumers,
    );
    let outcome = plan.stage_export(source, lookup);
    (plan, outcome)
}

/// Like [`stage`] with the synthetic complete-but-empty consumer index, so
/// single-page fixtures qualify and dangling references stay admissible.
pub(super) fn stage_with_empty_consumers(source: &[u8]) -> (FormCloneSetPlan, StageResult) {
    let access = inspect_document_access(source).expect("document access inspects");
    let lookup = lookup_from_backend(&access.backend);
    let consumers = empty_consumers(source.len());
    let selected: Vec<usize> = (0..access.page_leaves.leaves.len()).collect();
    let mut plan = FormCloneSetPlan::build(
        source,
        lookup,
        access.root_reference,
        access.catalog_pages.pages_reference,
        access.page_tree_root.object_byte_offset,
        &selected,
        &consumers,
    );
    let outcome = plan.stage_export(source, lookup);
    (plan, outcome)
}

pub(super) fn staged_fresh(outcome: StageResult) -> Vec<FreshObjectBytes> {
    match outcome {
        Ok(fresh_objects) => fresh_objects,
        Err(refusal) => {
            panic!("expected a staged batch, got refusal {refusal:?}")
        }
    }
}

pub(super) fn export_refusal(outcome: &StageResult) -> &CloneSetExportRefusal {
    match outcome {
        Err(refusal) => refusal,
        Ok(_) => panic!("expected an export refusal"),
    }
}

/// Identity-matched per-page counts for every leaf, in document order.
pub(super) fn all_page_counts(
    plan: &FormCloneSetPlan,
    source: &[u8],
) -> Vec<FormCloneSetPlanCounts> {
    let access = inspect_document_access(source).expect("document access inspects");
    access
        .page_leaves
        .leaves
        .iter()
        .enumerate()
        .map(|(ordinal, leaf)| plan.page_counts(leaf.reference, leaf.object_byte_offset, ordinal))
        .collect()
}

pub(super) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

// ---------------------------------------------------------------------------
// Hand-built plan helpers for the corroboration matrix and objstm paths.
// ---------------------------------------------------------------------------

const fn zero_budget() -> CloneSetBudgetUsage {
    CloneSetBudgetUsage {
        max_depth_reached: 0,
        unique_members: 0,
        reference_occurrences: 0,
        decode_work_bytes: 0,
    }
}

/// One hand-built planned set with sequential fresh identities from
/// `fresh_first`, aligned with `members`.
pub(super) fn hand_built_set(
    root: IndirectRef,
    root_object_byte_offset: usize,
    members: Vec<CloneSetMember>,
    fresh_first: u32,
) -> FormCloneSet {
    let source_to_fresh = members
        .iter()
        .enumerate()
        .map(|(index, member)| {
            (
                member.source,
                reference(fresh_first + u32::try_from(index).expect("small index"), 0),
            )
        })
        .collect();
    FormCloneSet {
        page: CloneSetPageIdentity {
            ordinal: 0,
            reference: reference(3, 0),
            object_byte_offset: 0,
        },
        root,
        root_object_byte_offset,
        retarget_sites: Vec::new(),
        budget: zero_budget(),
        page_ownership: None,
        outcome: CloneSetOutcome::Planned {
            members,
            null_equivalents: Vec::new(),
            source_to_fresh,
        },
    }
}

pub(super) fn uncompressed_member(
    object_number: u32,
    object_byte_offset: usize,
    outgoing: Vec<IndirectRef>,
) -> CloneSetMember {
    CloneSetMember {
        source: reference(object_number, 0),
        locator: CloneMemberLocator::Uncompressed { object_byte_offset },
        outgoing,
    }
}

/// A single-page classic doc whose form (object 4) has exactly the supplied
/// slot body, with the parsed xref table and the form's object offset.
fn hand_built_document(
    form_slot_body: &str,
    extra_slots: &[Vec<u8>],
) -> (Vec<u8>, presslint_pdf::ClassicXrefTableInspection, usize) {
    let mut slots = vec![
        in_use("<< /Type /Catalog /Pages 2 0 R >>"),
        in_use("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        page("/F1 4 0 R"),
        in_use(form_slot_body),
    ];
    slots.extend(extra_slots.iter().cloned());
    let (buf, offsets) = assemble_with_offsets(&slots);
    let xref = inspect_classic_xref_table(&buf, final_startxref(&buf)).expect("xref inspects");
    (buf, xref, offsets[3])
}

/// Buffer + hand-built xref-stream section: root form (object 4, uncompressed)
/// references compressed member `10 + i` inside its own raw `/ObjStm`
/// container `20 + i`, whose bare body is `member_bodies[i]`.
fn compressed_fixture(member_bodies: &[&[u8]]) -> (Vec<u8>, XrefStreamSection, usize) {
    use std::fmt::Write as _;
    let mut buf = b"%PDF-1.5\n".to_vec();
    let mut entries = Vec::new();

    let member_refs = (0..member_bodies.len()).fold(String::new(), |mut refs, index| {
        let _ = write!(refs, "{} 0 R ", 10 + index);
        refs
    });
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

    for (index, member_body) in member_bodies.iter().enumerate() {
        let member_number = 10 + index;
        let container_number = 20 + index;
        let mut body = format!("{member_number} 0 ").into_bytes();
        let first = body.len();
        body.extend_from_slice(member_body);

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
    (buf, section, root_offset)
}

fn compressed_member(
    object_number: u32,
    object_stream_number: usize,
    outgoing: Vec<IndirectRef>,
) -> CloneSetMember {
    CloneSetMember {
        source: reference(object_number, 0),
        locator: CloneMemberLocator::Compressed {
            object_stream_number,
            index_within_object_stream: 0,
        },
        outgoing,
    }
}

// ---------------------------------------------------------------------------
// Staged batches over real plans: order, bodies, counters.
// ---------------------------------------------------------------------------

#[test]
fn shared_form_stages_two_sets_in_plan_order_with_contiguous_fresh_identities() {
    let source = shared_form_document();
    let (plan, outcome) = stage(&source);
    let batch = staged_fresh(outcome);

    assert_eq!(batch.len(), 2, "one member per set, two page-specific sets");
    // Contiguous ascending generation-zero coverage in plan order.
    assert_eq!(
        batch[1].reference.object_number,
        batch[0].reference.object_number + 1,
    );
    assert!(batch.iter().all(|fresh| fresh.reference.generation == 0));
    // No in-set references: the staged body is the source extent verbatim.
    for fresh in &batch {
        assert_eq!(fresh.body_bytes, FORM_BODY.as_bytes());
    }

    for counts in all_page_counts(&plan, &source) {
        assert_eq!(
            counts,
            FormCloneSetPlanCounts {
                candidate_sets: 1,
                planned_sets: 1,
                planned_objects: 1,
                staged_sets: 1,
                staged_objects: 1,
                staged_body_bytes: FORM_BODY.len(),
                ..FormCloneSetPlanCounts::new()
            },
        );
    }
}

#[test]
fn splice_rewrites_only_numeric_tokens_and_preserves_interior_comments() {
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            page("/F1 5 0 R"),
            page("/F2 5 0 R"),
            form(" /Resources << /XObject << /N 6 % keep\n 0 R >> >>"),
            in_use("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ],
    );
    let (_, outcome) = stage(&source);
    let batch = staged_fresh(outcome);

    // Two sets, two members each (form 5 + font 6), plan order.
    assert_eq!(batch.len(), 4);
    for set_start in [0, 2] {
        let form_clone = &batch[set_start];
        let font_clone = &batch[set_start + 1];
        let expected_form_body = format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Resources << /XObject << /N {} % keep\n 0 R >> >> /Length 0 >>\nstream\n\nendstream",
            font_clone.reference.object_number,
        );
        assert_eq!(
            form_clone.body_bytes,
            expected_form_body.as_bytes(),
            "only the two numeric tokens change; comment and R keyword stay",
        );
        assert_eq!(
            font_clone.body_bytes,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        );
    }
}

#[test]
fn null_equivalent_reference_tokens_are_preserved_unchanged() {
    let source = document("3 0 R", 1, &[page("/F1 4 0 R"), form(" /Gone 40 0 R")]);
    let (_, outcome) = stage_with_empty_consumers(&source);
    let batch = staged_fresh(outcome);

    assert_eq!(batch.len(), 1);
    assert!(
        contains(&batch[0].body_bytes, b"/Gone 40 0 R"),
        "the null-equivalent token is preserved byte-for-byte",
    );
}

#[test]
fn no_ready_sets_stage_an_empty_batch_and_no_counters() {
    // One page, one form, complete real consumer index: ProvenPageLocal.
    let source = document("3 0 R", 1, &[page("/F1 4 0 R"), form("")]);
    let (plan, outcome) = stage(&source);

    assert!(staged_fresh(outcome).is_empty());
    for counts in all_page_counts(&plan, &source) {
        assert!(counts.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Stream framing: LF / CRLF preserved verbatim, lone CR refuses.
// ---------------------------------------------------------------------------

#[test]
fn crlf_stream_framing_is_preserved_byte_for_byte() {
    let body =
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 2 >>\nstream\r\nAB\r\nendstream";
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[page("/F1 5 0 R"), page("/F2 5 0 R"), in_use(body)],
    );
    let (_, outcome) = stage(&source);
    let batch = staged_fresh(outcome);

    assert_eq!(batch[0].body_bytes, body.as_bytes());
}

#[test]
fn lone_cr_stream_framing_refuses_the_whole_batch() {
    let body = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\rendstream";
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[page("/F1 5 0 R"), page("/F2 5 0 R"), in_use(body)],
    );
    let (plan, outcome) = stage(&source);

    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::StreamFraming { member } if *member == reference(5, 0)
    ));
    for counts in all_page_counts(&plan, &source) {
        assert_eq!(counts.export_refused_sets, 1);
        assert_eq!(counts.staged_sets, 0);
        assert_eq!(counts.staged_objects, 0);
        assert_eq!(counts.staged_body_bytes, 0);
    }
}

// ---------------------------------------------------------------------------
// /Length verdict matrix.
// ---------------------------------------------------------------------------

#[test]
fn in_set_indirect_length_rewrites_the_reference_and_copies_the_integer_verbatim() {
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 5 0 R >>\nstream\nXYZ\nendstream",
            ),
            in_use("3"),
        ],
    );
    let (_, outcome) = stage_with_empty_consumers(&source);
    let batch = staged_fresh(outcome);

    assert_eq!(batch.len(), 2, "the /Length integer is an ordinary member");
    let integer_fresh = batch[1].reference.object_number;
    let expected_form = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length {integer_fresh} 0 R >>\nstream\nXYZ\nendstream"
    );
    assert_eq!(batch[0].body_bytes, expected_form.as_bytes());
    assert_eq!(batch[1].body_bytes, b"3", "integer object copied verbatim");
}

#[test]
fn self_referential_length_refuses_explicitly() {
    // pypdf #3112 class: the plan admits the cycle; the export must refuse.
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
    let (_, outcome) = stage_with_empty_consumers(&source);

    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::LengthSelfReferential { member } if *member == reference(4, 0)
    ));
}

#[test]
fn null_equivalent_length_refuses_without_repair() {
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 40 0 R >>\nstream\n\nendstream",
            ),
        ],
    );
    let (_, outcome) = stage_with_empty_consumers(&source);

    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::LengthNullEquivalent { member } if *member == reference(4, 0)
    ));
}

#[test]
fn extent_mismatched_length_refuses_without_recompute() {
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 5 >>\nstream\nAB\nendstream",
            ),
        ],
    );
    let (_, outcome) = stage_with_empty_consumers(&source);

    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::LengthExtentMismatch { member } if *member == reference(4, 0)
    ));
}

#[test]
fn missing_and_non_numeric_length_refuse() {
    let missing = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] >>\nstream\n\nendstream"),
        ],
    );
    let (_, outcome) = stage_with_empty_consumers(&missing);
    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::LengthMissing { .. }
    ));

    let null_length = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length null >>\nstream\n\nendstream",
            ),
        ],
    );
    let (_, outcome) = stage_with_empty_consumers(&null_length);
    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::LengthUnsupportedShape { .. }
    ));
}

#[test]
fn out_of_set_resolvable_length_is_a_plan_consistency_refusal() {
    // Hand-built: the /Length target (object 5) resolves but is NOT a set
    // member — the closure walk would have admitted it, so the plan is
    // inconsistent and the export must refuse.
    let (source, xref, form_offset) = hand_built_document(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 5 0 R >>\nstream\nXYZ\nendstream",
        &[in_use("3")],
    );
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            form_offset,
            vec![uncompressed_member(4, form_offset, vec![reference(5, 0)])],
            100,
        )],
    );

    let refusal = build_staged_export(&source, ObjectLookup::ClassicXref(&xref), &plan)
        .expect_err("out-of-set /Length must refuse");
    assert!(matches!(
        refusal,
        CloneSetExportRefusal::LengthOutOfSet { member } if member == reference(4, 0)
    ));
}

#[test]
fn escaped_length_spelling_and_semantic_collision_refuse_before_extent_lookup() {
    for (body, expected_collision) in [
        (
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Lengt#68 0 >>\nstream\n\nendstream",
            false,
        ),
        (
            "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Lengt#68 0 /Length 0 >>\nstream\n\nendstream",
            true,
        ),
    ] {
        let (source, xref, form_offset) = hand_built_document(body, &[]);
        let set = hand_built_set(
            reference(4, 0),
            form_offset,
            vec![uncompressed_member(4, form_offset, Vec::new())],
            100,
        );
        let refusal = export_error(&source, &xref, vec![set], source.len());
        assert!(
            matches!(
                refusal,
                CloneSetExportRefusal::LengthDuplicate { .. }
                    if expected_collision
            ) || matches!(
                refusal,
                CloneSetExportRefusal::LengthNonCanonical { .. }
                    if !expected_collision
            )
        );
    }
}

// ---------------------------------------------------------------------------
// Corroboration matrix (hand-built plans over a real classic document).
// ---------------------------------------------------------------------------

/// The baseline hand-built single-member set over the plain form fixture.
fn baseline_fixture() -> (Vec<u8>, presslint_pdf::ClassicXrefTableInspection, usize) {
    hand_built_document(FORM_BODY, &[])
}

fn export_error(
    source: &[u8],
    xref: &presslint_pdf::ClassicXrefTableInspection,
    sets: Vec<FormCloneSet>,
    input_byte_len: usize,
) -> CloneSetExportRefusal {
    let plan = FormCloneSetPlan::from_sets_for_tests(input_byte_len, sets);
    build_staged_export(source, ObjectLookup::ClassicXref(xref), &plan)
        .expect_err("corroboration must refuse")
}

#[test]
fn corroboration_matrix_refuses_every_inconsistency() {
    let (source, xref, form_offset) = baseline_fixture();
    let good_member = || uncompressed_member(4, form_offset, Vec::new());
    let good_set = |fresh_first| {
        hand_built_set(
            reference(4, 0),
            form_offset,
            vec![good_member()],
            fresh_first,
        )
    };

    // The baseline itself stages cleanly.
    let plan = FormCloneSetPlan::from_sets_for_tests(source.len(), vec![good_set(100)]);
    let batch = build_staged_export(&source, ObjectLookup::ClassicXref(&xref), &plan)
        .expect("baseline stages");
    assert_eq!(batch.fresh_objects.len(), 1);
    assert_eq!(batch.fresh_objects[0].reference, reference(100, 0));
    assert_eq!(batch.staged_sets.len(), 1);
    assert_eq!(batch.staged_sets[0].body_bytes_total, FORM_BODY.len());

    // Input length mismatch.
    assert!(matches!(
        export_error(&source, &xref, vec![good_set(100)], source.len() + 1),
        CloneSetExportRefusal::InputLengthMismatch { .. }
    ));

    // Locator mismatch: the retained locator (consistent with the retained
    // root offset, so the mapping check passes) no longer matches the
    // re-resolved offset.
    let set = hand_built_set(
        reference(4, 0),
        form_offset + 1,
        vec![uncompressed_member(4, form_offset + 1, Vec::new())],
        100,
    );
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::LocatorMismatch { member } if member == reference(4, 0)
    ));

    // Census mismatch: the retained outgoing list disagrees with the re-scan.
    let set = hand_built_set(
        reference(4, 0),
        form_offset,
        vec![uncompressed_member(4, form_offset, vec![reference(9, 0)])],
        100,
    );
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::CensusMismatch { member } if member == reference(4, 0)
    ));

    // Misaligned mapping: no pairs for one member.
    let mut set = good_set(100);
    let CloneSetOutcome::Planned {
        source_to_fresh, ..
    } = &mut set.outcome
    else {
        unreachable!()
    };
    source_to_fresh.clear();
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::MappingMisaligned { .. }
    ));

    // A repeated non-root source would be silently collapsed by BTreeMap
    // collection unless it is rejected first.
    let duplicate = uncompressed_member(5, form_offset, Vec::new());
    let set = hand_built_set(
        reference(4, 0),
        form_offset,
        vec![good_member(), duplicate.clone(), duplicate],
        100,
    );
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::MappingDuplicateSource { source }
            if source == reference(5, 0)
    ));

    // Nonzero fresh generation.
    let mut set = good_set(100);
    let CloneSetOutcome::Planned {
        source_to_fresh, ..
    } = &mut set.outcome
    else {
        unreachable!()
    };
    source_to_fresh[0].1 = reference(100, 1);
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::MappingNotGenerationZero { .. }
    ));

    // Non-contiguous reservation coverage across plan order.
    assert!(matches!(
        export_error(
            &source,
            &xref,
            vec![good_set(100), good_set(102)],
            source.len(),
        ),
        CloneSetExportRefusal::ReservationNotContiguous { expected: 101, .. }
    ));

    // Root not among the members.
    let set = hand_built_set(reference(9, 0), form_offset, vec![good_member()], 100);
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::RootNotUnique { root } if root == reference(9, 0)
    ));

    // Root offset mismatch against the retained binding witness offset.
    let set = hand_built_set(reference(4, 0), form_offset + 1, vec![good_member()], 100);
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::RootOffsetMismatch { root } if root == reference(4, 0)
    ));

    // Resolution failure: a generation the xref does not carry.
    let mut member = good_member();
    member.source = reference(4, 1);
    let set = hand_built_set(reference(4, 1), form_offset, vec![member], 100);
    assert!(matches!(
        export_error(&source, &xref, vec![set], source.len()),
        CloneSetExportRefusal::MemberResolutionFailed { member } if member == reference(4, 1)
    ));
}

// ---------------------------------------------------------------------------
// Object-stream members: materialization, rebasing, §7.5.7 admission, budget.
// ---------------------------------------------------------------------------

#[test]
fn compressed_member_materializes_as_plain_gen_zero_body_with_rebased_splices() {
    let (source, section, root_offset) = compressed_fixture(&[b"<< /P 4 0 R >>"]);
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            root_offset,
            vec![
                uncompressed_member(4, root_offset, vec![reference(10, 0)]),
                compressed_member(10, 20, vec![reference(4, 0)]),
            ],
            100,
        )],
    );

    let batch = build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
        .expect("compressed member stages");

    assert_eq!(batch.fresh_objects.len(), 2);
    // Root form: the /Deps reference is renumbered to the member's fresh
    // identity (101), framing preserved.
    let expected_root = "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Deps [101 0 R ] /Length 0 >>\nstream\n\nendstream";
    assert_eq!(batch.fresh_objects[0].reference, reference(100, 0));
    assert_eq!(batch.fresh_objects[0].body_bytes, expected_root.as_bytes());
    // Compressed member: decoded-buffer ranges rebased against the body span,
    // materialized as a bare generation-zero body.
    assert_eq!(batch.fresh_objects[1].reference, reference(101, 0));
    assert_eq!(batch.fresh_objects[1].body_bytes, b"<< /P 100 0 R >>");
}

#[test]
fn objstm_literal_and_hex_strings_copy_exactly_and_keep_reference_text_opaque() {
    let bodies: [&[u8]; 2] = [br"(4 0 R \(nested\))", b"<3420302052>"];
    let (source, section, root_offset) = compressed_fixture(&bodies);
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            root_offset,
            vec![
                uncompressed_member(4, root_offset, vec![reference(10, 0), reference(11, 0)]),
                compressed_member(10, 20, Vec::new()),
                compressed_member(11, 21, Vec::new()),
            ],
            100,
        )],
    );

    let batch = build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
        .expect("valid string members stage");
    assert_eq!(batch.fresh_objects[1].body_bytes, bodies[0]);
    assert_eq!(batch.fresh_objects[2].body_bytes, bodies[1]);
}

#[test]
fn objstm_unterminated_strings_and_trailing_second_values_refuse() {
    for body in [
        &b"(unterminated"[..],
        &b"<1234"[..],
        &b"(one) 2"[..],
        &b"<00> /Two"[..],
    ] {
        let (source, section, root_offset) = compressed_fixture(&[body]);
        let plan = FormCloneSetPlan::from_sets_for_tests(
            source.len(),
            vec![hand_built_set(
                reference(4, 0),
                root_offset,
                vec![
                    uncompressed_member(4, root_offset, vec![reference(10, 0)]),
                    compressed_member(10, 20, Vec::new()),
                ],
                100,
            )],
        );
        let refusal =
            build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
                .expect_err("member span must contain exactly one complete string value");
        assert!(matches!(
            refusal,
            CloneSetExportRefusal::ObjectStreamMemberNotSingleValue { member }
                if member == reference(10, 0)
        ));
    }
}

#[test]
fn objstm_number_member_requires_one_valid_pdf_number_lexeme() {
    let valid = &b"-.002"[..];
    let (source, section, root_offset) = compressed_fixture(&[valid]);
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            root_offset,
            vec![
                uncompressed_member(4, root_offset, vec![reference(10, 0)]),
                compressed_member(10, 20, Vec::new()),
            ],
            100,
        )],
    );
    let batch = build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
        .expect("a valid PDF real stages");
    assert_eq!(batch.fresh_objects[1].body_bytes, valid);

    for invalid in [&b"+"[..], &b"."[..], &b"1.2.3"[..]] {
        let (source, section, root_offset) = compressed_fixture(&[invalid]);
        let plan = FormCloneSetPlan::from_sets_for_tests(
            source.len(),
            vec![hand_built_set(
                reference(4, 0),
                root_offset,
                vec![
                    uncompressed_member(4, root_offset, vec![reference(10, 0)]),
                    compressed_member(10, 20, Vec::new()),
                ],
                100,
            )],
        );
        assert!(matches!(
            build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan),
            Err(CloneSetExportRefusal::ObjectStreamMemberNotSingleValue { member })
                if member == reference(10, 0)
        ));
    }
}

#[test]
fn uncompressed_string_body_remains_an_unsupported_shape() {
    for body in ["(literal)", "<00>"] {
        let (source, xref, offset) = hand_built_document(body, &[]);
        let set = hand_built_set(
            reference(4, 0),
            offset,
            vec![uncompressed_member(4, offset, Vec::new())],
            100,
        );
        assert!(matches!(
            export_error(&source, &xref, vec![set], source.len()),
            CloneSetExportRefusal::UnsupportedBodyShape { member }
                if member == reference(4, 0)
        ));
    }
}

#[test]
fn objstm_member_solely_a_reference_refuses_per_spec() {
    let (source, section, root_offset) = compressed_fixture(&[b"4 0 R"]);
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            root_offset,
            vec![
                uncompressed_member(4, root_offset, vec![reference(10, 0)]),
                compressed_member(10, 20, vec![reference(4, 0)]),
            ],
            100,
        )],
    );

    let refusal = build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
        .expect_err("a member that is solely a reference must refuse");
    assert!(matches!(
        refusal,
        CloneSetExportRefusal::ObjectStreamMemberSolelyReference { member }
            if member == reference(10, 0)
    ));
}

#[test]
fn objstm_member_with_trailing_second_value_refuses_one_value_consumption() {
    let (source, section, root_offset) = compressed_fixture(&[b"<< >> 5"]);
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            root_offset,
            vec![
                uncompressed_member(4, root_offset, vec![reference(10, 0)]),
                compressed_member(10, 20, Vec::new()),
            ],
            100,
        )],
    );

    let refusal = build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
        .expect_err("a multi-value member span must refuse");
    assert!(matches!(
        refusal,
        CloneSetExportRefusal::ObjectStreamMemberNotSingleValue { member }
            if member == reference(10, 0)
    ));
}

#[test]
fn objstm_member_with_trailing_comment_is_still_one_value() {
    let (source, section, root_offset) = compressed_fixture(&[b"<< >> % trailing note"]);
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            root_offset,
            vec![
                uncompressed_member(4, root_offset, vec![reference(10, 0)]),
                compressed_member(10, 20, Vec::new()),
            ],
            100,
        )],
    );

    let batch = build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
        .expect("trailing trivia is not a second value");
    assert_eq!(batch.fresh_objects[1].body_bytes, b"<< >> % trailing note");
}

#[test]
fn per_set_decode_budget_is_reapplied_during_export_re_resolution() {
    // Two ~700 KiB containers: the first decodes inside the per-set budget,
    // the second is cut off by the residual bound.
    let size = 700 * 1024;
    let mut padded_body = b"<< >>".to_vec();
    padded_body.resize(size, b' ');
    let (source, section, root_offset) =
        compressed_fixture(&[padded_body.as_slice(), padded_body.as_slice()]);
    let plan = FormCloneSetPlan::from_sets_for_tests(
        source.len(),
        vec![hand_built_set(
            reference(4, 0),
            root_offset,
            vec![
                uncompressed_member(4, root_offset, vec![reference(10, 0), reference(11, 0)]),
                compressed_member(10, 20, Vec::new()),
                compressed_member(11, 21, Vec::new()),
            ],
            100,
        )],
    );

    let refusal = build_staged_export(&source, ObjectLookup::XrefStreamSection(&section), &plan)
        .expect_err("the second container decode must exhaust the per-set budget");
    assert!(matches!(
        refusal,
        CloneSetExportRefusal::DecodeWorkBudgetExhausted { max_decoded_bytes }
            if max_decoded_bytes == MAX_CLONE_SET_DECODE_WORK_BYTES
    ));
}

// ---------------------------------------------------------------------------
// Request-level materialized-body budget: charge before copy, duplicates
// count each time, typed refusal distinct from other classes.
// ---------------------------------------------------------------------------

#[test]
fn cumulative_materialized_body_budget_counts_duplicate_bodies_per_set() {
    // One 32 MiB form shared by two pages: each page-specific set charges the
    // full extent, so the second pre-charge exceeds the 64 MiB budget BEFORE
    // any copy work for that set.
    let data_len = 32 * 1024 * 1024;
    let mut form_body =
        format!("<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length {data_len} >>\nstream\n")
            .into_bytes();
    form_body.resize(form_body.len() + data_len, b'x');
    form_body.extend_from_slice(b"\nendstream");
    let slots = vec![
        in_use("<< /Type /Catalog /Pages 2 0 R >>"),
        in_use("<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>"),
        page("/F1 5 0 R"),
        page("/F2 5 0 R"),
        form_body,
    ];
    let source = assemble(&slots);

    let (plan, outcome) = stage(&source);

    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::MaterializedBodyBudgetExceeded { max_body_bytes }
            if *max_body_bytes == MAX_FORM_CLONE_MATERIALIZED_BODY_BYTES
    ));
    // All-or-nothing: the first set had already materialized, yet no staged
    // counter survives — both pages read export-suppressed.
    for counts in all_page_counts(&plan, &source) {
        assert_eq!(counts.planned_sets, 1);
        assert_eq!(counts.export_refused_sets, 1);
        assert_eq!(counts.staged_sets, 0);
        assert_eq!(counts.staged_body_bytes, 0);
    }
}

// ---------------------------------------------------------------------------
// All-or-nothing suppression and counter atomicity.
// ---------------------------------------------------------------------------

#[test]
fn one_failing_set_discards_the_whole_batch_and_suppresses_every_ready_set() {
    let lone_cr_form =
        "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\rendstream";
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            page("/A 5 0 R /B 6 0 R"),
            page("/C 5 0 R /D 6 0 R"),
            form(""),
            in_use(lone_cr_form),
        ],
    );
    let (plan, outcome) = stage(&source);

    // Plan order: set (page 0, root 5) stages fine BEFORE set (page 0,
    // root 6) fails — yet nothing partial is ever published.
    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::StreamFraming { member } if *member == reference(6, 0)
    ));
    for counts in all_page_counts(&plan, &source) {
        assert_eq!(counts.candidate_sets, 2);
        assert_eq!(counts.planned_sets, 2);
        assert_eq!(counts.export_refused_sets, 2);
        assert_eq!(counts.staged_sets, 0);
        assert_eq!(counts.staged_objects, 0);
        assert_eq!(counts.staged_body_bytes, 0);
    }
}

// ---------------------------------------------------------------------------
// Doctrine guard: staged export never changes emitted product bytes.
// ---------------------------------------------------------------------------

/// Two pages sharing one form, each with its own content stream.
pub(super) fn convertible_clone_set_document(content: &str) -> Vec<u8> {
    document(
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
            in_use(&format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )),
            in_use(&format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            )),
        ],
    )
}

#[test]
fn staged_export_leaves_emitted_bytes_byte_identical_to_pre_export_behaviour() {
    // No convertible colour operator: the pre-export pipeline emitted exactly
    // the empty incremental append. With staging active the emitted bytes
    // must stay BYTE-IDENTICAL, even though a real batch was built.
    let source = convertible_clone_set_document("q\nQ");
    let output = convert_content_colors_incremental(
        &source,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: one_link(GRAY_TO_GRAY_LINK),
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("conversion succeeds");

    // Staging really happened and is visible in the telemetry...
    assert_eq!(output.converted.len(), 2);
    for page in &output.converted {
        assert_eq!(page.form_clone_set_plan_counts.staged_sets, 1);
        assert_eq!(page.form_clone_set_plan_counts.staged_objects, 1);
    }
    // ...but the emitted bytes are exactly the pre-export behaviour.
    let baseline =
        write_incremental_revision(&source, &[]).expect("baseline empty append succeeds");
    assert_eq!(output.bytes, baseline);

    // And no staged fresh identity leaks into the appended revision.
    let (_, outcome) = stage(&source);
    let tail = &output.bytes[source.len()..];
    for fresh in staged_fresh(outcome) {
        let header = format!("{} 0 obj", fresh.reference.object_number);
        assert!(
            !contains(tail, header.as_bytes()),
            "no clone body may be emitted before the retarget slice",
        );
    }
}

// ---------------------------------------------------------------------------
// Writer integration (test-only hand-off proving end-to-end viability).
// ---------------------------------------------------------------------------

#[test]
fn built_batch_drives_the_public_fresh_object_writer_end_to_end() {
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            page("/F1 5 0 R"),
            page("/F2 5 0 R"),
            form(" /Resources << /XObject << /N 6 0 R >> >>"),
            in_use("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"),
        ],
    );
    let (_, outcome) = stage(&source);
    let batch = staged_fresh(outcome);
    assert_eq!(batch.len(), 4);

    let output = write_incremental_revision_with_fresh_objects(&source, &[], &batch)
        .expect("the staged batch satisfies the writer's independent floor proof");

    // Prefix preservation and a /Size covering the fresh identities
    // (PDFBOX-5945 interaction: the writer's existing defense).
    assert_eq!(&output[..source.len()], source.as_slice());
    let highest_fresh = batch
        .iter()
        .map(|fresh| fresh.reference.object_number)
        .max()
        .expect("batch is nonempty");
    assert!(last_trailer_size(&output) > highest_fresh as usize);

    // The output reopens and every clone resolves at its fresh identity with
    // the renumbered census: each cloned form references its set's cloned
    // font — reachability-consistent renumbering, end to end.
    let access = reopen(&output);
    let lookup = lookup_from_backend(&access.backend);
    for set_start in [0, 2] {
        let form_clone = &batch[set_start];
        let font_clone = &batch[set_start + 1];
        let resolved = resolve_object(&output, lookup, form_clone.reference, 1 << 20)
            .expect("cloned form resolves in the appended revision");
        let ResolvedObjectData::Uncompressed { resolved } = resolved else {
            panic!("appended clones are uncompressed");
        };
        let census = inspect_object_body_references(&output, resolved.object_byte_offset)
            .expect("cloned form body scans");
        assert_eq!(census.references, vec![font_clone.reference]);
        assert!(
            resolve_object(&output, lookup, font_clone.reference, 1 << 20).is_ok(),
            "the cloned font resolves at its fresh identity",
        );
    }
}
