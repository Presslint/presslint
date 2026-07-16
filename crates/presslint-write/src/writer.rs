use std::ops::Range;

use serde::{Deserialize, Serialize};

use presslint_pdf::{
    ClassicXrefChain, ClassicXrefChainError, ClassicXrefTableInspectionError,
    ClassicXrefTrailerDictionaryInspectionError, DictionaryEntryInspectionError,
    IndirectObjectHeaderInspectionError, IndirectRef, MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    ObjectBodyReferencesInspectionError, ObjectLookup, ObjectResolutionError,
    ObjectResolutionRejection, PdfSourceInspectionError, XrefSection, XrefStreamChainError,
    XrefStreamTrailerInspectionError, build_classic_xref_chain, build_xref_stream_chain,
    inspect_classic_xref_table, inspect_classic_xref_trailer_dictionary,
    inspect_dictionary_entries, inspect_pdf_source, inspect_xref_stream_trailer,
    resolve_xref_object_offset,
};

use crate::fresh_objects::{compute_classic_fresh_floor, compute_xref_stream_fresh_floor};
use crate::xref_stream_writer::{
    scan_active_xref_stream, write_xref_stream_incremental_revision,
    write_xref_stream_incremental_revision_with_fresh,
};

const ENCRYPT_KEY: &[u8] = b"/Encrypt";
const ID_KEY: &[u8] = b"/ID";
const INFO_KEY: &[u8] = b"/Info";
const XREF_STM_KEY: &[u8] = b"/XRefStm";

/// Largest byte offset that fits the fixed 10-digit classic xref offset field.
///
/// Classic cross-reference entries write the object offset as exactly ten
/// decimal digits (see ISO 32000-1 §7.5.4). An appended object offset above this
/// value cannot be represented and is rejected rather than silently truncated.
const MAX_CLASSIC_XREF_OFFSET: usize = 9_999_999_999;

/// Total byte width of one fixed classic cross-reference entry line.
const XREF_ENTRY_WIDTH: usize = 20;

/// One existing indirect object to rewrite in the appended revision.
///
/// `body_bytes` is the replacement object *body* only: the bytes between the
/// `N G obj` header and the closing `endobj`, exactly as the caller wants them
/// serialized. The writer wraps them in an indirect-object header/footer but
/// never inspects, decodes, or edits them, so a byte-identical body yields a
/// semantic no-op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtyObjectBytes {
    /// Indirect reference of the existing uncompressed object to rewrite.
    pub reference: IndirectRef,
    /// Replacement object body bytes (no header, no `endobj`).
    pub body_bytes: Vec<u8>,
}

/// One new, uncompressed, generation-zero object to append at a proved fresh
/// identity.
///
/// `FreshObjectBytes` mirrors [`DirtyObjectBytes`] structurally but carries
/// the opposite identity contract: `reference` must be one of the exact
/// contiguous identities [`reserve_fresh_object_references`] proves for the
/// current input, never an existing object number. `body_bytes` is the new
/// object *body* only, wrapped in a header and `endobj` exactly like a dirty
/// rewrite; it is never inspected, decoded, or edited beyond the bounded
/// reference scan [`write_incremental_revision_with_fresh_objects`] performs
/// to keep the xref-stream backend's own self-object number out of every
/// reference the caller writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreshObjectBytes {
    /// Reserved fresh generation-zero indirect reference.
    pub reference: IndirectRef,
    /// New object body bytes (no header, no `endobj`).
    pub body_bytes: Vec<u8>,
}

