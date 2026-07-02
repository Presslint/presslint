//! Exact neutral-black preservation policy for `DeviceLink` content conversion.

use serde::{Deserialize, Serialize};

use crate::{
    content_color_convert::DeviceColorSpace, pdf_number_serialize::serialize_color_component,
};

/// Optional black-preservation overlay for `DeviceLink` content conversion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlackPreservationPolicy {
    /// Apply every matching source-space operator through the `DeviceLink`.
    #[default]
    None,
    /// Preserve exact neutral black as destination CMYK K-only black.
    NeutralBlackToK,
}

/// Return canonical K-only replacement bytes when the policy preserves black.
pub fn black_preservation_replacement(
    operands: &[f64],
    source: DeviceColorSpace,
    destination: DeviceColorSpace,
    stroking: bool,
    policy: BlackPreservationPolicy,
) -> Option<Vec<u8>> {
    if policy != BlackPreservationPolicy::NeutralBlackToK || destination != DeviceColorSpace::Cmyk {
        return None;
    }
    if !is_neutral_black(operands, source) {
        return None;
    }
    Some(cmyk_black_bytes(stroking))
}

fn is_neutral_black(operands: &[f64], source: DeviceColorSpace) -> bool {
    match source {
        DeviceColorSpace::Gray => operands == [0.0],
        DeviceColorSpace::Rgb => operands == [0.0, 0.0, 0.0],
        DeviceColorSpace::Cmyk => operands == [0.0, 0.0, 0.0, 1.0],
    }
}

fn cmyk_black_bytes(stroking: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    for component in [0.0, 0.0, 0.0, 1.0] {
        bytes.extend_from_slice(serialize_color_component(component).as_bytes());
        bytes.push(b' ');
    }
    bytes.extend_from_slice(DeviceColorSpace::Cmyk.operator(stroking));
    bytes
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::{BlackPreservationPolicy, black_preservation_replacement};
    use crate::content_color_convert::DeviceColorSpace;

    #[test]
    fn exact_neutral_black_sources_map_to_k_only_for_cmyk_destination() {
        assert_eq!(
            black_preservation_replacement(
                &[0.0, 0.0, 0.0],
                DeviceColorSpace::Rgb,
                DeviceColorSpace::Cmyk,
                false,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            Some(b"0 0 0 1 k".to_vec())
        );
        assert_eq!(
            black_preservation_replacement(
                &[0.0],
                DeviceColorSpace::Gray,
                DeviceColorSpace::Cmyk,
                true,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            Some(b"0 0 0 1 K".to_vec())
        );
        assert_eq!(
            black_preservation_replacement(
                &[0.0, 0.0, 0.0, 1.0],
                DeviceColorSpace::Cmyk,
                DeviceColorSpace::Cmyk,
                false,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            Some(b"0 0 0 1 k".to_vec())
        );
    }

    #[test]
    fn nonblack_policy_none_and_non_cmyk_destination_fall_through() {
        assert_eq!(
            black_preservation_replacement(
                &[0.0, 0.0, 0.1],
                DeviceColorSpace::Rgb,
                DeviceColorSpace::Cmyk,
                false,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            None
        );
        assert_eq!(
            black_preservation_replacement(
                &[0.0, 0.0, 0.0],
                DeviceColorSpace::Rgb,
                DeviceColorSpace::Cmyk,
                false,
                BlackPreservationPolicy::None,
            ),
            None
        );
        assert_eq!(
            black_preservation_replacement(
                &[0.0],
                DeviceColorSpace::Gray,
                DeviceColorSpace::Gray,
                false,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            None
        );
    }
}
