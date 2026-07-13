use std::rc::Rc;

use presslint_syntax::OperatorRecord;
use presslint_types::{
    BoundingBox, ColorObservation, ColorSpace, ColorUsage, ContentScope, EditCapability,
    InvocationPath, ObjectId, ObjectKind, PageIndex, PdfName, Provenance,
};
use serde::{Deserialize, Serialize};

use crate::digest::{
    form_object_digest, image_object_digest, text_object_digest, usize_to_u32, vector_object_digest,
};
use presslint_paint::{
    ColorSpaceEnv, ExtGStateEnv, GraphicsStateSnapshot, GraphicsWalkError, PaintOp, PaintOpKind,
    PaintOps, PaintProgram, PathPaintKind, TextRenderingMode,
};

/// The empty invocation path folded into every single-stream (page or form)
/// digest built through the public builders below. Form expansion supplies a
/// non-empty path through [`expanded_entry_identity`] instead.
const NO_INVOCATION: InvocationPath = InvocationPath { frames: Vec::new() };

/// One queryable page object discovered by the inventory pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InventoryEntry {
    /// Stable object identity.
    pub id: ObjectId,
    /// Object class.
    pub kind: ObjectKind,
    /// Source location that enables later action planning.
    pub provenance: Provenance,
    /// Optional object bounds.
    pub bounds: Option<BoundingBox>,
    /// Color observations associated with the object.
    pub colors: Vec<ColorObservation>,
    /// Edit capabilities known at inventory time.
    pub capabilities: Vec<EditCapability>,
}

/// Deterministic document inventory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    /// Entries in page order and then content order.
    pub entries: Vec<InventoryEntry>,
}

