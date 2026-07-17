//! Behaviour of the proof-only clone commit transaction: the uncompressed
//! single-value-through-`endobj` export prerequisite, the pre-reservation
//! page-leaf retarget proof, and the request-atomic `CloneCommitBatch`
//! (grouping, corroboration, retarget materialization, suppression, and the
//! emitted-byte identity guard). Production builds the batch and deliberately
//! drops it; these tests build the same batch directly.

use presslint_actions::{
    DictionaryEntryOp, DictionaryValueLocator, MutationBoundary, PlannedValueProvenance,
};
use presslint_pdf::{
    ClassicXrefTableInspection, IndirectObjectEditDisposition, IndirectObjectOwnership,
    inspect_classic_xref_table, inspect_document_access,
};

use crate::{
    BlackPreservationPolicy, ConvertContentColorsRequest, PageSelection,
    content_edit_pipeline::lookup_from_backend,
    convert_content_colors_incremental,
    form_clone_set_plan::{
        CloneSetOutcome, CloneSetPageIdentity, CloneSetRefusal, FormCloneSet, FormCloneSetPlan,
        PageRetargetProofRefusal,
        commit::{CloneCommitBatch, CloneCommitRefusal, build_clone_commit_batch},
        export::{CloneSetExportRefusal, build_staged_export},
    },
    write_incremental_revision,
};

use super::content_color_convert::{GRAY_TO_GRAY_LINK, one_link};
use super::form_clone_export::{
    FORM_BODY, all_page_counts, contains, convertible_clone_set_document, document,
    empty_consumers, export_refusal, final_startxref, form, hand_built_set, in_use, page,
    reference, shared_form_document, stage, stage_with_empty_consumers, staged_fresh,
    uncompressed_member,
};

fn set_refusal(set: &FormCloneSet) -> &CloneSetRefusal {
    match &set.outcome {
        CloneSetOutcome::Refused { refusal } => refusal,
        CloneSetOutcome::Planned { .. } => panic!("expected a refused set"),
    }
}

/// Build the plan directly with an explicit page selection and the synthetic
/// complete-but-empty consumer index.
fn build_plan_with_selected(source: &[u8], selected: &[usize]) -> FormCloneSetPlan {
    let access = inspect_document_access(source).expect("document access inspects");
    let lookup = lookup_from_backend(&access.backend);
    let consumers = empty_consumers(source.len());
    FormCloneSetPlan::build(
        source,
        lookup,
        access.root_reference,
        access.catalog_pages.pages_reference,
        access.page_tree_root.object_byte_offset,
        selected,
        &consumers,
    )
}

/// Stage a real plan and materialize its commit batch.
fn commit(source: &[u8]) -> (FormCloneSetPlan, CloneCommitBatch) {
    let (plan, outcome) = stage(source);
    let staged = staged_fresh(outcome);
    let batch = build_clone_commit_batch(source, &plan, staged).expect("commit batch materializes");
    (plan, batch)
}

// ---------------------------------------------------------------------------
// Uncompressed single-value-through-endobj export prerequisite.
// ---------------------------------------------------------------------------

