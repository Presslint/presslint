//! Mutation-boundary contracts for future PDF patch execution.

use presslint_pdf::{IndirectObjectEditDecision, IndirectRef};
use presslint_types::{ByteRange, ContentScope, PageIndex, PdfName};
use serde::{Deserialize, Serialize};

use crate::{Action, SetPageBox};

const MEDIA_BOX_KEY: &[u8] = b"MediaBox";
const CROP_BOX_KEY: &[u8] = b"CropBox";

/// Serializable boundary for a future mutation.
///
/// Live planning currently emits only [`MutationBoundary::ContentStreamOperand`].
/// The indirect-object variants are frozen public contract shapes for a future
/// incremental-update executor and are exercised only by serde shape tests.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MutationBoundary {
    /// Operand in one decoded content stream operator record.
    ContentStreamOperand {
        /// Page where the editable content stream was observed.
        page: PageIndex,
        /// Stable content scope identifier.
        scope: ContentScope,
        /// Byte range of the sourced operator record.
        record_range: ByteRange,
        /// Optional byte range for the operand within the operator record.
        operand_range: Option<ByteRange>,
        /// Optional byte range for the operator token within the record.
        operator_range: Option<ByteRange>,
        /// Proven ownership decision when the content stream has a concrete
        /// indirect reference. Inventory object IDs are not indirect refs, so
        /// live content-stream planning leaves this unset.
        ownership: Option<IndirectObjectEditDecision>,
        /// Provenance of the planned replacement value.
        value_provenance: PlannedValueProvenance,
    },
    /// Dictionary key/value edit in an indirect object.
    DictionaryEntry {
        /// Indirect object whose dictionary entry would be edited.
        target: IndirectRef,
        /// Dictionary key to replace or insert.
        key: PdfName,
        /// Dictionary-entry operation.
        op: DictionaryEntryOp,
        /// Source locator for the existing value or insertion point.
        value_locator: DictionaryValueLocator,
        /// Proven shared-object ownership decision.
        ownership: IndirectObjectEditDecision,
        /// Provenance of the planned value.
        value_provenance: PlannedValueProvenance,
    },
    /// Whole stream replacement in an indirect object.
    WholeStream {
        /// Indirect stream object to replace.
        target: IndirectRef,
        /// Optional byte range for the existing decoded stream data.
        stream_data_range: Option<ByteRange>,
        /// Proven shared-object ownership decision.
        ownership: IndirectObjectEditDecision,
        /// Provenance of the planned stream value.
        value_provenance: PlannedValueProvenance,
    },
    /// Private clone of a shared indirect object plus the reference patch that
    /// redirects one consumer to the clone.
    IndirectObjectClone {
        /// Shared source object that must not be edited in place.
        source: IndirectRef,
        /// Consumer whose reference would be redirected.
        consumer: IndirectRef,
        /// Allocation plan for the private clone.
        new_object: PlannedObjectAllocation,
        /// Boundary for the consumer-side reference rewrite.
        reference_patch: Box<Self>,
        /// Proven shared-object ownership decision for the source object.
        ownership: IndirectObjectEditDecision,
        /// Provenance of the planned clone value.
        value_provenance: PlannedValueProvenance,
    },
}

/// Planned dictionary-entry operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryEntryOp {
    /// Replace an existing key/value entry.
    Replace,
    /// Insert a new key/value entry.
    Insert,
}

/// Source locator for a planned dictionary value edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DictionaryValueLocator {
    /// Existing key/value spans in the source dictionary.
    ExistingValue {
        /// Source range of the key token.
        key_range: ByteRange,
        /// Source range of the value token or object.
        value_range: ByteRange,
    },
    /// Dictionary span where a new key/value entry would be inserted.
    InsertionPoint {
        /// Source range of the containing dictionary.
        dictionary_range: ByteRange,
    },
}

/// Provenance of a planned replacement value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlannedValueProvenance {
    /// Value generated from an action request.
    ActionGenerated {
        /// Action that requested the planned value.
        action: Action,
    },
    /// Value derived from an existing indirect object.
    DerivedFromObject {
        /// Source object for the derived value.
        object: IndirectRef,
    },
    /// Value supplied by a named external policy.
    ExternalPolicy {
        /// Stable policy name.
        name: String,
    },
}

/// Plan report-only [`MutationBoundary::DictionaryEntry`] records for a
/// `SetPageBox` action against one leaf page dictionary.
///
/// This is a planning contract only: it emits mutation-boundary metadata and
/// never calls `presslint-write`, reads PDF bytes, or mutates a document. A
/// boundary is emitted for `MediaBox` and/or `CropBox` only when the action
/// requests that box *and* a value locator is supplied for it. `MediaBox`
/// always precedes `CropBox` in the returned order. Boundary keys are stored
/// as slashless [`PdfName`] bytes; slash-prefixed names are PDF syntax only.
///
/// Each supplied [`DictionaryValueLocator`] determines the operation:
/// [`DictionaryValueLocator::ExistingValue`] plans a
/// [`DictionaryEntryOp::Replace`], and [`DictionaryValueLocator::InsertionPoint`]
/// plans a [`DictionaryEntryOp::Insert`]. Every boundary carries the caller's
/// proven ownership decision and records the value provenance as the requesting
/// `SetPageBox` action.
#[must_use]
pub fn plan_set_page_box_boundaries(
    action: &SetPageBox,
    target: IndirectRef,
    ownership: &IndirectObjectEditDecision,
    media_locator: Option<DictionaryValueLocator>,
    crop_locator: Option<DictionaryValueLocator>,
) -> Vec<MutationBoundary> {
    let mut boundaries = Vec::new();
    if action.media_box.is_some() {
        if let Some(locator) = media_locator {
            boundaries.push(dictionary_entry_boundary(
                target,
                MEDIA_BOX_KEY,
                locator,
                ownership,
                action,
            ));
        }
    }
    if action.crop_box.is_some() {
        if let Some(locator) = crop_locator {
            boundaries.push(dictionary_entry_boundary(
                target,
                CROP_BOX_KEY,
                locator,
                ownership,
                action,
            ));
        }
    }
    boundaries
}

/// Build one dictionary-entry boundary, deriving the operation from the locator.
fn dictionary_entry_boundary(
    target: IndirectRef,
    key: &[u8],
    locator: DictionaryValueLocator,
    ownership: &IndirectObjectEditDecision,
    action: &SetPageBox,
) -> MutationBoundary {
    let op = match locator {
        DictionaryValueLocator::ExistingValue { .. } => DictionaryEntryOp::Replace,
        DictionaryValueLocator::InsertionPoint { .. } => DictionaryEntryOp::Insert,
    };
    MutationBoundary::DictionaryEntry {
        target,
        key: PdfName(key.to_vec()),
        op,
        value_locator: locator,
        ownership: ownership.clone(),
        value_provenance: PlannedValueProvenance::ActionGenerated {
            action: Action::SetPageBox(*action),
        },
    }
}

/// Allocation plan for a future indirect-object clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlannedObjectAllocation {
    /// Append a new object with a known object number.
    AppendNew {
        /// Allocated object number.
        object_number: u32,
    },
    /// Allocation is deferred to a later planning phase.
    Deferred,
}
