#![allow(clippy::expect_used, clippy::missing_errors_doc)]

mod array_extent;
mod catalog_pages;
mod classic_xref;
mod content_stream_extent;
mod dictionary_entries;
mod dictionary_extent;
mod indirect_reference;
mod integer_object;
mod object_body;
mod object_dictionary;
mod object_header;
mod object_stream;
mod page_content_extents;
mod page_content_targets;
mod page_contents;
mod page_tree_kid_targets;
mod page_tree_kids;
mod page_tree_leaves;
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

mod indirect_length_stream_extent {
    use super::{classic_entry, classic_inspection, classic_subsection};
    use crate::{
        ClassicXrefEntryState, ClassicXrefIntegerObjectResolutionRejection,
        ContentStreamStartInspectionRejection, DictionaryValueKind,
        IndirectLengthContentStreamDataExtentInspectionRejection,
        IndirectObjectBodyLeadingTokenKind, IndirectRef, PageContentTargetInspection,
        StreamEolIssue, inspect_catalog_pages, inspect_classic_xref_table,
        inspect_classic_xref_trailer_root, inspect_indirect_length_content_stream_data_extent,
        inspect_page_content_targets, inspect_page_contents, inspect_page_tree_kids,
        inspect_page_tree_reference_target,
    };

    fn fixture(
        dictionary: &[u8],
        stream_tail: &[u8],
        length_object: &[u8],
        entry_state: ClassicXrefEntryState,
    ) -> (Vec<u8>, crate::ClassicXrefTableInspection, usize) {
        let mut source = b"%PDF-1.7\n5 0 obj\n".to_vec();
        let content_offset = b"%PDF-1.7\n".len();
        source.extend_from_slice(dictionary);
        source.extend_from_slice(b"\nstream\n");
        source.extend_from_slice(stream_tail);
        source.extend_from_slice(b"endobj\n");
        let length_offset = source.len();
        source.extend_from_slice(length_object);
        let xref = classic_inspection(vec![
            classic_subsection(
                0,
                vec![classic_entry(0, 65535, 0, ClassicXrefEntryState::Free)],
            ),
            classic_subsection(7, vec![classic_entry(7, 0, length_offset, entry_state)]),
        ]);
        (source, xref, content_offset)
    }

    fn reason(
        dictionary: &[u8],
        stream_tail: &[u8],
        length_object: &[u8],
    ) -> IndirectLengthContentStreamDataExtentInspectionRejection {
        let (source, xref, offset) = fixture(
            dictionary,
            stream_tail,
            length_object,
            ClassicXrefEntryState::InUse,
        );
        inspect_indirect_length_content_stream_data_extent(&source, &xref, offset)
            .expect_err("indirect-length stream extent should reject")
            .reason
    }

    #[test]
    fn locates_resolved_data_range() {
        let (source, xref, offset) = fixture(
            b"<< /Length 7 0 R >>",
            b"hello world!\nendstream\n",
            b"7 0 obj\n12\nendobj\n",
            ClassicXrefEntryState::InUse,
        );

        let report = inspect_indirect_length_content_stream_data_extent(&source, &xref, offset)
            .expect("indirect-length stream extent should inspect");

        assert_eq!(report.length, 12);
        assert_eq!(report.length_resolution.value, 12);
        assert_eq!(
            report.length_resolution.reference,
            IndirectRef {
                object_number: 7,
                generation: 0,
            }
        );
        assert_eq!(
            &source[report.length_key_range.start..report.length_key_range.end],
            b"/Length"
        );
        assert_eq!(
            &source[report.length_value_range.start..report.length_value_range.end],
            b"7 0 R"
        );
        assert_eq!(
            &source[report.stream_data_start_byte_offset..report.stream_data_end_byte_offset],
            b"hello world!"
        );
    }

    #[test]
    fn rejects_bad_length_shapes_and_delegated_stream_start_failure() {
        assert_eq!(
            reason(
                b"<< /Other 7 0 R >>",
                b"X\nendstream\n",
                b"7 0 obj\n1\nendobj\n"
            ),
            IndirectLengthContentStreamDataExtentInspectionRejection::MissingLength
        );
        assert!(matches!(
            reason(
                b"<< /Length 7 0 R /Other 0 /Length 7 0 R >>",
                b"X\nendstream\n",
                b"7 0 obj\n1\nendobj\n"
            ),
            IndirectLengthContentStreamDataExtentInspectionRejection::DuplicateLength { .. }
        ));
        assert_eq!(
            reason(
                b"<< /Length 1 >>",
                b"X\nendstream\n",
                b"7 0 obj\n1\nendobj\n"
            ),
            IndirectLengthContentStreamDataExtentInspectionRejection::NonReferenceLength {
                value_kind: DictionaryValueKind::NumberLike,
            }
        );
        assert_eq!(
            reason(
                b"[ /Length 7 0 R ]",
                b"X\nendstream\n",
                b"7 0 obj\n1\nendobj\n"
            ),
            IndirectLengthContentStreamDataExtentInspectionRejection::StreamStart {
                stream_start_reason: ContentStreamStartInspectionRejection::NonDictionaryBody {
                    token_kind: IndirectObjectBodyLeadingTokenKind::ArrayOpen,
                },
            }
        );
    }

