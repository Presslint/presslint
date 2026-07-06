//! Bounded recursive Form `XObject` content expansion for the PDF inventory bridge.
//!
//! A page-level Form `XObject` invocation (`/Fm Do`) is inventoried by the
//! page-content walker only as a `FormXObject` invocation entry; the colors,
//! text, and vectors painted INSIDE the form stay invisible. This module walks a
//! form's OWN decoded content stream, classifies the form's own resource names,
//! resolves the form's own colour-space environment, re-invokes
//! [`presslint_inventory::build_inventory_with_color_space_env`] in
//! [`ContentScope::FormXObject`] with the ORIGINAL invoking page index, and
//! merges nested entries immediately after the form invocation entry.
//!
//! The walk is bounded by [`FormWalkContext`]: the default limit is 8 form-stream
//! descents from the page plus a per-page total expansion budget, with
//! active-path cycle detection. Every per-form failure is a structured
//! [`SkippedFormInventory`], never a page failure, panic, or infinite loop; the
//! page's own inventory is always emitted.

use std::collections::BTreeSet;

use presslint_inventory::{
    ColorSpaceEnv, ColorSpaceResource, GraphicsWalkError, Inventory, InventoryEntry, PaintOpKind,
    build_inventory_with_color_space_env, walk_graphics_state,
};
use presslint_pdf::{
    IndirectRef, ObjectLookup, PageXObjectResourceTarget, SkippedPageXObjectResource,
    SkippedPageXObjectResourceReason, inspect_content_stream_data_extent_with_lookup,
    inspect_form_color_space_resources, inspect_form_xobject_resources,
};
use presslint_syntax::{assemble_operators, tokenize};
use presslint_types::{ContentScope, ObjectKind, PageIndex, PdfName};
use serde::{Deserialize, Serialize};

use crate::document_inventory::{
    InventoryPageSkip, color_space_env_resources, decode_page_content, inventory_names,
};
use crate::page_content::decode_content;
use crate::pdf_inventory::PdfInventorySkip;

/// Combined page inventory plus per-form expansion diagnostics for one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormExpandedInventory {
    /// Page inventory with nested form entries merged after their invocation.
    pub inventory: Inventory,
    /// Structured per-form expansion skips for this page, in content order.
    pub form_skipped: Vec<SkippedFormInventory>,
}

/// One structured Form `XObject` expansion skip.
///
/// The page's own inventory is always produced; this records a page-level (or,
/// for future deeper walks, nested) form whose content could not be inventoried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkippedFormInventory {
    /// Resource name used to invoke the form.
    pub name: PdfName,
    /// Resolved indirect reference of the form stream object.
    pub reference: IndirectRef,
    /// Resolved form stream object byte offset.
    pub object_byte_offset: usize,
    /// Structured reason the form content was not inventoried.
    pub reason: SkippedFormInventoryReason,
}

/// Structured reason a Form `XObject`'s own content was not inventoried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum SkippedFormInventoryReason {
    /// The form re-invokes a form already on the active walk stack (self-ref or
    /// cycle); descending would not terminate.
    Cycle,
    /// The bounded walk reached its configured maximum form nesting depth, so
    /// this nested form was inventoried as an invocation but not descended.
    MaxDepth {
        /// Configured maximum form nesting depth for the walk.
        max_depth: usize,
    },
    /// The page-level total form-expansion budget was exhausted before this
    /// form could be decoded, tokenized, assembled, or inventoried.
    BudgetExhausted {
        /// Configured maximum number of form expansion attempts for one page.
        max_expansions: usize,
    },
    /// The form stream could not be located, decoded, tokenized, assembled, or
    /// walked. Delegates to the shared content-skip vocabulary.
    Content {
        /// Delegated content-processing skip for the form stream.
        skip: PdfInventorySkip,
    },
    /// A nested resource in the form's own `/Resources /XObject` dictionary
    /// could not be classified, so invocations of that resource cannot be
    /// recursively inventoried.
    Resource {
        /// Delegated resource-classification diagnostic.
        skip: SkippedPageXObjectResource,
    },
}

/// Bounded walk context for one page's form expansion.
///
/// `max_depth` bounds form-stream descents from the page. `max_expansions`
/// bounds total form expansion attempts for one page and is consumed before any
/// form stream work begins; it is not restored on ascent. `visited` keys the
/// forms currently on the active descent path by resolved `(object_number,
/// generation)` plus byte offset, so a form that re-invokes an ancestor is
/// detected as a cycle without blocking legitimate sibling re-invocations.
/// Because `visited` is inserted on descent and removed on ascent, its length is
/// the current descent depth.
#[derive(Debug, Clone)]
pub struct FormWalkContext {
    pub(crate) max_depth: usize,
    pub(crate) max_expansions: usize,
    pub(crate) remaining_expansions: usize,
    visited: BTreeSet<FormObjectKey>,
}