/// Classic doc: objects 1..=3 standard, object 4 written as
/// `4 0 obj\n{FORM_BODY}{tail}` with NO automatic `endobj`, then the xref.
fn document_with_form_tail(tail: &str) -> (Vec<u8>, ClassicXrefTableInspection, usize) {
    let slots = [
        in_use("<< /Type /Catalog /Pages 2 0 R >>"),
        in_use("<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        page("/F1 4 0 R"),
    ];
    let mut buf = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in slots.iter().enumerate() {
        offsets.push(buf.len());
        buf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let form_offset = buf.len();
    buf.extend_from_slice(b"4 0 obj\n");
    buf.extend_from_slice(FORM_BODY.as_bytes());
    buf.extend_from_slice(tail.as_bytes());
    offsets.push(form_offset);
    let xref_offset = buf.len();
    buf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF").as_bytes(),
    );
    let xref = inspect_classic_xref_table(&buf, final_startxref(&buf)).expect("xref inspects");
    (buf, xref, form_offset)
}

#[test]
fn endobj_admission_matrix_over_hand_built_object_framing() {
    // Trailing trivia (whitespace and comments) before one exact
    // delimiter-bounded `endobj` is admitted; everything else refuses.
    for (tail, admitted) in [
        ("\nendobj\n", true),
        ("\n% trailing note\nendobj\n", true),
        (" endobj]", true),
        ("\n5\nendobj\n", false),
        ("\nendobjX\nendobj\n", false),
        ("\n", false),
    ] {
        let (source, xref, form_offset) = document_with_form_tail(tail);
        let plan = FormCloneSetPlan::from_sets_for_tests(
            source.len(),
            vec![hand_built_set(
                reference(4, 0),
                form_offset,
                vec![uncompressed_member(4, form_offset, Vec::new())],
                100,
            )],
        );
        let outcome = build_staged_export(
            &source,
            presslint_pdf::ObjectLookup::ClassicXref(&xref),
            &plan,
        );
        if admitted {
            let batch = outcome.unwrap_or_else(|refusal| {
                panic!("tail {tail:?} must admit, got refusal {refusal:?}")
            });
            assert_eq!(batch.fresh_objects[0].body_bytes, FORM_BODY.as_bytes());
        } else {
            assert!(
                matches!(
                    outcome,
                    Err(CloneSetExportRefusal::UncompressedMemberNotSingleValue { member })
                        if member == reference(4, 0)
                ),
                "tail {tail:?} must refuse the single-value admission",
            );
        }
    }
}

#[test]
fn second_value_members_refuse_the_whole_batch_via_the_real_plan() {
    // A dictionary, scalar, or post-`endstream` second value in any reached
    // member suppresses the ENTIRE staged batch, visible in the counters.
    for member_body in ["12 34", "<< /K 1 >> 5", "[1 2] (x)"] {
        let source = document(
            "3 0 R",
            1,
            &[page("/F1 4 0 R"), form(" /Aux 5 0 R"), in_use(member_body)],
        );
        let (plan, outcome) = stage_with_empty_consumers(&source);
        assert!(
            matches!(
                export_refusal(&outcome),
                CloneSetExportRefusal::UncompressedMemberNotSingleValue { member }
                    if *member == reference(5, 0)
            ),
            "member body {member_body:?} must refuse",
        );
        for counts in all_page_counts(&plan, &source) {
            assert_eq!(counts.export_refused_sets, 1);
            assert_eq!(counts.staged_sets, 0);
        }
    }
}

#[test]
fn malformed_digit_led_scalar_member_refuses_the_whole_batch() {
    // `12foo` parses as neither a reference nor a PDF number: a classified
    // NumberLike body must satisfy the number grammar exactly (the same
    // lexeme rule as compressed-member admission), or the whole staged
    // batch is suppressed.
    let source = document(
        "3 0 R",
        1,
        &[page("/F1 4 0 R"), form(" /Aux 5 0 R"), in_use("12foo")],
    );
    let (plan, outcome) = stage_with_empty_consumers(&source);
    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::UncompressedMemberNotSingleValue { member }
            if *member == reference(5, 0)
    ));
    for counts in all_page_counts(&plan, &source) {
        assert_eq!(counts.export_refused_sets, 1);
        assert_eq!(counts.staged_sets, 0);
    }
}

#[test]
fn value_after_endstream_refuses_the_stream_member() {
    let source = document(
        "3 0 R",
        1,
        &[
            page("/F1 4 0 R"),
            in_use(
                "<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length 0 >>\nstream\n\nendstream true",
            ),
        ],
    );
    let (_, outcome) = stage_with_empty_consumers(&source);
    assert!(matches!(
        export_refusal(&outcome),
        CloneSetExportRefusal::UncompressedMemberNotSingleValue { member }
            if *member == reference(4, 0)
    ));
}

// ---------------------------------------------------------------------------
// Page-leaf retarget proof: refusal paths before reservation.
// ---------------------------------------------------------------------------

#[test]
fn duplicated_leaf_reference_refuses_every_set_before_walking() {
    let source = document("3 0 R 3 0 R", 2, &[page("/F1 4 0 R"), form("")]);
    let (plan, outcome) = stage_with_empty_consumers(&source);

    assert_eq!(plan.sets.len(), 2);
    for set in &plan.sets {
        assert!(matches!(
            set_refusal(set),
            CloneSetRefusal::PageRetargetRefused {
                refusal: PageRetargetProofRefusal::LeafNotUnique { occurrences: 2 },
            }
        ));
        assert!(set.page_ownership.is_none());
        assert_eq!(set.budget.unique_members, 0, "the closure never walked");
    }
    assert!(staged_fresh(outcome).is_empty());
}

