//! Behavior of the plan-to-writer bridge `write_incremental_revision_plan`.
//!
//! These cover plan-layer validation order (rejections that must happen before
//! the byte writer is reached), the executable single-object dictionary path,
//! and the delegated-writer error wrapping.

use presslint_actions::{
    DictionaryEntryOp, DictionaryValueLocator, IncrementalRevisionPlan, MutationBoundary,
    PlannedDirtyObject, PlannedObjectAllocation, PlannedValueProvenance,
};
use presslint_pdf::{
    IndirectObjectEditDecision, IndirectObjectEditDisposition, IndirectObjectOwnership, IndirectRef,
};
use presslint_types::{ByteRange, ContentScope, PageIndex, PdfName};

use crate::{
    PlannedWriteError, UnsupportedBoundaryKind, WriteError, write_incremental_revision_plan,
};

use super::{
    PAGE_BODY, page_leaf_numbers, reopen, sample_pdf, sample_xref_stream_pdf, xref_stream_chain,
};

fn iref(object_number: u32) -> IndirectRef {
    IndirectRef {
        object_number,
        generation: 0,
    }
}

/// Proven single-use, in-place-mutation ownership for `target`.
fn in_place(target: IndirectRef) -> IndirectObjectEditDecision {
    IndirectObjectEditDecision {
        target,
        ownership: IndirectObjectOwnership::ProvenSingleUse { owner: iref(2) },
        disposition: IndirectObjectEditDisposition::InPlaceMutation,
    }
}

/// Shared, private-copy ownership for `target` (disposition is not in-place).
fn private_copy(target: IndirectRef) -> IndirectObjectEditDecision {
    IndirectObjectEditDecision {
        target,
        ownership: IndirectObjectOwnership::Shared {
            consumers: vec![iref(2), iref(5)],
        },
        disposition: IndirectObjectEditDisposition::PrivateCopy,
    }
}

/// A `/MediaBox` replace dictionary-entry boundary for `target`.
fn media_entry(target: IndirectRef, ownership: IndirectObjectEditDecision) -> MutationBoundary {
    MutationBoundary::DictionaryEntry {
        target,
        key: PdfName(b"MediaBox".to_vec()),
        op: DictionaryEntryOp::Replace,
        value_locator: DictionaryValueLocator::ExistingValue {
            key_range: ByteRange { start: 20, end: 29 },
            value_range: ByteRange { start: 30, end: 52 },
        },
        ownership,
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: target },
    }
}

/// A whole-stream replacement boundary for `target`.
fn whole_stream(target: IndirectRef, ownership: IndirectObjectEditDecision) -> MutationBoundary {
    MutationBoundary::WholeStream {
        target,
        stream_data_range: Some(ByteRange { start: 40, end: 52 }),
        ownership,
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: target },
    }
}

/// A `/CropBox` insert dictionary-entry boundary for `target`.
fn crop_entry(target: IndirectRef, ownership: IndirectObjectEditDecision) -> MutationBoundary {
    MutationBoundary::DictionaryEntry {
        target,
        key: PdfName(b"CropBox".to_vec()),
        op: DictionaryEntryOp::Insert,
        value_locator: DictionaryValueLocator::InsertionPoint {
            dictionary_range: ByteRange { start: 8, end: 60 },
        },
        ownership,
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: target },
    }
}

fn one_object_plan(
    reference: IndirectRef,
    boundaries: Vec<MutationBoundary>,
    body: &[u8],
) -> IncrementalRevisionPlan {
    IncrementalRevisionPlan {
        dirty_objects: vec![PlannedDirtyObject {
            reference,
            boundaries,
            body_bytes: body.to_vec(),
        }],
    }
}