impl Inventory {
    /// Return the number of entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return true when no entries were discovered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Build vector inventory entries from assembled content-stream operators.
///
/// This slice records path paint operations that actually paint. Geometry is
/// intentionally not inferred yet, so vector bounds are left unset.
///
/// # Errors
///
/// Returns a structured graphics-state walker error for malformed records in
/// the supported operator set or invalid source ranges.
pub fn build_vector_inventory(
    source: &[u8],
    records: &[OperatorRecord],
    page: PageIndex,
    scope: &ContentScope,
) -> Result<Inventory, GraphicsWalkError> {
    collect_entries_streaming(
        source,
        records,
        ColorSpaceEnv::empty(),
        |event, sequence| vector_entry(page, scope, event, sequence),
    )
}

/// Build text inventory entries from assembled content-stream operators.
///
/// This slice recognizes text-showing operators and records the active text
/// rendering mode. It does not decode glyph strings or infer text geometry.
///
/// # Errors
///
/// Returns a structured graphics-state walker error for malformed records in
/// the supported operator set or invalid source ranges.
pub fn build_text_inventory(
    source: &[u8],
    records: &[OperatorRecord],
    page: PageIndex,
    scope: &ContentScope,
) -> Result<Inventory, GraphicsWalkError> {
    collect_entries_streaming(
        source,
        records,
        ColorSpaceEnv::empty(),
        |event, sequence| text_entry(page, scope, event, sequence),
    )
}

/// Build image inventory entries from assembled content-stream operators.
///
/// This slice recognizes `Do` `XObject` invocations but emits image entries
/// only for resource names the caller has already classified as image
/// `XObjects`.
/// Resource dictionaries, image streams, filters, and bounds are intentionally
/// not inspected here.
///
/// # Errors
///
/// Returns a structured graphics-state walker error for malformed records in
/// the supported operator set or invalid source ranges.
pub fn build_image_inventory(
    source: &[u8],
    records: &[OperatorRecord],
    page: PageIndex,
    scope: &ContentScope,
    image_xobject_names: &[PdfName],
) -> Result<Inventory, GraphicsWalkError> {
    collect_entries_streaming(
        source,
        records,
        ColorSpaceEnv::empty(),
        |event, sequence| image_entry(page, scope, event, image_xobject_names, sequence),
    )
}

/// Build form `XObject` invocation inventory entries from assembled
/// content-stream operators.
///
/// This slice recognizes `Do` `XObject` invocations but emits form entries
/// only for resource names the caller has already classified as form
/// `XObjects`.
/// Resource dictionaries, nested form streams, bounds, and colors are
/// intentionally not inspected here.
///
/// # Errors
///
/// Returns a structured graphics-state walker error for malformed records in
/// the supported operator set or invalid source ranges.
pub fn build_form_inventory(
    source: &[u8],
    records: &[OperatorRecord],
    page: PageIndex,
    scope: &ContentScope,
    form_xobject_names: &[PdfName],
) -> Result<Inventory, GraphicsWalkError> {
    collect_entries_streaming(
        source,
        records,
        ColorSpaceEnv::empty(),
        |event, sequence| form_entry(page, scope, event, form_xobject_names, sequence),
    )
}

/// Build a combined page-object inventory from assembled content-stream
/// operators.
///
/// This is the consolidation of the four per-kind slices: it walks the
/// graphics-state events exactly once and merges vector, text, image, and form
/// entries into a single `Inventory` in content (event) order, assigning one
/// monotonic `sequence` shared across all kinds.
///
/// `image_xobject_names` and `form_xobject_names` must be disjoint by contract.
/// If a `Do` name appears in both lists, the image classification wins.
///
/// See [`inventory_from_graphics_events`] for the per-event classification rules.
///
/// # Errors
///
/// Returns a structured graphics-state walker error for malformed records in
/// the supported operator set or invalid source ranges.
pub fn build_inventory(
    source: &[u8],
    records: &[OperatorRecord],
    page: PageIndex,
    scope: &ContentScope,
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
) -> Result<Inventory, GraphicsWalkError> {
    build_inventory_with_color_space_env(
        source,
        records,
        page,
        scope,
        image_xobject_names,
        form_xobject_names,
        ColorSpaceEnv::empty(),
    )
}

/// Build a combined page-object inventory, resolving `cs`/`scn` resource colours
/// against a borrowed page colour-space environment.
///
/// This is [`build_inventory`] plus the one new abstraction: `cs`/`CS` +
/// `sc`/`scn`/`SC`/`SCN` colours are resolved against `color_space_env` and
/// emitted as honest [`ColorObservation`]s carrying the real source family and
/// spot colorant. With [`ColorSpaceEnv::empty`] this is byte-identical to
/// `build_inventory`, so device-only pages and form content (which must NOT
/// inherit the page environment in this slice) reduce to the prior behaviour.
///
/// # Errors
///
/// Returns a structured graphics-state walker error for malformed records in the
/// supported operator set or invalid source ranges.
pub fn build_inventory_with_color_space_env(
    source: &[u8],
    records: &[OperatorRecord],
    page: PageIndex,
    scope: &ContentScope,
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
    color_space_env: ColorSpaceEnv<'_>,
) -> Result<Inventory, GraphicsWalkError> {
    build_inventory_with_initial_state_and_envs(
        source,
        records,
        page,
        scope,
        image_xobject_names,
        form_xobject_names,
        Rc::new(GraphicsStateSnapshot::page_default()),
        color_space_env,
        ExtGStateEnv::empty(),
    )
}

/// Build a combined page-object inventory whose walk starts from a supplied
/// shared graphics-state snapshot and resolves resource names against both
/// borrowed environments.
///
/// This is the seeded sibling of [`build_inventory_with_color_space_env`] for
/// invocation-specific Form `XObject` templates (ISO 32000-1 §8.10.1): the walk
/// begins from `initial_state` — normally the exact caller state at the Form's
/// `Do` event — instead of [`GraphicsStateSnapshot::page_default`], with an
/// empty local `q`/`Q` stack. Resource lookup stays a separate axis: `cs`/`scn`
/// resolve against `color_space_env` and `gs` against `extgstate_env` (both
/// scope-LOCAL by the caller's contract), never against the inherited state.
/// Installing the seed is an `Rc` refcount bump; the first mutating operator
/// copies-on-write, so the caller-held snapshot is never mutated.
///
/// A sourced vector/text colour that still equals the corresponding seed
/// colour is inherited provenance without owning-stream identity. Such an entry
/// omits [`EditCapability::RewriteColorOperand`] until a local colour operator
/// establishes distinct provenance; unrelated capabilities are unchanged.
///
/// Passing [`GraphicsStateSnapshot::page_default`], any `color_space_env`, and
/// [`ExtGStateEnv::empty`] is byte-identical to
/// [`build_inventory_with_color_space_env`], which delegates here. Form
/// `/Matrix` concatenation, `/BBox` clipping, and transparency-group entry
/// resets are NOT applied by this builder.
///
/// # Errors
///
/// Returns a structured graphics-state walker error for malformed records in the
/// supported operator set or invalid source ranges.
#[allow(clippy::too_many_arguments)]
pub fn build_inventory_with_initial_state_and_envs(
    source: &[u8],
    records: &[OperatorRecord],
    page: PageIndex,
    scope: &ContentScope,
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
    initial_state: Rc<GraphicsStateSnapshot>,
    color_space_env: ColorSpaceEnv<'_>,
    extgstate_env: ExtGStateEnv<'_>,
) -> Result<Inventory, GraphicsWalkError> {
    let capability_seed = Rc::clone(&initial_state);
    let program = PaintProgram::with_envs(source, records, color_space_env, extgstate_env);
    collect_entries_from_ops(
        program.ops_with_initial_state(initial_state),
        |event, sequence| {
            combined_entry(
                page,
                scope,
                event,
                image_xobject_names,
                form_xobject_names,
                sequence,
            )
            .map(|entry| withhold_inherited_color_rewrite(entry, event, &capability_seed))
        },
    )
}

/// Build vector inventory entries from graphics-state events.
///
/// Only path paint events that use stroke or fill color become inventory
/// entries. Path-ending and unsupported no-op events are skipped.
#[must_use]
pub fn vector_inventory_from_graphics_events(
    page: PageIndex,
    scope: &ContentScope,
    events: &[PaintOp],
) -> Inventory {
    collect_entries(events, |event, sequence| {
        vector_entry(page, scope, event, sequence)
    })
}

/// Build text inventory entries from graphics-state events.
///
/// Text-showing events always become `ObjectKind::Text` entries. Supported
/// visible rendering modes carry color observations and edit capabilities;
/// invisible or unsupported modes remain conservative.
#[must_use]
pub fn text_inventory_from_graphics_events(
    page: PageIndex,
    scope: &ContentScope,
    events: &[PaintOp],
) -> Inventory {
    collect_entries(events, |event, sequence| {
        text_entry(page, scope, event, sequence)
    })
}

/// Build image inventory entries from graphics-state events.
///
/// Only `Do` invocations whose resource names appear in `image_xobject_names`
/// become `ObjectKind::Image` entries. Other `XObject` invocations are
/// preserved as walker events but skipped by this inventory slice.
#[must_use]
pub fn image_inventory_from_graphics_events(
    page: PageIndex,
    scope: &ContentScope,
    events: &[PaintOp],
    image_xobject_names: &[PdfName],
) -> Inventory {
    collect_entries(events, |event, sequence| {
        image_entry(page, scope, event, image_xobject_names, sequence)
    })
}

/// Build form `XObject` invocation inventory entries from graphics-state events.
///
/// Only `Do` invocations whose resource names appear in `form_xobject_names`
/// become `ObjectKind::FormXObject` entries. Other `XObject` invocations are
/// preserved as walker events but skipped by this inventory slice.
#[must_use]
pub fn form_inventory_from_graphics_events(
    page: PageIndex,
    scope: &ContentScope,
    events: &[PaintOp],
    form_xobject_names: &[PdfName],
) -> Inventory {
    collect_entries(events, |event, sequence| {
        form_entry(page, scope, event, form_xobject_names, sequence)
    })
}

/// Build a combined page-object inventory from graphics-state events.
///
/// This walks the events once and merges the vector, text, image, and form
/// slices into a single `Inventory` in content (event) order. Each emitted
/// entry receives one monotonic `sequence` from a counter shared across all
/// kinds, so the inventory is a single content-ordered identity space.
///
/// For each event the same kind the matching per-kind builder would emit is
/// produced:
///
/// - `PathPaint` that uses color -> vector (path paint with no color is skipped);
/// - `TextShow` -> text;
/// - `XObjectInvoke` whose name is in `image_xobject_names` -> image;
/// - `XObjectInvoke` whose name is in `form_xobject_names` -> form;
/// - any other `XObjectInvoke` and all no-op/path-ending events -> skipped.
///
/// `image_xobject_names` and `form_xobject_names` must be disjoint by contract.
/// If a `Do` name appears in both lists, the image classification wins.
///
/// Each merged entry's kind, provenance, colors, and capabilities equal what the
/// matching per-kind builder would produce for the same event; only the
/// `sequence` (and therefore the digest) differs, because the counter is global.
///
/// This materialized helper has no initial-state parameter, so it cannot
/// distinguish a caller-stream colour source inherited by a Form. Use
/// [`build_inventory_with_initial_state_and_envs`] for seeded, action-capable
/// Form inventory; that builder withholds rewrite capability when the seed
/// identifies inherited source provenance.
#[must_use]
pub fn inventory_from_graphics_events(
    page: PageIndex,
    scope: &ContentScope,
    events: &[PaintOp],
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
) -> Inventory {
    collect_entries(events, |event, sequence| {
        combined_entry(
            page,
            scope,
            event,
            image_xobject_names,
            form_xobject_names,
            sequence,
        )
    })
}

/// Classify one event using the combined inventory's fixed dispatch order.
/// Image classification precedes Form classification for a name present in
/// both caller-supplied lists.
fn combined_entry(
    page: PageIndex,
    scope: &ContentScope,
    event: &PaintOp,
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
    sequence: u32,
) -> Option<InventoryEntry> {
    vector_entry(page, scope, event, sequence)
        .or_else(|| text_entry(page, scope, event, sequence))
        .or_else(|| image_entry(page, scope, event, image_xobject_names, sequence))
        .or_else(|| form_entry(page, scope, event, form_xobject_names, sequence))
}

/// Walk events once, assigning a shared monotonic content-order `sequence` to
/// each emitted entry. `classify` returns `None` for events that emit nothing,
/// which leaves the counter unchanged.
fn collect_entries(
    events: &[PaintOp],
    mut classify: impl FnMut(&PaintOp, u32) -> Option<InventoryEntry>,
) -> Inventory {
    let mut entries = Vec::new();
    for event in events {
        let sequence = usize_to_u32(entries.len());
        if let Some(entry) = classify(event, sequence) {
            entries.push(entry);
        }
    }
    Inventory { entries }
}

/// Consume a replayable [`PaintProgram`] over the `source + records` path,
/// classifying each op as it streams past instead of first materializing the
/// whole `Vec<PaintOp>`.
///
/// This mirrors [`collect_entries`] exactly: iterating the program walks every
/// record in order via `GraphicsStateWalker::step` (so save/restore, snapshot
/// propagation, and error detection on records after the last entry-producing
/// operator match [`walk_graphics_state`](presslint_paint::walk_graphics_state)),
/// and assigns the same shared monotonic content-order `sequence` (`entries.len()`
/// at emit time) to each emitted entry. The program fuses on the first malformed
/// record, so the `?` short-circuits with the same `GraphicsWalkError` the
/// materializing path would return. Output is therefore bit-identical to feeding
/// the full event slice to `collect_entries`, but peak retained event memory drops
/// from O(records) to O(1).
fn collect_entries_streaming(
    source: &[u8],
    records: &[OperatorRecord],
    color_space_env: ColorSpaceEnv<'_>,
    classify: impl FnMut(&PaintOp, u32) -> Option<InventoryEntry>,
) -> Result<Inventory, GraphicsWalkError> {
    collect_entries_from_ops(
        PaintProgram::new(source, records, color_space_env).ops(),
        classify,
    )
}

/// Drive one already-started [`PaintOps`] walk, classifying each op as it
/// streams past. This is the shared tail of [`collect_entries_streaming`] and
/// the seeded builder: the walk's initial state and environments were fixed by
/// whoever constructed the iterator, so default and seeded paths share the same
/// sequence assignment, error short-circuit, and O(1) event retention.
fn collect_entries_from_ops(
    ops: PaintOps<'_>,
    mut classify: impl FnMut(&PaintOp, u32) -> Option<InventoryEntry>,
) -> Result<Inventory, GraphicsWalkError> {
    let mut entries = Vec::new();
    for op in ops {
        let event = op?;
        let sequence = usize_to_u32(entries.len());
        if let Some(entry) = classify(&event, sequence) {
            entries.push(entry);
        }
    }
    Ok(Inventory { entries })
}

fn vector_entry(
    page: PageIndex,
    scope: &ContentScope,
    event: &PaintOp,
    sequence: u32,
) -> Option<InventoryEntry> {
    let PaintOpKind::PathPaint { paint } = &event.kind else {
        return None;
    };
    let paint = *paint;
    let colors = color_observations(paint, &event.state);
    if colors.is_empty() {
        return None;
    }
    let digest = vector_object_digest(page, sequence, scope, &NO_INVOCATION, event, paint, &colors);
    Some(inventory_entry(
        page,
        scope,
        event,
        sequence,
        ObjectKind::Vector,
        colors,
        vec![EditCapability::RewriteColorOperand],
        digest,
    ))
}

fn text_entry(
    page: PageIndex,
    scope: &ContentScope,
    event: &PaintOp,
    sequence: u32,
) -> Option<InventoryEntry> {
    let PaintOpKind::TextShow {
        operator,
        rendering_mode,
    } = &event.kind
    else {
        return None;
    };
    let operator = *operator;
    let rendering_mode = *rendering_mode;
    let colors = text_color_observations(rendering_mode, &event.state);
    let capabilities = text_capabilities(&colors);
    let digest = text_object_digest(
        page,
        sequence,
        scope,
        &NO_INVOCATION,
        event,
        operator,
        rendering_mode,
        &colors,
    );
    Some(inventory_entry(
        page,
        scope,
        event,
        sequence,
        ObjectKind::Text,
        colors,
        capabilities,
        digest,
    ))
}

/// Return the invoked `XObject` name when `event` is a `Do` for a name the
/// caller declared in `names`; otherwise `None`.
fn matched_xobject<'a>(event: &'a PaintOp, names: &[PdfName]) -> Option<&'a PdfName> {
    let PaintOpKind::XObjectInvoke { name } = &event.kind else {
        return None;
    };
    names.contains(name).then_some(name)
}

