#![allow(clippy::expect_used, clippy::missing_errors_doc)]

mod array_extent;
mod catalog_pages;
mod classic_xref;
mod dictionary_entries;
mod dictionary_extent;
mod indirect_reference;
mod object_body;
mod object_dictionary;
mod object_header;
mod page_contents;
mod page_tree_kids;
mod page_tree_node;
mod page_tree_node_type;
mod page_tree_reference;
mod source;
mod trailer;
mod trailer_root;

use super::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefEntry, ClassicXrefEntryState,
    ClassicXrefObjectLocation, ClassicXrefSubsection, ClassicXrefTableInspection,
    IndirectObjectEditDisposition, IndirectObjectOwnership, IndirectRef,
    decide_indirect_object_edit, resolve_classic_xref_object,
};

fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

fn classic_entry(
    object_number: u32,
    generation: u16,
    byte_offset: usize,
    state: ClassicXrefEntryState,
) -> ClassicXrefEntry {
    ClassicXrefEntry {
        object_number,
        generation,
        byte_offset,
        state,
    }
}

fn classic_subsection(
    first_object_number: u32,
    entries: Vec<ClassicXrefEntry>,
) -> ClassicXrefSubsection {
    ClassicXrefSubsection {
        first_object_number,
        entry_count: entries
            .len()
            .try_into()
            .expect("test subsection length fits u32"),
        entries,
    }
}

fn classic_inspection(subsections: Vec<ClassicXrefSubsection>) -> ClassicXrefTableInspection {
    ClassicXrefTableInspection {
        table_byte_offset: 0,
        subsections,
        trailer_byte_offset: 0,
    }
}

#[test]
fn one_proven_consumer_allows_in_place_mutation() {
    let target = indirect_ref(10, 0);
    let owner = indirect_ref(2, 0);

    let decision = decide_indirect_object_edit(target, [owner]);

    assert_eq!(decision.target, target);
    assert_eq!(
        decision.ownership,
        IndirectObjectOwnership::ProvenSingleUse { owner }
    );
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::InPlaceMutation
    );
}

#[test]
fn multiple_proven_consumers_require_private_copy() {
    let target = indirect_ref(10, 0);
    let first = indirect_ref(2, 0);
    let second = indirect_ref(3, 0);

    let decision = decide_indirect_object_edit(target, [first, second]);

    assert_eq!(
        decision.ownership,
        IndirectObjectOwnership::Shared {
            consumers: vec![first, second],
        }
    );
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::PrivateCopy
    );
}

#[test]
fn no_proven_consumers_require_private_copy() {
    let target = indirect_ref(10, 0);

    let decision = decide_indirect_object_edit(target, []);

    assert_eq!(decision.ownership, IndirectObjectOwnership::Unproven);
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::PrivateCopy
    );
}

#[test]
fn shared_consumer_refs_are_reported_deterministically() {
    let target = indirect_ref(10, 0);
    let high_generation = indirect_ref(2, 1);
    let lowest = indirect_ref(1, 0);
    let low_generation = indirect_ref(2, 0);

    let decision =
        decide_indirect_object_edit(target, [high_generation, lowest, low_generation, lowest]);

    assert_eq!(
        decision.ownership,
        IndirectObjectOwnership::Shared {
            consumers: vec![lowest, low_generation, high_generation],
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_single_subsection_in_use_hit() {
    let inspection = classic_inspection(vec![classic_subsection(
        0,
        vec![
            classic_entry(0, 65535, 0, ClassicXrefEntryState::Free),
            classic_entry(1, 0, 42, ClassicXrefEntryState::InUse),
        ],
    )]);

    let location = resolve_classic_xref_object(&inspection, 1);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::InUse {
            object_number: 1,
            generation: 0,
            byte_offset: 42,
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_multi_subsection_in_use_hit() {
    let inspection = classic_inspection(vec![
        classic_subsection(
            0,
            vec![classic_entry(0, 65535, 0, ClassicXrefEntryState::Free)],
        ),
        classic_subsection(
            10,
            vec![
                classic_entry(10, 0, 100, ClassicXrefEntryState::InUse),
                classic_entry(11, 2, 200, ClassicXrefEntryState::InUse),
            ],
        ),
    ]);

    let location = resolve_classic_xref_object(&inspection, 11);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::InUse {
            object_number: 11,
            generation: 2,
            byte_offset: 200,
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_free_entry() {
    let inspection = classic_inspection(vec![classic_subsection(
        0,
        vec![classic_entry(0, 65535, 7, ClassicXrefEntryState::Free)],
    )]);

    let location = resolve_classic_xref_object(&inspection, 0);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::Free {
            object_number: 0,
            generation: 65535,
            next_free_object_number: 7,
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_not_found() {
    let inspection = classic_inspection(vec![classic_subsection(
        1,
        vec![classic_entry(1, 0, 42, ClassicXrefEntryState::InUse)],
    )]);

    let location = resolve_classic_xref_object(&inspection, 2);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::NotFound { object_number: 2 }
    );
}

#[test]
fn classic_xref_object_resolution_reports_duplicate_object_number_ambiguity() {
    let inspection = classic_inspection(vec![
        classic_subsection(
            5,
            vec![classic_entry(5, 0, 100, ClassicXrefEntryState::InUse)],
        ),
        classic_subsection(
            5,
            vec![classic_entry(5, 1, 200, ClassicXrefEntryState::InUse)],
        ),
    ]);

    let location = resolve_classic_xref_object(&inspection, 5);

    assert_eq!(
        location,
        ClassicXrefObjectLocation::Ambiguous {
            object_number: 5,
            first: ClassicXrefAmbiguousObjectEntry {
                generation: 0,
                byte_offset: 100,
                state: ClassicXrefEntryState::InUse,
            },
            second: ClassicXrefAmbiguousObjectEntry {
                generation: 1,
                byte_offset: 200,
                state: ClassicXrefEntryState::InUse,
            },
        }
    );
}

#[test]
fn classic_xref_object_resolution_reports_lowest_and_highest_subsection_objects() {
    let inspection = classic_inspection(vec![classic_subsection(
        100,
        vec![
            classic_entry(100, 0, 1000, ClassicXrefEntryState::InUse),
            classic_entry(101, 0, 1001, ClassicXrefEntryState::InUse),
            classic_entry(102, 3, 1002, ClassicXrefEntryState::InUse),
        ],
    )]);

    let lowest = resolve_classic_xref_object(&inspection, 100);
    let highest = resolve_classic_xref_object(&inspection, 102);

    assert_eq!(
        lowest,
        ClassicXrefObjectLocation::InUse {
            object_number: 100,
            generation: 0,
            byte_offset: 1000,
        }
    );
    assert_eq!(
        highest,
        ClassicXrefObjectLocation::InUse {
            object_number: 102,
            generation: 3,
            byte_offset: 1002,
        }
    );
}
