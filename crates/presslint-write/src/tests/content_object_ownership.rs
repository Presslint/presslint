use presslint_pdf::{
    DocumentAccessBackend, IndirectObjectEditDisposition, IndirectObjectOwnership, IndirectRef,
    ObjectConsumerIndexInspection, ObjectConsumerIndexLimit, ObjectConsumerIndexTruncation,
    ObjectConsumerReferrer, ObjectConsumerUnresolvedEdge, ObjectConsumersEntry, ObjectLookup,
    ObjectResolutionRejection, ObjectStreamCacheReport, PdfName, SkippedObjectConsumerScan,
    inspect_document_access, inspect_document_page_content_extents_with_lookup,
};

use crate::content_object_ownership::{ContentObjectOwnershipIndex, inspection_is_complete};

fn reference(object_number: u32) -> IndirectRef {
    IndirectRef {
        object_number,
        generation: 0,
    }
}

fn complete_inspection() -> ObjectConsumerIndexInspection {
    ObjectConsumerIndexInspection {
        byte_len: 0,
        entries: Vec::new(),
        unresolved_edges: Vec::new(),
        skipped: Vec::new(),
        truncations: Vec::new(),
        expanded_node_count: 0,
        recorded_pair_count: 0,
        object_stream_cache: ObjectStreamCacheReport {
            budget_bytes: 0,
            cached_container_count: 0,
            cached_byte_count: 0,
            dropped_over_budget: true,
        },
        unreferenced: vec![reference(99)],
    }
}

fn single_page_with_content() -> (Vec<u8>, presslint_pdf::DocumentPageContentExtentsInspection) {
    let bodies: [&[u8]; 4] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /Contents 4 0 R >>",
        b"<< /Length 4 >>\nstream\nq Q\nendstream",
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in bodies.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        bytes.extend_from_slice(body);
        bytes.extend_from_slice(b"\nendobj\n");
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF").as_bytes(),
    );

    let access = inspect_document_access(&bytes).expect("fixture opens");
    let lookup = match &access.backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    };
    let document = inspect_document_page_content_extents_with_lookup(
        &bytes,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .expect("fixture content opens");
    (bytes, document)
}

fn page_referrer() -> ObjectConsumerReferrer {
    ObjectConsumerReferrer::Page {
        page_index: 0,
        page: reference(3),
    }
}

#[test]
fn incomplete_snapshot_vetoes_a_valid_unique_direct_owner() {
    let (_bytes, document) = single_page_with_content();
    let mut inspection = complete_inspection();
    inspection.entries.push(ObjectConsumersEntry {
        target: reference(4),
        referrers: vec![page_referrer()],
    });
    inspection.truncations.push(ObjectConsumerIndexTruncation {
        referrer: Some(page_referrer()),
        target: Some(reference(4)),
        limit: ObjectConsumerIndexLimit::MaxTraversalDepth { max_depth: 1 },
    });

    let ownership = ContentObjectOwnershipIndex::new(&document.pages, inspection);
    let (occurrences, decision) = ownership.decide(reference(4));
    assert_eq!(occurrences, 1);
    assert_eq!(decision.ownership, IndirectObjectOwnership::Unproven);
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::PrivateCopy
    );
}

#[test]
fn missing_target_entry_vetoes_a_valid_unique_direct_owner() {
    let (_bytes, document) = single_page_with_content();
    let ownership = ContentObjectOwnershipIndex::new(&document.pages, complete_inspection());
    let (occurrences, decision) = ownership.decide(reference(4));
    assert_eq!(occurrences, 1);
    assert_eq!(decision.ownership, IndirectObjectOwnership::Unproven);
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::PrivateCopy
    );
}

#[test]
fn trailer_key_user_vetoes_the_matching_page_user() {
    let (_bytes, document) = single_page_with_content();
    let mut inspection = complete_inspection();
    inspection.entries.push(ObjectConsumersEntry {
        target: reference(4),
        referrers: vec![
            ObjectConsumerReferrer::TrailerKey {
                key: PdfName(b"Info".to_vec()),
            },
            page_referrer(),
        ],
    });

    let ownership = ContentObjectOwnershipIndex::new(&document.pages, inspection);
    let (occurrences, decision) = ownership.decide(reference(4));
    assert_eq!(occurrences, 1);
    assert_eq!(decision.ownership, IndirectObjectOwnership::Unproven);
    assert_eq!(
        decision.disposition,
        IndirectObjectEditDisposition::PrivateCopy
    );
}

#[test]
fn diagnostics_cache_drop_and_unreferenced_scan_skip_do_not_poison() {
    let mut inspection = complete_inspection();
    inspection
        .skipped
        .push(SkippedObjectConsumerScan::UnreferencedEntryUnresolvable { object_number: 99 });
    assert!(inspection_is_complete(&inspection));
}

#[test]
fn every_edge_hiding_scan_skip_poisons() {
    let page = ObjectConsumerReferrer::Page {
        page_index: 0,
        page: reference(3),
    };
    let skips = [
        SkippedObjectConsumerScan::NewestTrailerDictionary {
            section_byte_offset: 10,
        },
        SkippedObjectConsumerScan::CatalogDictionary,
        SkippedObjectConsumerScan::BodyScan {
            target: reference(4),
            referrer: page.clone(),
        },
        SkippedObjectConsumerScan::ReferenceShapes {
            target: Some(reference(4)),
            referrer: page,
            skipped_references: Vec::new(),
        },
    ];
    for skip in skips {
        let mut inspection = complete_inspection();
        inspection.skipped.push(skip);
        assert!(!inspection_is_complete(&inspection));
    }
}

#[test]
fn unresolved_edges_and_every_truncation_poison() {
    let page = ObjectConsumerReferrer::Page {
        page_index: 0,
        page: reference(3),
    };
    let mut unresolved = complete_inspection();
    unresolved
        .unresolved_edges
        .push(ObjectConsumerUnresolvedEdge {
            target: reference(88),
            referrer: page.clone(),
            resolution_reason: ObjectResolutionRejection::GenerationMismatch {
                requested_generation: 0,
                xref_generation: 1,
            },
        });
    assert!(!inspection_is_complete(&unresolved));

    let limits = [
        ObjectConsumerIndexLimit::MaxTraversalDepth { max_depth: 1 },
        ObjectConsumerIndexLimit::MaxVisitedNodes {
            max_visited_nodes: 1,
        },
        ObjectConsumerIndexLimit::MaxExpandedNodes {
            max_expanded_nodes: 1,
        },
        ObjectConsumerIndexLimit::MaxRecordedPairs {
            max_recorded_pairs: 1,
        },
        ObjectConsumerIndexLimit::MaxBodyReferences { max_references: 1 },
        ObjectConsumerIndexLimit::MaxDecodedObjectStreamBytes {
            decoded_length: 2,
            max_decoded_object_stream_bytes: 1,
        },
    ];
    for limit in limits {
        let mut inspection = complete_inspection();
        inspection.truncations.push(ObjectConsumerIndexTruncation {
            referrer: Some(page.clone()),
            target: Some(reference(4)),
            limit,
        });
        assert!(!inspection_is_complete(&inspection));
    }
}
