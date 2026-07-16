//! Collision, duplicate, malformed, compressed, missing-container, and
//! wrong-subtype matrix. Everything here fails closed with a distinct
//! classification — never first/last-wins recovery, never fabricated offsets,
//! never the obsolete content-stream resource fallback.

use super::{CATALOG, Fixture, empty_consumers, fixture_owned, form_object, indirect_ref};
use crate::{
    BindingContainer, DictionaryValueKind, ObjectLookup, ObjectLookupLocation,
    PageXObjectBindingRefusal, PdfName, XObjectBindingSubtype, XrefStreamEntryRecord,
    inspect_document_page_xobject_bindings_with_lookup,
};

fn single_page(page_body: &str, extra_objects: &[Vec<u8>]) -> Fixture {
    let mut objects = vec![
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        format!("3 0 obj\n{page_body}\nendobj\n").into_bytes(),
        form_object(4, ""),
    ];
    objects.extend_from_slice(extra_objects);
    fixture_owned(&objects)
}

#[test]
fn decoded_name_collision_poisons_every_colliding_entry() {
    // `/Fm0` and `/Fm#30` decode to the same name; BOTH entries are refused.
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R /Fm#30 5 0 R >> >> >>",
        &[form_object(5, "")],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(page.refused.len(), 2);
    assert_eq!(
        page.refused[0].resource_name,
        Some(PdfName(b"Fm0".to_vec()))
    );
    assert_eq!(
        page.refused[1].resource_name,
        Some(PdfName(b"Fm#30".to_vec()))
    );
    for refused in &page.refused {
        assert!(matches!(
            &refused.reason,
            PageXObjectBindingRefusal::EntryNameCollision {
                colliding_key_ranges,
            } if colliding_key_ranges.len() == 2
        ));
    }
}

#[test]
fn raw_duplicate_entry_keys_poison_every_occurrence() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R /Fm0 5 0 R >> >> >>",
        &[form_object(5, "")],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(page.refused.len(), 2);
    assert!(page.refused.iter().all(|refused| matches!(
        refused.reason,
        PageXObjectBindingRefusal::EntryNameCollision { .. }
    )));
}

#[test]
fn malformed_entry_name_refuses_fail_closed_without_poisoning_others() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /F#zz 4 0 R /Ok 5 0 R >> >> >>",
        &[form_object(5, "")],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert_eq!(page.witnesses.len(), 1);
    assert_eq!(page.witnesses[0].name, PdfName(b"Ok".to_vec()));
    assert_eq!(page.refused.len(), 1);
    assert_eq!(
        page.refused[0].resource_name,
        Some(PdfName(b"F#zz".to_vec()))
    );
    assert!(matches!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::MalformedEntryName { .. }
    ));
}

#[test]
fn absent_resources_everywhere_is_a_classified_refusal() {
    let pdf = single_page("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>", &[]);

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::MissingResources
    );
}

#[test]
fn null_resources_with_no_ancestor_value_refuses_as_missing_resources() {
    // `null` is equivalent to an absent entry (ISO 32000-1 §7.3.9): with no
    // ancestor value, Table 30 inheritance is exhausted.
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources null >>",
        &[],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::MissingResources
    );
}

#[test]
fn null_xobject_value_classifies_as_missing_not_unsupported() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject null >> >>",
        &[],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::MissingXObject {
            defining_node: indirect_ref(3, 0),
        }
    );
}

#[test]
fn no_obsolete_fallback_into_form_local_resources() {
    // The form's OWN `/Resources` carries `/Inner`; the page namespace is
    // empty. Nothing may resolve `/Inner` through the obsolete fallback: the
    // page enumerates zero witnesses and zero refusals for it.
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << >> >> >>",
        &[form_object(
            5,
            " /Resources << /XObject << /Inner 4 0 R >> >>",
        )],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert!(page.refused.is_empty());
}

#[test]
fn duplicate_resources_keys_refuse_by_decoded_comparison() {
    // `/Resources` and `/R#65sources` decode to the same key.
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << >> /R#65sources << >> >>",
        &[],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert!(matches!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::DuplicateContainerKey {
            container: BindingContainer::Resources,
            defining_node,
            ..
        } if defining_node == indirect_ref(3, 0)
    ));
}

#[test]
fn duplicate_xobject_keys_refuse_by_decoded_comparison() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << >> /X#4fbject << >> >> >>",
        &[],
    );

    let report = pdf.inspect();

    assert!(matches!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::DuplicateContainerKey {
            container: BindingContainer::XObjectDictionary,
            ..
        }
    ));
}

#[test]
fn unsupported_container_values_refuse_with_the_shallow_kind() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources [ 4 0 R ] >>",
        &[],
    );

    let report = pdf.inspect();

    assert!(matches!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::UnsupportedContainerValue {
            container: BindingContainer::Resources,
            ..
        }
    ));
}

