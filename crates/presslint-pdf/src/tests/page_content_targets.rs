use super::{
    classic_entry, classic_inspection, classic_subsection, xref_stream_entry, xref_stream_section,
    xref_stream_uncompressed,
};

use crate::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefEntryState, ClassicXrefObjectLocation,
    ObjectLookup, ObjectLookupLocation, PageContentTargetInspection,
    SkippedPageContentTargetReason, XrefStreamEntryRecord, inspect_catalog_pages,
    inspect_classic_xref_table, inspect_classic_xref_trailer_root, inspect_page_content_targets,
    inspect_page_content_targets_with_lookup, inspect_page_contents, inspect_page_tree_kids,
    inspect_page_tree_reference_target,
};

#[test]
fn page_content_targets_resolves_all_references_in_source_order() {
    let source = b"4 0 obj\n<< /Type /Page /Contents [ 5 0 R 6 2 R 7 0 R ] >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let xref = classic_inspection(vec![classic_subsection(
        5,
        vec![
            classic_entry(5, 0, 500, ClassicXrefEntryState::InUse),
            classic_entry(6, 2, 600, ClassicXrefEntryState::InUse),
            classic_entry(7, 0, 700, ClassicXrefEntryState::InUse),
        ],
    )]);

    let report = inspect_page_content_targets(source, &xref, &contents);

    assert_eq!(report.byte_len, source.len());
    assert_eq!(
        report.entries,
        vec![
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[0],
                object_byte_offset: 500,
                xref_generation: 0,
            },
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[1],
                object_byte_offset: 600,
                xref_generation: 2,
            },
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[2],
                object_byte_offset: 700,
                xref_generation: 0,
            },
        ]
    );
}

#[test]
fn page_content_targets_reports_mixed_resolved_and_skipped_entries_in_source_order() {
    let source = b"4 0 obj\n<< /Contents [ 5 0 R 6 0 R 7 0 R 8 0 R ] >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let xref = classic_inspection(vec![classic_subsection(
        5,
        vec![
            classic_entry(5, 0, 500, ClassicXrefEntryState::InUse),
            classic_entry(6, 0, 0, ClassicXrefEntryState::Free),
            classic_entry(7, 1, 700, ClassicXrefEntryState::InUse),
            classic_entry(8, 0, 800, ClassicXrefEntryState::InUse),
        ],
    )]);

    let report = inspect_page_content_targets(source, &xref, &contents);

    assert_eq!(
        report.entries,
        vec![
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[0],
                object_byte_offset: 500,
                xref_generation: 0,
            },
            PageContentTargetInspection::Skipped {
                content_reference: contents.contents[1],
                reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
                    location: ClassicXrefObjectLocation::Free {
                        object_number: 6,
                        generation: 0,
                        next_free_object_number: 0,
                    },
                },
            },
            PageContentTargetInspection::Skipped {
                content_reference: contents.contents[2],
                reason: SkippedPageContentTargetReason::GenerationMismatch {
                    requested_generation: 0,
                    xref_generation: 1,
                    object_byte_offset: 700,
                },
            },
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[3],
                object_byte_offset: 800,
                xref_generation: 0,
            },
        ]
    );
}

#[test]
fn page_content_targets_reports_generation_mismatch_as_structured_skip() {
    let source = b"4 0 obj\n<< /Contents 5 2 R >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let xref = classic_inspection(vec![classic_subsection(
        5,
        vec![classic_entry(5, 0, 500, ClassicXrefEntryState::InUse)],
    )]);

    let report = inspect_page_content_targets(source, &xref, &contents);

    assert_eq!(
        report.entries,
        vec![PageContentTargetInspection::Skipped {
            content_reference: contents.contents[0],
            reason: SkippedPageContentTargetReason::GenerationMismatch {
                requested_generation: 2,
                xref_generation: 0,
                object_byte_offset: 500,
            },
        }]
    );
}

