use std::collections::HashSet;

use presslint_core::{ColorSpace, PdfName};

use super::assert_json_round_trip;
use super::json::Json;
use super::{ColorPolicy, OverprintPolicy, SpotPolicy, TransformRequest};
use crate::{
    DeviceLinkDecision, DeviceLinkDescription, DeviceLinkRejection, ProfileReference,
    TransformCacheKey, TransformCacheKeyDecision, TransformCacheKeyRejection,
    derive_transform_cache_key,
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

fn rgb_to_cmyk_request() -> TransformRequest {
    request(ColorSpace::DeviceRgb, ColorSpace::DeviceCmyk)
}

fn use_link(id: &str) -> DeviceLinkDecision {
    DeviceLinkDecision::UseDeviceLink {
        device_link: DeviceLinkDescription {
            id: id.to_owned(),
            source: ColorSpace::DeviceRgb,
            destination: ColorSpace::DeviceCmyk,
        },
    }
}

fn profile(id: &str) -> ProfileReference {
    ProfileReference { id: id.to_owned() }
}

fn keyed(decision: TransformCacheKeyDecision) -> TransformCacheKey {
    match decision {
        TransformCacheKeyDecision::Keyed { key } => Some(key),
        TransformCacheKeyDecision::NoKey { .. } => None,
    }
    .expect("expected a keyed decision")
}

// --- DeviceLink path ---------------------------------------------------------

#[test]
fn device_link_decision_keys_by_link_id_and_color_spaces() {
    assert_eq!(
        derive_transform_cache_key(&use_link("rgb-to-cmyk"), &rgb_to_cmyk_request(), None, None),
        TransformCacheKeyDecision::Keyed {
            key: TransformCacheKey::DeviceLink {
                device_link_id: "rgb-to-cmyk".to_owned(),
                source: ColorSpace::DeviceRgb,
                destination: ColorSpace::DeviceCmyk,
            },
        },
    );
}

#[test]
fn device_link_path_ignores_supplied_profile_references() {
    // The PCS profile references are consulted only on the PCS path; supplying
    // them on the DeviceLink path must not change the key.
    assert_eq!(
        derive_transform_cache_key(
            &use_link("rgb-to-cmyk"),
            &rgb_to_cmyk_request(),
            Some(&profile("ignored-src")),
            Some(&profile("ignored-dst")),
        ),
        derive_transform_cache_key(&use_link("rgb-to-cmyk"), &rgb_to_cmyk_request(), None, None),
    );
}

// --- PCS path ----------------------------------------------------------------

#[test]
fn profile_connection_space_decision_keys_by_profile_references_and_color_spaces() {
    assert_eq!(
        derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            Some(&profile("src-rgb")),
            Some(&profile("dst-cmyk")),
        ),
        TransformCacheKeyDecision::Keyed {
            key: TransformCacheKey::ProfileConnectionSpace {
                source_profile: profile("src-rgb"),
                destination_profile: profile("dst-cmyk"),
                source: ColorSpace::DeviceRgb,
                destination: ColorSpace::DeviceCmyk,
            },
        },
    );
}

// --- No-key reasons ----------------------------------------------------------

#[test]
fn rejected_device_link_decision_has_no_key() {
    assert_eq!(
        derive_transform_cache_key(
            &DeviceLinkDecision::Rejected {
                rejection: DeviceLinkRejection::NoMatchingDeviceLink,
            },
            &rgb_to_cmyk_request(),
            Some(&profile("src")),
            Some(&profile("dst")),
        ),
        TransformCacheKeyDecision::NoKey {
            reason: TransformCacheKeyRejection::RejectedDeviceLink,
        },
    );
}

#[test]
fn profile_connection_space_without_source_profile_has_no_key() {
    assert_eq!(
        derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            None,
            Some(&profile("dst-cmyk")),
        ),
        TransformCacheKeyDecision::NoKey {
            reason: TransformCacheKeyRejection::MissingSourceProfileId,
        },
    );
}

#[test]
fn profile_connection_space_without_destination_profile_has_no_key() {
    assert_eq!(
        derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            Some(&profile("src-rgb")),
            None,
        ),
        TransformCacheKeyDecision::NoKey {
            reason: TransformCacheKeyRejection::MissingDestinationProfileId,
        },
    );
}