#[test]
fn non_reference_entry_value_refuses() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 << >> >> >> >>",
        &[],
    );

    let report = pdf.inspect();

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::NonReferenceEntry {
            value_kind: DictionaryValueKind::Dictionary,
        }
    );
}

#[test]
fn dangling_entry_target_refuses_as_unresolved() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 9 0 R >> >> >>",
        &[],
    );

    let report = pdf.inspect();

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::UnresolvedEntryTarget {
            reference: indirect_ref(9, 0),
            location: ObjectLookupLocation::ClassicNotFound { object_number: 9 },
        }
    );
}

#[test]
fn generation_mismatched_entry_target_refuses_distinctly() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 1 R >> >> >>",
        &[],
    );

    let report = pdf.inspect();

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::EntryTargetGenerationMismatch {
            reference: indirect_ref(4, 1),
            xref_generation: 0,
        }
    );
}

#[test]
fn compressed_xobject_container_refuses_without_fabricated_offsets() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject 5 0 R >> >>",
        &[b"5 0 obj\n<< /Fm0 4 0 R >>\nendobj\n".to_vec()],
    );
    let section = pdf.xref_stream_section(&[(
        5,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 7,
            index_within_object_stream: 1,
        },
    )]);
    let consumers = empty_consumers(pdf.source.len());

    let report = inspect_document_page_xobject_bindings_with_lookup(
        &pdf.source,
        ObjectLookup::XrefStreamSection(&section),
        pdf.object_offset(2),
        &consumers,
    )
    .expect("page xobject bindings should inspect");

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::CompressedContainer {
            container: BindingContainer::XObjectDictionary,
            defining_node: indirect_ref(3, 0),
            reference: indirect_ref(5, 0),
            object_stream_number: 7,
            index_within_object_stream: 1,
        }
    );
}

#[test]
fn nonzero_generation_compressed_xobject_container_refuses_as_generation_mismatch() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject 5 1 R >> >>",
        &[b"5 0 obj\n<< /Fm0 4 0 R >>\nendobj\n".to_vec()],
    );
    let section = pdf.xref_stream_section(&[(
        5,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 7,
            index_within_object_stream: 1,
        },
    )]);
    let consumers = empty_consumers(pdf.source.len());

    let report = inspect_document_page_xobject_bindings_with_lookup(
        &pdf.source,
        ObjectLookup::XrefStreamSection(&section),
        pdf.object_offset(2),
        &consumers,
    )
    .expect("page xobject bindings should inspect");

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::ContainerGenerationMismatch {
            container: BindingContainer::XObjectDictionary,
            defining_node: indirect_ref(3, 0),
            reference: indirect_ref(5, 1),
            xref_generation: 0,
        }
    );
}

#[test]
fn compressed_resources_container_refuses_distinctly() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 0 R >>",
        &[b"5 0 obj\n<< /XObject << /Fm0 4 0 R >> >>\nendobj\n".to_vec()],
    );
    let section = pdf.xref_stream_section(&[(
        5,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 7,
            index_within_object_stream: 0,
        },
    )]);
    let consumers = empty_consumers(pdf.source.len());

    let report = inspect_document_page_xobject_bindings_with_lookup(
        &pdf.source,
        ObjectLookup::XrefStreamSection(&section),
        pdf.object_offset(2),
        &consumers,
    )
    .expect("page xobject bindings should inspect");

    assert!(matches!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::CompressedContainer {
            container: BindingContainer::Resources,
            ..
        }
    ));
}

#[test]
fn nonzero_generation_compressed_resources_container_refuses_as_generation_mismatch() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 1 R >>",
        &[b"5 0 obj\n<< /XObject << /Fm0 4 0 R >> >>\nendobj\n".to_vec()],
    );
    let section = pdf.xref_stream_section(&[(
        5,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 7,
            index_within_object_stream: 0,
        },
    )]);
    let consumers = empty_consumers(pdf.source.len());

    let report = inspect_document_page_xobject_bindings_with_lookup(
        &pdf.source,
        ObjectLookup::XrefStreamSection(&section),
        pdf.object_offset(2),
        &consumers,
    )
    .expect("page xobject bindings should inspect");

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::ContainerGenerationMismatch {
            container: BindingContainer::Resources,
            defining_node: indirect_ref(3, 0),
            reference: indirect_ref(5, 1),
            xref_generation: 0,
        }
    );
}

#[test]
fn compressed_entry_target_refuses_distinctly() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 0 R >> >> >>",
        &[],
    );
    let section = pdf.xref_stream_section(&[(
        4,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 7,
            index_within_object_stream: 2,
        },
    )]);
    let consumers = empty_consumers(pdf.source.len());

    let report = inspect_document_page_xobject_bindings_with_lookup(
        &pdf.source,
        ObjectLookup::XrefStreamSection(&section),
        pdf.object_offset(2),
        &consumers,
    )
    .expect("page xobject bindings should inspect");

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::CompressedEntryTarget {
            reference: indirect_ref(4, 0),
            object_stream_number: 7,
            index_within_object_stream: 2,
        }
    );
}

