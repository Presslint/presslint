//! Private allocation-floor proof for deterministic fresh-object reservation.
//!
//! This module holds the mechanics behind [`crate::reserve_fresh_object_references`]
//! and [`crate::write_incremental_revision_with_fresh_objects`]: a complete,
//! bounded scan of the effective newest-wins object set that proves a
//! collision-free floor for a caller's fresh-object reservation. It is not a
//! second public domain abstraction — every item here is a mechanic consumed
//! by the two public entry points in [`crate::writer`].
//!
//! The floor is the smallest object number at least one greater than every
//! identity that could collide: the whole-`/Prev`-chain effective `/Size`,
//! every effective xref entry object number (including high free entries and,
//! for xref streams, compressed-member container numbers and each observed
//! section's own header number), every indirect-reference target in the
//! active trailer/xref-stream dictionary, and every indirect-reference target
//! in every effective live object body — including xref-defined but
//! unreachable/unreferenced objects and compressed members. Any incomplete
//! proof (an unresolvable object, a body-reference scan that cannot prove
//! completeness, a reserved/future xref entry type, or a bounded cumulative
//! cap) fails the whole computation closed rather than returning a partial
//! ceiling.

use std::ops::Range;

use presslint_pdf::{
    ClassicXrefChain, ClassicXrefEntryState, FlateDecodeStreamRejection, IndirectRef,
    ObjectBodyReferencesInspection, ObjectLookup, ObjectResolutionError, ObjectResolutionRejection,
    ObjectStreamMemberExtractionRejection, ResolvedObjectData, XrefStreamChain,
    XrefStreamEntryRecord, XrefStreamTrailerInspection, inspect_indirect_object_header,
    inspect_object_body_references_resolved, resolve_object, scan_indirect_references_in_span,
};

use crate::writer::WriteError;

/// Cumulative budget for indirect-reference targets accepted while proving
/// one fresh-object reservation floor, across the trailer/dictionary scan and
/// every effective live object body.
///
/// This is not a hard cap on scanner work: each delegated body/span scan may
/// first discover up to `presslint_pdf::MAX_OBJECT_BODY_REFERENCES` (65,536)
/// targets. If 4,096 targets were accepted by earlier scans, the rejecting
/// scan may therefore bring worst-case cumulative valid-reference discovery
/// to 69,632 before this budget fails the proof closed.
pub const MAX_FRESH_FLOOR_REFERENCES: usize = 4096;

/// Writer-local cumulative cap, in decoded bytes, on repeated compressed
/// object-stream container decoding while proving one fresh-object
/// reservation floor. Also used as the per-call decode bound passed to
/// [`resolve_object`], so no single call can itself exceed the cumulative
/// cap. Compressed members are never cached across calls (mirroring
/// [`resolve_object`]'s own no-cache contract), so two members sharing one
/// container each pay the container's full decode cost; this cap makes that
/// amplification bounded and counted rather than assumed away.
pub const MAX_FRESH_FLOOR_DECODE_WORK_BYTES: usize = 1_048_576;

/// Prove the smallest collision-free classic fresh-object reservation floor
/// for the currently active newest-wins chain.
///
/// # Errors
///
/// Returns [`WriteError`] when a live object body cannot be resolved, a body
/// or trailer reference scan cannot prove completeness (an out-of-range
/// reference shape or a per-body truncation), or the cumulative accepted-
/// reference budget is exceeded. Classic tables never carry compressed
/// entries, so the cumulative decode-work cap can never trigger on this path.
/// Never returns a partial floor.
pub fn compute_classic_fresh_floor(
    input: &[u8],
    chain: &ClassicXrefChain,
    trailer_dictionary_range: Range<usize>,
) -> Result<u64, WriteError> {
    let mut acc = FloorAccumulator::new();
    acc.raise_at_least(chain.effective_size as u64);
    for entry in &chain.entries {
        acc.raise_after(u64::from(entry.object_number));
    }

    absorb_trailer_references(input, &mut acc, trailer_dictionary_range)?;

    for entry in &chain.entries {
        if entry.state != ClassicXrefEntryState::InUse {
            continue;
        }
        let reference = IndirectRef {
            object_number: entry.object_number,
            generation: entry.generation,
        };
        let resolved = resolve_for_floor(
            input,
            ObjectLookup::ClassicXrefChain(chain),
            reference,
            &acc,
        )?;
        absorb_resolved_object(input, &mut acc, reference, &resolved)?;
    }

    Ok(acc.floor)
}

