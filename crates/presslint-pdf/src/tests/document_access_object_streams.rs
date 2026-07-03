#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use crate::{
    DocumentAccess, DocumentAccessBackend, DocumentAccessRejection,
    DocumentPageContentExtentResult, DocumentPageContentExtentsInspection,
    MAX_XREF_STREAM_SECTION_DECODED_BYTES, ObjectLookup, ObjectResolutionRejection,
    ResolvedObjectPosition, SkippedPageTreeLeafReason, inspect_document_access,
    inspect_document_page_content_extents_resolved,
    inspect_document_page_content_extents_with_lookup, resolve_object,
};

fn xref_record(entry_type: u8, field2: usize, field3: u8) -> [u8; 4] {
    let [hi, lo] = u16::try_from(field2)
        .expect("test xref field fits u16")
        .to_be_bytes();
    [entry_type, hi, lo, field3]
}

fn object_stream(members: &[(usize, &[u8])]) -> Vec<u8> {
    let mut header = Vec::new();
    let mut offset = 0usize;
    for (object_number, body) in members {
        header.extend_from_slice(format!("{object_number} {offset} ").as_bytes());
        offset += body.len();
    }
    let first = header.len();
    let mut stream_body = header;
    for (_, body) in members {
        stream_body.extend_from_slice(body);
    }

    let mut object = format!(
        "5 0 obj\n<< /Type /ObjStm /N {} /First {first} /Length {} >>\nstream\n",
        members.len(),
        stream_body.len()
    )
    .into_bytes();
    object.extend_from_slice(&stream_body);
    object.extend_from_slice(b"\nendstream\nendobj\n");
    object
}

fn compressed_document(extra_objects: &[(&[u8], usize)]) -> Vec<u8> {
    let prefix = b"%PDF-1.5\n";
    let catalog: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
    let page_tree_root: &[u8] = b"<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>";
    let first_leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R >>";
    let second_leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R /Secret (do-not-copy) >>";
    let objstm = object_stream(&[
        (1, catalog),
        (2, page_tree_root),
        (3, first_leaf),
        (4, second_leaf),
    ]);

    let objstm_offset = prefix.len();
    let mut source = prefix.to_vec();
    source.extend_from_slice(&objstm);
    let mut extra_offsets = Vec::new();
    for (object, _) in extra_objects {
        extra_offsets.push(source.len());
        source.extend_from_slice(object);
    }
    let xref_offset = source.len();

    let size = 8 + extra_objects.len();
    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0));
    for index in 0..4 {
        records.extend_from_slice(&xref_record(2, 5, index));
    }
    records.extend_from_slice(&xref_record(1, objstm_offset, 0));
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(1, xref_offset, 0));
    for (offset, (_, generation)) in extra_offsets.iter().zip(extra_objects) {
        let generation = u8::try_from(*generation).expect("test generation fits u8");
        records.extend_from_slice(&xref_record(1, *offset, generation));
    }

    source.extend_from_slice(
        format!(
            "7 0 obj\n<< /Type /XRef /Size {size} /W [ 1 2 1 ] /Index [ 0 {size} ] /Root 1 0 R /Length {} >>\nstream\n",
            records.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&records);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    source
}

fn invalid_child_document() -> Vec<u8> {
    let prefix = b"%PDF-1.5\n";
    let catalog: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
    let page_tree_root: &[u8] = b"<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>";
    let first_leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R >>";
    let objstm = object_stream(&[(1, catalog), (2, page_tree_root), (3, first_leaf)]);
    let invalid_objstm =
        b"8 0 obj\n<< /Type /ObjStm /N 1 /Length 0 >>\nstream\n\nendstream\nendobj\n";

    let objstm_offset = prefix.len();
    let invalid_offset = objstm_offset + objstm.len();
    let xref_offset = invalid_offset + invalid_objstm.len();
    let size = 9;

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(2, 5, 0));
    records.extend_from_slice(&xref_record(2, 5, 1));
    records.extend_from_slice(&xref_record(2, 5, 2));
    records.extend_from_slice(&xref_record(2, 8, 0));
    records.extend_from_slice(&xref_record(1, objstm_offset, 0));
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(1, xref_offset, 0));
    records.extend_from_slice(&xref_record(1, invalid_offset, 0));

    let mut source = prefix.to_vec();
    source.extend_from_slice(&objstm);
    source.extend_from_slice(invalid_objstm);
    source.extend_from_slice(
        format!(
            "7 0 obj\n<< /Type /XRef /Size {size} /W [ 1 2 1 ] /Index [ 0 {size} ] /Root 1 0 R /Length {} >>\nstream\n",
            records.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&records);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    source
}

