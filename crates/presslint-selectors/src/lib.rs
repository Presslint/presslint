//! Serializable selectors for inventory entries.

#![forbid(unsafe_code)]

use presslint_inventory::InventoryEntry;
use presslint_types::{ColorSpace, ColorUsage, ContentScope, ObjectKind, PageIndex};
use serde::{Deserialize, Serialize};

/// Boolean selector expression.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Selector {
    /// Match every entry.
    All,
    /// Match no entries.
    None,
    /// Negate an expression.
    Not {
        /// Expression to negate.
        expr: Box<Self>,
    },
    /// Match when every child matches.
    And {
        /// Child expressions evaluated with logical AND.
        exprs: Vec<Self>,
    },
    /// Match when any child matches.
    Or {
        /// Child expressions evaluated with logical OR.
        exprs: Vec<Self>,
    },
    /// Leaf predicate.
    Predicate {
        /// Predicate to evaluate.
        predicate: Predicate,
    },
}

/// Selector leaf predicate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Predicate {
    /// Match object kind.
    ObjectKind {
        /// Object kind to match.
        object_kind: ObjectKind,
    },
    /// Match observed color space.
    ColorSpace {
        /// Color space to match.
        space: ColorSpace,
    },
    /// Match zero-based page index.
    Page {
        /// Page index to match.
        page: PageIndex,
    },
    /// Match a page by parity, inclusive index range, or explicit index set.
    PageMatch {
        /// Page matcher applied to `entry.id.page`.
        matcher: PageMatcher,
    },
    /// Match entries that advertise an edit capability.
    Editable {
        /// Required edit capability.
        capability: presslint_types::EditCapability,
    },
    /// Match entries discovered in a specific content scope.
    Scope {
        /// Content scope matched by equality against `provenance.scope`.
        scope: ContentScope,
    },
    /// Match observed color usage.
    ColorUsage {
        /// Color usage to match.
        usage: ColorUsage,
    },
    /// Match one observed color by color space, optional usage, and components.
    ColorComponents {
        /// Color space to match on the same observation as `components`.
        space: ColorSpace,
        /// Optional color usage to match on the same observation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<ColorUsage>,
        /// Source-space components to match in order.
        components: Vec<f64>,
        /// Optional absolute per-component tolerance.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tolerance: Option<f64>,
    },
    /// Numerically compare a single observed component against a threshold.
    ///
    /// Matches when one `ColorObservation` supplies the requested color space,
    /// the optional usage, and a component at `component_index` whose value
    /// satisfies `component op value`. A threshold such as `K >= 0.85` is one
    /// `ComponentCompare`; a band such as `K >= 0.2 and K < 0.8` is an
    /// [`Selector::And`] of two `ComponentCompare` predicates — there is
    /// deliberately no dedicated band variant.
    ///
    /// VALUE CONVENTION: `value` and the observed components are PDF fractions in
    /// `0.0..=1.0`, so `K >= 85%` is `value: 0.85`. Any `%`-to-fraction
    /// conversion is the caller's responsibility and is not encoded here.
    ComponentCompare {
        /// Color space to match on the compared observation.
        space: ColorSpace,
        /// Optional color usage to match on the same observation.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<ColorUsage>,
        /// Zero-based index of the component to compare. An index past the
        /// observed components is a clean non-match, never a panic.
        component_index: usize,
        /// Comparison operator applied as `component op value`.
        op: CompareOp,
        /// Threshold as a PDF fraction in `0.0..=1.0` (caller does `%`->fraction).
        value: f64,
    },
}

/// Numeric comparison operator for [`Predicate::ComponentCompare`].
///
/// `op` is applied as `component op value`, so [`CompareOp::Ge`] with
/// `value: 0.85` expresses `component >= 0.85`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    /// `component >= value`.
    Ge,
    /// `component > value`.
    Gt,
    /// `component <= value`.
    Le,
    /// `component < value`.
    Lt,
    /// `component == value` (exact `f64` equality; no tolerance).
    Eq,
}