/// Error returned when an incremental append cannot be produced.
///
/// Each variant names the stage that stopped and preserves the delegated
/// [`presslint_pdf`] failure verbatim where one applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum WriteError {
    /// The input could not be identified as a PDF source.
    Source {
        /// Delegated source-inspection failure.
        error: PdfSourceInspectionError,
    },
    /// No final `startxref` record could be located in the trailing window.
    StartXrefUnavailable,
    /// The cross-reference section at the final `startxref` offset could not be
    /// classified as a table or a stream.
    XrefSectionUnclassified,
    /// The xref-stream `/Prev` chain could not be built.
    XrefStreamChain {
        /// Delegated xref-stream chain-building failure.
        error: Box<XrefStreamChainError>,
    },
    /// The active xref-stream dictionary could not be scanned for trailer-style
    /// values carried into the appended stream dictionary.
    ActiveXrefStream {
        /// Delegated xref-stream trailer inspection failure.
        error: XrefStreamTrailerInspectionError,
    },
    /// The newest classic cross-reference table could not be inspected.
    XrefTable {
        /// Delegated classic xref table inspection failure.
        error: ClassicXrefTableInspectionError,
    },
    /// The active trailer dictionary could not be scanned for `/Encrypt`,
    /// `/XRefStm`, and `/ID`.
    ActiveTrailer {
        /// Delegated trailer-scan failure.
        error: ActiveTrailerError,
    },
    /// The active trailer dictionary declares `/Encrypt`. Encryption
    /// preservation is out of scope for this slice.
    EncryptedInput,
    /// The active classic trailer declares `/XRefStm`, making the input a
    /// hybrid-reference file. This slice follows classic xref chains only and
    /// does not merge supplemental xref-stream entries.
    HybridXrefStmInput,
    /// The classic cross-reference `/Prev` chain could not be built.
    ClassicXrefChain {
        /// Delegated classic chain-building failure.
        error: Box<ClassicXrefChainError>,
    },
    /// Two dirty objects share the same object number.
    DuplicateDirtyObject {
        /// The repeated object number.
        object_number: u32,
    },
    /// A dirty object does not resolve to an existing in-use uncompressed object
    /// in the classic chain, so it cannot be no-op replaced.
    DirtyObjectNotInUse {
        /// The unresolved dirty reference.
        reference: IndirectRef,
    },
    /// A dirty object's generation does not match the existing in-use object.
    GenerationMismatch {
        /// The dirty object number.
        object_number: u32,
        /// Generation recorded by the classic chain for this object.
        expected: u16,
        /// Generation supplied by the caller.
        found: u16,
    },
    /// A dirty object's newest-wins cross-reference offset does not point at a
    /// matching indirect object header, so the existing object cannot be located
    /// for byte-for-byte no-op replacement.
    DirtyObjectHeaderMismatch {
        /// The dirty reference whose resolved offset failed header validation.
        reference: IndirectRef,
        /// Delegated object-resolution failure carrying the resolved offset and
        /// the structured header rejection reason.
        error: Box<ObjectResolutionError>,
    },
    /// A dirty object resolves to a type-2 compressed object-stream member.
    CompressedDirtyObject {
        /// The dirty reference whose newest xref entry is compressed.
        reference: IndirectRef,
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Member index inside the containing object stream.
        index_within_object_stream: usize,
    },
    /// A dirty object resolves to a reserved or future xref-stream entry type.
    ReservedDirtyObject {
        /// The dirty reference whose newest xref entry is reserved.
        reference: IndirectRef,
        /// Raw xref-stream entry type.
        entry_type: u64,
    },
    /// An appended object offset does not fit the fixed 10-digit classic xref
    /// offset field.
    XrefOffsetTooLarge {
        /// The offending byte offset.
        byte_offset: usize,
    },
    /// The xref-stream backend could not allocate the appended xref-stream
    /// object number inside the public indirect-reference range.
    XrefStreamObjectNumberTooLarge {
        /// Highest existing or newly requested object number that overflowed.
        object_number: usize,
    },
    /// A fresh object declares a nonzero generation. Fresh objects are always
    /// newly allocated generation-zero identities.
    NonZeroFreshGeneration {
        /// The offending fresh reference.
        reference: IndirectRef,
    },
    /// Two fresh objects share the same object number.
    DuplicateFreshObject {
        /// The repeated object number.
        object_number: u32,
    },
    /// A dirty object and a fresh object share the same object number.
    FreshDirtyObjectCollision {
        /// The colliding object number.
        object_number: u32,
    },
    /// The supplied fresh references are not exactly ascending, contiguous,
    /// generation-zero identities.
    FreshReservationNotContiguous {
        /// The earlier reference in sorted order.
        previous: IndirectRef,
        /// The following reference that did not continue the sequence.
        next: IndirectRef,
    },
    /// The supplied fresh references do not begin exactly at the freshly
    /// recomputed collision-free floor for the current input bytes. A
    /// reservation computed for different input bytes, or any hand-picked
    /// first number, is rejected rather than trusted as a capability token.
    FreshReservationFloorMismatch {
        /// The floor recomputed for the current input.
        expected_floor: u32,
        /// The first object number the caller actually supplied.
        found_first: u32,
    },
    /// Reservation count/object-number arithmetic does not fit the public
    /// `u32` object-number field.
    FreshReservationNumberOverflow,
    /// A numeric conversion inside the allocation-floor proof (a decoded
    /// object or generation number, or a checked-add) did not fit its target
    /// type.
    FreshFloorNumericOverflow,
    /// The allocation-floor proof could not resolve an effective live object
    /// body.
    FreshFloorResolution {
        /// The unresolved reference.
        reference: IndirectRef,
        /// Delegated object-resolution failure.
        error: Box<ObjectResolutionError>,
    },
    /// The allocation-floor proof could not scan an effective live object
    /// body for indirect references. Also raised when scanning a newly
    /// appended dirty/fresh body while placing the xref-stream self-object.
    FreshFloorBodyReferences {
        /// The object whose body could not be scanned.
        reference: IndirectRef,
        /// Delegated object-body-reference inspection failure.
        error: Box<ObjectBodyReferencesInspectionError>,
    },
    /// The active trailer/xref-stream dictionary reference scan could not
    /// prove completeness (an out-of-range reference shape or a truncation).
    FreshFloorTrailerReferencesIncomplete,
    /// An object body's reference scan could not prove completeness (an
    /// out-of-range reference shape or a truncation). Also raised when
    /// scanning a newly appended dirty/fresh body while placing the
    /// xref-stream self-object.
    FreshFloorObjectReferencesIncomplete {
        /// The object whose body scan was incomplete.
        reference: IndirectRef,
    },
    /// The allocation-floor proof reached a reserved/future xref-stream
    /// entry type whose semantics cannot be proven complete.
    FreshFloorReservedEntry {
        /// The reserved entry's object number.
        object_number: usize,
        /// Raw type field value.
        entry_type: u64,
    },
    /// The allocation-floor proof's cumulative accepted-reference budget was
    /// exceeded. A rejecting delegated scan may already have discovered up to
    /// its separate 65,536-reference per-scan cap.
    FreshFloorReferenceCapExceeded {
        /// Configured cumulative accepted-reference budget.
        max_references: usize,
    },
    /// The allocation-floor proof's cumulative compressed-container
    /// decode-work cap was exceeded.
    FreshFloorDecodeWorkCapExceeded {
        /// Configured cumulative decode-work byte cap.
        max_decoded_bytes: usize,
    },
    /// The allocation-floor proof could not parse an xref-stream section's
    /// own object header while proving self-object identity.
    FreshFloorSectionHeader {
        /// The section byte offset whose header failed to parse.
        byte_offset: usize,
        /// Delegated object-header inspection failure.
        error: Box<IndirectObjectHeaderInspectionError>,
    },
    /// The xref-stream self-object number could not be allocated above every
    /// fresh reservation and every reference target discovered in newly
    /// supplied bodies.
    FreshXrefSelfObjectOverflow,
}

