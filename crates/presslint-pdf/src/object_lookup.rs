use serde::{Deserialize, Serialize};

use crate::{
    ClassicXrefAmbiguousObjectEntry, ClassicXrefObjectLocation, ClassicXrefTableInspection,
    XrefStreamEntry, XrefStreamEntryRecord, XrefStreamSection, resolve_classic_xref_object,
};

/// Borrowed object lookup backend.
///
/// This abstraction is intentionally only a view over already-parsed
/// cross-reference metadata. It builds no cache, owns no source bytes, and does
/// not parse object bodies.
#[derive(Debug, Clone, Copy)]
pub enum ObjectLookup<'a> {
    /// Parsed classic cross-reference table backend.
    ClassicXref(&'a ClassicXrefTableInspection),
    /// Decoded single cross-reference-stream section backend.
    XrefStreamSection(&'a XrefStreamSection),
}

/// Locate-only result for an object number from any supported xref backend.
///
/// In-use or uncompressed entries report byte offsets but do not validate that
/// the offset points at a matching indirect object header. Compressed and
/// reserved xref-stream entries are structural results, not byte offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "location", rename_all = "snake_case")]
pub enum ObjectLookupLocation {
    /// A classic xref table produced exactly one in-use entry.
    ClassicInUse {
        /// Resolved object number.
        object_number: usize,
        /// Generation number from the xref entry.
        generation: u16,
        /// Byte offset field from the in-use xref entry.
        byte_offset: usize,
    },
    /// A classic xref table produced exactly one free entry.
    ClassicFree {
        /// Resolved object number.
        object_number: usize,
        /// Generation number from the xref entry.
        generation: u16,
        /// First numeric field from the free xref entry.
        next_free_object_number: usize,
    },
    /// The object number is absent from a parsed classic xref table.
    ClassicNotFound {
        /// Requested object number.
        object_number: usize,
    },
    /// The requested object number cannot fit the classic xref public contract.
    ClassicObjectNumberOutOfRange {
        /// Requested object number.
        object_number: usize,
    },
    /// A parsed classic xref table contains more than one matching entry.
    ClassicAmbiguous {
        /// Requested object number.
        object_number: usize,
        /// First matching entry in deterministic table scan order.
        first: ClassicXrefAmbiguousObjectEntry,
        /// Second matching entry in deterministic table scan order.
        second: ClassicXrefAmbiguousObjectEntry,
    },
    /// A cross-reference stream produced a type-1 uncompressed-object entry.
    XrefStreamUncompressed {
        /// Resolved object number.
        object_number: usize,
        /// Generation number from the xref-stream entry.
        generation: u16,
        /// Byte offset of the object in the PDF source.
        byte_offset: usize,
    },
    /// A cross-reference stream produced a type-1 uncompressed-object entry
    /// whose generation cannot fit the public indirect-reference contract.
    XrefStreamUncompressedGenerationOutOfRange {
        /// Resolved object number.
        object_number: usize,
        /// Generation number from the xref-stream entry.
        generation: usize,
        /// Byte offset of the object in the PDF source.
        byte_offset: usize,
    },
    /// A cross-reference stream produced a type-0 free-object entry.
    XrefStreamFree {
        /// Resolved object number.
        object_number: usize,
        /// Generation number of the free object.
        generation: u16,
        /// Object number of the next free object.
        next_free_object_number: usize,
    },
    /// A cross-reference stream produced a type-0 free-object entry whose
    /// generation cannot fit the public indirect-reference contract.
    XrefStreamFreeGenerationOutOfRange {
        /// Resolved object number.
        object_number: usize,
        /// Generation number of the free object.
        generation: usize,
        /// Object number of the next free object.
        next_free_object_number: usize,
    },
    /// A cross-reference stream produced a type-2 compressed-object entry.
    XrefStreamCompressed {
        /// Resolved object number.
        object_number: usize,
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Index of this object inside the object stream.
        index_within_object_stream: usize,
    },
    /// A cross-reference stream produced a reserved or future entry type.
    XrefStreamReserved {
        /// Resolved object number.
        object_number: usize,
        /// Raw type field value.
        entry_type: u64,
        /// Raw second field value.
        field2: u64,
        /// Raw third field value.
        field3: u64,
    },
    /// The object number is absent from the decoded cross-reference-stream
    /// section.
    XrefStreamNotFound {
        /// Requested object number.
        object_number: usize,
    },
    /// A matching decoded cross-reference-stream entry has an object number
    /// that cannot fit the public indirect-reference contract.
    XrefStreamObjectNumberOutOfRange {
        /// Decoded xref-stream object number.
        object_number: usize,
    },
}

/// Locate an object number in a borrowed xref backend.
///
/// Classic lookup delegates to the existing deterministic table scan.
/// Cross-reference-stream lookup binary-searches the already-sorted
/// [`XrefStreamSection::entries`] vector and builds no per-call map.
#[must_use]
pub fn locate_xref_object(lookup: ObjectLookup<'_>, object_number: usize) -> ObjectLookupLocation {
    match lookup {
        ObjectLookup::ClassicXref(xref) => locate_classic_object(xref, object_number),
        ObjectLookup::XrefStreamSection(section) => {
            locate_xref_stream_object(section, object_number)
        }
    }
}

fn locate_classic_object(
    xref: &ClassicXrefTableInspection,
    object_number: usize,
) -> ObjectLookupLocation {
    let Ok(classic_object_number) = u32::try_from(object_number) else {
        return ObjectLookupLocation::ClassicObjectNumberOutOfRange { object_number };
    };
    classic_location(resolve_classic_xref_object(xref, classic_object_number))
}

fn classic_location(location: ClassicXrefObjectLocation) -> ObjectLookupLocation {
    match location {
        ClassicXrefObjectLocation::InUse {
            object_number,
            generation,
            byte_offset,
        } => ObjectLookupLocation::ClassicInUse {
            object_number: classic_object_number(object_number),
            generation,
            byte_offset,
        },
        ClassicXrefObjectLocation::Free {
            object_number,
            generation,
            next_free_object_number,
        } => ObjectLookupLocation::ClassicFree {
            object_number: classic_object_number(object_number),
            generation,
            next_free_object_number,
        },
        ClassicXrefObjectLocation::NotFound { object_number } => {
            ObjectLookupLocation::ClassicNotFound {
                object_number: classic_object_number(object_number),
            }
        }
        ClassicXrefObjectLocation::Ambiguous {
            object_number,
            first,
            second,
        } => ObjectLookupLocation::ClassicAmbiguous {
            object_number: classic_object_number(object_number),
            first,
            second,
        },
    }
}

fn classic_object_number(object_number: u32) -> usize {
    usize::try_from(object_number).map_or(usize::MAX, |value| value)
}

fn locate_xref_stream_object(
    section: &XrefStreamSection,
    object_number: usize,
) -> ObjectLookupLocation {
    let Ok(index) = section
        .entries
        .binary_search_by_key(&object_number, |entry| entry.object_number)
    else {
        return ObjectLookupLocation::XrefStreamNotFound { object_number };
    };

    xref_stream_location(section.entries[index])
}

fn xref_stream_location(entry: XrefStreamEntry) -> ObjectLookupLocation {
    if entry.object_number > max_indirect_object_number() {
        return ObjectLookupLocation::XrefStreamObjectNumberOutOfRange {
            object_number: entry.object_number,
        };
    }

    let object_number = entry.object_number;
    match entry.record {
        XrefStreamEntryRecord::Free {
            next_free_object_number,
            generation,
        } => u16::try_from(generation).map_or(
            ObjectLookupLocation::XrefStreamFreeGenerationOutOfRange {
                object_number,
                generation,
                next_free_object_number,
            },
            |generation| ObjectLookupLocation::XrefStreamFree {
                object_number,
                generation,
                next_free_object_number,
            },
        ),
        XrefStreamEntryRecord::Uncompressed {
            byte_offset,
            generation,
        } => u16::try_from(generation).map_or(
            ObjectLookupLocation::XrefStreamUncompressedGenerationOutOfRange {
                object_number,
                generation,
                byte_offset,
            },
            |generation| ObjectLookupLocation::XrefStreamUncompressed {
                object_number,
                generation,
                byte_offset,
            },
        ),
        XrefStreamEntryRecord::Compressed {
            object_stream_number,
            index_within_object_stream,
        } => ObjectLookupLocation::XrefStreamCompressed {
            object_number,
            object_stream_number,
            index_within_object_stream,
        },
        XrefStreamEntryRecord::Reserved {
            entry_type,
            field2,
            field3,
        } => ObjectLookupLocation::XrefStreamReserved {
            object_number,
            entry_type,
            field2,
            field3,
        },
    }
}

fn max_indirect_object_number() -> usize {
    classic_object_number(u32::MAX)
}
