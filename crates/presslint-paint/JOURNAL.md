# presslint-paint Journal

## T156 - ExtGState env + snapshot classification (Phase 1-2)

- New `extgstate_env.rs`: per-parameter classification `GsParam<T>` (`Default` /
  `Set(T)` / `Unresolved` / `Unclassified`), classified value enums
  (`OverprintMode`, `AlphaClass`, `BlendModeClass`, `SoftMaskClass`),
  `ExtGStateParams` (OP, op, OPM, CA, ca, BM, SMask), `ExtGStateResource`
  (name + params + `has_unclassified_keys`), and the borrowed `Copy`
  `ExtGStateEnv<'a>` mirroring `ColorSpaceEnv` (`new`/`empty`/`resolve`/
  `is_empty`). No resource names enter the graphics-state snapshot.
- `GraphicsStateSnapshot` gains one nested `Copy` field
  (`extgstate: GraphicsExtGStateSnapshot`, all-`Default` in `page_default()`),
  so `q`/`Q` save/restore it for free and the copy-on-write cost class is
  unchanged.
- `gs` still emits `SetExtGState { name }` and now also mutates the snapshot
  with a documented three-level rule: EMPTY env (every pre-existing caller)
  leaves the state untouched — byte-and-state-identical legacy behaviour,
  pinned by a dedicated test; a non-empty env with an unresolved name sets all
  seven parameters to `Unresolved` (classification never invents values); a
  resolved resource LAYERS only the parameters its dictionary actually sets
  over the current state.
- `PaintProgram`/`GraphicsStateWalker` grow sibling env constructors;
  `PaintSubProgram` gains the `extgstate_env` field (mechanical empty-env
  updates in the umbrella adapter). Digests are untouched by construction —
  snapshot fields are not digest inputs; the bit-identity and form goldens
  pass byte-identical.

## T154 - Split the paint test module (Phase 1-0, tests-only)

- Verbatim mechanical split of the 985-line `tests.rs` into a thin aggregator
  (shared `assemble`/`name`/`page_program`/`form_program` builders + the
  dependency-free `mini_json` serializer) plus focused submodules under
  `src/tests/`: `walker.rs`, `paint_program.rs`, `provenance.rs`,
  `mutation_class.rs`, `call_machine.rs`. Same 23 tests, same names, no
  assertion or production change; every file well under the file-size gate.
- Ablation: lifted the embedded `mini_json` serializer out of the aggregator
  into its own `src/tests/mini_json.rs` submodule (same `tests::mini_json`
  module path and `pub(super)` visibility), so the aggregator is genuinely thin
  (441 -> 73 lines). Pure relocation, behavior identical, still 23 green tests.

## T150 - Form Resolver Return Hook (Phase 0b-4a)

- `FormResolver` now has a default no-op `on_return(&InvocationPath)` hook.
  `CallMachine::walk` calls it immediately before popping a descended frame,
  including empty callees, and unwinds every still-open descended frame in LIFO
  order before returning a callee walker or resolver error. Resolver skips and
  resolver errors still do not create a new child frame, so they only fire the
  hook for already-open frames being unwound.
- Added focused call-machine tests for LIFO return ordering, empty callee
  return cleanup, and error-path cleanup. The hook is mechanics only; depth,
  cycle, resource, and budget policy remain outside `presslint-paint`.

## T149 - Exact depth-first flat projection (Phase 0b-3)

- New `flat_projection.rs` adds `flat_call_events(root, resolver, sink)` plus the
  borrowed `FlatPaintOp { path: &InvocationPath, op: &PaintOp }` stream item,
  re-exported from the crate root. The driver composes `CallMachine::walk` (no
  duplicated traversal) and re-presents its depth-first visitor stream as ONE
  flat, fused, path-annotated stream in the umbrella's emission order: caller ops
  in program order, and a `Descend`-resolved form call's callee ops delivered
  immediately after that invocation op (recursively, depth-first) before the
  caller's next op; a `Skip`/refused descent yields no callee ops; every `Ok`
  item carries its `InvocationPath` (empty for root ops).
- Streaming and allocation-light by construction: items are handed to the sink as
  they are visited (no materialized `Vec`), and the invocation path is yielded by
  reference — no per-op deep path clone. The interleaving is a pure function of
  `(root, resolver decisions)` with no hash-map iteration order.
- Error/fuse semantics mirror `PaintOps`: a walk error in any program (root or
  callee) or a resolver error is delivered as the FINAL sink call wrapped in
  `Err`, after which nothing more is delivered; there is no resumption in the
  caller after a callee error, matching the umbrella's short-circuit on a
  decode/walk failure inside a form expansion.
- SEQUENCE NUMBERS ARE OUT OF SCOPE and the module doc says so: this slice
  reproduces POSITIONAL order only. Today's umbrella gives page entries their
  walk-local sequence and seeds expanded (form) entries a continuation at
  `page_inventory.len()` in splice order, so positional order and sequence
  numbering diverge by design; sequence assignment stays an inventory/adapter
  concern in the umbrella (pinned by its form-expanded goldens) until a later
  slice.
- Five synthetic tests (colocated in `flat_projection.rs`, reusing the shared
  `tests.rs` program builders): order equivalence over two call sites with a
  nested descent, skip yields no callee ops while the caller continues, double
  invocation of one form yields the callee twice under paths `[(0,F)]`/`[(1,F)]`,
  a callee walk error fuses the stream (the post-descent caller op does not
  stream), and a no-form root degenerates element-by-element to the plain
  `PaintProgram` op stream. No consumer changed: inventory and the umbrella
  compile and pass untouched.

## T148 - Call/return traversal substrate (Phase 0b-2)

- Added `call_machine.rs` with the pure borrowed call/return contract:
  `PaintSubProgram`, `CallSite`, `ResolveForm`, `FormResolver`, `CallEvent`,
  and `CallMachine`.
- `CallMachine::walk` composes the existing replayable `PaintProgram` iterator,
  classifies form `Do` calls with image-name precedence, assigns caller-local
  form invocation ordinals, and traverses depth-first with an explicit stack.
  The resolver owns all resource, budget, depth, and cycle policy.
- Added synthetic preloaded-program coverage for repeated same-name ordinals,
  nested path construction and return popping, resolver skip, image/form
  conflicts, resolver error surfacing, and invocation-path JSON shape.
- No production consumer changed in this slice; this is the traversal substrate
  for later form-expansion migration.

## T146 - MutationClass routing contract (Phase 0a-7)

- Added the public `MutationClass` routing vocabulary for per-paint-act mutation
  decisions: `PreserveBytes`, `SurgicalRewrite`, `AppearanceReplacement`, and
  unit-only `UnsupportedSkip`. The enum is deliberately not serialized and has
  no live producers or consumers yet.
- The module rustdoc records the partial reconciliation with the existing
  `EditCapability`, `MutationBoundary` / `SkipReason`, and write pipeline skip
  taxonomies, including the Phase-4 target to centralize or intentionally split
  the current write skip-reason mappings.
- Added two `const fn` predicates with focused unit coverage:
  `preserves_source_bytes` and `may_emit_replacement_bytes`. This is additive
  only: no inventory capability production, action planning, write skip enums,
  digest input, or JSON shape changed.

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
