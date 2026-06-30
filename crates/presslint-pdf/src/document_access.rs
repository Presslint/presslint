use serde::{Deserialize, Serialize};

use crate::startxref::inspect_startxref;
use crate::xref_section::classify_xref_section;
use crate::{
    CatalogPagesInspection, CatalogPagesInspectionError, ClassicXrefTableInspection,
    ClassicXrefTableInspectionError, ClassicXrefTrailerRootInspection,
    ClassicXrefTrailerRootInspectionError, IndirectRef, ObjectLookup, ObjectResolutionError,
    PageTreeLeavesInspection, PageTreeLeavesInspectionError, PdfSourceDiagnostic, PdfStartXref,
    ResolvedObject, XrefSection, XrefStreamSection, XrefStreamSectionError,
    decode_xref_stream_section, inspect_catalog_pages, inspect_classic_xref_table,
    inspect_classic_xref_trailer_root, inspect_page_tree_leaves,
    inspect_page_tree_leaves_with_lookup, resolve_classic_xref_object_offset,
    resolve_xref_object_offset,
};

/// Report-only structural access summary for a classic-xref PDF.
///
/// This is the first composing document-access spine. It threads the existing
/// low-level inspectors together: `startxref`, xref-section classification, the
/// classic xref table, the trailer `/Root`, root-reference object resolution,
/// the catalog `/Pages`, pages-reference object resolution, and document-ordered
/// page-tree leaf enumeration.
///
/// This report stores only structural metadata already produced by the delegated
/// inspections. It does not retain or copy PDF bytes, object bodies, stream
/// bodies, dictionaries, decoded streams, or source slices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicDocumentAccess {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Final `startxref` record located in the bounded trailing window.
    pub startxref: PdfStartXref,
    /// Parsed classic cross-reference table.
    pub xref_table: ClassicXrefTableInspection,
    /// Trailer `/Root` inspection, including the parsed catalog reference.
    pub trailer_root: ClassicXrefTrailerRootInspection,
    /// Catalog object resolved from the trailer `/Root` reference.
    pub catalog: ResolvedObject,
    /// Catalog `/Pages` inspection, including the parsed page-tree-root
    /// reference.
    pub catalog_pages: CatalogPagesInspection,
    /// Page-tree-root object resolved from the catalog `/Pages` reference.
    pub page_tree_root: ResolvedObject,
    /// Document-ordered leaf `/Page` enumeration, including non-fatal skips.
    pub page_leaves: PageTreeLeavesInspection,
}

/// Error returned when the classic document-access spine cannot complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassicDocumentAccessError {
    /// Total source length.
    pub byte_len: usize,
    /// Structured failure reason, naming the spine stage that failed.
    pub reason: ClassicDocumentAccessRejection,
}

/// Structured classic document-access rejection reasons.
///
/// Each variant names the spine stage that failed and preserves the delegated
/// failure verbatim, so a caller can see exactly where the ordered path stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum ClassicDocumentAccessRejection {
    /// The final `startxref` record could not be located.
    StartXref {
        /// Delegated source diagnostic from `startxref` inspection.
        diagnostic: PdfSourceDiagnostic,
    },
    /// The cross-reference section at the `startxref` offset could not be
    /// classified.
    XrefSectionUnclassified {
        /// Delegated source diagnostic from section classification.
        diagnostic: PdfSourceDiagnostic,
    },
    /// The section is a cross-reference stream. This spine handles only classic
    /// xref tables; the xref-stream object-map backend is a separate, future
    /// path and is not attempted here.
    UnsupportedXrefStream {
        /// Object number from the xref-stream indirect object header.
        object_number: u32,
        /// Generation number from the xref-stream indirect object header.
        generation: u16,
    },
    /// Classic xref table inspection failed.
    XrefTable {
        /// Delegated classic xref table inspection failure.
        error: ClassicXrefTableInspectionError,
    },
    /// Trailer `/Root` inspection failed.
    TrailerRoot {
        /// Delegated trailer `/Root` inspection failure.
        error: ClassicXrefTrailerRootInspectionError,
    },
    /// The trailer `/Root` reference did not resolve to a catalog object.
    RootObject {
        /// Delegated object-resolution failure.
        error: ObjectResolutionError,
    },
    /// Catalog `/Pages` inspection failed.
    CatalogPages {
        /// Delegated catalog `/Pages` inspection failure.
        error: CatalogPagesInspectionError,
    },
    /// The catalog `/Pages` reference did not resolve to a page-tree-root
    /// object.
    PagesObject {
        /// Delegated object-resolution failure.
        error: ObjectResolutionError,
    },
    /// Leaf-page enumeration could not begin at the resolved page-tree root.
    PageTreeLeaves {
        /// Delegated leaf-enumeration failure.
        error: PageTreeLeavesInspectionError,
    },
}

