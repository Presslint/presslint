#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use super::indirect_ref;

use serde_harness::{from_serde_value, serde_value};

use crate::{
    ClassicDocumentAccess, ClassicDocumentAccessRejection, ObjectResolutionError,
    ObjectResolutionRejection, SkippedPageTreeLeafReason, inspect_classic_document_access,
};

/// Extract the delegated object-resolution error from a `RootObject` rejection.
fn root_object_error(reason: ClassicDocumentAccessRejection) -> Option<ObjectResolutionError> {
    match reason {
        ClassicDocumentAccessRejection::RootObject { error } => Some(error),
        _ => None,
    }
}

/// Extract the delegated object-resolution error from a `PagesObject` rejection.
fn pages_object_error(reason: ClassicDocumentAccessRejection) -> Option<ObjectResolutionError> {
    match reason {
        ClassicDocumentAccessRejection::PagesObject { error } => Some(error),
        _ => None,
    }
}

/// One synthetic indirect object plus the xref entry the fixture should emit for
/// it. The xref `number`/`xref_generation` are decoupled from the body header so
/// tests can exercise header-mismatch paths.
struct ObjSpec {
    number: u32,
    xref_generation: u16,
    body: &'static [u8],
}

const fn object(number: u32, xref_generation: u16, body: &'static [u8]) -> ObjSpec {
    ObjSpec {
        number,
        xref_generation,
        body,
    }
}

