//! Public bridge from classic-xref PDF bytes to page-object inventory.

use presslint_inventory::{GraphicsWalkError, Inventory};
use presslint_pdf::{
    ContentStreamDataExtentInspectionError, ContentStreamDataSliceError,
    ContentStreamFilterClassification, ContentStreamFilterClassificationError,
    DocumentPageContentExtentInspection, DocumentPageContentExtentResult,
    DocumentPageContentExtentsInspectionError, FlateDecodeParametersResolution,
    FlateDecodeParametersResolutionError, FlateDecodeStreamError, ObjectLookup,
    SkippedPageContentTargetReason, inspect_classic_document_access,
    inspect_document_page_content_extents,
};
use presslint_syntax::{AssembleError, TokenizeError};
use presslint_types::{PageIndex, PdfName};
use serde::{Deserialize, Serialize};

use crate::form_inventory::{
    FormExpandedInventory, FormWalkContext, SkippedFormInventory, build_page_inventory_with_forms,
};
use crate::page_content::{PageContentBytes, page_content_bytes};

/// Result of building inventory from a classic-xref PDF.
///
/// Page indexes in this report are zero-based document-order ordinals assigned
/// by the page-content-extents pass. Inventoried page entries are merged into
/// `inventory` in the same page order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassicPdfInventory {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Combined inventory for all pages that this bridge could build.
    pub inventory: Inventory,
    /// Non-fatal page `XObject` resource inspection failure, when the resource
    /// pass could not begin.
    pub xobject_resource_error: Option<presslint_pdf::DocumentPageXObjectResourcesInspectionError>,
    /// One document-ordered result for each enumerated page.
    pub pages: Vec<ClassicPdfInventoryPage>,
}

/// Per-page bridge result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicPdfInventoryPage {
    /// Zero-based document-order page ordinal.
    pub page_index: PageIndex,
    /// Page inventory result or structured skip.
    pub result: ClassicPdfInventoryPageResult,
    /// Page-local `XObject` resource diagnostics.
    pub xobject_resource_skipped: Vec<presslint_pdf::SkippedPageXObjectResource>,
}

/// Inventory result for one page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ClassicPdfInventoryPageResult {
    /// The page had supported content streams and was inventoried.
    Inventoried {
        /// Number of entries emitted for this page.
        entry_count: usize,
        /// Structured per-form expansion skips emitted while inventorying this
        /// page. Empty when every invoked form was fully walked.
        form_skipped: Vec<SkippedFormInventory>,
    },
    /// The page was intentionally skipped with a structured reason.
    Skipped {
        /// Structured reason the page was not inventoried.
        reason: ClassicPdfInventorySkip,
    },
}

/// Structured page/stream skip reasons for the classic PDF inventory bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "skip", rename_all = "snake_case")]
pub enum ClassicPdfInventorySkip {
    /// The page `/Contents` value could not be inspected.
    ContentsFailed {
        /// Delegated page `/Contents` inspection failure.
        error: presslint_pdf::PageContentsInspectionError,
    },
    /// The leaf `/Page` object is a type-2 compressed object-stream member; the
    /// classic offset-based path never produces this, but the shared skip
    /// vocabulary carries it so the conversion stays total.
    CompressedLeaf {
        /// Object number of the containing object stream.
        object_stream_number: usize,
        /// Index of this object inside the object stream.
        index_within_object_stream: usize,
    },
    /// The page had no content-stream targets.
    NoContentStreams,
    /// The page had more than one content-stream target.
    MultipleContentStreams {
        /// Number of content targets reported for this page.
        stream_count: usize,
    },
    /// A content-stream target could not be resolved through the classic xref table.
    TargetSkipped {
        /// Delegated target skip reason.
        reason: SkippedPageContentTargetReason,
    },
    /// A resolved content-stream target's stream extent could not be located.
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
        error: TokenizeError,
    },
    /// The tokenized content stream could not be assembled into operators.
    AssembleFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated assembler failure.
        error: AssembleError,
    },
    /// The graphics-state inventory walk failed.
    GraphicsWalkFailed {
        /// Resolved content-stream object byte offset.
        object_byte_offset: usize,
        /// Delegated graphics-state walk failure.
        error: GraphicsWalkError,
    },
}

/// Error returned when the classic PDF inventory bridge cannot establish the
/// document path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicPdfInventoryError {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Structured top-level failure.
    pub reason: ClassicPdfInventoryRejection,
}