impl FormWalkContext {
    /// Create a context bounded to `max_depth` levels of form nesting.
    #[must_use]
    pub const fn new(max_depth: usize) -> Self {
        Self::with_budget(max_depth, 256)
    }

    /// Create a context bounded by nesting depth and total page expansion
    /// attempts.
    #[must_use]
    pub const fn with_budget(max_depth: usize, max_expansions: usize) -> Self {
        Self {
            max_depth,
            max_expansions,
            remaining_expansions: max_expansions,
            visited: BTreeSet::new(),
        }
    }

    /// Create the default bounded context used by the inventory bridges.
    #[must_use]
    pub const fn bounded_default() -> Self {
        Self::new(8)
    }

    /// Create a one-level context for focused tests and compatibility.
    #[must_use]
    pub const fn one_level() -> Self {
        Self::new(1)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FormObjectKey {
    object_number: u32,
    generation: u16,
    object_byte_offset: usize,
}

impl FormObjectKey {
    const fn from_target(target: &PageXObjectResourceTarget) -> Self {
        Self {
            object_number: target.reference.object_number,
            generation: target.reference.generation,
            object_byte_offset: target.object_byte_offset,
        }
    }
}

/// Build a page's combined inventory with bounded Form `XObject` content
/// expansion.
///
/// The page content is decoded, tokenized, assembled, and inventoried through
/// the same page decode/tokenize/assemble path and
/// [`presslint_inventory::build_inventory`] the page-only bridge used. When the
/// page declares no form `XObject` resources this reduces to the page-only path
/// with an empty skip list, so pages without form invocations are byte-for-byte
/// unchanged. Otherwise each page-level form invocation entry is
/// followed by the form's own inventory entries, rebased onto page-global
/// sequence values that continue after the page's sequence space.
///
/// # Errors
///
/// Returns [`InventoryPageSkip`] only for page-level failures (page decode,
/// tokenize, assemble, or graphics walk). Per-form failures are collected as
/// [`SkippedFormInventory`] and never fail the page.
#[allow(clippy::too_many_arguments)]
pub fn build_page_inventory_with_forms(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page: &presslint_pdf::DocumentPageContentExtentInspection,
    page_index: PageIndex,
    max_decoded_stream_bytes: usize,
    page_image_names: &[PdfName],
    page_form_names: &[PdfName],
    form_targets: &[PageXObjectResourceTarget],
    page_color_spaces: &[ColorSpaceResource],
    context: FormWalkContext,
) -> Result<FormExpandedInventory, InventoryPageSkip> {
    let (page_bytes, first_stream_offset) =
        decode_page_content(input, page, max_decoded_stream_bytes)?;
    let source = page_bytes.as_slice();
    let tokens = tokenize(source).map_err(|error| InventoryPageSkip::TokenizeFailed {
        object_byte_offset: first_stream_offset,
        error,
    })?;
    let assembled =
        assemble_operators(&tokens).map_err(|error| InventoryPageSkip::AssembleFailed {
            object_byte_offset: first_stream_offset,
            error,
        })?;

    // The PAGE content stream resolves `cs`/`scn` against the page colour-space
    // environment. Form XObject content (below) keeps its OWN resource
    // environment and must NOT inherit the page one, so each form walk resolves
    // `cs`/`scn` against a LOCAL env built from that form's own
    // `/Resources /ColorSpace` (see `FormExpansion::expand`).
    let page_inventory = build_inventory_with_color_space_env(
        source,
        &assembled.records,
        page_index,
        &ContentScope::Page,
        page_image_names,
        page_form_names,
        ColorSpaceEnv::new(page_color_spaces),
    )
    .map_err(|error| InventoryPageSkip::GraphicsWalkFailed {
        object_byte_offset: first_stream_offset,
        error,
    })?;

    // Fast path: a page with no classified form resources cannot invoke a form,
    // so it needs no second walk and stays identical to the page-only bridge.
    if page_form_names.is_empty() || form_targets.is_empty() {
        return Ok(FormExpandedInventory {
            inventory: page_inventory,
            form_skipped: Vec::new(),
        });
    }

    let invocation_names = form_invocation_names(
        source,
        &assembled.records,
        page_image_names,
        page_form_names,
    )
    .map_err(|error| InventoryPageSkip::GraphicsWalkFailed {
        object_byte_offset: first_stream_offset,
        error,
    })?;

    let mut expansion = FormExpansion {
        input,
        lookup,
        page_index,
        max_decoded_stream_bytes,
        context,
        next_sequence: usize_to_u32(page_inventory.len()),
        entries: Vec::with_capacity(page_inventory.len()),
        skipped: Vec::new(),
    };
    let mut invocation_iter = invocation_names.into_iter();
    for entry in page_inventory.entries {
        let is_form = entry.kind == ObjectKind::FormXObject;
        expansion.entries.push(entry);
        if is_form {
            if let Some(name) = invocation_iter.next() {
                if let Some(target) = find_form_target(form_targets, &name) {
                    expansion.expand(&name, target);
                }
            }
        }
    }

    Ok(FormExpandedInventory {
        inventory: Inventory {
            entries: expansion.entries,
        },
        form_skipped: expansion.skipped,
    })
}

struct FormExpansion<'input> {
    input: &'input [u8],
    lookup: ObjectLookup<'input>,
    page_index: PageIndex,
    max_decoded_stream_bytes: usize,
    context: FormWalkContext,
    next_sequence: u32,
    entries: Vec<InventoryEntry>,
    skipped: Vec<SkippedFormInventory>,
}

impl<'input> FormExpansion<'input> {
    /// Expand one form invocation, merging its own inventory entries (rebased
    /// onto the page-global sequence space) after the current position.
    fn expand(&mut self, name: &PdfName, target: &PageXObjectResourceTarget) {
        let key = FormObjectKey::from_target(target);
        if self.context.visited.contains(&key) {
            self.push_skip(name, target, SkippedFormInventoryReason::Cycle);
            return;
        }
        if self.context.visited.len() >= self.context.max_depth {
            self.push_skip(
                name,
                target,
                SkippedFormInventoryReason::MaxDepth {
                    max_depth: self.context.max_depth,
                },
            );
            return;
        }
        if !self.consume_expansion_budget(name, target) {
            return;
        }

        let Some((source, records)) = self.decode_form(name, target) else {
            return;
        };
        let source = source.as_slice();

        let form_resources =
            inspect_form_xobject_resources(self.input, self.lookup, target.object_byte_offset);
        for skip in form_resources
            .skipped
            .iter()
            .filter(|skip| is_form_resource_coverage_skip(skip))
        {
            self.push_skip(
                name,
                target,
                SkippedFormInventoryReason::Resource { skip: skip.clone() },
            );
        }
        let image_names = inventory_names(&form_resources.image_xobject_names);
        let form_names = inventory_names(&form_resources.form_xobject_names);
        let scope = ContentScope::FormXObject { name: name.clone() };

        // The form paints against its OWN `/Resources /ColorSpace` only (ISO
        // 32000-1 §7.8.3 + §8.10.2 Table 95): build a LOCAL colour-space env from
        // the form's own classified spaces and resolve this form's `cs`/`scn`
        // against it. No page-scope inheritance — a form with no `/ColorSpace`
        // gets an empty env, so its `cs CS0` stays `Resource(CS0)`. The env
        // borrows into `form_color_spaces`, which lives only for this walk and is
        // dropped when `expand` returns; it is never merged across the
        // page/form/nested-form boundary. Colour-space skips stay in the
        // `presslint-pdf` report (no `SkippedFormInventory` plumbing this slice).
        let form_color_spaces = color_space_env_resources(
            &inspect_form_color_space_resources(self.input, self.lookup, target.object_byte_offset)
                .color_spaces,
        );

        let form_inventory = match build_inventory_with_color_space_env(
            source,
            &records,
            self.page_index,
            &scope,
            &image_names,
            &form_names,
            ColorSpaceEnv::new(&form_color_spaces),
        ) {
            Ok(inventory) => inventory,
            Err(error) => {
                self.push_content_skip(
                    name,
                    target,
                    PdfInventorySkip::GraphicsWalkFailed {
                        object_byte_offset: target.object_byte_offset,
                        error,
                    },
                );
                return;
            }
        };
        let nested_names = if form_names.is_empty() {
            Vec::new()
        } else {
            match form_invocation_names(source, &records, &image_names, &form_names) {
                Ok(names) => names,
                Err(error) => {
                    self.push_content_skip(
                        name,
                        target,
                        PdfInventorySkip::GraphicsWalkFailed {
                            object_byte_offset: target.object_byte_offset,
                            error,
                        },
                    );
                    return;
                }
            }
        };

        self.context.visited.insert(key);
        let mut nested_iter = nested_names.into_iter();
        for mut entry in form_inventory.entries {
            let is_form = entry.kind == ObjectKind::FormXObject;
            entry.id.sequence = self.next_sequence;
            self.next_sequence = self.next_sequence.saturating_add(1);
            self.entries.push(entry);
            if is_form {
                if let Some(nested_name) = nested_iter.next() {
                    if let Some(nested_target) =
                        find_form_target(&form_resources.form_xobjects, &nested_name)
                    {
                        self.expand(&nested_name, nested_target);
                    }
                }
            }
        }
        self.context.visited.remove(&key);
    }

