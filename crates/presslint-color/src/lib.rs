//! Color-management policy interfaces.

#![forbid(unsafe_code)]

#[cfg(test)]
mod tests;

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

/// `DeviceLink` usage policy for a transform request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceLinkPolicy {
    /// Require a matching `DeviceLink` and reject the plan when none is supplied.
    Require,
    /// Use a matching `DeviceLink` when present, otherwise plan via PCS.
    Prefer,
    /// Never use `DeviceLink` profiles for this request; always plan via PCS.
    Forbid,
}

/// Caller-supplied, ICC-free description of an available `DeviceLink` profile.
///
/// This is a planning input only. It carries no ICC data and no profile bytes:
/// callers describe the link by a stable identifier plus the abstract source
/// and destination color spaces it connects. `presslint-color` never derives
/// these values from ICC bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLinkDescription {
    /// Stable caller-defined `DeviceLink` identifier.
    pub id: String,
    /// Source color space accepted by the `DeviceLink`.
    pub source: ColorSpace,
    /// Destination color space produced by the `DeviceLink`.
    pub destination: ColorSpace,
}

/// Reason a `DeviceLink` policy could not be satisfied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum DeviceLinkRejection {
    /// `Require` was requested but no supplied `DeviceLink` matched exactly.
    NoMatchingDeviceLink,
}

/// Pure resolution of a [`DeviceLinkPolicy`] against a transform request.
///
/// This decision is report-only. Producing it parses no ICC bytes, executes no
/// color transform, and mutates no PDF bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum DeviceLinkDecision {
    /// Plan conversion with the first matching caller-supplied `DeviceLink`.
    UseDeviceLink {
        /// Matching `DeviceLink` selected in caller order.
        device_link: DeviceLinkDescription,
    },
    /// Plan ordinary source-profile to destination-profile conversion via PCS.
    UseProfileConnectionSpace,
    /// A required `DeviceLink` could not be selected.
    Rejected {
        /// Structured rejection reason.
        rejection: DeviceLinkRejection,
    },
}

/// Document-level output-intent policy for future color planning.
///
/// This contract is a planning input only. It does not inspect existing PDF
/// catalog entries, parse ICC profile contents, embed streams, or mutate PDF
/// bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum OutputIntentPolicy {
    /// Leave any existing output intents untouched and do not require one.
    Preserve,
    /// Require a suitable output intent to already be present before writing.
    RequireExisting,
    /// Ask a later PDF writer to ensure that the requested target exists.
    EnsureTarget {
        /// Requested output intent target.
        target: OutputIntentTarget,
    },
}

/// Target output condition requested by an output-intent policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OutputIntentTarget {
    /// A named production condition, typically resolved by a registry.
    NamedCondition {
        /// Named output condition to reference.
        condition: NamedOutputCondition,
    },
    /// A target backed by an explicit profile source supplied to a future writer.
    ProfileBacked {
        /// Profile-backed output intent request.
        intent: ProfileBackedOutputIntent,
    },
}

/// Output intent subtype for the `S` entry of a future `OutputIntent` dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputIntentSubtype {
    /// PDF/X output intent subtype.
    GtsPdfx,
    /// PDF/A-1 output intent subtype.
    GtsPdfa1,
    /// PDF/E-1 output intent subtype.
    IsoPdfe1,
}

/// Named output condition reference.
///
/// This describes a registry-backed output condition by name. It intentionally
/// carries no ICC data; resolution of the named condition belongs to later
/// planning or writing layers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedOutputCondition {
    /// Output intent subtype to request.
    pub subtype: OutputIntentSubtype,
    /// Registry identifier for the intended output condition.
    pub output_condition_identifier: String,
    /// Registry URI or stable registry name that defines the condition.
    pub registry_name: String,
}

/// Profile-backed output intent request.
///
/// The profile source is opaque to `presslint-color`: these contracts do not
/// validate ICC bytes, derive profile metadata, or decide how a PDF catalog is
/// updated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileBackedOutputIntent {
    /// Output intent subtype to request.
    pub subtype: OutputIntentSubtype,
    /// Identifier for the intended output condition.
    pub output_condition_identifier: String,
    /// Human-readable output condition label.
    pub output_condition: String,
    /// Additional human-readable target condition information.
    pub info: String,
    /// Opaque profile source for a later writer.
    pub profile: OutputProfileSource,
}

