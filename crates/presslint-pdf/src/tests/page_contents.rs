use super::{classic_entry, classic_inspection, classic_subsection, indirect_ref};

use crate::{
    ClassicXrefEntryState, DictionaryValueKind, IndirectObjectDictionaryInspectionRejection,
    IndirectObjectHeaderInspectionRejection, IndirectReferenceInspectionRejection,
    PageContentsInspectionRejection, PageContentsValueShape, PageTreeNodeType,
    SkippedPageContentEntryKind, inspect_catalog_pages, inspect_classic_xref_table,
    inspect_classic_xref_trailer_root, inspect_page_contents, inspect_page_tree_kids,
    inspect_page_tree_reference_target,
};

#[test]
fn page_contents_reports_single_reference_value() {
    let source = b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n";

    let report = inspect_page_contents(source, 0).expect("contents should inspect");

    assert_eq!(report.value_shape, PageContentsValueShape::SingleReference);
    assert_eq!(
        report
            .contents
            .iter()
            .map(|content| content.reference)
            .collect::<Vec<_>>(),
        vec![indirect_ref(5, 0)]
    );
    assert!(report.skipped.is_empty());
    assert_eq!(
        &source[report.contents_key_range.start..report.contents_key_range.end],
        b"/Contents"
    );
    assert_eq!(
        &source[report.contents_value_range.start..report.contents_value_range.end],
        b"5 0 R"
    );
    assert_eq!(
        &source[report.contents[0].reference_range.start..report.contents[0].reference_range.end],
        b"5 0 R"
    );
}

#[test]
fn page_contents_reports_multiple_array_references_in_source_order() {
    let source = b"4 0 obj\n<< /Type /Page /Contents [ 5 0 R 6 2 R 7 0 R ] >>\nendobj\n";

    let report = inspect_page_contents(source, 0).expect("contents should inspect");

    assert_eq!(report.value_shape, PageContentsValueShape::Array);
    assert_eq!(
        report
            .contents
            .iter()
            .map(|content| content.reference)
            .collect::<Vec<_>>(),
        vec![indirect_ref(5, 0), indirect_ref(6, 2), indirect_ref(7, 0)]
    );
    assert!(report.skipped.is_empty());
    assert_eq!(
        &source[report.contents_value_range.start..report.contents_value_range.end],
        b"[ 5 0 R 6 2 R 7 0 R ]"
    );
    assert_eq!(
        &source[report.contents[0].reference_range.start..report.contents[0].reference_range.end],
        b"5 0 R"
    );
}

#[test]
fn page_contents_reports_array_skips_for_non_reference_entries() {
    let source = b"4 0 obj\n<< /Contents [ 5 0 R /Bad (str) 6 0 R ] >>\nendobj\n";

    let report = inspect_page_contents(source, 0).expect("contents should inspect");

    assert_eq!(report.value_shape, PageContentsValueShape::Array);
    assert_eq!(
        report
            .contents
            .iter()
            .map(|content| content.reference)
            .collect::<Vec<_>>(),
        vec![indirect_ref(5, 0), indirect_ref(6, 0)]
    );
    assert_eq!(
        report
            .skipped
            .iter()
            .map(|skip| skip.kind)
            .collect::<Vec<_>>(),
        vec![
            SkippedPageContentEntryKind::Name,
            SkippedPageContentEntryKind::String,
        ]
    );
    assert_eq!(
        &source[report.skipped[0].entry_range.start..report.skipped[0].entry_range.end],
        b"/Bad"
    );
}

#[test]
fn page_contents_rejects_missing_contents() {
    let source = b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";

    let error = inspect_page_contents(source, 0).expect_err("missing contents should reject");

    assert_eq!(error.page_header_byte_offset, Some(0));
    assert_eq!(
        error.reason,
        PageContentsInspectionRejection::MissingContents
    );
}

#[test]
fn page_contents_rejects_duplicate_contents() {
    let source = b"4 0 obj\n<< /Contents 5 0 R /Type /Page /Contents 6 0 R >>\nendobj\n";

    let error = inspect_page_contents(source, 0).expect_err("duplicate contents should reject");

    assert_eq!(
        error.reason,
        PageContentsInspectionRejection::DuplicateContents {
            first_key_range: crate::DictionaryEntryByteRange { start: 11, end: 20 },
            duplicate_key_range: crate::DictionaryEntryByteRange { start: 39, end: 48 },
        }
    );
    assert_eq!(error.error_byte_offset, Some(39));
}

#[test]
fn page_contents_rejects_non_reference_non_array_scalar_value() {
    for (source, expected_kind) in [
        (
            b"4 0 obj\n<< /Contents /Foo >>\nendobj\n".as_slice(),
            DictionaryValueKind::Name,
        ),
        (
            b"4 0 obj\n<< /Contents << /K 9 >> >>\nendobj\n".as_slice(),
            DictionaryValueKind::Dictionary,
        ),
        (
            b"4 0 obj\n<< /Contents 9 >>\nendobj\n".as_slice(),
            DictionaryValueKind::NumberLike,
        ),
    ] {
        let error = inspect_page_contents(source, 0).expect_err("scalar value should reject");

        assert_eq!(
            error.reason,
            PageContentsInspectionRejection::NonReferenceOrArrayContentsValue {
                value_kind: expected_kind,
            }
        );
    }
}