#[test]
fn page_content_targets_reports_free_not_found_and_ambiguous_xref_results() {
    let source = b"4 0 obj\n<< /Contents [ 5 0 R 6 0 R 7 0 R ] >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let xref = classic_inspection(vec![
        classic_subsection(5, vec![classic_entry(5, 0, 0, ClassicXrefEntryState::Free)]),
        classic_subsection(
            7,
            vec![classic_entry(7, 0, 700, ClassicXrefEntryState::InUse)],
        ),
        classic_subsection(
            7,
            vec![classic_entry(7, 1, 701, ClassicXrefEntryState::InUse)],
        ),
    ]);

    let report = inspect_page_content_targets(source, &xref, &contents);

    assert_eq!(
        report.entries,
        vec![
            PageContentTargetInspection::Skipped {
                content_reference: contents.contents[0],
                reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
                    location: ClassicXrefObjectLocation::Free {
                        object_number: 5,
                        generation: 0,
                        next_free_object_number: 0,
                    },
                },
            },
            PageContentTargetInspection::Skipped {
                content_reference: contents.contents[1],
                reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
                    location: ClassicXrefObjectLocation::NotFound { object_number: 6 },
                },
            },
            PageContentTargetInspection::Skipped {
                content_reference: contents.contents[2],
                reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
                    location: ClassicXrefObjectLocation::Ambiguous {
                        object_number: 7,
                        first: ClassicXrefAmbiguousObjectEntry {
                            generation: 0,
                            byte_offset: 700,
                            state: ClassicXrefEntryState::InUse,
                        },
                        second: ClassicXrefAmbiguousObjectEntry {
                            generation: 1,
                            byte_offset: 701,
                            state: ClassicXrefEntryState::InUse,
                        },
                    },
                },
            },
        ]
    );
}

#[test]
fn page_content_targets_with_lookup_resolves_through_xref_stream_section() {
    let source = b"4 0 obj\n<< /Contents [ 5 0 R 6 0 R ] >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let section = xref_stream_section(vec![
        xref_stream_entry(5, xref_stream_uncompressed(500)),
        xref_stream_entry(6, xref_stream_uncompressed(600)),
    ]);

    let report = inspect_page_content_targets_with_lookup(
        source,
        ObjectLookup::XrefStreamSection(&section),
        &contents,
    );

    assert_eq!(
        report.entries,
        vec![
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[0],
                object_byte_offset: 500,
                xref_generation: 0,
            },
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[1],
                object_byte_offset: 600,
                xref_generation: 0,
            },
        ]
    );
}

#[test]
fn page_content_targets_classic_wrapper_equals_classic_lookup() {
    let source = b"4 0 obj\n<< /Contents [ 5 0 R 6 0 R ] >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let xref = classic_inspection(vec![classic_subsection(
        5,
        vec![
            classic_entry(5, 0, 500, ClassicXrefEntryState::InUse),
            classic_entry(6, 0, 0, ClassicXrefEntryState::Free),
        ],
    )]);

    let wrapper = inspect_page_content_targets(source, &xref, &contents);
    let lookup = inspect_page_content_targets_with_lookup(
        source,
        ObjectLookup::ClassicXref(&xref),
        &contents,
    );

    assert_eq!(wrapper, lookup);
}

#[test]
fn page_content_targets_with_lookup_reports_unresolved_and_compressed_xref_stream_entries() {
    let source = b"4 0 obj\n<< /Contents [ 5 0 R 6 0 R 7 0 R ] >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let section = xref_stream_section(vec![
        xref_stream_entry(5, xref_stream_uncompressed(500)),
        xref_stream_entry(
            7,
            XrefStreamEntryRecord::Compressed {
                object_stream_number: 9,
                index_within_object_stream: 2,
            },
        ),
    ]);

    let report = inspect_page_content_targets_with_lookup(
        source,
        ObjectLookup::XrefStreamSection(&section),
        &contents,
    );

    assert_eq!(
        report.entries,
        vec![
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[0],
                object_byte_offset: 500,
                xref_generation: 0,
            },
            PageContentTargetInspection::Skipped {
                content_reference: contents.contents[1],
                reason: SkippedPageContentTargetReason::UnresolvedLookupLocation {
                    location: ObjectLookupLocation::XrefStreamNotFound { object_number: 6 },
                },
            },
            PageContentTargetInspection::Skipped {
                content_reference: contents.contents[2],
                reason: SkippedPageContentTargetReason::UnresolvedLookupLocation {
                    location: ObjectLookupLocation::XrefStreamCompressed {
                        object_number: 7,
                        object_stream_number: 9,
                        index_within_object_stream: 2,
                    },
                },
            },
        ]
    );
}