    fn consume_expansion_budget(
        &mut self,
        name: &PdfName,
        target: &PageXObjectResourceTarget,
    ) -> bool {
        if self.context.remaining_expansions == 0 {
            self.push_skip(
                name,
                target,
                SkippedFormInventoryReason::BudgetExhausted {
                    max_expansions: self.context.max_expansions,
                },
            );
            return false;
        }
        self.context.remaining_expansions -= 1;
        true
    }

    /// Locate, decode, tokenize, and assemble a form stream through the shared
    /// page filter/decode machinery, recording a structured skip on failure.
    fn decode_form(
        &mut self,
        name: &PdfName,
        target: &PageXObjectResourceTarget,
    ) -> Option<(
        crate::page_content::PageContentBytes<'input>,
        Vec<presslint_syntax::OperatorRecord>,
    )> {
        let extent = match inspect_content_stream_data_extent_with_lookup(
            self.input,
            Some(self.lookup),
            target.object_byte_offset,
        ) {
            Ok(extent) => extent,
            Err(error) => {
                self.push_content_skip(
                    name,
                    target,
                    PdfInventorySkip::ExtentFailed {
                        object_byte_offset: target.object_byte_offset,
                        error,
                    },
                );
                return None;
            }
        };
        let content = match decode_content(
            self.input,
            target.object_byte_offset,
            &extent,
            self.max_decoded_stream_bytes,
        ) {
            Ok(content) => content,
            Err(skip) => {
                self.push_content_skip(name, target, skip.into());
                return None;
            }
        };
        let tokens = match tokenize(content.as_slice()) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.push_content_skip(
                    name,
                    target,
                    PdfInventorySkip::TokenizeFailed {
                        object_byte_offset: target.object_byte_offset,
                        error,
                    },
                );
                return None;
            }
        };
        let assembled = match assemble_operators(&tokens) {
            Ok(assembled) => assembled,
            Err(error) => {
                self.push_content_skip(
                    name,
                    target,
                    PdfInventorySkip::AssembleFailed {
                        object_byte_offset: target.object_byte_offset,
                        error,
                    },
                );
                return None;
            }
        };
        Some((content, assembled.records))
    }

    fn push_content_skip(
        &mut self,
        name: &PdfName,
        target: &PageXObjectResourceTarget,
        skip: PdfInventorySkip,
    ) {
        self.push_skip(name, target, SkippedFormInventoryReason::Content { skip });
    }

    fn push_skip(
        &mut self,
        name: &PdfName,
        target: &PageXObjectResourceTarget,
        reason: SkippedFormInventoryReason,
    ) {
        self.skipped.push(SkippedFormInventory {
            name: name.clone(),
            reference: target.reference,
            object_byte_offset: target.object_byte_offset,
            reason,
        });
    }
}