#[test]
fn leaf_multiplicity_counts_unselected_binding_report_pages() {
    // Only ordinal 0 is selected, but the duplicated leaf at unselected
    // ordinal 1 still makes the retarget unsafe.
    let source = document("3 0 R 3 0 R", 2, &[page("/F1 4 0 R"), form("")]);
    let plan = build_plan_with_selected(&source, &[0]);

    assert_eq!(plan.sets.len(), 1);
    assert!(matches!(
        set_refusal(&plan.sets[0]),
        CloneSetRefusal::PageRetargetRefused {
            refusal: PageRetargetProofRefusal::LeafNotUnique { occurrences: 2 },
        }
    ));
}

#[test]
fn parent_refusal_matrix_refuses_before_reservation() {
    // Missing /Parent; a decoded-duplicate escaped spelling; a non-reference
    // value: each refuses the page's sets with the parent proof class.
    for page_body in [
        "<< /Type /Page /MediaBox [0 0 100 100] /Resources << /XObject << /F1 4 0 R >> >> >>",
        "<< /Type /Page /Parent 2 0 R /P#61rent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /F1 4 0 R >> >> >>",
        "<< /Type /Page /Parent 2 /MediaBox [0 0 100 100] /Resources << /XObject << /F1 4 0 R >> >> >>",
    ] {
        let source = document("3 0 R", 1, &[in_use(page_body), form("")]);
        let (plan, outcome) = stage_with_empty_consumers(&source);
        assert_eq!(plan.sets.len(), 1, "page body {page_body:?} must seed");
        assert!(
            matches!(
                set_refusal(&plan.sets[0]),
                CloneSetRefusal::PageRetargetRefused {
                    refusal: PageRetargetProofRefusal::ParentNotSingleReference,
                }
            ),
            "page body {page_body:?} must refuse the parent proof",
        );
        assert!(staged_fresh(outcome).is_empty());
        for counts in all_page_counts(&plan, &source) {
            assert_eq!(counts.candidate_sets, 1);
            assert_eq!(counts.refused_sets, 1);
            assert_eq!(counts.planned_sets, 0);
        }
    }
}

#[test]
fn proven_pages_retain_the_page_ownership_decision() {
    let source = shared_form_document();
    let (plan, _) = stage(&source);

    assert_eq!(plan.sets.len(), 2);
    for set in &plan.sets {
        let decision = set.page_ownership.as_ref().expect("proof retained");
        assert_eq!(decision.target, set.page.reference);
        assert_eq!(
            decision.disposition,
            IndirectObjectEditDisposition::InPlaceMutation
        );
        assert!(matches!(
            &decision.ownership,
            IndirectObjectOwnership::ProvenSingleUse { owner } if *owner == reference(2, 0)
        ));
    }
}

// ---------------------------------------------------------------------------
// Commit batch materialization over real plans.
// ---------------------------------------------------------------------------

#[test]
fn shared_root_commits_page_specific_retargets_in_plan_order() {
    let source = shared_form_document();
    let (_, batch) = commit(&source);

    assert_eq!(batch.fresh_objects.len(), 2);
    assert_eq!(batch.sets.len(), 2);
    assert_eq!(batch.page_retargets.len(), 2);

    // Deterministic plan order and page-specific fresh identities for the
    // one shared source root.
    assert_eq!(batch.sets[0].page.ordinal, 0);
    assert_eq!(batch.sets[1].page.ordinal, 1);
    assert_eq!(batch.sets[0].root, reference(5, 0));
    assert_eq!(batch.sets[1].root, reference(5, 0));
    assert_ne!(batch.sets[0].fresh_root, batch.sets[1].fresh_root);
    assert_eq!(batch.sets[0].fresh_root, batch.fresh_objects[0].reference);
    assert_eq!(batch.sets[1].fresh_root, batch.fresh_objects[1].reference);

    for (index, (page_number, name)) in [(3u32, "F1"), (4u32, "F2")].iter().enumerate() {
        let retarget = &batch.page_retargets[index];
        assert_eq!(retarget.reference, reference(*page_number, 0));
        // The page dictionary is copied once; every non-target byte stays
        // verbatim and only the old reference value becomes the mapped
        // generation-zero fresh identity.
        let expected_body = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /{name} {} 0 R >> >> >>",
            batch.sets[index].fresh_root.object_number,
        );
        assert_eq!(retarget.body_bytes, expected_body.as_bytes());

        assert_eq!(retarget.boundaries.len(), 1);
        let MutationBoundary::DictionaryEntry {
            target,
            key,
            op,
            value_locator,
            ownership,
            value_provenance,
        } = &retarget.boundaries[0]
        else {
            panic!("expected a dictionary-entry boundary");
        };
        assert_eq!(*target, reference(*page_number, 0));
        assert_eq!(key.0, name.as_bytes().to_vec());
        assert_eq!(*op, DictionaryEntryOp::Replace);
        assert!(matches!(
            value_locator,
            DictionaryValueLocator::ExistingValue { .. }
        ));
        assert_eq!(
            ownership.disposition,
            IndirectObjectEditDisposition::InPlaceMutation
        );
        assert!(matches!(
            value_provenance,
            PlannedValueProvenance::DerivedFromObject { object } if *object == reference(5, 0)
        ));
    }
}