#[test]
fn page_content_targets_with_lookup_reports_generation_mismatch_over_xref_stream() {
    let source = b"4 0 obj\n<< /Contents 5 0 R >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let section = xref_stream_section(vec![xref_stream_entry(
        5,
        XrefStreamEntryRecord::Uncompressed {
            byte_offset: 500,
            generation: 1,
        },
    )]);

    let report = inspect_page_content_targets_with_lookup(
        source,
        ObjectLookup::XrefStreamSection(&section),
        &contents,
    );

    assert_eq!(
        report.entries,
        vec![PageContentTargetInspection::Skipped {
            content_reference: contents.contents[0],
            reason: SkippedPageContentTargetReason::GenerationMismatch {
                requested_generation: 0,
                xref_generation: 1,
                object_byte_offset: 500,
            },
        }]
    );
}

#[test]
fn page_content_targets_report_does_not_retain_source_bytes() {
    let source = b"4 0 obj\n<< /Contents 5 0 R /Secret (not-copied) >>\nendobj\n";
    let contents = inspect_page_contents(source, 0).expect("contents should inspect");
    let xref = classic_inspection(vec![classic_subsection(
        5,
        vec![classic_entry(5, 0, 500, ClassicXrefEntryState::InUse)],
    )]);

    let report = inspect_page_content_targets(source, &xref, &contents);

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("Secret"));
    assert!(!debug_report.contains("not-copied"));
    assert!(!debug_report.contains("/Contents"));
}

#[test]
fn page_content_targets_compose_from_catalog_page_tree_page_contents_to_resolved_targets() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [ 5 0 R 6 0 R ] >>\nendobj\n";
    let content_five = b"5 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n";
    let content_six = b"6 0 obj\n<< /Length 0 >>\nstream\n\nendstream\nendobj\n";
    let catalog_offset = prefix.len();
    let pages_offset = prefix.len() + catalog.len();
    let page_offset = prefix.len() + catalog.len() + pages.len();
    let content_five_offset = prefix.len() + catalog.len() + pages.len() + page.len();
    let content_six_offset =
        prefix.len() + catalog.len() + pages.len() + page.len() + content_five.len();
    let xref_offset = prefix.len()
        + catalog.len()
        + pages.len()
        + page.len()
        + content_five.len()
        + content_six.len();
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

    let xref_report =
        inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    let root_report = inspect_classic_xref_trailer_root(&source, xref_report.trailer_byte_offset)
        .expect("root should inspect");
    let catalog_target =
        inspect_page_tree_reference_target(&source, &xref_report, root_report.root_reference)
            .expect("catalog reference should resolve");
    let catalog_pages = inspect_catalog_pages(&source, catalog_target.object_byte_offset)
        .expect("catalog pages should inspect");
    let page_tree =
        inspect_page_tree_reference_target(&source, &xref_report, catalog_pages.pages_reference)
            .expect("page tree should resolve");
    let kids =
        inspect_page_tree_kids(&source, page_tree.object_byte_offset).expect("kids should inspect");
    let page_target =
        inspect_page_tree_reference_target(&source, &xref_report, kids.kids[0].reference)
            .expect("page should resolve");
    let contents = inspect_page_contents(&source, page_target.object_byte_offset)
        .expect("contents should inspect");

    let targets = inspect_page_content_targets(&source, &xref_report, &contents);

    assert_eq!(
        targets.entries,
        vec![
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[0],
                object_byte_offset: content_five_offset,
                xref_generation: 0,
            },
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[1],
                object_byte_offset: content_six_offset,
                xref_generation: 0,
            },
        ]
    );
    assert_eq!(
        targets
            .entries
            .iter()
            .filter(|entry| matches!(entry, PageContentTargetInspection::Skipped { .. }))
            .count(),
        0
    );
}
