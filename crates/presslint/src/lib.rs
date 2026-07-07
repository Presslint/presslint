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

mod color_audit;
mod color_environment;
mod default_color_space_findings;
mod document_inventory;
mod form_expansion_machine;
mod form_inventory;
mod graphics_state_findings;
mod page_content;
mod pdf_inventory;
mod pdf_query;
mod preflight;

pub use presslint_types::*;

pub use color_audit::{
    ColorAuditStatus, ColorSpaceCount, ColorUsageAudit, ColorUsageAuditWithPolicyError,
    ColorUsageCount, ColorUsageSummary, CoverageGap, CoverageGapKind, ObjectKindCount,
    PageColorUsage, RgbFinding, audit_color_usage, audit_color_usage_with_output_intent_policy,
};
pub use color_environment::{
    OutputIntentEligibility, evaluate_pdf_output_intent_eligibility,
    observed_output_intents_from_pdf, resolve_output_intent_eligibility,
};
pub use default_color_space_findings::{DefaultColorSpaceFinding, DefaultColorSpaceFindingSource};
pub use document_inventory::{
    ClassicPdfInventory, ClassicPdfInventoryError, ClassicPdfInventoryPage,
    ClassicPdfInventoryPageResult, ClassicPdfInventoryRejection, ClassicPdfInventorySkip,
    build_classic_pdf_inventory,
};
pub use form_inventory::{
    FormExpandedInventory, FormWalkContext, SkippedFormInventory, SkippedFormInventoryReason,
    build_page_inventory_with_forms,
};
pub use graphics_state_findings::{GraphicsStateFinding, GraphicsStateFindingSource};
pub use pdf_inventory::{
    PdfInventory, PdfInventoryError, PdfInventoryPage, PdfInventoryPageResult,
    PdfInventoryRejection, PdfInventorySkip, build_pdf_inventory,
};
pub use pdf_query::{PdfInventoryMatch, PdfInventoryQuery, query_pdf_inventory};
pub use preflight::{
    PreflightCheck, PreflightFinding, PreflightReason, PreflightReport, PreflightSeverity,
    PreflightStatus, check_no_rgb_in_print,
};

pub use presslint_actions as actions;
pub use presslint_color as color;
pub use presslint_inventory as inventory;
pub use presslint_pdf as pdf;
pub use presslint_selectors as selectors;
pub use presslint_syntax as syntax;

#[cfg(test)]
mod tests;