#[test]
fn several_roots_and_sites_on_one_page_become_one_dirty_page_object() {
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            page("/A 5 0 R /B 5 0 R /C 6 0 R"),
            page("/D 5 0 R /E 6 0 R"),
            form(""),
            form(""),
        ],
    );
    let (_, batch) = commit(&source);

    // Plan order: (page 0, root 5), (page 0, root 6), (page 1, root 5),
    // (page 1, root 6) — but only TWO dirty page objects.
    assert_eq!(batch.sets.len(), 4);
    assert_eq!(batch.page_retargets.len(), 2);
    assert_eq!(
        batch
            .sets
            .iter()
            .map(|set| (set.page.ordinal, set.root, set.retarget_sites))
            .collect::<Vec<_>>(),
        vec![
            (0, reference(5, 0), 2),
            (0, reference(6, 0), 1),
            (1, reference(5, 0), 1),
            (1, reference(6, 0), 1),
        ],
    );

    let first = &batch.page_retargets[0];
    assert_eq!(first.reference, reference(3, 0));
    assert_eq!(first.boundaries.len(), 3, "one boundary per site");
    let root5_fresh = batch.sets[0].fresh_root.object_number;
    let root6_fresh = batch.sets[1].fresh_root.object_number;
    let expected_body = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /A {root5_fresh} 0 R /B {root5_fresh} 0 R /C {root6_fresh} 0 R >> >> >>",
    );
    assert_eq!(first.body_bytes, expected_body.as_bytes());

    let second = &batch.page_retargets[1];
    assert_eq!(second.reference, reference(4, 0));
    assert_eq!(second.boundaries.len(), 2);
    let root5_page1 = batch.sets[2].fresh_root.object_number;
    let root6_page1 = batch.sets[3].fresh_root.object_number;
    let expected_body = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Resources << /XObject << /D {root5_page1} 0 R /E {root6_page1} 0 R >> >> >>",
    );
    assert_eq!(second.body_bytes, expected_body.as_bytes());
}

// ---------------------------------------------------------------------------
// Commit corroboration matrix and request-wide suppression.
// ---------------------------------------------------------------------------

