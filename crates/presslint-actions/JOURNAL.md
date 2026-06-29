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
- Tests are split into `src/tests.rs` (planner behavior plus legacy shape
  tests), `src/tests/patch_boundary.rs` (boundary planning and JSON shape
  tests), and `src/tests/json.rs`, a dependency-free in-memory JSON serde
  harness modeled on `presslint-selectors` and extended with `bool`/`f64`
  scalars for the action payloads. `src/lib.rs` holds production code only. No
  `serde_json` or other dependency is added.

## Follow-Ups

- Add the first executor only after patch byte serialization and mutation
  boundaries are designed.
