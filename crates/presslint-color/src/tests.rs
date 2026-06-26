#![allow(clippy::expect_used, clippy::missing_errors_doc)]

mod json;

use std::fmt;

use presslint_core::{ColorSpace, PdfName};
use serde::{Deserialize, Serialize};

use self::json::{Json, JsonSerializer};
use super::{
    ColorPolicy, NamedOutputCondition, OutputIntentPolicy, OutputIntentSubtype, OutputIntentTarget,
    OutputProfileSource, OverprintPolicy, ProfileBackedOutputIntent, SpotPolicy, TransformRequest,
};

fn assert_json_round_trip<T>(value: &T, expected: Json)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + fmt::Debug,
{
    let encoded = value.serialize(JsonSerializer).expect("serialize value");
    assert_eq!(encoded, expected);

    let decoded = T::deserialize(expected).expect("deserialize fixture");
    assert_eq!(&decoded, value);
}

// --- Color policy and transform shape tests ----------------------------------
//
// These lock the public JSON encoding of the color policy and abstract
// transform contracts. Each fixture asserts a full round-trip: the value
// serializes to the locked `Json` tree and that tree deserializes back to the
// equal value. The fixtures assert the `snake_case` variant names exactly as
// the current `#[serde(rename_all = "snake_case")]` attributes emit them and
// the struct field names exactly as `ColorPolicy`/`TransformRequest` declare
// them; if a fixture and the code disagree, the fixture is wrong.

#[test]
fn spot_policy_variants_have_stable_json_shape() {
    assert_json_round_trip(&SpotPolicy::Preserve, Json::string("preserve"));
    assert_json_round_trip(&SpotPolicy::Reject, Json::string("reject"));
    assert_json_round_trip(
        &SpotPolicy::ConvertAlternate,
        Json::string("convert_alternate"),
    );
}

#[test]
fn overprint_policy_variants_have_stable_json_shape() {
    assert_json_round_trip(&OverprintPolicy::Preserve, Json::string("preserve"));
    assert_json_round_trip(
        &OverprintPolicy::RejectUnsafe,
        Json::string("reject_unsafe"),
    );
    assert_json_round_trip(&OverprintPolicy::Mitigate, Json::string("mitigate"));
}

#[test]
fn color_policy_has_stable_json_shape() {
    assert_json_round_trip(&color_policy(), color_policy_json());
}

#[test]
fn transform_request_has_stable_json_shape() {
    // The request pins both a unit `ColorSpace` variant (`device_cmyk`) and the
    // `Resource(PdfName)` newtype variant, so the nested `presslint-core`
    // `ColorSpace` encoding is locked inside the request.
    assert_json_round_trip(
        &TransformRequest {
            source: ColorSpace::DeviceCmyk,
            destination: ColorSpace::Resource(PdfName(b"PressLintLink".to_vec())),
            policy: color_policy(),
        },
        Json::object([
            ("source", Json::string("device_cmyk")),
            (
                "destination",
                Json::object([(
                    "resource",
                    Json::array(
                        b"PressLintLink"
                            .iter()
                            .map(|byte| Json::U32(u32::from(*byte))),
                    ),
                )]),
            ),
            ("policy", color_policy_json()),
        ]),
    );
}

fn color_policy() -> ColorPolicy {
    ColorPolicy {
        spot: SpotPolicy::ConvertAlternate,
        overprint: OverprintPolicy::RejectUnsafe,
    }
}

fn color_policy_json() -> Json {
    Json::object([
        ("spot", Json::string("convert_alternate")),
        ("overprint", Json::string("reject_unsafe")),
    ])
}

// --- Output-intent shape tests -----------------------------------------------
//
// These lock the public JSON encoding of the output-intent contracts. Each
// fixture asserts a full round-trip exactly as the current `#[serde(...)]`
// attributes emit it.