fn image_entry(
    page: PageIndex,
    scope: &ContentScope,
    event: &PaintOp,
    image_xobject_names: &[PdfName],
    sequence: u32,
) -> Option<InventoryEntry> {
    let name = matched_xobject(event, image_xobject_names)?;
    let colors = vec![image_color_observation()];
    let digest = image_object_digest(page, sequence, scope, &NO_INVOCATION, event, name, &colors);
    Some(inventory_entry(
        page,
        scope,
        event,
        sequence,
        ObjectKind::Image,
        colors,
        vec![EditCapability::ReadOnly],
        digest,
    ))
}

fn form_entry(
    page: PageIndex,
    scope: &ContentScope,
    event: &PaintOp,
    form_xobject_names: &[PdfName],
    sequence: u32,
) -> Option<InventoryEntry> {
    let name = matched_xobject(event, form_xobject_names)?;
    let digest = form_object_digest(page, sequence, scope, &NO_INVOCATION, event, name);
    Some(inventory_entry(
        page,
        scope,
        event,
        sequence,
        ObjectKind::FormXObject,
        Vec::new(),
        vec![EditCapability::ReadOnly],
        digest,
    ))
}

#[allow(clippy::too_many_arguments)]
fn inventory_entry(
    page: PageIndex,
    scope: &ContentScope,
    event: &PaintOp,
    sequence: u32,
    kind: ObjectKind,
    colors: Vec<ColorObservation>,
    capabilities: Vec<EditCapability>,
    digest: [u8; 32],
) -> InventoryEntry {
    InventoryEntry {
        id: ObjectId {
            page,
            sequence,
            digest,
        },
        kind,
        provenance: Provenance {
            page,
            scope: scope.clone(),
            // Explicit identity-only unwrap: `Provenance.range` is a public
            // bare-`ByteRange` contract; the paint op's range is decoded-based.
            range: Some(event.record_range.into_byte_range()),
            invocation: None,
        },
        bounds: None,
        colors,
        capabilities,
    }
}

