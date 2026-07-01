//! Public bridge from backend-neutral PDF bytes to page-object inventory.

use presslint_inventory::Inventory;
use presslint_pdf::{
    ContentStreamDataExtentInspectionError, ContentStreamDataSliceError,
    ContentStreamFilterClassification, ContentStreamFilterClassificationError,
    DocumentAccessBackend, DocumentAccessError, DocumentPageContentExtentsInspectionError,
    FlateDecodeParametersResolution, FlateDecodeParametersResolutionError, FlateDecodeStreamError,
    ObjectLookup, SkippedPageContentTargetReason, inspect_document_access,
    inspect_document_page_content_extents_with_lookup,
};
use presslint_types::PageIndex;
use serde::{Deserialize, Serialize};

use crate::document_inventory::{InventoryPageSkip, build_page_inventory, page_index};

/// Result of building inventory from a backend-neutral PDF.
///
/// Page indexes in this report are zero-based document-order ordinals assigned
/// by the page-content-extents pass. Inventoried page entries are merged into
/// `inventory` in the same page order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PdfInventory {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Combined inventory for all pages that this bridge could build.
    pub inventory: Inventory,
    /// One document-ordered result for each enumerated page.
    pub pages: Vec<PdfInventoryPage>,
}

/// Per-page bridge result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfInventoryPage {
    /// Zero-based document-order page ordinal.
    pub page_index: PageIndex,
    /// Page inventory result or structured skip.
    pub result: PdfInventoryPageResult,
}

/// Inventory result for one page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PdfInventoryPageResult {
    /// The page had exactly one supported content stream and was inventoried.
    Inventoried {
        /// Number of entries emitted for this page.
        entry_count: usize,
    },
    /// The page was intentionally skipped with a structured reason.
    Skipped {
        /// Structured reason the page was not inventoried.
        reason: PdfInventorySkip,
    },
}

/// Structured page/stream skip reasons for the neutral PDF inventory bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "skip", rename_all = "snake_case")]
pub enum PdfInventorySkip {
    /// The page `/Contents` value could not be inspected.
    ContentsFailed {
        /// Delegated page `/Contents` inspection failure.
        error: presslint_pdf::PageContentsInspectionError,
    },
    /// The page had no content-stream targets.
    NoContentStreams,
    /// The page had more than one content-stream target.
    MultipleContentStreams {
        /// Number of content targets reported for this page.
        stream_count: usize,
    },
    /// The single target could not be resolved through the selected backend.
    TargetSkipped {
        /// Delegated target skip reason.
        reason: SkippedPageContentTargetReason,
    },
    /// The single resolved target's stream extent could not be located.
    ExtentFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated stream-extent failure.
        error: ContentStreamDataExtentInspectionError,
    },
    /// The located extent could not be bridged to a borrowed source slice.
    SliceFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated slice failure.
        error: ContentStreamDataSliceError,
    },
    /// The stream `/Filter` declaration was malformed.
    FilterClassificationFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated filter-classification failure.
        error: ContentStreamFilterClassificationError,
    },
    /// The stream uses a filter shape this bridge does not decode.
    UnsupportedFilter {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated filter classification.
        classification: ContentStreamFilterClassification,
    },
    /// The stream `/DecodeParms` declaration was malformed.
    DecodeParmsFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated `/DecodeParms` failure.
        error: FlateDecodeParametersResolutionError,
    },
    /// The stream uses the `/DecodeParms` array form.
    UnsupportedDecodeParms {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated `/DecodeParms` resolution.
        resolution: FlateDecodeParametersResolution,
    },
    /// The bounded `/FlateDecode` operation failed.
    DecodeFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated Flate decode failure.
        error: FlateDecodeStreamError,
    },
    /// The decoded content stream could not be tokenized.
    TokenizeFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated tokenizer failure.
        error: presslint_syntax::TokenizeError,
    },
    /// The tokenized content stream could not be assembled into operators.
    AssembleFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated assembler failure.
        error: presslint_syntax::AssembleError,
    },
    /// The graphics-state inventory walk failed.
    GraphicsWalkFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated graphics-state walk failure.
        error: presslint_inventory::GraphicsWalkError,
    },
}

/// Error returned when the neutral PDF inventory bridge cannot establish the
/// document path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PdfInventoryError {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Structured top-level failure.
    pub reason: PdfInventoryRejection,
}

/// Top-level bridge failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum PdfInventoryRejection {
    /// The backend-neutral document-access spine failed.
    DocumentAccess {
        /// Delegated document-access failure.
        error: Box<DocumentAccessError>,
    },
    /// The page-content-extents aggregate could not be established.
    PageContentExtents {
        /// Delegated page-content-extents failure.
        error: Box<DocumentPageContentExtentsInspectionError>,
    },
    /// A document-order page ordinal did not fit in [`PageIndex`].
    PageIndexOutOfRange {
        /// Zero-based document-order ordinal that exceeded `u32`.
        ordinal: usize,
    },
}

