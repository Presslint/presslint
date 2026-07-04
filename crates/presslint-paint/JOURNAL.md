# presslint-paint Journal

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
