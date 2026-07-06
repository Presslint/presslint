//! Machine-driven Form `XObject` expansion: the production form-expansion path.
//!
//! This module drives the shared paint call/return machine
//! ([`presslint_paint::flat_call_events`]) to expand a page's Form `XObject`
//! invocations. It owns the public entry [`build_page_inventory_with_forms`]
//! (re-exported through [`crate::form_inventory`], which owns the caller-facing
//! result and diagnostic types). The traversal is the single source of
//! traversal truth: the umbrella no longer re-walks graphics state to pair
//! invocation names.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

use presslint_inventory::{
    Inventory, InventoryEntry, build_inventory_with_color_space_env, expanded_entry_identity,
};
use presslint_paint::{
    CallSite, ColorSpaceEnv, ExtGStateEnv, FormResolver, GraphicsWalkError, PaintSubProgram,
    ResolveForm, flat_call_events,
};
use presslint_pdf::{
    ObjectLookup, PageXObjectResourceTarget, SkippedPageXObjectResource,
    SkippedPageXObjectResourceReason, inspect_content_stream_data_extent_with_lookup,
    inspect_form_color_space_resources, inspect_form_extgstate_resources,
    inspect_form_xobject_resources,
};
use presslint_syntax::{OperatorRecord, assemble_operators, tokenize};
use presslint_types::{ContentScope, InvocationFrame, InvocationPath, PageIndex, PdfName};

use crate::document_inventory::{
    InventoryPageSkip, color_space_env_resources, decode_page_content, extgstate_env_resources,
    inventory_names,
};
use crate::form_inventory::{
    FormExpandedInventory, FormWalkContext, SkippedFormInventory, SkippedFormInventoryReason,
};
use crate::page_content::{PageContentBytes, decode_content};
use crate::pdf_inventory::PdfInventorySkip;

/// Lazy tree of form sub-programs, rooted at the page's form resource slots.
///
/// The arena holds only the page-level slots up front: [`build`] clones the
/// caller-supplied targets and inspects NOTHING. Each form's own content is
/// decoded and its resources inspected on demand, the first time a permitted
/// invocation actually resolves it (the `OnceLock` in [`PreparedFormObject`]),
/// so a declared-but-never-invoked, over-budget, or over-depth form is never
/// touched. Nested slots grow lazily the same way: preparing a form materialises
/// empty child slots for its own declared form resources, and each is prepared
/// only if it is in turn invoked and passes the resolver's cycle/depth/budget
/// checks. This mirrors the bounded lazy semantics of the retired manual
/// recursion — no eager structural walk of the declared form graph.
///
/// [`build`]: FormProgramArena::build
struct FormProgramArena<'input> {
    roots: Vec<PreparedFormObject<'input>>,
}

impl FormProgramArena<'_> {
    fn build(targets: &[PageXObjectResourceTarget]) -> Self {
        Self {
            roots: targets.iter().map(PreparedFormObject::new).collect(),
        }
    }
}

struct PreparedFormObject<'input> {
    key: FormObjectKey,
    target: PageXObjectResourceTarget,
    program: OnceLock<PreparedFormProgram<'input>>,
}

impl PreparedFormObject<'_> {
    fn new(target: &PageXObjectResourceTarget) -> Self {
        Self {
            key: FormObjectKey::from_target(target),
            target: target.clone(),
            program: OnceLock::new(),
        }
    }
}

enum PreparedFormProgram<'input> {
    Ready(FormProgramData<'input>),
    ContentSkipped(PdfInventorySkip),
}

