use presslint_core::{ColorSpace, PdfName};

use super::assert_json_round_trip;
use super::json::Json;
use crate::{
    ObservedSpotColor, SkippedSpotConversion, SpotDecision, SpotPolicy, SpotRejection,
    SpotSkipReason, resolve_spot_policy,
};

fn spot(name: &str, alternate: ColorSpace) -> ObservedSpotColor {
    ObservedSpotColor {
        name: name.to_owned(),
        alternate,
    }
}

fn skipped(name: &str, alternate: ColorSpace) -> SkippedSpotConversion {
    SkippedSpotConversion {
        spot: spot(name, alternate),
        reason: SpotSkipReason::UnsupportedAlternate,
    }
}

// --- Spot resolution tests ---------------------------------------------------
//
// These cover every policy/observed-state combination of `resolve_spot_policy`:
// `Preserve` regardless of observed state, `Reject` with and without observed
// spots, and `ConvertAlternate` with all-eligible, all-skipped, and mixed input
// where the eligible and skipped lists preserve caller iteration order.

#[test]
fn preserve_leaves_as_is_regardless_of_observed_state() {
    assert_eq!(
        resolve_spot_policy(SpotPolicy::Preserve, []),
        SpotDecision::Preserve,
    );
    assert_eq!(
        resolve_spot_policy(
            SpotPolicy::Preserve,
            [spot("Pantone 185 C", ColorSpace::DeviceCmyk)],
        ),
        SpotDecision::Preserve,
    );
}

#[test]
fn reject_is_satisfied_when_no_spot_is_observed() {
    assert_eq!(
        resolve_spot_policy(SpotPolicy::Reject, []),
        SpotDecision::NoSpotColors,
    );
}

#[test]
fn reject_rejects_when_any_spot_is_observed() {
    assert_eq!(
        resolve_spot_policy(
            SpotPolicy::Reject,
            [spot("Pantone 185 C", ColorSpace::DeviceCmyk)],
        ),
        SpotDecision::Rejected {
            rejection: SpotRejection::SpotConversionRequired,
        },
    );
}

#[test]
fn convert_alternate_marks_all_process_device_alternates_eligible() {
    assert_eq!(
        resolve_spot_policy(
            SpotPolicy::ConvertAlternate,
            [
                spot("Gray Spot", ColorSpace::DeviceGray),
                spot("Rgb Spot", ColorSpace::DeviceRgb),
                spot("Cmyk Spot", ColorSpace::DeviceCmyk),
            ],
        ),
        SpotDecision::ConvertAlternate {
            eligible: vec![
                spot("Gray Spot", ColorSpace::DeviceGray),
                spot("Rgb Spot", ColorSpace::DeviceRgb),
                spot("Cmyk Spot", ColorSpace::DeviceCmyk),
            ],
            skipped: vec![],
        },
    );
}

#[test]
fn convert_alternate_skips_every_non_process_device_alternate() {
    // Every non-process `ColorSpace` variant is skipped with the single
    // `UnsupportedAlternate` reason. This covers each remaining variant of the
    // enum so the eligible set stays exactly `DeviceGray`/`DeviceRgb`/`DeviceCmyk`.
    assert_eq!(
        resolve_spot_policy(
            SpotPolicy::ConvertAlternate,
            [
                spot("Icc Spot", ColorSpace::IccBased),
                spot("Lab Spot", ColorSpace::Lab),
                spot("CalGray Spot", ColorSpace::CalGray),
                spot("CalRgb Spot", ColorSpace::CalRgb),
                spot("Sep Spot", ColorSpace::Separation),
                spot("DeviceN Spot", ColorSpace::DeviceN),
                spot("Indexed Spot", ColorSpace::Indexed),
                spot("Pattern Spot", ColorSpace::Pattern),
                spot(
                    "Resource Spot",
                    ColorSpace::Resource(PdfName(b"AltCS".to_vec())),
                ),
                spot("Unknown Spot", ColorSpace::Unknown),
            ],
        ),
        SpotDecision::ConvertAlternate {
            eligible: vec![],
            skipped: vec![
                skipped("Icc Spot", ColorSpace::IccBased),
                skipped("Lab Spot", ColorSpace::Lab),
                skipped("CalGray Spot", ColorSpace::CalGray),
                skipped("CalRgb Spot", ColorSpace::CalRgb),
                skipped("Sep Spot", ColorSpace::Separation),
                skipped("DeviceN Spot", ColorSpace::DeviceN),
                skipped("Indexed Spot", ColorSpace::Indexed),
                skipped("Pattern Spot", ColorSpace::Pattern),
                skipped(
                    "Resource Spot",
                    ColorSpace::Resource(PdfName(b"AltCS".to_vec())),
                ),
                skipped("Unknown Spot", ColorSpace::Unknown),
            ],
        },
    );
}

