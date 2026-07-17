//! Append-only incremental-update PDF writing.
//!
//! This crate holds the first byte-writing slice of the presslint F3 patch
//! executor: a deterministic *incremental append* writer.
//!
//! [`write_incremental_revision`] is the foundational semantic **no-op**: it
//! copies the caller's input verbatim and appends one incremental revision,
//! using a classic cross-reference table for classic inputs and a raw
//! cross-reference stream for xref-stream inputs. It rewrites selected
//! uncompressed objects with caller-supplied body bytes. [`set_page_boxes_incremental`]
//! is the first *semantic* mutation
//! built on it: it sets `/MediaBox` and/or `/CropBox` on selected uncompressed
//! leaf page dictionaries, reading leaf references and box provenance from
//! [`presslint_pdf::inspect_document_page_boxes`], deciding ownership with
//! [`presslint_pdf::decide_indirect_object_edit`], and rewriting only the edited
//! leaf bodies before delegating xref/trailer assembly to
//! [`write_incremental_revision`].
//!
//! The append mechanics prove what the semantic writer needs: it copies the
//! caller's input
//! verbatim as the output prefix, then appends one revision that rewrites
//! selected existing uncompressed indirect objects with caller-supplied body
//! bytes, followed by an xref section matching the input's final xref kind and a
//! `/Prev` link.
//!
//! It proves the append mechanics the future semantic writer needs — verbatim
//! prefix preservation, appended-object offset accounting, xref-entry encoding,
//! whole-`/Prev`-chain `/Size` computation, and newest-wins resolution
//! through the existing [`presslint_pdf`] access spine — without performing any
//! semantic edit. It deliberately does not encode dictionaries, rewrite content
//! operands, re-encode streams, clone shared objects, delete objects, repair
//! free lists, preserve encryption, write hybrid updates, or mutate compressed
//! object-stream members. It also rejects hybrid-reference classic trailers
//! carrying `/XRefStm`, because this slice does not merge supplemental
//! xref-stream entries.
//!
//! [`write_incremental_revision_plan`] is the validating bridge from the
//! backend-agnostic [`presslint_actions::IncrementalRevisionPlan`] contract to
//! the byte writer: it validates dirty-object intent (boundary kind, boundary
//! target agreement, in-place ownership, duplicate object numbers) before any
//! byte assembly, converts the validated dirty objects to [`DirtyObjectBytes`],
//! and delegates all xref/trailer/backend mechanics to
//! [`write_incremental_revision`]. `set_page_boxes_incremental` routes its
//! already-proven leaf edits through this bridge.
//!
//! Structural facts about the input (the final `startxref`, the active
//! cross-reference `/Prev` chain, `/Root`, and object currency) are read through
//! [`presslint_pdf`] rather than reparsed here, so the writer stays a thin byte
//! assembler over already-validated structural metadata.
//!
//! [`reserve_fresh_object_references`] and
//! [`write_incremental_revision_with_fresh_objects`] add one low-level,
//! collision-safe currency for *new* objects: a caller reserves exact
//! generation-zero identities proved never to collide with any existing
//! effective identity or indirect-reference target (including dangling
//! references in trailers, unreferenced bodies, and compressed members), then
//! supplies bodies for those identities as [`FreshObjectBytes`] alongside
//! ordinary dirty rewrites. This is a lower-level identity/serialization
//! prerequisite only: it does not build a Form clone, choose a consumer edge,
//! or authorize paint mutation, and `fresh_objects=[]` is byte-for-byte
//! identical to [`write_incremental_revision`].
//!
//! On top of that reservation currency the converter carries one private,
//! request-scoped Form clone-set PLAN: for qualifying page-local Form binding
//! witnesses it computes a bounded reached-Form closure and reserves fresh
//! identities, publishing only the additive observe-only
//! [`FormCloneSetPlanCounts`] per converted page. The plan writes no bytes,
//! clones no object, retargets no page, and admits nothing to conversion.

#![forbid(unsafe_code)]

mod alias_epoch_plan;
mod black_preservation;
mod content_color_convert;
mod content_color_rewrite;
mod content_edit_pipeline;
pub(crate) mod content_object_ownership;
mod content_sequence_pipeline;
mod content_stream_plan;
mod extgstate_page_guard;
mod form_clone_set_plan;
mod form_xobject_effect;
mod fresh_objects;
mod link_routing;
mod page_box_serialize;
mod page_boxes;
mod page_content_sequence;
mod page_device_space_policy;
mod page_font_policy;
mod page_xobject_policy;
mod pdf_number_serialize;
mod planned;
mod reencode_content;
mod selector_match;
mod stream_object_body;
mod writer;
mod xref_stream_writer;

pub use black_preservation::BlackPreservationPolicy;
pub use content_color_convert::{
    ConvertContentColorsError, ConvertContentColorsOutput, ConvertContentColorsRequest,
    ConvertPageSkip, ConvertPageSkipReason, ConvertedPage, OperatorSkipCounts,
    convert_content_colors_incremental,
};
pub use content_color_rewrite::{
    ContentColorRewriteError, ContentColorRewriteOutput, ContentColorRewriteRequest,
    ContentColorRewriteSkip, ContentColorRewriteSkipReason, RewrittenPage,
    rewrite_rgb_black_to_cmyk_incremental,
};
pub use content_edit_pipeline::PageSelection;
pub use form_clone_set_plan::FormCloneSetPlanCounts;
pub use form_xobject_effect::{FormXObjectRefusalClass, FormXObjectRefusalCounts};
pub use link_routing::{DeviceLinkInput, LinkConversionCounts};
pub use page_boxes::{
    AppliedBox, DictionaryEntryWrite, EditedPage, PageBoxEdit, SetPageBoxSkipReason,
    SetPageBoxesError, SetPageBoxesOutput, SetPageBoxesRequest, SkippedPageEdit,
    set_page_boxes_incremental,
};
pub use planned::{PlannedWriteError, UnsupportedBoundaryKind, write_incremental_revision_plan};
pub use reencode_content::{
    ReencodeFilterKind, ReencodePageContentError, ReencodePageContentOutput,
    ReencodePageContentRequest, ReencodePageSkip, ReencodePageSkipReason, ReencodedPage,
    reencode_page_content_incremental,
};
pub use selector_match::UnsupportedTargetLeaf;
pub use writer::{
    ActiveTrailerError, DirtyObjectBytes, FreshObjectBytes, WriteError,
    reserve_fresh_object_references, write_incremental_revision,
    write_incremental_revision_with_fresh_objects,
};

#[cfg(test)]
mod tests;