struct FormProgramData<'input> {
    content: PageContentBytes<'input>,
    records: Vec<OperatorRecord>,
    image_names: Vec<PdfName>,
    form_names: Vec<PdfName>,
    nested: Vec<PreparedFormObject<'input>>,
    resource_skips: Vec<SkippedPageXObjectResource>,
    color_spaces: Vec<presslint_inventory::ColorSpaceResource>,
    extgstates: Vec<presslint_inventory::ExtGStateResource>,
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
/// unchanged. Otherwise each page-level form invocation entry is followed by the
/// form's own inventory entries, whose identity is built born-final: the
/// page-global sequence (continuing after the page's sequence space) and the
/// invocation path are folded into the digest at construction, not rebased
/// afterwards.
///
/// Form `XObject` content keeps its OWN resource environment and must NOT
/// inherit the page one, so each form walk resolves `cs`/`scn` against a LOCAL
/// colour-space env AND `gs` against a LOCAL `ExtGState` env, both built from
/// that form's own `/Resources` and never merged with the page's.
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
    page_color_spaces: &[presslint_inventory::ColorSpaceResource],
    page_extgstates: &[presslint_inventory::ExtGStateResource],
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

    if page_form_names.is_empty() || form_targets.is_empty() {
        return Ok(FormExpandedInventory {
            inventory: page_inventory,
            form_skipped: Vec::new(),
        });
    }

    let arena = FormProgramArena::build(form_targets);
    let root = PaintSubProgram {
        source,
        records: &assembled.records,
        color_space_env: ColorSpaceEnv::new(page_color_spaces),
        extgstate_env: ExtGStateEnv::new(page_extgstates),
        image_xobject_names: page_image_names,
        form_xobject_names: page_form_names,
        scope: ContentScope::Page,
    };
    let prepared_paths = Rc::new(RefCell::new(Vec::new()));
    let mut resolver = UmbrellaFormResolver {
        arena: &arena,
        input,
        lookup,
        max_decoded_stream_bytes,
        page_index,
        context,
        active: Vec::new(),
        prepared_paths: Rc::clone(&prepared_paths),
        skipped: Vec::new(),
    };
    let mut adapter = MachineInventoryAdapter::new(page_inventory, Rc::clone(&prepared_paths));
    let mut walk_error = None;
    flat_call_events(root, &mut resolver, |item| match item {
        Ok(flat) => adapter.accept(flat.path, flat.op),
        Err(error) => walk_error = Some(error),
    });
    if let Some(error) = walk_error {
        return Err(InventoryPageSkip::GraphicsWalkFailed {
            object_byte_offset: first_stream_offset,
            error,
        });
    }

    Ok(FormExpandedInventory {
        inventory: Inventory {
            entries: adapter.entries,
        },
        form_skipped: resolver.skipped,
    })
}

struct UmbrellaFormResolver<'arena, 'input> {
    arena: &'arena FormProgramArena<'input>,
    input: &'input [u8],
    lookup: ObjectLookup<'input>,
    max_decoded_stream_bytes: usize,
    page_index: PageIndex,
    context: FormWalkContext,
    active: Vec<(InvocationPath, &'arena PreparedFormObject<'input>)>,
    prepared_paths: Rc<RefCell<Vec<(InvocationPath, Inventory)>>>,
    skipped: Vec<SkippedFormInventory>,
}

impl<'arena, 'input> UmbrellaFormResolver<'arena, 'input> {
    /// The form slots reachable from the program identified by `path`: the page's
    /// root slots for the page itself, otherwise the caller form's own lazily
    /// materialised child slots. The child slots live inside the caller's
    /// `OnceLock`-prepared data, which is stable for `'arena`, so the borrow
    /// outlives this call as the machine requires.
    fn caller_slots(&self, path: &InvocationPath) -> &'arena [PreparedFormObject<'input>] {
        if path.frames.is_empty() {
            return &self.arena.roots;
        }
        let Some(caller) = self
            .active
            .iter()
            .find_map(|(candidate, slot)| (candidate == path).then_some(*slot))
        else {
            return &[];
        };
        let Some(PreparedFormProgram::Ready(data)) = caller.program.get() else {
            return &[];
        };
        &data.nested
    }

    fn child_path(call: CallSite<'_>) -> InvocationPath {
        let mut path = call.caller_path.clone();
        path.frames.push(InvocationFrame {
            ordinal: call.ordinal,
            name: call.name.clone(),
        });
        path
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

    fn push_content_skip(
        &mut self,
        name: &PdfName,
        target: &PageXObjectResourceTarget,
        skip: PdfInventorySkip,
    ) {
        self.push_skip(name, target, SkippedFormInventoryReason::Content { skip });
    }
}