    fn expect_resolution_reason(
        dictionary: &[u8],
        length_object: &[u8],
        state: ClassicXrefEntryState,
        expected: ClassicXrefIntegerObjectResolutionRejection,
    ) {
        let (source, xref, offset) = fixture(dictionary, b"X\nendstream\n", length_object, state);
        let reason = inspect_indirect_length_content_stream_data_extent(&source, &xref, offset)
            .expect_err("indirect length resolution should reject")
            .reason;
        assert!(matches!(
            reason,
            IndirectLengthContentStreamDataExtentInspectionRejection::LengthResolution { .. }
        ));
        if let IndirectLengthContentStreamDataExtentInspectionRejection::LengthResolution {
            length_resolution_reason,
        } = reason
        {
            assert_eq!(length_resolution_reason, expected);
        }
    }

    #[test]
    fn propagates_indirect_integer_resolution_failures() {
        expect_resolution_reason(
            b"<< /Length 7 0 R >>",
            b"7 0 obj\n1\nendobj\n",
            ClassicXrefEntryState::Free,
            ClassicXrefIntegerObjectResolutionRejection::FreeObject,
        );
        expect_resolution_reason(
            b"<< /Length 7 1 R >>",
            b"7 0 obj\n1\nendobj\n",
            ClassicXrefEntryState::InUse,
            ClassicXrefIntegerObjectResolutionRejection::ReferenceMismatch {
                resolved: IndirectRef {
                    object_number: 7,
                    generation: 0,
                },
            },
        );
        expect_resolution_reason(
            b"<< /Length 7 0 R >>",
            b"7 0 obj\n1.0\nendobj\n",
            ClassicXrefEntryState::InUse,
            ClassicXrefIntegerObjectResolutionRejection::MalformedInteger,
        );

        let (source, _, offset) = fixture(
            b"<< /Length 7 0 R >>",
            b"X\nendstream\n",
            b"7 0 obj\n1\nendobj\n",
            ClassicXrefEntryState::InUse,
        );
        let not_found_xref = classic_inspection(vec![classic_subsection(
            0,
            vec![classic_entry(0, 65535, 0, ClassicXrefEntryState::Free)],
        )]);
        let error =
            inspect_indirect_length_content_stream_data_extent(&source, &not_found_xref, offset)
                .expect_err("absent length object should reject");
        assert_eq!(
            error.reason,
            IndirectLengthContentStreamDataExtentInspectionRejection::LengthResolution {
                length_resolution_reason:
                    ClassicXrefIntegerObjectResolutionRejection::ObjectNotFound,
            }
        );
    }

    #[test]
    fn propagates_ambiguous_length_object_resolution() {
        let (source, xref, offset) = fixture(
            b"<< /Length 7 0 R >>",
            b"X\nendstream\n",
            b"7 0 obj\n1\nendobj\n",
            ClassicXrefEntryState::InUse,
        );
        let length_offset = xref.subsections[1].entries[0].byte_offset;
        let ambiguous_xref = classic_inspection(vec![
            classic_subsection(
                7,
                vec![classic_entry(
                    7,
                    0,
                    length_offset,
                    ClassicXrefEntryState::InUse,
                )],
            ),
            classic_subsection(
                7,
                vec![classic_entry(
                    7,
                    0,
                    length_offset,
                    ClassicXrefEntryState::InUse,
                )],
            ),
        ]);
        let error =
            inspect_indirect_length_content_stream_data_extent(&source, &ambiguous_xref, offset)
                .expect_err("ambiguous length object should reject");
        assert_eq!(
            error.reason,
            IndirectLengthContentStreamDataExtentInspectionRejection::LengthResolution {
                length_resolution_reason:
                    ClassicXrefIntegerObjectResolutionRejection::AmbiguousObject,
            }
        );
    }