/// Compose the classic-xref document-access spine over caller-provided bytes.
///
/// The helper runs the existing inspectors in document order and stops at the
/// first stage that fails, reporting the delegated failure verbatim through
/// [`ClassicDocumentAccessRejection`]. A cross-reference-stream section is a
/// structured unsupported result, not a success; no xref-stream object-map work
/// is attempted.
///
/// Page-tree leaf enumeration is non-fatal for individual kids: other-typed
/// kids, per-kid resolution failures, and bound-stopped descents remain as
/// structured skips inside [`ClassicDocumentAccess::page_leaves`] rather than
/// failing the spine.
///
/// It builds no whole-document object map or cache, follows no `/Prev` chain,
/// merges no incremental sections, extracts no object streams, decodes no stream
/// bodies, and mutates no source bytes.
///
/// # Errors
///
/// Returns [`ClassicDocumentAccessError`] when `startxref` is missing or
/// malformed, the section cannot be classified, the section is a cross-reference
/// stream, or any delegated table/trailer/resolution/catalog/leaf stage fails.
pub fn inspect_classic_document_access(
    input: &[u8],
) -> Result<ClassicDocumentAccess, ClassicDocumentAccessError> {
    let startxref = inspect_startxref(input).map_err(|diagnostic| {
        document_access_error(
            input,
            ClassicDocumentAccessRejection::StartXref { diagnostic },
        )
    })?;

    match classify_xref_section(input, startxref.byte_offset).map_err(|diagnostic| {
        document_access_error(
            input,
            ClassicDocumentAccessRejection::XrefSectionUnclassified { diagnostic },
        )
    })? {
        XrefSection::Table => {}
        XrefSection::Stream {
            object_number,
            generation,
        } => {
            return Err(document_access_error(
                input,
                ClassicDocumentAccessRejection::UnsupportedXrefStream {
                    object_number,
                    generation,
                },
            ));
        }
    }

    let xref_table = inspect_classic_xref_table(input, startxref.byte_offset).map_err(|error| {
        document_access_error(input, ClassicDocumentAccessRejection::XrefTable { error })
    })?;

    let trailer_root = inspect_classic_xref_trailer_root(input, xref_table.trailer_byte_offset)
        .map_err(|error| {
            document_access_error(input, ClassicDocumentAccessRejection::TrailerRoot { error })
        })?;

    let catalog =
        resolve_classic_xref_object_offset(input, &xref_table, trailer_root.root_reference)
            .map_err(|error| {
                document_access_error(input, ClassicDocumentAccessRejection::RootObject { error })
            })?;

    let catalog_pages =
        inspect_catalog_pages(input, catalog.object_byte_offset).map_err(|error| {
            document_access_error(
                input,
                ClassicDocumentAccessRejection::CatalogPages { error },
            )
        })?;

    let page_tree_root =
        resolve_classic_xref_object_offset(input, &xref_table, catalog_pages.pages_reference)
            .map_err(|error| {
                document_access_error(input, ClassicDocumentAccessRejection::PagesObject { error })
            })?;

    let page_leaves =
        inspect_page_tree_leaves(input, &xref_table, page_tree_root.object_byte_offset).map_err(
            |error| {
                document_access_error(
                    input,
                    ClassicDocumentAccessRejection::PageTreeLeaves { error },
                )
            },
        )?;

    Ok(ClassicDocumentAccess {
        byte_len: input.len(),
        startxref,
        xref_table,
        trailer_root,
        catalog,
        catalog_pages,
        page_tree_root,
        page_leaves,
    })
}