/// Append an incremental revision that no-op rewrites existing uncompressed
/// objects with caller-supplied body bytes.
///
/// The output is `input` copied verbatim, followed by one appended revision.
/// Classic-table inputs keep the classic table/trailer append path; xref-stream
/// inputs append a raw `/Type /XRef` stream revision. Both paths preserve `/Root`
/// and optional `/ID`/`/Info`, set `/Prev` to the previous `startxref` target,
/// and set `/Size` from the whole `/Prev` chain. Therefore
/// `output[..input.len()] == input`.
///
/// Dirty objects are sorted deterministically by indirect reference, so the
/// output is independent of the caller's ordering. The only object bytes written
/// are the caller-provided bodies wrapped in a header and `endobj`; no semantic
/// edit is synthesized.
///
/// # Errors
///
/// Returns [`WriteError`] when the input is not a supported append source
/// (missing header/`startxref` or unclassifiable section), the active trailer
/// declares `/Encrypt`, a classic trailer declares `/XRefStm`, the selected
/// `/Prev` chain cannot be built, dirty object numbers duplicate, a dirty object
/// does not resolve to an existing in-use uncompressed object of matching
/// generation, or an appended classic offset exceeds the fixed 10-digit field.
pub fn write_incremental_revision(
    input: &[u8],
    dirty_objects: &[DirtyObjectBytes],
) -> Result<Vec<u8>, WriteError> {
    let source = inspect_pdf_source(input).map_err(|error| WriteError::Source { error })?;
    let startxref = source.startxref.ok_or(WriteError::StartXrefUnavailable)?;

    match source.xref_section {
        Some(XrefSection::Table) => {
            write_classic_incremental_revision(input, dirty_objects, startxref.byte_offset)
        }
        Some(XrefSection::Stream { .. }) => {
            write_xref_stream_incremental_revision(input, dirty_objects, startxref.byte_offset)
        }
        None => Err(WriteError::XrefSectionUnclassified),
    }
}

/// Reserve `count` exact contiguous ascending generation-zero fresh
/// [`IndirectRef`]s for `input`.
///
/// The reservation is a complete, bounded proof over the effective
/// newest-wins object set (see the [`crate::fresh_objects`] module doc): a
/// floor at least one greater than every identity that could collide,
/// including dangling references in trailers, unreferenced bodies, and
/// compressed members. It is not a capability token — the same input bytes
/// must still be supplied unchanged to
/// [`write_incremental_revision_with_fresh_objects`], which independently
/// recomputes and re-validates the floor before assembling output.
///
/// `count == 0` returns an empty vector without any whole-document
/// allocation scan.
///
/// Reference discovery uses a two-tier bound: at most 4,096 references are
/// accepted cumulatively, while each delegated scan may first discover up to
/// 65,536. Thus a rejecting scan can bring worst-case cumulative valid-
/// reference discovery to 69,632 before the proof refuses.
///
/// # Errors
///
/// Returns [`WriteError`] when the input is not a supported append source,
/// the active trailer/xref-stream dictionary declares `/Encrypt` or a classic
/// `/XRefStm` hybrid reference (the same retained safety boundary as
/// [`write_incremental_revision`]), the allocation-floor proof cannot
/// complete (see [`compute_classic_fresh_floor`] and
/// [`compute_xref_stream_fresh_floor`]), or when `count` does not fit
/// alongside the proved floor inside the public `u32` object-number field.
pub fn reserve_fresh_object_references(
    input: &[u8],
    count: usize,
) -> Result<Vec<IndirectRef>, WriteError> {
    if count == 0 {
        return Ok(Vec::new());
    }

    let source = inspect_pdf_source(input).map_err(|error| WriteError::Source { error })?;
    let startxref = source.startxref.ok_or(WriteError::StartXrefUnavailable)?;

    let floor = match source.xref_section {
        Some(XrefSection::Table) => {
            let newest_table = inspect_classic_xref_table(input, startxref.byte_offset)
                .map_err(|error| WriteError::XrefTable { error })?;
            let (_, trailer_dictionary_range) =
                scan_active_trailer(input, newest_table.trailer_byte_offset)?;
            let chain =
                build_classic_xref_chain(input, startxref.byte_offset).map_err(|error| {
                    WriteError::ClassicXrefChain {
                        error: Box::new(error),
                    }
                })?;
            compute_classic_fresh_floor(input, &chain, trailer_dictionary_range)?
        }
        Some(XrefSection::Stream { .. }) => {
            let active = inspect_xref_stream_trailer(input, startxref.byte_offset)
                .map_err(|error| WriteError::ActiveXrefStream { error })?;
            scan_active_xref_stream(input, &active)?;
            let chain = build_xref_stream_chain(
                input,
                startxref.byte_offset,
                MAX_XREF_STREAM_SECTION_DECODED_BYTES,
            )
            .map_err(|error| WriteError::XrefStreamChain {
                error: Box::new(error),
            })?;
            compute_xref_stream_fresh_floor(input, &chain, &active)?
        }
        None => return Err(WriteError::XrefSectionUnclassified),
    };

    build_reservation(floor, count)
}