/// Construct the born-final identity for a form-expanded entry.
///
/// The umbrella form-expansion adapter first classifies each form's paint ops
/// through the single-stream builders above (empty path, form-local sequence) to
/// obtain the kind, colours, scope, and capabilities, then calls this once per
/// emitted entry to stamp the FINAL page-global `sequence` and to fold the
/// invocation `path` into the digest. The identity is therefore computed a single
/// time with the sequence the entry actually carries: the earlier defect where
/// expansion rebased `id.sequence` AFTER the digest had already been computed
/// from a contradictory form-local sequence is removed.
///
/// `path` is borrowed for the digest and cloned once into
/// `provenance.invocation`, so that published field and the path folded into the
/// digest are one and the same source. An empty (page-level) path yields `None`,
/// matching the single-stream builders.
#[must_use]
pub fn expanded_entry_identity(
    template: &InventoryEntry,
    sequence: u32,
    path: &InvocationPath,
    event: &PaintOp,
) -> InventoryEntry {
    let digest = expanded_digest(template, sequence, path, event);
    InventoryEntry {
        id: ObjectId {
            page: template.id.page,
            sequence,
            digest,
        },
        kind: template.kind,
        provenance: Provenance {
            page: template.provenance.page,
            scope: template.provenance.scope.clone(),
            range: template.provenance.range,
            invocation: invocation_from_path(path),
        },
        bounds: template.bounds,
        colors: template.colors.clone(),
        capabilities: template.capabilities.clone(),
    }
}

