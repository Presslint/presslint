//! Backend-agnostic incremental-revision plan contract.
//!
//! An [`IncrementalRevisionPlan`] models *dirty-object intent only*: which
//! existing indirect objects will be rewritten, the [`MutationBoundary`] records
//! that justify each rewrite, and the replacement object body bytes. It is the
//! shared hand-off shape between action planning (`presslint-actions`) and byte
//! writing (`presslint-write`).
//!
//! The plan is deliberately **backend-agnostic**: it carries no cross-reference
//! table, cross-reference stream, trailer, `/Prev`, `/Size`, `/Root`, `/ID`,
//! `/Info`, object-number-allocation, or classic-vs-stream backend-selection
//! mechanics. Those remain owned by the writer, so a future writer backend can
//! consume xref-stream inputs without reshaping action plans. The plan also does
//! not copy source PDF bytes: boundaries carry references, ranges, ownership
//! decisions, and provenance only. The single owned byte payload is
//! [`PlannedDirtyObject::body_bytes`], the replacement-body bytes the existing
//! `DirtyObjectBytes` writer contract already requires.

use presslint_pdf::IndirectRef;
use serde::{Deserialize, Serialize};

use crate::MutationBoundary;

/// Backend-agnostic plan for one appended incremental revision.
///
/// Holds only the set of existing indirect objects to rewrite. It intentionally
/// carries no xref/trailer/`/Prev`/`/Size`/object-allocation/backend mechanics;
/// the writer owns those. Ordering of `dirty_objects` is not significant: the
/// writer sorts deterministically by [`IndirectRef`] before assembling bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncrementalRevisionPlan {
    /// Existing indirect objects to rewrite in the appended revision.
    pub dirty_objects: Vec<PlannedDirtyObject>,
}

/// One existing indirect object to rewrite, with the boundaries that justify it.
///
/// `body_bytes` is the replacement indirect-object *body* only, matching
/// `DirtyObjectBytes`: no `N G obj` header and no closing `endobj`. `boundaries`
/// records the mutation intent behind the rewrite; the writer validates the
/// boundaries against this object's `reference` before converting the plan to
/// the low-level dirty-object writer contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlannedDirtyObject {
    /// Indirect reference of the existing object to rewrite.
    pub reference: IndirectRef,
    /// Mutation boundaries that justify rewriting this object.
    pub boundaries: Vec<MutationBoundary>,
    /// Replacement indirect-object body bytes (no header, no `endobj`).
    pub body_bytes: Vec<u8>,
}