/// Append an incremental revision that no-op rewrites existing uncompressed
/// objects and appends caller-supplied fresh generation-zero objects at
/// exactly reserved identities.
///
/// `fresh_objects=[]` enters the pre-existing [`write_incremental_revision`]
/// backend paths directly: no allocation scan, no new error, and byte-for-byte
/// identical output. A nonempty `fresh_objects` recomputes the collision-free
/// floor for `input` (see [`reserve_fresh_object_references`]), requires the
/// caller's sorted fresh references to match that floor exactly, validates
/// dirty objects through the unchanged existing-object path, and rejects any
/// dirty/fresh object-number collision. Dirty and fresh objects are merged and
/// emitted in deterministic ascending-reference order regardless of caller
/// order.
///
/// # Errors
///
/// Returns [`WriteError`] for every [`write_incremental_revision`] failure,
/// plus a nonzero fresh generation, a duplicate fresh object number, a
/// gapped/noncontiguous fresh reservation, a floor mismatch, a dirty/fresh
/// collision, or any allocation-floor proof failure.
pub fn write_incremental_revision_with_fresh_objects(
    input: &[u8],
    dirty_objects: &[DirtyObjectBytes],
    fresh_objects: &[FreshObjectBytes],
) -> Result<Vec<u8>, WriteError> {
    if fresh_objects.is_empty() {
        return write_incremental_revision(input, dirty_objects);
    }

    let source = inspect_pdf_source(input).map_err(|error| WriteError::Source { error })?;
    let startxref = source.startxref.ok_or(WriteError::StartXrefUnavailable)?;

    match source.xref_section {
        Some(XrefSection::Table) => write_classic_incremental_revision_with_fresh(
            input,
            dirty_objects,
            fresh_objects,
            startxref.byte_offset,
        ),
        Some(XrefSection::Stream { .. }) => write_xref_stream_incremental_revision_with_fresh(
            input,
            dirty_objects,
            fresh_objects,
            startxref.byte_offset,
        ),
        None => Err(WriteError::XrefSectionUnclassified),
    }
}

pub fn write_classic_incremental_revision(
    input: &[u8],
    dirty_objects: &[DirtyObjectBytes],
    startxref_byte_offset: usize,
) -> Result<Vec<u8>, WriteError> {
    let newest_table = inspect_classic_xref_table(input, startxref_byte_offset)
        .map_err(|error| WriteError::XrefTable { error })?;
    let (active_trailer, _) = scan_active_trailer(input, newest_table.trailer_byte_offset)?;

    let chain = build_classic_xref_chain(input, startxref_byte_offset).map_err(|error| {
        WriteError::ClassicXrefChain {
            error: Box::new(error),
        }
    })?;

    let ordered = order_dirty_objects(dirty_objects)?;
    validate_dirty_objects(input, &chain, &ordered)?;

    let mut writer = AppendRevisionWriter::new(input, dirty_objects);
    writer.ensure_leading_eol();

    let mut records = Vec::with_capacity(ordered.len());
    for dirty in &ordered {
        let byte_offset = writer.append_object(dirty.reference, &dirty.body_bytes);
        records.push(AppendedEntry {
            object_number: dirty.reference.object_number,
            generation: dirty.reference.generation,
            byte_offset,
        });
    }

    let xref_byte_offset = writer.len();
    writer.push_xref_table(&records)?;
    writer.push_trailer(
        classic_effective_size(&chain),
        chain.root_reference,
        startxref_byte_offset,
        active_trailer.id_bytes(input),
        active_trailer.info_bytes(input),
    );
    writer.push_startxref(xref_byte_offset);

    Ok(writer.finish())
}

