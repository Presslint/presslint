//! Deterministic PDF colour-component number serialization.
//!
//! This is the quantisation policy F4-2 owns for converted colour operands.
//! `presslint-color-lcms` (F4-1) returns raw `f64` components; this helper turns
//! one such component into the minimal PDF real literal spliced into a content
//! stream. It is deliberately a separate helper from
//! `page_box_serialize::format_number`: page-box coordinates are arbitrary
//! finite reals serialized by shortest round-trip, whereas a colour component is
//! clamped to the ISO 32000 §8.6 `[0.0, 1.0]` operand domain and emitted at a
//! fixed precision, so the two policies do not share an implementation.

/// Serialize one colour component as a minimal PDF real literal.
///
/// The value is clamped to `[0.0, 1.0]`; a negative-zero or negative value maps
/// to `0`; a non-finite value (which a valid `DeviceLink` transform never
/// produces, but which is handled defensively) maps to `0`. Otherwise it is
/// formatted with fixed 8-decimal precision — never an exponent — then trailing
/// zeros and a trailing `.` are trimmed. The result is deterministic: the same
/// `f64` always serializes to the same bytes.
pub fn serialize_color_component(value: f64) -> String {
    if !value.is_finite() {
        return "0".to_owned();
    }
    let clamped = value.clamp(0.0, 1.0);
    if clamped == 0.0 {
        // Covers both `0.0` and `-0.0`.
        return "0".to_owned();
    }
    let formatted = format!("{clamped:.8}");
    formatted
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::serialize_color_component;

    #[test]
    fn zero_and_negative_map_to_bare_zero() {
        assert_eq!(serialize_color_component(0.0), "0");
        assert_eq!(serialize_color_component(-0.0), "0");
        assert_eq!(serialize_color_component(-0.5), "0");
    }

    #[test]
    fn one_and_above_clamp_to_one() {
        assert_eq!(serialize_color_component(1.0), "1");
        assert_eq!(serialize_color_component(1.5), "1");
        assert_eq!(serialize_color_component(2_000.0), "1");
    }

    #[test]
    fn fractional_values_trim_trailing_zeros() {
        assert_eq!(serialize_color_component(0.5), "0.5");
        assert_eq!(serialize_color_component(0.25), "0.25");
        assert_eq!(serialize_color_component(0.1), "0.1");
    }

    #[test]
    fn fixed_precision_rounds_to_eight_decimals() {
        assert_eq!(serialize_color_component(0.108_339_055_44), "0.10833906");
        assert_eq!(serialize_color_component(0.123_456_789), "0.12345679");
        assert_eq!(serialize_color_component(0.910_963_583_7), "0.91096358");
    }

    #[test]
    fn non_finite_maps_to_zero() {
        assert_eq!(serialize_color_component(f64::NAN), "0");
        assert_eq!(serialize_color_component(f64::INFINITY), "0");
        assert_eq!(serialize_color_component(f64::NEG_INFINITY), "0");
    }

    #[test]
    fn never_emits_an_exponent() {
        assert!(!serialize_color_component(0.000_000_01).contains('e'));
        assert_eq!(serialize_color_component(0.000_000_01), "0.00000001");
    }
}