fn count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#[test]
fn multi_boundary_single_object_writes_one_dirty_object_and_preserves_prefix() {
    let input = sample_pdf();
    let obj3 = iref(3);
    // Two dictionary-entry boundaries against the same object; the body is the
    // existing page body (currency-valid) so the delegated writer accepts it.
    let plan = one_object_plan(
        obj3,
        vec![
            media_entry(obj3, in_place(obj3)),
            crop_entry(obj3, in_place(obj3)),
        ],
        PAGE_BODY,
    );

    let output = write_incremental_revision_plan(&input, &plan).expect("plan writes");

    assert_eq!(&output[..input.len()], input.as_slice());
    let appended = &output[input.len()..];
    // Exactly one appended object header for the single dirty object.
    assert_eq!(count(appended, b"3 0 obj"), 1);

    let access = reopen(&output);
    assert_eq!(page_leaf_numbers(&access), vec![3]);
}

#[test]
fn ownership_not_in_place_rejects_before_delegating() {
    // A non-existent object number: if validation reached the byte writer it
    // would report `DirtyObjectNotInUse`. Instead the plan layer rejects on the
    // ownership disposition, proving validation precedes delegation.
    let obj99 = iref(99);
    let plan = one_object_plan(
        obj99,
        vec![media_entry(obj99, private_copy(obj99))],
        PAGE_BODY,
    );

    let error = write_incremental_revision_plan(&sample_pdf(), &plan).unwrap_err();

    assert_eq!(
        error,
        PlannedWriteError::OwnershipNotInPlace {
            reference: obj99,
            disposition: IndirectObjectEditDisposition::PrivateCopy,
        }
    );
}

#[test]
fn boundary_target_mismatch_rejects_before_delegating() {
    // Non-existent dirty object again; the boundary names a different target.
    let obj99 = iref(99);
    let obj3 = iref(3);
    let plan = one_object_plan(obj99, vec![media_entry(obj3, in_place(obj3))], PAGE_BODY);

    let error = write_incremental_revision_plan(&sample_pdf(), &plan).unwrap_err();

    assert_eq!(
        error,
        PlannedWriteError::BoundaryTargetMismatch {
            reference: obj99,
            boundary_target: obj3,
        }
    );
}

#[test]
fn duplicate_dirty_object_numbers_reject_at_plan_layer() {
    let obj3 = iref(3);
    let plan = IncrementalRevisionPlan {
        dirty_objects: vec![
            PlannedDirtyObject {
                reference: obj3,
                boundaries: vec![media_entry(obj3, in_place(obj3))],
                body_bytes: PAGE_BODY.to_vec(),
            },
            PlannedDirtyObject {
                reference: obj3,
                boundaries: vec![crop_entry(obj3, in_place(obj3))],
                body_bytes: PAGE_BODY.to_vec(),
            },
        ],
    };

    let error = write_incremental_revision_plan(&sample_pdf(), &plan).unwrap_err();

    assert_eq!(
        error,
        PlannedWriteError::DuplicateDirtyObject { object_number: 3 }
    );
}

#[test]
fn empty_plan_and_empty_boundaries_reject_at_plan_layer() {
    let empty_plan = IncrementalRevisionPlan {
        dirty_objects: Vec::new(),
    };
    assert_eq!(
        write_incremental_revision_plan(&sample_pdf(), &empty_plan).unwrap_err(),
        PlannedWriteError::EmptyPlan
    );

    let obj3 = iref(3);
    let empty_boundaries = one_object_plan(obj3, Vec::new(), PAGE_BODY);
    assert_eq!(
        write_incremental_revision_plan(&sample_pdf(), &empty_boundaries).unwrap_err(),
        PlannedWriteError::EmptyBoundaries { reference: obj3 }
    );
}

#[test]
fn whole_stream_boundary_is_accepted_and_writes_one_dirty_object() {
    let input = sample_pdf();
    let obj3 = iref(3);
    // A WholeStream boundary against object 3 with the existing (currency-valid)
    // body: the bridge now accepts WholeStream the same way it accepts
    // DictionaryEntry, so the delegated writer appends one dirty object.
    let plan = one_object_plan(obj3, vec![whole_stream(obj3, in_place(obj3))], PAGE_BODY);

    let output = write_incremental_revision_plan(&input, &plan).expect("plan writes");

    assert_eq!(&output[..input.len()], input.as_slice());
    assert_eq!(count(&output[input.len()..], b"3 0 obj"), 1);
    assert_eq!(page_leaf_numbers(&reopen(&output)), vec![3]);
}