/// Page matcher for the [`Predicate::PageMatch`] leaf predicate.
///
/// All variants match against a zero-based [`PageIndex`]. Parity is the only
/// variant defined against the one-based page number; see [`PageParity`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "match", rename_all = "snake_case")]
pub enum PageMatcher {
    /// Match by one-based page-number parity.
    Parity {
        /// Parity to match.
        parity: PageParity,
    },
    /// Match an inclusive zero-based page-index range.
    ///
    /// Both ends are inclusive; the matcher matches nothing when
    /// `start > end`.
    Range {
        /// First matching page index (inclusive).
        start: PageIndex,
        /// Last matching page index (inclusive).
        end: PageIndex,
    },
    /// Match an explicit set of zero-based page indexes.
    ///
    /// Membership is by [`PageIndex`] equality and is independent of order and
    /// duplicates in `pages`.
    Set {
        /// Page indexes to match by equality.
        pages: Vec<PageIndex>,
    },
}

impl PageMatcher {
    /// Return whether `page` satisfies this matcher.
    ///
    /// Parity and range checks are O(1); set membership is a linear scan over
    /// the caller-owned `pages` with no per-call allocation.
    #[must_use]
    fn matches(&self, page: PageIndex) -> bool {
        match self {
            Self::Parity { parity } => parity.matches(page),
            Self::Range { start, end } => start.0 <= page.0 && page.0 <= end.0,
            Self::Set { pages } => pages.contains(&page),
        }
    }
}

/// Parity of the one-based page number.
///
/// Parity is computed on the one-based page number (`PageIndex` value + 1), so
/// `Odd` matches the first, third, and fifth page (indices 0, 2, 4) and `Even`
/// matches the second, fourth, and sixth page (indices 1, 3, 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageParity {
    /// Odd one-based page number: pages 1, 3, 5 (indices 0, 2, 4).
    Odd,
    /// Even one-based page number: pages 2, 4, 6 (indices 1, 3, 5).
    Even,
}

impl PageParity {
    /// Return whether `page` has this one-based-page-number parity.
    #[must_use]
    const fn matches(self, page: PageIndex) -> bool {
        // The one-based page number (index + 1) and the zero-based index always
        // have opposite low bits, so test the index directly to stay panic-free
        // even at `u32::MAX`.
        let index_is_even = page.0.is_multiple_of(2);
        match self {
            Self::Odd => index_is_even,
            Self::Even => !index_is_even,
        }
    }
}

/// Evaluate a selector against one inventory entry.
#[must_use]
pub fn matches(selector: &Selector, entry: &InventoryEntry) -> bool {
    match selector {
        Selector::All => true,
        Selector::None => false,
        Selector::Not { expr } => !matches(expr, entry),
        Selector::And { exprs } => exprs.iter().all(|expr| matches(expr, entry)),
        Selector::Or { exprs } => exprs.iter().any(|expr| matches(expr, entry)),
        Selector::Predicate { predicate } => matches_predicate(predicate, entry),
    }
}

fn matches_predicate(predicate: &Predicate, entry: &InventoryEntry) -> bool {
    match predicate {
        Predicate::ObjectKind { object_kind } => entry.kind == *object_kind,
        Predicate::ColorSpace { space } => entry.colors.iter().any(|color| color.space == *space),
        Predicate::Page { page } => entry.id.page == *page,
        Predicate::PageMatch { matcher } => matcher.matches(entry.id.page),
        Predicate::Editable { capability } => entry.capabilities.contains(capability),
        Predicate::Scope { scope } => entry.provenance.scope == *scope,
        Predicate::ColorUsage { usage } => entry.colors.iter().any(|color| color.usage == *usage),
        Predicate::ColorComponents {
            space,
            usage,
            components,
            tolerance,
        } => entry.colors.iter().any(|color| {
            color.space == *space
                && usage.is_none_or(|usage| color.usage == usage)
                && components_match(components, &color.components, *tolerance)
        }),
        Predicate::ComponentCompare {
            space,
            usage,
            component_index,
            op,
            value,
        } => entry.colors.iter().any(|color| {
            color.space == *space
                && usage.is_none_or(|usage| color.usage == usage)
                && component_compare_matches(color.components.get(*component_index), *op, *value)
        }),
    }
}

/// Evaluate `actual op value` for a single component read.
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

/// Apply one [`CompareOp`] to two finite `f64`s.
///
/// `Eq` uses the crate's exact-float policy (no tolerance); callers guarantee
/// both operands are finite.
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

#[cfg(test)]
mod tests;
