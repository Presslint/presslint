//! Defining-node provenance, direct/indirect binding-path locality,
//! Table 30 inheritance, and the locked serde shape.

#[path = "../content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{TestSerdeValue, from_serde_value, serde_value};

use super::{CATALOG, Fixture, fixture_owned, form_object, indirect_ref};
use crate::{
    BindingContainerLocality, BindingResourcesSource, DocumentPageXObjectBindingsInspection,
    IndirectObjectOwnership, PageXObjectBindingRefusal, PageXObjectBindingUnprovenReason,
    PageXObjectBindingVerdict, PdfName, XObjectBindingSubtype,
};

fn single_page_fixture(page_body: &str) -> Fixture {
    fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        format!("3 0 obj\n{page_body}\nendobj\n").into_bytes(),
        form_object(4, ""),
    ])
}

#[test]
fn leaf_direct_binding_with_exclusive_target_is_proven_page_local() {
    let pdf = single_page_fixture(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>",
    );

    let report = pdf.inspect();

    assert_eq!(report.page_count(), 1);
    let page = &report.pages[0];
    assert_eq!(page.ordinal, 0);
    assert_eq!(page.page_reference, indirect_ref(3, 0));
    assert!(page.refused.is_empty());
    assert_eq!(page.witnesses.len(), 1);

    let witness = &page.witnesses[0];
    assert_eq!(witness.name, PdfName(b"Fm0".to_vec()));
    assert_eq!(witness.target, indirect_ref(4, 0));
    assert_eq!(witness.target_object_byte_offset, pdf.object_offset(4));
    assert_eq!(witness.subtype, XObjectBindingSubtype::Form);
    assert!(matches!(
        witness.path.resources_source,
        BindingResourcesSource::Direct { target, .. } if target == indirect_ref(3, 0)
    ));
    assert_eq!(
        witness.path.resources_locality,
        BindingContainerLocality::DirectDictionary
    );
    assert_eq!(
        witness.path.xobject_locality,
        BindingContainerLocality::DirectDictionary
    );
    assert_eq!(
        &pdf.source[witness.key_range.start..witness.key_range.end],
        b"/Fm0"
    );
    assert_eq!(
        &pdf.source[witness.value_range.start..witness.value_range.end],
        b"4 0 R"
    );
    assert_eq!(
        &pdf.source[witness.path.xobject_key_range.start..witness.path.xobject_key_range.end],
        b"/XObject"
    );
    assert_eq!(
        witness.target_ownership,
        IndirectObjectOwnership::ProvenSingleUse {
            owner: indirect_ref(3, 0),
        }
    );
    assert_eq!(witness.verdict, PageXObjectBindingVerdict::ProvenPageLocal);
}

#[test]
fn ancestor_inherited_resources_record_the_defining_node_and_stay_unproven() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>\nendobj\n".to_vec(),
        form_object(4, ""),
    ]);

    let report = pdf.inspect();

    let witness = &report.pages[0].witnesses[0];
    assert!(matches!(
        witness.path.resources_source,
        BindingResourcesSource::Inherited { ancestor, .. } if ancestor == indirect_ref(2, 0)
    ));
    assert_eq!(
        witness.verdict,
        PageXObjectBindingVerdict::Unproven {
            reason: PageXObjectBindingUnprovenReason::ResourcesInherited {
                ancestor: indirect_ref(2, 0),
            },
        }
    );
    // Inherited resources are reached by the RootKey(/Pages) user, never by
    // the page user (the leaf dictionary holds no edge to them), so the
    // target is not exclusively consumed by the page.
    assert_eq!(witness.target_ownership, IndirectObjectOwnership::Unproven);
}

#[test]
fn nearest_ancestor_wins_whole_value_replacement() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [5 0 R] /Count 1 /Resources << /XObject << /Root 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 5 0 R /MediaBox [0 0 10 10] >>\nendobj\n".to_vec(),
        form_object(4, ""),
        b"5 0 obj\n<< /Type /Pages /Parent 2 0 R /Kids [3 0 R] /Count 1 /Resources << /XObject << /Mid 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
    ]);

    let report = pdf.inspect();

    let witness = &report.pages[0].witnesses[0];
    assert_eq!(witness.name, PdfName(b"Mid".to_vec()));
    assert!(matches!(
        witness.path.resources_source,
        BindingResourcesSource::Inherited { ancestor, .. } if ancestor == indirect_ref(5, 0)
    ));
}