#[test]
fn page_contents_rejects_malformed_single_reference() {
    let source = b"4 0 obj\n<< /Contents 5 0 obj >>\nendobj\n";

    let error = inspect_page_contents(source, 0).expect_err("obj value should reject");

    assert_eq!(
        error.reason,
        PageContentsInspectionRejection::MalformedContentsReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
}

#[test]
fn page_contents_rejects_single_reference_with_extra_scalar_token() {
    let source = b"4 0 obj\n<< /Contents 5 0 R extra >>\nendobj\n";

    let error = inspect_page_contents(source, 0).expect_err("extra token should reject");

    assert_eq!(
        error.reason,
        PageContentsInspectionRejection::MalformedContentsReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
}

#[test]
fn page_contents_propagates_page_dictionary_rejection() {
    let source = b"xref\n0 1\n0000000000 65535 f \n";

    let error = inspect_page_contents(source, 0).expect_err("non-object should reject");

    assert_eq!(error.byte_offset, 0);
    assert_eq!(error.byte_len, source.len());
    assert_eq!(error.page_header_byte_offset, None);
    assert_eq!(
        error.reason,
        PageContentsInspectionRejection::PageDictionary {
            page_dictionary_reason: IndirectObjectDictionaryInspectionRejection::Header {
                header_reason: IndirectObjectHeaderInspectionRejection::MalformedHeader,
            },
        }
    );
}

#[test]
fn page_contents_report_does_not_retain_source_bytes() {
    let source = b"4 0 obj\n<< /Type /Page /Contents 5 0 R /Secret (not-copied) >>\nendobj\n";

    let report = inspect_page_contents(source, 0).expect("contents should inspect");

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("Secret"));
    assert!(!debug_report.contains("not-copied"));
    assert!(!debug_report.contains("/Contents"));
    assert!(!debug_report.contains("/Page"));
}

#[test]
fn page_contents_composes_from_catalog_pages_through_resolved_leaf_page() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let page_three = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n";
    let page_four = b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [ 6 0 R 7 0 R ] >>\nendobj\n";
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

    let catalog_pages =
        inspect_catalog_pages(&source, catalog_offset).expect("catalog pages should inspect");
    assert_eq!(catalog_pages.pages_reference, indirect_ref(2, 0));
    assert_eq!(root_report.root_reference, indirect_ref(1, 0));

    let pages_node =
        inspect_page_tree_reference_target(&source, &xref_report, catalog_pages.pages_reference)
            .expect("page tree node should resolve");
    assert_eq!(pages_node.node_type.node_type, PageTreeNodeType::Pages);

    let kids = inspect_page_tree_kids(&source, pages_node.object_byte_offset)
        .expect("page tree kids should inspect");
    let leaf_reference = kids.kids[0].reference;
    assert_eq!(leaf_reference, indirect_ref(3, 0));

    let leaf = inspect_page_tree_reference_target(&source, &xref_report, leaf_reference)
        .expect("leaf page should resolve");
    assert_eq!(leaf.node_type.node_type, PageTreeNodeType::Page);
    assert_eq!(leaf.object_byte_offset, page_three_offset);

    let contents = inspect_page_contents(&source, leaf.object_byte_offset)
        .expect("page contents should inspect");
    assert_eq!(
        contents.value_shape,
        PageContentsValueShape::SingleReference
    );
    assert_eq!(
        contents
            .contents
            .iter()
            .map(|content| content.reference)
            .collect::<Vec<_>>(),
        vec![indirect_ref(5, 0)]
    );
    assert!(contents.skipped.is_empty());

    // The second kid carries an array `/Contents`, exercised through the same
    // resolved-leaf path to confirm source-order array reporting.
    let second_leaf =
        inspect_page_tree_reference_target(&source, &xref_report, kids.kids[1].reference)
            .expect("second leaf page should resolve");
    assert_eq!(second_leaf.object_byte_offset, page_four_offset);
    let second_contents = inspect_page_contents(&source, second_leaf.object_byte_offset)
        .expect("second page contents should inspect");
    assert_eq!(second_contents.value_shape, PageContentsValueShape::Array);
    assert_eq!(
        second_contents
            .contents
            .iter()
            .map(|content| content.reference)
            .collect::<Vec<_>>(),
        vec![indirect_ref(6, 0), indirect_ref(7, 0)]
    );
}

#[test]
fn page_contents_composition_uses_in_use_leaf_xref_entry() {
    // Guards that the composition fixture's leaf page resolves through an in-use
    // xref entry rather than a free or missing one.
    let xref = classic_inspection(vec![classic_subsection(
        3,
        vec![classic_entry(3, 0, 0, ClassicXrefEntryState::InUse)],
    )]);
    let source = b"3 0 obj\n<< /Type /Page /Contents 5 0 R >>\nendobj\n";

    let leaf = inspect_page_tree_reference_target(&source[..], &xref, indirect_ref(3, 0))
        .expect("leaf should resolve");
    let report = inspect_page_contents(&source[..], leaf.object_byte_offset)
        .expect("contents should inspect");

    assert_eq!(
        report
            .contents
            .iter()
            .map(|content| content.reference)
            .collect::<Vec<_>>(),
        vec![indirect_ref(5, 0)]
    );
}
