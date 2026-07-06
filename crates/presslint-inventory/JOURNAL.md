# presslint-inventory Journal

## T152 - Invocation provenance shape lock

- Added a serde shape lock for an inventory entry whose provenance carries a
  non-empty invocation path. Existing `None`-case JSON expectations stay
  unchanged and exercise default deserialization for older shapes.

## T146 - Re-export paint mutation class

- The inventory facade now re-exports `presslint_paint::MutationClass` alongside
  the other paint types; no inventory records, capabilities, digests, or serde
  shapes changed.

## T145 - Adopt typed decoded ranges at the paint seams (Phase 0a-6)

- `presslint-paint` now types its provenance fields as `DecodedRange`
  (offsets into the decoded content-stream buffer). Inventory unwraps
  explicitly and identity-only at its seams: `Provenance.range` takes
  `event.record_range.into_byte_range()` and the four digest functions unwrap
  both event ranges before `push_range`, so digest input order and values are
  unchanged.
- The facade re-export adds `DecodedRange`/`SourceRange` so `PaintOp`'s public
  fields stay coherent through this crate. Public serde shapes are untouched
  (`serde_shape` tests pass unedited) and the golden bit-identity digests are
  unchanged; tests were only mechanically updated to wrap range fixtures and
  use the newtype's `start()`/`end()` accessors.
- Ablation pass: the identical page/sequence/scope/index/range header pushed
  by all four digest functions moved into one `push_event_header` helper, so
  the typed-range unwrap seam lives in a single place with unchanged push
  order (golden digests still pass). The duplicated tokenize/assemble
  error-mapping in the test helpers (`tests.rs` and `bit_identity.rs`) is now
  one shared `assembled_records` helper.

## T144 - Paint-op state is shared via `Rc` (Phase 0a-5)

- Updated inventory tests that assert the default graphics-state snapshot to
  compare `PaintOp.state.as_ref()` with `GraphicsStateSnapshot::page_default()`.
  Production inventory code continues to deref-coerce the shared paint-op state
  unchanged.

## T143 - Golden Bit-Identity Lock for Combined Inventory (Phase 0a-3)

- Added a tests-only `bit_identity` corpus that pins combined inventory entry
  counts, object kinds, and every 32-byte entry digest for representative
  vector, text, image, form, resource-colour, form-scope, many-no-op, shared-Do,
  and malformed-tail streams.
- Extracted a reusable streaming-vs-materialized assertion helper that compares
  `build_inventory` with `walk_graphics_state` plus
  `inventory_from_graphics_events`, including identical `GraphicsWalkError`
  propagation for a malformed record after the last entry-producing operator.
- The golden digest values are the durable guard for upcoming shared-walker
  refactors: differential equality alone would not catch a uniform regression
  that moves both paths together.

## T142 - Streaming Path Consumes `PaintProgram` (Phase 0a-2)

- `collect_entries_streaming` is rerouted to consume the replayable
  `presslint_paint::PaintProgram` instead of driving `GraphicsStateWalker::step`
  directly: `for op in PaintProgram::new(source, records, env) { let event = op?; … }`.
  The `?` short-circuit (the program fuses on the first malformed record) and the
  `sequence = usize_to_u32(entries.len())`-at-emit rule reproduce the previous
  behaviour exactly; the vector→text→image→form classify order is unchanged.
- The now-unused `GraphicsStateWalker` import was dropped; `GraphicsStateEvent`,
  `GraphicsStateSnapshot`, `GraphicsWalkError`, and the classify helpers remain. The
  doc-comment's intra-doc walker-step link was demoted to a plain code span so docs
  stay clean with the type out of scope.
- Bit-identical output: inventory entries, digests, serde shapes, and the colour
  audit are unchanged; the two streaming guard tests
  (`streaming_build_inventory_equals_events_path_for_mixed_stream`,
  `…_surfaces_walk_error_after_last_entry`) and the pinned fixtures pass unchanged,
  no test edited.

## T141 - Graphics-State Walker Moved to `presslint-paint` (Phase 0a-1)

- The graphics-state walker and its two private helpers (`walker.rs`,
  `color_space_env.rs`, `operands.rs`) were relocated verbatim into the new
  `presslint-paint` spine crate. `presslint-inventory` now depends on
  `presslint-paint` and re-exports the SAME public names via
  `pub use presslint_paint::{…}`, so the crate's public surface is byte-identical
  and every downstream consumer compiles unchanged.
- `digest.rs` and `inventory.rs` stay in this crate; their `use crate::walker::…`
  / `use crate::color_space_env::…` imports were repointed to `presslint_paint::…`.
- Pure mechanical crate move: no behaviour change. Inventory output, digests,
  serde shapes, and the colour audit are bit-identical (existing pinned fixtures
  and the full suite pass unchanged, no test edited).

