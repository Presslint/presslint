use presslint_syntax::OperatorRecord;
use presslint_types::{
    BoundingBox, ColorObservation, ColorSpace, ColorUsage, ContentScope, EditCapability, ObjectId,
    ObjectKind, PageIndex, PdfName, Provenance,
};
use serde::{Deserialize, Serialize};

use crate::digest::{
    form_object_digest, image_object_digest, text_object_digest, usize_to_u32, vector_object_digest,
};
use presslint_paint::{
    ColorSpaceEnv, GraphicsStateEvent, GraphicsStateEventKind, GraphicsStateSnapshot,
    GraphicsWalkError, PaintProgram, PathPaintKind, TextRenderingMode,
};

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
    collect_entries_streaming(source, records, color_space_env, |event, sequence| {
        // Same fixed dispatch order as `inventory_from_graphics_events`: image is
        // tried before form so a name present in both lists wins as an image.
        vector_entry(page, scope, event, sequence)
            .or_else(|| text_entry(page, scope, event, sequence))
            .or_else(|| image_entry(page, scope, event, image_xobject_names, sequence))
            .or_else(|| form_entry(page, scope, event, form_xobject_names, sequence))
    })
}

/// Build vector inventory entries from graphics-state events.
///
/// Only path paint events that use stroke or fill color become inventory
/// entries. Path-ending and unsupported no-op events are skipped.
#[must_use]
pub fn vector_inventory_from_graphics_events(
    page: PageIndex,
    scope: &ContentScope,
    events: &[GraphicsStateEvent],
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
    events: &[GraphicsStateEvent],
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
    events: &[GraphicsStateEvent],
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
    events: &[GraphicsStateEvent],
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
#[must_use]
pub fn inventory_from_graphics_events(
    page: PageIndex,
    scope: &ContentScope,
    events: &[GraphicsStateEvent],
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
) -> Inventory {
    collect_entries(events, |event, sequence| {
        // Dispatch in fixed order; the helpers are mutually exclusive by event
        // kind except for `XObjectInvoke`, where image is tried before form so
        // a name present in both lists is classified as an image.
        vector_entry(page, scope, event, sequence)
            .or_else(|| text_entry(page, scope, event, sequence))
            .or_else(|| image_entry(page, scope, event, image_xobject_names, sequence))
            .or_else(|| form_entry(page, scope, event, form_xobject_names, sequence))
    })
}

/// Walk events once, assigning a shared monotonic content-order `sequence` to
/// each emitted entry. `classify` returns `None` for events that emit nothing,
/// which leaves the counter unchanged.
fn collect_entries(
    events: &[GraphicsStateEvent],
    mut classify: impl FnMut(&GraphicsStateEvent, u32) -> Option<InventoryEntry>,
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
/// whole `Vec<GraphicsStateEvent>`.
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
    mut classify: impl FnMut(&GraphicsStateEvent, u32) -> Option<InventoryEntry>,
) -> Result<Inventory, GraphicsWalkError> {
    let mut entries = Vec::new();
    for op in PaintProgram::new(source, records, color_space_env) {
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
    event: &GraphicsStateEvent,
    sequence: u32,
) -> Option<InventoryEntry> {
    let GraphicsStateEventKind::PathPaint { paint } = &event.kind else {
        return None;
    };
    let paint = *paint;
    let colors = color_observations(paint, &event.state);
    if colors.is_empty() {
        return None;
    }
    let digest = vector_object_digest(page, sequence, scope, event, paint, &colors);
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
    event: &GraphicsStateEvent,
    sequence: u32,
) -> Option<InventoryEntry> {
    let GraphicsStateEventKind::TextShow {
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
fn matched_xobject<'a>(event: &'a GraphicsStateEvent, names: &[PdfName]) -> Option<&'a PdfName> {
    let GraphicsStateEventKind::XObjectInvoke { name } = &event.kind else {
        return None;
    };
    names.contains(name).then_some(name)
}

fn image_entry(
    page: PageIndex,
    scope: &ContentScope,
    event: &GraphicsStateEvent,
    image_xobject_names: &[PdfName],
    sequence: u32,
) -> Option<InventoryEntry> {
    let name = matched_xobject(event, image_xobject_names)?;
    let colors = vec![image_color_observation()];
    let digest = image_object_digest(page, sequence, scope, event, name, &colors);
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
    event: &GraphicsStateEvent,
    form_xobject_names: &[PdfName],
    sequence: u32,
) -> Option<InventoryEntry> {
    let name = matched_xobject(event, form_xobject_names)?;
    let digest = form_object_digest(page, sequence, scope, event, name);
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
    event: &GraphicsStateEvent,
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
            range: Some(event.record_range),
        },
        bounds: None,
        colors,
        capabilities,
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

const fn image_color_observation() -> ColorObservation {
    ColorObservation {
        usage: ColorUsage::Image,
        space: ColorSpace::Unknown,
        components: Vec::new(),
        spot_name: None,
        // Synthesized observation: no color-setting operator produced it.
        source: None,
    }
}