#[test]
fn output_intent_policy_variants_have_stable_json_shape() {
    assert_json_round_trip(
        &OutputIntentPolicy::Preserve,
        Json::object([("policy", Json::string("preserve"))]),
    );
    assert_json_round_trip(
        &OutputIntentPolicy::RequireExisting,
        Json::object([("policy", Json::string("require_existing"))]),
    );
    assert_json_round_trip(
        &OutputIntentPolicy::EnsureTarget {
            target: OutputIntentTarget::NamedCondition {
                condition: named_condition(),
            },
        },
        Json::object([
            ("policy", Json::string("ensure_target")),
            (
                "target",
                Json::object([
                    ("kind", Json::string("named_condition")),
                    ("condition", named_condition_json()),
                ]),
            ),
        ]),
    );
}

#[test]
fn output_intent_target_variants_have_stable_json_shape() {
    assert_json_round_trip(
        &OutputIntentTarget::NamedCondition {
            condition: named_condition(),
        },
        Json::object([
            ("kind", Json::string("named_condition")),
            ("condition", named_condition_json()),
        ]),
    );
    assert_json_round_trip(
        &OutputIntentTarget::ProfileBacked {
            intent: profile_backed_intent(),
        },
        Json::object([
            ("kind", Json::string("profile_backed")),
            ("intent", profile_backed_intent_json()),
        ]),
    );
}

#[test]
fn output_intent_subtype_has_stable_json_shape() {
    assert_json_round_trip(&OutputIntentSubtype::GtsPdfx, Json::string("gts_pdfx"));
    assert_json_round_trip(&OutputIntentSubtype::GtsPdfa1, Json::string("gts_pdfa1"));
    assert_json_round_trip(&OutputIntentSubtype::IsoPdfe1, Json::string("iso_pdfe1"));
}

#[test]
fn named_output_condition_has_stable_json_shape() {
    assert_json_round_trip(&named_condition(), named_condition_json());
}

#[test]
fn profile_backed_output_intent_has_stable_json_shape() {
    assert_json_round_trip(&profile_backed_intent(), profile_backed_intent_json());
}

#[test]
fn output_profile_source_variants_have_stable_json_shape() {
    assert_json_round_trip(
        &OutputProfileSource::OpaqueId {
            id: "profile:pso-coated-v3".to_owned(),
        },
        Json::object([
            ("source", Json::string("opaque_id")),
            ("id", Json::string("profile:pso-coated-v3")),
        ]),
    );
    assert_json_round_trip(
        &OutputProfileSource::EmbeddedBytes {
            bytes: vec![0, 1, 2, 255],
        },
        Json::object([
            ("source", Json::string("embedded_bytes")),
            (
                "bytes",
                Json::array([Json::U32(0), Json::U32(1), Json::U32(2), Json::U32(255)]),
            ),
        ]),
    );
}

fn named_condition() -> NamedOutputCondition {
    NamedOutputCondition {
        subtype: OutputIntentSubtype::GtsPdfx,
        output_condition_identifier: "FOGRA51".to_owned(),
        registry_name: "http://www.color.org".to_owned(),
    }
}

fn named_condition_json() -> Json {
    Json::object([
        ("subtype", Json::string("gts_pdfx")),
        ("output_condition_identifier", Json::string("FOGRA51")),
        ("registry_name", Json::string("http://www.color.org")),
    ])
}

fn profile_backed_intent() -> ProfileBackedOutputIntent {
    ProfileBackedOutputIntent {
        subtype: OutputIntentSubtype::GtsPdfx,
        output_condition_identifier: "Custom".to_owned(),
        output_condition: "Coated".to_owned(),
        info: "Coated 150lpi".to_owned(),
        profile: OutputProfileSource::OpaqueId {
            id: "profiles/coated.icc".to_owned(),
        },
    }
}

fn profile_backed_intent_json() -> Json {
    Json::object([
        ("subtype", Json::string("gts_pdfx")),
        ("output_condition_identifier", Json::string("Custom")),
        ("output_condition", Json::string("Coated")),
        ("info", Json::string("Coated 150lpi")),
        (
            "profile",
            Json::object([
                ("source", Json::string("opaque_id")),
                ("id", Json::string("profiles/coated.icc")),
            ]),
        ),
    ])
}