/// Classic backend for [`write_incremental_revision_with_fresh_objects`].
///
/// Recomputes the collision-free floor for `input`, validates dirty objects
/// through the unchanged existing-object path, validates the fresh
/// reservation against the recomputed floor, and emits dirty and fresh
/// objects merged in deterministic ascending-reference order. Classic tables
/// have no self-object concept, so there is nothing analogous to the
/// xref-stream backend's self-object avoidance scan.
fn write_classic_incremental_revision_with_fresh(
    input: &[u8],
    dirty_objects: &[DirtyObjectBytes],
    fresh_objects: &[FreshObjectBytes],
    startxref_byte_offset: usize,
) -> Result<Vec<u8>, WriteError> {
    let newest_table = inspect_classic_xref_table(input, startxref_byte_offset)
        .map_err(|error| WriteError::XrefTable { error })?;
    let (active_trailer, trailer_dictionary_range) =
        scan_active_trailer(input, newest_table.trailer_byte_offset)?;

    let chain = build_classic_xref_chain(input, startxref_byte_offset).map_err(|error| {
        WriteError::ClassicXrefChain {
            error: Box::new(error),
        }
    })?;

    let floor = compute_classic_fresh_floor(input, &chain, trailer_dictionary_range)?;

    let ordered_dirty = order_dirty_objects(dirty_objects)?;
    validate_dirty_objects(input, &chain, &ordered_dirty)?;

    let ordered_fresh = order_fresh_objects(fresh_objects)?;
    check_fresh_dirty_collision(&ordered_dirty, &ordered_fresh)?;
    validate_fresh_reservation(floor, &ordered_fresh)?;

    let combined = combine_dirty_and_fresh(&ordered_dirty, &ordered_fresh);
    let body_len_total: usize = combined.iter().map(|(_, body)| body.len()).sum();
    let mut writer =
        AppendRevisionWriter::with_capacity_estimate(input, body_len_total, combined.len());
    writer.ensure_leading_eol();

    let mut records = Vec::with_capacity(combined.len());
    for (reference, body) in &combined {
        let byte_offset = writer.append_object(*reference, body);
        records.push(AppendedEntry {
            object_number: reference.object_number,
            generation: reference.generation,
            byte_offset,
        });
    }

    let xref_byte_offset = writer.len();
    writer.push_xref_table(&records)?;
    let size = fresh_augmented_size(floor, ordered_fresh.len())?;
    writer.push_trailer(
        size,
        chain.root_reference,
        startxref_byte_offset,
        active_trailer.id_bytes(input),
        active_trailer.info_bytes(input),
    );
    writer.push_startxref(xref_byte_offset);

    Ok(writer.finish())
}

/// Sort fresh objects by reference, rejecting a nonzero generation, a
/// duplicate object number, or a gap in the ascending sequence.
pub fn order_fresh_objects(
    fresh_objects: &[FreshObjectBytes],
) -> Result<Vec<&FreshObjectBytes>, WriteError> {
    let mut ordered: Vec<&FreshObjectBytes> = fresh_objects.iter().collect();
    ordered.sort_by_key(|fresh| fresh.reference);

    for fresh in &ordered {
        if fresh.reference.generation != 0 {
            return Err(WriteError::NonZeroFreshGeneration {
                reference: fresh.reference,
            });
        }
    }

    for pair in ordered.windows(2) {
        if pair[0].reference.object_number == pair[1].reference.object_number {
            return Err(WriteError::DuplicateFreshObject {
                object_number: pair[0].reference.object_number,
            });
        }
        let expected_next = pair[0]
            .reference
            .object_number
            .checked_add(1)
            .ok_or(WriteError::FreshReservationNumberOverflow)?;
        if pair[1].reference.object_number != expected_next {
            return Err(WriteError::FreshReservationNotContiguous {
                previous: pair[0].reference,
                next: pair[1].reference,
            });
        }
    }

    Ok(ordered)
}

/// Reject any object number shared between the ordered dirty and fresh sets.
pub fn check_fresh_dirty_collision(
    ordered_dirty: &[&DirtyObjectBytes],
    ordered_fresh: &[&FreshObjectBytes],
) -> Result<(), WriteError> {
    for fresh in ordered_fresh {
        if ordered_dirty
            .binary_search_by_key(&fresh.reference.object_number, |dirty| {
                dirty.reference.object_number
            })
            .is_ok()
        {
            return Err(WriteError::FreshDirtyObjectCollision {
                object_number: fresh.reference.object_number,
            });
        }
    }
    Ok(())
}

/// Require the already-contiguous, already-deduplicated, already-generation-
/// zero `ordered_fresh` to begin exactly at `floor`.
///
/// Because `ordered_fresh` is already proven internally contiguous, matching
/// only the first object number is sufficient to prove the whole sequence
/// equals the exact reservation a caller must have requested for this input.
pub fn validate_fresh_reservation(
    floor: u64,
    ordered_fresh: &[&FreshObjectBytes],
) -> Result<(), WriteError> {
    let expected_first =
        u32::try_from(floor).map_err(|_| WriteError::FreshReservationNumberOverflow)?;
    let count = u32::try_from(ordered_fresh.len())
        .map_err(|_| WriteError::FreshReservationNumberOverflow)?;
    let last_offset = count
        .checked_sub(1)
        .ok_or(WriteError::FreshReservationNumberOverflow)?;
    expected_first
        .checked_add(last_offset)
        .ok_or(WriteError::FreshReservationNumberOverflow)?;

    let found_first = ordered_fresh[0].reference.object_number;
    if found_first != expected_first {
        return Err(WriteError::FreshReservationFloorMismatch {
            expected_floor: expected_first,
            found_first,
        });
    }
    Ok(())
}

/// Build exactly `count` contiguous ascending generation-zero references
/// starting at `floor`.
fn build_reservation(floor: u64, count: usize) -> Result<Vec<IndirectRef>, WriteError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let first = u32::try_from(floor).map_err(|_| WriteError::FreshReservationNumberOverflow)?;
    let count_u32 = u32::try_from(count).map_err(|_| WriteError::FreshReservationNumberOverflow)?;
    let last_offset = count_u32
        .checked_sub(1)
        .ok_or(WriteError::FreshReservationNumberOverflow)?;
    first
        .checked_add(last_offset)
        .ok_or(WriteError::FreshReservationNumberOverflow)?;

    Ok((0..count_u32)
        .map(|offset| IndirectRef {
            object_number: first + offset,
            generation: 0,
        })
        .collect())
}

