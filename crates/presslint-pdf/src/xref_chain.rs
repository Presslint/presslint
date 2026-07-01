use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::xref_section::classify_xref_section;
use crate::{
    IndirectRef, PdfSourceDiagnostic, XrefSection, XrefStreamEntry, XrefStreamEntryRecord,
    XrefStreamSection, XrefStreamSectionError, decode_xref_stream_section,
};

/// Maximum number of xref-stream sections followed through a `/Prev` chain.
pub const MAX_XREF_STREAM_CHAIN_SECTIONS: usize = 64;

/// Maximum number of merged entries retained across an xref-stream `/Prev`
/// chain.
pub const MAX_XREF_STREAM_CHAIN_ENTRIES: usize = 1_000_000;

/// Newest-wins object map built from a same-type xref-stream `/Prev` chain.
///
/// The first decoded section is the `startxref` section and therefore the
/// newest section. Earlier sections only fill object numbers not already present
/// in the accumulated map, including when the newest entry is a type-0 free
/// record. Final entries are deterministic and sorted by object number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrefStreamChain {
    /// Caller-supplied byte offset of the newest xref-stream section.
    pub startxref_byte_offset: usize,
    /// Decoded xref-stream section byte offsets, newest to oldest.
    pub section_byte_offsets: Vec<usize>,
    /// `/Root` reference read from the newest section only.
    pub root_reference: IndirectRef,
    /// Effective `/Size`, the maximum `/Size` observed across merged sections.
    pub effective_size: usize,
    /// Newest-wins entries in ascending object-number order.
    pub entries: Vec<XrefStreamEntry>,
}

/// Error returned when an xref-stream `/Prev` chain cannot be built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct XrefStreamChainError {
    /// Newest xref-stream byte offset supplied by the caller.
    pub startxref_byte_offset: usize,
    /// Total source length.
    pub byte_len: usize,
    /// Structured chain stop reason.
    pub reason: XrefStreamChainRejection,
}

/// Structured xref-stream chain rejection reasons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum XrefStreamChainRejection {
    /// The current section offset is outside the input bounds.
    OffsetOutOfBounds {
        /// Out-of-bounds xref-stream byte offset.
        byte_offset: usize,
    },
    /// Following `/Prev` revisited a section offset already seen.
    Cycle {
        /// Revisited xref-stream byte offset.
        byte_offset: usize,
    },
    /// The bounded maximum number of sections was reached before the chain
    /// ended.
    SectionLimitExceeded {
        /// Configured section limit.
        max_sections: usize,
    },
    /// The bounded maximum number of merged entries was exceeded.
    EntryLimitExceeded {
        /// Configured entry limit.
        max_entries: usize,
    },
    /// A `/Prev` target could not be classified as an xref section.
    PrevSectionUnclassified {
        /// `/Prev` byte offset that failed classification.
        byte_offset: usize,
        /// Delegated source diagnostic.
        diagnostic: Box<PdfSourceDiagnostic>,
    },
    /// A `/Prev` target classified as a classic xref table, which is a
    /// same-type-chain miss deferred to the mixed/classic follow-up slices.
    PrevSectionNotXrefStream {
        /// `/Prev` byte offset that was not an xref stream.
        byte_offset: usize,
    },
    /// Decoding one section in the chain failed.
    SectionDecode {
        /// Xref-stream section byte offset.
        byte_offset: usize,
        /// Delegated single-section decode failure.
        error: Box<XrefStreamSectionError>,
    },
}