/// Recompute the digest a classified entry must carry for the final `sequence`
/// and invocation `path`, dispatching on the entry kind and the paint op that
/// produced it. The per-kind ingredients (paint kind, text operator + mode,
/// resource name, byte-exact colour observations) match what the single-stream
/// builder folded, so only the header's sequence and invocation-path bytes differ
/// from the template digest.
fn expanded_digest(
    template: &InventoryEntry,
    sequence: u32,
    path: &InvocationPath,
    event: &PaintOp,
) -> [u8; 32] {
    let page = template.id.page;
    let scope = &template.provenance.scope;
    match (template.kind, &event.kind) {
        (ObjectKind::Vector, PaintOpKind::PathPaint { paint }) => {
            vector_object_digest(page, sequence, scope, path, event, *paint, &template.colors)
        }
        (
            ObjectKind::Text,
            PaintOpKind::TextShow {
                operator,
                rendering_mode,
            },
        ) => text_object_digest(
            page,
            sequence,
            scope,
            path,
            event,
            *operator,
            *rendering_mode,
            &template.colors,
        ),
        (ObjectKind::Image, PaintOpKind::XObjectInvoke { name }) => {
            image_object_digest(page, sequence, scope, path, event, name, &template.colors)
        }
        (ObjectKind::FormXObject, PaintOpKind::XObjectInvoke { name }) => {
            form_object_digest(page, sequence, scope, path, event, name)
        }
        // A classified template always pairs with the op kind that produced it,
        // so this arm is unreachable in practice; keep the template digest rather
        // than panic in a library path.
        _ => template.id.digest,
    }
}