#[test]
fn null_leaf_resources_keep_ancestor_resources_effective() {
    // ISO 32000-1 §7.3.9: a dictionary entry whose value is `null` is
    // equivalent to an absent entry, so the leaf's `/Resources null` never
    // replaces the inherited ancestor value.
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources null >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
    ]);

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.refused.is_empty());
    assert_eq!(page.witnesses.len(), 1);
    let witness = &page.witnesses[0];
    assert_eq!(witness.name, PdfName(b"Fm0".to_vec()));
    assert!(matches!(
        witness.path.resources_source,
        BindingResourcesSource::Inherited { ancestor, .. } if ancestor == indirect_ref(2, 0)
    ));
}

#[test]
fn table30_replacement_never_merges_across_levels() {
    // The leaf's own `/Resources` lacks `/XObject`; the ancestor's `/XObject`
    // must NOT leak through (whole-value replacement, never per-key merge).
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 /Resources << /XObject << /Fm0 4 0 R >> >> >>\nendobj\n"
            .to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /Font << >> >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
    ]);

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(page.refused.len(), 1);
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::MissingXObject {
            defining_node: indirect_ref(3, 0),
        }
    );
}

#[test]
fn indirect_resources_resolve_with_identity_and_stay_unproven() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 0 R >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
        b"5 0 obj\n<< /XObject << /Fm0 4 0 R >> >>\nendobj\n".to_vec(),
    ]);

    let report = pdf.inspect();

    let witness = &report.pages[0].witnesses[0];
    assert!(matches!(
        witness.path.resources_source,
        BindingResourcesSource::Direct { target, .. } if target == indirect_ref(3, 0)
    ));
    assert_eq!(
        witness.path.resources_locality,
        BindingContainerLocality::IndirectResolved {
            reference: indirect_ref(5, 0),
            object_byte_offset: pdf.object_offset(5),
        }
    );
    assert_eq!(
        witness.verdict,
        PageXObjectBindingVerdict::Unproven {
            reason: PageXObjectBindingUnprovenReason::ResourcesIndirect {
                reference: indirect_ref(5, 0),
            },
        }
    );
    // The target itself is still exclusively consumed by this page: path
    // locality and target ownership are independent facts.
    assert_eq!(
        witness.target_ownership,
        IndirectObjectOwnership::ProvenSingleUse {
            owner: indirect_ref(3, 0),
        }
    );
}

#[test]
fn indirect_xobject_subdictionary_resolves_instead_of_skipping() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject 5 0 R >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
        b"5 0 obj\n<< /Fm0 4 0 R >>\nendobj\n".to_vec(),
    ]);

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.refused.is_empty());
    let witness = &page.witnesses[0];
    assert_eq!(
        witness.path.xobject_locality,
        BindingContainerLocality::IndirectResolved {
            reference: indirect_ref(5, 0),
            object_byte_offset: pdf.object_offset(5),
        }
    );
    assert_eq!(
        witness.verdict,
        PageXObjectBindingVerdict::Unproven {
            reason: PageXObjectBindingUnprovenReason::XObjectDictionaryIndirect {
                reference: indirect_ref(5, 0),
            },
        }
    );
}

#[test]
fn escaped_names_bind_semantically_with_raw_spelling_retained() {
    // `/Fm#30` decodes to `Fm0`: an escaped spelling binds as one semantic
    // entry, and the witness retains the raw spelling for reporting.
    let pdf = single_page_fixture(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm#30 4 0 R >> >> >>",
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.refused.is_empty());
    assert_eq!(page.witnesses.len(), 1);
    assert_eq!(page.witnesses[0].name, PdfName(b"Fm#30".to_vec()));
}

#[test]
fn witnesses_are_sorted_by_raw_name() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Z9 4 0 R /A1 5 0 R >> >> >>\nendobj\n"
            .to_vec(),
        form_object(4, ""),
        form_object(5, ""),
    ]);

    let report = pdf.inspect();

    let names = report.pages[0]
        .witnesses
        .iter()
        .map(|witness| witness.name.clone())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![PdfName(b"A1".to_vec()), PdfName(b"Z9".to_vec())]
    );
}

fn map(entries: Vec<(&str, TestSerdeValue)>) -> TestSerdeValue {
    TestSerdeValue::Map(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_string(), value))
            .collect(),
    )
}

fn range(start: usize, end: usize) -> TestSerdeValue {
    map(vec![("start", u64_value(start)), ("end", u64_value(end))])
}