/// Prove the smallest collision-free xref-stream fresh-object reservation
/// floor for the currently active newest-wins chain.
///
/// Beyond the classic proof's shape, this additionally raises the floor above
/// every type-2 entry's object-stream container number and every observed
/// section's own indirect-object header number (proving self-identity even
/// for a defective section whose entry map omits itself), and refuses on any
/// reserved/future xref-stream entry type it cannot safely interpret.
///
/// # Errors
///
/// Returns [`WriteError`] for the same reasons as
/// [`compute_classic_fresh_floor`], plus a reserved/future entry type, a
/// section self-header that cannot be parsed, a numeric conversion overflow
/// while rebuilding a decoded entry's [`IndirectRef`], or the cumulative
/// compressed-container decode-work cap. Never returns a partial floor.
pub fn compute_xref_stream_fresh_floor(
    input: &[u8],
    chain: &XrefStreamChain,
    active: &XrefStreamTrailerInspection,
) -> Result<u64, WriteError> {
    let mut acc = FloorAccumulator::new();
    acc.raise_at_least(chain.effective_size as u64);
    for entry in &chain.entries {
        acc.raise_after(entry.object_number as u64);
        if let XrefStreamEntryRecord::Compressed {
            object_stream_number,
            ..
        } = entry.record
        {
            acc.raise_after(object_stream_number as u64);
        }
    }

    for &section_byte_offset in &chain.section_byte_offsets {
        let header =
            inspect_indirect_object_header(input, section_byte_offset).map_err(|error| {
                WriteError::FreshFloorSectionHeader {
                    byte_offset: section_byte_offset,
                    error: Box::new(error),
                }
            })?;
        acc.raise_after(u64::from(header.reference.object_number));
    }

    let object_dictionary = &active.xref_stream_dictionary.object_dictionary;
    absorb_trailer_references(
        input,
        &mut acc,
        object_dictionary.dictionary_open_byte_offset
            ..object_dictionary.after_dictionary_close_byte_offset,
    )?;

    for entry in &chain.entries {
        let reference = match entry.record {
            XrefStreamEntryRecord::Free { .. } => continue,
            XrefStreamEntryRecord::Reserved { entry_type, .. } => {
                return Err(WriteError::FreshFloorReservedEntry {
                    object_number: entry.object_number,
                    entry_type,
                });
            }
            XrefStreamEntryRecord::Uncompressed { generation, .. } => {
                let object_number = u32::try_from(entry.object_number)
                    .map_err(|_| WriteError::FreshFloorNumericOverflow)?;
                let generation =
                    u16::try_from(generation).map_err(|_| WriteError::FreshFloorNumericOverflow)?;
                IndirectRef {
                    object_number,
                    generation,
                }
            }
            XrefStreamEntryRecord::Compressed { .. } => {
                let object_number = u32::try_from(entry.object_number)
                    .map_err(|_| WriteError::FreshFloorNumericOverflow)?;
                IndirectRef {
                    object_number,
                    generation: 0,
                }
            }
        };

        let resolved =
            resolve_for_floor(input, ObjectLookup::XrefStreamChain(chain), reference, &acc)?;
        absorb_resolved_object(input, &mut acc, reference, &resolved)?;
    }

    Ok(acc.floor)
}

/// Bounded floor/cap state threaded through one reservation-floor proof.
struct FloorAccumulator {
    floor: u64,
    reference_budget: usize,
    decode_work_budget: usize,
}

impl FloorAccumulator {
    const fn new() -> Self {
        Self {
            floor: 0,
            reference_budget: MAX_FRESH_FLOOR_REFERENCES,
            decode_work_budget: MAX_FRESH_FLOOR_DECODE_WORK_BYTES,
        }
    }

    /// Raise the floor to one past `object_number` (the identity is occupied).
    fn raise_after(&mut self, object_number: u64) {
        self.floor = self.floor.max(object_number.saturating_add(1));
    }

    /// Raise the floor to at least `value` directly (already "one past" by
    /// construction, e.g. an effective `/Size`).
    fn raise_at_least(&mut self, value: u64) {
        self.floor = self.floor.max(value);
    }

    fn record_references(&mut self, count: usize) -> Result<(), WriteError> {
        self.reference_budget = self.reference_budget.checked_sub(count).ok_or(
            WriteError::FreshFloorReferenceCapExceeded {
                max_references: MAX_FRESH_FLOOR_REFERENCES,
            },
        )?;
        Ok(())
    }

    fn record_decode_work(&mut self, bytes: usize) -> Result<(), WriteError> {
        self.decode_work_budget = self.decode_work_budget.checked_sub(bytes).ok_or(
            WriteError::FreshFloorDecodeWorkCapExceeded {
                max_decoded_bytes: MAX_FRESH_FLOOR_DECODE_WORK_BYTES,
            },
        )?;
        Ok(())
    }
}

