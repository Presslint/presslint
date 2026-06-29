# presslint-actions Journal

## Current State

- Defines serializable recipe, recipe-step, action payload, no-op patch-plan,
  and action-plan report data contracts.
- Plans recipes against an `Inventory` by evaluating selectors in inventory
  order with `presslint-selectors`.
- Reports matched targets and matched-but-skipped objects with structured
  reasons: `UnsupportedCapability` for entries lacking the action's required
  edit capability, and `NonProcessColor` for `ConvertColor` entries that
  advertise the rewrite capability but carry no process device color
  observation (`DeviceGray` / `DeviceRGB` / `DeviceCMYK`). Process-color
  entries without a color-operator source range are reported separately as
  `MissingColorSource`; entries with multiple sourced process-color operands
  are reported as `AmbiguousColorSource`.
- `ConvertColor` process eligibility is a pure planning diagnostic: it never
  converts color or rewrites operands. A target must advertise
  `RewriteColorOperand` and carry exactly one sourced process device color
  observation, with no unsourced process device observations. Non-process
  observations may coexist with that single sourced process operand.
  `SpreadText` and `MinimumStrokeWidth` stay capability-only.
- `ActionPlan` now carries report-only `PlannedPatch` records for targets with
  concrete future mutation boundaries. For `ConvertColor`, each planned patch
  records the selected `ObjectId`, required `EditCapability`, entry page/scope,
  and the exact `ByteRange` of the sourced process color operator. This is
  boundary metadata only; it does not read, decode, serialize, or mutate PDF
  bytes.
- Actions are requests only; they do not mutate documents directly.
- Depends on `presslint-selectors` for selector data and `presslint-core` for
  object identities and edit capabilities.
- The public JSON encoding of `Recipe`, `RecipeStep`, every `Action` variant,
  `PatchPlan`, `PatchPlanMode`, `ActionPlan`, `PlannedPatch`,
  `MutationBoundary`, `SkippedTarget`, and every `SkipReason` variant is locked
  by focused serde shape tests. Each fixture asserts a full round-trip and pins
  the externally-tagged `action`/`reason`/`kind` field names and `snake_case`
  variant names exactly as the current `#[serde(...)]` attributes emit them.
- A Criterion benchmark target `actions`
  (`benches/actions.rs`, `harness = false`) measures the action-planning hot
  path without touching production source or public contracts. It mirrors the
  `presslint-inventory` bench: synthetic public content streams are tokenized
  and assembled via `presslint_syntax::{tokenize, assemble_operators}` and built
  into inventories with `presslint_inventory::build_inventory` once, outside the
  timed loop, so only the planner/matcher are measured.
  - `actions/plan_recipe` times `plan_recipe` over three synthetic inventories —
    `small_mixed` (sourced process-color fills plus a default-color skip),
    `large_repeated_targets` (target/patch-heavy), and `many_skip_few_target`
    (skip-branch-heavy with a small target tail) — reporting inventory entries
    per second via `Throughput::Elements`. The `ConvertColor`/`Selector::All`
    recipe exercises both the target/patch and skip (`MissingColorSource`)
    branches.
  - `actions/selector_matches` times `presslint_selectors::matches` over a large
    diverse inventory (vector/text/image/form entries) with a multi-predicate
    `Or`/`And` selector, reporting entries per second.
  - Adds only `criterion` and `presslint-syntax` as `[dev-dependencies]` (both
    workspace dependencies); no production code, public types, serde shapes, or
    planner behavior changed.
- Tests are split into `src/tests.rs` (planner behavior plus legacy shape
  tests), `src/tests/patch_boundary.rs` (boundary planning and JSON shape
  tests), and `src/tests/json.rs`, a dependency-free in-memory JSON serde
  harness modeled on `presslint-selectors` and extended with `bool`/`f64`
  scalars for the action payloads. `src/lib.rs` holds production code only. No
  `serde_json` or other dependency is added.

## Follow-Ups

- Add the first executor only after patch byte serialization and mutation
  boundaries are designed.
