#[path = "content_stream_extent/serde_harness.rs"]
#[allow(clippy::duplicate_mod)]
mod serde_harness;

use serde_harness::{from_serde_value, serde_value};

use super::indirect_ref;

use crate::{
    IndirectRef, MAX_OBJECT_CONSUMER_TRAVERSAL_DEPTH, MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    ObjectConsumerIndexInspection, ObjectConsumerIndexLimit, ObjectConsumerReferrer,
    ObjectConsumersEntry, ObjectResolutionRejection, ObjectStreamMemberExtractionRejection,
    PdfName, inspect_document_access, inspect_object_consumer_index,
};

/// One synthetic object for the classic fixture builder: a free xref entry
/// when `free`, otherwise an in-use entry pointing at `body`.
struct ClassicObj {
    number: u32,
    xref_generation: u16,
    body: Vec<u8>,
    free: bool,
}

fn obj(number: u32, body: &str) -> ClassicObj {
    ClassicObj {
        number,
        xref_generation: 0,
        body: body.as_bytes().to_vec(),
        free: false,
    }
}

fn obj_gen(number: u32, xref_generation: u16, body: &str) -> ClassicObj {
    ClassicObj {
        number,
        xref_generation,
        body: body.as_bytes().to_vec(),
        free: false,
    }
}

fn free_obj(number: u32) -> ClassicObj {
    ClassicObj {
        number,
        xref_generation: 65535,
        body: Vec::new(),
        free: true,
    }
}