/// Resolve one reference through `lookup`, bounding any compressed-member
/// container decode by the *remaining* cumulative decode-work budget rather
/// than the whole-cap constant.
///
/// Passing the remaining budget (not [`MAX_FRESH_FLOOR_DECODE_WORK_BYTES`])
/// as the per-call bound makes a second, third, or later compressed-member
/// decode stop the moment it would exceed the cumulative cap, instead of
/// paying the full decode cost of each container and only discovering the
/// overage afterward in [`FloorAccumulator::record_decode_work`]. A rejection
/// caused by that reduced bound is reported as
/// [`WriteError::FreshFloorDecodeWorkCapExceeded`], matching what the
/// eventual [`FloorAccumulator::record_decode_work`] call would have reported
/// for the same input had the decode been allowed to complete unbounded.
fn resolve_for_floor(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    reference: IndirectRef,
    acc: &FloorAccumulator,
) -> Result<ResolvedObjectData, WriteError> {
    resolve_object(input, lookup, reference, acc.decode_work_budget).map_err(|error| {
        if is_decode_work_limit_rejection(&error) {
            WriteError::FreshFloorDecodeWorkCapExceeded {
                max_decoded_bytes: MAX_FRESH_FLOOR_DECODE_WORK_BYTES,
            }
        } else {
            WriteError::FreshFloorResolution {
                reference,
                error: Box::new(error),
            }
        }
    })
}

/// True when `error` is exactly the compressed-member decode rejection caused
/// by hitting the caller-supplied decode-byte bound (an unfiltered body over
/// the limit, or a Flate inflate stopped by its output limit) rather than a
/// genuinely malformed object stream.
const fn is_decode_work_limit_rejection(error: &ObjectResolutionError) -> bool {
    matches!(
        error.reason,
        ObjectResolutionRejection::ObjectStreamMemberExtraction {
            extraction_reason: ObjectStreamMemberExtractionRejection::DecodedObjectStreamTooLarge { .. }
                | ObjectStreamMemberExtractionRejection::FlateDecode {
                    flate_reason: FlateDecodeStreamRejection::OutputLimitExceeded,
                },
        }
    )
}

/// Absorb one resolved live object's body references into the accumulator.
///
/// The compressed-member decode itself is already bounded by the remaining
/// decode-work budget in [`resolve_for_floor`], so this only records the
/// actual decoded byte count against that budget; it can never observe more
/// than the budget it was given.
fn absorb_resolved_object(
    input: &[u8],
    acc: &mut FloorAccumulator,
    reference: IndirectRef,
    resolved: &ResolvedObjectData,
) -> Result<(), WriteError> {
    if let ResolvedObjectData::Compressed {
        decoded_object_stream,
        ..
    } = resolved
    {
        acc.record_decode_work(decoded_object_stream.len())?;
    }
    let inspection = inspect_object_body_references_resolved(input, resolved).map_err(|error| {
        WriteError::FreshFloorBodyReferences {
            reference,
            error: Box::new(error),
        }
    })?;
    absorb_object_references(acc, reference, &inspection)
}

/// Absorb one object body's discovered references, failing closed when the
/// scan could not prove completeness (a skipped out-of-range reference shape
/// or a per-body truncation).
fn absorb_object_references(
    acc: &mut FloorAccumulator,
    reference: IndirectRef,
    inspection: &ObjectBodyReferencesInspection,
) -> Result<(), WriteError> {
    if inspection.truncation.is_some() || !inspection.skipped_references.is_empty() {
        return Err(WriteError::FreshFloorObjectReferencesIncomplete { reference });
    }
    acc.record_references(inspection.references.len())?;
    for found in &inspection.references {
        acc.raise_after(u64::from(found.object_number));
    }
    Ok(())
}

/// Scan one trailer/xref-stream dictionary span for indirect-reference
/// targets, failing closed under the same completeness rule as
/// [`absorb_object_references`].
fn absorb_trailer_references(
    input: &[u8],
    acc: &mut FloorAccumulator,
    span: Range<usize>,
) -> Result<(), WriteError> {
    let inspection = scan_indirect_references_in_span(input, span);
    if inspection.truncation.is_some() || !inspection.skipped_references.is_empty() {
        return Err(WriteError::FreshFloorTrailerReferencesIncomplete);
    }
    acc.record_references(inspection.references.len())?;
    for found in &inspection.references {
        acc.raise_after(u64::from(found.object_number));
    }
    Ok(())
}
