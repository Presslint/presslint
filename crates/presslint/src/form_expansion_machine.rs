//! Parallel machine-driven Form `XObject` expansion adapter.
//!
//! This module is intentionally not the public entry point yet. It drives the
//! paint call machine beside the existing recursive `FormExpansion` path so
//! tests can prove bit-for-bit equality before the later flip.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::sync::OnceLock;

use presslint_inventory::{Inventory, InventoryEntry, build_inventory_with_color_space_env};
use presslint_paint::{
    CallSite, ColorSpaceEnv, FormResolver, GraphicsWalkError, PaintSubProgram, ResolveForm,
    flat_call_events,
};
use presslint_pdf::{
    ObjectLookup, PageXObjectResourceTarget, SkippedPageXObjectResource,
    SkippedPageXObjectResourceReason, inspect_content_stream_data_extent_with_lookup,
    inspect_form_color_space_resources, inspect_form_xobject_resources,
};
use presslint_syntax::{OperatorRecord, assemble_operators, tokenize};
use presslint_types::{ContentScope, InvocationFrame, InvocationPath, PageIndex, PdfName};

use crate::document_inventory::{
    InventoryPageSkip, color_space_env_resources, decode_page_content, inventory_names,
};
use crate::form_inventory::{
    FormExpandedInventory, FormWalkContext, SkippedFormInventory, SkippedFormInventoryReason,
};
use crate::page_content::{PageContentBytes, decode_content};
use crate::pdf_inventory::PdfInventorySkip;

struct FormProgramArena<'input> {
    programs: Vec<PreparedFormObject<'input>>,
}

impl<'input> FormProgramArena<'input> {
    fn build(
        input: &'input [u8],
        lookup: ObjectLookup<'input>,
        targets: &[PageXObjectResourceTarget],
    ) -> Self {
        let mut arena = Self {
            programs: Vec::new(),
        };
        let mut seen = BTreeSet::new();
        for target in targets {
            arena.insert_reachable(input, lookup, target, &mut seen);
        }
        arena
    }

    fn insert_reachable(
        &mut self,
        input: &'input [u8],
        lookup: ObjectLookup<'input>,
        target: &PageXObjectResourceTarget,
        seen: &mut BTreeSet<FormObjectKey>,
    ) {
        let key = FormObjectKey::from_target(target);
        if !seen.insert(key) {
            return;
        }
        let nested_targets =
            inspect_form_xobject_resources(input, lookup, target.object_byte_offset).form_xobjects;
        self.programs.push(PreparedFormObject {
            key,
            target: target.clone(),
            program: OnceLock::new(),
        });
        for nested in nested_targets {
            self.insert_reachable(input, lookup, &nested, seen);
        }
    }

    fn get(&self, key: FormObjectKey) -> Option<&PreparedFormObject<'input>> {
        self.programs.iter().find(|program| program.key == key)
    }
}

struct PreparedFormObject<'input> {
    key: FormObjectKey,
    target: PageXObjectResourceTarget,
    program: OnceLock<PreparedFormProgram<'input>>,
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
    form_targets: Vec<PageXObjectResourceTarget>,
    resource_skips: Vec<SkippedPageXObjectResource>,
    color_spaces: Vec<presslint_inventory::ColorSpaceResource>,
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

