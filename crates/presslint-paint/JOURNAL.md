# presslint-paint Journal

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
