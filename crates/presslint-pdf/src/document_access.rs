use serde::{Deserialize, Serialize};

use crate::startxref::inspect_startxref;
use crate::xref_section::classify_xref_section;
use crate::{
    CatalogPagesInspection, CatalogPagesInspectionError, ClassicXrefTableInspection,
    ClassicXrefTableInspectionError, ClassicXrefTrailerRootInspection,
    ClassicXrefTrailerRootInspectionError, ObjectResolutionError, PageTreeLeavesInspection,
    PageTreeLeavesInspectionError, PdfSourceDiagnostic, PdfStartXref, ResolvedObject, XrefSection,
    inspect_catalog_pages, inspect_classic_xref_table, inspect_classic_xref_trailer_root,
    inspect_page_tree_leaves, resolve_classic_xref_object_offset,
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
