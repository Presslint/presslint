use super::{
    classic_entry, classic_inspection, classic_subsection, indirect_ref, xref_stream_entry,
    xref_stream_section,
};

use crate::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefEntryState, ClassicXrefObjectLocation,
    ObjectLookup, ObjectLookupLocation, PageTreeNodeType, PageTreeNodeTypeInspectionRejection,
    PageTreeReferenceTargetInspectionRejection, XrefStreamEntryRecord, inspect_catalog_pages,
    inspect_classic_xref_table, inspect_classic_xref_trailer_root, inspect_page_tree_kids,
    inspect_page_tree_reference_target, inspect_page_tree_reference_target_with_lookup,
};

#[test]
fn page_tree_reference_target_resolves_intermediate_pages_node() {
    let source = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let report = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(2, 0))
        .expect("reference target should inspect");

    assert_eq!(report.reference, indirect_ref(2, 0));
    assert_eq!(report.object_byte_offset, 0);
    assert_eq!(report.xref_generation, 0);
    assert_eq!(report.node_type.node_type, PageTreeNodeType::Pages);
}

#[test]
fn page_tree_reference_target_resolves_leaf_page_object() {
    let source = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let xref = classic_inspection(vec![classic_subsection(
        3,
        vec![classic_entry(3, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let report = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(3, 0))
        .expect("reference target should inspect");

    assert_eq!(report.node_type.node_type, PageTreeNodeType::Page);
}

#[test]
fn page_tree_reference_target_resolves_other_name_type() {
    let source = b"5 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let xref = classic_inspection(vec![classic_subsection(
        5,
        vec![classic_entry(5, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let report = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(5, 0))
        .expect("reference target should inspect");

    assert_eq!(report.node_type.node_type, PageTreeNodeType::Other);
}

#[test]
fn page_tree_reference_target_rejects_free_xref_result() {
    let source = b"";
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 7, 0, ClassicXrefEntryState::Free)],
    )]);

    let error = inspect_page_tree_reference_target(source, &xref, indirect_ref(2, 7))
        .expect_err("free xref entry should reject");

    assert_eq!(error.reference, indirect_ref(2, 7));
    assert_eq!(error.object_byte_offset, None);
    assert_eq!(
        error.reason,
        PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::Free {
                object_number: 2,
                generation: 7,
                next_free_object_number: 0,
            },
        }
    );
}

#[test]
fn page_tree_reference_target_rejects_not_found_xref_result() {
    let source = b"";
    let xref = classic_inspection(vec![classic_subsection(
        1,
        vec![classic_entry(1, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let error = inspect_page_tree_reference_target(source, &xref, indirect_ref(2, 0))
        .expect_err("missing xref entry should reject");

    assert_eq!(
        error.reason,
        PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::NotFound { object_number: 2 },
        }
    );
}

#[test]
fn page_tree_reference_target_rejects_ambiguous_xref_result() {
    let source = b"";
    let xref = classic_inspection(vec![
        classic_subsection(
            2,
            vec![classic_entry(2, 0, 10, ClassicXrefEntryState::InUse)],
        ),
        classic_subsection(
            2,
            vec![classic_entry(2, 1, 20, ClassicXrefEntryState::InUse)],
        ),
    ]);

    let error = inspect_page_tree_reference_target(source, &xref, indirect_ref(2, 0))
        .expect_err("ambiguous xref entry should reject");

    assert_eq!(
        error.reason,
        PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::Ambiguous {
                object_number: 2,
                first: ClassicXrefAmbiguousObjectEntry {
                    generation: 0,
                    byte_offset: 10,
                    state: ClassicXrefEntryState::InUse,
                },
                second: ClassicXrefAmbiguousObjectEntry {
                    generation: 1,
                    byte_offset: 20,
                    state: ClassicXrefEntryState::InUse,
                },
            },
        }
    );
}

#[test]
fn page_tree_reference_target_rejects_generation_mismatch() {
    let source = b"2 1 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 1, 0, ClassicXrefEntryState::InUse)],
    )]);

    let error = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(2, 0))
        .expect_err("generation mismatch should reject");

    assert_eq!(error.object_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        PageTreeReferenceTargetInspectionRejection::GenerationMismatch {
            requested_generation: 0,
            xref_generation: 1,
        }
    );
}

#[test]
fn page_tree_reference_target_surfaces_delegated_node_type_failure() {
    let source = b"2 0 obj\n<< /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let error = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(2, 0))
        .expect_err("delegated node type inspection should reject");

    assert_eq!(error.object_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        PageTreeReferenceTargetInspectionRejection::NodeType {
            node_type_reason: PageTreeNodeTypeInspectionRejection::MissingType,
        }
    );
}

#[test]
fn page_tree_reference_target_report_does_not_retain_source_bytes() {
    let source =
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /Secret (not-copied) >>\nendobj\n";
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let report = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(2, 0))
        .expect("reference target should inspect");

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("Secret"));
    assert!(!debug_report.contains("not-copied"));
    assert!(!debug_report.contains("/Pages"));
    assert!(!debug_report.contains("/Type"));
}

