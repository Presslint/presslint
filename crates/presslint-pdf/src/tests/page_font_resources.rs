#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use crate::{
    ClassicXrefTableInspection, DocumentPageFontResourcesInspection, FontSubtypeClass,
    MAX_PAGE_TREE_DEPTH, MAX_VISITED_PAGE_TREE_NODES, ObjectLookup, PageTreeLeavesTruncation,
    PdfName, SkippedFontResourceReason, SkippedPageTreeLeafReason, inspect_classic_xref_table,
    inspect_document_page_extgstate_resources_with_lookup,
    inspect_document_page_font_resources_with_lookup,
};

struct Fixture {
    source: Vec<u8>,
    xref: ClassicXrefTableInspection,
    offsets: Vec<usize>,
}

impl Fixture {
    fn object_offset(&self, object_number: usize) -> usize {
        self.offsets[object_number - 1]
    }

    fn lookup(&self) -> ObjectLookup<'_> {
        ObjectLookup::ClassicXref(&self.xref)
    }
}

fn fixture_owned(objects: &[Vec<u8>]) -> Fixture {
    let mut source = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for object in objects {
        offsets.push(source.len());
        source.extend_from_slice(object);
    }

    let xref_offset = source.len();
    let object_count = objects.len() + 1;
    source.extend_from_slice(format!("xref\n0 {object_count}\n").as_bytes());
    source.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        source.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    source.extend_from_slice(
        format!(
            "trailer\n<< /Size {object_count} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n"
        )
        .as_bytes(),
    );

    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    Fixture {
        source,
        xref,
        offsets,
    }
}

fn fixture(objects: &[&[u8]]) -> Fixture {
    let owned = objects
        .iter()
        .map(|object| object.to_vec())
        .collect::<Vec<_>>();
    fixture_owned(&owned)
}

fn inspect(pdf: &Fixture) -> DocumentPageFontResourcesInspection {
    inspect_document_page_font_resources_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page font resources should inspect")
}

#[test]
fn page_with_own_resources_lacking_font_is_missing_font_not_ancestor() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /ExtGState << >> >> >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    assert!(report.pages[0].fonts.is_empty());
    assert_eq!(report.pages[0].skipped.len(), 1);
    assert_eq!(
        report.pages[0].skipped[0].reason,
        SkippedFontResourceReason::MissingFont
    );
}

#[test]
fn page_without_resources_inherits_nearest_ancestor_fonts() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Parent 1 0 R /Kids [3 0 R] /Count 1 /Resources << /Font << /F2 << /Type /Font /Subtype /TrueType >> >> >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    assert!(report.pages[0].skipped.is_empty());
    assert_eq!(report.pages[0].fonts.len(), 1);
    assert_eq!(report.pages[0].fonts[0].name, PdfName(b"F2".to_vec()));
    assert_eq!(report.pages[0].fonts[0].subtype, FontSubtypeClass::TrueType);
}

#[test]
fn escaped_font_namespace_is_inherited_as_one_whole_semantic_value() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /F#6fnt << /F#31 << /Type /Font /Subtype /Type1 >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    assert!(report.pages[0].skipped.is_empty());
    assert_eq!(report.pages[0].fonts.len(), 1);
    assert_eq!(report.pages[0].fonts[0].name, PdfName(b"F#31".to_vec()));
}

#[test]
fn exact_and_escaped_font_namespace_keys_poison_inheritance() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 /Resources << /Font << >> /F#6fnt << >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    assert!(report.pages[0].fonts.is_empty());
    assert!(matches!(
        report.pages[0].skipped[0].reason,
        SkippedFontResourceReason::DuplicateFont { .. }
    ));
}

#[test]
fn empty_replacement_resources_and_empty_font_are_present_not_omitted() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R 3 0 R] /Count 2 /Resources << /Font << /F1 << /Type /Font /Subtype /Type1 >> >> >> >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << >> >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /Font << >> >> >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    // Empty replacement `/Resources << >>` suppresses the ancestor's fonts.
    assert!(report.pages[0].fonts.is_empty());
    assert_eq!(
        report.pages[0].skipped[0].reason,
        SkippedFontResourceReason::MissingFont
    );
    // Empty `/Font << >>` is present-not-omitted: no fonts and no skip.
    assert!(report.pages[1].fonts.is_empty());
    assert!(report.pages[1].skipped.is_empty());
}