#[test]
fn whole_stream_boundary_target_and_ownership_are_validated() {
    let obj3 = iref(3);
    let obj99 = iref(99);

    // Target mismatch: the WholeStream boundary names a different object.
    assert_eq!(
        write_incremental_revision_plan(
            &sample_pdf(),
            &one_object_plan(obj99, vec![whole_stream(obj3, in_place(obj3))], PAGE_BODY)
        )
        .unwrap_err(),
        PlannedWriteError::BoundaryTargetMismatch {
            reference: obj99,
            boundary_target: obj3,
        }
    );

    // Ownership not in-place: a WholeStream with a private-copy disposition is
    // rejected before delegation.
    assert_eq!(
        write_incremental_revision_plan(
            &sample_pdf(),
            &one_object_plan(
                obj99,
                vec![whole_stream(obj99, private_copy(obj99))],
                PAGE_BODY
            )
        )
        .unwrap_err(),
        PlannedWriteError::OwnershipNotInPlace {
            reference: obj99,
            disposition: IndirectObjectEditDisposition::PrivateCopy,
        }
    );
}

#[test]
fn unsupported_boundary_kinds_reject_explicitly() {
    let obj3 = iref(3);

    let content = MutationBoundary::ContentStreamOperand {
        page: PageIndex(0),
        scope: ContentScope::Page,
        record_range: ByteRange { start: 0, end: 4 },
        operand_range: None,
        operator_range: None,
        ownership: None,
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: obj3 },
    };
    assert_eq!(
        write_incremental_revision_plan(
            &sample_pdf(),
            &one_object_plan(obj3, vec![content], PAGE_BODY)
        )
        .unwrap_err(),
        PlannedWriteError::UnsupportedBoundaryKind {
            reference: obj3,
            kind: UnsupportedBoundaryKind::ContentStreamOperand,
        }
    );

    let clone = MutationBoundary::IndirectObjectClone {
        source: obj3,
        consumer: iref(2),
        new_object: PlannedObjectAllocation::Deferred,
        reference_patch: Box::new(media_entry(iref(2), in_place(iref(2)))),
        ownership: in_place(obj3),
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: obj3 },
    };
    assert_eq!(
        write_incremental_revision_plan(
            &sample_pdf(),
            &one_object_plan(obj3, vec![clone], PAGE_BODY)
        )
        .unwrap_err(),
        PlannedWriteError::UnsupportedBoundaryKind {
            reference: obj3,
            kind: UnsupportedBoundaryKind::IndirectObjectClone,
        }
    );
}

#[test]
fn delegated_writer_error_is_wrapped() {
    // A valid plan over a non-PDF input: plan validation passes, so the delegated
    // byte writer's source-inspection failure surfaces as `Write`.
    let obj3 = iref(3);
    let plan = one_object_plan(obj3, vec![media_entry(obj3, in_place(obj3))], PAGE_BODY);

    let error = write_incremental_revision_plan(b"definitely not a pdf", &plan).unwrap_err();

    assert!(matches!(
        error,
        PlannedWriteError::Write { error } if matches!(*error, WriteError::Source { .. })
    ));
}

#[test]
fn plan_bridge_writes_xref_stream_input() {
    let input = sample_xref_stream_pdf();
    let obj3 = iref(3);
    let plan = one_object_plan(obj3, vec![media_entry(obj3, in_place(obj3))], PAGE_BODY);

    let output = write_incremental_revision_plan(&input, &plan).expect("plan writes");

    assert_eq!(&output[..input.len()], input.as_slice());
    let access = reopen(&output);
    assert_eq!(xref_stream_chain(&access).section_byte_offsets.len(), 2);
    assert_eq!(page_leaf_numbers(&access), vec![3]);
}