## T137 - Page Resource Colour-Space Tracking (F4-6 slice 1a)

- The graphics-state colour slot is generalised from `GraphicsDeviceColor` to
  `GraphicsColor { space, components, resource_name, spot_name, source }`. Direct
  device operators (`g/G`, `rg/RG`, `k/K`) keep byte-identical behaviour and
  provenance (`resource_name`/`spot_name` stay `None`); the rename is contained to
  this crate plus the one colour event type. The colour events are renamed
  `SetStrokingColor`/`SetNonstrokingColor` (from `Set*DeviceColor`).
- The walker now interprets `cs`/`CS` (select the current nonstroking/stroking
  colour space by resource name) and `sc`/`scn`/`SC`/`SCN` (set a colour value in
  the current space), resolving names against a BORROWED page colour-space
  environment (`ColorSpaceEnv<'a>` over a `&[ColorSpaceResource]` slice). This is
  the one new abstraction; `GraphicsStateWalker<'a>` carries it and
  `with_color_space_env` builds it. A default-empty environment reproduces the
  device-only walk byte-for-byte (a device-only stream is unaffected — regression
  tested).
- Honest reporting: a resolved resource colour emits a normal `ColorObservation`
  with the REAL source family (`IccBased`/`Separation`/`DeviceN`), `spot_name`
  populated for `Separation`/`DeviceN`, and the `scn`/`cs` record range as `source`.
  `ICCBased` with `N=4` is reported as `IccBased`, never `DeviceCmyk`.
- Initial-colour-after-`cs` (ISO 32000-1 §8.6.3): selecting a space also sets its
  implied initial colour (device zeros, `DeviceCMYK` `0 0 0 1`, ICC zeros,
  `Separation`/`DeviceN` full tint), so paint observed after `cs` but before any
  `sc`/`scn` reports a real colour, never a stale device colour. An unresolved
  `cs`/`CS` name is reported honestly as `ColorSpace::Resource(name)`, never a bare
  `Unknown`, so the audit surfaces it as a coverage gap.
- Crate layering: `presslint-inventory` owns paint/graphics-state semantics only and
  never parses PDF dictionaries — it consumes the classified model produced by
  `presslint-pdf` and mapped by the umbrella crate. `presslint-color-lcms`/write path
  untouched; a trailing `scn` pattern name is recorded as `resource_name` but not
  otherwise modelled (Pattern is out of this slice).

## Current State

- Defines deterministic inventory and inventory-entry data contracts.
- The crate root is a small public facade over focused internal modules for
  inventory builders, graphics-state walking, digest stability, operand
  parsing, and tests.
- Includes the first graphics-state walker over
  `presslint-syntax::OperatorRecord`.
- The walker emits ordered events with operator and record byte provenance.
- Supported state slice: `q`, `Q`, `cm`, device color operators (`G`, `g`,
  `RG`, `rg`, `K`, `k`), text rendering mode (`Tr`), basic path paint
  operators, first-slice text-showing operators (`Tj`, `TJ`, `'`, `"`),
  XObject invocation (`Do`), and ExtGState invocation (`gs`).
- `gs` emits a named `SetExtGState { name }` event that carries the ExtGState
  resource name without the leading slash, reusing the shared `name_operand`
  helper exactly like `Do`'s `XObjectInvoke`. It surfaces only the invocation
  plus operator/record byte provenance: the graphics-state snapshot is left
  unchanged (relying on the existing `q`/`Q` clone for save/restore), and no
  ExtGState parameter semantics (overprint, blend mode, alpha, soft mask) are
  modelled yet. A malformed `gs` operand reuses the existing
  `MalformedOperandCount`/`MalformedNameOperand` errors, and `gs` no longer
  falls into the silent `NoOp` bucket.
- Unsupported operators emit explicit no-op events.
- Structured errors cover graphics-state stack underflow, malformed operand
  counts, malformed numeric operands, non-finite numeric operands, and invalid
  source ranges.
- Builds the first vector inventory slice from supported path-paint events,
  carrying caller-provided page/content scope, path-paint byte provenance,
  observed stroke/fill colors, deterministic object IDs, and color-operand
  rewrite capability.
- `GraphicsDeviceColor` records the device-color operator's record byte range as
  the color `source`. The range is stamped when `G`/`g`/`RG`/`rg`/`K`/`k` set
  the color, travels with the saved snapshot across `q`/`Q`, and is surfaced on
  vector/text `ColorObservation`s so they point at the color operator rather
  than the paint/text-show operator. The page-default color and the synthesized
  image observation carry `None`. The stroking/nonstroking setters share a
  single `sourced_device_color` helper that resolves the operator and stamps its
  record range, keeping the source invariant in one place. Digest version tags
  were bumped to `presslint.vector.v2`/`.text.v2`/`.image.v2` to make the
  object-ID change explicit, and a locked digest test pins the new value.