/// Concatenate validated dirty and fresh objects into ascending reference
/// order. The dirty objects resolve to existing identities, while the fresh
/// reservation starts above the entire existing object set, so every dirty
/// reference necessarily precedes every fresh reference.
pub fn combine_dirty_and_fresh<'a>(
    ordered_dirty: &[&'a DirtyObjectBytes],
    ordered_fresh: &[&'a FreshObjectBytes],
) -> Vec<(IndirectRef, &'a [u8])> {
    let mut combined: Vec<(IndirectRef, &[u8])> =
        Vec::with_capacity(ordered_dirty.len() + ordered_fresh.len());
    combined.extend(
        ordered_dirty
            .iter()
            .map(|dirty| (dirty.reference, dirty.body_bytes.as_slice())),
    );
    combined.extend(
        ordered_fresh
            .iter()
            .map(|fresh| (fresh.reference, fresh.body_bytes.as_slice())),
    );
    combined
}

/// The classic `/Size` augmented with a nonempty fresh reservation: at least
/// one past the last reserved fresh object number.
fn fresh_augmented_size(floor: u64, fresh_count: usize) -> Result<usize, WriteError> {
    let augmented = floor
        .checked_add(fresh_count as u64)
        .ok_or(WriteError::FreshReservationNumberOverflow)?;
    let augmented =
        usize::try_from(augmented).map_err(|_| WriteError::FreshReservationNumberOverflow)?;
    Ok(augmented)
}

/// Whole-`/Prev`-chain `/Size`: at least one greater than the highest object
/// number seen anywhere in the classic chain.
///
/// This intentionally spans the entire chain rather than the newest section or
/// the dirty set, matching the incremental-save `/Size` rule (a narrower value
/// is the concrete `PDFBOX-5945` pitfall). The chain's best-effort
/// `effective_size` (the maximum trailer `/Size` observed) is kept as a floor so
/// the appended value never regresses below a previously declared `/Size`.
fn classic_effective_size(chain: &ClassicXrefChain) -> usize {
    let highest_object_number = chain
        .entries
        .iter()
        .map(|entry| entry.object_number)
        .max()
        .unwrap_or(0);
    chain.effective_size.max(highest_object_number as usize + 1)
}

/// Dirty objects sorted by indirect reference, rejecting duplicate object
/// numbers.
pub fn order_dirty_objects(
    dirty_objects: &[DirtyObjectBytes],
) -> Result<Vec<&DirtyObjectBytes>, WriteError> {
    let mut ordered: Vec<&DirtyObjectBytes> = dirty_objects.iter().collect();
    ordered.sort_by_key(|dirty| dirty.reference);
    for pair in ordered.windows(2) {
        if pair[0].reference.object_number == pair[1].reference.object_number {
            return Err(WriteError::DuplicateDirtyObject {
                object_number: pair[0].reference.object_number,
            });
        }
    }
    Ok(ordered)
}

/// Prove every dirty object resolves to an existing in-use uncompressed object
/// of matching generation *and* that the resolved offset points at a matching
/// indirect object header in the classic newest-wins chain.
///
/// This delegates to [`resolve_xref_object_offset`] rather than the locate-only
/// chain lookup so a newest-wins `InUse` entry is only accepted when the object
/// header at its byte offset parses and its object/generation match the dirty
/// reference. A stale, corrupt, or mis-pointed xref entry is therefore rejected
/// before it can be shadowed by an appended object, which keeps this slice a true
/// no-op replacement of existing ordinary uncompressed objects.
fn validate_dirty_objects(
    input: &[u8],
    chain: &ClassicXrefChain,
    ordered: &[&DirtyObjectBytes],
) -> Result<(), WriteError> {
    for dirty in ordered {
        let reference = dirty.reference;
        if let Err(error) =
            resolve_xref_object_offset(input, ObjectLookup::ClassicXrefChain(chain), reference)
        {
            return Err(classify_resolution_error(reference, error));
        }
    }
    Ok(())
}

/// Map a delegated [`ObjectResolutionError`] onto the writer's dirty-object
/// rejection surface, preserving the distinct generation-mismatch and
/// not-in-use cases and folding every header-validation failure into
/// [`WriteError::DirtyObjectHeaderMismatch`].
pub fn classify_resolution_error(
    reference: IndirectRef,
    error: ObjectResolutionError,
) -> WriteError {
    match error.reason {
        ObjectResolutionRejection::UnsupportedCompressedXrefStreamEntry {
            object_stream_number,
            index_within_object_stream,
            ..
        } => WriteError::CompressedDirtyObject {
            reference,
            object_stream_number,
            index_within_object_stream,
        },
        ObjectResolutionRejection::UnsupportedReservedXrefStreamEntry { entry_type, .. } => {
            WriteError::ReservedDirtyObject {
                reference,
                entry_type,
            }
        }
        ObjectResolutionRejection::GenerationMismatch {
            requested_generation,
            xref_generation,
        } => WriteError::GenerationMismatch {
            object_number: reference.object_number,
            expected: xref_generation,
            found: requested_generation,
        },
        ObjectResolutionRejection::UnresolvedXrefLocation { .. } => {
            WriteError::DirtyObjectNotInUse { reference }
        }
        _ => WriteError::DirtyObjectHeaderMismatch {
            reference,
            error: Box::new(error),
        },
    }
}

/// Delegated failure while scanning the active trailer dictionary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ActiveTrailerError {
    /// The trailer dictionary extent could not be located.
    TrailerDictionary {
        /// Delegated trailer dictionary inspection failure.
        error: ClassicXrefTrailerDictionaryInspectionError,
    },
    /// The trailer dictionary entries could not be scanned.
    Entries {
        /// Delegated dictionary-entry inspection failure.
        error: DictionaryEntryInspectionError,
    },
}

