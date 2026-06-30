#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use super::{classic_entry, classic_inspection, classic_subsection, indirect_ref};

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ClassicXrefEntryState, ClassicXrefObjectLocation, PageTreeKidTargetInspection,
    PageTreeKidTargetsInspection, PageTreeNodeType, PageTreeReferenceTargetInspectionRejection,
    SkippedPageTreeKidKind, inspect_catalog_pages, inspect_classic_xref_table,
    inspect_classic_xref_trailer_root, inspect_page_tree_kid_targets,
    inspect_page_tree_reference_target,
};

#[test]
fn page_tree_kid_targets_resolves_mixed_pages_and_page_targets_in_source_order() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let intermediate = b"3 0 obj\n<< /Type /Pages /Kids [ 5 0 R ] /Count 1 >>\nendobj\n";
    let page = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let intermediate_offset = root.len();
    let page_offset = root.len() + intermediate.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(intermediate);
    source.extend_from_slice(page);
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![
            classic_entry(2, 0, 0, ClassicXrefEntryState::InUse),
            classic_entry(3, 0, intermediate_offset, ClassicXrefEntryState::InUse),
            classic_entry(4, 0, page_offset, ClassicXrefEntryState::InUse),
        ],
    )]);

    let report =
        inspect_page_tree_kid_targets(&source, &xref, 0).expect("kid targets should inspect");

    assert_eq!(report.byte_len, source.len());
    assert_eq!(
        report
            .kids
            .kids
            .iter()
            .map(|kid| kid.reference)
            .collect::<Vec<_>>(),
        vec![indirect_ref(3, 0), indirect_ref(4, 0)]
    );
    assert_eq!(report.entries.len(), 2);
    assert_eq!(report.resolved_count(), 2);

    assert!(matches!(
        &report.entries[0],
        PageTreeKidTargetInspection::Resolved { .. }
    ));
    if let PageTreeKidTargetInspection::Resolved {
        kid: first_kid,
        target: first_target,
    } = &report.entries[0]
    {
        assert_eq!(first_kid.reference, indirect_ref(3, 0));
        assert_eq!(first_target.object_byte_offset, intermediate_offset);
        assert_eq!(first_target.node_type.node_type, PageTreeNodeType::Pages);
    }

    assert!(matches!(
        &report.entries[1],
        PageTreeKidTargetInspection::Resolved { .. }
    ));
    if let PageTreeKidTargetInspection::Resolved {
        kid: second_kid,
        target: second_target,
    } = &report.entries[1]
    {
        assert_eq!(second_kid.reference, indirect_ref(4, 0));
        assert_eq!(second_target.object_byte_offset, page_offset);
        assert_eq!(second_target.node_type.node_type, PageTreeNodeType::Page);
    }
}

#[test]
fn page_tree_kid_targets_reports_failed_child_then_continues_to_successful_child() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 99 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let page = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_offset = root.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(page);
    let xref = classic_inspection(vec![classic_subsection(
        4,
        vec![classic_entry(
            4,
            0,
            page_offset,
            ClassicXrefEntryState::InUse,
        )],
    )]);

    let report =
        inspect_page_tree_kid_targets(&source, &xref, 0).expect("kid targets should inspect");

    assert_eq!(report.entries.len(), 2);
    assert_eq!(report.resolved_count(), 1);
    assert!(matches!(
        &report.entries[0],
        PageTreeKidTargetInspection::Failed { .. }
    ));
    if let PageTreeKidTargetInspection::Failed { kid, error } = &report.entries[0] {
        assert_eq!(kid.reference, indirect_ref(99, 0));
        assert_eq!(error.reference, indirect_ref(99, 0));
        assert_eq!(
            error.reason,
            PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
                location: ClassicXrefObjectLocation::NotFound { object_number: 99 },
            }
        );
    }

    assert!(matches!(
        &report.entries[1],
        PageTreeKidTargetInspection::Resolved { .. }
    ));
    if let PageTreeKidTargetInspection::Resolved { kid, target } = &report.entries[1] {
        assert_eq!(kid.reference, indirect_ref(4, 0));
        assert_eq!(target.object_byte_offset, page_offset);
        assert_eq!(target.node_type.node_type, PageTreeNodeType::Page);
    }
}