/// Collect, in content order, the resource names of form `Do` invocations that
/// [`presslint_inventory::build_inventory_with_color_space_env`] classifies as
/// form entries: an `XObject` invocation whose name is in `form_names` and not
/// in `image_names` (image classification wins).
///
/// The returned names align one-to-one with the `FormXObject`-kind entries
/// the inventory builder emits over the same records, so callers can pair each
/// form invocation entry with its invoking name.
fn form_invocation_names(
    source: &[u8],
    records: &[presslint_syntax::OperatorRecord],
    image_names: &[PdfName],
    form_names: &[PdfName],
) -> Result<Vec<PdfName>, GraphicsWalkError> {
    let events = walk_graphics_state(source, records)?;
    Ok(events
        .into_iter()
        .filter_map(|event| match event.kind {
            PaintOpKind::XObjectInvoke { name }
                if form_names.contains(&name) && !image_names.contains(&name) =>
            {
                Some(name)
            }
            _ => None,
        })
        .collect())
}

fn find_form_target<'a>(
    targets: &'a [PageXObjectResourceTarget],
    name: &PdfName,
) -> Option<&'a PageXObjectResourceTarget> {
    targets.iter().find(|target| target.name.0 == name.0)
}

const fn is_form_resource_coverage_skip(skip: &SkippedPageXObjectResource) -> bool {
    !matches!(
        skip.reason,
        SkippedPageXObjectResourceReason::MissingResources
            | SkippedPageXObjectResourceReason::MissingXObject
    )
}

fn usize_to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
