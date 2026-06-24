//! Color-management policy interfaces.

#![forbid(unsafe_code)]

use presslint_core::ColorSpace;
use serde::{Deserialize, Serialize};

/// Color-conversion policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorPolicy {
    /// Spot handling.
    pub spot: SpotPolicy,
    /// Overprint handling.
    pub overprint: OverprintPolicy,
}

/// Spot-color handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpotPolicy {
    /// Preserve spot colors.
    Preserve,
    /// Reject jobs that would require spot conversion.
    Reject,
    /// Convert spot alternate colors when supported.
    ConvertAlternate,
}

/// Overprint handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverprintPolicy {
    /// Preserve and report.
    Preserve,
    /// Reject unsafe overprint-sensitive conversions.
    RejectUnsafe,
    /// Apply supported mitigation rules.
    Mitigate,
}

/// Abstract color transform request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransformRequest {
    /// Source color space.
    pub source: ColorSpace,
    /// Destination color space.
    pub destination: ColorSpace,
    /// Policy for ambiguous prepress semantics.
    pub policy: ColorPolicy,
}
