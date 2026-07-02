//! Validating plan-to-writer bridge for incremental revisions.
//!
//! [`write_incremental_revision_plan`] accepts a backend-agnostic
//! [`IncrementalRevisionPlan`] from `presslint-actions`, validates the whole
//! plan before assembling any bytes, converts the validated dirty objects to the
//! low-level [`DirtyObjectBytes`] contract, and delegates final PDF-format
//! checks and byte assembly to [`write_incremental_revision`].
//!
//! The plan carries *dirty-object intent only*. All xref/trailer/backend
//! mechanics — classic-vs-stream dispatch, `/Prev`, `/Size`, `/Root`, `/ID`,
//! `/Info`, encryption rejection, hybrid rejection, and object-currency/header
//! validation — stay owned by [`write_incremental_revision`]. This bridge adds
//! only the plan-layer checks the byte writer cannot express: boundary kind,
//! boundary target agreement, and ownership disposition.
//!
//! This slice executes [`MutationBoundary::DictionaryEntry`] and
//! [`MutationBoundary::WholeStream`] boundaries: both validate that the boundary
//! target matches the dirty object and that ownership is in-place mutation, then
//! reuse the caller-built replacement `body_bytes`.
//! [`MutationBoundary::ContentStreamOperand`] and
//! [`MutationBoundary::IndirectObjectClone`] are still rejected as unsupported
//! execution shapes.

use presslint_actions::{IncrementalRevisionPlan, MutationBoundary, PlannedDirtyObject};
use presslint_pdf::{IndirectObjectEditDisposition, IndirectRef};
use serde::{Deserialize, Serialize};

use crate::{DirtyObjectBytes, WriteError, write_incremental_revision};

/// Boundary execution shape this slice does not yet write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedBoundaryKind {
    /// A content-stream operand rewrite.
    ContentStreamOperand,
    /// A whole-stream replacement.
    WholeStream,
    /// A private-copy indirect-object clone.
    IndirectObjectClone,
}

/// Error returned when an [`IncrementalRevisionPlan`] cannot be written.
///
/// Plan-layer variants are decided before any bytes are assembled; `Write`
/// carries a delegated [`WriteError`] from the byte writer verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum PlannedWriteError {
    /// The plan contained no dirty objects.
    EmptyPlan,
    /// A dirty object carried no mutation boundaries.
    EmptyBoundaries {
        /// Reference of the dirty object with no boundaries.
        reference: IndirectRef,
    },
    /// A boundary used an execution shape this slice does not write.
    UnsupportedBoundaryKind {
        /// Reference of the dirty object carrying the boundary.
        reference: IndirectRef,
        /// The unsupported boundary shape.
        kind: UnsupportedBoundaryKind,
    },
    /// A dictionary-entry boundary targeted a different object than the dirty
    /// object it belongs to.
    BoundaryTargetMismatch {
        /// Reference of the dirty object the boundary belongs to.
        reference: IndirectRef,
        /// Target the boundary named instead.
        boundary_target: IndirectRef,
    },
    /// A boundary's proven ownership disposition was not in-place mutation, so it
    /// must not be executed as an in-place object rewrite.
    OwnershipNotInPlace {
        /// Reference of the dirty object carrying the boundary.
        reference: IndirectRef,
        /// The disposition the boundary carried instead.
        disposition: IndirectObjectEditDisposition,
    },
    /// Two dirty objects shared the same object number.
    DuplicateDirtyObject {
        /// The repeated object number.
        object_number: u32,
    },
    /// The delegated byte writer rejected the input or the assembled revision.
    Write {
        /// Delegated append-writer failure.
        error: Box<WriteError>,
    },
}

