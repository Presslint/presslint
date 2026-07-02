//! Serde shape locks for the backend-agnostic incremental-revision plan.
//!
//! These fixtures pin the public JSON encoding of [`IncrementalRevisionPlan`]
//! and [`PlannedDirtyObject`]: field order, the nested `dictionary_entry`
//! boundary shape, and the `body_bytes` byte-array encoding. If a fixture and
//! the code disagree, the fixture is wrong.

use presslint_pdf::{
    IndirectObjectEditDecision, IndirectObjectEditDisposition, IndirectObjectOwnership, IndirectRef,
};
use presslint_types::{ByteRange, PdfName};
use serde::Deserialize;

use super::super::{
    DictionaryEntryOp, DictionaryValueLocator, IncrementalRevisionPlan, MutationBoundary,
    PlannedDirtyObject, PlannedValueProvenance,
};
use super::*;

fn indirect_ref(object_number: u32, generation: u16) -> IndirectRef {
    IndirectRef {
        object_number,
        generation,
    }
}

fn indirect_ref_json(object_number: u32, generation: u32) -> Json {
    Json::object([
        ("object_number", Json::U32(object_number)),
        ("generation", Json::U32(generation)),
    ])
}

fn byte_range_json(start: u32, end: u32) -> Json {
    Json::object([("start", Json::U32(start)), ("end", Json::U32(end))])
}

fn in_place_ownership(target: IndirectRef, owner: IndirectRef) -> IndirectObjectEditDecision {
    IndirectObjectEditDecision {
        target,
        ownership: IndirectObjectOwnership::ProvenSingleUse { owner },
        disposition: IndirectObjectEditDisposition::InPlaceMutation,
    }
}

fn in_place_ownership_json(target: (u32, u32), owner: (u32, u32)) -> Json {
    Json::object([
        ("target", indirect_ref_json(target.0, target.1)),
        (
            "ownership",
            Json::object([
                ("status", Json::string("proven_single_use")),
                ("owner", indirect_ref_json(owner.0, owner.1)),
            ]),
        ),
        ("disposition", Json::string("in_place_mutation")),
    ])
}

/// A `dictionary_entry` boundary that rewrites `/MediaBox` on `target`.
fn media_box_boundary(target: IndirectRef, owner: IndirectRef) -> MutationBoundary {
    MutationBoundary::DictionaryEntry {
        target,
        key: PdfName(b"MediaBox".to_vec()),
        op: DictionaryEntryOp::Replace,
        value_locator: DictionaryValueLocator::ExistingValue {
            key_range: ByteRange { start: 20, end: 29 },
            value_range: ByteRange { start: 30, end: 52 },
        },
        ownership: in_place_ownership(target, owner),
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: target },
    }
}

fn media_box_boundary_json(target: (u32, u32), owner: (u32, u32)) -> Json {
    Json::object([
        ("kind", Json::string("dictionary_entry")),
        ("target", indirect_ref_json(target.0, target.1)),
        (
            "key",
            Json::array(
                b"MediaBox"
                    .iter()
                    .copied()
                    .map(|byte| Json::U32(u32::from(byte))),
            ),
        ),
        ("op", Json::string("replace")),
        (
            "value_locator",
            Json::object([
                ("kind", Json::string("existing_value")),
                ("key_range", byte_range_json(20, 29)),
                ("value_range", byte_range_json(30, 52)),
            ]),
        ),
        ("ownership", in_place_ownership_json(target, owner)),
        (
            "value_provenance",
            Json::object([
                ("kind", Json::string("derived_from_object")),
                ("object", indirect_ref_json(target.0, target.1)),
            ]),
        ),
    ])
}

/// JSON array for a `Vec<u8>` body payload (each byte encodes as a `U32`).
fn body_bytes_json(body: &[u8]) -> Json {
    Json::array(body.iter().copied().map(|byte| Json::U32(u32::from(byte))))
}

#[test]
fn planned_dirty_object_has_stable_json_shape() {
    let target = indirect_ref(4, 0);
    let owner = indirect_ref(2, 0);
    let body = b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 400] >>";

    assert_json_round_trip(
        &PlannedDirtyObject {
            reference: target,
            boundaries: vec![media_box_boundary(target, owner)],
            body_bytes: body.to_vec(),
        },
        Json::object([
            ("reference", indirect_ref_json(4, 0)),
            (
                "boundaries",
                Json::array([media_box_boundary_json((4, 0), (2, 0))]),
            ),
            ("body_bytes", body_bytes_json(body)),
        ]),
    );
}

#[test]
fn incremental_revision_plan_has_stable_json_shape() {
    let target = indirect_ref(7, 0);
    let owner = indirect_ref(3, 0);
    let body = b"<< /Type /Page >>";

    assert_json_round_trip(
        &IncrementalRevisionPlan {
            dirty_objects: vec![PlannedDirtyObject {
                reference: target,
                boundaries: vec![media_box_boundary(target, owner)],
                body_bytes: body.to_vec(),
            }],
        },
        Json::object([(
            "dirty_objects",
            Json::array([Json::object([
                ("reference", indirect_ref_json(7, 0)),
                (
                    "boundaries",
                    Json::array([media_box_boundary_json((7, 0), (3, 0))]),
                ),
                ("body_bytes", body_bytes_json(body)),
            ])]),
        )]),
    );
}

#[test]
fn plan_carries_multiple_boundaries_for_one_object() {
    let target = indirect_ref(5, 0);
    let owner = indirect_ref(2, 0);
    let crop = MutationBoundary::DictionaryEntry {
        target,
        key: PdfName(b"CropBox".to_vec()),
        op: DictionaryEntryOp::Insert,
        value_locator: DictionaryValueLocator::InsertionPoint {
            dictionary_range: ByteRange { start: 8, end: 60 },
        },
        ownership: in_place_ownership(target, owner),
        value_provenance: PlannedValueProvenance::DerivedFromObject { object: target },
    };
    let planned = PlannedDirtyObject {
        reference: target,
        boundaries: vec![media_box_boundary(target, owner), crop],
        body_bytes: b"<< >>".to_vec(),
    };

    // Round-trip preserves both boundaries in order; the plan carries dirty
    // intent only and never rejects on its own (validation lives in the writer).
    let encoded = planned.serialize(JsonSerializer).expect("serialize");
    let decoded = PlannedDirtyObject::deserialize(encoded).expect("decode");
    assert_eq!(decoded, planned);
    assert_eq!(decoded.boundaries.len(), 2);
}