/// `None` for page-level (empty) paths, otherwise the owned invocation path
/// published in `Provenance.invocation` and folded into the entry digest.
fn invocation_from_path(path: &InvocationPath) -> Option<InvocationPath> {
    if path.frames.is_empty() {
        None
    } else {
        Some(path.clone())
    }
}

fn color_observations(
    paint: PathPaintKind,
    state: &GraphicsStateSnapshot,
) -> Vec<ColorObservation> {
    let mut colors = Vec::with_capacity(2);
    if paint.uses_stroke() {
        colors.push(state.stroke_observation());
    }
    if paint.uses_fill() {
        colors.push(state.fill_observation());
    }
    colors
}

fn text_color_observations(
    mode: TextRenderingMode,
    state: &GraphicsStateSnapshot,
) -> Vec<ColorObservation> {
    let mut colors = Vec::with_capacity(2);
    if mode.uses_stroke() {
        colors.push(state.stroke_observation());
    }
    if mode.uses_fill() {
        colors.push(state.fill_observation());
    }
    colors
}

fn text_capabilities(colors: &[ColorObservation]) -> Vec<EditCapability> {
    if colors.is_empty() {
        Vec::new()
    } else {
        vec![
            EditCapability::RewriteColorOperand,
            EditCapability::AddTextSpreadStroke,
        ]
    }
}

/// Fail closed when an entry observes a sourced colour carried in by the
/// supplied initial state. Its bare range may address the caller stream, while
/// the entry's scope addresses the callee stream, so it is evidence for
/// observation/identity only and cannot authorize a local operand rewrite.
///
/// Equality compares the whole corresponding `GraphicsColor`, including source
/// provenance and the resource name that is not published on the observation.
/// A local colour operator can therefore regain the normal capability. If two
/// streams happen to produce an identical colour at an identical decoded range,
/// the conservative false positive removes capability; it can never grant
/// capability to inherited provenance.
fn withhold_inherited_color_rewrite(
    mut entry: InventoryEntry,
    event: &PaintOp,
    initial_state: &GraphicsStateSnapshot,
) -> InventoryEntry {
    if matches!(entry.kind, ObjectKind::Vector | ObjectKind::Text)
        && entry.colors.iter().any(|color| {
            color.source.is_some()
                && match color.usage {
                    ColorUsage::Stroke => {
                        event.state.stroking_color == initial_state.stroking_color
                    }
                    ColorUsage::Fill => {
                        event.state.nonstroking_color == initial_state.nonstroking_color
                    }
                    ColorUsage::Image | ColorUsage::Shading => false,
                }
        })
    {
        entry
            .capabilities
            .retain(|capability| *capability != EditCapability::RewriteColorOperand);
    }
    entry
}

const fn image_color_observation() -> ColorObservation {
    ColorObservation {
        usage: ColorUsage::Image,
        space: ColorSpace::Unknown,
        components: Vec::new(),
        spot_name: None,
        spot_names: Vec::new(),
        // Synthesized observation: no color-setting operator produced it.
        source: None,
    }
}
