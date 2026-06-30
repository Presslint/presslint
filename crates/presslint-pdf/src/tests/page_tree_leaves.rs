#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use super::{
    classic_entry, classic_inspection, classic_subsection, indirect_ref, xref_stream_entry,
    xref_stream_section, xref_stream_uncompressed,
};

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ClassicXrefEntryState, ObjectLookup, ObjectLookupLocation,
    PageTreeKidTargetsInspectionRejection, PageTreeLeavesInspection, PageTreeLeavesTruncation,
    PageTreeReferenceTargetInspectionRejection, SkippedPageTreeLeafReason, XrefStreamEntryRecord,
    inspect_catalog_pages, inspect_classic_xref_table, inspect_classic_xref_trailer_root,
    inspect_page_tree_leaves, inspect_page_tree_leaves_with_lookup,
    inspect_page_tree_reference_target,
};

#[test]
fn page_tree_leaves_enumerate_two_level_tree_in_document_order() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 6 0 R ] /Count 3 >>\nendobj\n";
    let intermediate = b"3 0 obj\n<< /Type /Pages /Kids [ 4 0 R 5 0 R ] /Count 2 >>\nendobj\n";
    let page_four = b"4 0 obj\n<< /Type /Page /Parent 3 0 R >>\nendobj\n";
    let page_five = b"5 0 obj\n<< /Type /Page /Parent 3 0 R >>\nendobj\n";
    let page_six = b"6 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let catalog_offset = prefix.len();
    let root_offset = catalog_offset + catalog.len();
    let intermediate_offset = root_offset + root.len();
    let page_four_offset = intermediate_offset + intermediate.len();
    let page_five_offset = page_four_offset + page_four.len();
    let page_six_offset = page_five_offset + page_five.len();
    let xref_offset = page_six_offset + page_six.len();
    let source = format!(
        "{}{}{}{}{}{}{}xref\n0 7\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{root_offset:010} 00000 n \n{intermediate_offset:010} 00000 n \n{page_four_offset:010} 00000 n \n{page_five_offset:010} 00000 n \n{page_six_offset:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(catalog),
        String::from_utf8_lossy(root),
        String::from_utf8_lossy(intermediate),
        String::from_utf8_lossy(page_four),
        String::from_utf8_lossy(page_five),
        String::from_utf8_lossy(page_six),
    )
    .into_bytes();

    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    let root_trailer = inspect_classic_xref_trailer_root(&source, xref.trailer_byte_offset)
        .expect("trailer root should inspect");
    let catalog_target =
        inspect_page_tree_reference_target(&source, &xref, root_trailer.root_reference)
            .expect("catalog reference should resolve");
    let catalog_pages = inspect_catalog_pages(&source, catalog_target.object_byte_offset)
        .expect("catalog pages should inspect");
    let page_tree_root =
        inspect_page_tree_reference_target(&source, &xref, catalog_pages.pages_reference)
            .expect("page tree root should resolve");

    let report = inspect_page_tree_leaves(&source, &xref, page_tree_root.object_byte_offset)
        .expect("leaf enumeration should inspect");

    assert_eq!(report.byte_len, source.len());
    assert_eq!(report.leaf_count(), 3);
    assert_eq!(report.visited_node_count, 2);
    assert!(report.skipped.is_empty());
    assert!(report.truncated.is_none());
    assert_eq!(
        report
            .leaves
            .iter()
            .map(|leaf| (leaf.reference, leaf.object_byte_offset))
            .collect::<Vec<_>>(),
        vec![
            (indirect_ref(4, 0), page_four_offset),
            (indirect_ref(5, 0), page_five_offset),
            (indirect_ref(6, 0), page_six_offset),
        ]
    );
}

fn skip_fixture() -> (Vec<u8>, crate::ClassicXrefTableInspection) {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R 99 0 R ] /Count 3 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let other = b"4 0 obj\n<< /Type /Annot >>\nendobj\n";
    let page_offset = root.len();
    let other_offset = root.len() + page.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(page);
    source.extend_from_slice(other);
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![
            classic_entry(2, 0, 0, ClassicXrefEntryState::InUse),
            classic_entry(3, 0, page_offset, ClassicXrefEntryState::InUse),
            classic_entry(4, 0, other_offset, ClassicXrefEntryState::InUse),
        ],
    )]);
    (source, xref)
}

#[test]
fn page_tree_leaves_keep_other_and_failed_kids_as_non_fatal_skips() {
    let (source, xref) = skip_fixture();

    let report =
        inspect_page_tree_leaves(&source, &xref, 0).expect("leaf enumeration should inspect");

    assert_eq!(report.leaf_count(), 1);
    assert_eq!(report.leaves[0].reference, indirect_ref(3, 0));
    assert_eq!(report.visited_node_count, 1);
    assert!(report.truncated.is_none());

    assert_eq!(report.skipped.len(), 2);
    assert_eq!(report.skipped[0].kid, indirect_ref(4, 0));
    assert!(matches!(
        report.skipped[0].reason,
        SkippedPageTreeLeafReason::OtherNodeType { .. }
    ));
    assert_eq!(report.skipped[1].kid, indirect_ref(99, 0));
    assert!(matches!(
        report.skipped[1].reason,
        SkippedPageTreeLeafReason::UnresolvedTarget { .. }
    ));
}