/// Assemble a classic-xref PDF: a header, the object bodies, a classic xref
/// table with one single-entry subsection per object, and a trailer/`startxref`
/// pointing at the table. Returns the source and per-object byte offsets.
fn assemble(objects: &[ObjSpec], trailer_dict: &str) -> (Vec<u8>, Vec<usize>) {
    let mut source = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for object in objects {
        offsets.push(source.len());
        source.extend_from_slice(object.body);
    }

    let xref_offset = source.len();
    source.extend_from_slice(b"xref\n");
    for (object, offset) in objects.iter().zip(&offsets) {
        source.extend_from_slice(format!("{} 1\n", object.number).as_bytes());
        source.extend_from_slice(
            format!("{offset:010} {gen:05} n \n", gen = object.xref_generation).as_bytes(),
        );
    }
    source.extend_from_slice(
        format!("trailer\n{trailer_dict}\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
    );

    (source, offsets)
}

fn two_page_document() -> (Vec<u8>, Vec<usize>) {
    assemble(
        &[
            object(
                1,
                0,
                b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
            ),
            object(
                2,
                0,
                b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n",
            ),
            object(3, 0, b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n"),
            object(4, 0, b"4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n"),
        ],
        "<< /Size 5 /Root 1 0 R >>",
    )
}

#[test]
fn composes_spine_over_two_page_document() {
    let (source, offsets) = two_page_document();

    let access = inspect_classic_document_access(&source)
        .expect("classic document-access spine should compose");

    assert_eq!(access.byte_len, source.len());
    assert_eq!(
        access.startxref.byte_offset,
        access.xref_table.table_byte_offset
    );
    assert_eq!(access.trailer_root.root_reference, indirect_ref(1, 0));
    assert_eq!(access.catalog.reference, indirect_ref(1, 0));
    assert_eq!(access.catalog.object_byte_offset, offsets[0]);
    assert_eq!(access.catalog_pages.pages_reference, indirect_ref(2, 0));
    assert_eq!(access.page_tree_root.reference, indirect_ref(2, 0));
    assert_eq!(access.page_tree_root.object_byte_offset, offsets[1]);

    assert_eq!(access.page_leaves.leaf_count(), 2);
    assert!(access.page_leaves.skipped.is_empty());
    assert_eq!(
        access
            .page_leaves
            .leaves
            .iter()
            .map(|leaf| (leaf.reference, leaf.object_byte_offset))
            .collect::<Vec<_>>(),
        vec![
            (indirect_ref(3, 0), offsets[2]),
            (indirect_ref(4, 0), offsets[3]),
        ]
    );
}

#[test]
fn reports_unsupported_xref_stream_without_attempting_object_map() {
    // `startxref` points at offset 9, the `1 0 obj` indirect object header that
    // follows the 9-byte `%PDF-1.7\n` prefix, so classification sees a stream.
    let source = b"%PDF-1.7\n1 0 obj\n<< /Type /XRef /Size 1 /W [ 1 1 1 ] /Root 1 0 R >>\nstream\n\x00\x00\x00\nendstream\nendobj\nstartxref\n9\n%%EOF\n".to_vec();

    let error = inspect_classic_document_access(&source)
        .expect_err("an xref-stream section must be an unsupported result");

    assert_eq!(
        error.reason,
        ClassicDocumentAccessRejection::UnsupportedXrefStream {
            object_number: 1,
            generation: 0,
        }
    );
}

#[test]
fn reports_missing_startxref() {
    let source = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec();

    let error = inspect_classic_document_access(&source)
        .expect_err("a source without startxref must fail at the first stage");

    assert!(matches!(
        error.reason,
        ClassicDocumentAccessRejection::StartXref { .. }
    ));
}

#[test]
fn reports_trailer_root_resolution_failure() {
    // The trailer `/Root` parses, but object 99 is absent from the xref table.
    let (source, _) = assemble(
        &[object(1, 0, b"1 0 obj\n<< /Type /Catalog >>\nendobj\n")],
        "<< /Size 2 /Root 99 0 R >>",
    );

    let error = inspect_classic_document_access(&source)
        .expect_err("an unresolved trailer /Root must fail the spine");

    let error = root_object_error(error.reason).expect("expected a root-object resolution failure");
    assert_eq!(error.reference, indirect_ref(99, 0));
    assert!(matches!(
        error.reason,
        ObjectResolutionRejection::UnresolvedXrefLocation { .. }
    ));
}

#[test]
fn reports_catalog_pages_resolution_failure() {
    // The catalog `/Pages` parses, but its target object is absent from the xref.
    let (source, _) = assemble(
        &[object(
            1,
            0,
            b"1 0 obj\n<< /Type /Catalog /Pages 99 0 R >>\nendobj\n",
        )],
        "<< /Size 2 /Root 1 0 R >>",
    );

    let error = inspect_classic_document_access(&source)
        .expect_err("an unresolved catalog /Pages must fail the spine");

    let error =
        pages_object_error(error.reason).expect("expected a pages-object resolution failure");
    assert_eq!(error.reference, indirect_ref(99, 0));
    assert!(matches!(
        error.reason,
        ObjectResolutionRejection::UnresolvedXrefLocation { .. }
    ));
}

#[test]
fn reports_root_xref_generation_mismatch() {
    // The xref entry for object 1 carries generation 5, but the trailer requests
    // `1 0 R`, so the first of the two generation checks fails.
    let (source, _) = assemble(
        &[object(1, 5, b"1 0 obj\n<< /Type /Catalog >>\nendobj\n")],
        "<< /Size 2 /Root 1 0 R >>",
    );

    let error = inspect_classic_document_access(&source)
        .expect_err("a root generation mismatch must fail the spine");

    let error = root_object_error(error.reason).expect("expected a root-object resolution failure");
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::GenerationMismatch {
            requested_generation: 0,
            xref_generation: 5,
        }
    );
}

#[test]
fn reports_object_header_mismatch_at_resolved_offset() {
    // The xref entry claims object 1, but the body header at that offset is
    // `7 0 obj`, so the second (header) reference check fails.
    let (source, _) = assemble(
        &[object(1, 0, b"7 0 obj\n<< /Type /Catalog >>\nendobj\n")],
        "<< /Size 2 /Root 1 0 R >>",
    );

    let error = inspect_classic_document_access(&source)
        .expect_err("a header object-number mismatch must fail the spine");

    let error = root_object_error(error.reason).expect("expected a root-object resolution failure");
    assert_eq!(
        error.reason,
        ObjectResolutionRejection::ObjectHeaderReferenceMismatch {
            header_reference: indirect_ref(7, 0),
        }
    );
}

#[test]
fn preserves_page_tree_leaf_skips_in_spine_report() {
    let (source, _) = assemble(
        &[
            object(
                1,
                0,
                b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
            ),
            object(
                2,
                0,
                b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n",
            ),
            object(3, 0, b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n"),
            object(4, 0, b"4 0 obj\n<< /Type /Annot >>\nendobj\n"),
        ],
        "<< /Size 5 /Root 1 0 R >>",
    );

    let access = inspect_classic_document_access(&source)
        .expect("spine should complete with a non-leaf kid skipped");

    assert_eq!(access.page_leaves.leaf_count(), 1);
    assert_eq!(access.page_leaves.leaves[0].reference, indirect_ref(3, 0));
    assert_eq!(access.page_leaves.skipped.len(), 1);
    assert_eq!(access.page_leaves.skipped[0].kid, indirect_ref(4, 0));
    assert!(matches!(
        access.page_leaves.skipped[0].reason,
        SkippedPageTreeLeafReason::OtherNodeType { .. }
    ));
}

#[test]
fn report_retains_no_source_bytes() {
    let (source, _) = assemble(
        &[
            object(
                1,
                0,
                b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /DoNotCopy (secret) >>\nendobj\n",
            ),
            object(
                2,
                0,
                b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
            ),
            object(3, 0, b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n"),
        ],
        "<< /Size 4 /Root 1 0 R >>",
    );

    let access = inspect_classic_document_access(&source).expect("spine should compose");
    let debug = format!("{access:?}");

    assert!(!debug.contains("DoNotCopy"));
    assert!(!debug.contains("secret"));
}

#[test]
fn serde_round_trips_access_report() {
    let (source, _) = two_page_document();
    let access = inspect_classic_document_access(&source).expect("spine should compose");

    let value = serde_value(&access).expect("access report should serialize");
    let restored: ClassicDocumentAccess =
        from_serde_value(value).expect("access report should deserialize");
    assert_eq!(restored, access);
}
