use crate::{
    CatalogPagesInspectionRejection, ClassicXrefObjectLocation, DictionaryValueKind,
    IndirectObjectDictionaryInspectionRejection, IndirectObjectHeaderInspectionRejection,
    IndirectRef, IndirectReferenceInspectionRejection, inspect_catalog_pages,
    inspect_classic_xref_table, inspect_classic_xref_trailer_root, resolve_classic_xref_object,
};

#[test]
fn catalog_pages_reports_pages_reference_and_ranges() {
    let source = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";

    let report = inspect_catalog_pages(source, 0).expect("pages should inspect");

    assert_eq!(
        report.catalog_dictionary.reference,
        IndirectRef {
            object_number: 1,
            generation: 0,
        }
    );
    assert_eq!(
        &source[report.pages_key_range.start..report.pages_key_range.end],
        b"/Pages"
    );
    assert_eq!(
        &source[report.pages_value_range.start..report.pages_value_range.end],
        b"2 0 R"
    );
    assert_eq!(
        report.pages_reference,
        IndirectRef {
            object_number: 2,
            generation: 0,
        }
    );
}

#[test]
fn catalog_pages_skips_leading_whitespace_before_catalog_header() {
    let source = b"\t \r\n1 0 obj << /Type /Catalog /Pages 2 0 R >>\nendobj\n";

    let report = inspect_catalog_pages(source, 0).expect("pages should inspect");

    assert_eq!(report.catalog_dictionary.header_range.start, 4);
    assert_eq!(
        report.pages_reference,
        IndirectRef {
            object_number: 2,
            generation: 0,
        }
    );
}

#[test]
fn catalog_pages_propagates_catalog_dictionary_rejection() {
    let source = b"xref\n0 1\n0000000000 65535 f \n";

    let error = inspect_catalog_pages(source, 0).expect_err("non-object should reject");

    assert_eq!(error.byte_offset, 0);
    assert_eq!(error.byte_len, source.len());
    assert_eq!(error.catalog_header_byte_offset, None);
    assert_eq!(
        error.reason,
        CatalogPagesInspectionRejection::CatalogDictionary {
            catalog_dictionary_reason: IndirectObjectDictionaryInspectionRejection::Header {
                header_reason: IndirectObjectHeaderInspectionRejection::MalformedHeader,
            },
        }
    );
}

#[test]
fn catalog_pages_rejects_missing_pages() {
    let source = b"1 0 obj\n<< /Type /Catalog /Pages#20 2 0 R >>\nendobj\n";

    let error = inspect_catalog_pages(source, 0).expect_err("missing pages should reject");

    assert_eq!(error.catalog_header_byte_offset, Some(0));
    assert_eq!(error.reason, CatalogPagesInspectionRejection::MissingPages);
}

#[test]
fn catalog_pages_rejects_duplicate_pages() {
    let source = b"1 0 obj\n<< /Pages 2 0 R /Type /Catalog /Pages 3 0 R >>\nendobj\n";

    let error = inspect_catalog_pages(source, 0).expect_err("duplicate pages should reject");

    assert_eq!(
        error.reason,
        CatalogPagesInspectionRejection::DuplicatePages {
            first_key_range: crate::DictionaryEntryByteRange { start: 11, end: 17 },
            duplicate_key_range: crate::DictionaryEntryByteRange { start: 39, end: 45 },
        }
    );
    assert_eq!(error.error_byte_offset, Some(39));
}

#[test]
fn catalog_pages_rejects_direct_dictionary_name_and_number_values() {
    for (source, expected_kind) in [
        (
            b"1 0 obj\n<< /Pages << /Type /Pages >> >>\nendobj\n".as_slice(),
            DictionaryValueKind::Dictionary,
        ),
        (
            b"1 0 obj\n<< /Pages /Pages >>\nendobj\n".as_slice(),
            DictionaryValueKind::Name,
        ),
        (
            b"1 0 obj\n<< /Pages 2 >>\nendobj\n".as_slice(),
            DictionaryValueKind::NumberLike,
        ),
    ] {
        let error = inspect_catalog_pages(source, 0).expect_err("direct value should reject");

        assert_eq!(
            error.reason,
            CatalogPagesInspectionRejection::NonReferencePagesValue {
                value_kind: expected_kind,
            }
        );
    }
}

#[test]
fn catalog_pages_rejects_malformed_obj_keyword_reference() {
    let source = b"1 0 obj\n<< /Pages 2 0 obj >>\nendobj\n";

    let error = inspect_catalog_pages(source, 0).expect_err("obj value should reject");

    assert_eq!(
        error.reason,
        CatalogPagesInspectionRejection::MalformedPagesReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
    assert_eq!(error.error_byte_offset, Some(22));
}

#[test]
fn catalog_pages_rejects_reference_with_extra_scalar_token() {
    let source = b"1 0 obj\n<< /Pages 2 0 R extra >>\nendobj\n";

    let error = inspect_catalog_pages(source, 0).expect_err("extra token should reject");

    assert_eq!(
        error.reason,
        CatalogPagesInspectionRejection::MalformedPagesReference {
            reference_reason: IndirectReferenceInspectionRejection::MalformedReference,
        }
    );
    assert_eq!(error.error_byte_offset, Some(23));
}

#[test]
fn catalog_pages_report_does_not_retain_source_bytes() {
    let source = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R /Secret (not-copied) >>\nendobj\n";

    let report = inspect_catalog_pages(source, 0).expect("pages should inspect");

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("Secret"));
    assert!(!debug_report.contains("not-copied"));
    assert!(!debug_report.contains("/Catalog"));
    assert!(!debug_report.contains("/Pages"));
}

#[test]
fn catalog_pages_composes_from_xref_trailer_root_to_page_tree_reference() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n";
    let catalog_offset = prefix.len();
    let pages_offset = prefix.len() + catalog.len();
    let page_offset = prefix.len() + catalog.len() + pages.len();
    let xref_offset = prefix.len() + catalog.len() + pages.len() + page.len();
    let source = format!(
        "{}{}{}{}xref\n0 4\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{pages_offset:010} 00000 n \n{page_offset:010} 00000 n \ntrailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(catalog),
        String::from_utf8_lossy(pages),
        String::from_utf8_lossy(page),
    )
    .into_bytes();

    let xref_report =
        inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    let root_report = inspect_classic_xref_trailer_root(&source, xref_report.trailer_byte_offset)
        .expect("root should inspect");
    assert_eq!(
        root_report.root_reference,
        IndirectRef {
            object_number: 1,
            generation: 0,
        }
    );

    let catalog_location =
        resolve_classic_xref_object(&xref_report, root_report.root_reference.object_number);
    assert_eq!(
        catalog_location,
        ClassicXrefObjectLocation::InUse {
            object_number: 1,
            generation: 0,
            byte_offset: catalog_offset,
        }
    );

    let catalog_report =
        inspect_catalog_pages(&source, catalog_offset).expect("catalog pages should inspect");
    assert_eq!(
        catalog_report.pages_reference,
        IndirectRef {
            object_number: 2,
            generation: 0,
        }
    );
    assert_eq!(catalog_report.pages_value_range.start, catalog_offset + 33);
    assert_eq!(catalog_report.pages_value_range.end, catalog_offset + 38);

    let page_tree_location =
        resolve_classic_xref_object(&xref_report, catalog_report.pages_reference.object_number);
    assert_eq!(
        page_tree_location,
        ClassicXrefObjectLocation::InUse {
            object_number: 2,
            generation: 0,
            byte_offset: pages_offset,
        }
    );
}