- Builds the first text inventory slice from text-showing events, carrying
  caller-provided page/content scope, text-showing byte provenance, unset
  bounds, deterministic object IDs, and color observations for supported
  visible rendering modes.
- Supported visible text rendering modes advertise color-operand rewrite and
  text spread-stroke capabilities. Invisible text and unsupported text
  rendering modes remain represented but carry no color-edit capability.
- Builds the first image inventory slice from `Do` XObject invocation events.
  Image entries are emitted only for caller-declared image XObject resource
  names, carry caller-provided page/content scope and `Do` provenance, leave
  bounds unset, record an unknown image color observation, and advertise only
  read-only capability.
- Builds the first form XObject invocation inventory slice from the same `Do`
  events. Form entries are emitted only for caller-declared form XObject
  resource names, carry caller-provided page/content scope and `Do` provenance,
  leave bounds unset, synthesize no color observations, use a dedicated
  `presslint.form.v1` digest tag, and advertise only read-only capability.
- Adds a combined page-object inventory builder pair (`build_inventory` plus
  `inventory_from_graphics_events`) that walks the graphics-state events exactly
  once and merges the vector, text, image, and form slices into a single
  `Inventory` in content (event) order. One monotonic `sequence` counter is
  shared across all kinds, so the merged inventory is a single content-ordered
  identity space rather than four disjoint per-kind ones. Each merged entry's
  kind, provenance, colors, and capabilities equal what the matching per-kind
  builder would produce for the same event; only the global `sequence` (and
  therefore the digest) differs. `XObjectInvoke` names are classified image
  first, then form, so a name present in both the image and form lists (which
  are disjoint by contract) is classified as an image. The per-kind builders now
  share a single `collect_entries` walk plus per-event entry helpers, so the
  combined and per-kind paths construct entries from the same code and the
  existing per-kind builders keep identical signatures, behavior, and digests.
  The image and form entry helpers share a single `matched_xobject` lookup for
  the `Do`-name classification instead of duplicating the `XObjectInvoke` match
  and name-list check, with no change to the resolved name or any digest.
- Adds focused dependency-free serde shape tests for `Inventory` and
  `InventoryEntry`. The locked fixtures round-trip through an in-memory JSON
  harness and pin the public encoding of nested core inventory-report fields:
  object IDs, page indexes, provenance, content scopes, byte ranges, PDF names,
  bounds, color observations, color spaces/usages, object kinds, and edit
  capabilities. The fixture includes bounded vector output, sourced color
  provenance, and a read-only form-style entry with empty colors.
- The `source + records` inventory builders (`build_inventory` and the four
  per-kind `build_*_inventory` slices) drive the walker step-by-step through a
  private `collect_entries_streaming` driver instead of first materializing a
  full `Vec<GraphicsStateEvent>`. The driver creates one `GraphicsStateWalker`,
  walks every record in order via `step`, and feeds each owned event to the same
  per-event classifier closures the builders already used, with the same shared
  monotonic content-order `sequence` (`entries.len()` at emit time). Output is
  bit-identical to the materializing path (object IDs, digests, sequence
  assignment, error kind/range all unchanged), but peak retained event memory
  drops from O(records) to O(1) event plus the produced entries: the
  intermediate event vector is removed, so the per-event snapshot clones the
  walker still makes inside `step` are no longer all retained at once, only one
  event at a time, before M4 layers real conversion on top. The events-based `*_from_graphics_events` builders are
  untouched, so callers that already hold a materialized event slice keep using
  them. An equivalence test pins `build_inventory` equal to
  `inventory_from_graphics_events` over `walk_graphics_state` for a mixed
  many-no-op/few-entry stream, and an error-parity test pins the same
  `GraphicsWalkError` for a malformed record placed after the last
  entry-producing operator.
- Criterion benchmark target `inventory` covers graphics-state walking
  throughput in operator records/events and combined inventory-building
  throughput in emitted inventory entries over small, repeated, and
  many-no-op/few-entry synthetic public content streams.

## Follow-Ups

- Do not create shading inventory before the text, vector, image, and form
  slices are stable.
- Add geometry/bounds only after path construction interpretation is designed.
- Add glyph decoding, font resource lookup, CMaps, shaping, and text geometry
  only after the text inventory provenance model is stable.
- Add page resource traversal, image stream inspection, image bounds, and image
  replacement only after the invocation-level image inventory model is stable.
- Add form stream recursion, page resource traversal, shared-object ownership
  analysis, and form geometry only after the invocation-level form inventory
  model is stable.
