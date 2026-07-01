//! Umbrella crate for the `presslint` workspace: a single dependency that
//! re-exports the other crates.
//!
//! Shared data types are available at the crate root; each functional layer is a
//! namespaced module.
//!
//! ```text
//! presslint::{ObjectId, PageIndex, ...}  // shared types (from presslint-types)
//! presslint::pdf         // structural PDF access
//! presslint::syntax      // byte-preserving content-stream syntax
//! presslint::inventory   // page-object inventory
//! presslint::selectors   // selector model and matching
//! presslint::actions     // action/recipe model and planning
//! presslint::color       // color policy and transform planning
//! ```

mod document_inventory;
mod page_content;
mod pdf_inventory;
mod pdf_query;

pub use presslint_types::*;

pub use document_inventory::{
    ClassicPdfInventory, ClassicPdfInventoryError, ClassicPdfInventoryPage,
    ClassicPdfInventoryPageResult, ClassicPdfInventoryRejection, ClassicPdfInventorySkip,
    build_classic_pdf_inventory,
};
pub use pdf_inventory::{
    PdfInventory, PdfInventoryError, PdfInventoryPage, PdfInventoryPageResult,
    PdfInventoryRejection, PdfInventorySkip, build_pdf_inventory,
};
pub use pdf_query::{PdfInventoryMatch, PdfInventoryQuery, query_pdf_inventory};

pub use presslint_actions as actions;
pub use presslint_color as color;
pub use presslint_inventory as inventory;
pub use presslint_pdf as pdf;
pub use presslint_selectors as selectors;
pub use presslint_syntax as syntax;

#[cfg(test)]
mod tests;