/// Top-level bridge failures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ClassicPdfInventoryRejection {
    /// The classic-xref document-access spine failed.
    ClassicDocumentAccess {
        /// Delegated document-access failure.
        error: Box<presslint_pdf::ClassicDocumentAccessError>,
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

/// Build page-object inventory from a classic-xref PDF byte slice.
///
/// The bridge accepts borrowed PDF bytes and never mutates them. It establishes
/// the existing classic-xref document-access path, locates page content-stream
/// data extents, and inventories pages whose located content streams are raw or
/// individual `/FlateDecode` streams with resolved non-array `/DecodeParms`.
///
/// Raw streams are passed to syntax and inventory as borrowed slices. Flate
/// streams allocate only the bounded decoded buffer returned by
/// [`presslint_pdf::decode_flate_stream`]. Multiple decoded streams are joined
/// with an explicit whitespace separator into one bounded synthetic page
/// content buffer before tokenization. Page `XObject` resources are inspected
/// structurally when available so image/form `Do` invocations can be classified.
///
/// Page indexes in the returned report are zero-based document-order ordinals.
///
/// # Errors
///
/// Returns [`ClassicPdfInventoryError`] only for failures that prevent the
/// classic document/page-content path from being established. Unsupported page
/// and stream shapes are represented as structured page skips.
pub fn build_classic_pdf_inventory(
    input: &[u8],
    max_decoded_stream_bytes: usize,
) -> Result<ClassicPdfInventory, ClassicPdfInventoryError> {
    let access = inspect_classic_document_access(input).map_err(|error| {
        inventory_error(
            input,
            ClassicPdfInventoryRejection::ClassicDocumentAccess {
                error: Box::new(error),
            },
        )
    })?;
    let extents = inspect_document_page_content_extents(
        input,
        &access.xref_table,
        access.page_tree_root.object_byte_offset,
    )
    .map_err(|error| {
        inventory_error(
            input,
            ClassicPdfInventoryRejection::PageContentExtents {
                error: Box::new(error),
            },
        )
    })?;
    let xobject_resources = presslint_pdf::inspect_document_page_xobject_resources(
        input,
        &access.xref_table,
        access.page_tree_root.object_byte_offset,
    );
    let (xobject_resource_error, xobject_pages) = match xobject_resources {
        Ok(report) => (None, Some(report.pages)),
        Err(error) => (Some(error), None),
    };

    let lookup = ObjectLookup::ClassicXref(&access.xref_table);
    let mut inventory = Inventory::default();
    let mut pages = Vec::with_capacity(extents.pages.len());
    for page in &extents.pages {
        let page_index = page_index_error(input, page.ordinal)?;
        let resources = xobject_pages
            .as_ref()
            .and_then(|pages| pages.get(page.ordinal));
        let image_xobject_names = resources.map_or_else(Vec::new, |resources| {
            inventory_names(&resources.image_xobject_names)
        });
        let form_xobject_names = resources.map_or_else(Vec::new, |resources| {
            inventory_names(&resources.form_xobject_names)
        });
        let form_targets =
            resources.map_or(&[][..], |resources| resources.form_xobjects.as_slice());
        let result = match build_page_inventory_with_forms(
            input,
            lookup,
            page,
            page_index,
            max_decoded_stream_bytes,
            &image_xobject_names,
            &form_xobject_names,
            form_targets,
            FormWalkContext::bounded_default(),
        ) {
            Ok(expanded) => {
                let FormExpandedInventory {
                    inventory: page_inventory,
                    form_skipped,
                } = expanded;
                let entry_count = page_inventory.len();
                inventory.entries.extend(page_inventory.entries);
                ClassicPdfInventoryPageResult::Inventoried {
                    entry_count,
                    form_skipped,
                }
            }
            Err(reason) => ClassicPdfInventoryPageResult::Skipped {
                reason: reason.into(),
            },
        };
        pages.push(ClassicPdfInventoryPage {
            page_index,
            result,
            xobject_resource_skipped: resources
                .map_or_else(Vec::new, |resources| resources.skipped.clone()),
        });
    }

    Ok(ClassicPdfInventory {
        byte_len: input.len(),
        inventory,
        xobject_resource_error,
        pages,
    })
}

/// Decode a page's located content streams into borrowed or bounded-owned bytes,
/// returning the assembled page-content buffer and the first stream's object
/// byte offset for diagnostic attribution.
///
/// This is the shared front half of page inventory: both the page-only path and
/// the form-expansion path decode page content the same way before tokenizing.
pub fn decode_page_content<'input>(
    input: &'input [u8],
    page: &DocumentPageContentExtentInspection,
    max_decoded_stream_bytes: usize,
) -> Result<(PageContentBytes<'input>, usize), InventoryPageSkip> {
    let extents = match &page.result {
        DocumentPageContentExtentResult::Inspected { extents, .. } => extents,
        DocumentPageContentExtentResult::ContentsFailed { error } => {
            return Err(InventoryPageSkip::ContentsFailed {
                error: error.clone(),
            });
        }
        // A compressed leaf `/Page` has no source byte offset to read a
        // `/Contents` value from, so it is enumerated and honestly skipped rather
        // than inventoried; compressed-leaf content inventory is a follow-up.
        DocumentPageContentExtentResult::CompressedLeaf {
            object_stream_number,
            index_within_object_stream,
        } => {
            return Err(InventoryPageSkip::CompressedLeaf {
                object_stream_number: *object_stream_number,
                index_within_object_stream: *index_within_object_stream,
            });
        }
    };

    if extents.entries.is_empty() {
        return Err(InventoryPageSkip::NoContentStreams);
    }
    let first_stream_offset = extents
        .entries
        .iter()
        .find_map(|entry| match entry {
            presslint_pdf::PageContentExtentInspection::Located {
                object_byte_offset, ..
            }
            | presslint_pdf::PageContentExtentInspection::Failed {
                object_byte_offset, ..
            } => Some(*object_byte_offset),
            presslint_pdf::PageContentExtentInspection::Skipped { .. } => None,
        })
        .unwrap_or_default();
    let content = page_content_bytes(input, &extents.entries, max_decoded_stream_bytes)?;
    Ok((content, first_stream_offset))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryPageSkip {
    ContentsFailed {
        error: presslint_pdf::PageContentsInspectionError,
    },
    /// The leaf `/Page` object is a type-2 compressed object-stream member.
    CompressedLeaf {
        object_stream_number: usize,
        index_within_object_stream: usize,
    },
    NoContentStreams,
    MultipleContentStreams {
        stream_count: usize,
    },
    TargetSkipped {
        reason: SkippedPageContentTargetReason,
    },
    ExtentFailed {
        object_byte_offset: usize,
        error: ContentStreamDataExtentInspectionError,
    },
    SliceFailed {
        object_byte_offset: usize,
        error: ContentStreamDataSliceError,
    },
    FilterClassificationFailed {
        object_byte_offset: usize,
        error: ContentStreamFilterClassificationError,
    },
    UnsupportedFilter {
        object_byte_offset: usize,
        classification: ContentStreamFilterClassification,
    },
    DecodeParmsFailed {
        object_byte_offset: usize,
        error: FlateDecodeParametersResolutionError,
    },
    UnsupportedDecodeParms {
        object_byte_offset: usize,
        resolution: FlateDecodeParametersResolution,
    },
    DecodeFailed {
        object_byte_offset: usize,
        error: FlateDecodeStreamError,
    },
    TokenizeFailed {
        object_byte_offset: usize,
        error: TokenizeError,
    },
    AssembleFailed {
        object_byte_offset: usize,
        error: AssembleError,
    },
    GraphicsWalkFailed {
        object_byte_offset: usize,
        error: GraphicsWalkError,
    },
}

impl From<InventoryPageSkip> for ClassicPdfInventorySkip {
    fn from(skip: InventoryPageSkip) -> Self {
        match skip {
            InventoryPageSkip::ContentsFailed { error } => Self::ContentsFailed { error },
            InventoryPageSkip::CompressedLeaf {
                object_stream_number,
                index_within_object_stream,
            } => Self::CompressedLeaf {
                object_stream_number,
                index_within_object_stream,
            },
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

pub fn page_index(ordinal: usize) -> Result<PageIndex, usize> {
    u32::try_from(ordinal).map(PageIndex).map_err(|_| ordinal)
}

fn page_index_error(input: &[u8], ordinal: usize) -> Result<PageIndex, ClassicPdfInventoryError> {
    page_index(ordinal).map_err(|ordinal| {
        inventory_error(
            input,
            ClassicPdfInventoryRejection::PageIndexOutOfRange { ordinal },
        )
    })
}

pub fn inventory_names(names: &[presslint_pdf::PdfName]) -> Vec<PdfName> {
    names.iter().map(|name| PdfName(name.0.clone())).collect()
}

const fn inventory_error(
    input: &[u8],
    reason: ClassicPdfInventoryRejection,
) -> ClassicPdfInventoryError {
    ClassicPdfInventoryError {
        byte_len: input.len(),
        reason,
    }
}