/// Opaque profile source for a future output-intent writer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum OutputProfileSource {
    /// Stable profile handle supplied by a caller or higher-level planner.
    OpaqueId {
        /// Opaque profile identifier.
        id: String,
    },
    /// ICC profile bytes supplied by a caller and left unparsed by this crate.
    EmbeddedBytes {
        /// Raw profile bytes.
        bytes: Vec<u8>,
    },
}

/// Caller-supplied, ICC-free description of one output intent already observed
/// in a document.
///
/// This is a planning input only. It carries no ICC data and no profile bytes:
/// an observed intent is described abstractly by its [`OutputIntentSubtype`]
/// and its output-condition identifier string. `presslint-color` never derives
/// these values from PDF bytes; a caller supplies them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedOutputIntent {
    /// Observed output intent subtype (the `S` entry of a real dictionary).
    pub subtype: OutputIntentSubtype,
    /// Observed output-condition identifier string.
    pub output_condition_identifier: String,
}

/// Reason an output-intent policy could not be satisfied by the observed state.
///
/// This is a report-only planning result; it triggers no PDF mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum OutputIntentRejection {
    /// `RequireExisting` was requested but no output intent was observed.
    NoExistingIntent,
}

/// Pure resolution of an [`OutputIntentPolicy`] against the observed state.
///
/// This decision is a planning input for a later PDF writer only. Producing it
/// inspects no PDF catalog, parses no ICC profile, and mutates no PDF bytes; it
/// reports what a writer should do, not what it has done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum OutputIntentDecision {
    /// `Preserve`: leave any existing intents untouched; nothing to plan.
    Preserve,
    /// `RequireExisting` satisfied: at least one output intent is present.
    SatisfiedByExisting,
    /// A policy could not be satisfied by the observed state.
    Rejected {
        /// Structured rejection reason.
        rejection: OutputIntentRejection,
    },
    /// `EnsureTarget` already satisfied: an observed intent matches the
    /// requested target identity (same subtype and output-condition identifier).
    AlreadySatisfied {
        /// Requested target that the observed state already satisfies.
        target: OutputIntentTarget,
    },
    /// `EnsureTarget` conflict: an observed intent shares the requested subtype
    /// but carries a different output-condition identifier.
    ConflictsWithExisting {
        /// Requested target a later writer was asked to ensure.
        requested: OutputIntentTarget,
        /// First observed intent that conflicts with the requested target.
        existing: ObservedOutputIntent,
    },
    /// `EnsureTarget` otherwise: a later writer must ensure the requested target.
    RequiresEnsureTarget {
        /// Requested target a later writer must ensure.
        target: OutputIntentTarget,
    },
}

/// Extract the comparable identity (`subtype`, output-condition identifier) of a
/// requested target.
///
/// Target identity is compared only by [`OutputIntentSubtype`] and the
/// output-condition identifier string. This deliberately ignores
/// `registry_name`, `info`, and any profile bytes; both target variants expose
/// the same two comparable fields.
const fn target_identity(target: &OutputIntentTarget) -> (OutputIntentSubtype, &str) {
    match target {
        OutputIntentTarget::NamedCondition { condition } => (
            condition.subtype,
            condition.output_condition_identifier.as_str(),
        ),
        OutputIntentTarget::ProfileBacked { intent } => {
            (intent.subtype, intent.output_condition_identifier.as_str())
        }
    }
}

