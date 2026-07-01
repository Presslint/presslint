use super::{classic_inspection, xref_stream_entry, xref_stream_section, xref_stream_uncompressed};

use crate::{
    ClassicXrefObjectLocation, ClassicXrefTableInspection, ContentStreamDataExtentInspection,
    IndirectRef, IndirectReferenceByteRange, ObjectLookup, ObjectLookupLocation,
    PageContentExtentInspection, PageContentExtentsInspection, PageContentReference,
    PageContentTargetInspection, PageContentTargetsInspection, SkippedPageContentTargetReason,
    inspect_catalog_pages, inspect_classic_xref_table, inspect_classic_xref_trailer_root,
    inspect_content_stream_data_extent, inspect_content_stream_data_extent_with_lookup,
    inspect_page_content_extents, inspect_page_content_extents_with_lookup,
    inspect_page_content_targets, inspect_page_contents, inspect_page_tree_kids,
    inspect_page_tree_reference_target,
};

use serde_harness::{from_serde_value, serde_value};

fn empty_xref() -> ClassicXrefTableInspection {
    classic_inspection(Vec::new())
}

fn content_reference(object_number: u32) -> PageContentReference {
    PageContentReference {
        reference: IndirectRef {
            object_number,
            generation: 0,
        },
        reference_range: IndirectReferenceByteRange { start: 0, end: 0 },
    }
}

fn resolved_target(object_number: u32, object_byte_offset: usize) -> PageContentTargetInspection {
    PageContentTargetInspection::Resolved {
        content_reference: content_reference(object_number),
        object_byte_offset,
        xref_generation: 0,
    }
}

fn not_found_skip(object_number: u32) -> PageContentTargetInspection {
    PageContentTargetInspection::Skipped {
        content_reference: content_reference(object_number),
        reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::NotFound { object_number },
        },
    }
}

fn make_targets(
    byte_len: usize,
    entries: Vec<PageContentTargetInspection>,
) -> PageContentTargetsInspection {
    PageContentTargetsInspection { byte_len, entries }
}

#[test]
fn single_reference_page_locates_one_direct_length_extent() {
    let source = b"5 0 obj\n<< /Length 12 >>\nstream\nhello world!\nendstream\nendobj\n";
    let xref = empty_xref();
    let targets = make_targets(source.len(), vec![resolved_target(5, 0)]);

    let report = inspect_page_content_extents(source, &xref, &targets);

    assert_eq!(report.byte_len, source.len());
    assert_eq!(report.located_count(), 1);
    let expected = inspect_content_stream_data_extent(source, Some(&xref), 0)
        .expect("direct-length extent should inspect");
    assert_eq!(
        &source[expected.stream_data_start_byte_offset()..expected.stream_data_end_byte_offset()],
        b"hello world!"
    );
    assert_eq!(
        report.entries,
        vec![PageContentExtentInspection::Located {
            content_reference: content_reference(5),
            object_byte_offset: 0,
            extent: expected,
        }]
    );
}

#[test]
fn with_lookup_locates_direct_length_extent_via_xref_stream_section() {
    let source = b"5 0 obj\n<< /Length 12 >>\nstream\nhello world!\nendstream\nendobj\n";
    let section = xref_stream_section(vec![xref_stream_entry(5, xref_stream_uncompressed(0))]);
    let targets = make_targets(source.len(), vec![resolved_target(5, 0)]);

    let report = inspect_page_content_extents_with_lookup(
        source,
        ObjectLookup::XrefStreamSection(&section),
        &targets,
    );

    assert_eq!(report.located_count(), 1);
    let expected = inspect_content_stream_data_extent_with_lookup(
        source,
        Some(ObjectLookup::XrefStreamSection(&section)),
        0,
    )
    .expect("direct-length extent should locate");
    assert_eq!(
        report.entries,
        vec![PageContentExtentInspection::Located {
            content_reference: content_reference(5),
            object_byte_offset: 0,
            extent: expected,
        }]
    );
}

#[test]
fn with_lookup_locates_indirect_length_extent_via_xref_stream_section() {
    let content = b"5 0 obj\n<< /Length 8 0 R >>\nstream\nABCDEFG\nendstream\nendobj\n";
    let length = b"8 0 obj\n7\nendobj\n";
    let mut source = content.to_vec();
    let length_offset = source.len();
    source.extend_from_slice(length);
    let section = xref_stream_section(vec![
        xref_stream_entry(5, xref_stream_uncompressed(0)),
        xref_stream_entry(8, xref_stream_uncompressed(length_offset)),
    ]);
    let targets = make_targets(source.len(), vec![resolved_target(5, 0)]);

    let report = inspect_page_content_extents_with_lookup(
        &source,
        ObjectLookup::XrefStreamSection(&section),
        &targets,
    );

    assert_eq!(report.located_count(), 1);
    assert!(matches!(
        &report.entries[0],
        PageContentExtentInspection::Located {
            extent: ContentStreamDataExtentInspection::IndirectLength(_),
            ..
        }
    ));
    if let PageContentExtentInspection::Located { extent, .. } = &report.entries[0] {
        assert_eq!(extent.length(), 7);
        assert_eq!(
            &source[extent.stream_data_start_byte_offset()..extent.stream_data_end_byte_offset()],
            b"ABCDEFG"
        );
    }
}