/// Byte ranges of single top-level `/ID` and `/Info` values in the active
/// trailer dictionary.
pub struct ActiveTrailerScan {
    pub id_value_range: Option<(usize, usize)>,
    pub info_value_range: Option<(usize, usize)>,
}

impl ActiveTrailerScan {
    /// Borrow the preserved `/ID` value bytes from the source, if present.
    pub fn id_bytes<'a>(&self, input: &'a [u8]) -> Option<&'a [u8]> {
        self.id_value_range
            .and_then(|(start, end)| input.get(start..end))
    }

    /// Borrow the preserved `/Info` value bytes from the source, if present.
    pub fn info_bytes<'a>(&self, input: &'a [u8]) -> Option<&'a [u8]> {
        self.info_value_range
            .and_then(|(start, end)| input.get(start..end))
    }
}

/// Scan the newest trailer dictionary for `/Encrypt`, `/XRefStm`, `/ID`, and
/// `/Info`.
///
/// This reuses the same bounded dictionary inspectors the classic chain already
/// ran over this trailer, so it copies no trailer bytes and only records small
/// offsets. Present `/Encrypt` and `/XRefStm` keys are rejected upstream; a
/// present `/ID` or `/Info` value range is preserved verbatim into the appended
/// trailer.
fn scan_active_trailer(
    input: &[u8],
    trailer_byte_offset: usize,
) -> Result<(ActiveTrailerScan, Range<usize>), WriteError> {
    let trailer_dictionary = inspect_classic_xref_trailer_dictionary(input, trailer_byte_offset)
        .map_err(|error| WriteError::ActiveTrailer {
            error: ActiveTrailerError::TrailerDictionary { error },
        })?;
    let entries = inspect_dictionary_entries(input, trailer_dictionary.dictionary_open_byte_offset)
        .map_err(|error| WriteError::ActiveTrailer {
            error: ActiveTrailerError::Entries { error },
        })?;

    let mut scan = ActiveTrailerScan {
        id_value_range: None,
        info_value_range: None,
    };
    let mut has_encrypt = false;
    let mut has_xref_stm = false;
    for entry in &entries.entries {
        let key = input.get(entry.key_range.start..entry.key_range.end);
        if key == Some(ENCRYPT_KEY) {
            has_encrypt = true;
        } else if key == Some(XREF_STM_KEY) {
            has_xref_stm = true;
        } else if key == Some(ID_KEY) && scan.id_value_range.is_none() {
            scan.id_value_range = Some((entry.value_range.start, entry.value_range.end));
        } else if key == Some(INFO_KEY) && scan.info_value_range.is_none() {
            scan.info_value_range = Some((entry.value_range.start, entry.value_range.end));
        }
    }

    if has_encrypt {
        return Err(WriteError::EncryptedInput);
    }
    if has_xref_stm {
        return Err(WriteError::HybridXrefStmInput);
    }
    let dictionary_range = trailer_dictionary.dictionary_open_byte_offset
        ..trailer_dictionary.after_dictionary_close_byte_offset;
    Ok((scan, dictionary_range))
}

/// Format one fixed-width classic cross-reference entry
/// `{offset:010} {generation:05} n \n` (20 bytes total).
///
/// # Errors
///
/// Returns [`WriteError::XrefOffsetTooLarge`] when `byte_offset` exceeds
/// [`MAX_CLASSIC_XREF_OFFSET`] and cannot fit the fixed 10-digit offset field.
fn xref_entry_bytes(
    byte_offset: usize,
    generation: u16,
) -> Result<[u8; XREF_ENTRY_WIDTH], WriteError> {
    if byte_offset > MAX_CLASSIC_XREF_OFFSET {
        return Err(WriteError::XrefOffsetTooLarge { byte_offset });
    }
    let text = format!("{byte_offset:010} {generation:05} n \n");
    let mut entry = [0u8; XREF_ENTRY_WIDTH];
    entry.copy_from_slice(text.as_bytes());
    Ok(entry)
}

/// One appended object's classic xref entry inputs.
struct AppendedEntry {
    object_number: u32,
    generation: u16,
    byte_offset: usize,
}

/// Owns the growing output buffer and the appended-revision byte assembly.
///
/// The buffer is seeded with the input bytes verbatim, so every later push only
/// appends writer-owned bytes (using LF end-of-line) after the preserved prefix.
struct AppendRevisionWriter {
    out: Vec<u8>,
}