#[test]
fn convert_alternate_preserves_caller_order_in_both_partitions() {
    // Eligible and skipped spots are interleaved in the input; each list keeps
    // its caller iteration order independently.
    assert_eq!(
        resolve_spot_policy(
            SpotPolicy::ConvertAlternate,
            [
                spot("First Skip", ColorSpace::Separation),
                spot("First Keep", ColorSpace::DeviceCmyk),
                spot("Second Skip", ColorSpace::Lab),
                spot("Second Keep", ColorSpace::DeviceGray),
                spot("Third Keep", ColorSpace::DeviceRgb),
                spot("Third Skip", ColorSpace::IccBased),
            ],
        ),
        SpotDecision::ConvertAlternate {
            eligible: vec![
                spot("First Keep", ColorSpace::DeviceCmyk),
                spot("Second Keep", ColorSpace::DeviceGray),
                spot("Third Keep", ColorSpace::DeviceRgb),
            ],
            skipped: vec![
                skipped("First Skip", ColorSpace::Separation),
                skipped("Second Skip", ColorSpace::Lab),
                skipped("Third Skip", ColorSpace::IccBased),
            ],
        },
    );
}

// --- Spot shape tests --------------------------------------------------------
//
// These lock the public JSON encoding of every new spot type. Each fixture
// asserts a full round-trip exactly as the current `#[serde(...)]` attributes
// emit it.

#[test]
fn observed_spot_color_has_stable_json_shape() {
    assert_json_round_trip(
        &spot("Pantone 185 C", ColorSpace::DeviceCmyk),
        Json::object([
            ("name", Json::string("Pantone 185 C")),
            ("alternate", Json::string("device_cmyk")),
        ]),
    );
}

#[test]
fn spot_rejection_has_stable_json_shape() {
    assert_json_round_trip(
        &SpotRejection::SpotConversionRequired,
        Json::object([("reason", Json::string("spot_conversion_required"))]),
    );
}

#[test]
fn spot_skip_reason_has_stable_json_shape() {
    assert_json_round_trip(
        &SpotSkipReason::UnsupportedAlternate,
        Json::object([("reason", Json::string("unsupported_alternate"))]),
    );
}

#[test]
fn skipped_spot_conversion_has_stable_json_shape() {
    assert_json_round_trip(
        &skipped("Sep Spot", ColorSpace::Separation),
        Json::object([
            (
                "spot",
                Json::object([
                    ("name", Json::string("Sep Spot")),
                    ("alternate", Json::string("separation")),
                ]),
            ),
            (
                "reason",
                Json::object([("reason", Json::string("unsupported_alternate"))]),
            ),
        ]),
    );
}

#[test]
fn spot_decision_unit_variants_have_stable_json_shape() {
    assert_json_round_trip(
        &SpotDecision::Preserve,
        Json::object([("decision", Json::string("preserve"))]),
    );
    assert_json_round_trip(
        &SpotDecision::NoSpotColors,
        Json::object([("decision", Json::string("no_spot_colors"))]),
    );
}

#[test]
fn spot_decision_rejected_has_stable_json_shape() {
    assert_json_round_trip(
        &SpotDecision::Rejected {
            rejection: SpotRejection::SpotConversionRequired,
        },
        Json::object([
            ("decision", Json::string("rejected")),
            (
                "rejection",
                Json::object([("reason", Json::string("spot_conversion_required"))]),
            ),
        ]),
    );
}

#[test]
fn spot_decision_convert_alternate_has_stable_json_shape() {
    assert_json_round_trip(
        &SpotDecision::ConvertAlternate {
            eligible: vec![spot("Cmyk Spot", ColorSpace::DeviceCmyk)],
            skipped: vec![skipped("Lab Spot", ColorSpace::Lab)],
        },
        Json::object([
            ("decision", Json::string("convert_alternate")),
            (
                "eligible",
                Json::array([Json::object([
                    ("name", Json::string("Cmyk Spot")),
                    ("alternate", Json::string("device_cmyk")),
                ])]),
            ),
            (
                "skipped",
                Json::array([Json::object([
                    (
                        "spot",
                        Json::object([
                            ("name", Json::string("Lab Spot")),
                            ("alternate", Json::string("lab")),
                        ]),
                    ),
                    (
                        "reason",
                        Json::object([("reason", Json::string("unsupported_alternate"))]),
                    ),
                ])]),
            ),
        ]),
    );
}
