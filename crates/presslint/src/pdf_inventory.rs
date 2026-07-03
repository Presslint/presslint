//! Public bridge from backend-neutral PDF bytes to page-object inventory.

use presslint_inventory::Inventory;
use presslint_pdf::{
    ContentStreamDataExtentInspectionError, ContentStreamDataSliceError,
    ContentStreamFilterClassification, ContentStreamFilterClassificationError, DocumentAccess,
    DocumentAccessBackend, DocumentAccessError, DocumentAccessRejection,
    DocumentPageContentExtentsInspection, DocumentPageContentExtentsInspectionError,
    FlateDecodeParametersResolution, FlateDecodeParametersResolutionError, FlateDecodeStreamError,
    ObjectLookup, SkippedPageContentTargetReason, inspect_document_access,
    inspect_document_page_content_extents_resolved,
    inspect_document_page_xobject_resources_with_lookup, resolve_object,
};
use presslint_types::PageIndex;
use serde::{Deserialize, Serialize};

use crate::document_inventory::{InventoryPageSkip, inventory_names, page_index};
use crate::form_inventory::{
    FormExpandedInventory, FormWalkContext, SkippedFormInventory, build_page_inventory_with_forms,
};

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
    /// Non-fatal page `XObject` resource inspection failure, when the resource
    /// pass could not begin.
    pub xobject_resource_error: Option<presslint_pdf::DocumentPageXObjectResourcesInspectionError>,
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
    /// Top-level page-scope image `XObject` targets, each carrying the resolved
    /// image reference/offset and structural
    /// [`ImageXObjectMetadata`](presslint_pdf::ImageXObjectMetadata) (image
    /// dimensions and colour-space family) without decoding image samples.
    pub image_xobjects: Vec<presslint_pdf::PageXObjectResourceTarget>,
    /// Page-local `XObject` resource diagnostics.
    pub xobject_resource_skipped: Vec<presslint_pdf::SkippedPageXObjectResource>,
}

/// Inventory result for one page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PdfInventoryPageResult {
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
    /// The leaf `/Page` object is a type-2 compressed object-stream member. The
    /// page is COUNTED as enumerated with zero colour observations; its content
    /// is a coverage gap (`SkippedPage`) rather than a hard failure, since
    /// compressed-leaf content inventory is a deliberate follow-up.
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
    /// A content-stream target could not be resolved through the selected backend.
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
/// classic-xref backend, bounded same-type classic-table `/Prev` chains, one
/// `/Type /XRef` stream backend, and bounded same-type xref-stream `/Prev`
/// chains selected by [`inspect_document_access`], then resolves the page-tree
/// root and locates page content-stream data extents through the resolved
/// content-extents bridge.
///
/// Raw single streams are passed to syntax and inventory as borrowed slices.
/// Flate streams allocate only the bounded decoded buffer returned by
/// [`presslint_pdf::decode_flate_stream`]. Multiple decoded streams are joined
/// with an explicit whitespace separator into one bounded synthetic page
/// content buffer before tokenization. Page `XObject` resources are inspected
/// structurally when available so image/form `Do` invocations can be classified.
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
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    };
    let extents = resolved_page_content_extents(input, lookup, &access, max_decoded_stream_bytes)?;
    let xobject_resources = inspect_document_page_xobject_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    );
    let (xobject_resource_error, xobject_pages) = match xobject_resources {
        Ok(report) => (None, Some(report.pages)),
        Err(error) => (Some(error), None),
    };

    let mut inventory = Inventory::default();
    let mut pages = Vec::with_capacity(extents.pages.len());
    for page in &extents.pages {
        let page_index = page_index(page.ordinal).map_err(|ordinal| {
            inventory_error(
                input,
                PdfInventoryRejection::PageIndexOutOfRange { ordinal },
            )
        })?;
        let resources = xobject_pages
            .as_ref()
            .and_then(|pages| pages.get(page.ordinal));
        let image_xobject_names =
            resources.map_or_else(Vec::new, |r| inventory_names(&r.image_xobject_names));
        let form_xobject_names =
            resources.map_or_else(Vec::new, |r| inventory_names(&r.form_xobject_names));
        let form_targets = resources.map_or(&[][..], |r| r.form_xobjects.as_slice());
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
                PdfInventoryPageResult::Inventoried {
                    entry_count,
                    form_skipped,
                }
            }
            Err(reason) => PdfInventoryPageResult::Skipped {
                reason: reason.into(),
            },
        };
        pages.push(PdfInventoryPage {
            page_index,
            result,
            image_xobjects: resources
                .map_or_else(Vec::new, |resources| resources.image_xobjects.clone()),
            xobject_resource_skipped: resources
                .map_or_else(Vec::new, |resources| resources.skipped.clone()),
        });
    }

    Ok(PdfInventory {
        byte_len: input.len(),
        inventory,
        xobject_resource_error,
        pages,
    })
}

impl From<InventoryPageSkip> for PdfInventorySkip {
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

/// Re-resolve the page-tree root to body-aware data and locate document page
/// content-stream extents through the resolved bridge.
///
/// `access.page_tree_root` is a `ResolvedStructuralObject`, which does NOT retain
/// the decoded bytes of a compressed page-tree root, so this re-resolves the root
/// to `ResolvedObjectData` and routes through
/// [`inspect_document_page_content_extents_resolved`]. That lets a page tree
/// whose root or intermediate `/Pages` nodes are compressed object-stream members
/// navigate to its leaves instead of hard-failing on an offset-0 header read.
/// Exactly one `resolve_object` runs for the root; the resolved leaf enumeration
/// owns any bounded object-stream decode, capped by `max_decoded_stream_bytes`.
fn resolved_page_content_extents(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    access: &DocumentAccess,
    max_decoded_stream_bytes: usize,
) -> Result<DocumentPageContentExtentsInspection, PdfInventoryError> {
    let resolved_root = resolve_object(
        input,
        lookup,
        access.page_tree_root.reference,
        max_decoded_stream_bytes,
    )
    .map_err(|error| {
        inventory_error(
            input,
            PdfInventoryRejection::DocumentAccess {
                error: Box::new(DocumentAccessError {
                    byte_len: input.len(),
                    reason: DocumentAccessRejection::PagesObject { error },
                }),
            },
        )
    })?;
    inspect_document_page_content_extents_resolved(
        input,
        lookup,
        &resolved_root,
        max_decoded_stream_bytes,
    )
    .map_err(|error| {
        inventory_error(
            input,
            PdfInventoryRejection::PageContentExtents {
                error: Box::new(error),
            },
        )
    })
}

const fn inventory_error(input: &[u8], reason: PdfInventoryRejection) -> PdfInventoryError {
    PdfInventoryError {
        byte_len: input.len(),
        reason,
    }
}
