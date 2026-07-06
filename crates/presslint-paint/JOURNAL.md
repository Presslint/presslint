# presslint-paint Journal

## T145 - Typed decoded/source provenance ranges (Phase 0a-6)

- New `provenance.rs` defines two zero-cost newtypes over the shared
  `ByteRange`: `DecodedRange` (offsets into the DECODED buffer the walker
  consumed — the walked content stream) and `SourceRange` (offsets into the
  original PDF source file; reserved, no producer yet). Both are
  `#[repr(transparent)]` + `#[serde(transparent)]` `Copy` newtypes with
  `#[must_use]` `const fn new`/`into_byte_range` (plus `const fn start`/`end`
  on `DecodedRange`). They are paint-local for now and intended to move to
  `presslint-types` when later layers adopt typed range bases.
- Four paint provenance fields adopt the decoded basis as a TYPE:
  `PaintOp.operator_range`, `PaintOp.record_range`, `GraphicsColor.source`
  (`Option<DecodedRange>`), and `GraphicsWalkError.range`. The wrap-boundary
  rule: syntax record/token ranges stay bare (caller-relative by design); the
  walker wraps via `DecodedRange::new(...)` at every point a range enters a
  paint-owned type (`step`, the colour-setting stamps, and every
  `GraphicsWalkError::new` call site in `walker.rs`/`operands.rs`).
- Conversion at the seams is EXPLICIT AND IDENTITY-ONLY: `into_byte_range()`
  (or `.map(DecodedRange::into_byte_range)` in `GraphicsColor::observation`,
  which keeps emitting `ColorObservation.source: Option<ByteRange>` unchanged).
  No deref coercion, no blanket conversion traits in either direction, so the
  compiler refuses to mix range bases silently.
- Public serde shapes are byte-identical (`#[serde(transparent)]` keeps the
  JSON of `GraphicsColor` and `GraphicsWalkError` a plain
  `{"start":..,"end":..}` for the range fields). New unit tests lock this: the
  newtype serializes exactly like the bare range, a `GraphicsColor` with a
  typed `source` keeps the prior JSON shape, and the newtype round-trips from
  the bare-range wire shape via a dependency-free mini JSON harness in
  `tests.rs`.
- Zero-cost by construction: transparent `Copy` newtypes over two `usize`
  offsets; `new`/`into_byte_range` are `const fn` and compile away — no
  allocation, no branch, no hot-path change. Same misleading doc comments
  fixed alongside ("Source range" → "Decoded-buffer range"). Inventory output
  proven bit-identical by the golden digest locks passing unchanged.
- Ablation pass: the two per-side device-colour setters merged into one
  `set_device_color(..., side)` that reuses `apply_color`, with the six
  device-colour operators dispatching through a compact `(space, count, side)`
  table; `numeric_operands_vec` now reuses `parse_finite_number` instead of
  duplicating its parse/finite checks, and a `malformed_name` helper mirrors
  `malformed_numeric` at the three name-operand error sites. Behaviour, error
  values, and the golden digest locks are unchanged.

## T144 - Intern the per-event graphics state via `Rc` (Phase 0a-5)

- Replaced the per-event `GraphicsStateSnapshot` clone with shared
  `Rc<GraphicsStateSnapshot>` state on the walker, save stack, and `PaintOp`.
  Emitting an op now clones the `Rc`; state-changing operators mutate through
  the private `state_mut()` copy-on-write helper.
- `q` saves the current `Rc` and `Q` restores the popped `Rc`, preserving
  save/restore identity while keeping operator semantics and emit order
  unchanged.
- Dropped serde derives from `PaintOp` and `GraphicsStateSnapshot`; serde remains
  on the event-kind and colour payload types that are still serialized.
- Added `Rc::ptr_eq` unit coverage for no-state-change sharing and for
  `q cm Q n` save/restore identity. No new dependency; `Rc` comes from `std`.

## Phase 0a-4 - Rename to paint vocabulary (`PaintOp`/`PaintOpKind`)

- Canonical rename, FULL migration with NO compatibility aliases: `GraphicsStateEvent` ->
  `PaintOp` and `GraphicsStateEventKind` -> `PaintOpKind`, defined in `walker.rs` and
  re-exported from the crate root; the transient `pub type PaintOp = GraphicsStateEvent;`
  alias from 0a-2 was removed. Scope was limited to the two "emitted op" nouns; the
  state/mechanism vocabulary (`GraphicsStateWalker`, `GraphicsStateSnapshot`, `GraphicsColor`,
  `GraphicsWalkError`/`Kind`, `walk_graphics_state`, `PathPaintKind`, `TextRenderingMode`,
  `TextShowOperator`) is intentionally left as-is — layered vocabulary, not a half-rename.