impl<'arena> FormResolver<'arena> for UmbrellaFormResolver<'arena, '_> {
    fn resolve_form(
        &mut self,
        call: CallSite<'_>,
    ) -> Result<ResolveForm<'arena>, GraphicsWalkError> {
        let Some(slot) = find_slot(self.caller_slots(call.caller_path), call.name) else {
            return Ok(ResolveForm::Skip);
        };
        let key = slot.key;
        // Active-path cycle detection: the same form object (by key) is already
        // on the descent stack. The stack is bounded by `max_depth`, so a linear
        // scan is its own source of truth — no separate visited index.
        if self.active.iter().any(|(_, active)| active.key == key) {
            self.push_skip(call.name, &slot.target, SkippedFormInventoryReason::Cycle);
            return Ok(ResolveForm::Skip);
        }
        if self.active.len() >= self.context.max_depth {
            self.push_skip(
                call.name,
                &slot.target,
                SkippedFormInventoryReason::MaxDepth {
                    max_depth: self.context.max_depth,
                },
            );
            return Ok(ResolveForm::Skip);
        }
        if self.context.remaining_expansions == 0 {
            self.push_skip(
                call.name,
                &slot.target,
                SkippedFormInventoryReason::BudgetExhausted {
                    max_expansions: self.context.max_expansions,
                },
            );
            return Ok(ResolveForm::Skip);
        }
        self.context.remaining_expansions -= 1;

        // The form's own content is decoded and its resources inspected here, the
        // first time a permitted invocation reaches it — never for declared-but-
        // uninvoked, over-budget, or over-depth forms.
        let program = slot.program.get_or_init(|| {
            prepare_form_object(
                self.input,
                self.lookup,
                &slot.target,
                self.max_decoded_stream_bytes,
            )
        });
        let data = match program {
            PreparedFormProgram::Ready(data) => data,
            PreparedFormProgram::ContentSkipped(skip) => {
                self.push_content_skip(call.name, &slot.target, skip.clone());
                return Ok(ResolveForm::Skip);
            }
        };

        for skip in &data.resource_skips {
            self.push_skip(
                call.name,
                &slot.target,
                SkippedFormInventoryReason::Resource { skip: skip.clone() },
            );
        }
        let scope = ContentScope::FormXObject {
            name: call.name.clone(),
        };
        let inventory = match build_inventory_with_color_space_env(
            data.content.as_slice(),
            &data.records,
            self.page_index,
            &scope,
            &data.image_names,
            &data.form_names,
            ColorSpaceEnv::new(&data.color_spaces),
        ) {
            Ok(inventory) => inventory,
            Err(error) => {
                self.push_content_skip(
                    call.name,
                    &slot.target,
                    PdfInventorySkip::GraphicsWalkFailed {
                        object_byte_offset: slot.target.object_byte_offset,
                        error,
                    },
                );
                return Ok(ResolveForm::Skip);
            }
        };
        let child_path = Self::child_path(call);
        self.prepared_paths
            .borrow_mut()
            .push((child_path.clone(), inventory));
        self.active.push((child_path, slot));
        Ok(ResolveForm::Descend(PaintSubProgram {
            source: data.content.as_slice(),
            records: &data.records,
            color_space_env: ColorSpaceEnv::new(&data.color_spaces),
            extgstate_env: ExtGStateEnv::new(&data.extgstates),
            image_xobject_names: &data.image_names,
            form_xobject_names: &data.form_names,
            scope,
        }))
    }

    fn on_return(&mut self, path: &InvocationPath) {
        if let Some(index) = self
            .active
            .iter()
            .rposition(|(candidate, _)| candidate == path)
        {
            self.active.remove(index);
        }
    }
}

#[derive(Debug)]
struct MachineInventoryAdapter {
    page_entries: Vec<InventoryEntry>,
    page_cursor: usize,
    form_cursors: Vec<(InvocationPath, usize)>,
    prepared_paths: Rc<RefCell<Vec<(InvocationPath, Inventory)>>>,
    entries: Vec<InventoryEntry>,
    next_sequence: u32,
}

impl MachineInventoryAdapter {
    fn new(
        page_inventory: Inventory,
        prepared_paths: Rc<RefCell<Vec<(InvocationPath, Inventory)>>>,
    ) -> Self {
        let next_sequence = usize_to_u32(page_inventory.len());
        Self {
            page_entries: page_inventory.entries,
            page_cursor: 0,
            form_cursors: Vec::new(),
            prepared_paths,
            entries: Vec::new(),
            next_sequence,
        }
    }