#[test]
fn page_tree_kid_targets_preserves_delegated_skipped_kids_without_target_entries() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ /Name 3 0 R [ 4 0 R ] ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_offset = root.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(page);
    let xref = classic_inspection(vec![classic_subsection(
        3,
        vec![classic_entry(
            3,
            0,
            page_offset,
            ClassicXrefEntryState::InUse,
        )],
    )]);

    let report =
        inspect_page_tree_kid_targets(&source, &xref, 0).expect("kid targets should inspect");

    assert_eq!(report.kids.kids.len(), 1);
    assert_eq!(report.entries.len(), 1);
    assert_eq!(
        report
            .kids
            .skipped
            .iter()
            .map(|skip| skip.kind)
            .collect::<Vec<_>>(),
        vec![SkippedPageTreeKidKind::Name, SkippedPageTreeKidKind::Array]
    );
    assert!(matches!(
        &report.entries[0],
        PageTreeKidTargetInspection::Resolved { .. }
    ));
    if let PageTreeKidTargetInspection::Resolved { kid, target } = &report.entries[0] {
        assert_eq!(kid.reference, indirect_ref(3, 0));
        assert_eq!(target.node_type.node_type, PageTreeNodeType::Page);
    }
}

#[test]
fn page_tree_kid_targets_resolved_count_reports_only_successful_targets() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 99 0 R 4 0 R 100 0 R ] /Count 3 >>\nendobj\n";
    let page = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_offset = root.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(page);
    let xref = classic_inspection(vec![classic_subsection(
        4,
        vec![classic_entry(
            4,
            0,
            page_offset,
            ClassicXrefEntryState::InUse,
        )],
    )]);

    let report =
        inspect_page_tree_kid_targets(&source, &xref, 0).expect("kid targets should inspect");

    assert_eq!(report.entries.len(), 3);
    assert_eq!(report.resolved_count(), 1);
}

#[test]
fn page_tree_kid_targets_serde_round_trips_aggregate_report_and_failed_entry() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 99 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let page = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_offset = root.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(page);
    let xref = classic_inspection(vec![classic_subsection(
        4,
        vec![classic_entry(
            4,
            0,
            page_offset,
            ClassicXrefEntryState::InUse,
        )],
    )]);

    let report =
        inspect_page_tree_kid_targets(&source, &xref, 0).expect("kid targets should inspect");

    let value = serde_value(&report).expect("aggregate report should serialize");
    let restored: PageTreeKidTargetsInspection =
        from_serde_value(value).expect("aggregate report should deserialize");
    assert_eq!(restored, report);

    let failed_entry = report.entries[0].clone();
    assert!(matches!(
        failed_entry,
        PageTreeKidTargetInspection::Failed { .. }
    ));
    let entry_value = serde_value(&failed_entry).expect("failed entry should serialize");
    let restored_entry: PageTreeKidTargetInspection =
        from_serde_value(entry_value).expect("failed entry should deserialize");
    assert_eq!(restored_entry, failed_entry);
}

#[test]
fn page_tree_kid_targets_composes_trailer_root_catalog_pages_page_tree_root() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let page_three = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_four = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let catalog_offset = prefix.len();
    let pages_offset = prefix.len() + catalog.len();
    let page_three_offset = pages_offset + pages.len();
    let page_four_offset = page_three_offset + page_three.len();
    let xref_offset = page_four_offset + page_four.len();
    let source = format!(
        "{}{}{}{}{}xref\n0 5\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{pages_offset:010} 00000 n \n{page_three_offset:010} 00000 n \n{page_four_offset:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(catalog),
        String::from_utf8_lossy(pages),
        String::from_utf8_lossy(page_three),
        String::from_utf8_lossy(page_four),
    )
    .into_bytes();

    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    let root = inspect_classic_xref_trailer_root(&source, xref.trailer_byte_offset)
        .expect("trailer root should inspect");
    let catalog_target = inspect_page_tree_reference_target(&source, &xref, root.root_reference)
        .expect("catalog reference should resolve");
    assert_eq!(catalog_target.node_type.node_type, PageTreeNodeType::Other);
    let catalog_pages = inspect_catalog_pages(&source, catalog_target.object_byte_offset)
        .expect("catalog pages should inspect");
    let page_tree_root =
        inspect_page_tree_reference_target(&source, &xref, catalog_pages.pages_reference)
            .expect("page tree root should resolve");
    assert_eq!(page_tree_root.node_type.node_type, PageTreeNodeType::Pages);

    let report = inspect_page_tree_kid_targets(&source, &xref, page_tree_root.object_byte_offset)
        .expect("kid targets should inspect");

    assert_eq!(report.resolved_count(), 2);
    assert!(report.kids.skipped.is_empty());
    assert_eq!(
        report
            .entries
            .iter()
            .filter_map(|entry| match entry {
                PageTreeKidTargetInspection::Resolved { kid, target } => Some((
                    kid.reference,
                    target.object_byte_offset,
                    target.node_type.node_type,
                )),
                PageTreeKidTargetInspection::Failed { .. } => None,
            })
            .collect::<Vec<_>>(),
        vec![
            (
                indirect_ref(3, 0),
                page_three_offset,
                PageTreeNodeType::Page
            ),
            (indirect_ref(4, 0), page_four_offset, PageTreeNodeType::Page),
        ]
    );
}