- PURE identifier rename, ZERO behaviour change: the T143 `bit_identity` golden-lock (which pins
  entry `id.digest` sequences) passed UNCHANGED, proving inventory output stays byte-identical.
  Build/tests/clippy clean across the workspace; no old-name references remain in source/API.
  Done directly by the supervisor (not via the loop); Codex sanity-checked the approach as sound.

## T142 - Replayable `PaintProgram` Iterator (Phase 0a-2)

- New `paint_program.rs` introduces the paint program as a REPLAYABLE stream.
  `PaintProgram<'a>` is a cheap, `Copy` descriptor `{ source, records, env }`
  holding only borrowed bytes/records and the `Copy` `ColorSpaceEnv`; it owns no
  walk state and materializes no `Vec`. `PaintProgram::new(source, records, env)`
  builds it; `ops()` (and `IntoIterator` for both the owned value and
  `&PaintProgram`, plus an inherent `iter()` alias for the reference impl)
  constructs a FRESH `GraphicsStateWalker::with_color_space_env(env)` and re-walks
  `records` from index `0` every call, so the same program can be replayed.
- `PaintOps<'a>` is the iterator: `Iterator<Item = Result<GraphicsStateEvent,
  GraphicsWalkError>>` holding `{ walker, source, records, index, done }`. `next`
  drives `walker.step(source, index, record)`, increments `index`, and FUSES on the
  first `Err` — it yields that `Err` once, sets `done`, then returns `None` forever,
  faithfully modelling the current first-malformed-record short-circuit.
- An optional `pub type PaintOp = GraphicsStateEvent;` alias seeds paint vocabulary
  WITHOUT any type restructuring (the canonical rename is a later slice). The yielded
  item type stays `GraphicsStateEvent` this slice.
- Thin driver over the SAME `walker.step`: allocates nothing per op beyond what the
  walker already does (the per-event `state.clone()` hotspot is untouched). Replay
  re-runs the walk from scratch by design; extra retained memory is O(1). No new
  dependency; crate stays pure (`#![forbid(unsafe_code)]`).
- New `tests.rs` proves replay (iterating one program twice/thrice yields identical
  op vectors) and equality with `walk_graphics_state` (the short-circuiting
  `collect::<Result<Vec<_>, _>>()` equals the materializing walk, including the fused
  error case). `presslint-inventory` now consumes this program on its streaming path
  with bit-identical output (its pinned fixtures and the two streaming guard tests
  pass unchanged).

## T141 - Crate Created; Graphics-State Walker Moved In (Phase 0a-1)

- New workspace crate `presslint-paint` (Apache-2.0, `#![forbid(unsafe_code)]`)
  stands up the load-bearing paint spine. It depends ONLY on `presslint-types`
  and `presslint-syntax` (plus `serde` for the walker's serde derives), never on
  `presslint-inventory`, so inventory and later consumers (the rewriter,
  `presslint-render`) can build on the same paint model without a dependency
  cycle.
- The graphics-state walker (`walker.rs`) and its two private helpers
  (`color_space_env.rs`, `operands.rs`) were MOVED verbatim from
  `presslint-inventory/src/` into this crate. The three files move together, so
  their intra-crate `crate::walker` / `crate::color_space_env` / `crate::operands`
  paths still resolve unchanged; content is byte-identical.
- `lib.rs` declares `mod walker; mod color_space_env; mod operands;` and re-exports
  exactly the public surface inventory previously exposed from `walker` +
  `color_space_env` (`GraphicsColor`, `GraphicsStateEvent`, `GraphicsStateEventKind`,
  `GraphicsStateSnapshot`, `GraphicsStateWalker`, `GraphicsWalkError`,
  `GraphicsWalkErrorKind`, `PathPaintKind`, `TextRenderingMode`, `TextShowOperator`,
  `walk_graphics_state`, `ColorSpaceEnv`, `ColorSpaceResource`). `operands` stays a
  PRIVATE module, as it was in inventory.
- Pure mechanical relocation: no behaviour change, no new logic, no API additions,
  no walker-type renames. The per-event `state: self.state.clone()` hotspot is left
  exactly as-is (its fix is a later slice). Proven bit-identical by the inventory
  crate's pinned fixtures and full suite passing unchanged.