#[test]
fn identity_triples_match_the_extgstate_inspector() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R 5 0 R] /Count 3 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Parent 1 0 R /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
        b"4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /Font << /F1 << /Type /Font /Subtype /Type3 >> >> >> >>\nendobj\n",
        b"5 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
    ]);

    let fonts = inspect(&pdf);
    let extgstates = inspect_document_page_extgstate_resources_with_lookup(
        &pdf.source,
        pdf.lookup(),
        pdf.object_offset(1),
    )
    .expect("page ExtGState resources should inspect");

    let font_triples = fonts
        .pages
        .iter()
        .map(|page| {
            (
                page.ordinal,
                page.page_reference,
                page.page_object_byte_offset,
            )
        })
        .collect::<Vec<_>>();
    let extgstate_triples = extgstates
        .pages
        .iter()
        .map(|page| {
            (
                page.ordinal,
                page.page_reference,
                page.page_object_byte_offset,
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(font_triples.len(), 3);
    assert_eq!(font_triples, extgstate_triples);
    assert_eq!(fonts.visited_node_count, extgstates.visited_node_count);
}

#[test]
fn duplicate_font_key_in_effective_resources_fails_closed() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /Font << /F1 << /Subtype /Type1 >> >> /Font << >> >> >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    assert!(report.pages[0].fonts.is_empty());
    assert_eq!(report.pages[0].skipped.len(), 1);
    assert!(matches!(
        report.pages[0].skipped[0].reason,
        SkippedFontResourceReason::DuplicateFont { .. }
    ));
}

#[test]
fn duplicate_font_name_inside_font_dictionary_fails_closed() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /Font << /F1 << /Subtype /Type1 >> /F1 << /Subtype /Type3 >> >> >> >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    assert_eq!(report.pages[0].fonts.len(), 1);
    assert_eq!(report.pages[0].fonts[0].subtype, FontSubtypeClass::Type1);
    assert_eq!(report.pages[0].skipped.len(), 1);
    assert!(matches!(
        report.pages[0].skipped[0].reason,
        SkippedFontResourceReason::DuplicateFontName { .. }
    ));
}

#[test]
fn fonts_are_sorted_deterministically_by_raw_name() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /Font << /Z9 << /Subtype /Type1 >> /A1 << /Subtype /Type0 >> >> >> >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    let names = report.pages[0]
        .fonts
        .iter()
        .map(|font| font.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![PdfName(b"A1".to_vec()), PdfName(b"Z9".to_vec())]
    );
}

#[test]
fn page_tree_cycle_sets_truncated_exactly_once() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Pages /Parent 1 0 R /Kids [1 0 R 3 0 R 1 0 R] /Count 1 >>\nendobj\n",
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>\nendobj\n",
    ]);

    let report = inspect(&pdf);

    assert_eq!(report.page_count(), 1);
    assert_eq!(
        report.truncated,
        Some(PageTreeLeavesTruncation::Cycle { object_number: 1 })
    );
    let cycle_skips = report
        .page_tree_skipped
        .iter()
        .filter(|entry| matches!(entry.reason, SkippedPageTreeLeafReason::Cycle { .. }))
        .count();
    assert_eq!(cycle_skips, 2);
    assert_eq!(report.visited_node_count, 2);
}

#[test]
fn walk_honors_the_max_depth_bound() {
    let node_count = MAX_PAGE_TREE_DEPTH + 2;
    let mut objects = Vec::with_capacity(node_count + 1);
    for object_number in 1..=node_count {
        objects.push(
            format!(
                "{object_number} 0 obj\n<< /Type /Pages /Kids [{} 0 R] /Count 1 >>\nendobj\n",
                object_number + 1
            )
            .into_bytes(),
        );
    }
    objects.push(
        format!(
            "{} 0 obj\n<< /Type /Page /MediaBox [0 0 10 10] >>\nendobj\n",
            node_count + 1
        )
        .into_bytes(),
    );
    let pdf = fixture_owned(&objects);

    let report = inspect(&pdf);

    assert!(report.pages.is_empty());
    assert_eq!(
        report.truncated,
        Some(PageTreeLeavesTruncation::MaxDepth {
            max_depth: MAX_PAGE_TREE_DEPTH,
        })
    );
    assert!(report.page_tree_skipped.iter().any(|entry| matches!(
        entry.reason,
        SkippedPageTreeLeafReason::MaxDepthExceeded { .. }
    )));
}

