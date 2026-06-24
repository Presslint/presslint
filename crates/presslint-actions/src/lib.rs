//! Serializable actions, recipes, and patch-plan identifiers.

#![forbid(unsafe_code)]

use presslint_core::ObjectId;
use presslint_selectors::Selector;
use serde::{Deserialize, Serialize};

/// Versioned recipe document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Recipe {
    /// Schema version.
    pub schema_version: u32,
    /// Ordered recipe steps.
    pub steps: Vec<RecipeStep>,
}

/// One selector/action pair.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeStep {
    /// Selector choosing inventory entries.
    pub select: Selector,
    /// Action applied to matching entries.
    pub action: Action,
}

/// Serializable action request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Action {
    /// Convert selected process color observations.
    ConvertColor(ConvertColor),
    /// Add spread stroke to selected text.
    SpreadText(SpreadText),
    /// Enforce a minimum vector stroke width.
    MinimumStrokeWidth(MinimumStrokeWidth),
}

/// Color-conversion action payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertColor {
    /// Named target condition or profile identifier.
    pub target: String,
}

/// Text spreading action payload.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpreadText {
    /// Spread amount in points.
    pub amount_pt: f64,
    /// Whether the added stroke should overprint.
    pub overprint: bool,
}

/// Minimum stroke-width action payload.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MinimumStrokeWidth {
    /// Minimum stroke width in points.
    pub width_pt: f64,
}

/// Planned action against selected objects.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionPlan {
    /// Objects selected for the action.
    pub targets: Vec<ObjectId>,
    /// Requested action.
    pub action: Action,
}
