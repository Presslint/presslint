//! Serializable selectors for inventory entries.

#![forbid(unsafe_code)]

use presslint_core::{ColorSpace, ObjectKind, PageIndex};
use presslint_inventory::InventoryEntry;
use serde::{Deserialize, Serialize};

/// Boolean selector expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Match entries that advertise an edit capability.
    Editable {
        /// Required edit capability.
        capability: presslint_core::EditCapability,
    },
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
        Predicate::Editable { capability } => entry.capabilities.contains(capability),
    }
}