/// Validate an [`IncrementalRevisionPlan`] and write one appended incremental
/// revision, delegating byte assembly to [`write_incremental_revision`].
///
/// The whole plan is validated before any bytes are assembled: an empty plan, a
/// dirty object with no boundaries, an unsupported boundary kind, a boundary
/// whose target does not match its dirty object, a boundary whose ownership
/// disposition is not in-place mutation, and duplicate dirty object numbers are
/// all rejected at the plan layer. Dirty objects are then ordered deterministically
/// by [`IndirectRef`] and converted to [`DirtyObjectBytes`]; the byte writer owns
/// all remaining xref/trailer/backend checks and returns `input` verbatim as the
/// output prefix.
///
/// # Errors
///
/// Returns [`PlannedWriteError`] for any plan-layer rejection above, or
/// [`PlannedWriteError::Write`] wrapping the delegated [`WriteError`].
pub fn write_incremental_revision_plan(
    input: &[u8],
    plan: &IncrementalRevisionPlan,
) -> Result<Vec<u8>, PlannedWriteError> {
    if plan.dirty_objects.is_empty() {
        return Err(PlannedWriteError::EmptyPlan);
    }

    // Order deterministically by indirect reference, independent of the caller's
    // plan order and any future backend choice.
    let mut ordered: Vec<&PlannedDirtyObject> = plan.dirty_objects.iter().collect();
    ordered.sort_by_key(|dirty| dirty.reference);

    // Reject duplicate object numbers at the plan layer, before the byte writer
    // would see them.
    for pair in ordered.windows(2) {
        if pair[0].reference.object_number == pair[1].reference.object_number {
            return Err(PlannedWriteError::DuplicateDirtyObject {
                object_number: pair[0].reference.object_number,
            });
        }
    }

    // Validate every dirty object's boundaries before assembling any bytes.
    for dirty in &ordered {
        validate_dirty_object(dirty)?;
    }

    // Convert to the low-level dirty-object contract. `body_bytes` is the one
    // intentional owned copy: the replacement-body payload the `DirtyObjectBytes`
    // writer contract already requires. No source PDF bytes are copied here.
    let dirty_objects: Vec<DirtyObjectBytes> = ordered
        .iter()
        .map(|dirty| DirtyObjectBytes {
            reference: dirty.reference,
            body_bytes: dirty.body_bytes.clone(),
        })
        .collect();

    write_incremental_revision(input, &dirty_objects).map_err(|error| PlannedWriteError::Write {
        error: Box::new(error),
    })
}

/// Validate one dirty object: it must carry at least one boundary, and every
/// boundary must be an executable in-place dictionary-entry edit of this object.
fn validate_dirty_object(dirty: &PlannedDirtyObject) -> Result<(), PlannedWriteError> {
    if dirty.boundaries.is_empty() {
        return Err(PlannedWriteError::EmptyBoundaries {
            reference: dirty.reference,
        });
    }
    for boundary in &dirty.boundaries {
        validate_boundary(dirty.reference, boundary)?;
    }
    Ok(())
}

/// Validate one boundary against the dirty object it belongs to.
///
/// [`MutationBoundary::DictionaryEntry`] and [`MutationBoundary::WholeStream`]
/// are both executed as in-place object rewrites: their `target` must equal
/// `reference` and their ownership disposition must be in-place mutation. The
/// caller has already built the full replacement `body_bytes`, so the bridge
/// only checks intent, never re-derives the payload.
/// [`MutationBoundary::ContentStreamOperand`] and
/// [`MutationBoundary::IndirectObjectClone`] remain unsupported.
fn validate_boundary(
    reference: IndirectRef,
    boundary: &MutationBoundary,
) -> Result<(), PlannedWriteError> {
    match boundary {
        MutationBoundary::DictionaryEntry {
            target, ownership, ..
        }
        | MutationBoundary::WholeStream {
            target, ownership, ..
        } => validate_in_place_target(reference, *target, ownership.disposition),
        MutationBoundary::ContentStreamOperand { .. } => Err(unsupported(
            reference,
            UnsupportedBoundaryKind::ContentStreamOperand,
        )),
        MutationBoundary::IndirectObjectClone { .. } => Err(unsupported(
            reference,
            UnsupportedBoundaryKind::IndirectObjectClone,
        )),
    }
}

/// Validate that an in-place boundary targets its own dirty object with proven
/// in-place-mutation ownership.
fn validate_in_place_target(
    reference: IndirectRef,
    target: IndirectRef,
    disposition: IndirectObjectEditDisposition,
) -> Result<(), PlannedWriteError> {
    if target != reference {
        return Err(PlannedWriteError::BoundaryTargetMismatch {
            reference,
            boundary_target: target,
        });
    }
    if disposition != IndirectObjectEditDisposition::InPlaceMutation {
        return Err(PlannedWriteError::OwnershipNotInPlace {
            reference,
            disposition,
        });
    }
    Ok(())
}

/// Build an [`PlannedWriteError::UnsupportedBoundaryKind`] for `reference`.
const fn unsupported(reference: IndirectRef, kind: UnsupportedBoundaryKind) -> PlannedWriteError {
    PlannedWriteError::UnsupportedBoundaryKind { reference, kind }
}
