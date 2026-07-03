#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use super::{
    classic_entry, classic_inspection, classic_subsection, indirect_ref, xref_stream_section,
    xref_stream_uncompressed,
};

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ClassicXrefEntryState, ClassicXrefTableInspection, ContentStreamDataExtentInspection,
    DocumentPageContentExtentInspection, DocumentPageContentExtentResult,
    DocumentPageContentExtentsInspection, ObjectLookup, PageContentExtentInspection,
    PageContentsInspectionRejection, PageTreeLeaf, PageTreeLeavesTruncation,
    ResolvedObjectPosition, SkippedPageTreeLeafReason, XrefStreamEntry, XrefStreamEntryRecord,
    XrefStreamSection, inspect_classic_xref_table, inspect_document_page_content_extents,
    inspect_document_page_content_extents_with_lookup,
};

/// Mirror an in-use classic xref table into an uncompressed cross-reference
/// stream section pointing at identical byte offsets, so the lookup spine
/// resolves the same structural objects.
fn section_from_classic(xref: &ClassicXrefTableInspection) -> XrefStreamSection {
    let mut entries: Vec<_> = xref
        .subsections
        .iter()
        .flat_map(|subsection| subsection.entries.iter())
        .map(|entry| XrefStreamEntry {
            object_number: entry.object_number as usize,
            record: match entry.state {
                ClassicXrefEntryState::InUse => xref_stream_uncompressed(entry.byte_offset),
                ClassicXrefEntryState::Free => XrefStreamEntryRecord::Free {
                    next_free_object_number: 0,
                    generation: usize::from(entry.generation),
                },
            },
        })
        .collect();
    entries.sort_by_key(|entry| entry.object_number);
    xref_stream_section(entries)
}

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
fn document_page_content_extents_with_lookup_locates_direct_and_indirect_lengths_via_xref_stream() {
    let fixture = full_document_fixture(
        &[
            b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n",
            b"4 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 6 0 R >>\nendobj\n",
        ],
        &[
            b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n",
            b"6 0 obj\n<< /Length 7 0 R >>\nstream\nLMNOPQR\nendstream\nendobj\n",
            b"7 0 obj\n7\nendobj\n",
        ],
    );
    let section = section_from_classic(&fixture.xref);

    let report = inspect_document_page_content_extents_with_lookup(
        &fixture.source,
        ObjectLookup::XrefStreamSection(&section),
        fixture.pages_offset,
    )
    .expect("lookup spine should locate document page content extents");

    assert_eq!(report.page_count(), 2);
    assert_eq!(report.located_page_count(), 2);

    // The xref-stream backend must produce the same result as the classic spine
    // because both resolve identical object byte offsets.
    let classic =
        inspect_document_page_content_extents(&fixture.source, &fixture.xref, fixture.pages_offset)
            .expect("classic spine should locate");
    assert_eq!(report, classic);

    // Page 1's single /Contents object uses an indirect /Length resolved through
    // the xref-stream backend.
    assert!(matches!(
        &report.pages[1].result,
        DocumentPageContentExtentResult::Inspected { extents, .. }
            if matches!(
                &extents.entries[0],
                PageContentExtentInspection::Located {
                    extent: ContentStreamDataExtentInspection::IndirectLength(_),
                    ..
                }
            )
    ));
    if let DocumentPageContentExtentResult::Inspected { extents, .. } = &report.pages[1].result {
        if let PageContentExtentInspection::Located { extent, .. } = &extents.entries[0] {
            assert_eq!(extent.length(), 7);
            assert_eq!(
                &fixture.source
                    [extent.stream_data_start_byte_offset()..extent.stream_data_end_byte_offset()],
                b"LMNOPQR"
            );
        }
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
fn compressed_leaf_result_is_never_located() {
    let page = DocumentPageContentExtentInspection {
        ordinal: 0,
        leaf: PageTreeLeaf {
            reference: indirect_ref(3, 0),
            object_byte_offset: 0,
            position: ResolvedObjectPosition::Compressed {
                object_stream_number: 5,
                index_within_object_stream: 2,
            },
        },
        result: DocumentPageContentExtentResult::CompressedLeaf {
            object_stream_number: 5,
            index_within_object_stream: 2,
        },
    };
    assert!(!page.is_located());
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
