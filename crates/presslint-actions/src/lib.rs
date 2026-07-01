//! Serializable actions, recipes, and no-op patch planning.

#![forbid(unsafe_code)]

mod patch;

use presslint_inventory::{Inventory, InventoryEntry};
use presslint_pdf::PageRectangle;
use presslint_selectors::{Selector, matches as selector_matches};
use presslint_types::{ColorSpace, EditCapability, ObjectId};
use serde::{Deserialize, Serialize};

pub use patch::{
    DictionaryEntryOp, DictionaryValueLocator, MutationBoundary, PlannedObjectAllocation,
    PlannedValueProvenance, plan_set_page_box_boundaries,
};

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
    /// Set `/MediaBox` and/or `/CropBox` on a leaf page dictionary.
    ///
    /// This is a page-dictionary action, not a content-inventory action: it is
    /// planned through [`plan_set_page_box_boundaries`] against page-box
    /// provenance, not through [`plan_recipe`] against an [`Inventory`]. It is a
    /// report-only planning contract and never writes bytes.
    SetPageBox(SetPageBox),
}

impl Action {
    /// Return the inventory edit capability required to plan this action, or
    /// `None` for actions that do not target content-inventory entries.
    ///
    /// [`Action::SetPageBox`] returns `None`: it edits page dictionaries rather
    /// than inventory entries, so it is not selected through the inventory recipe
    /// planner.
    #[must_use]
    pub const fn required_capability(&self) -> Option<EditCapability> {
        match self {
            Self::ConvertColor(_) => Some(EditCapability::RewriteColorOperand),
            Self::SpreadText(_) => Some(EditCapability::AddTextSpreadStroke),
            Self::MinimumStrokeWidth(_) => Some(EditCapability::AdjustStrokeWidth),
            Self::SetPageBox(_) => None,
        }
    }
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

/// Page-box action payload.
///
/// At least one of `media_box` / `crop_box` is expected to be set. The rectangle
/// values are the requested page boundaries; normalization and geometric
/// validation live in the writer, not in this report-only contract.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SetPageBox {
    /// Requested `/MediaBox`, when set.
    pub media_box: Option<PageRectangle>,
    /// Requested `/CropBox`, when set.
    pub crop_box: Option<PageRectangle>,
}

/// Explicit patch-plan mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PatchPlanMode {
    /// Planning report only; no PDF bytes are written or mutated.
    NoOp,
}

/// Deterministic no-op patch plan for a recipe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchPlan {
    /// Explicitly records that this plan is report-only.
    pub mode: PatchPlanMode,
    /// Planned recipe steps in recipe order.
    pub steps: Vec<ActionPlan>,
}

impl PatchPlan {
    /// Return true when the plan is explicitly report-only.
    #[must_use]
    pub const fn is_no_op(&self) -> bool {
        matches!(self.mode, PatchPlanMode::NoOp)
    }
}

/// Planned action report for one recipe step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionPlan {
    /// Requested action.
    pub action: Action,
    /// Objects selected for the action in inventory order.
    pub targets: Vec<ObjectId>,
    /// Planned report-only patches for targets with a concrete future mutation
    /// boundary.
    pub patches: Vec<PlannedPatch>,
    /// Objects matched by the selector but skipped before future mutation.
    pub skipped: Vec<SkippedTarget>,
}

/// Report-only patch request for a selected target.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedPatch {
    /// Selected object the future patch would apply to.
    pub object: ObjectId,
    /// Edit capability proven for this planned patch.
    pub capability: EditCapability,
    /// Narrow source boundary a future executor would be allowed to edit.
    pub boundary: MutationBoundary,
}

/// Matched inventory entry skipped during planning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedTarget {
    /// Matched object that cannot currently receive the requested action.
    pub object: ObjectId,
    /// Structured reason for the skip.
    pub reason: SkipReason,
}

/// Structured reason a matched object was omitted from action targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkipReason {
    /// The inventory entry does not advertise the action's required capability.
    UnsupportedCapability {
        /// Required edit capability for the requested action.
        required: EditCapability,
    },
    /// The matched `ConvertColor` entry advertises the rewrite capability but
    /// carries no process device color observation (`DeviceGray` /
    /// `DeviceRGB` / `DeviceCMYK`) that the no-op planner can target.
    NonProcessColor,
    /// The matched `ConvertColor` entry carries at least one process device
    /// color observation whose color operand source byte provenance is missing.
    MissingColorSource,
    /// The matched `ConvertColor` entry carries more than one sourced process
    /// device color observation, so object-level planning cannot choose a
    /// single future rewrite operand.
    AmbiguousColorSource,
}

