#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use crate::{
    DocumentAccess, DocumentAccessBackend, DocumentAccessRejection, ObjectLookup,
    ObjectResolutionRejection, ResolvedObjectPosition, SkippedPageTreeLeafReason,
    inspect_document_access, inspect_document_page_content_extents_with_lookup,
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