const fn document_access_error(
    input: &[u8],
    reason: ClassicDocumentAccessRejection,
) -> ClassicDocumentAccessError {
    ClassicDocumentAccessError {
        byte_len: input.len(),
        reason,
    }
}

/// Maximum decoded cross-reference-stream body size accepted by the neutral
/// single-section spine.
///
/// The decoded body holds only fixed-width cross-reference records, so this
/// bound caps the [`decode_xref_stream_section`] allocation without limiting
/// realistic single-section documents.
pub const MAX_XREF_STREAM_SECTION_DECODED_BYTES: usize = 8 * 1024 * 1024;

/// Report-only structural access summary for a single-section PDF, backend
/// neutral over a classic xref table or one decoded cross-reference-stream
/// section.
///
/// This is the first composing spine that threads the [`ObjectLookup`] backend
/// boundary through page-tree traversal. It selects the backend from the
/// `startxref` section classification, reads `/Root` from the matching trailer,
/// resolves the catalog and page-tree root through the selected backend, and
/// enumerates document-ordered leaves through
/// [`inspect_page_tree_leaves_with_lookup`].
///
/// This report stores only structural metadata already produced by the delegated
/// inspections. It does not retain or copy PDF bytes, object bodies, stream
/// bodies, dictionaries, decoded streams, or source slices; the single decoded
/// cross-reference-stream buffer is dropped inside
/// [`decode_xref_stream_section`] before this report is built.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentAccess {
    /// Total source length supplied by the caller.
    pub byte_len: usize,
    /// Final `startxref` record located in the bounded trailing window.
    pub startxref: PdfStartXref,
    /// Selected cross-reference backend plus its parsed metadata.
    pub backend: DocumentAccessBackend,
    /// Document catalog `/Root` reference read from the matching trailer.
    pub root_reference: IndirectRef,
    /// Catalog object resolved from the trailer `/Root` reference.
    pub catalog: ResolvedObject,
    /// Catalog `/Pages` inspection, including the parsed page-tree-root
    /// reference.
    pub catalog_pages: CatalogPagesInspection,
    /// Page-tree-root object resolved from the catalog `/Pages` reference.
    pub page_tree_root: ResolvedObject,
    /// Document-ordered leaf `/Page` enumeration, including non-fatal skips.
    pub page_leaves: PageTreeLeavesInspection,
}

/// Cross-reference backend selected by the neutral document-access spine.
///
/// Each variant carries the parsed backend metadata the spine threaded through
/// page-tree traversal. It retains no PDF source bytes or decoded stream data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum DocumentAccessBackend {
    /// The `startxref` section classified as a classic cross-reference table.
    ClassicXref {
        /// Parsed classic cross-reference table.
        xref_table: ClassicXrefTableInspection,
        /// Trailer `/Root` inspection, including the parsed catalog reference.
        trailer_root: ClassicXrefTrailerRootInspection,
    },
    /// The `startxref` section classified as a single cross-reference stream.
    XrefStreamSection {
        /// Decoded single cross-reference-stream section, including its `/Root`
        /// reference and (absent) `/Prev` byte offset.
        section: XrefStreamSection,
    },
}

/// Error returned when the neutral document-access spine cannot complete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentAccessError {
    /// Total source length.
    pub byte_len: usize,
    /// Structured failure reason, naming the spine stage that failed.
    pub reason: DocumentAccessRejection,
}

