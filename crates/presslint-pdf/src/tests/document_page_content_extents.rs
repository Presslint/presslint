#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use super::{classic_entry, classic_inspection, classic_subsection, indirect_ref};

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ClassicXrefEntryState, DocumentPageContentExtentResult, DocumentPageContentExtentsInspection,
    PageContentExtentInspection, PageContentsInspectionRejection, PageTreeLeavesTruncation,
    SkippedPageTreeLeafReason, inspect_classic_xref_table, inspect_document_page_content_extents,
};

struct ParsedFixture {
    source: Vec<u8>,
    xref: crate::ClassicXrefTableInspection,
    pages_offset: usize,
}

fn full_document_fixture(page_objects: &[&[u8]], content_objects: &[&[u8]]) -> ParsedFixture {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let kids = (0..page_objects.len())
        .map(|index| format!("{} 0 R", index + 3))
        .collect::<Vec<_>>()
        .join(" ");
    let pages = format!(
        "2 0 obj\n<< /Type /Pages /Kids [ {kids} ] /Count {} >>\nendobj\n",
        page_objects.len()
    );

    let mut source = Vec::new();
    source.extend_from_slice(prefix);
    let catalog_offset = source.len();
    source.extend_from_slice(catalog);
    let pages_offset = source.len();
    source.extend_from_slice(pages.as_bytes());

    let mut object_offsets = vec![0, catalog_offset, pages_offset];
    for object in page_objects {
        object_offsets.push(source.len());
        source.extend_from_slice(object);
    }
    for object in content_objects {
        object_offsets.push(source.len());
        source.extend_from_slice(object);
    }

    let xref_offset = source.len();
    let object_count = object_offsets.len();
    source.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
    source.extend_from_slice(b"0000000000 65535 f \n");
    for offset in object_offsets.iter().skip(1) {
        if *offset == 0 {
            source.extend_from_slice(b"0000000000 00000 f \n");
        } else {
            source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
    }
    source.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );

    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    ParsedFixture {
        source,
        xref,
        pages_offset,
    }
}

#[test]
fn document_page_content_extents_locates_all_pages_in_document_order() {
    let fixture = full_document_fixture(
        &[
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n",
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [ 6 0 R 7 0 R ] >>\nendobj\n",
        ],
        &[
            b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n",
            b"6 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n",
            b"7 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n",
        ],
    );

    let report =
        inspect_document_page_content_extents(&fixture.source, &fixture.xref, fixture.pages_offset)
            .expect("document page content extents should inspect");

    assert_eq!(report.byte_len, fixture.source.len());
    assert_eq!(report.page_count(), 2);
    assert_eq!(report.located_page_count(), 2);
    assert_eq!(report.leaves.leaf_count(), 2);
    assert!(report.leaves.skipped.is_empty());
    assert!(report.leaves.truncated.is_none());
    assert_eq!(
        report
            .pages
            .iter()
            .map(|page| (page.ordinal, page.leaf.reference))
            .collect::<Vec<_>>(),
        vec![(0, indirect_ref(3, 0)), (1, indirect_ref(4, 0))]
    );

    assert!(matches!(
        report.pages[1].result,
        DocumentPageContentExtentResult::Inspected { .. }
    ));
    if let DocumentPageContentExtentResult::Inspected {
        contents,
        targets,
        extents,
    } = &report.pages[1].result
    {
        assert_eq!(
            contents
                .contents
                .iter()
                .map(|content| content.reference)
                .collect::<Vec<_>>(),
            vec![indirect_ref(6, 0), indirect_ref(7, 0)]
        );
        assert_eq!(targets.entries.len(), 2);
        assert_eq!(extents.located_count(), 2);
        assert!(
            extents
                .entries
                .iter()
                .all(|entry| matches!(entry, PageContentExtentInspection::Located { .. }))
        );
    }
}

#[test]
fn document_page_content_extents_keeps_contents_failure_per_page_and_continues() {
    let fixture = full_document_fixture(
        &[
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>\nendobj\n",
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
            b"5 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 7 0 R >>\nendobj\n",
        ],
        &[
            b"6 0 obj\n<< /Length 3 >>\nstream\none\nendstream\nendobj\n",
            b"7 0 obj\n<< /Length 3 >>\nstream\ntwo\nendstream\nendobj\n",
        ],
    );

    let report =
        inspect_document_page_content_extents(&fixture.source, &fixture.xref, fixture.pages_offset)
            .expect("document page content extents should inspect");

    assert_eq!(report.page_count(), 3);
    assert_eq!(report.located_page_count(), 2);
    assert!(report.pages[0].is_located());
    assert!(!report.pages[1].is_located());
    assert!(report.pages[2].is_located());
    assert_eq!(report.pages[1].ordinal, 1);
    assert_eq!(report.pages[1].leaf.reference, indirect_ref(4, 0));
    assert!(matches!(
        report.pages[1].result,
        DocumentPageContentExtentResult::ContentsFailed { .. }
    ));
    if let DocumentPageContentExtentResult::ContentsFailed { error } = &report.pages[1].result {
        assert_eq!(
            error.reason,
            PageContentsInspectionRejection::MissingContents
        );
    }
}

#[test]
fn document_page_content_extents_preserves_leaf_skips_and_truncation_separately() {
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R 2 0 R ] /Count 2 >>\nendobj\n";
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

    let report = inspect_document_page_content_extents(&source, &xref, 0)
        .expect("leaf skips should not fail the aggregate");

    assert_eq!(report.page_count(), 1);
    assert_eq!(report.located_page_count(), 0);
    assert_eq!(
        report.leaves.truncated,
        Some(PageTreeLeavesTruncation::Cycle { object_number: 2 })
    );
    assert_eq!(report.leaves.skipped.len(), 2);
    assert_eq!(report.leaves.skipped[0].kid, indirect_ref(4, 0));
    assert!(matches!(
        report.leaves.skipped[0].reason,
        SkippedPageTreeLeafReason::OtherNodeType { .. }
    ));
    assert_eq!(report.leaves.skipped[1].kid, indirect_ref(2, 0));
    assert!(matches!(
        report.leaves.skipped[1].reason,
        SkippedPageTreeLeafReason::Cycle { .. }
    ));
    assert!(matches!(
        report.pages[0].result,
        DocumentPageContentExtentResult::ContentsFailed { .. }
    ));
    if let DocumentPageContentExtentResult::ContentsFailed { error } = &report.pages[0].result {
        assert_eq!(
            error.reason,
            PageContentsInspectionRejection::MissingContents
        );
    }
}

#[test]
fn document_page_content_extents_serde_round_trips_success_and_contents_failure() {
    let fixture = full_document_fixture(
        &[
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n",
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n",
        ],
        &[b"5 0 obj\n<< /Length 1 >>\nstream\nx\nendstream\nendobj\n"],
    );
    let report =
        inspect_document_page_content_extents(&fixture.source, &fixture.xref, fixture.pages_offset)
            .expect("document page content extents should inspect");

    assert_eq!(report.page_count(), 2);
    assert_eq!(report.located_page_count(), 1);
    assert!(matches!(
        report.pages[0].result,
        DocumentPageContentExtentResult::Inspected { .. }
    ));
    assert!(matches!(
        report.pages[1].result,
        DocumentPageContentExtentResult::ContentsFailed { .. }
    ));

    let value = serde_value(&report).expect("aggregate report should serialize");
    let restored: DocumentPageContentExtentsInspection =
        from_serde_value(value).expect("aggregate report should deserialize");
    assert_eq!(restored, report);
}