    #[test]
    fn rejects_overflow_out_of_bounds_and_bad_endstream() {
        let overflow_object = format!("7 0 obj\n{}\nendobj\n", usize::MAX).into_bytes();
        assert_eq!(
            reason(b"<< /Length 7 0 R >>", b"X\nendstream\n", &overflow_object),
            IndirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOverflow
        );
        assert_eq!(
            reason(
                b"<< /Length 7 0 R >>",
                b"X\nendstream\n",
                b"7 0 obj\n99\nendobj\n"
            ),
            IndirectLengthContentStreamDataExtentInspectionRejection::StreamDataEndOutOfBounds
        );
        assert_eq!(
            reason(b"<< /Length 7 0 R >>", b"X", b"7 0 obj\n1\nendobj\n"),
            IndirectLengthContentStreamDataExtentInspectionRejection::InvalidEndstreamEol {
                eol_issue: StreamEolIssue::NotEndOfLine,
            }
        );
        assert_eq!(
            reason(
                b"<< /Length 7 0 R >>",
                b"X\rendstream\n",
                b"7 0 obj\n1\nendobj\n"
            ),
            IndirectLengthContentStreamDataExtentInspectionRejection::InvalidEndstreamEol {
                eol_issue: StreamEolIssue::LoneCarriageReturn,
            }
        );
        for tail in [&b"X\nendstram\n"[..], &b"X\nendstream0\n"[..]] {
            assert_eq!(
                reason(b"<< /Length 7 0 R >>", tail, b"7 0 obj\n1\nendobj\n"),
                IndirectLengthContentStreamDataExtentInspectionRejection::MissingEndstreamKeyword
            );
        }
    }

    #[test]
    fn report_does_not_retain_source_bytes() {
        let (source, xref, offset) = fixture(
            b"<< /Secret (do-not-copy) /Length 7 0 R >>",
            b"secret-payload\nendstream\n",
            b"7 0 obj\n14\nendobj\n",
            ClassicXrefEntryState::InUse,
        );
        let report = inspect_indirect_length_content_stream_data_extent(&source, &xref, offset)
            .expect("indirect-length stream extent should inspect");
        let debug_report = format!("{report:?}");
        assert!(!debug_report.contains("secret-payload"));
        assert!(!debug_report.contains("do-not-copy"));
        assert!(!debug_report.contains("Secret"));
    }

    #[test]
    fn composes_from_resolved_page_content_target() {
        let prefix = b"%PDF-1.7\n";
        let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
        let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
        let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R >>\nendobj\n";
        let content = b"5 0 obj\n<< /Length 7 0 R >>\nstream\nABCDEFGHIJK\nendstream\nendobj\n";
        let length = b"7 0 obj\n11\nendobj\n";
        let catalog_offset = prefix.len();
        let pages_offset = catalog_offset + catalog.len();
        let page_offset = pages_offset + pages.len();
        let content_offset = page_offset + page.len();
        let length_offset = content_offset + content.len();
        let xref_offset = length_offset + length.len();
        let source = format!(
            "{}{}{}{}{}{}xref\n0 8\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{pages_offset:010} 00000 n \n{page_offset:010} 00000 n \n0000000000 00000 f \n{content_offset:010} 00000 n \n0000000000 00000 f \n{length_offset:010} 00000 n \ntrailer\n<< /Size 8 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            String::from_utf8_lossy(prefix),
            String::from_utf8_lossy(catalog),
            String::from_utf8_lossy(pages),
            String::from_utf8_lossy(page),
            String::from_utf8_lossy(content),
            String::from_utf8_lossy(length),
        )
        .into_bytes();

        let xref_pos = source
            .windows(b"xref".len())
            .position(|w| w == b"xref")
            .expect("xref keyword present");
        let xref = inspect_classic_xref_table(&source, xref_pos).expect("xref should inspect");
        let root = inspect_classic_xref_trailer_root(&source, xref.trailer_byte_offset)
            .expect("root should inspect");
        let catalog = inspect_page_tree_reference_target(&source, &xref, root.root_reference)
            .expect("catalog should resolve");
        let catalog_pages = inspect_catalog_pages(&source, catalog.object_byte_offset)
            .expect("catalog pages should inspect");
        let pages =
            inspect_page_tree_reference_target(&source, &xref, catalog_pages.pages_reference)
                .expect("page tree should resolve");
        let kids =
            inspect_page_tree_kids(&source, pages.object_byte_offset).expect("kids should inspect");
        let page = inspect_page_tree_reference_target(&source, &xref, kids.kids[0].reference)
            .expect("page should resolve");
        let contents = inspect_page_contents(&source, page.object_byte_offset)
            .expect("contents should inspect");
        let targets = inspect_page_content_targets(&source, &xref, &contents);
        assert_eq!(
            targets.entries[0],
            PageContentTargetInspection::Resolved {
                content_reference: contents.contents[0],
                object_byte_offset: content_offset,
                xref_generation: 0,
            }
        );

        let extent =
            inspect_indirect_length_content_stream_data_extent(&source, &xref, content_offset)
                .expect("resolved indirect-length stream extent should inspect");
        assert_eq!(extent.length, 11);
        assert_eq!(
            &source[extent.stream_data_start_byte_offset..extent.stream_data_end_byte_offset],
            b"ABCDEFGHIJK"
        );
    }
}