#[allow(clippy::too_many_arguments)]
pub fn build_page_inventory_with_forms_machine(
    input: &[u8],
    lookup: ObjectLookup<'_>,
    page: &presslint_pdf::DocumentPageContentExtentInspection,
    page_index: PageIndex,
    max_decoded_stream_bytes: usize,
    page_image_names: &[PdfName],
    page_form_names: &[PdfName],
    form_targets: &[PageXObjectResourceTarget],
    page_color_spaces: &[presslint_inventory::ColorSpaceResource],
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

    let arena = FormProgramArena::build(input, lookup, form_targets);
    let root = PaintSubProgram {
        source,
        records: &assembled.records,
        color_space_env: ColorSpaceEnv::new(page_color_spaces),
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
        page_form_targets: form_targets,
        page_index,
        context,
        visited: BTreeSet::new(),
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
    page_form_targets: &'arena [PageXObjectResourceTarget],
    page_index: PageIndex,
    context: FormWalkContext,
    visited: BTreeSet<FormObjectKey>,
    active: Vec<(InvocationPath, FormObjectKey)>,
    prepared_paths: Rc<RefCell<Vec<(InvocationPath, Inventory)>>>,
    skipped: Vec<SkippedFormInventory>,
}

impl UmbrellaFormResolver<'_, '_> {
    fn caller_targets(&self, path: &InvocationPath) -> &[PageXObjectResourceTarget] {
        if path.frames.is_empty() {
            return self.page_form_targets;
        }
        let Some(key) = self
            .active
            .iter()
            .find_map(|(candidate, key)| (candidate == path).then_some(*key))
        else {
            return &[];
        };
        let Some(prepared) = self.arena.get(key) else {
            return &[];
        };
        let Some(PreparedFormProgram::Ready(data)) = prepared.program.get() else {
            return &[];
        };
        &data.form_targets
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
        let Some(target) =
            find_form_target(self.caller_targets(call.caller_path), call.name).cloned()
        else {
            return Ok(ResolveForm::Skip);
        };
        let key = FormObjectKey::from_target(&target);
        if self.visited.contains(&key) {
            self.push_skip(call.name, &target, SkippedFormInventoryReason::Cycle);
            return Ok(ResolveForm::Skip);
        }
        if self.active.len() >= self.context.max_depth {
            self.push_skip(
                call.name,
                &target,
                SkippedFormInventoryReason::MaxDepth {
                    max_depth: self.context.max_depth,
                },
            );
            return Ok(ResolveForm::Skip);
        }
        if self.context.remaining_expansions == 0 {
            self.push_skip(
                call.name,
                &target,
                SkippedFormInventoryReason::BudgetExhausted {
                    max_expansions: self.context.max_expansions,
                },
            );
            return Ok(ResolveForm::Skip);
        }
        self.context.remaining_expansions -= 1;

        let Some(prepared) = self.arena.get(key) else {
            return Ok(ResolveForm::Skip);
        };
        let program = prepared.program.get_or_init(|| {
            prepare_form_object(
                self.input,
                self.lookup,
                &prepared.target,
                self.max_decoded_stream_bytes,
            )
        });
        let data = match program {
            PreparedFormProgram::Ready(data) => data,
            PreparedFormProgram::ContentSkipped(skip) => {
                self.push_content_skip(call.name, &target, skip.clone());
                return Ok(ResolveForm::Skip);
            }
        };

        for skip in &data.resource_skips {
            self.push_skip(
                call.name,
                &target,
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
                    &target,
                    PdfInventorySkip::GraphicsWalkFailed {
                        object_byte_offset: target.object_byte_offset,
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
        self.visited.insert(key);
        self.active.push((child_path, key));
        Ok(ResolveForm::Descend(PaintSubProgram {
            source: data.content.as_slice(),
            records: &data.records,
            color_space_env: ColorSpaceEnv::new(&data.color_spaces),
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
            let (_, key) = self.active.remove(index);
            self.visited.remove(&key);
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
        let Some(entry) = inventory.entries.get(cursor) else {
            return;
        };
        if entry.provenance.range != range {
            return;
        }
        let mut entry = entry.clone();
        entry.id.sequence = self.next_sequence;
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
    PreparedFormProgram::Ready(FormProgramData {
        content,
        records: assembled.records,
        image_names: inventory_names(&form_resources.image_xobject_names),
        form_names: inventory_names(&form_resources.form_xobject_names),
        form_targets: form_resources.form_xobjects,
        resource_skips,
        color_spaces,
    })
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