/// Assemble a classic-xref PDF with one single-entry subsection per object.
fn classic_document(objects: &[ClassicObj], trailer_dict: &str) -> Vec<u8> {
    let mut source = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for object in objects {
        offsets.push(source.len());
        source.extend_from_slice(&object.body);
    }

    let xref_offset = source.len();
    source.extend_from_slice(b"xref\n");
    for (object, offset) in objects.iter().zip(&offsets) {
        source.extend_from_slice(format!("{} 1\n", object.number).as_bytes());
        if object.free {
            source.extend_from_slice(b"0000000000 65535 f \n");
        } else {
            source.extend_from_slice(
                format!("{offset:010} {gen:05} n \n", gen = object.xref_generation).as_bytes(),
            );
        }
    }
    source.extend_from_slice(
        format!("trailer\n{trailer_dict}\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
    );
    source
}

/// Object bodies shared by BOTH comprehensive fixtures, so the cross-backend
/// equivalence test compares indexes built over identical bodies: two pages
/// sharing one Form `XObject`, a content stream with an indirect `/Length`, a
/// trailer `/Info`, and one in-use object nothing references.
fn comprehensive_bodies() -> [(u32, &'static str); 9] {
    [
        (1, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"),
        (
            2,
            "2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n",
        ),
        (
            3,
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents 5 0 R \
             /Resources << /XObject << /Fx 6 0 R >> >> >>\nendobj\n",
        ),
        (
            4,
            "4 0 obj\n<< /Type /Page /Parent 2 0 R /Secret (do-not-copy) \
             /Resources << /XObject << /Fx 6 0 R >> >> >>\nendobj\n",
        ),
        (
            5,
            "5 0 obj\n<< /Length 7 0 R >>\nstream\nBT ET\nendstream\nendobj\n",
        ),
        (
            6,
            "6 0 obj\n<< /Subtype /Form /Length 0 >>\nstream\n\nendstream\nendobj\n",
        ),
        (7, "7 0 obj\n5\nendobj\n"),
        (8, "8 0 obj\n<< /Producer (p) >>\nendobj\n"),
        (9, "9 0 obj\n<< /Orphan true >>\nendobj\n"),
    ]
}

/// The comprehensive fixture over a classic-table backend.
fn comprehensive_classic_document() -> Vec<u8> {
    let objects: Vec<ClassicObj> = comprehensive_bodies()
        .iter()
        .map(|(number, body)| obj(*number, body))
        .collect();
    classic_document(&objects, "<< /Size 10 /Root 1 0 R /Info 8 0 R >>")
}

fn index_of(source: &[u8]) -> ObjectConsumerIndexInspection {
    let access = inspect_document_access(source).expect("fixture spine should resolve");
    inspect_object_consumer_index(source, &access)
}

fn trailer_key(name: &str) -> ObjectConsumerReferrer {
    ObjectConsumerReferrer::TrailerKey {
        key: PdfName(name.as_bytes().to_vec()),
    }
}

fn root_key(name: &str) -> ObjectConsumerReferrer {
    ObjectConsumerReferrer::RootKey {
        key: PdfName(name.as_bytes().to_vec()),
    }
}

fn page_referrer(page_index: usize, object_number: u32) -> ObjectConsumerReferrer {
    ObjectConsumerReferrer::Page {
        page_index,
        page: indirect_ref(object_number, 0),
    }
}

fn entry(target: IndirectRef, referrers: Vec<ObjectConsumerReferrer>) -> ObjectConsumersEntry {
    ObjectConsumersEntry { target, referrers }
}

fn referrers_of(
    report: &ObjectConsumerIndexInspection,
    object_number: u32,
) -> &ObjectConsumersEntry {
    report
        .entries
        .iter()
        .find(|entry| entry.target.object_number == object_number)
        .expect("target should have a consumer entry")
}

#[test]
fn classic_index_builds_expected_taxonomy() {
    let source = comprehensive_classic_document();
    let report = index_of(&source);

    assert_eq!(
        report.entries,
        vec![
            entry(indirect_ref(1, 0), vec![ObjectConsumerReferrer::Root]),
            entry(indirect_ref(2, 0), vec![root_key("Pages")]),
            entry(
                indirect_ref(3, 0),
                vec![root_key("Pages"), page_referrer(0, 3)],
            ),
            entry(
                indirect_ref(4, 0),
                vec![root_key("Pages"), page_referrer(1, 4)],
            ),
            entry(indirect_ref(5, 0), vec![page_referrer(0, 3)]),
            entry(
                indirect_ref(6, 0),
                vec![page_referrer(0, 3), page_referrer(1, 4)],
            ),
            entry(indirect_ref(7, 0), vec![page_referrer(0, 3)]),
            entry(indirect_ref(8, 0), vec![trailer_key("Info")]),
        ],
    );
    assert_eq!(report.unreferenced, vec![indirect_ref(9, 0)]);
    assert!(report.unresolved_edges.is_empty());
    assert!(report.skipped.is_empty());
    assert!(report.truncations.is_empty());
    assert_eq!(report.recorded_pair_count, 11);
    assert!(report.expanded_node_count > 0);
    assert_eq!(report.object_stream_cache.cached_container_count, 0);
    assert!(!report.object_stream_cache.dropped_over_budget);
}

#[test]
fn shared_xobject_has_two_page_referrers() {
    let source = comprehensive_classic_document();
    let report = index_of(&source);

    // The per-user visited sets are the load-bearing property here: a global
    // visited set would let only the first page register the shared XObject.
    assert_eq!(
        referrers_of(&report, 6).referrers,
        vec![page_referrer(0, 3), page_referrer(1, 4)],
    );
    // A page-interior object referenced by one page only has exactly one
    // referrer.
    assert_eq!(
        referrers_of(&report, 5).referrers,
        vec![page_referrer(0, 3)],
    );
}

#[test]
fn page_dictionaries_are_multi_user_by_design() {
    let source = comprehensive_classic_document();
    let report = index_of(&source);

    // PINNED: a page dictionary is registered by the RootKey(/Pages) descent
    // AND by its own Page user. This double registration keeps page objects
    // out of single-owner in-place mutation and must not be "fixed".
    assert_eq!(
        referrers_of(&report, 3).referrers,
        vec![root_key("Pages"), page_referrer(0, 3)],
    );
    // The page-tree root is owned by RootKey(/Pages) only: the pages' own
    // top-level /Parent edges are skipped and never climb back up.
    assert_eq!(referrers_of(&report, 2).referrers, vec![root_key("Pages")]);
}

#[test]
fn indirect_length_is_a_stream_parameter_consumer_edge() {
    let source = comprehensive_classic_document();
    let report = index_of(&source);

    // The `/Length 7 0 R` edge lives inside the content-stream dictionary
    // extent, so the length object belongs to the page that owns the stream.
    assert_eq!(
        referrers_of(&report, 7).referrers,
        vec![page_referrer(0, 3)],
    );
}

#[test]
fn dangling_free_and_generation_mismatch_are_unresolved_edge_facts() {
    let source = classic_document(
        &[
            obj(1, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"),
            obj(
                2,
                "2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
            ),
            obj(
                3,
                "3 0 obj\n<< /Type /Page /Parent 2 0 R /A 10 0 R /B 11 0 R /C 12 0 R >>\nendobj\n",
            ),
            free_obj(10),
            obj_gen(11, 3, "11 0 obj\n<< >>\nendobj\n"),
        ],
        "<< /Size 13 /Root 1 0 R >>",
    );
    let report = index_of(&source);

    // ISO 32000-1 7.3.10: an unresolvable reference denotes null. The edges
    // are facts, never consumer entries, and the index stays complete-shaped.
    assert_eq!(report.unresolved_edges.len(), 3);
    assert_eq!(report.unresolved_edges[0].target, indirect_ref(10, 0));
    assert!(matches!(
        report.unresolved_edges[0].resolution_reason,
        ObjectResolutionRejection::UnresolvedXrefLocation { .. }
    ));
    assert_eq!(report.unresolved_edges[1].target, indirect_ref(11, 0));
    assert!(matches!(
        report.unresolved_edges[1].resolution_reason,
        ObjectResolutionRejection::GenerationMismatch {
            requested_generation: 0,
            xref_generation: 3,
        }
    ));
    assert_eq!(report.unresolved_edges[2].target, indirect_ref(12, 0));
    assert!(matches!(
        report.unresolved_edges[2].resolution_reason,
        ObjectResolutionRejection::UnresolvedXrefLocation { .. }
    ));
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.target.object_number < 10)
    );
    assert!(report.truncations.is_empty());
}

#[test]
fn generation_mismatched_page_reference_is_an_unresolved_edge_not_a_consumer_edge() {
    let source = classic_document(
        &[
            obj(1, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"),
            obj(
                2,
                "2 0 obj\n<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>\nendobj\n",
            ),
            obj(
                3,
                "3 0 obj\n<< /Type /Page /Parent 2 0 R /Bad 4 1 R /Peer 4 0 R >>\nendobj\n",
            ),
            obj(4, "4 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n"),
        ],
        "<< /Size 5 /Root 1 0 R >>",
    );
    let report = index_of(&source);

    // The stop-at-other-page rule matches the FULL page-leaf reference: the
    // generation-mismatched `4 1 R` edge must go through resolution and land
    // as an unresolved-edge fact, never as a consumer edge, while the
    // correct-generation `4 0 R` edge is registered but not expanded.
    assert_eq!(report.unresolved_edges.len(), 1);
    assert_eq!(report.unresolved_edges[0].target, indirect_ref(4, 1));
    assert_eq!(report.unresolved_edges[0].referrer, page_referrer(0, 3));
    assert!(matches!(
        report.unresolved_edges[0].resolution_reason,
        ObjectResolutionRejection::GenerationMismatch {
            requested_generation: 1,
            xref_generation: 0,
        }
    ));
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.target != indirect_ref(4, 1))
    );
    assert_eq!(
        referrers_of(&report, 4).referrers,
        vec![root_key("Pages"), page_referrer(0, 3), page_referrer(1, 4)],
    );
}

#[test]
fn generation_mismatched_edge_to_visited_object_number_is_an_unresolved_edge() {
    // The page user seeds its own page object into the visited set. A
    // generation-mismatched self-edge (`3 1 R` inside page `3 0 R`) shares
    // that object NUMBER, so number-keyed visited suppression would drop it
    // silently; full-reference visited keying lets it reach resolution and
    // surface as an unresolved-edge fact.
    let source = classic_document(
        &[
            obj(1, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"),
            obj(
                2,
                "2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
            ),
            obj(
                3,
                "3 0 obj\n<< /Type /Page /Parent 2 0 R /Bad 3 1 R >>\nendobj\n",
            ),
        ],
        "<< /Size 4 /Root 1 0 R >>",
    );
    let report = index_of(&source);

    assert_eq!(report.unresolved_edges.len(), 1);
    assert_eq!(report.unresolved_edges[0].target, indirect_ref(3, 1));
    assert_eq!(report.unresolved_edges[0].referrer, page_referrer(0, 3));
    assert!(matches!(
        report.unresolved_edges[0].resolution_reason,
        ObjectResolutionRejection::GenerationMismatch {
            requested_generation: 1,
            xref_generation: 0,
        }
    ));
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.target != indirect_ref(3, 1))
    );
}