#[test]
fn with_lookup_carries_through_unresolved_lookup_skip() {
    let source = b"5 0 obj\n<< /Length 12 >>\nstream\nhello world!\nendstream\nendobj\n";
    let section = xref_stream_section(vec![xref_stream_entry(5, xref_stream_uncompressed(0))]);
    let skip_reason = SkippedPageContentTargetReason::UnresolvedLookupLocation {
        location: ObjectLookupLocation::XrefStreamNotFound { object_number: 6 },
    };
    let targets = make_targets(
        source.len(),
        vec![
            resolved_target(5, 0),
            PageContentTargetInspection::Skipped {
                content_reference: content_reference(6),
                reason: skip_reason.clone(),
            },
        ],
    );

    let report = inspect_page_content_extents_with_lookup(
        source,
        ObjectLookup::XrefStreamSection(&section),
        &targets,
    );

    assert_eq!(report.located_count(), 1);
    assert_eq!(
        report.entries[1],
        PageContentExtentInspection::Skipped {
            content_reference: content_reference(6),
            reason: skip_reason,
        }
    );
}

#[test]
fn classic_wrapper_equals_classic_lookup_extents() {
    let source = b"5 0 obj\n<< /Length 12 >>\nstream\nhello world!\nendstream\nendobj\n";
    let xref = empty_xref();
    let targets = make_targets(source.len(), vec![resolved_target(5, 0)]);

    let wrapper = inspect_page_content_extents(source, &xref, &targets);
    let lookup = inspect_page_content_extents_with_lookup(
        source,
        ObjectLookup::ClassicXref(&xref),
        &targets,
    );

    assert_eq!(wrapper, lookup);
}

#[test]
fn multi_stream_array_page_locates_extents_in_content_order() {
    let first = b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n";
    let second = b"6 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(first);
    let second_offset = source.len();
    source.extend_from_slice(second);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![resolved_target(5, 0), resolved_target(6, second_offset)],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.located_count(), 2);
    let first_extent = inspect_content_stream_data_extent(&source, Some(&xref), 0)
        .expect("first direct-length extent should inspect");
    let second_extent = inspect_content_stream_data_extent(&source, Some(&xref), second_offset)
        .expect("second direct-length extent should inspect");
    assert_eq!(
        &source[first_extent.stream_data_start_byte_offset()
            ..first_extent.stream_data_end_byte_offset()],
        b"abc"
    );
    assert_eq!(
        &source[second_extent.stream_data_start_byte_offset()
            ..second_extent.stream_data_end_byte_offset()],
        b"hello"
    );
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Located {
                content_reference: content_reference(5),
                object_byte_offset: 0,
                extent: first_extent,
            },
            PageContentExtentInspection::Located {
                content_reference: content_reference(6),
                object_byte_offset: second_offset,
                extent: second_extent,
            },
        ]
    );
}

#[test]
fn page_mixing_resolved_stream_and_skipped_target_preserves_skip() {
    let source = b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n";
    let xref = empty_xref();
    let targets = make_targets(source.len(), vec![resolved_target(5, 0), not_found_skip(6)]);

    let report = inspect_page_content_extents(source, &xref, &targets);

    assert_eq!(report.located_count(), 1);
    let extent = inspect_content_stream_data_extent(source, Some(&xref), 0)
        .expect("direct-length extent should inspect");
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Located {
                content_reference: content_reference(5),
                object_byte_offset: 0,
                extent,
            },
            PageContentExtentInspection::Skipped {
                content_reference: content_reference(6),
                reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
                    location: ClassicXrefObjectLocation::NotFound { object_number: 6 },
                },
            },
        ]
    );
}

#[test]
fn resolved_target_with_failing_extent_still_processes_later_targets() {
    let malformed = b"5 0 obj\n<< /Other 1 >>\nstream\nabc\nendstream\nendobj\n";
    let healthy = b"6 0 obj\n<< /Length 3 >>\nstream\nxyz\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(malformed);
    let healthy_offset = source.len();
    source.extend_from_slice(healthy);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![resolved_target(5, 0), resolved_target(6, healthy_offset)],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.located_count(), 1);
    let expected_error = inspect_content_stream_data_extent(&source, Some(&xref), 0)
        .expect_err("missing /Length should fail extent inspection");
    let healthy_extent = inspect_content_stream_data_extent(&source, Some(&xref), healthy_offset)
        .expect("healthy direct-length extent should inspect");
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Failed {
                content_reference: content_reference(5),
                object_byte_offset: 0,
                error: expected_error,
            },
            PageContentExtentInspection::Located {
                content_reference: content_reference(6),
                object_byte_offset: healthy_offset,
                extent: healthy_extent,
            },
        ]
    );
}

