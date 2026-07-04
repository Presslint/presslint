//! Operator-local selector evaluation for targeted colour conversion (F4-4).
//!
//! This is a small, self-contained evaluator that decides — for ONE direct
//! device colour-setting operator — whether a caller-supplied
//! [`presslint_selectors::Selector`] matches it. It deliberately does NOT build
//! an `presslint_inventory::InventoryEntry`, does NOT track graphics state, and
//! does NOT call the inventory-backed `presslint_selectors::matches`. Instead it
//! evaluates the selector boolean tree against a tiny borrowed [`OperatorView`]
//! synthesised from what is knowable locally at the operator: the page index, the
//! operator's declared device colour space, its fill/stroke usage, and its
//! already-parsed operand components (borrowed, never copied).
//!
//! Selector leaves that require associating a colour with a painted object
//! ([`Predicate::ObjectKind`], [`Predicate::Editable`], [`Predicate::Scope`]) or a
//! non-operator-local colour usage/space (image/shading usage, ICCBased/Lab/spot
//! colour spaces) cannot be answered here. They are detected UP FRONT by
//! [`collect_unsupported_leaves`] so the caller can reject the whole request
//! before any page traversal, rather than silently under-converting.

use presslint_selectors::{CompareOp, PageMatcher, PageParity, Predicate, Selector};
use presslint_types::{ColorSpace, ColorUsage, PageIndex};
use serde::{Deserialize, Serialize};

use crate::content_color_convert::DeviceColorSpace;

/// A synthetic per-operator view over the facts a colour operator makes locally
/// available. All fields are cheap: `components` borrows the caller's already
/// parsed operand slice, so building a view allocates nothing.
pub struct OperatorView<'a> {
    /// Zero-based document page index of the page being converted.
    pub page_index: PageIndex,
    /// The operator's declared direct device colour space.
    pub color_space: DeviceColorSpace,
    /// Fill for lowercase `g`/`rg`/`k`, Stroke for uppercase `G`/`RG`/`K`.
    pub usage: ColorUsage,
    /// The operator's parsed operand components, in source-space order.
    pub components: &'a [f64],
}

/// A supported-in-principle predicate that this operator-local evaluator cannot
/// answer, surfaced up front so the whole request is rejected honestly.
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

/// Walk `selector` and collect every leaf this operator-local evaluator cannot
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

/// Evaluate `selector`'s boolean tree against one operator view.
///
/// Callers must have already rejected any unsupported leaf up front (see
/// [`collect_unsupported_leaves`]); an unsupported leaf reaching this evaluator
/// is treated as a non-match for totality.
#[must_use]
pub fn selector_matches(selector: &Selector, view: &OperatorView) -> bool {
    match selector {
        Selector::All => true,
        Selector::None => false,
        Selector::Not { expr } => !selector_matches(expr, view),
        Selector::And { exprs } => exprs.iter().all(|expr| selector_matches(expr, view)),
        Selector::Or { exprs } => exprs.iter().any(|expr| selector_matches(expr, view)),
        Selector::Predicate { predicate } => predicate_matches(predicate, view),
    }
}

fn predicate_matches(predicate: &Predicate, view: &OperatorView) -> bool {
    match predicate {
        Predicate::ColorSpace { space } => device_color_space(view.color_space) == *space,
        Predicate::Page { page } => view.page_index == *page,
        Predicate::PageMatch { matcher } => page_matches(matcher, view.page_index),
        Predicate::ColorUsage { usage } => view.usage == *usage,
        Predicate::ColorComponents {
            space,
            usage,
            components,
            tolerance,
        } => {
            device_color_space(view.color_space) == *space
                && usage.is_none_or(|usage| view.usage == usage)
                && components_match(components, view.components, *tolerance)
        }
        Predicate::ComponentCompare {
            space,
            usage,
            component_index,
            op,
            value,
        } => {
            device_color_space(view.color_space) == *space
                && usage.is_none_or(|usage| view.usage == usage)
                && component_compare_matches(view.components.get(*component_index), *op, *value)
        }
        // Unsupported leaves are rejected up front; never reached here.
        Predicate::ObjectKind { .. } | Predicate::Editable { .. } | Predicate::Scope { .. } => {
            false
        }
    }
}

/// Reimplements the selector crate's private page-match semantics locally, so we
/// never route a synthetic entry through the inventory matcher.
fn page_matches(matcher: &PageMatcher, page: PageIndex) -> bool {
    match matcher {
        PageMatcher::Parity { parity } => {
            // One-based page number and zero-based index have opposite low bits,
            // so test the index directly (panic-free even at `u32::MAX`).
            let index_is_even = page.0.is_multiple_of(2);
            match parity {
                PageParity::Odd => index_is_even,
                PageParity::Even => !index_is_even,
            }
        }
        PageMatcher::Range { start, end } => start.0 <= page.0 && page.0 <= end.0,
        PageMatcher::Set { pages } => pages.contains(&page),
    }
}

/// Reimplements the selector crate's private component-match semantics locally.
fn components_match(expected: &[f64], actual: &[f64], tolerance: Option<f64>) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    match tolerance {
        None => expected.iter().zip(actual).all(|(expected, actual)| {
            expected.is_finite()
                && actual.is_finite()
                && expected.partial_cmp(actual) == Some(std::cmp::Ordering::Equal)
        }),
        Some(tolerance) if tolerance.is_finite() && tolerance >= 0.0 => {
            expected.iter().zip(actual).all(|(expected, actual)| {
                expected.is_finite() && actual.is_finite() && (expected - actual).abs() <= tolerance
            })
        }
        Some(_) => false,
    }
}

/// Reimplements the selector crate's private component-compare semantics locally.
///
/// A missing component (index out of range), a non-finite `value`, or a
/// non-finite `actual` is a clean non-match — never a panic.
#[must_use]
fn component_compare_matches(actual: Option<&f64>, op: CompareOp, value: f64) -> bool {
    let Some(&actual) = actual else {
        return false;
    };
    if !actual.is_finite() || !value.is_finite() {
        return false;
    }
    compare_op(actual, op, value)
}

/// Apply one [`CompareOp`] to two finite `f64`s (`Eq` is exact, no tolerance).
#[must_use]
#[allow(clippy::float_cmp)]
fn compare_op(actual: f64, op: CompareOp, value: f64) -> bool {
    match op {
        CompareOp::Ge => actual >= value,
        CompareOp::Gt => actual > value,
        CompareOp::Le => actual <= value,
        CompareOp::Lt => actual < value,
        CompareOp::Eq => actual == value,
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
