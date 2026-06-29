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
