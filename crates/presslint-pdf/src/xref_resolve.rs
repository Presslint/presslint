use serde::{Deserialize, Serialize};

use crate::{ClassicXrefEntry, ClassicXrefEntryState, ClassicXrefTableInspection};

/// Location result for an object number resolved from a classic xref table.
///
/// This is a locate-only result. In-use entries report the byte offset field
/// from the xref entry, but the resolver does not read or validate bytes at
/// that offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "location", rename_all = "snake_case")]
pub enum ClassicXrefObjectLocation {
    /// The object number matched exactly one in-use entry.
    InUse {
        /// Resolved object number.
        object_number: u32,
        /// Generation number from the xref entry.
        generation: u16,
        /// Byte offset field from the in-use xref entry.
        byte_offset: usize,
    },
    /// The object number matched exactly one free entry.
    Free {
        /// Resolved object number.
        object_number: u32,
        /// Generation number from the xref entry.
        generation: u16,
        /// First numeric field from the free xref entry.
        next_free_object_number: usize,
    },
    /// The object number is not present in any parsed subsection entry.
    NotFound {
        /// Requested object number.
        object_number: u32,
    },
    /// The object number appears in more than one parsed subsection entry.
    Ambiguous {
        /// Requested object number.
        object_number: u32,
        /// First matching entry in deterministic table scan order.
        first: ClassicXrefAmbiguousObjectEntry,
        /// Second matching entry in deterministic table scan order.
        second: ClassicXrefAmbiguousObjectEntry,
    },
}

/// Entry summary reported for duplicate classic xref object numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicXrefAmbiguousObjectEntry {
    /// Generation number from the matching xref entry.
    pub generation: u16,
    /// Numeric offset field from the matching xref entry.
    pub byte_offset: usize,
    /// Free or in-use entry state.
    pub state: ClassicXrefEntryState,
}

/// Resolve an object number against a parsed classic xref table inspection.
///
/// The scan is deterministic and allocation-free. It only reads parsed
/// subsection and entry metadata already present in `inspection`; it performs no
/// source-byte access, I/O, trailer parsing, object parsing, or generation
/// acceptance policy.
#[must_use]
pub fn resolve_classic_xref_object(
    inspection: &ClassicXrefTableInspection,
    object_number: u32,
) -> ClassicXrefObjectLocation {
    let mut found = None;

    for subsection in &inspection.subsections {
        let Some(relative_index) = object_number.checked_sub(subsection.first_object_number) else {
            continue;
        };
        if relative_index >= subsection.entry_count {
            continue;
        }

        let Some(entry) = subsection.entries.get(relative_index as usize).copied() else {
            continue;
        };
        if entry.object_number != object_number {
            continue;
        }

        if let Some(first) = found {
            return ClassicXrefObjectLocation::Ambiguous {
                object_number,
                first: ambiguous_entry(first),
                second: ambiguous_entry(entry),
            };
        }

        found = Some(entry);
    }

    found.map_or(
        ClassicXrefObjectLocation::NotFound { object_number },
        location_for_entry,
    )
}

const fn location_for_entry(entry: ClassicXrefEntry) -> ClassicXrefObjectLocation {
    match entry.state {
        ClassicXrefEntryState::InUse => ClassicXrefObjectLocation::InUse {
            object_number: entry.object_number,
            generation: entry.generation,
            byte_offset: entry.byte_offset,
        },
        ClassicXrefEntryState::Free => ClassicXrefObjectLocation::Free {
            object_number: entry.object_number,
            generation: entry.generation,
            next_free_object_number: entry.byte_offset,
        },
    }
}

const fn ambiguous_entry(entry: ClassicXrefEntry) -> ClassicXrefAmbiguousObjectEntry {
    ClassicXrefAmbiguousObjectEntry {
        generation: entry.generation,
        byte_offset: entry.byte_offset,
        state: entry.state,
    }
}