/// Build a bounded newest-wins xref-stream object map by following `/Prev`.
///
/// The builder decodes one section at a time with
/// [`decode_xref_stream_section`], retains only the merged entries and section
/// offsets, and stops with structured errors for out-of-bounds offsets, cycles,
/// over-long chains, non-xref-stream `/Prev` targets, classification failures,
/// decode failures, and merged-entry bound overflow.
///
/// # Errors
///
/// Returns [`XrefStreamChainError`] for any bounded chain stop. No partial map is
/// returned.
pub fn build_xref_stream_chain(
    input: &[u8],
    startxref_byte_offset: usize,
    max_decoded_stream_bytes: usize,
) -> Result<XrefStreamChain, XrefStreamChainError> {
    let mut visited = BTreeSet::new();
    let mut section_byte_offsets = Vec::new();
    let mut entries = BTreeMap::<usize, XrefStreamEntryRecord>::new();
    let mut next_offset = Some(startxref_byte_offset);
    let mut root_reference = None;
    let mut effective_size = 0usize;

    while let Some(byte_offset) = next_offset {
        if section_byte_offsets.len() >= MAX_XREF_STREAM_CHAIN_SECTIONS {
            return Err(chain_error(
                input,
                startxref_byte_offset,
                XrefStreamChainRejection::SectionLimitExceeded {
                    max_sections: MAX_XREF_STREAM_CHAIN_SECTIONS,
                },
            ));
        }
        let section = decode_chain_section(
            input,
            startxref_byte_offset,
            byte_offset,
            max_decoded_stream_bytes,
            &mut visited,
        )?;

        if root_reference.is_none() {
            root_reference = Some(section.root_reference);
        }
        effective_size = effective_size.max(section.size);
        for entry in section.entries {
            if !entries.contains_key(&entry.object_number)
                && entries.len() >= MAX_XREF_STREAM_CHAIN_ENTRIES
            {
                return Err(chain_error(
                    input,
                    startxref_byte_offset,
                    XrefStreamChainRejection::EntryLimitExceeded {
                        max_entries: MAX_XREF_STREAM_CHAIN_ENTRIES,
                    },
                ));
            }
            entries.entry(entry.object_number).or_insert(entry.record);
        }

        section_byte_offsets.push(byte_offset);
        next_offset = section.prev_byte_offset;
    }

    let Some(root_reference) = root_reference else {
        return Err(chain_error(
            input,
            startxref_byte_offset,
            XrefStreamChainRejection::OffsetOutOfBounds {
                byte_offset: startxref_byte_offset,
            },
        ));
    };

    Ok(XrefStreamChain {
        startxref_byte_offset,
        section_byte_offsets,
        root_reference,
        effective_size,
        entries: entries
            .into_iter()
            .map(|(object_number, record)| XrefStreamEntry {
                object_number,
                record,
            })
            .collect(),
    })
}

fn decode_chain_section(
    input: &[u8],
    startxref_byte_offset: usize,
    byte_offset: usize,
    max_decoded_stream_bytes: usize,
    visited: &mut BTreeSet<usize>,
) -> Result<XrefStreamSection, XrefStreamChainError> {
    if byte_offset >= input.len() {
        return Err(chain_error(
            input,
            startxref_byte_offset,
            XrefStreamChainRejection::OffsetOutOfBounds { byte_offset },
        ));
    }
    if !visited.insert(byte_offset) {
        return Err(chain_error(
            input,
            startxref_byte_offset,
            XrefStreamChainRejection::Cycle { byte_offset },
        ));
    }

    match classify_xref_section(input, byte_offset).map_err(|diagnostic| {
        chain_error(
            input,
            startxref_byte_offset,
            XrefStreamChainRejection::PrevSectionUnclassified {
                byte_offset,
                diagnostic: Box::new(diagnostic),
            },
        )
    })? {
        XrefSection::Stream { .. } => {}
        XrefSection::Table => {
            return Err(chain_error(
                input,
                startxref_byte_offset,
                XrefStreamChainRejection::PrevSectionNotXrefStream { byte_offset },
            ));
        }
    }

    decode_xref_stream_section(input, byte_offset, max_decoded_stream_bytes).map_err(|error| {
        chain_error(
            input,
            startxref_byte_offset,
            XrefStreamChainRejection::SectionDecode {
                byte_offset,
                error: Box::new(error),
            },
        )
    })
}

const fn chain_error(
    input: &[u8],
    startxref_byte_offset: usize,
    reason: XrefStreamChainRejection,
) -> XrefStreamChainError {
    XrefStreamChainError {
        startxref_byte_offset,
        byte_len: input.len(),
        reason,
    }
}