fn mixed_uncompressed_root_compressed_leaf_document() -> Vec<u8> {
    let prefix = b"%PDF-1.5\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let page_tree_root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n";
    let first_leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R >>";
    let second_leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R >>";
    let objstm = object_stream(&[(3, first_leaf), (4, second_leaf)]);

    let catalog_offset = prefix.len();
    let page_tree_root_offset = catalog_offset + catalog.len();
    let objstm_offset = page_tree_root_offset + page_tree_root.len();
    let xref_offset = objstm_offset + objstm.len();
    let size = 8;

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(1, catalog_offset, 0));
    records.extend_from_slice(&xref_record(1, page_tree_root_offset, 0));
    records.extend_from_slice(&xref_record(2, 5, 0));
    records.extend_from_slice(&xref_record(2, 5, 1));
    records.extend_from_slice(&xref_record(1, objstm_offset, 0));
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(1, xref_offset, 0));

    let mut source = prefix.to_vec();
    source.extend_from_slice(catalog);
    source.extend_from_slice(page_tree_root);
    source.extend_from_slice(&objstm);
    source.extend_from_slice(
        format!(
            "7 0 obj\n<< /Type /XRef /Size {size} /W [ 1 2 1 ] /Index [ 0 {size} ] /Root 1 0 R /Length {} >>\nstream\n",
            records.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&records);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    source
}

/// Build an uncompressed indirect content-stream object with a matching
/// `/Length`.
fn content_object(number: u32, data: &[u8]) -> Vec<u8> {
    let mut object = format!("{number} 0 obj\n<< /Length {} >>\nstream\n", data.len()).into_bytes();
    object.extend_from_slice(data);
    object.extend_from_slice(b"\nendstream\nendobj\n");
    object
}

/// A page tree whose ROOT `/Pages` node (object `2`) is uncompressed and whose
/// INTERMEDIATE `/Pages` node (object `3`) is a type-2 compressed member of
/// `/ObjStm` `5`, with two UNCOMPRESSED leaf `/Page` objects (`6`, `7`) carrying
/// real content streams. The offset-only walk would read a header at the
/// fabricated offset `0` for the compressed intermediate node and skip it; the
/// resolved walk navigates it and reaches the uncompressed leaves.
fn compressed_intermediate_node_document() -> Vec<u8> {
    let prefix = b"%PDF-1.5\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_vec();
    let root = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 2 >>\nendobj\n".to_vec();
    let intermediate: &[u8] = b"<< /Type /Pages /Kids [ 6 0 R 7 0 R ] /Count 2 >>";
    let objstm = object_stream(&[(3, intermediate)]);
    let leaf6 = b"6 0 obj\n<< /Type /Page /Parent 3 0 R /Contents 8 0 R >>\nendobj\n".to_vec();
    let leaf7 = b"7 0 obj\n<< /Type /Page /Parent 3 0 R /Contents 9 0 R >>\nendobj\n".to_vec();
    let content8 = content_object(8, b"0 0 1 rg\n0 0 9 9 re\nf\n");
    let content9 = content_object(9, b"1 0 0 rg\n0 0 4 4 re\nf\n");

    let mut source = prefix.to_vec();
    let catalog_offset = source.len();
    source.extend_from_slice(&catalog);
    let root_offset = source.len();
    source.extend_from_slice(&root);
    let objstm_offset = source.len();
    source.extend_from_slice(&objstm);
    let leaf6_offset = source.len();
    source.extend_from_slice(&leaf6);
    let leaf7_offset = source.len();
    source.extend_from_slice(&leaf7);
    let content8_offset = source.len();
    source.extend_from_slice(&content8);
    let content9_offset = source.len();
    source.extend_from_slice(&content9);
    let xref_offset = source.len();
    let size = 10;

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(1, catalog_offset, 0));
    records.extend_from_slice(&xref_record(1, root_offset, 0));
    records.extend_from_slice(&xref_record(2, 5, 0));
    records.extend_from_slice(&xref_record(1, xref_offset, 0));
    records.extend_from_slice(&xref_record(1, objstm_offset, 0));
    records.extend_from_slice(&xref_record(1, leaf6_offset, 0));
    records.extend_from_slice(&xref_record(1, leaf7_offset, 0));
    records.extend_from_slice(&xref_record(1, content8_offset, 0));
    records.extend_from_slice(&xref_record(1, content9_offset, 0));

    source.extend_from_slice(
        format!(
            "4 0 obj\n<< /Type /XRef /Size {size} /W [ 1 2 1 ] /Index [ 0 {size} ] /Root 1 0 R /Length {} >>\nstream\n",
            records.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&records);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    source
}