/// Build page-object inventory from a PDF byte slice through the neutral
/// document-access spine.
///
/// The bridge accepts borrowed PDF bytes and never mutates them. It supports a
/// classic-xref backend, one `/Type /XRef` stream backend, and bounded
/// same-type xref-stream `/Prev` chains selected by [`inspect_document_access`],
/// then locates page content-stream data extents through
/// [`inspect_document_page_content_extents_with_lookup`].
///
/// Raw streams are passed to syntax and inventory as borrowed slices. Flate
/// streams allocate only the bounded decoded buffer returned by
/// [`presslint_pdf::decode_flate_stream`]. Resource dictionaries are not
/// inspected in this slice, so empty image and form `XObject` name lists are
/// supplied to the combined inventory builder by the shared page helper.
///
/// # Errors
///
/// Returns [`PdfInventoryError`] only for failures that prevent the neutral
/// document/page-content path from being established. Unsupported page and
/// stream shapes are represented as structured page skips.
pub fn build_pdf_inventory(
    input: &[u8],
    max_decoded_stream_bytes: usize,
) -> Result<PdfInventory, PdfInventoryError> {
    let access = inspect_document_access(input).map_err(|error| {
        inventory_error(
            input,
            PdfInventoryRejection::DocumentAccess {
                error: Box::new(error),
            },
        )
    })?;

    let lookup = match &access.backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    };
    let extents = inspect_document_page_content_extents_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    )
    .map_err(|error| {
        inventory_error(
            input,
            PdfInventoryRejection::PageContentExtents {
                error: Box::new(error),
            },
        )
    })?;

    let mut inventory = Inventory::default();
    let mut pages = Vec::with_capacity(extents.pages.len());
    for page in &extents.pages {
        let page_index = page_index(page.ordinal).map_err(|ordinal| {
            inventory_error(
                input,
                PdfInventoryRejection::PageIndexOutOfRange { ordinal },
            )
        })?;
        let result = match build_page_inventory(input, page, page_index, max_decoded_stream_bytes) {
            Ok(page_inv) => {
                let entry_count = page_inv.len();
                inventory.entries.extend(page_inv.entries);
                PdfInventoryPageResult::Inventoried { entry_count }
            }
            Err(reason) => PdfInventoryPageResult::Skipped {
                reason: reason.into(),
            },
        };
        pages.push(PdfInventoryPage { page_index, result });
    }

    Ok(PdfInventory {
        byte_len: input.len(),
        inventory,
        pages,
    })
}

impl From<InventoryPageSkip> for PdfInventorySkip {
    fn from(skip: InventoryPageSkip) -> Self {
        match skip {
            InventoryPageSkip::ContentsFailed { error } => Self::ContentsFailed { error },
            InventoryPageSkip::NoContentStreams => Self::NoContentStreams,
            InventoryPageSkip::MultipleContentStreams { stream_count } => {
                Self::MultipleContentStreams { stream_count }
            }
            InventoryPageSkip::TargetSkipped { reason } => Self::TargetSkipped { reason },
            InventoryPageSkip::ExtentFailed {
                object_byte_offset,
                error,
            } => Self::ExtentFailed {
                object_byte_offset,
                error,
            },
            InventoryPageSkip::SliceFailed {
                object_byte_offset,
                error,
            } => Self::SliceFailed {
                object_byte_offset,
                error,
            },
            InventoryPageSkip::FilterClassificationFailed {
                object_byte_offset,
                error,
            } => Self::FilterClassificationFailed {
                object_byte_offset,
                error,
            },
            InventoryPageSkip::UnsupportedFilter {
                object_byte_offset,
                classification,
            } => Self::UnsupportedFilter {
                object_byte_offset,
                classification,
            },
            InventoryPageSkip::DecodeParmsFailed {
                object_byte_offset,
                error,
            } => Self::DecodeParmsFailed {
                object_byte_offset,
                error,
            },
            InventoryPageSkip::UnsupportedDecodeParms {
                object_byte_offset,
                resolution,
            } => Self::UnsupportedDecodeParms {
                object_byte_offset,
                resolution,
            },
            InventoryPageSkip::DecodeFailed {
                object_byte_offset,
                error,
            } => Self::DecodeFailed {
                object_byte_offset,
                error,
            },
            InventoryPageSkip::TokenizeFailed {
                object_byte_offset,
                error,
            } => Self::TokenizeFailed {
                object_byte_offset,
                error,
            },
            InventoryPageSkip::AssembleFailed {
                object_byte_offset,
                error,
            } => Self::AssembleFailed {
                object_byte_offset,
                error,
            },
            InventoryPageSkip::GraphicsWalkFailed {
                object_byte_offset,
                error,
            } => Self::GraphicsWalkFailed {
                object_byte_offset,
                error,
            },
        }
    }
}

const fn inventory_error(input: &[u8], reason: PdfInventoryRejection) -> PdfInventoryError {
    PdfInventoryError {
        byte_len: input.len(),
        reason,
    }
}