/// Return true when `space` is one of the three process device color spaces the
/// no-op convert slice can target (`DeviceGray` / `DeviceRGB` / `DeviceCMYK`).
///
/// Every other `ColorSpace` shape (`IccBased`, `Lab`, `CalGray`, `CalRgb`,
/// `Indexed`, `Separation`, `DeviceN`, `Pattern`, `Resource`, `Unknown`) is
/// treated as non-process for this slice.
const fn is_process_space(space: &ColorSpace) -> bool {
    matches!(
        space,
        ColorSpace::DeviceGray | ColorSpace::DeviceRgb | ColorSpace::DeviceCmyk
    )
}

/// Return the planned mutation boundary for a capability-passing `ConvertColor`
/// entry, or a structured skip reason when no single sourced process color
/// operator can be selected.
///
/// `ConvertColor` additionally requires exactly one sourced process device
/// color observation.
fn convert_color_boundary(
    entry: &InventoryEntry,
    action: Action,
) -> Result<MutationBoundary, SkipReason> {
    let mut has_process_color = false;
    let mut missing_source = false;
    let mut ambiguous_source = false;
    let mut source = None;

    for color in &entry.colors {
        if is_process_space(&color.space) {
            has_process_color = true;

            match color.source {
                Some(range) if source.is_none() => source = Some(range),
                Some(_) => ambiguous_source = true,
                None => missing_source = true,
            }
        }
    }

    if !has_process_color {
        Err(SkipReason::NonProcessColor)
    } else if missing_source {
        Err(SkipReason::MissingColorSource)
    } else if ambiguous_source {
        Err(SkipReason::AmbiguousColorSource)
    } else if let Some(record_range) = source {
        Ok(MutationBoundary::ContentStreamOperand {
            page: entry.provenance.page,
            scope: entry.provenance.scope.clone(),
            record_range,
            operand_range: None,
            operator_range: None,
            ownership: None,
            value_provenance: PlannedValueProvenance::ActionGenerated { action },
        })
    } else {
        Err(SkipReason::MissingColorSource)
    }
}

fn plan_patch_for_action(
    action: &Action,
    entry: &InventoryEntry,
    required: EditCapability,
) -> Result<Option<PlannedPatch>, SkipReason> {
    match action {
        Action::ConvertColor(_) => {
            let boundary = convert_color_boundary(entry, action.clone())?;
            Ok(Some(PlannedPatch {
                object: entry.id.clone(),
                capability: required,
                boundary,
            }))
        }
        Action::SpreadText(_) | Action::MinimumStrokeWidth(_) | Action::SetPageBox(_) => Ok(None),
    }
}

/// Evaluate a recipe against an inventory and return a deterministic no-op plan.
///
/// Selectors are evaluated against entries in inventory order. Matching entries
/// without the action's required edit capability are reported as
/// `UnsupportedCapability` skips. For `ConvertColor`, capability-passing entries
/// become targets only when they carry exactly one process device color
/// observation with source byte provenance. Capability-passing entries with no
/// process device color are reported as `NonProcessColor`; entries with any
/// process device color missing source byte provenance are reported as
/// `MissingColorSource`; entries with multiple sourced process device color
/// observations are reported as `AmbiguousColorSource`.
/// `SpreadText` and `MinimumStrokeWidth` treat every capability-passing entry
/// as a target.
#[must_use]
pub fn plan_recipe(recipe: &Recipe, inventory: &Inventory) -> PatchPlan {
    let steps = recipe
        .steps
        .iter()
        .map(|step| plan_step(step, inventory))
        .collect();

    PatchPlan {
        mode: PatchPlanMode::NoOp,
        steps,
    }
}

fn plan_step(step: &RecipeStep, inventory: &Inventory) -> ActionPlan {
    // Actions without an inventory capability (for example page-dictionary
    // actions) are not selected through the inventory recipe planner; they emit
    // an empty plan here and are planned through their dedicated helper instead.
    let Some(required) = step.action.required_capability() else {
        return ActionPlan {
            action: step.action.clone(),
            targets: Vec::new(),
            patches: Vec::new(),
            skipped: Vec::new(),
        };
    };
    let mut targets = Vec::new();
    let mut patches = Vec::new();
    let mut skipped = Vec::new();

    for entry in &inventory.entries {
        if !selector_matches(&step.select, entry) {
            continue;
        }

        if !entry.capabilities.contains(&required) {
            skipped.push(SkippedTarget {
                object: entry.id.clone(),
                reason: SkipReason::UnsupportedCapability { required },
            });
            continue;
        }

        // The capability check takes precedence over per-action eligibility.
        let patch = match plan_patch_for_action(&step.action, entry, required) {
            Ok(patch) => patch,
            Err(reason) => {
                skipped.push(SkippedTarget {
                    object: entry.id.clone(),
                    reason,
                });
                continue;
            }
        };

        targets.push(entry.id.clone());
        if let Some(patch) = patch {
            patches.push(patch);
        }
    }

    ActionPlan {
        action: step.action.clone(),
        targets,
        patches,
        skipped,
    }
}

#[cfg(test)]
mod tests;