fn lookup_from_access(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
    match backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    }
}

/// Resolve the page-tree root and run the resolved content-extents bridge the way
/// `build_pdf_inventory` does.
fn resolved_extents(source: &[u8]) -> DocumentPageContentExtentsInspection {
    let access = inspect_document_access(source).expect("resolved spine should resolve");
    let lookup = lookup_from_access(&access.backend);
    let resolved_root = resolve_object(
        source,
        lookup,
        access.page_tree_root.reference,
        MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    )
    .expect("page-tree root should resolve to body-aware data");
    inspect_document_page_content_extents_resolved(
        source,
        lookup,
        &resolved_root,
        MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    )
    .expect("resolved content-extents bridge should navigate")
}

#[test]
fn resolved_content_extents_navigates_compressed_intermediate_pages_node() {
    let source = compressed_intermediate_node_document();

    // The resolved bridge navigates the compressed intermediate `/Pages` node and
    // inspects the two uncompressed leaves end to end.
    let report = resolved_extents(&source);
    assert_eq!(report.page_count(), 2);
    assert_eq!(report.located_page_count(), 2);
    assert_eq!(report.leaves.leaves[0].reference.object_number, 6);
    assert_eq!(report.leaves.leaves[1].reference.object_number, 7);
    assert!(report.pages[0].is_located());
    assert!(report.pages[1].is_located());
    assert!(matches!(
        report.pages[0].result,
        DocumentPageContentExtentResult::Inspected { .. }
    ));

    // The pre-change offset-only bridge cannot expand the compressed intermediate
    // node (offset `0`), so it enumerates zero leaves and records one skip.
    let access = inspect_document_access(&source).expect("spine should resolve");
    let lookup = lookup_from_access(&access.backend);
    let legacy = inspect_document_page_content_extents_with_lookup(
        &source,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .expect("offset-only bridge still succeeds at the uncompressed root");
    assert!(legacy.pages.is_empty());
    assert!(legacy.leaves.leaves.is_empty());
    assert_eq!(legacy.leaves.skipped.len(), 1);
}

#[test]
fn resolved_content_extents_reports_compressed_leaves_honestly() {
    let source = compressed_document(&[]);

    let report = resolved_extents(&source);
    assert_eq!(report.page_count(), 2);
    // A compressed leaf is never "located": no offset-0 parse is attempted.
    assert_eq!(report.located_page_count(), 0);
    assert!(!report.pages[0].is_located());
    assert!(!report.pages[1].is_located());
    assert_eq!(
        report.pages[0].result,
        DocumentPageContentExtentResult::CompressedLeaf {
            object_stream_number: 5,
            index_within_object_stream: 2,
        }
    );
    assert_eq!(
        report.pages[1].result,
        DocumentPageContentExtentResult::CompressedLeaf {
            object_stream_number: 5,
            index_within_object_stream: 3,
        }
    );

    // The honest compressed-leaf report serialises with its own tag and never
    // leaks the compressed leaf body bytes.
    let value = serde_value(&report).expect("resolved report should serialize");
    assert!(format!("{value:?}").contains(r#"("status", String("compressed_leaf"))"#));
    let restored: DocumentPageContentExtentsInspection =
        from_serde_value(value).expect("resolved report should deserialize");
    assert_eq!(restored, report);
    assert!(!format!("{report:?}").contains("do-not-copy"));
}

#[test]
fn legacy_content_extents_hard_fail_on_compressed_root_is_navigated_when_resolved() {
    let source = compressed_document(&[]);
    let access = inspect_document_access(&source).expect("spine should resolve compressed root");
    // A compressed root carries the fabricated offset `0`.
    assert_eq!(access.page_tree_root.object_byte_offset, 0);
    let lookup = lookup_from_access(&access.backend);

    // Feeding offset `0` into the offset-only bridge is the reproduced hard fail.
    let legacy = inspect_document_page_content_extents_with_lookup(
        &source,
        lookup,
        access.page_tree_root.object_byte_offset,
    );
    assert!(legacy.is_err());

    // The resolved bridge navigates the same compressed root instead.
    let resolved = resolved_extents(&source);
    assert_eq!(resolved.page_count(), 2);
}

#[test]
fn document_access_enumerates_compressed_catalog_pages_and_leaves() {
    let source = compressed_document(&[]);

    let access =
        inspect_document_access(&source).expect("compressed structural spine should resolve");

    assert_eq!(access.catalog.reference.object_number, 1);
    assert_eq!(
        access.catalog.position,
        ResolvedObjectPosition::Compressed {
            object_stream_number: 5,
            index_within_object_stream: 0,
        }
    );
    assert_eq!(access.page_tree_root.reference.object_number, 2);
    assert_eq!(access.page_leaves.leaf_count(), 2);
    assert_eq!(access.page_leaves.leaves[0].reference.object_number, 3);
    assert_eq!(access.page_leaves.leaves[1].reference.object_number, 4);
    let value = serde_value(&access).expect("access report should serialize");
    assert!(format!("{value:?}").contains(r#"("kind", String("compressed"))"#));
    let restored: DocumentAccess =
        from_serde_value(value).expect("access report should deserialize");
    assert_eq!(restored, access);
    assert!(!format!("{access:?}").contains("do-not-copy"));
}

#[test]
fn compressed_root_object_stream_failure_is_spine_error() {
    let mut source = compressed_document(&[]);
    let first = source
        .windows(b"/First".len())
        .position(|window| window == b"/First")
        .expect("fixture should contain /First");
    source[first..first + b"/First".len()].copy_from_slice(b"/Furst");

    let error = inspect_document_access(&source)
        .expect_err("unresolvable compressed root should reject the spine");

    assert!(matches!(
        error.reason,
        DocumentAccessRejection::RootObject {
            error: crate::ObjectResolutionError {
                reason: ObjectResolutionRejection::ObjectStreamMemberExtraction { .. },
                ..
            }
        }
    ));
}

#[test]
fn invalid_compressed_child_is_non_fatal_leaf_skip() {
    let source = invalid_child_document();

    let access =
        inspect_document_access(&source).expect("invalid compressed child should be a skip");

    assert_eq!(access.page_leaves.leaf_count(), 1);
    assert_eq!(access.page_leaves.leaves[0].reference.object_number, 3);
    assert_eq!(access.page_leaves.skipped.len(), 1);
    assert!(matches!(
        access.page_leaves.skipped[0].reason,
        SkippedPageTreeLeafReason::UnresolvedTarget { .. }
    ));
}

#[test]
fn mixed_uncompressed_root_enumerates_compressed_leaf_pages() {
    let source = mixed_uncompressed_root_compressed_leaf_document();

    let access = inspect_document_access(&source)
        .expect("uncompressed root should resolve compressed child leaves");

    assert_eq!(access.catalog.reference.object_number, 1);
    assert!(matches!(
        access.catalog.position,
        ResolvedObjectPosition::Uncompressed { .. }
    ));
    assert_eq!(access.page_tree_root.reference.object_number, 2);
    assert!(matches!(
        access.page_tree_root.position,
        ResolvedObjectPosition::Uncompressed { .. }
    ));
    assert_eq!(access.page_leaves.leaf_count(), 2);
    assert!(access.page_leaves.skipped.is_empty());
    assert_eq!(access.page_leaves.leaves[0].reference.object_number, 3);
    assert_eq!(
        access.page_leaves.leaves[0].position,
        ResolvedObjectPosition::Compressed {
            object_stream_number: 5,
            index_within_object_stream: 0,
        }
    );
    assert_eq!(access.page_leaves.leaves[1].reference.object_number, 4);
    assert_eq!(
        access.page_leaves.leaves[1].position,
        ResolvedObjectPosition::Compressed {
            object_stream_number: 5,
            index_within_object_stream: 1,
        }
    );
}

#[test]
fn legacy_content_extents_do_not_inspect_compressed_leaf_pages_at_offset_zero() {
    let source = mixed_uncompressed_root_compressed_leaf_document();

    let access = inspect_document_access(&source)
        .expect("resolved document-access spine should enumerate compressed leaves");
    assert_eq!(access.page_leaves.leaf_count(), 2);

    assert!(matches!(
        access.backend,
        DocumentAccessBackend::XrefStreamSection { .. }
    ));
    let DocumentAccessBackend::XrefStreamSection { section } = &access.backend else {
        return;
    };
    let extents = inspect_document_page_content_extents_with_lookup(
        &source,
        ObjectLookup::XrefStreamSection(section),
        access.page_tree_root.object_byte_offset,
    )
    .expect("legacy content-extents path should keep compressed leaves as skips");

    assert!(extents.pages.is_empty());
    assert!(extents.leaves.leaves.is_empty());
    assert_eq!(extents.leaves.skipped.len(), 2);
    assert!(extents.leaves.skipped.iter().all(|skip| matches!(
        skip.reason,
        SkippedPageTreeLeafReason::UnresolvedTarget { .. }
    )));
}
