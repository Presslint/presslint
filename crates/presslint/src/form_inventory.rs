//! Public data types and bounded walk context for Form `XObject` content
//! expansion in the PDF inventory bridge.
//!
//! A page-level Form `XObject` invocation (`/Fm Do`) is inventoried by the
//! page-content walker only as a `FormXObject` invocation entry; the colors,
//! text, and vectors painted INSIDE the form stay invisible. Form expansion
//! walks a form's OWN decoded content stream, resolves the form's own
//! colour-space environment, re-invokes the inventory builder in
//! [`ContentScope::FormXObject`] with the ORIGINAL invoking page index, and
//! merges nested entries immediately after the form invocation entry.
//!
//! The traversal itself now runs on the shared paint call/return machine in
//! [`crate::form_expansion_machine`]; the public entry
//! [`build_page_inventory_with_forms`] is re-exported from there. This module
//! owns the caller-facing result and diagnostic types plus the bounded
//! [`FormWalkContext`] (default limit 8 form-stream descents from the page plus
//! a per-page total expansion budget, with active-path cycle detection). Every
//! per-form failure is a structured [`SkippedFormInventory`], never a page
//! failure, panic, or infinite loop; the page's own inventory is always emitted.
//!
//! [`ContentScope`]: presslint_types::ContentScope

use presslint_inventory::Inventory;
use presslint_pdf::{IndirectRef, SkippedPageXObjectResource};
use presslint_types::PdfName;
use serde::{Deserialize, Serialize};

use crate::pdf_inventory::PdfInventorySkip;

pub use crate::form_expansion_machine::build_page_inventory_with_forms;

/// Combined page inventory plus per-form expansion diagnostics for one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormExpandedInventory {
    /// Page inventory with nested form entries merged after their invocation.
    pub inventory: Inventory,
    /// Structured per-form expansion skips for this page, in content order.
    pub form_skipped: Vec<SkippedFormInventory>,
}

/// One structured Form `XObject` expansion skip.
///
/// The page's own inventory is always produced; this records a page-level (or,
/// for future deeper walks, nested) form whose content could not be inventoried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedFormInventory {
    /// Resource name used to invoke the form.
    pub name: PdfName,
    /// Resolved indirect reference of the form stream object.
    pub reference: IndirectRef,
    /// Resolved form stream object byte offset.
    pub object_byte_offset: usize,
    /// Structured reason the form content was not inventoried.
    pub reason: SkippedFormInventoryReason,
}

/// Structured reason a Form `XObject`'s own content was not inventoried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedFormInventoryReason {
    /// The form re-invokes a form already on the active walk stack (self-ref or
    /// cycle); descending would not terminate.
    Cycle,
    /// The bounded walk reached its configured maximum form nesting depth, so
    /// this nested form was inventoried as an invocation but not descended.
    MaxDepth {
        /// Configured maximum form nesting depth for the walk.
        max_depth: usize,
    },
    /// The page-level total form-expansion budget was exhausted before this
    /// form could be decoded, tokenized, assembled, or inventoried.
    BudgetExhausted {
        /// Configured maximum number of form expansion attempts for one page.
        max_expansions: usize,
    },
    /// The form stream could not be located, decoded, tokenized, assembled, or
    /// walked. Delegates to the shared content-skip vocabulary.
    Content {
        /// Delegated content-processing skip for the form stream.
        skip: PdfInventorySkip,
    },
    /// A nested resource in the form's own `/Resources /XObject` dictionary
    /// could not be classified, so invocations of that resource cannot be
    /// recursively inventoried.
    Resource {
        /// Delegated resource-classification diagnostic.
        skip: SkippedPageXObjectResource,
    },
}

/// Bounded walk context for one page's form expansion.
///
/// `max_depth` bounds form-stream descents from the page. `max_expansions`
/// bounds total form expansion attempts for one page and is consumed before any
/// form stream work begins; it is not restored on ascent. Active-path cycle
/// detection (a form that re-invokes an ancestor) is enforced by the resolver in
/// [`crate::form_expansion_machine`], which keys forms currently on the active
/// descent path by resolved `(object_number, generation)` plus byte offset so a
/// form that re-invokes an ancestor is detected as a cycle without blocking
/// legitimate sibling re-invocations.
#[derive(Debug, Clone)]
pub struct FormWalkContext {
    pub(crate) max_depth: usize,
    pub(crate) max_expansions: usize,
    pub(crate) remaining_expansions: usize,
}

impl FormWalkContext {
    /// Create a context bounded to `max_depth` levels of form nesting.
    #[must_use]
    pub const fn new(max_depth: usize) -> Self {
        Self::with_budget(max_depth, 256)
    }

    /// Create a context bounded by nesting depth and total page expansion
    /// attempts.
    #[must_use]
    pub const fn with_budget(max_depth: usize, max_expansions: usize) -> Self {
        Self {
            max_depth,
            max_expansions,
            remaining_expansions: max_expansions,
        }
    }

    /// Create the default bounded context used by the inventory bridges.
    #[must_use]
    pub const fn bounded_default() -> Self {
        Self::new(8)
    }

    /// Create a one-level context for focused tests and compatibility.
    #[must_use]
    pub const fn one_level() -> Self {
        Self::new(1)
    }
}
