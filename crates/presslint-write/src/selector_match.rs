//! Operator-local selector evaluation for targeted colour conversion (F4-4).
//!
//! Two pieces cooperate. [`collect_unsupported_leaves`] walks a caller-supplied
//! [`presslint_selectors::Selector`] UP FRONT — recursively under `Not`, `And`,
//! and `Or` — and reports every leaf that cannot be answered from what a single
//! direct device colour-setting operator makes locally available, so the caller
//! rejects the whole request before any page traversal rather than silently
//! under-converting.
//!
//! Once that total precheck has passed, [`selector_matches_operator`] evaluates
//! the selector through the CANONICAL inventory matcher
//! ([`presslint_selectors::matches`]) over a private, ephemeral, single-
//! observation `InventoryEntry` synthesised from the operator's page index,
//! declared device colour space, fill/stroke usage, and parsed components. The
//! entry's identity fields (sequence, digest, kind, scope, capabilities,
//! provenance range) are inert sentinels: the precheck guarantees the accepted
//! selector subset can only observe the page index and the one real colour
//! observation, so the sentinels are unobservable. The entry is never exposed,
//! serialized, cached, or treated as a real inventory object — it exists only
//! for the duration of one boolean evaluation.

use presslint_inventory::InventoryEntry;
use presslint_selectors::{Predicate, Selector};
use presslint_types::{
    ColorObservation, ColorSpace, ColorUsage, ContentScope, ObjectId, ObjectKind, PageIndex,
    Provenance,
};
use serde::{Deserialize, Serialize};

use crate::content_color_convert::DeviceColorSpace;

/// A supported-in-principle predicate that this operator-local evaluation
/// cannot answer, surfaced up front so the whole request is rejected honestly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "leaf", rename_all = "snake_case")]
pub enum UnsupportedTargetLeaf {
    /// `ObjectKind` targeting needs graphics-state association of a colour with
    /// the painted object it feeds (a later slice).
    ObjectKind {
        /// The requested object kind.
        object_kind: presslint_types::ObjectKind,
    },
    /// `Editable` targeting needs inventory edit-capability analysis.
    Editable {
        /// The requested edit capability.
        capability: presslint_types::EditCapability,
    },
    /// `Scope` targeting needs content-scope (form/annotation) association.
    Scope {
        /// The requested content scope.
        scope: presslint_types::ContentScope,
    },
    /// A `ColorSpace` predicate over a non-direct-device space (`ICCBased`, Lab,
    /// spot, resource, etc.).
    ColorSpace {
        /// The requested non-device colour space.
        space: ColorSpace,
    },
    /// A `ColorUsage` predicate for a usage other than Fill/Stroke (Image,
    /// Shading).
    ColorUsage {
        /// The requested non-fill/stroke usage.
        usage: ColorUsage,
    },
    /// A `ColorComponents` predicate over a non-device space or an
    /// Image/Shading usage.
    ColorComponents {
        /// The requested colour space.
        space: ColorSpace,
        /// The requested optional usage.
        usage: Option<ColorUsage>,
    },
    /// A `ComponentCompare` predicate over a non-direct-device space or an
    /// Image/Shading usage.
    ComponentCompare {
        /// The requested colour space.
        space: ColorSpace,
        /// The requested optional usage.
        usage: Option<ColorUsage>,
    },
}

/// Walk `selector` and collect every leaf this operator-local evaluation cannot
/// answer, in pre-order. An empty result means the selector is fully supported.
pub fn collect_unsupported_leaves(selector: &Selector) -> Vec<UnsupportedTargetLeaf> {
    let mut out = Vec::new();
    walk_unsupported(selector, &mut out);
    out
}

fn walk_unsupported(selector: &Selector, out: &mut Vec<UnsupportedTargetLeaf>) {
    match selector {
        Selector::All | Selector::None => {}
        Selector::Not { expr } => walk_unsupported(expr, out),
        Selector::And { exprs } | Selector::Or { exprs } => {
            for expr in exprs {
                walk_unsupported(expr, out);
            }
        }
        Selector::Predicate { predicate } => {
            if let Some(leaf) = unsupported_leaf(predicate) {
                out.push(leaf);
            }
        }
    }
}

