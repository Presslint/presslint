use presslint_core::{ColorSpace, PdfName};

use super::json::Json;
use super::{ColorPolicy, OverprintPolicy, SpotPolicy, TransformRequest, assert_json_round_trip};
use crate::{
    DeviceLinkDecision, DeviceLinkDescription, DeviceLinkPolicy, DeviceLinkRejection,
    resolve_device_link_policy,
};

fn request(source: ColorSpace, destination: ColorSpace) -> TransformRequest {
    TransformRequest {
        source,
        destination,
        policy: ColorPolicy {
            spot: SpotPolicy::Preserve,
            overprint: OverprintPolicy::Preserve,
        },
    }
}

fn link(id: &str, source: ColorSpace, destination: ColorSpace) -> DeviceLinkDescription {
    DeviceLinkDescription {
        id: id.to_owned(),
        source,
        destination,
    }
}

fn rgb_to_cmyk_request() -> TransformRequest {
    request(ColorSpace::DeviceRgb, ColorSpace::DeviceCmyk)
}

fn rgb_to_cmyk_link(id: &str) -> DeviceLinkDescription {
    link(id, ColorSpace::DeviceRgb, ColorSpace::DeviceCmyk)
}

fn rgb_to_cmyk_link_json(id: &'static str) -> Json {
    Json::object([
        ("id", Json::string(id)),
        ("source", Json::string("device_rgb")),
        ("destination", Json::string("device_cmyk")),
    ])
}

#[test]
fn require_uses_exact_matching_device_link() {
    assert_eq!(
        resolve_device_link_policy(
            DeviceLinkPolicy::Require,
            &rgb_to_cmyk_request(),
            [rgb_to_cmyk_link("rgb-to-cmyk")],
        ),
        DeviceLinkDecision::UseDeviceLink {
            device_link: rgb_to_cmyk_link("rgb-to-cmyk"),
        },
    );
}

#[test]
fn require_rejects_when_no_matching_device_link_is_available() {
    assert_eq!(
        resolve_device_link_policy(DeviceLinkPolicy::Require, &rgb_to_cmyk_request(), []),
        DeviceLinkDecision::Rejected {
            rejection: DeviceLinkRejection::NoMatchingDeviceLink,
        },
    );
}

#[test]
fn prefer_uses_matching_device_link_when_present() {
    assert_eq!(
        resolve_device_link_policy(
            DeviceLinkPolicy::Prefer,
            &rgb_to_cmyk_request(),
            [rgb_to_cmyk_link("preferred")],
        ),
        DeviceLinkDecision::UseDeviceLink {
            device_link: rgb_to_cmyk_link("preferred"),
        },
    );
}

#[test]
fn prefer_falls_back_to_profile_connection_space_without_match() {
    assert_eq!(
        resolve_device_link_policy(
            DeviceLinkPolicy::Prefer,
            &rgb_to_cmyk_request(),
            [link(
                "gray-to-cmyk",
                ColorSpace::DeviceGray,
                ColorSpace::DeviceCmyk
            )],
        ),
        DeviceLinkDecision::UseProfileConnectionSpace,
    );
}

#[test]
fn forbid_ignores_available_device_links_and_uses_profile_connection_space() {
    assert_eq!(
        resolve_device_link_policy(
            DeviceLinkPolicy::Forbid,
            &rgb_to_cmyk_request(),
            [rgb_to_cmyk_link("ignored")],
        ),
        DeviceLinkDecision::UseProfileConnectionSpace,
    );
}

#[test]
fn destination_mismatch_does_not_match() {
    assert_eq!(
        resolve_device_link_policy(
            DeviceLinkPolicy::Require,
            &rgb_to_cmyk_request(),
            [link(
                "rgb-to-gray",
                ColorSpace::DeviceRgb,
                ColorSpace::DeviceGray
            )],
        ),
        DeviceLinkDecision::Rejected {
            rejection: DeviceLinkRejection::NoMatchingDeviceLink,
        },
    );
}

#[test]
fn source_mismatch_does_not_match() {
    assert_eq!(
        resolve_device_link_policy(
            DeviceLinkPolicy::Require,
            &rgb_to_cmyk_request(),
            [link("lab-to-cmyk", ColorSpace::Lab, ColorSpace::DeviceCmyk)],
        ),
        DeviceLinkDecision::Rejected {
            rejection: DeviceLinkRejection::NoMatchingDeviceLink,
        },
    );
}

#[test]
fn exact_match_includes_named_resource_color_spaces() {
    let resource = ColorSpace::Resource(PdfName(b"SourceCS".to_vec()));
    let request = request(resource.clone(), ColorSpace::DeviceCmyk);
    let matching = link("resource-to-cmyk", resource, ColorSpace::DeviceCmyk);

    assert_eq!(
        resolve_device_link_policy(DeviceLinkPolicy::Require, &request, [matching.clone()]),
        DeviceLinkDecision::UseDeviceLink {
            device_link: matching,
        },
    );
}

#[test]
fn first_matching_device_link_is_selected_deterministically() {
    assert_eq!(
        resolve_device_link_policy(
            DeviceLinkPolicy::Prefer,
            &rgb_to_cmyk_request(),
            [
                rgb_to_cmyk_link("first"),
                rgb_to_cmyk_link("second"),
                link(
                    "gray-to-cmyk",
                    ColorSpace::DeviceGray,
                    ColorSpace::DeviceCmyk
                ),
            ],
        ),
        DeviceLinkDecision::UseDeviceLink {
            device_link: rgb_to_cmyk_link("first"),
        },
    );
}

#[test]
fn device_link_policy_variants_have_stable_json_shape() {
    assert_json_round_trip(&DeviceLinkPolicy::Require, Json::string("require"));
    assert_json_round_trip(&DeviceLinkPolicy::Prefer, Json::string("prefer"));
    assert_json_round_trip(&DeviceLinkPolicy::Forbid, Json::string("forbid"));
}

#[test]
fn device_link_description_has_stable_json_shape() {
    assert_json_round_trip(
        &rgb_to_cmyk_link("rgb-to-cmyk"),
        rgb_to_cmyk_link_json("rgb-to-cmyk"),
    );
}

#[test]
fn device_link_rejection_has_stable_json_shape() {
    assert_json_round_trip(
        &DeviceLinkRejection::NoMatchingDeviceLink,
        Json::object([("reason", Json::string("no_matching_device_link"))]),
    );
}

#[test]
fn device_link_decision_use_device_link_has_stable_json_shape() {
    assert_json_round_trip(
        &DeviceLinkDecision::UseDeviceLink {
            device_link: rgb_to_cmyk_link("rgb-to-cmyk"),
        },
        Json::object([
            ("decision", Json::string("use_device_link")),
            ("device_link", rgb_to_cmyk_link_json("rgb-to-cmyk")),
        ]),
    );
}

#[test]
fn device_link_decision_use_profile_connection_space_has_stable_json_shape() {
    assert_json_round_trip(
        &DeviceLinkDecision::UseProfileConnectionSpace,
        Json::object([("decision", Json::string("use_profile_connection_space"))]),
    );
}

#[test]
fn device_link_decision_rejected_has_stable_json_shape() {
    assert_json_round_trip(
        &DeviceLinkDecision::Rejected {
            rejection: DeviceLinkRejection::NoMatchingDeviceLink,
        },
        Json::object([
            ("decision", Json::string("rejected")),
            (
                "rejection",
                Json::object([("reason", Json::string("no_matching_device_link"))]),
            ),
        ]),
    );
}
