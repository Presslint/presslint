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
- Depends on `presslint-selectors` for selector data and `presslint-types` for
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
- T116 typed the public `MutationBoundary` contract as a four-variant
  serde-tagged enum in `src/patch.rs`: `ContentStreamOperand`,
  `DictionaryEntry`, `WholeStream`, and `IndirectObjectClone`, with supporting
  enums for dictionary operations, value locators, value provenance, and planned
  object allocation. The old `ContentStream` boundary was renamed to
  `ContentStreamOperand`; live planning still emits only that variant.
- The three indirect-object boundary variants are contract-only shapes for the
  future incremental-update executor and are currently exercised only by serde
  shape tests. They carry concrete `presslint_pdf::IndirectRef` values and
  non-optional `IndirectObjectEditDecision` ownership decisions.
- `presslint-actions` now has a normal first-party dependency on
  `presslint-pdf` for `IndirectRef` and `IndirectObjectEditDecision`. The
  dependency direction remains `actions -> pdf -> types`; no cycle is
  introduced.
- `ContentStreamOperand.ownership` is optional because the live inventory
  `ObjectId` is a content-derived identity, not a PDF indirect reference. The
  planner records `ownership: None` instead of synthesizing a fake reference or
  decision.
- `MutationBoundary` and `PlannedPatch` intentionally keep `PartialEq` but no
  longer derive `Eq`, because `PlannedValueProvenance::ActionGenerated` carries
  `Action`, and some action payloads contain `f64`.
- This is a data-shape contract change only. Target selection, patch selection,
  skip taxonomy, emitted target order, and emitted patch order are unchanged;
  no hot-path impact is expected and no new benchmark was added.
- Added `docs/mutation-boundary-contract.md` as public design notes for the
  boundary model and the shared-object ownership rule. F3b remains next:
  `IncrementalRevisionPlan` / dirty-object revision-plan contracts plus
  `docs/incremental-update-contract.md`, still contract-only and with no byte
  mutation until an explicit go-ahead.
- T119 added a report-only `Action::SetPageBox(SetPageBox)` variant.
  `SetPageBox { media_box: Option<PageRectangle>, crop_box: Option<PageRectangle> }`
  reuses `presslint_pdf::PageRectangle`. It is a page-dictionary action, not a
  content-inventory action, so `Action::required_capability` now returns
  `Option<EditCapability>` (`None` for `SetPageBox`) and `plan_step` emits an
  empty plan for capability-less actions — the inventory recipe planner does not
  select page-dictionary edits.
- `plan_set_page_box_boundaries(action, target, ownership, media_locator,
  crop_locator) -> Vec<MutationBoundary>` plans `dictionary_entry` boundaries:
  one per requested-and-located box (`MediaBox` before `CropBox`). Keys are
  stored as slashless `PdfName` bytes, the op is derived from the locator
  (`existing_value` → `replace`, `insertion_point` → `insert`), the caller's
  ownership decision is recorded verbatim, and the value provenance is set to
  the `SetPageBox` action. It is planning metadata only: it never calls
  `presslint-write`, reads PDF bytes, or gates on the disposition (that is the
  writer's responsibility). The byte writer lives in
  `presslint-write::set_page_boxes_incremental`.
- The `SetPageBox` action JSON shape is locked by
  `action_variants_have_stable_json_shape`; planner behavior (replace+insert,
  request/locator gating, verbatim ownership) is covered by `set_page_box`
  tests in `src/tests/patch_boundary.rs`.

## Follow-Ups

- Add the first executor only after patch byte serialization and mutation
  boundaries are designed.
