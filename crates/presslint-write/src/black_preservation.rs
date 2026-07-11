//! Exact neutral-black preservation policy for `DeviceLink` content conversion.

use serde::{Deserialize, Serialize};

use crate::content_color_convert::DeviceColorSpace;

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

/// Return exact destination K-only components when the policy preserves black.
pub fn black_preservation_components(
    operands: &[f64],
    source: DeviceColorSpace,
    destination: DeviceColorSpace,
    policy: BlackPreservationPolicy,
) -> Option<Vec<f64>> {
    if policy != BlackPreservationPolicy::NeutralBlackToK || destination != DeviceColorSpace::Cmyk {
        return None;
    }
    if !is_neutral_black(operands, source) {
        return None;
    }
    Some(vec![0.0, 0.0, 0.0, 1.0])
}

fn is_neutral_black(operands: &[f64], source: DeviceColorSpace) -> bool {
    match source {
        DeviceColorSpace::Gray => operands == [0.0],
        DeviceColorSpace::Rgb => operands == [0.0, 0.0, 0.0],
        DeviceColorSpace::Cmyk => operands == [0.0, 0.0, 0.0, 1.0],
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::float_cmp)]

    use super::{BlackPreservationPolicy, black_preservation_components};
    use crate::content_color_convert::DeviceColorSpace;

    #[test]
    fn exact_neutral_black_sources_map_to_k_only_for_cmyk_destination() {
        assert_eq!(
            black_preservation_components(
                &[0.0, 0.0, 0.0],
                DeviceColorSpace::Rgb,
                DeviceColorSpace::Cmyk,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            Some(vec![0.0, 0.0, 0.0, 1.0])
        );
        assert_eq!(
            black_preservation_components(
                &[0.0],
                DeviceColorSpace::Gray,
                DeviceColorSpace::Cmyk,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            Some(vec![0.0, 0.0, 0.0, 1.0])
        );
        assert_eq!(
            black_preservation_components(
                &[0.0, 0.0, 0.0, 1.0],
                DeviceColorSpace::Cmyk,
                DeviceColorSpace::Cmyk,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            Some(vec![0.0, 0.0, 0.0, 1.0])
        );
    }

    #[test]
    fn nonblack_policy_none_and_non_cmyk_destination_fall_through() {
        assert_eq!(
            black_preservation_components(
                &[0.0, 0.0, 0.1],
                DeviceColorSpace::Rgb,
                DeviceColorSpace::Cmyk,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            None
        );
        assert_eq!(
            black_preservation_components(
                &[0.0, 0.0, 0.0],
                DeviceColorSpace::Rgb,
                DeviceColorSpace::Cmyk,
                BlackPreservationPolicy::None,
            ),
            None
        );
        assert_eq!(
            black_preservation_components(
                &[0.0],
                DeviceColorSpace::Gray,
                DeviceColorSpace::Gray,
                BlackPreservationPolicy::NeutralBlackToK,
            ),
            None
        );
    }
}