#[test]
fn self_referential_dictionary_terminates_via_visited_set() {
    let source = classic_document(
        &[
            obj(1, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"),
            obj(
                2,
                "2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
            ),
            obj(
                3,
                "3 0 obj\n<< /Type /Page /Parent 2 0 R /E 6 0 R >>\nendobj\n",
            ),
            obj(6, "6 0 obj\n<< /Self 6 0 R /Peer 6 0 R >>\nendobj\n"),
        ],
        "<< /Size 7 /Root 1 0 R >>",
    );
    let report = index_of(&source);

    assert_eq!(
        referrers_of(&report, 6).referrers,
        vec![page_referrer(0, 3)],
    );
    assert!(report.truncations.is_empty());
}

#[test]
fn depth_bound_yields_structured_truncation_fact() {
    let mut objects = vec![
        obj(1, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"),
        obj(
            2,
            "2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
        ),
        obj(
            3,
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /Chain 5 0 R >>\nendobj\n",
        ),
    ];
    for number in 5..=75u32 {
        let body = if number < 75 {
            format!("{number} 0 obj\n<< /Next {} 0 R >>\nendobj\n", number + 1)
        } else {
            format!("{number} 0 obj\n<< >>\nendobj\n")
        };
        objects.push(obj(number, &body));
    }
    let source = classic_document(&objects, "<< /Size 76 /Root 1 0 R >>");
    let report = index_of(&source);

    // The page seed is depth 0, chain node `k` is depth `k - 4`, so the edge
    // into object 69 is the first one past the per-user depth bound.
    let truncation = report
        .truncations
        .iter()
        .find(|fact| {
            matches!(
                fact.limit,
                ObjectConsumerIndexLimit::MaxTraversalDepth { max_depth }
                    if max_depth == MAX_OBJECT_CONSUMER_TRAVERSAL_DEPTH
            )
        })
        .expect("depth bound should surface a structured truncation fact");
    assert_eq!(truncation.referrer, Some(page_referrer(0, 3)));
    assert_eq!(truncation.target, Some(indirect_ref(69, 0)));
    assert!(referrers_of(&report, 68).referrers == vec![page_referrer(0, 3)]);
    assert!(report.unreferenced.contains(&indirect_ref(69, 0)));
}

fn xref_record(entry_type: u8, field2: usize, field3: u8) -> [u8; 4] {
    let [hi, lo] = u16::try_from(field2)
        .expect("test xref field fits u16")
        .to_be_bytes();
    [entry_type, hi, lo, field3]
}

fn xref_record_w4(entry_type: u8, field2: usize, field3: u8) -> [u8; 6] {
    let bytes = u32::try_from(field2)
        .expect("test xref field fits u32")
        .to_be_bytes();
    [entry_type, bytes[0], bytes[1], bytes[2], bytes[3], field3]
}

/// The comprehensive fixture rebuilt over an xref-stream backend with the
/// same object numbers and bodies, all as uncompressed type-1 entries.
fn comprehensive_xref_stream_document() -> Vec<u8> {
    let bodies = comprehensive_bodies();

    let mut source = b"%PDF-1.5\n".to_vec();
    let mut offsets = Vec::new();
    for (_, body) in &bodies {
        offsets.push(source.len());
        source.extend_from_slice(body.as_bytes());
    }
    let xref_offset = source.len();

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0));
    for offset in &offsets {
        records.extend_from_slice(&xref_record(1, *offset, 0));
    }
    records.extend_from_slice(&xref_record(1, xref_offset, 0));

    source.extend_from_slice(
        format!(
            "10 0 obj\n<< /Type /XRef /Size 11 /W [ 1 2 1 ] /Index [ 0 11 ] \
             /Root 1 0 R /Info 8 0 R /Length {} >>\nstream\n",
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
fn xref_stream_backend_produces_equivalent_entries() {
    let classic = index_of(&comprehensive_classic_document());
    let xref_stream = index_of(&comprehensive_xref_stream_document());

    // Cross-backend: the consumer entries are shape-identical; only the diff
    // differs because the xref-stream object itself is an in-use entry nothing
    // references.
    assert_eq!(classic.entries, xref_stream.entries);
    assert_eq!(
        xref_stream.unreferenced,
        vec![indirect_ref(9, 0), indirect_ref(10, 0)],
    );
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

fn oversized_compressed_member_document() -> Vec<u8> {
    let prefix = b"%PDF-1.5\n";
    let catalog: &[u8] = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let page_tree: &[u8] = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page: &[u8] = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Payload 10 0 R >>\nendobj\n";

    let mut decoded = b"10 0 << /Marker true >>".to_vec();
    decoded.resize(MAX_XREF_STREAM_SECTION_DECODED_BYTES + 1, b' ');
    let objstm = {
        let mut object = format!(
            "5 0 obj\n<< /Type /ObjStm /N 1 /First 5 /Length {} >>\nstream\n",
            decoded.len()
        )
        .into_bytes();
        object.extend_from_slice(&decoded);
        object.extend_from_slice(b"\nendstream\nendobj\n");
        object
    };

    let mut source = prefix.to_vec();
    let catalog_offset = source.len();
    source.extend_from_slice(catalog);
    let page_tree_offset = source.len();
    source.extend_from_slice(page_tree);
    let page_offset = source.len();
    source.extend_from_slice(page);
    let objstm_offset = source.len();
    source.extend_from_slice(&objstm);
    let xref_offset = source.len();

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record_w4(0, 0, 0));
    records.extend_from_slice(&xref_record_w4(1, catalog_offset, 0));
    records.extend_from_slice(&xref_record_w4(1, page_tree_offset, 0));
    records.extend_from_slice(&xref_record_w4(1, page_offset, 0));
    records.extend_from_slice(&xref_record_w4(0, 0, 0));
    records.extend_from_slice(&xref_record_w4(1, objstm_offset, 0));
    for _ in 6..10 {
        records.extend_from_slice(&xref_record_w4(0, 0, 0));
    }
    records.extend_from_slice(&xref_record_w4(2, 5, 0));
    records.extend_from_slice(&xref_record_w4(1, xref_offset, 0));

    source.extend_from_slice(
        format!(
            "11 0 obj\n<< /Type /XRef /Size 12 /W [ 1 4 1 ] /Index [ 0 12 ] \
             /Root 1 0 R /Length {} >>\nstream\n",
            records.len()
        )
        .as_bytes(),
    );
    source.extend_from_slice(&records);
    source.extend_from_slice(b"\nendstream\nendobj\n");
    source.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF\n").as_bytes());
    source
}

/// A fully compressed structural spine (catalog, pages root, two pages inside
/// `/ObjStm` 5) where the second compressed page references an uncompressed
/// object 6.
fn compressed_members_document() -> Vec<u8> {
    let prefix = b"%PDF-1.5\n";
    let catalog: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
    let page_tree_root: &[u8] = b"<< /Type /Pages /Kids [ 3 0 R 4 0 R ] /Count 2 >>";
    let first_leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R >>";
    let second_leaf: &[u8] = b"<< /Type /Page /Parent 2 0 R /Fx 6 0 R >>";
    let objstm = object_stream(&[
        (1, catalog),
        (2, page_tree_root),
        (3, first_leaf),
        (4, second_leaf),
    ]);
    let marker: &[u8] = b"6 0 obj\n<< /Marker true >>\nendobj\n";

    let objstm_offset = prefix.len();
    let mut source = prefix.to_vec();
    source.extend_from_slice(&objstm);
    let marker_offset = source.len();
    source.extend_from_slice(marker);
    let xref_offset = source.len();

    let mut records = Vec::new();
    records.extend_from_slice(&xref_record(0, 0, 0));
    for index in 0..4 {
        records.extend_from_slice(&xref_record(2, 5, index));
    }
    records.extend_from_slice(&xref_record(1, objstm_offset, 0));
    records.extend_from_slice(&xref_record(1, marker_offset, 0));
    records.extend_from_slice(&xref_record(1, xref_offset, 0));

    source.extend_from_slice(
        format!(
            "7 0 obj\n<< /Type /XRef /Size 8 /W [ 1 2 1 ] /Index [ 0 8 ] \
             /Root 1 0 R /Length {} >>\nstream\n",
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
fn compressed_members_are_transparent_and_container_is_never_a_consumer_target() {
    let source = compressed_members_document();
    let report = index_of(&source);

    // The reference inside compressed member 4 is an edge of the MEMBER: the
    // uncompressed target carries the page referrer, not the container.
    assert_eq!(
        referrers_of(&report, 6).referrers,
        vec![page_referrer(1, 4)],
    );
    assert_eq!(
        referrers_of(&report, 3).referrers,
        vec![root_key("Pages"), page_referrer(0, 3)],
    );
    // Nothing references the `/ObjStm` container or the xref stream: both
    // appear only in the unreferenced diff.
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.target.object_number != 5)
    );
    assert_eq!(
        report.unreferenced,
        vec![indirect_ref(5, 0), indirect_ref(7, 0)],
    );
    // The container decoded once into the cache; later members reused it.
    assert_eq!(report.object_stream_cache.cached_container_count, 1);
    assert!(report.object_stream_cache.cached_byte_count > 0);
    assert!(!report.object_stream_cache.dropped_over_budget);
    assert!(report.unresolved_edges.is_empty());
    assert!(report.truncations.is_empty());
}

#[test]
fn oversized_object_stream_member_decode_cap_is_a_truncation_fact() {
    let source = oversized_compressed_member_document();
    let report = index_of(&source);

    assert_eq!(report.unresolved_edges.len(), 1);
    assert_eq!(report.unresolved_edges[0].target, indirect_ref(10, 0));
    assert_eq!(report.unresolved_edges[0].referrer, page_referrer(0, 3));
    assert!(matches!(
        report.unresolved_edges[0].resolution_reason,
        ObjectResolutionRejection::ObjectStreamMemberExtraction {
            extraction_reason:
                ObjectStreamMemberExtractionRejection::DecodedObjectStreamTooLarge {
                    length,
                    limit,
                },
        } if length == MAX_XREF_STREAM_SECTION_DECODED_BYTES + 1
            && limit == MAX_XREF_STREAM_SECTION_DECODED_BYTES
    ));

    assert_eq!(report.truncations.len(), 1);
    let truncation = &report.truncations[0];
    assert_eq!(truncation.referrer, Some(page_referrer(0, 3)));
    assert_eq!(truncation.target, Some(indirect_ref(10, 0)));
    assert!(matches!(
        truncation.limit,
        ObjectConsumerIndexLimit::MaxDecodedObjectStreamBytes {
            decoded_length,
            max_decoded_object_stream_bytes,
        } if decoded_length == MAX_XREF_STREAM_SECTION_DECODED_BYTES + 1
            && max_decoded_object_stream_bytes == MAX_XREF_STREAM_SECTION_DECODED_BYTES
    ));
    assert!(
        report
            .entries
            .iter()
            .all(|entry| entry.target != indirect_ref(10, 0))
    );
}

#[test]
fn index_is_deterministic_and_serde_shape_locked() {
    let source = comprehensive_classic_document();
    let first = index_of(&source);
    let second = index_of(&source);
    assert_eq!(first, second);

    let value = serde_value(&first).expect("index report should serialize");
    let rendered = format!("{value:?}");
    assert!(rendered.contains(r#"("referrer", String("root"))"#));
    assert!(rendered.contains(r#"("referrer", String("root_key"))"#));
    assert!(rendered.contains(r#"("referrer", String("trailer_key"))"#));
    assert!(rendered.contains(r#"("referrer", String("page"))"#));
    let oversized = serde_value(&index_of(&oversized_compressed_member_document()))
        .expect("truncated index report should serialize");
    assert!(format!("{oversized:?}").contains("max_decoded_object_stream_bytes"));
    let restored: ObjectConsumerIndexInspection =
        from_serde_value(value).expect("index report should deserialize");
    assert_eq!(restored, first);

    // No PDF value bytes leak into the report; only key NAME bytes are
    // retained by design.
    assert!(!format!("{first:?}").contains("do-not-copy"));
}

#[test]
fn unresolved_edge_report_roundtrips_serde() {
    let source = classic_document(
        &[
            obj(1, "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"),
            obj(
                2,
                "2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n",
            ),
            obj(
                3,
                "3 0 obj\n<< /Type /Page /Parent 2 0 R /A 12 0 R >>\nendobj\n",
            ),
        ],
        "<< /Size 13 /Root 1 0 R >>",
    );
    let report = index_of(&source);
    assert_eq!(report.unresolved_edges.len(), 1);

    let value = serde_value(&report).expect("report with unresolved edges should serialize");
    let restored: ObjectConsumerIndexInspection =
        from_serde_value(value).expect("report with unresolved edges should deserialize");
    assert_eq!(restored, report);
}