#[test]
fn page_tree_leaves_guard_cyclic_kids_with_truncation_marker() {
    let node_a = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let node_b = b"3 0 obj\n<< /Type /Pages /Kids [ 2 0 R ] /Count 1 >>\nendobj\n";
    let node_b_offset = node_a.len();
    let mut source = Vec::new();
    source.extend_from_slice(node_a);
    source.extend_from_slice(node_b);
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![
            classic_entry(2, 0, 0, ClassicXrefEntryState::InUse),
            classic_entry(3, 0, node_b_offset, ClassicXrefEntryState::InUse),
        ],
    )]);

    let report = inspect_page_tree_leaves(&source, &xref, 0)
        .expect("cyclic kids must terminate, not recurse forever");

    assert!(report.leaves.is_empty());
    assert_eq!(report.visited_node_count, 2);
    assert_eq!(
        report.truncated,
        Some(PageTreeLeavesTruncation::Cycle { object_number: 2 })
    );
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].kid, indirect_ref(2, 0));
    assert!(matches!(
        report.skipped[0].reason,
        SkippedPageTreeLeafReason::Cycle { .. }
    ));
}

#[test]
fn page_tree_leaves_fail_only_when_root_expansion_fails() {
    let root = b"2 0 obj\n<< /Type /Pages /Count 0 >>\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(root);
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let error = inspect_page_tree_leaves(&source, &xref, 0)
        .expect_err("missing /Kids root node should fail enumeration");

    assert_eq!(error.root_node_byte_offset, 0);
    assert_eq!(error.byte_len, source.len());
    assert!(matches!(
        error.error.reason,
        PageTreeKidTargetsInspectionRejection::PageTreeKids { .. }
    ));
}

#[test]
fn with_lookup_over_classic_backend_matches_classic_helper() {
    let (source, xref) = skip_fixture();

    let classic = inspect_page_tree_leaves(&source, &xref, 0);
    let neutral =
        inspect_page_tree_leaves_with_lookup(&source, ObjectLookup::ClassicXref(&xref), 0);

    assert_eq!(classic, neutral);
}

#[test]
fn with_lookup_enumerates_xref_stream_backed_leaves_in_document_order() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let page_three = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_four = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_three_offset = root.len();
    let page_four_offset = root.len() + page_three.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(page_three);
    source.extend_from_slice(page_four);
    let section = xref_stream_section(vec![
        xref_stream_entry(2, xref_stream_uncompressed(0)),
        xref_stream_entry(3, xref_stream_uncompressed(page_three_offset)),
        xref_stream_entry(4, xref_stream_uncompressed(page_four_offset)),
    ]);

    let report =
        inspect_page_tree_leaves_with_lookup(&source, ObjectLookup::XrefStreamSection(&section), 0)
            .expect("xref-stream-backed leaf enumeration should inspect");

    assert_eq!(report.leaf_count(), 2);
    assert!(report.skipped.is_empty());
    assert!(report.truncated.is_none());
    assert_eq!(
        report
            .leaves
            .iter()
            .map(|leaf| (leaf.reference, leaf.object_byte_offset))
            .collect::<Vec<_>>(),
        vec![
            (indirect_ref(3, 0), page_three_offset),
            (indirect_ref(4, 0), page_four_offset),
        ]
    );
}

#[test]
fn with_lookup_skips_compressed_kid_as_non_leaf() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let page_three = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_three_offset = root.len();
    let mut source = Vec::new();
    source.extend_from_slice(root);
    source.extend_from_slice(page_three);
    let section = xref_stream_section(vec![
        xref_stream_entry(3, xref_stream_uncompressed(page_three_offset)),
        xref_stream_entry(
            4,
            XrefStreamEntryRecord::Compressed {
                object_stream_number: 9,
                index_within_object_stream: 1,
            },
        ),
    ]);

    let report =
        inspect_page_tree_leaves_with_lookup(&source, ObjectLookup::XrefStreamSection(&section), 0)
            .expect("a compressed kid must not abort leaf enumeration");

    assert_eq!(report.leaf_count(), 1);
    assert_eq!(report.leaves[0].reference, indirect_ref(3, 0));
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].kid, indirect_ref(4, 0));
    assert!(matches!(
        report.skipped[0].reason,
        SkippedPageTreeLeafReason::UnresolvedTarget { .. }
    ));
    if let SkippedPageTreeLeafReason::UnresolvedTarget { error } = &report.skipped[0].reason {
        assert_eq!(
            error.reason,
            PageTreeReferenceTargetInspectionRejection::UnresolvedLookupLocation {
                location: ObjectLookupLocation::XrefStreamCompressed {
                    object_number: 4,
                    object_stream_number: 9,
                    index_within_object_stream: 1,
                },
            }
        );
    }
}

#[test]
fn page_tree_leaves_serde_round_trips_report_with_skips() {
    let (source, xref) = skip_fixture();
    let report =
        inspect_page_tree_leaves(&source, &xref, 0).expect("leaf enumeration should inspect");

    let value = serde_value(&report).expect("leaves report should serialize");
    let restored: PageTreeLeavesInspection =
        from_serde_value(value).expect("leaves report should deserialize");
    assert_eq!(restored, report);
}
