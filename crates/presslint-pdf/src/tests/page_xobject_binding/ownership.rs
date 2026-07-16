//! Consumer-index exclusivity veto, incomplete-index fail-closed behavior,
//! and reached-offset identity corroboration.

use super::{CATALOG, fixture_owned, form_object, indirect_ref};
use crate::{
    IndirectObjectOwnership, ObjectConsumerIndexLimit, ObjectConsumerIndexTruncation,
    ObjectConsumerReferrer, ObjectConsumerUnresolvedEdge, ObjectLookupLocation,
    ObjectResolutionRejection, PageXObjectBindingRefusal, PageXObjectBindingUnprovenReason,
    PageXObjectBindingVerdict, SkippedObjectConsumerScan,
};

fn two_page_shared_target() -> super::Fixture {
    fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 5 0 R >> >> >>\nendobj\n"
            .to_vec(),
        b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 5 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(5, ""),
    ])
}

#[test]
fn shared_target_across_two_pages_fails_exclusivity_on_both() {
    let pdf = two_page_shared_target();

    let report = pdf.inspect();

    assert_eq!(report.page_count(), 2);
    for page in &report.pages {
        let witness = &page.witnesses[0];
        assert_eq!(witness.target, indirect_ref(5, 0));
        assert_eq!(witness.target_ownership, IndirectObjectOwnership::Unproven);
        assert_eq!(
            witness.verdict,
            PageXObjectBindingVerdict::Unproven {
                reason: PageXObjectBindingUnprovenReason::TargetConsumersNotExclusive {
                    referrer_count: 2,
                },
            }
        );
    }
}

#[test]
fn per_page_exclusive_targets_pass_the_veto_independently() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 5 0 R >> >> >>\nendobj\n"
            .to_vec(),
        b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 6 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(5, ""),
        form_object(6, ""),
    ]);

    let report = pdf.inspect();

    for (page, target) in report.pages.iter().zip([5u32, 6u32]) {
        let witness = &page.witnesses[0];
        assert_eq!(witness.target, indirect_ref(target, 0));
        assert_eq!(
            witness.target_ownership,
            IndirectObjectOwnership::ProvenSingleUse {
                owner: page.page_reference,
            }
        );
        assert_eq!(witness.verdict, PageXObjectBindingVerdict::ProvenPageLocal);
    }
}

#[test]
fn catalog_key_consumer_defeats_exclusivity() {
    let pdf = fixture_owned(&[
        b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Extra 4 0 R >>\nendobj\n".to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
    ]);

    let report = pdf.inspect();

    let witness = &report.pages[0].witnesses[0];
    assert_eq!(witness.target_ownership, IndirectObjectOwnership::Unproven);
    assert_eq!(
        witness.verdict,
        PageXObjectBindingVerdict::Unproven {
            reason: PageXObjectBindingUnprovenReason::TargetConsumersNotExclusive {
                referrer_count: 2,
            },
        }
    );
}

#[test]
fn any_consumer_index_incompleteness_fails_closed() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
    ]);
    let complete = pdf.consumers();
    assert_eq!(
        pdf.inspect_with(&complete).pages[0].witnesses[0].verdict,
        PageXObjectBindingVerdict::ProvenPageLocal
    );

    let mut truncated = complete.clone();
    truncated.truncations.push(ObjectConsumerIndexTruncation {
        referrer: None,
        target: None,
        limit: ObjectConsumerIndexLimit::MaxRecordedPairs {
            max_recorded_pairs: 0,
        },
    });

    let mut unresolved = complete.clone();
    unresolved
        .unresolved_edges
        .push(ObjectConsumerUnresolvedEdge {
            target: indirect_ref(9, 0),
            referrer: ObjectConsumerReferrer::Root,
            resolution_reason: ObjectResolutionRejection::UnresolvedXrefLocation {
                location: ObjectLookupLocation::ClassicNotFound { object_number: 9 },
            },
        });

    let mut skipped = complete;
    skipped
        .skipped
        .push(SkippedObjectConsumerScan::CatalogDictionary);

    for incomplete in [truncated, unresolved, skipped] {
        let report = pdf.inspect_with(&incomplete);
        let witness = &report.pages[0].witnesses[0];
        assert_eq!(witness.target_ownership, IndirectObjectOwnership::Unproven);
        assert_eq!(
            witness.verdict,
            PageXObjectBindingVerdict::Unproven {
                reason: PageXObjectBindingUnprovenReason::ConsumerIndexIncomplete,
            }
        );
    }
}