fn unsupported_leaf(predicate: &Predicate) -> Option<UnsupportedTargetLeaf> {
    match predicate {
        Predicate::ObjectKind { object_kind } => Some(UnsupportedTargetLeaf::ObjectKind {
            object_kind: *object_kind,
        }),
        Predicate::Editable { capability } => Some(UnsupportedTargetLeaf::Editable {
            capability: *capability,
        }),
        Predicate::Scope { scope } => Some(UnsupportedTargetLeaf::Scope {
            scope: scope.clone(),
        }),
        Predicate::ColorSpace { space } => {
            (!is_device_space(space)).then(|| UnsupportedTargetLeaf::ColorSpace {
                space: space.clone(),
            })
        }
        Predicate::ColorUsage { usage } => (!is_fill_or_stroke(*usage))
            .then_some(UnsupportedTargetLeaf::ColorUsage { usage: *usage }),
        Predicate::ColorComponents { space, usage, .. } => {
            (!is_supported_component_leaf(space, *usage)).then(|| {
                UnsupportedTargetLeaf::ColorComponents {
                    space: space.clone(),
                    usage: *usage,
                }
            })
        }
        Predicate::ComponentCompare { space, usage, .. } => {
            (!is_supported_component_leaf(space, *usage)).then(|| {
                UnsupportedTargetLeaf::ComponentCompare {
                    space: space.clone(),
                    usage: *usage,
                }
            })
        }
        Predicate::Page { .. } | Predicate::PageMatch { .. } => None,
    }
}

const fn is_supported_component_leaf(space: &ColorSpace, usage: Option<ColorUsage>) -> bool {
    is_device_space(space)
        && match usage {
            Some(usage) => is_fill_or_stroke(usage),
            None => true,
        }
}

/// Evaluate `selector` for ONE direct device colour-setting operator through
/// the canonical inventory matcher.
///
/// PRECONDITION: the caller has already run [`collect_unsupported_leaves`] over
/// the WHOLE selector and rejected any unsupported leaf. Only then is the
/// private, ephemeral single-observation entry built here safe: every field the
/// accepted selector subset can observe (the page index and the one colour
/// observation) is real, and every other field is an inert sentinel that no
/// accepted leaf reads. The entry lives only for this call; it is never
/// exposed, serialized, cached, or reported as a real inventory object.
#[must_use]
pub fn selector_matches_operator(
    selector: &Selector,
    page_index: PageIndex,
    space: DeviceColorSpace,
    usage: ColorUsage,
    components: &[f64],
) -> bool {
    let entry = ephemeral_operator_entry(page_index, space, usage, components);
    presslint_selectors::matches(selector, &entry)
}

/// Build the private single-observation adapter entry for one operator.
///
/// Real fields: `id.page`, `provenance.page`, and exactly one
/// [`ColorObservation`] carrying the operator's declared device space,
/// fill/stroke usage, and components (copied once; at most four `f64`s). Inert
/// sentinel fields, unobservable by the accepted selector subset: zero
/// sequence, zero digest, `Vector` kind, `Page` scope, no provenance range, no
/// bounds, no capabilities.
fn ephemeral_operator_entry(
    page_index: PageIndex,
    space: DeviceColorSpace,
    usage: ColorUsage,
    components: &[f64],
) -> InventoryEntry {
    InventoryEntry {
        id: ObjectId {
            page: page_index,
            sequence: 0,
            digest: [0; 32],
        },
        kind: ObjectKind::Vector,
        provenance: Provenance {
            page: page_index,
            scope: ContentScope::Page,
            range: None,
            invocation: None,
        },
        bounds: None,
        colors: vec![ColorObservation {
            usage,
            space: device_color_space(space),
            components: components.to_vec(),
            spot_name: None,
            spot_names: Vec::new(),
            source: None,
        }],
        capabilities: Vec::new(),
    }
}

const fn device_color_space(space: DeviceColorSpace) -> ColorSpace {
    match space {
        DeviceColorSpace::Gray => ColorSpace::DeviceGray,
        DeviceColorSpace::Rgb => ColorSpace::DeviceRgb,
        DeviceColorSpace::Cmyk => ColorSpace::DeviceCmyk,
    }
}

const fn is_device_space(space: &ColorSpace) -> bool {
    matches!(
        space,
        ColorSpace::DeviceGray | ColorSpace::DeviceRgb | ColorSpace::DeviceCmyk
    )
}

const fn is_fill_or_stroke(usage: ColorUsage) -> bool {
    matches!(usage, ColorUsage::Fill | ColorUsage::Stroke)
}