#[test]
fn with_lookup_over_classic_backend_matches_classic_helper_on_success() {
    let source = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 0, 0, ClassicXrefEntryState::InUse)],
    )]);

    let classic = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(2, 0));
    let neutral = inspect_page_tree_reference_target_with_lookup(
        &source[..],
        ObjectLookup::ClassicXref(&xref),
        indirect_ref(2, 0),
    );

    assert_eq!(classic, neutral);
}

#[test]
fn with_lookup_over_classic_backend_matches_classic_helper_on_free_entry() {
    let source = b"";
    let xref = classic_inspection(vec![classic_subsection(
        2,
        vec![classic_entry(2, 7, 0, ClassicXrefEntryState::Free)],
    )]);

    let classic = inspect_page_tree_reference_target(source, &xref, indirect_ref(2, 7));
    let neutral = inspect_page_tree_reference_target_with_lookup(
        source,
        ObjectLookup::ClassicXref(&xref),
        indirect_ref(2, 7),
    );

    assert_eq!(classic, neutral);
    let error = neutral.expect_err("classic free entry should reject through the lookup path");
    assert_eq!(
        error.reason,
        PageTreeReferenceTargetInspectionRejection::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::Free {
                object_number: 2,
                generation: 7,
                next_free_object_number: 0,
            },
        }
    );
}

#[test]
fn with_lookup_resolves_uncompressed_xref_stream_entry() {
    let source = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let section = xref_stream_section(vec![xref_stream_entry(
        3,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 0,
            generation: 0,
        },
    )]);

    let report = inspect_page_tree_reference_target_with_lookup(
        &source[..],
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect("uncompressed xref-stream entry should resolve");

    assert_eq!(report.reference, indirect_ref(3, 0));
    assert_eq!(report.object_byte_offset, 0);
    assert_eq!(report.xref_generation, 0);
    assert_eq!(report.node_type.node_type, PageTreeNodeType::Page);
}

#[test]
fn with_lookup_reports_compressed_entry_as_neutral_unresolved() {
    let source = b"";
    let section = xref_stream_section(vec![xref_stream_entry(
        3,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 5,
            index_within_object_stream: 2,
        },
    )]);

    let error = inspect_page_tree_reference_target_with_lookup(
        source,
        ObjectLookup::XrefStreamSection(&section),
        indirect_ref(3, 0),
    )
    .expect_err("a compressed xref-stream entry has no resolvable byte offset");

    assert_eq!(error.object_byte_offset, None);
    assert_eq!(
        error.reason,
        PageTreeReferenceTargetInspectionRejection::UnresolvedLookupLocation {
            location: ObjectLookupLocation::XrefStreamCompressed {
                object_number: 3,
                object_stream_number: 5,
                index_within_object_stream: 2,
            },
        }
    );
}

#[test]
fn page_tree_reference_target_composes_catalog_pages_kids_and_one_kid_classification() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let page_three = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let page_four = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let catalog_offset = prefix.len();
    let pages_offset = prefix.len() + catalog.len();
    let page_three_offset = prefix.len() + catalog.len() + pages.len();
    let page_four_offset = prefix.len() + catalog.len() + pages.len() + page_three.len();
    let xref_offset =
        prefix.len() + catalog.len() + pages.len() + page_three.len() + page_four.len();
    let source = format!(
        "{}{}{}{}{}xref\n0 5\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{pages_offset:010} 00000 n \n{page_three_offset:010} 00000 n \n{page_four_offset:010} 00000 n \ntrailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(catalog),
        String::from_utf8_lossy(pages),
        String::from_utf8_lossy(page_three),
        String::from_utf8_lossy(page_four),
    )
    .into_bytes();

    let xref_report =
        inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    let root_report = inspect_classic_xref_trailer_root(&source, xref_report.trailer_byte_offset)
        .expect("root should inspect");
    let catalog_report =
        inspect_page_tree_reference_target(&source, &xref_report, root_report.root_reference)
            .expect("catalog reference target should inspect");
    assert_eq!(catalog_report.node_type.node_type, PageTreeNodeType::Other);

    let catalog_pages = inspect_catalog_pages(&source, catalog_report.object_byte_offset)
        .expect("catalog pages should inspect");
    let page_tree_report =
        inspect_page_tree_reference_target(&source, &xref_report, catalog_pages.pages_reference)
            .expect("page tree root should inspect");
    assert_eq!(
        page_tree_report.node_type.node_type,
        PageTreeNodeType::Pages
    );

    let kids_report = inspect_page_tree_kids(&source, page_tree_report.object_byte_offset)
        .expect("page tree kids should inspect");
    let first_kid = kids_report.kids[0].reference;
    let child_target = inspect_page_tree_reference_target(&source, &xref_report, first_kid)
        .expect("kid reference target should inspect");

    assert_eq!(first_kid, indirect_ref(3, 0));
    assert_eq!(child_target.object_byte_offset, page_three_offset);
    assert_eq!(child_target.node_type.node_type, PageTreeNodeType::Page);
}