#[test]
fn benign_unreferenced_entry_skips_do_not_defeat_completeness() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
    ]);
    let mut consumers = pdf.consumers();
    consumers
        .skipped
        .push(SkippedObjectConsumerScan::UnreferencedEntryUnresolvable { object_number: 9 });

    let report = pdf.inspect_with(&consumers);

    assert_eq!(
        report.pages[0].witnesses[0].verdict,
        PageXObjectBindingVerdict::ProvenPageLocal
    );
}

#[test]
fn unindexed_target_reports_zero_referrers() {
    let pdf = two_page_shared_target();
    let consumers = super::empty_consumers(pdf.source.len());

    let report = pdf.inspect_with(&consumers);

    let witness = &report.pages[0].witnesses[0];
    assert_eq!(witness.target_ownership, IndirectObjectOwnership::Unproven);
    assert_eq!(
        witness.verdict,
        PageXObjectBindingVerdict::Unproven {
            reason: PageXObjectBindingUnprovenReason::TargetConsumersNotExclusive {
                referrer_count: 0,
            },
        }
    );
}

#[test]
fn reached_offset_identity_mismatch_refuses_the_entry() {
    let mut pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
        form_object(5, ""),
    ]);
    let consumers = pdf.consumers();
    // Point object 4's xref entry at object 5's header: the reached offset no
    // longer corroborates the requested identity.
    pdf.xref.subsections[0].entries[4].byte_offset = pdf.object_offset(5);

    let report = pdf.inspect_with(&consumers);

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::EntryTargetIdentityMismatch {
            reference: indirect_ref(4, 0),
            object_byte_offset: pdf.object_offset(5),
            header_reference: indirect_ref(5, 0),
        }
    );
}

#[test]
fn container_identity_mismatch_wins_over_a_malformed_reached_body() {
    // The reached offset holds object 6 with a NON-dictionary body: the
    // header is corroborated BEFORE body validation, so the mismatch is
    // classified as such, never masked as a dictionary failure.
    let mut pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 0 R >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
        b"5 0 obj\n<< /XObject << /Fm0 4 0 R >> >>\nendobj\n".to_vec(),
        b"6 0 obj\n[ 1 2 3 ]\nendobj\n".to_vec(),
    ]);
    let consumers = pdf.consumers();
    pdf.xref.subsections[0].entries[5].byte_offset = pdf.object_offset(6);

    let report = pdf.inspect_with(&consumers);

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::ContainerIdentityMismatch {
            container: crate::BindingContainer::Resources,
            defining_node: indirect_ref(3, 0),
            reference: indirect_ref(5, 0),
            object_byte_offset: pdf.object_offset(6),
            header_reference: indirect_ref(6, 0),
        }
    );
}

#[test]
fn entry_target_identity_mismatch_wins_over_a_malformed_reached_body() {
    let mut pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
        b"5 0 obj\n[ 1 2 3 ]\nendobj\n".to_vec(),
    ]);
    let consumers = pdf.consumers();
    pdf.xref.subsections[0].entries[4].byte_offset = pdf.object_offset(5);

    let report = pdf.inspect_with(&consumers);

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::EntryTargetIdentityMismatch {
            reference: indirect_ref(4, 0),
            object_byte_offset: pdf.object_offset(5),
            header_reference: indirect_ref(5, 0),
        }
    );
}

#[test]
fn container_identity_mismatch_refuses_the_page_bindings() {
    let mut pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 0 R >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
        b"5 0 obj\n<< /XObject << /Fm0 4 0 R >> >>\nendobj\n".to_vec(),
        b"6 0 obj\n<< /XObject << /Fm0 4 0 R >> >>\nendobj\n".to_vec(),
    ]);
    let consumers = pdf.consumers();
    pdf.xref.subsections[0].entries[5].byte_offset = pdf.object_offset(6);

    let report = pdf.inspect_with(&consumers);

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::ContainerIdentityMismatch {
            container: crate::BindingContainer::Resources,
            defining_node: indirect_ref(3, 0),
            reference: indirect_ref(5, 0),
            object_byte_offset: pdf.object_offset(6),
            header_reference: indirect_ref(6, 0),
        }
    );
}