/// Resolve an [`OutputIntentPolicy`] against the document's observed output
/// intents into a structured [`OutputIntentDecision`].
///
/// This function is pure: it performs no I/O, reads no PDF bytes, parses no ICC
/// profile, and does not panic on valid input. It is a planning input for a
/// later writer only.
///
/// Resolution rules:
///
/// - `Preserve` resolves to [`OutputIntentDecision::Preserve`] regardless of the
///   observed state.
/// - `RequireExisting` resolves to [`OutputIntentDecision::SatisfiedByExisting`]
///   when at least one intent is observed, otherwise to a
///   [`OutputIntentDecision::Rejected`] with
///   [`OutputIntentRejection::NoExistingIntent`].
/// - `EnsureTarget` resolves to [`OutputIntentDecision::AlreadySatisfied`] when
///   an observed intent matches the requested target identity, to
///   [`OutputIntentDecision::ConflictsWithExisting`] when an observed intent
///   shares the requested subtype but carries a different identifier, and
///   otherwise to [`OutputIntentDecision::RequiresEnsureTarget`].
///
/// When several intents are observed, a match takes priority over a conflict,
/// and a conflict takes priority over requires-ensure-target.
#[must_use]
pub fn resolve_output_intent_policy<I>(
    policy: &OutputIntentPolicy,
    observed: I,
) -> OutputIntentDecision
where
    I: IntoIterator<Item = ObservedOutputIntent>,
{
    match policy {
        OutputIntentPolicy::Preserve => OutputIntentDecision::Preserve,
        OutputIntentPolicy::RequireExisting => {
            if observed.into_iter().next().is_some() {
                OutputIntentDecision::SatisfiedByExisting
            } else {
                OutputIntentDecision::Rejected {
                    rejection: OutputIntentRejection::NoExistingIntent,
                }
            }
        }
        OutputIntentPolicy::EnsureTarget { target } => {
            let (subtype, identifier) = target_identity(target);
            // The first same-subtype, different-identifier intent is remembered
            // as a conflict, but a later exact match still wins: match takes
            // priority over conflict.
            let mut conflict: Option<ObservedOutputIntent> = None;
            for intent in observed {
                if intent.subtype == subtype {
                    if intent.output_condition_identifier.as_str() == identifier {
                        return OutputIntentDecision::AlreadySatisfied {
                            target: target.clone(),
                        };
                    }
                    if conflict.is_none() {
                        conflict = Some(intent);
                    }
                }
            }
            conflict.map_or_else(
                || OutputIntentDecision::RequiresEnsureTarget {
                    target: target.clone(),
                },
                |existing| OutputIntentDecision::ConflictsWithExisting {
                    requested: target.clone(),
                    existing,
                },
            )
        }
    }
}

/// Caller-supplied, ICC-free description of one spot color observed in a job.
///
/// This is a planning input only. It carries no ICC data, no tint-transform
/// function, and no color components: an observed spot is described abstractly
/// by its colorant `name` and the abstract `alternate` color space its tint
/// transform targets. `presslint-color` never derives these values from PDF
/// bytes; a caller supplies them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedSpotColor {
    /// Colorant name of the observed spot (e.g. the `Separation` name).
    pub name: String,
    /// Abstract alternate color space the spot's tint transform targets.
    pub alternate: ColorSpace,
}

/// Reason a spot policy could not be satisfied by the observed state.
///
/// This is a report-only planning result; it triggers no PDF mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SpotRejection {
    /// `Reject` was requested but at least one spot color was observed.
    SpotConversionRequired,
}

/// Reason a single observed spot was skipped instead of converted.
///
/// This is a report-only planning result; it triggers no PDF mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SpotSkipReason {
    /// The spot's alternate color space is not a process device color space, so
    /// this slice cannot plan its conversion.
    UnsupportedAlternate,
}

/// One observed spot that `ConvertAlternate` explicitly skipped, paired with the
/// reason it was skipped.
///
/// Reporting a skip rather than dropping the spot keeps unsupported shapes
/// preserved or explicitly skipped, never silently converted partially wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedSpotConversion {
    /// The observed spot that was skipped.
    pub spot: ObservedSpotColor,
    /// Structured reason the spot was skipped.
    pub reason: SpotSkipReason,
}

/// Pure resolution of a [`SpotPolicy`] against the observed spot colors.
///
/// This decision is a planning input for a later planner only. Producing it
/// inspects no PDF catalog, reads no `Separation`/`DeviceN` color space, parses
/// no ICC profile, evaluates no tint transform, and mutates no PDF bytes; it
/// reports what a later planner should do, not what it has done.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum SpotDecision {
    /// `Preserve`: leave any observed spot colors as is; nothing to plan.
    Preserve,
    /// `Reject` satisfied: no spot color was observed.
    NoSpotColors,
    /// `Reject` could not be satisfied: at least one spot color was observed.
    Rejected {
        /// Structured rejection reason.
        rejection: SpotRejection,
    },
    /// `ConvertAlternate`: the observed spots partitioned into those eligible for
    /// alternate conversion and those explicitly skipped, both in caller order.
    ConvertAlternate {
        /// Spots whose alternate is a process device color space, in caller order.
        eligible: Vec<ObservedSpotColor>,
        /// Spots explicitly skipped with a structured reason, in caller order.
        skipped: Vec<SkippedSpotConversion>,
    },
}