fn u64_value(value: usize) -> TestSerdeValue {
    TestSerdeValue::U64(u64::try_from(value).expect("value fits u64"))
}

fn reference_value(object_number: u64, generation: u64) -> TestSerdeValue {
    map(vec![
        ("object_number", TestSerdeValue::U64(object_number)),
        ("generation", TestSerdeValue::U64(generation)),
    ])
}

fn locality_value(kind: &str) -> TestSerdeValue {
    map(vec![("locality", TestSerdeValue::String(kind.to_string()))])
}

fn range_value(range_ref: crate::DictionaryEntryByteRange) -> TestSerdeValue {
    range(range_ref.start, range_ref.end)
}

fn expected_path_value(witness: &crate::PageXObjectBindingWitness) -> TestSerdeValue {
    let (resources_key, resources_value) = match &witness.path.resources_source {
        BindingResourcesSource::Direct {
            key_range,
            value_range,
            ..
        } => (*key_range, *value_range),
        BindingResourcesSource::Inherited { .. } => {
            unreachable!("fixture resources are leaf-direct")
        }
    };
    map(vec![
        (
            "resources_source",
            map(vec![
                ("source", TestSerdeValue::String("direct".to_string())),
                ("target", reference_value(3, 0)),
                ("key_range", range_value(resources_key)),
                ("value_range", range_value(resources_value)),
            ]),
        ),
        ("resources_locality", locality_value("direct_dictionary")),
        (
            "xobject_key_range",
            range_value(witness.path.xobject_key_range),
        ),
        (
            "xobject_value_range",
            range_value(witness.path.xobject_value_range),
        ),
        ("xobject_locality", locality_value("direct_dictionary")),
    ])
}

fn expected_witness_value(
    pdf: &Fixture,
    witness: &crate::PageXObjectBindingWitness,
) -> TestSerdeValue {
    map(vec![
        (
            "name",
            TestSerdeValue::Seq(vec![
                TestSerdeValue::U64(u64::from(b'F')),
                TestSerdeValue::U64(u64::from(b'm')),
                TestSerdeValue::U64(u64::from(b'0')),
            ]),
        ),
        ("key_range", range_value(witness.key_range)),
        ("value_range", range_value(witness.value_range)),
        ("path", expected_path_value(witness)),
        ("target", reference_value(4, 0)),
        ("target_object_byte_offset", u64_value(pdf.object_offset(4))),
        (
            "subtype",
            map(vec![("kind", TestSerdeValue::String("form".to_string()))]),
        ),
        (
            "target_ownership",
            map(vec![
                (
                    "status",
                    TestSerdeValue::String("proven_single_use".to_string()),
                ),
                ("owner", reference_value(3, 0)),
            ]),
        ),
        (
            "verdict",
            map(vec![(
                "verdict",
                TestSerdeValue::String("proven_page_local".to_string()),
            )]),
        ),
    ])
}

#[test]
fn document_report_serde_shape_is_locked() {
    let pdf = single_page_fixture(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>",
    );

    let report = pdf.inspect();
    let value = serde_value(&report).expect("document report should serialize");

    let witness = &report.pages[0].witnesses[0];
    assert_eq!(
        value,
        map(vec![
            ("byte_len", u64_value(pdf.source.len())),
            (
                "pages",
                TestSerdeValue::Seq(vec![map(vec![
                    ("ordinal", TestSerdeValue::U64(0)),
                    ("page_reference", reference_value(3, 0)),
                    ("page_object_byte_offset", u64_value(pdf.object_offset(3))),
                    (
                        "witnesses",
                        TestSerdeValue::Seq(vec![expected_witness_value(&pdf, witness)]),
                    ),
                    ("refused", TestSerdeValue::Seq(Vec::new())),
                ])]),
            ),
            ("page_tree_skipped", TestSerdeValue::Seq(Vec::new())),
            ("visited_node_count", TestSerdeValue::U64(1)),
            ("truncated", TestSerdeValue::None),
        ])
    );

    let decoded: DocumentPageXObjectBindingsInspection =
        from_serde_value(value).expect("document report should deserialize");
    assert_eq!(decoded, report);
}

#[test]
fn report_does_not_retain_source_bytes() {
    let pdf = single_page_fixture(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Secret (do-not-copy) /Resources << /XObject << /Fm0 4 0 R >> >> >>",
    );

    let report = pdf.inspect();

    let debug_report = format!("{report:?}");
    assert!(!debug_report.contains("do-not-copy"));
    assert!(!debug_report.contains("Secret"));
}