/// Structured neutral document-access rejection reasons.
///
/// Each variant names the spine stage that failed and preserves the delegated
/// failure verbatim, so a caller can see exactly where the ordered path stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "stage", rename_all = "snake_case")]
pub enum DocumentAccessRejection {
    /// The final `startxref` record could not be located.
    StartXref {
        /// Delegated source diagnostic from `startxref` inspection.
        diagnostic: PdfSourceDiagnostic,
    },
    /// The cross-reference section at the `startxref` offset could not be
    /// classified.
    XrefSectionUnclassified {
        /// Delegated source diagnostic from section classification.
        diagnostic: PdfSourceDiagnostic,
    },
    /// Classic xref table inspection failed.
    XrefTable {
        /// Delegated classic xref table inspection failure.
        error: ClassicXrefTableInspectionError,
    },
    /// Trailer `/Root` inspection failed.
    TrailerRoot {
        /// Delegated trailer `/Root` inspection failure.
        error: ClassicXrefTrailerRootInspectionError,
    },
    /// The single cross-reference-stream section could not be decoded.
    XrefStreamDecode {
        /// Delegated single-section cross-reference-stream decode failure.
        error: XrefStreamSectionError,
    },
    /// The decoded cross-reference-stream section carries a `/Prev`. This spine
    /// decodes exactly one section and never follows `/Prev`, so a present
    /// `/Prev` is a structured stop rather than a multi-section merge.
    PrevPresentUnsupported {
        /// Parsed `/Prev` previous cross-reference byte offset that was not
        /// followed.
        prev_byte_offset: usize,
    },
    /// The trailer `/Root` reference did not resolve to a catalog object.
    RootObject {
        /// Delegated object-resolution failure.
        error: ObjectResolutionError,
    },
    /// Catalog `/Pages` inspection failed.
    CatalogPages {
        /// Delegated catalog `/Pages` inspection failure.
        error: CatalogPagesInspectionError,
    },
    /// The catalog `/Pages` reference did not resolve to a page-tree-root
    /// object.
    PagesObject {
        /// Delegated object-resolution failure.
        error: ObjectResolutionError,
    },
    /// Leaf-page enumeration could not begin at the resolved page-tree root.
    PageTreeLeaves {
        /// Delegated leaf-enumeration failure.
        error: PageTreeLeavesInspectionError,
    },
}

/// Compose the neutral single-section document-access spine over caller bytes.
///
/// The helper selects the cross-reference backend from the `startxref` section
/// classification: a classic table uses [`ObjectLookup::ClassicXref`] over the
/// parsed table; a single `/Type /XRef` stream uses
/// [`ObjectLookup::XrefStreamSection`] over exactly ONE section decoded by
/// [`decode_xref_stream_section`]. In both cases `/Root` is read from the
/// matching trailer, the catalog and page-tree root are resolved through
/// [`resolve_xref_object_offset`], and leaves are enumerated through
/// [`inspect_page_tree_leaves_with_lookup`].
///
/// A decoded cross-reference-stream section whose `/Prev` is present is a
/// structured [`DocumentAccessRejection::PrevPresentUnsupported`] stop: `/Prev`
/// is never followed, no previous section is decoded, no incremental section is
/// merged, and no `/XRefStm` hybrid reference is consulted.
///
/// Page-tree leaf enumeration is non-fatal for individual kids: other-typed
/// kids, per-kid resolution failures (including compressed or reserved
/// cross-reference-stream entries), and bound-stopped descents remain as
/// structured skips inside [`DocumentAccess::page_leaves`] rather than failing
/// the spine.
///
/// It builds no whole-document object map or cache, extracts no object streams,
/// resolves no type-2 compressed objects, opens no document, and mutates no
/// source bytes.
///
/// # Errors
///
/// Returns [`DocumentAccessError`] when `startxref` is missing or malformed, the
/// section cannot be classified, the single cross-reference-stream section fails
/// to decode, the decoded section carries a `/Prev`, or any delegated
/// table/trailer/resolution/catalog/leaf stage fails.
pub fn inspect_document_access(input: &[u8]) -> Result<DocumentAccess, DocumentAccessError> {
    let startxref = inspect_startxref(input).map_err(|diagnostic| {
        access_error(input, DocumentAccessRejection::StartXref { diagnostic })
    })?;

    let section = classify_xref_section(input, startxref.byte_offset).map_err(|diagnostic| {
        access_error(
            input,
            DocumentAccessRejection::XrefSectionUnclassified { diagnostic },
        )
    })?;

    match section {
        XrefSection::Table => classic_spine(input, startxref),
        XrefSection::Stream { .. } => xref_stream_spine(input, startxref),
    }
}