#[test]
fn walk_honors_the_max_visited_node_bound() {
    // Root plus MAX kids: the root occupies one visited slot, so the last two
    // `/Pages` kids find the visited-node budget already exhausted.
    let kid_count = MAX_VISITED_PAGE_TREE_NODES + 1;
    let kid_refs = (2..2 + kid_count)
        .map(|object_number| format!("{object_number} 0 R"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects = Vec::with_capacity(kid_count + 1);
    objects.push(
        format!("1 0 obj\n<< /Type /Pages /Kids [{kid_refs}] /Count 0 >>\nendobj\n").into_bytes(),
    );
    for object_number in 2..2 + kid_count {
        objects.push(
            format!(
                "{object_number} 0 obj\n<< /Type /Pages /Parent 1 0 R /Kids [] /Count 0 >>\nendobj\n"
            )
            .into_bytes(),
        );
    }
    let pdf = fixture_owned(&objects);

    let report = inspect(&pdf);

    assert!(report.pages.is_empty());
    assert_eq!(report.visited_node_count, MAX_VISITED_PAGE_TREE_NODES);
    assert_eq!(
        report.truncated,
        Some(PageTreeLeavesTruncation::MaxVisitedNodes {
            max_visited_nodes: MAX_VISITED_PAGE_TREE_NODES,
        })
    );
    let refused = report
        .page_tree_skipped
        .iter()
        .filter(|entry| {
            matches!(
                entry.reason,
                SkippedPageTreeLeafReason::MaxVisitedNodesExceeded { .. }
            )
        })
        .count();
    assert_eq!(refused, 2);
}

fn kind_map(kind: &str) -> TestSerdeValue {
    TestSerdeValue::Map(vec![(
        "kind".to_string(),
        TestSerdeValue::String(kind.to_string()),
    )])
}

#[test]
fn document_report_serde_shape_is_locked() {
    let pdf = fixture(&[
        b"1 0 obj\n<< /Type /Pages /Kids [2 0 R] /Count 1 >>\nendobj\n",
        b"2 0 obj\n<< /Type /Page /Parent 1 0 R /MediaBox [0 0 10 10] /Resources << /Font << /F1 << /Type /Font /Subtype /Type0 >> >> >> >>\nendobj\n",
    ]);

    let report = inspect(&pdf);
    let value = serde_value(&report).expect("document report should serialize");

    let byte_len = u64::try_from(pdf.source.len()).expect("length fits u64");
    let page_offset = u64::try_from(pdf.object_offset(2)).expect("offset fits u64");
    assert_eq!(
        value,
        TestSerdeValue::Map(vec![
            ("byte_len".to_string(), TestSerdeValue::U64(byte_len)),
            (
                "pages".to_string(),
                TestSerdeValue::Seq(vec![TestSerdeValue::Map(vec![
                    ("ordinal".to_string(), TestSerdeValue::U64(0)),
                    (
                        "page_reference".to_string(),
                        TestSerdeValue::Map(vec![
                            ("object_number".to_string(), TestSerdeValue::U64(2)),
                            ("generation".to_string(), TestSerdeValue::U64(0)),
                        ]),
                    ),
                    (
                        "page_object_byte_offset".to_string(),
                        TestSerdeValue::U64(page_offset),
                    ),
                    (
                        "fonts".to_string(),
                        TestSerdeValue::Seq(vec![TestSerdeValue::Map(vec![
                            (
                                "name".to_string(),
                                TestSerdeValue::Seq(vec![
                                    TestSerdeValue::U64(u64::from(b'F')),
                                    TestSerdeValue::U64(u64::from(b'1')),
                                ]),
                            ),
                            ("dictionary_type".to_string(), kind_map("font")),
                            ("subtype".to_string(), kind_map("type0")),
                            ("reference".to_string(), TestSerdeValue::None),
                            ("object_byte_offset".to_string(), TestSerdeValue::None),
                        ])]),
                    ),
                    ("skipped".to_string(), TestSerdeValue::Seq(Vec::new())),
                ])]),
            ),
            (
                "page_tree_skipped".to_string(),
                TestSerdeValue::Seq(Vec::new()),
            ),
            ("visited_node_count".to_string(), TestSerdeValue::U64(1)),
            ("truncated".to_string(), TestSerdeValue::None),
        ])
    );

    let decoded: DocumentPageFontResourcesInspection =
        from_serde_value(value).expect("document report should deserialize");
    assert_eq!(decoded, report);
}