impl AppendRevisionWriter {
    /// Seed the output with the verbatim input plus a small headroom estimate.
    fn new(input: &[u8], dirty_objects: &[DirtyObjectBytes]) -> Self {
        let body_bytes: usize = dirty_objects
            .iter()
            .map(|dirty| dirty.body_bytes.len())
            .sum();
        Self::with_capacity_estimate(input, body_bytes, dirty_objects.len())
    }

    /// Seed the output with the verbatim input plus a headroom estimate sized
    /// from the total appended body byte length and item count.
    fn with_capacity_estimate(input: &[u8], body_bytes: usize, item_count: usize) -> Self {
        let headroom = body_bytes + 64 * item_count + 256;
        let mut out = Vec::with_capacity(input.len() + headroom);
        out.extend_from_slice(input);
        Self { out }
    }

    /// Current output length, used as the offset of the next appended bytes.
    const fn len(&self) -> usize {
        self.out.len()
    }

    /// Append a single `\n` before the first appended object only when the input
    /// prefix does not already end in an end-of-line byte.
    fn ensure_leading_eol(&mut self) {
        if !self
            .out
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            self.out.push(b'\n');
        }
    }

    /// Append one `N G obj … endobj` object, returning the offset recorded before
    /// the header.
    fn append_object(&mut self, reference: IndirectRef, body: &[u8]) -> usize {
        let byte_offset = self.out.len();
        let object_number = reference.object_number;
        let generation = reference.generation;
        self.out
            .extend_from_slice(format!("{object_number} {generation} obj\n").as_bytes());
        self.out.extend_from_slice(body);
        if !self
            .out
            .last()
            .is_some_and(|byte| matches!(byte, b'\n' | b'\r'))
        {
            self.out.push(b'\n');
        }
        self.out.extend_from_slice(b"endobj\n");
        byte_offset
    }

    /// Append the classic cross-reference table over the appended entries,
    /// grouping consecutive object numbers into subsections.
    fn push_xref_table(&mut self, records: &[AppendedEntry]) -> Result<(), WriteError> {
        self.out.extend_from_slice(b"xref\n");
        let mut index = 0;
        while index < records.len() {
            let start = index;
            let first = records[start].object_number;
            while index + 1 < records.len()
                && records[index + 1].object_number == records[index].object_number + 1
            {
                index += 1;
            }
            index += 1;
            let count = index - start;
            self.out
                .extend_from_slice(format!("{first} {count}\n").as_bytes());
            for record in &records[start..index] {
                self.out
                    .extend_from_slice(&xref_entry_bytes(record.byte_offset, record.generation)?);
            }
        }
        Ok(())
    }

    /// Append the trailer dictionary, preserving `/Root` and optional `/ID`, and
    /// setting `/Size` and `/Prev`.
    fn push_trailer(
        &mut self,
        size: usize,
        root: IndirectRef,
        prev_byte_offset: usize,
        id_bytes: Option<&[u8]>,
        info_bytes: Option<&[u8]>,
    ) {
        let root_number = root.object_number;
        let root_generation = root.generation;
        self.out.extend_from_slice(b"trailer\n<< /Size ");
        self.out.extend_from_slice(size.to_string().as_bytes());
        self.out.extend_from_slice(b" /Root ");
        self.out
            .extend_from_slice(format!("{root_number} {root_generation} R").as_bytes());
        self.out.extend_from_slice(b" /Prev ");
        self.out
            .extend_from_slice(prev_byte_offset.to_string().as_bytes());
        if let Some(id) = id_bytes {
            self.out.extend_from_slice(b" /ID ");
            self.out.extend_from_slice(id);
        }
        if let Some(info) = info_bytes {
            self.out.extend_from_slice(b" /Info ");
            self.out.extend_from_slice(info);
        }
        self.out.extend_from_slice(b" >>\n");
    }

    /// Append the `startxref` pointer at the appended table and the final
    /// `%%EOF` marker.
    fn push_startxref(&mut self, xref_byte_offset: usize) {
        self.out.extend_from_slice(b"startxref\n");
        self.out
            .extend_from_slice(xref_byte_offset.to_string().as_bytes());
        self.out.extend_from_slice(b"\n%%EOF\n");
    }

    /// Consume the writer and yield the assembled output bytes.
    fn finish(self) -> Vec<u8> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::{MAX_CLASSIC_XREF_OFFSET, WriteError, XREF_ENTRY_WIDTH, xref_entry_bytes};

    #[test]
    fn entry_is_fixed_twenty_byte_width() {
        assert_eq!(XREF_ENTRY_WIDTH, 20);
        assert_eq!(xref_entry_bytes(1234, 0), Ok(*b"0000001234 00000 n \n"));
    }

    #[test]
    fn max_offset_and_generation_format() {
        assert_eq!(
            xref_entry_bytes(MAX_CLASSIC_XREF_OFFSET, 7),
            Ok(*b"9999999999 00007 n \n")
        );
    }

    #[test]
    fn offset_above_ten_digit_field_rejects() {
        let byte_offset = MAX_CLASSIC_XREF_OFFSET + 1;
        assert_eq!(
            xref_entry_bytes(byte_offset, 0),
            Err(WriteError::XrefOffsetTooLarge { byte_offset })
        );
    }
}