/// Walk the catalog, `/Pages`, page-tree root, and leaves through a selected
/// backend, holding the four shared stage results.
struct SpineWalk {
    catalog: ResolvedObject,
    catalog_pages: CatalogPagesInspection,
    page_tree_root: ResolvedObject,
    page_leaves: PageTreeLeavesInspection,
}

/// Resolve the catalog, catalog `/Pages`, page-tree root, and document-ordered
/// leaves through the supplied backend, stopping at the first failing stage.
fn walk_spine(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    root_reference: IndirectRef,
) -> Result<SpineWalk, DocumentAccessError> {
    let catalog = resolve_xref_object_offset(input, lookup, root_reference)
        .map_err(|error| access_error(input, DocumentAccessRejection::RootObject { error }))?;

    let catalog_pages = inspect_catalog_pages(input, catalog.object_byte_offset)
        .map_err(|error| access_error(input, DocumentAccessRejection::CatalogPages { error }))?;

    let page_tree_root =
        resolve_xref_object_offset(input, lookup, catalog_pages.pages_reference)
            .map_err(|error| access_error(input, DocumentAccessRejection::PagesObject { error }))?;

    let page_leaves =
        inspect_page_tree_leaves_with_lookup(input, lookup, page_tree_root.object_byte_offset)
            .map_err(|error| {
                access_error(input, DocumentAccessRejection::PageTreeLeaves { error })
            })?;

    Ok(SpineWalk {
        catalog,
        catalog_pages,
        page_tree_root,
        page_leaves,
    })
}

/// Compose the spine over a classic cross-reference table backend.
fn classic_spine(
    input: &[u8],
    startxref: PdfStartXref,
) -> Result<DocumentAccess, DocumentAccessError> {
    let xref_table = inspect_classic_xref_table(input, startxref.byte_offset)
        .map_err(|error| access_error(input, DocumentAccessRejection::XrefTable { error }))?;

    let trailer_root = inspect_classic_xref_trailer_root(input, xref_table.trailer_byte_offset)
        .map_err(|error| access_error(input, DocumentAccessRejection::TrailerRoot { error }))?;
    let root_reference = trailer_root.root_reference;

    let walk = walk_spine(
        input,
        ObjectLookup::ClassicXref(&xref_table),
        root_reference,
    )?;

    Ok(DocumentAccess {
        byte_len: input.len(),
        startxref,
        backend: DocumentAccessBackend::ClassicXref {
            xref_table,
            trailer_root,
        },
        root_reference,
        catalog: walk.catalog,
        catalog_pages: walk.catalog_pages,
        page_tree_root: walk.page_tree_root,
        page_leaves: walk.page_leaves,
    })
}

/// Compose the spine over a single decoded cross-reference-stream backend.
fn xref_stream_spine(
    input: &[u8],
    startxref: PdfStartXref,
) -> Result<DocumentAccess, DocumentAccessError> {
    let section = decode_xref_stream_section(
        input,
        startxref.byte_offset,
        MAX_XREF_STREAM_SECTION_DECODED_BYTES,
    )
    .map_err(|error| access_error(input, DocumentAccessRejection::XrefStreamDecode { error }))?;

    if let Some(prev_byte_offset) = section.prev_byte_offset {
        return Err(access_error(
            input,
            DocumentAccessRejection::PrevPresentUnsupported { prev_byte_offset },
        ));
    }
    let root_reference = section.root_reference;

    let walk = walk_spine(
        input,
        ObjectLookup::XrefStreamSection(&section),
        root_reference,
    )?;

    Ok(DocumentAccess {
        byte_len: input.len(),
        startxref,
        backend: DocumentAccessBackend::XrefStreamSection { section },
        root_reference,
        catalog: walk.catalog,
        catalog_pages: walk.catalog_pages,
        page_tree_root: walk.page_tree_root,
        page_leaves: walk.page_leaves,
    })
}

const fn access_error(input: &[u8], reason: DocumentAccessRejection) -> DocumentAccessError {
    DocumentAccessError {
        byte_len: input.len(),
        reason,
    }
}
