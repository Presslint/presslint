use crate::{
    PageBoxKind, PageBoxSource, PageRectangle, ResolvedObjectPosition, SkippedPageBoxReason,
    inspect_document_page_boxes,
};

fn classic_pdf(objects: &[&[u8]]) -> Vec<u8> {
    let mut buf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(buf.len());
        let number = index + 1;
        buf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = buf.len();
    buf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    buf
}

fn page_tree_pdf(root: &[u8], pages: &[&[u8]]) -> Vec<u8> {
    let mut objects: Vec<&[u8]> = vec![b"<< /Type /Catalog /Pages 2 0 R >>", root];
    objects.extend_from_slice(pages);
    classic_pdf(&objects)
}

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

fn compressed_leaf_pdf() -> Vec<u8> {
    let prefix = b"%PDF-1.5\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let root =
        b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /MediaBox [0 0 612 792] >>\nendobj\n";
    let leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R /Secret (do-not-copy) >>";
    let objstm = object_stream(&[(3, leaf)]);

    let catalog_offset = prefix.len();
    let root_offset = catalog_offset + catalog.len();
    let objstm_offset = root_offset + root.len();
    let xref_offset = objstm_offset + objstm.len();

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(1, catalog_offset, 0));
    records.extend_from_slice(&xref_record(1, root_offset, 0));
    records.extend_from_slice(&xref_record(2, 5, 0));
    records.extend_from_slice(&xref_record(0, 0, 0));
    records.extend_from_slice(&xref_record(1, objstm_offset, 0));
    records.extend_from_slice(&xref_record(1, xref_offset, 0));

    let mut source = prefix.to_vec();
    source.extend_from_slice(catalog);
    source.extend_from_slice(root);
    source.extend_from_slice(&objstm);
    source.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /W [ 1 2 1 ] /Index [ 0 7 ] /Root 1 0 R /Length {} >>\nstream\n",
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
fn direct_leaf_media_and_crop_boxes_report_direct_sources() {
    let source = page_tree_pdf(
        b"<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>",
        &[b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /CropBox [10 20 300 400] >>"],
    );

    let report = inspect_document_page_boxes(&source).expect("page boxes inspect");

    assert!(report.skipped.is_empty());
    assert_eq!(report.pages.len(), 1);
    let page = &report.pages[0];
    assert_eq!(page.page_index, 0);
    assert_eq!(
        page.media_box.effective,
        PageRectangle {
            llx: 0.0,
            lly: 0.0,
            urx: 612.0,
            ury: 792.0,
        }
    );
    assert_eq!(
        page.crop_box.effective,
        PageRectangle {
            llx: 10.0,
            lly: 20.0,
            urx: 300.0,
            ury: 400.0,
        }
    );
    assert!(matches!(
        page.media_box.source,
        PageBoxSource::Direct { .. }
    ));
    assert!(matches!(page.crop_box.source, PageBoxSource::Direct { .. }));
}

#[test]
fn inherited_media_box_and_default_crop_box_are_reported() {
    let source = page_tree_pdf(
        b"<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /MediaBox [0 0 600 800] >>",
        &[b"<< /Type /Page /Parent 2 0 R >>"],
    );

    let report = inspect_document_page_boxes(&source).expect("page boxes inspect");
    let page = &report.pages[0];

    assert!(report.skipped.is_empty());
    assert_eq!(
        page.media_box.effective,
        PageRectangle {
            llx: 0.0,
            lly: 0.0,
            urx: 600.0,
            ury: 800.0,
        }
    );
    assert!(matches!(
        page.media_box.source,
        PageBoxSource::Inherited { .. }
    ));
    assert_eq!(page.crop_box.effective, page.media_box.effective);
    assert_eq!(page.crop_box.source, PageBoxSource::DefaultedToMediaBox);
}

#[test]
fn leaf_direct_media_box_overrides_inherited_ancestor() {
    let source = page_tree_pdf(
        b"<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /MediaBox [0 0 600 800] >>",
        &[b"<< /Type /Page /Parent 2 0 R /MediaBox [1 2 3 4] >>"],
    );

    let report = inspect_document_page_boxes(&source).expect("page boxes inspect");
    let page = &report.pages[0];

    assert_eq!(
        page.media_box.effective,
        PageRectangle {
            llx: 1.0,
            lly: 2.0,
            urx: 3.0,
            ury: 4.0,
        }
    );
    assert!(matches!(
        page.media_box.source,
        PageBoxSource::Direct { .. }
    ));
}

#[test]
fn duplicate_malformed_non_array_indirect_and_missing_media_boxes_skip_pages() {
    let source = page_tree_pdf(
        b"<< /Type /Pages /Kids [ 3 0 R 4 0 R 5 0 R 6 0 R 7 0 R ] /Count 5 >>",
        &[
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /MediaBox [0 0 2 2] >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1] >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox /AName >>",
            b"<< /Type /Page /Parent 2 0 R /MediaBox 8 0 R >>",
            b"<< /Type /Page /Parent 2 0 R >>",
        ],
    );

    let report = inspect_document_page_boxes(&source).expect("page boxes inspect");

    assert!(report.pages.is_empty());
    assert_eq!(report.skipped.len(), 5);
    assert!(matches!(
        report.skipped[0].reason,
        SkippedPageBoxReason::DuplicateKey { .. }
    ));
    assert_eq!(
        report.skipped[1].reason,
        SkippedPageBoxReason::MalformedRectangle
    );
    assert!(matches!(
        report.skipped[2].reason,
        SkippedPageBoxReason::UnsupportedValueKind { .. }
    ));
    assert!(matches!(
        report.skipped[3].reason,
        SkippedPageBoxReason::UnsupportedValueKind { .. }
    ));
    assert_eq!(
        report.skipped[4].reason,
        SkippedPageBoxReason::MissingEffectiveMediaBox
    );
}

#[test]
fn malformed_crop_box_skips_that_page() {
    let source = page_tree_pdf(
        b"<< /Type /Pages /Kids [ 3 0 R ] /Count 1 /MediaBox [0 0 600 800] >>",
        &[b"<< /Type /Page /Parent 2 0 R /CropBox [0 0 nope 1] >>"],
    );

    let report = inspect_document_page_boxes(&source).expect("page boxes inspect");

    assert!(report.pages.is_empty());
    assert_eq!(report.skipped[0].kind, Some(PageBoxKind::CropBox));
    assert_eq!(
        report.skipped[0].reason,
        SkippedPageBoxReason::MalformedRectangle
    );
}

#[test]
fn compressed_leaf_page_dictionary_is_a_structured_skip() {
    let source = compressed_leaf_pdf();

    let report = inspect_document_page_boxes(&source).expect("page boxes inspect");

    assert!(report.pages.is_empty());
    assert_eq!(report.skipped.len(), 1);
    assert!(matches!(
        report.skipped[0].leaf_position,
        Some(ResolvedObjectPosition::Compressed { .. })
    ));
    assert!(matches!(
        report.skipped[0].reason,
        SkippedPageBoxReason::CompressedLeafDictionary { .. }
    ));
    assert!(!format!("{report:?}").contains("do-not-copy"));
}