#[test]
fn commit_corroboration_matrix_refuses_every_inconsistency() {
    let source = shared_form_document();
    let staged_pair = || {
        let (plan, outcome) = stage(&source);
        (plan, staged_fresh(outcome))
    };

    // Staged batch misaligned with the planned member total.
    let (plan, mut staged) = staged_pair();
    staged.pop();
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::StagedBatchMisaligned {
            expected: 2,
            found: 1,
        })
    ));

    // Missing page ownership proof.
    let (mut plan, staged) = staged_pair();
    plan.sets[0].page_ownership = None;
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::PageOwnershipMissing { page }) if page == reference(3, 0)
    ));

    // No retarget sites on a planned set.
    let (mut plan, staged) = staged_pair();
    plan.sets[0].retarget_sites.clear();
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::NoRetargetSites { root }) if root == reference(5, 0)
    ));

    // Root absent from the set-local source-to-fresh pairs.
    let (mut plan, staged) = staged_pair();
    let CloneSetOutcome::Planned {
        source_to_fresh, ..
    } = &mut plan.sets[0].outcome
    else {
        unreachable!()
    };
    source_to_fresh[0].0 = reference(9, 0);
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::RootMappingInvalid { root }) if root == reference(5, 0)
    ));

    // The mapped fresh root carries a nonzero generation.
    let (mut plan, staged) = staged_pair();
    let CloneSetOutcome::Planned {
        source_to_fresh, ..
    } = &mut plan.sets[0].outcome
    else {
        unreachable!()
    };
    source_to_fresh[0].1.generation = 1;
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::FreshRootNotGenerationZero { .. })
    ));

    // Page re-resolution mismatch at a stale retained offset.
    let (mut plan, staged) = staged_pair();
    plan.sets[0].page.object_byte_offset += 1;
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::PageResolutionMismatch { page }) if page == reference(3, 0)
    ));

    // Site ranges outside the re-inspected page dictionary.
    let (mut plan, staged) = staged_pair();
    plan.sets[0].retarget_sites[0].value_range.end = source.len();
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::SiteOutsideDictionary { page }) if page == reference(3, 0)
    ));

    // Site key bytes no longer spell the retained name.
    let (mut plan, staged) = staged_pair();
    plan.sets[0].retarget_sites[0].name = presslint_pdf::PdfName(b"Zz".to_vec());
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::SiteKeyMismatch { page }) if page == reference(3, 0)
    ));

    // Site value no longer parses as the expected old target.
    let (mut plan, staged) = staged_pair();
    plan.sets[0].retarget_sites[0].expected_target = reference(9, 0);
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::SiteValueMismatch { page }) if page == reference(3, 0)
    ));

    // Duplicate site on one page.
    let (mut plan, staged) = staged_pair();
    let site = plan.sets[0].retarget_sites[0].clone();
    plan.sets[0].retarget_sites.push(site);
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::SiteOverlap { page }) if page == reference(3, 0)
    ));

    // Two distinct page-identity groups anchoring one page object.
    let (mut plan, staged) = staged_pair();
    plan.sets[1].page = CloneSetPageIdentity {
        ordinal: 1,
        ..plan.sets[0].page
    };
    plan.sets[1].page_ownership = plan.sets[0].page_ownership.clone();
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::PageIdentityCollision { page }) if page == reference(3, 0)
    ));
}

#[test]
fn site_bound_to_a_non_root_target_refuses() {
    // Move a root-6 site into the root-5 set: the value parses exactly as
    // its expected target (6 0 R), but that target is not the set root.
    let source = document(
        "3 0 R 4 0 R",
        2,
        &[
            page("/A 5 0 R /B 6 0 R"),
            page("/C 5 0 R /D 6 0 R"),
            form(""),
            form(""),
        ],
    );
    let (mut plan, outcome) = stage(&source);
    let staged = staged_fresh(outcome);
    let foreign_site = plan.sets[1].retarget_sites[0].clone();
    plan.sets[0].retarget_sites[0] = foreign_site;
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::SiteTargetNotRoot { root }) if root == reference(5, 0)
    ));
}

#[test]
fn one_failing_page_suppresses_the_whole_commit_including_ready_pages() {
    let source = shared_form_document();
    let (mut plan, outcome) = stage(&source);
    let staged = staged_fresh(outcome);
    // Page 0's set is fully consistent; page 1's tampered site still
    // discards the ENTIRE batch — no prefix salvage.
    plan.sets[1].retarget_sites[0].expected_target = reference(9, 0);
    assert!(matches!(
        build_clone_commit_batch(&source, &plan, staged),
        Err(CloneCommitRefusal::SiteValueMismatch { page }) if page == reference(4, 0)
    ));
}

// ---------------------------------------------------------------------------
// Doctrine guard: the built-and-dropped batch changes no emitted byte.
// ---------------------------------------------------------------------------

#[test]
fn commit_batch_leaves_emitted_bytes_byte_identical_and_unrevised() {
    // No convertible colour operator: the pre-commit pipeline emitted exactly
    // the empty incremental append. With the commit batch built and dropped
    // in production, the emitted bytes must stay BYTE-IDENTICAL, and neither
    // a fresh header nor a page-retarget revision may appear in the tail.
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

    let baseline =
        write_incremental_revision(&source, &[]).expect("baseline empty append succeeds");
    assert_eq!(output.bytes, baseline);

    // Rebuild the same batch the pipeline built and dropped: it must be a
    // real, fully validated transaction...
    let (_, batch) = commit(&source);
    assert_eq!(batch.fresh_objects.len(), 2);
    assert_eq!(batch.page_retargets.len(), 2);
    // ...and nothing from it may be emitted before the activation slice.
    let tail = &output.bytes[source.len()..];
    for fresh in &batch.fresh_objects {
        let header = format!("{} 0 obj", fresh.reference.object_number);
        assert!(!contains(tail, header.as_bytes()));
    }
    for retarget in &batch.page_retargets {
        let header = format!("{} 0 obj", retarget.reference.object_number);
        assert!(!contains(tail, header.as_bytes()));
    }
}