    fn accept(&mut self, path: &InvocationPath, op: &presslint_paint::PaintOp) {
        let range = Some(op.record_range.into_byte_range());
        if path.frames.is_empty() {
            if self
                .page_entries
                .get(self.page_cursor)
                .is_some_and(|entry| entry.provenance.range == range)
            {
                self.entries
                    .push(self.page_entries[self.page_cursor].clone());
                self.page_cursor += 1;
            }
            return;
        }

        let cursor_index = self.cursor_index(path);
        let cursor = self.form_cursors[cursor_index].1;
        let prepared_paths = self.prepared_paths.borrow();
        let Some((_, inventory)) = prepared_paths
            .iter()
            .find(|(candidate, _)| candidate == path)
        else {
            return;
        };
        let Some(template) = inventory.entries.get(cursor) else {
            return;
        };
        if template.provenance.range != range {
            return;
        }
        // Born-final identity: build the entry once with the FINAL page-global
        // sequence and the machine's invocation `path` folded into the digest.
        // `path` is the same object published as `provenance.invocation`, so the
        // identity never carries a sequence or path that contradicts its digest
        // (the deleted post-hoc `id.sequence` rebase).
        let entry = expanded_entry_identity(template, self.next_sequence, path, op);
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.entries.push(entry);
        self.form_cursors[cursor_index].1 += 1;
    }

    fn cursor_index(&mut self, path: &InvocationPath) -> usize {
        if let Some(index) = self
            .form_cursors
            .iter()
            .position(|(candidate, _)| candidate == path)
        {
            return index;
        }
        self.form_cursors.push((path.clone(), 0));
        self.form_cursors.len() - 1
    }
}

fn prepare_form_object<'input>(
    input: &'input [u8],
    lookup: ObjectLookup<'input>,
    target: &PageXObjectResourceTarget,
    max_decoded_stream_bytes: usize,
) -> PreparedFormProgram<'input> {
    let extent = match inspect_content_stream_data_extent_with_lookup(
        input,
        Some(lookup),
        target.object_byte_offset,
    ) {
        Ok(extent) => extent,
        Err(error) => {
            return PreparedFormProgram::ContentSkipped(PdfInventorySkip::ExtentFailed {
                object_byte_offset: target.object_byte_offset,
                error,
            });
        }
    };
    let content = match decode_content(
        input,
        target.object_byte_offset,
        &extent,
        max_decoded_stream_bytes,
    ) {
        Ok(content) => content,
        Err(skip) => return PreparedFormProgram::ContentSkipped(skip.into()),
    };
    let tokens = match tokenize(content.as_slice()) {
        Ok(tokens) => tokens,
        Err(error) => {
            return PreparedFormProgram::ContentSkipped(PdfInventorySkip::TokenizeFailed {
                object_byte_offset: target.object_byte_offset,
                error,
            });
        }
    };
    let assembled = match assemble_operators(&tokens) {
        Ok(assembled) => assembled,
        Err(error) => {
            return PreparedFormProgram::ContentSkipped(PdfInventorySkip::AssembleFailed {
                object_byte_offset: target.object_byte_offset,
                error,
            });
        }
    };
    let form_resources = inspect_form_xobject_resources(input, lookup, target.object_byte_offset);
    let resource_skips = form_resources
        .skipped
        .iter()
        .filter(|skip| is_form_resource_coverage_skip(skip))
        .cloned()
        .collect();
    let color_spaces = color_space_env_resources(
        &inspect_form_color_space_resources(input, lookup, target.object_byte_offset).color_spaces,
    );
    // Form-LOCAL `ExtGState` env: the form's own `/Resources /ExtGState` only, no
    // page inheritance — the same rule as the colour-space env above.
    let form_extgstates =
        inspect_form_extgstate_resources(input, lookup, target.object_byte_offset);
    let extgstates = extgstate_env_resources(&form_extgstates.extgstates, &form_extgstates.skipped);
    // Materialise empty child slots for this form's own declared form resources.
    // They are inspected/decoded only if a nested invocation in turn reaches
    // them, keeping the whole descent lazy and bounded.
    let nested = form_resources
        .form_xobjects
        .iter()
        .map(PreparedFormObject::new)
        .collect();
    PreparedFormProgram::Ready(FormProgramData {
        content,
        records: assembled.records,
        image_names: inventory_names(&form_resources.image_xobject_names),
        form_names: inventory_names(&form_resources.form_xobject_names),
        nested,
        resource_skips,
        color_spaces,
        extgstates,
    })
}

fn find_slot<'a, 'input>(
    slots: &'a [PreparedFormObject<'input>],
    name: &PdfName,
) -> Option<&'a PreparedFormObject<'input>> {
    slots.iter().find(|slot| slot.target.name.0 == name.0)
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