#[test]
fn located_count_reports_only_located_entries() {
    let healthy = b"7 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n";
    let malformed = b"5 0 obj\n<< /Other 1 >>\nstream\nz\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(healthy);
    let malformed_offset = source.len();
    source.extend_from_slice(malformed);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![
            resolved_target(7, 0),
            not_found_skip(6),
            resolved_target(5, malformed_offset),
        ],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.entries.len(), 3);
    assert_eq!(report.located_count(), 1);
}

#[test]
fn serde_round_trip_preserves_aggregate_report_and_failed_entry() {
    let healthy = b"6 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n";
    let malformed = b"5 0 obj\n<< /Other 1 >>\nstream\nz\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(healthy);
    let malformed_offset = source.len();
    source.extend_from_slice(malformed);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![resolved_target(6, 0), resolved_target(5, malformed_offset)],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);
    assert_eq!(report.located_count(), 1);

    let value = serde_value(&report).expect("aggregate report should serialize");
    let restored: PageContentExtentsInspection =
        from_serde_value(value).expect("aggregate report should deserialize");
    assert_eq!(restored, report);

    let failed_entry = report.entries[1].clone();
    assert!(matches!(
        failed_entry,
        PageContentExtentInspection::Failed { .. }
    ));
    let entry_value = serde_value(&failed_entry).expect("failed entry should serialize");
    let restored_entry: PageContentExtentInspection =
        from_serde_value(entry_value).expect("failed entry should deserialize");
    assert_eq!(restored_entry, failed_entry);
}

#[test]
fn composition_chains_page_contents_targets_and_aggregator() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [ 5 0 R 6 0 R ] >>\nendobj\n";
    let content_five = b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n";
    let content_six = b"6 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n";
    let catalog_offset = prefix.len();
    let pages_offset = prefix.len() + catalog.len();
    let page_offset = pages_offset + pages.len();
    let content_five_offset = page_offset + page.len();
    let content_six_offset = content_five_offset + content_five.len();
    let xref_offset = content_six_offset + content_six.len();
    let source = format!(
        "{}{}{}{}{}{}xref\n0 7\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{pages_offset:010} 00000 n \n{page_offset:010} 00000 n \n0000000000 00000 f \n{content_five_offset:010} 00000 n \n{content_six_offset:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(catalog),
        String::from_utf8_lossy(pages),
        String::from_utf8_lossy(page),
        String::from_utf8_lossy(content_five),
        String::from_utf8_lossy(content_six),
    )
    .into_bytes();

    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    let root = inspect_classic_xref_trailer_root(&source, xref.trailer_byte_offset)
        .expect("trailer root should inspect");
    let catalog_target = inspect_page_tree_reference_target(&source, &xref, root.root_reference)
        .expect("catalog reference should resolve");
    let catalog_pages = inspect_catalog_pages(&source, catalog_target.object_byte_offset)
        .expect("catalog pages should inspect");
    let page_tree =
        inspect_page_tree_reference_target(&source, &xref, catalog_pages.pages_reference)
            .expect("page tree should resolve");
    let kids =
        inspect_page_tree_kids(&source, page_tree.object_byte_offset).expect("kids should inspect");
    let page_target = inspect_page_tree_reference_target(&source, &xref, kids.kids[0].reference)
        .expect("page should resolve");
    let contents = inspect_page_contents(&source, page_target.object_byte_offset)
        .expect("page contents should inspect");
    let targets = inspect_page_content_targets(&source, &xref, &contents);

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.located_count(), 2);
    let expected_five =
        inspect_content_stream_data_extent(&source, Some(&xref), content_five_offset)
            .expect("first resolved extent should inspect");
    let expected_six = inspect_content_stream_data_extent(&source, Some(&xref), content_six_offset)
        .expect("second resolved extent should inspect");
    assert_eq!(
        &source[expected_five.stream_data_start_byte_offset()
            ..expected_five.stream_data_end_byte_offset()],
        b"abc"
    );
    assert_eq!(
        &source[expected_six.stream_data_start_byte_offset()
            ..expected_six.stream_data_end_byte_offset()],
        b"hello"
    );
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Located {
                content_reference: contents.contents[0],
                object_byte_offset: content_five_offset,
                extent: expected_five,
            },
            PageContentExtentInspection::Located {
                content_reference: contents.contents[1],
                object_byte_offset: content_six_offset,
                extent: expected_six,
            },
        ]
    );
}

/// Minimal dependency-free serde value tree and adapters for shape round-trip
/// tests, mirroring the focused harness used by the content-stream-extent tests.
mod serde_harness;