#[test]
fn missing_source_profile_is_reported_before_missing_destination() {
    // With neither reference supplied, the source reason is reported first.
    assert_eq!(
        derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            None,
            None,
        ),
        TransformCacheKeyDecision::NoKey {
            reason: TransformCacheKeyRejection::MissingSourceProfileId,
        },
    );
}

// --- Key equality and inequality ---------------------------------------------

#[test]
fn equal_device_link_inputs_yield_equal_keys() {
    let first = keyed(derive_transform_cache_key(
        &use_link("rgb-to-cmyk"),
        &rgb_to_cmyk_request(),
        None,
        None,
    ));
    let second = keyed(derive_transform_cache_key(
        &use_link("rgb-to-cmyk"),
        &rgb_to_cmyk_request(),
        None,
        None,
    ));
    assert_eq!(first, second);
}

#[test]
fn device_link_keys_differ_on_link_id() {
    assert_ne!(
        keyed(derive_transform_cache_key(
            &use_link("first"),
            &rgb_to_cmyk_request(),
            None,
            None
        )),
        keyed(derive_transform_cache_key(
            &use_link("second"),
            &rgb_to_cmyk_request(),
            None,
            None
        )),
    );
}

#[test]
fn device_link_keys_differ_on_source_color_space() {
    assert_ne!(
        keyed(derive_transform_cache_key(
            &use_link("link"),
            &request(ColorSpace::DeviceRgb, ColorSpace::DeviceCmyk),
            None,
            None,
        )),
        keyed(derive_transform_cache_key(
            &use_link("link"),
            &request(ColorSpace::DeviceGray, ColorSpace::DeviceCmyk),
            None,
            None,
        )),
    );
}

#[test]
fn device_link_keys_differ_on_destination_color_space() {
    assert_ne!(
        keyed(derive_transform_cache_key(
            &use_link("link"),
            &request(ColorSpace::DeviceRgb, ColorSpace::DeviceCmyk),
            None,
            None,
        )),
        keyed(derive_transform_cache_key(
            &use_link("link"),
            &request(ColorSpace::DeviceRgb, ColorSpace::DeviceGray),
            None,
            None,
        )),
    );
}

#[test]
fn pcs_keys_differ_on_source_profile_reference() {
    assert_ne!(
        keyed(derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            Some(&profile("src-a")),
            Some(&profile("dst")),
        )),
        keyed(derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            Some(&profile("src-b")),
            Some(&profile("dst")),
        )),
    );
}

#[test]
fn pcs_keys_differ_on_destination_profile_reference() {
    assert_ne!(
        keyed(derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            Some(&profile("src")),
            Some(&profile("dst-a")),
        )),
        keyed(derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            Some(&profile("src")),
            Some(&profile("dst-b")),
        )),
    );
}

#[test]
fn device_link_and_pcs_keys_with_same_color_spaces_differ() {
    // A DeviceLink-keyed transform and a PCS-keyed transform over the same
    // abstract color spaces are distinct cache entries.
    assert_ne!(
        keyed(derive_transform_cache_key(
            &use_link("rgb-to-cmyk"),
            &rgb_to_cmyk_request(),
            None,
            None
        )),
        keyed(derive_transform_cache_key(
            &DeviceLinkDecision::UseProfileConnectionSpace,
            &rgb_to_cmyk_request(),
            Some(&profile("src")),
            Some(&profile("dst")),
        )),
    );
}

#[test]
fn equal_keys_hash_equal_in_a_set() {
    // The key derives `Hash`, so a future bounded cache can use it as a map key.
    let mut set = HashSet::new();
    set.insert(keyed(derive_transform_cache_key(
        &use_link("rgb-to-cmyk"),
        &rgb_to_cmyk_request(),
        None,
        None,
    )));
    assert!(set.contains(&keyed(derive_transform_cache_key(
        &use_link("rgb-to-cmyk"),
        &rgb_to_cmyk_request(),
        None,
        None,
    ))));
    // A differing link id is a distinct entry.
    set.insert(keyed(derive_transform_cache_key(
        &use_link("other"),
        &rgb_to_cmyk_request(),
        None,
        None,
    )));
    assert_eq!(set.len(), 2);
}

