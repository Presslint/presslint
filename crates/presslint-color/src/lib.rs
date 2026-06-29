//! Color-management policy interfaces.

#![forbid(unsafe_code)]

mod color_policy;
mod devicelink;
mod output_intent;
mod overprint;
mod policy;
mod spot;
mod transform_cache;
mod transform_plan;

#[cfg(test)]
mod tests;

pub use color_policy::{ColorPolicyDecision, resolve_color_policy};
pub use devicelink::{
    DeviceLinkDecision, DeviceLinkDescription, DeviceLinkRejection, resolve_device_link_policy,
};
pub use output_intent::{
    ObservedOutputIntent, OutputIntentDecision, OutputIntentRejection, resolve_output_intent_policy,
};
pub use overprint::{
    ObservedOverprintInteraction, OverprintDecision, OverprintMitigation,
    OverprintMitigationAction, OverprintRejection, OverprintSensitivity, OverprintSkipReason,
    SkippedOverprintMitigation, resolve_overprint_policy,
};
pub use policy::{
    ColorPolicy, DeviceLinkPolicy, NamedOutputCondition, OutputIntentPolicy, OutputIntentSubtype,
    OutputIntentTarget, OutputProfileSource, OverprintPolicy, ProfileBackedOutputIntent,
    SpotPolicy, TransformPlanRequest, TransformRequest,
};
pub use spot::{
    ObservedSpotColor, SkippedSpotConversion, SpotDecision, SpotRejection, SpotSkipReason,
    resolve_spot_policy,
};
pub use transform_cache::{
    ProfileReference, TransformCacheKey, TransformCacheKeyDecision, TransformCacheKeyRejection,
    derive_transform_cache_key,
};
pub use transform_plan::{TransformPlanDecision, resolve_transform_plan};
