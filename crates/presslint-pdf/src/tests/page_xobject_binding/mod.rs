//! Shared fixtures for the page `/XObject` binding witness tests.
//!
//! Every fixture is a classic-xref document whose object 1 is the catalog and
//! whose object 2 is the page-tree root, so the same source feeds both the
//! binding inspector (rooted at object 2) and the document consumer index
//! (rooted through the trailer).

mod locality;
mod ownership;
mod refusal;

use crate::{
    ClassicXrefTableInspection, DocumentPageXObjectBindingsInspection, IndirectRef,
    ObjectConsumerIndexInspection, ObjectStreamCacheReport, XrefStreamEntry, XrefStreamEntryRecord,
    XrefStreamSection, inspect_classic_xref_table, inspect_document_access,
    inspect_document_page_xobject_bindings, inspect_object_consumer_index,
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

    /// Build the real document consumer index over this fixture.
    fn consumers(&self) -> ObjectConsumerIndexInspection {
        let access = inspect_document_access(&self.source).expect("document access should inspect");
        inspect_object_consumer_index(&self.source, &access)
    }

    /// Inspect bindings with a freshly computed consumer index.
    fn inspect(&self) -> DocumentPageXObjectBindingsInspection {
        self.inspect_with(&self.consumers())
    }

    /// Inspect bindings against a caller-supplied consumer index.
    fn inspect_with(
        &self,
        consumers: &ObjectConsumerIndexInspection,
    ) -> DocumentPageXObjectBindingsInspection {
        inspect_document_page_xobject_bindings(
            &self.source,
            &self.xref,
            self.object_offset(2),
            consumers,
        )
        .expect("page xobject bindings should inspect")
    }

    /// Hand-built single-section xref-stream lookup over the same object
    /// offsets,
    /// with per-object record overrides (e.g. compressed members).
    fn xref_stream_section(
        &self,
        overrides: &[(usize, XrefStreamEntryRecord)],
    ) -> XrefStreamSection {
        let entries = (1..=self.offsets.len())
            .map(|object_number| XrefStreamEntry {
                object_number,
                record: overrides
                    .iter()
                    .find(|(overridden, _)| *overridden == object_number)
                    .map_or_else(
                        || XrefStreamEntryRecord::Uncompressed {
                            byte_offset: self.object_offset(object_number),
                            generation: 0,
                        },
                        |(_, record)| *record,
                    ),
            })
            .collect();
        XrefStreamSection {
            object_byte_offset: 0,
            widths: [1, 2, 1],
            size: self.offsets.len() + 1,
            index_subsections: Vec::new(),
            root_reference: indirect_ref(1, 0),
            prev_byte_offset: None,
            entries,
        }
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

const fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

/// A synthetic COMPLETE-but-empty consumer index: no facts at all, so the
/// completeness gate passes while every exclusivity check conservatively
/// fails. Useful for refusal tests that never reach a verdict.
const fn empty_consumers(byte_len: usize) -> ObjectConsumerIndexInspection {
    ObjectConsumerIndexInspection {
        byte_len,
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
            dropped_over_budget: false,
        },
        unreferenced: Vec::new(),
    }
}

/// Standard catalog and page-tree-root objects shared by most fixtures.
const CATALOG: &[u8] = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";

/// A form `XObject` stream object body with the given object number and extra
/// dictionary keys.
fn form_object(object_number: usize, extra: &str) -> Vec<u8> {
    format!(
        "{object_number} 0 obj\n<< /Type /XObject /Subtype /Form /BBox [0 0 1 1]{extra} /Length 0 >>\nstream\n\nendstream\nendobj\n"
    )
    .into_bytes()
}