/// Whether a color space is a process device color space.
///
/// "Process device color space" means exactly [`ColorSpace::DeviceGray`],
/// [`ColorSpace::DeviceRgb`], or [`ColorSpace::DeviceCmyk`], matching the
/// process-color notion used elsewhere in the workspace. Every other
/// `ColorSpace` (`IccBased`, `Lab`, `CalGray`, `CalRgb`, `Indexed`,
/// `Separation`, `DeviceN`, `Pattern`, `Resource`, and `Unknown`) is not a
/// process device color space for this slice.
const fn is_process_device_color_space(space: &ColorSpace) -> bool {
    matches!(
        space,
        ColorSpace::DeviceGray | ColorSpace::DeviceRgb | ColorSpace::DeviceCmyk
    )
}

/// Resolve a [`SpotPolicy`] against the job's observed spot colors into a
/// structured [`SpotDecision`].
///
/// This function is pure: it performs no I/O, reads no PDF bytes, parses no ICC
/// profile, evaluates no tint transform, and does not panic on valid input. It
/// is a planning input for a later planner only.
///
/// Resolution rules:
///
/// - `Preserve` resolves to [`SpotDecision::Preserve`] regardless of the observed
///   spot colors.
/// - `Reject` resolves to [`SpotDecision::NoSpotColors`] when no spot color is
///   observed, and otherwise to a [`SpotDecision::Rejected`] with
///   [`SpotRejection::SpotConversionRequired`].
/// - `ConvertAlternate` partitions the observed spots in a single pass: a spot is
///   `eligible` only when its alternate is a process device color space (see
///   [`is_process_device_color_space`]), otherwise it is `skipped` with
///   [`SpotSkipReason::UnsupportedAlternate`]. Both lists preserve caller
///   iteration order.
#[must_use]
pub fn resolve_spot_policy<I>(policy: SpotPolicy, observed: I) -> SpotDecision
where
    I: IntoIterator<Item = ObservedSpotColor>,
{
    match policy {
        SpotPolicy::Preserve => SpotDecision::Preserve,
        SpotPolicy::Reject => {
            if observed.into_iter().next().is_some() {
                SpotDecision::Rejected {
                    rejection: SpotRejection::SpotConversionRequired,
                }
            } else {
                SpotDecision::NoSpotColors
            }
        }
        SpotPolicy::ConvertAlternate => {
            let mut eligible = Vec::new();
            let mut skipped = Vec::new();
            for spot in observed {
                if is_process_device_color_space(&spot.alternate) {
                    eligible.push(spot);
                } else {
                    skipped.push(SkippedSpotConversion {
                        spot,
                        reason: SpotSkipReason::UnsupportedAlternate,
                    });
                }
            }
            SpotDecision::ConvertAlternate { eligible, skipped }
        }
    }
}

/// Resolve a [`DeviceLinkPolicy`] against an abstract [`TransformRequest`] and
/// caller-supplied available `DeviceLink` descriptions.
///
/// This function is pure: it performs no I/O, reads no PDF bytes, parses no ICC
/// profile, and executes no transform. Matching is deliberately limited to exact
/// equality of `request.source == device_link.source` and
/// `request.destination == device_link.destination`.
///
/// Resolution rules:
///
/// - `Forbid` resolves to [`DeviceLinkDecision::UseProfileConnectionSpace`]
///   without inspecting supplied `DeviceLink` profiles.
/// - `Prefer` resolves to [`DeviceLinkDecision::UseDeviceLink`] for the first
///   exact match in caller order, otherwise to
///   [`DeviceLinkDecision::UseProfileConnectionSpace`].
/// - `Require` resolves to [`DeviceLinkDecision::UseDeviceLink`] for the first
///   exact match in caller order, otherwise to [`DeviceLinkDecision::Rejected`]
///   with [`DeviceLinkRejection::NoMatchingDeviceLink`].
#[must_use]
pub fn resolve_device_link_policy<I>(
    policy: DeviceLinkPolicy,
    request: &TransformRequest,
    available: I,
) -> DeviceLinkDecision
where
    I: IntoIterator<Item = DeviceLinkDescription>,
{
    if policy == DeviceLinkPolicy::Forbid {
        return DeviceLinkDecision::UseProfileConnectionSpace;
    }
    let requires_match = policy == DeviceLinkPolicy::Require;

    for device_link in available {
        if device_link.source == request.source && device_link.destination == request.destination {
            return DeviceLinkDecision::UseDeviceLink { device_link };
        }
    }

    if requires_match {
        DeviceLinkDecision::Rejected {
            rejection: DeviceLinkRejection::NoMatchingDeviceLink,
        }
    } else {
        DeviceLinkDecision::UseProfileConnectionSpace
    }
}