#[test]
fn nonzero_generation_compressed_entry_target_refuses_as_generation_mismatch() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Fm0 4 1 R >> >> >>",
        &[],
    );
    let section = pdf.xref_stream_section(&[(
        4,
        XrefStreamEntryRecord::Compressed {
            object_stream_number: 7,
            index_within_object_stream: 2,
        },
    )]);
    let consumers = empty_consumers(pdf.source.len());

    let report = inspect_document_page_xobject_bindings_with_lookup(
        &pdf.source,
        ObjectLookup::XrefStreamSection(&section),
        pdf.object_offset(2),
        &consumers,
    )
    .expect("page xobject bindings should inspect");

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::EntryTargetGenerationMismatch {
            reference: indirect_ref(4, 1),
            xref_generation: 0,
        }
    );
}

#[test]
fn stream_resources_container_refuses() {
    // Object 5's dictionary portion would scan, but `/Resources` requires a
    // dictionary OBJECT: the stream refuses with its own classification.
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 0 R >>",
        &[
            b"5 0 obj\n<< /XObject << /Fm0 4 0 R >> /Length 0 >>\nstream\n\nendstream\nendobj\n"
                .to_vec(),
        ],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::StreamContainer {
            container: BindingContainer::Resources,
            defining_node: indirect_ref(3, 0),
            reference: indirect_ref(5, 0),
            object_byte_offset: pdf.object_offset(5),
        }
    );
}

#[test]
fn stream_xobject_container_refuses() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject 5 0 R >> >>",
        &[b"5 0 obj\n<< /Fm0 4 0 R /Length 0 >>\nstream\n\nendstream\nendobj\n".to_vec()],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert_eq!(
        page.refused[0].reason,
        PageXObjectBindingRefusal::StreamContainer {
            container: BindingContainer::XObjectDictionary,
            defining_node: indirect_ref(3, 0),
            reference: indirect_ref(5, 0),
            object_byte_offset: pdf.object_offset(5),
        }
    );
}

#[test]
fn missing_xobject_in_indirect_resources_names_the_resolved_container() {
    // The dictionary owning (or here, lacking) the `/XObject` entry is the
    // RESOLVED indirect resources object, not the page-tree node.
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 0 R >>",
        &[b"5 0 obj\n<< /Font << >> >>\nendobj\n".to_vec()],
    );

    let report = pdf.inspect();

    assert_eq!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::MissingXObject {
            defining_node: indirect_ref(5, 0),
        }
    );
}

#[test]
fn duplicate_xobject_keys_in_indirect_resources_name_the_resolved_container() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources 5 0 R >>",
        &[b"5 0 obj\n<< /XObject << >> /X#4fbject << >> >>\nendobj\n".to_vec()],
    );

    let report = pdf.inspect();

    assert!(matches!(
        report.pages[0].refused[0].reason,
        PageXObjectBindingRefusal::DuplicateContainerKey {
            container: BindingContainer::XObjectDictionary,
            defining_node,
            ..
        } if defining_node == indirect_ref(5, 0)
    ));
}

#[test]
fn wrong_subtype_is_a_witness_classification_not_a_refusal() {
    let pdf = fixture_owned(&[
        CATALOG.to_vec(),
        b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_vec(),
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << /Ps 4 0 R /Bare 5 0 R /Im 6 0 R >> >> >>\nendobj\n"
            .to_vec(),
        b"4 0 obj\n<< /Type /XObject /Subtype /PS /Length 0 >>\nstream\n\nendstream\nendobj\n"
            .to_vec(),
        b"5 0 obj\n<< /Type /XObject /Length 0 >>\nstream\n\nendstream\nendobj\n".to_vec(),
        b"6 0 obj\n<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /Length 0 >>\nstream\n\nendstream\nendobj\n"
            .to_vec(),
    ]);

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.refused.is_empty());
    assert_eq!(page.witnesses.len(), 3);
    assert_eq!(page.witnesses[0].subtype, XObjectBindingSubtype::Missing);
    assert_eq!(page.witnesses[1].subtype, XObjectBindingSubtype::Image);
    assert_eq!(
        page.witnesses[2].subtype,
        XObjectBindingSubtype::OtherName {
            name: PdfName(b"PS".to_vec()),
        }
    );
}

#[test]
fn empty_effective_xobject_dictionary_is_present_not_omitted() {
    let pdf = single_page(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Resources << /XObject << >> >> >>",
        &[],
    );

    let report = pdf.inspect();

    let page = &report.pages[0];
    assert!(page.witnesses.is_empty());
    assert!(page.refused.is_empty());
}