#[test]
fn key_includes_named_resource_color_spaces() {
    let resource = ColorSpace::Resource(PdfName(b"SourceCS".to_vec()));
    assert_eq!(
        derive_transform_cache_key(
            &use_link("resource-to-cmyk"),
            &request(resource.clone(), ColorSpace::DeviceCmyk),
            None,
            None,
        ),
        TransformCacheKeyDecision::Keyed {
            key: TransformCacheKey::DeviceLink {
                device_link_id: "resource-to-cmyk".to_owned(),
                source: resource,
                destination: ColorSpace::DeviceCmyk,
            },
        },
    );
}

// --- Serde shape tests -------------------------------------------------------
//
// These lock the public JSON encoding of the cache-key contract exactly as the
// current `#[serde(...)]` attributes emit it.

#[test]
fn profile_reference_has_stable_json_shape() {
    assert_json_round_trip(
        &profile("profile:pso-coated-v3"),
        Json::object([("id", Json::string("profile:pso-coated-v3"))]),
    );
}

#[test]
fn device_link_cache_key_has_stable_json_shape() {
    assert_json_round_trip(
        &TransformCacheKey::DeviceLink {
            device_link_id: "rgb-to-cmyk".to_owned(),
            source: ColorSpace::DeviceRgb,
            destination: ColorSpace::DeviceCmyk,
        },
        Json::object([
            ("path", Json::string("device_link")),
            ("device_link_id", Json::string("rgb-to-cmyk")),
            ("source", Json::string("device_rgb")),
            ("destination", Json::string("device_cmyk")),
        ]),
    );
}

#[test]
fn profile_connection_space_cache_key_has_stable_json_shape() {
    assert_json_round_trip(
        &TransformCacheKey::ProfileConnectionSpace {
            source_profile: profile("src-rgb"),
            destination_profile: profile("dst-cmyk"),
            source: ColorSpace::DeviceRgb,
            destination: ColorSpace::DeviceCmyk,
        },
        Json::object([
            ("path", Json::string("profile_connection_space")),
            (
                "source_profile",
                Json::object([("id", Json::string("src-rgb"))]),
            ),
            (
                "destination_profile",
                Json::object([("id", Json::string("dst-cmyk"))]),
            ),
            ("source", Json::string("device_rgb")),
            ("destination", Json::string("device_cmyk")),
        ]),
    );
}

#[test]
fn cache_key_rejection_variants_have_stable_json_shape() {
    assert_json_round_trip(
        &TransformCacheKeyRejection::RejectedDeviceLink,
        Json::object([("reason", Json::string("rejected_device_link"))]),
    );
    assert_json_round_trip(
        &TransformCacheKeyRejection::MissingSourceProfileId,
        Json::object([("reason", Json::string("missing_source_profile_id"))]),
    );
    assert_json_round_trip(
        &TransformCacheKeyRejection::MissingDestinationProfileId,
        Json::object([("reason", Json::string("missing_destination_profile_id"))]),
    );
}

#[test]
fn cache_key_decision_keyed_has_stable_json_shape() {
    assert_json_round_trip(
        &TransformCacheKeyDecision::Keyed {
            key: TransformCacheKey::DeviceLink {
                device_link_id: "rgb-to-cmyk".to_owned(),
                source: ColorSpace::DeviceRgb,
                destination: ColorSpace::DeviceCmyk,
            },
        },
        Json::object([
            ("decision", Json::string("keyed")),
            (
                "key",
                Json::object([
                    ("path", Json::string("device_link")),
                    ("device_link_id", Json::string("rgb-to-cmyk")),
                    ("source", Json::string("device_rgb")),
                    ("destination", Json::string("device_cmyk")),
                ]),
            ),
        ]),
    );
}

#[test]
fn cache_key_decision_no_key_has_stable_json_shape() {
    assert_json_round_trip(
        &TransformCacheKeyDecision::NoKey {
            reason: TransformCacheKeyRejection::RejectedDeviceLink,
        },
        Json::object([
            ("decision", Json::string("no_key")),
            (
                "reason",
                Json::object([("reason", Json::string("rejected_device_link"))]),
            ),
        ]),
    );
}
