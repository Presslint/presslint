# presslint Journal

## T161a - OutputIntent Observation Bridge

- Added `observed_output_intents_from_pdf`, the umbrella-only bridge from
  neutral `presslint-pdf` output-intent facts into
  `presslint-color::ObservedOutputIntent`.
- Keeps crate boundaries intact: PDF observation remains in `presslint-pdf`,
  color policy stays in `presslint-color`, and unsupported PDF-side entries
  remain structured PDF diagnostics rather than color-layer inputs.

## T160 - Transparency Group Audit Findings

- `GraphicsStateFindingSource` gains additive page/form transparency-group
  variants. The audit emits the page variant when a page dictionary contains
  `/Group << /S /Transparency ... >>`, with `transparency=true` and
  `unclassified=true` only for malformed or out-of-scope group safety fields.
- Page `/Group` facts are derived from the new pdf-side page-group inspector and
  are emitted alongside each page's ExtGState findings in document order.
  Malformed or unknowable page `/Group` entries become additive transparency
  group coverage gaps instead of being reported as ExtGState facts.

## T158 - Audit graphics-state findings, page scope (Phase 1-4)

- `ColorUsageAudit` gains ONE additive top-level field:
  `#[serde(default, skip_serializing_if = "Vec::is_empty")] graphics_state_findings:
  Vec<GraphicsStateFinding>`. An empty vec is omitted from serialization, so
  every existing audit JSON shape is byte-identical, and old JSON deserializes
  through `default`. The new `graphics_state_findings.rs` module owns the
  finding types and derivation (the conditional split target from the brief;
  `color_audit.rs` stays at 697 lines).
- `GraphicsStateFinding { page, source, overprint, transparency, unresolved,
  unclassified }` (serde snake_case, shape-locked by a new pinned serde-value
  test). `GraphicsStateFindingSource` has `PageExtGState` and `FormExtGState`;
  this slice EMITS ONLY the page variant — the form variant is the declared
  contract for the deep form-scope follow-up in the `/Group` slice era. At most
  one finding per page per source: the booleans aggregate over the page's
  classified `/ExtGState` resources; absent or all-trigger-free resources
  produce no finding.
- SEMANTICS: declared-in-resources presence. A resource declared but never
  selected by any `gs` still counts; per-`gs` usage precision belongs to the
  convert-guard slice (which will reuse this derivation on the write side).
  Booleans: `overprint` = `OP`/`op` written `true` or `OPM` written `1`/other
  classified non-default token (ISO 32000-1 §8.6.7 — `OPM` alone modulates
  rather than enables, but it is declared overprint-relevant state);
  `transparency` = non-opaque
  `CA`/`ca`, non-`/Normal` blend mode (including unrecognised/array shapes), or
  present `SMask` (§11.3.5); `unresolved` = an all-`Unresolved` env entry (a
  named slot that could not be classified); `unclassified` = any parameter
  `Unclassified` or `has_unclassified_keys` (partial classification is worth
  surfacing on its own).
- Findings-vs-gaps discipline: detection success is a finding, inspection
  failure is a gap, never both for one fact. `CoverageGapKind` gains the
  additive `ExtGStateResourceInspectionError` (the pass could not begin;
  document-anchored, mirrors the colour-space pair — a compressed offset-only
  root now honestly reports it, symmetric with the existing XObject/colour-space
  inspection-error gaps) and `ExtGStateResourceSkipped` (a skip the derivation
  cannot see: unnamed non-absence skips, and a named duplicate shadowed by an
  earlier classified entry). Named skips the env surfaces as all-`Unresolved`
  entries are findings, not gaps; absence (`MissingExtGState(Resources)`,
  missing `/Resources`) is neither.
- DATA PATH and purity: the Phase 1-3 bridges run the inspection but drop the
  reports, and `PdfInventory`'s serde shape must not change, so
  `audit_color_usage` re-runs `inspect_document_page_extgstate_resources_with_lookup`
  itself (dictionary-only, one bounded page-tree walk, no content-stream work)
  and folds the result into the report through a private `build_audit`.
  `build_color_usage_audit(inventory)` stays pure and behaviour-identical
  (empty findings, no `ExtGState` gaps; now test-only). Findings reuse the
  exact `page_extgstate_env` mapping the walker consumes, so audit and paint
  agree on the classified vocabulary by construction.
- Tests: existing audit pins untouched and green; new pinned serde-value shapes
  for the finding, both source strings, and both gap kinds; a pinned
  finding-bearing audit entry (declared-only resource, no `gs` in content);
  behavioural cases for OP-true + OPM-1 aggregation, BM Multiply, CA 0.5,
  unresolved entry (finding not gap), `/LW`-only and malformed-value
  unclassified, all-default/no-`ExtGState` (no finding), non-dictionary
  `/ExtGState` (gap not finding), begin-failure (document-anchored gap), and
  duplicate-name shadowing (finding plus gap for distinct facts).
- Cost class: one extra dictionary-only resource pass per audited document
  (same class as the audit's existing document access); the report grows only
  on finding-bearing pages; no content re-reads, no stream or profile bytes
  retained. Every changed file stays under 1000 lines.

## T157 - Wire page- and form-scope ExtGState environments (Phase 1-3)

- Turns the classified-`ExtGState` read path ON end-to-end, mirroring the
  colour-space flow exactly. `document_inventory.rs` gains the bridge helpers:
  `extgstate_env_resources` maps a scope's `presslint-pdf` classified resources
  (T155) into paint-native `ExtGStateResource`s (T156), and `page_extgstate_env`
  is the thin per-page adapter (mirrors `color_space_env_resources` /
  `page_color_space_env`). Per-parameter mapping: pdf `Unset -> GsParam::Default`,
  `Set(v) -> Set(mapped v)`, `Malformed -> Unclassified`. `BM` names are
  reclassified here per ISO 32000-1 §11.3.5 — `Normal`/`Compatible -> Normal`,
  the standard separable (Table 136) + non-separable (Table 137) names ->
  `NonNormal`, any other name or array-form list -> `OtherNamed` (the pdf side
  only splits `Normal` from the rest, so the real vocabulary lives in the seam).
- An UNRESOLVED entry (a named `/ExtGState` slot that could not be classified —
  unresolved reference, non-dictionary entry, malformed entry dict) surfaces in
  the env with EVERY param `Unresolved`, so `gs` on that name is honestly unknown
  rather than silently swallowed; a named skip whose resource is already
  classified is dropped. Resources-level skips (no resource name) cannot key an
  env entry and are deferred to the audit/guard slices (1-4/1-5); no report serde
  field was added, so all page/report shapes stay untouched.
- PAGE scope: both inventory bridges (`document_inventory.rs` classic,
  `pdf_inventory.rs` neutral) inspect page `/ExtGState` alongside the colour-space
  pass and build the page env once per page (shared `page_extgstates_at` helper);
  a begin-failure yields an empty env (legacy no-op). FORM scope:
  `form_expansion_machine.rs` inspects each descended form's OWN `/ExtGState` via
  the T155 form inspector and builds a form-LOCAL env (no page inheritance, same
  rule as colour spaces). The root and descend `PaintSubProgram` literals now pass
  the real envs instead of `ExtGStateEnv::empty()`.
- Identity untouched: the classified state rides only the walk snapshot, never an
  `InventoryEntry`, so digests are snapshot-independent. All six form-expanded
  goldens, the inventory bit-identity/corpus goldens, and every umbrella test pass
  byte-untouched even though the fixtures now walk through the new (empty-env)
  code path. New `tests/extgstate_wiring.rs` proves the behavioural effect end to
  end (page `gs` hit carries `Set(true)`, the set/unset/alpha/OPM/SMask/BM mapping
  table, a miss on a non-empty env goes all-`Unresolved`, an unresolved entry
  surfaces all-`Unresolved`, a page without `/ExtGState` stays legacy, and a
  populated env leaves the page inventory identity byte-equal); `tests/form_inventory.rs`
  adds the form-local + no-inheritance assertions. The behaviour is observed with a
  direct paint walk because the snapshot deliberately does not reach the inventory
  report.
- `build_classic_pdf_inventory`/`build_pdf_inventory` are a couple of lines over
  the clippy per-function default with the third resource pass added; the mapping
  is fully factored into helpers, so both carry a scoped
  `#[allow(clippy::too_many_lines)]` with a rationale rather than an artificial
  split. Cost class: one extra dictionary-only resource inspection per page and
  per descended form; envs built once per scope and borrowed by the walk, no
  content re-reads. Every changed file stays under 1000 lines.

## T156 - Mechanical ExtGState env threading (Phase 1-2 ripple)

- `form_expansion_machine.rs` constructs `PaintSubProgram` with the new
  `extgstate_env` field set to `ExtGStateEnv::empty()` at the root- and
  descend-sites: the classified graphics-state feature stays OFF here until the
  wiring slice populates page- and form-scope environments. No behaviour
  change; form-expanded goldens byte-identical.

## T153 - Born-final form-expanded identity (entry identity v3)

- The machine-driven form-expansion adapter now builds each expanded entry's
  identity born-final via `presslint_inventory::expanded_entry_identity`: it stamps
  the current page-global sequence and folds the machine's `InvocationPath` into the
  digest in one construction. The post-hoc `id.sequence` rebase (which left the
  digest computed from a contradictory form-local sequence) is deleted, and the
  invocation path folded into the digest is the same object published in
  `Provenance.invocation` — one source.
- Sequence VALUES are unchanged (page entries keep their walk-local sequence;
  expanded entries continue at the page-inventory length in splice order); only
  digests move, per the versioned identity-v3 break. The shared-form-invoked-twice
  golden was regenerated and now asserts the two invocations' digests are DISTINCT;
  the nested, cycle, max-depth, budget, and image/form-conflict goldens were
  regenerated by running the fixtures (entry order, scopes, sequences, and skip
  lists unchanged). Added an umbrella determinism-and-uniqueness test.

## T152 - Populate form invocation provenance

- The machine-driven form expansion adapter now stamps form-expanded inventory
  entries with the machine's `InvocationPath` at the same point it rebases
  page-global sequence numbers. Page-level entries remain `None`; identity and
  golden digest behavior are unchanged.

## T151 - Ablation: fold the resolver's cycle index into the active stack

- Behaviour-preserving cleanup of `form_expansion_machine.rs`. The resolver held
  the active descent path twice: an `active` stack (`InvocationPath -> slot`) and
  a parallel `visited: BTreeSet<FormObjectKey>` inserted/removed in lock-step with
  it. `visited` was a pure derived index of `active`, so it is removed; active-path
  cycle detection now scans the `active` stack directly (`active.iter().any(|(_, a)|
  a.key == key)`). The stack is bounded by `max_depth` (default 8), so the scan is
  cheap and drops one `BTreeSet` allocation per form-bearing page walk plus the
  `std::collections::BTreeSet` import. No public surface, semantics, or golden
  changed: the six T147 goldens (incl. the self-referential-cycle and
  shared-form-twice locks) stay byte-untouched and green.

## T151 - Flip to the Machine Path; Delete the Manual Recursion (Phase 0b-4b)

- Form expansion now runs on the shared paint traversal machine; the manual
  recursion is removed. `build_page_inventory_with_forms` (public signature
  unchanged) is now the machine-driven entry in `form_expansion_machine.rs`,
  re-exported through `form_inventory` so the umbrella consumers
  (`pdf_inventory`, `document_inventory`) keep importing it from the same path.
  The machine is the single source of traversal truth: the umbrella no longer
  re-walks graphics state to pair invocation names.
- Deleted the old path from `form_inventory.rs`: the `FormExpansion` struct with
  `expand`/`consume_expansion_budget`/`decode_form`/skip helpers, and the
  redundant `form_invocation_names` graphics-state re-walk. `form_inventory.rs`
  now owns only the caller-facing types (`FormExpandedInventory`,
  `SkippedFormInventory`, `SkippedFormInventoryReason`) and the bounded
  `FormWalkContext`. The now-unused private `visited`/`FormObjectKey` state was
  dropped from `FormWalkContext`; cycle keying lives solely in the resolver.
  `FormWalkContext` semantics are unchanged (depth 8, budget 256,
  consumed-pre-work-not-restored, active-path cycle key), now enforced only
  through the resolver.
- Retired the T150 differential scaffold coherently: `form_expansion_machine`
  is no longer `#[cfg(test)]`-gated, and `tests/form_expansion_differential.rs`
  is deleted (its six fixtures map one-to-one onto the six T147 goldens, so it
  added no coverage the goldens lack once the old path it compared against is
  gone). The `*_using(..., machine: bool)` fixture switch and the
  `*_machine` helper variants in `tests/form_inventory.rs` are simplified back
  to the single path.
- PROOF OF IDENTITY: the T147 golden module passed byte-untouched (empty diff on
  `form_inventory_golden.rs`, all six goldens green now driving the machine via
  the public entry), and the broader `form_inventory` suite passed with no
  expectation changes. Live check: `presslint audit --json` over a synthetic
  form-heavy page (nested `/A -> /B` with CMYK-then-RGB fills) inventories
  identically, surfacing both nested form invocations and the RGB fill inside
  the nested form.
- Bounded-traversal parity with the retired manual recursion: the machine's
  `FormProgramArena` is a lazy `OnceLock` tree, not an eager reachability walk.
  `build` seeds only the page's root form slots (a target clone, no structural
  inspection); a form's content is decoded and its `/Resources` inspected exactly
  once, the first time a permitted invocation resolves it in `resolve_form`
  (after the cycle/depth/budget checks), and preparing a form materialises empty
  child slots that are themselves inspected only if invoked. A declared-but-never-
  invoked, over-budget, or over-depth form is never touched, so the flip keeps the
  old lazy cost profile and drops `form_invocation_names`' redundant graphics-state
  re-walk. The `shared_form_reached_by_two_non_cyclic_branches` golden confirms a
  form reached by two branches still inventories identically under per-branch
  preparation.

## T150 - Machine-Driven Form Expansion Parallel Proof (Phase 0b-4a)

- Added a private parallel form-expansion adapter in `form_expansion_machine.rs`
  that drives `presslint-paint`'s call machine while leaving the public
  `build_page_inventory_with_forms` entry on the existing recursive path. The
  resolver preserves today's policy order: missing targets are silent skips,
  cycle/depth skips do not consume budget, budget is consumed before content
  work, content failures keep that budget consumed, resource coverage skips are
  recorded in the existing order, and active form keys are removed on the paint
  return hook.
- The adapter builds the page inventory exactly as today, uses the machine as
  call-order truth, matches local form entries to streamed callee ops by decoded
  record range, rebases only form-path `id.sequence` values onto the page-global
  counter, and deliberately does not recompute digests so the frozen v2 identity
  quirk remains byte-identical.
- Added `form_expansion_differential.rs` over the six golden fixtures
  (shared-twice, nested, cycle, max-depth, budget-exhaustion, image/form
  conflict), asserting full `FormExpandedInventory` equality between the old
  and machine paths. Existing T147 goldens remain unchanged.
- Added a direct `presslint-paint` dependency for the umbrella crate because the
  parallel adapter consumes machine types directly. The new path is test-only in
  this slice, so there is no production hot-path change; the arena prebuild
  keeps decoded form buffers and resource vectors owned for the page walk.

## T147 - Form-Expanded Inventory Golden Oracle (Phase 0b-1)

- New tests-only module `src/tests/form_inventory_golden.rs`: a golden lock for
  the form-expanded inventory path (`build_page_inventory_with_forms`) ahead of
  upcoming internal refactors that will migrate the manual `FormExpansion`
  recursion onto a paint-side call/return machine. Mirroring the
  `presslint-inventory` bit-identity lock, each fixture pins as inline consts
  the full ordered entry identity — `id.sequence` AND `id.digest` per entry —
  plus the kind vector, the `provenance.scope` vector, and the ordered
  `(name, reason)` skip list; comparisons are plain Rust values, no new
  dependency.
- Six fixtures over the existing synthetic classic-xref builders: (1) one form
  invoked TWICE by the page — pins today's flat-model fact that expansion
  rebases `id.sequence` onto the page-global space but never recomputes
  `id.digest` (computed in the form's own walk from its form-local sequence),
  so double invocations share IDENTICAL digests under DIFFERENT sequences; a
  future, deliberate invocation-identity change must move this golden visibly;
  (2) nested form — pins depth-first emission (callee entries immediately after
  their invocation entry, one monotonic sequence space seeded at the page
  inventory length) and the collapsed innermost-name scope; (3) self-referential
  form — pins the `Cycle` skip with no entries from the refused descent;
  (4) depth chain under `FormWalkContext::new(2)` — pins the `MaxDepth` skip;
  (5) shared form under `with_budget(8, 3)` — pins the `BudgetExhausted` skip
  and the exact interleaving `[page inv, A inv, C vector, page inv, B inv]`
  with sequences `[0, 2, 3, 1, 4]`; (6) image/form name conflict — pins that
  image classification wins, the invocation is an Image entry, and no form
  expansion or skip happens.
- Zero production changes: the only non-golden edits are in
  `src/tests/form_inventory.rs` — `pub(super)` visibility on existing fixture
  helpers (`classic_pdf`, `stream_object`, object-builder helpers,
  `expand_first_page_with_context`) so the golden module reuses known-good
  fixtures instead of duplicating them, plus a shared
  `expand_first_page_with_extra_images` variant (an appended image-name list)
  so the name-conflict golden reuses the one pipeline definition instead of
  duplicating it — plus registering the module in `src/tests.rs`.
- No hot-path interaction: fixtures are hundreds-of-bytes synthetic PDFs and
  the golden comparison is O(entries); no benchmark required.

## T138 - Form-Scope Resource Colour-Space Tracking Wiring (F4-6 slice 1b)

- `FormExpansion::expand` now inspects each target form's OWN
  `/Resources /ColorSpace` via `inspect_form_color_space_resources`, builds a
  LOCAL `Vec<ColorSpaceResource>` from it, and walks that form's content through
  `build_inventory_with_color_space_env(ColorSpaceEnv::new(&form_color_spaces))`
  instead of the no-env `build_inventory`. So a form's `cs`/`scn` resolve against
  the FORM's own colour spaces, reporting the real
  `IccBased`/`Separation`/`DeviceN` family. The env borrows into a per-form local
  vector that lives only for that synchronous form inventory and is dropped when
  `expand` returns — no env is merged across the page/form/nested-form boundary.
- No page inheritance: the env is built ONLY from the form's own report, never
  the invoking page's colour spaces. A nested form invoked inside a form is
  inspected for ITS OWN `/Resources /ColorSpace` in the same recursive `expand`
  step and gets its own local env; a nested form with no `/ColorSpace` does NOT
  see the invoking form's spaces (ISO 32000-1 §7.8.3 + §8.10.2 Table 95).
- The umbrella mapper `color_space_env_resources` is generalised to take
  `&[ClassifiedColorSpaceResource]` (from a page report OR the new form report);
  the two page bridges keep a one-line call site through the thin per-page
  adapter `page_color_space_env` (`&page.color_spaces`), and the form bridge
  calls `color_space_env_resources(&form_report.color_spaces)` directly.
- READ-ONLY / no-audit-plumbing scope: the write/convert path is untouched and
  the converter's up-front selector rejection is preserved. NO new
  `SkippedFormInventoryReason` colour-space variant and NO `ColorAudit`
  coverage-gap plumbing for form colour-space skips this slice (deferred to avoid
  the enum ripple); the form colour-space skip buckets live only in the
  `presslint-pdf` report for tests.
- Copy budget: one `/Resources /ColorSpace` inspection per expanded form (bounded
  by the existing `FormWalkContext` depth + budget + cycle detection), reusing the
  same lookup/inheritance machinery. The per-form env is a borrow into a local
  vector, not a per-operator clone; the decoded object bytes are not retained.
- BEFORE/AFTER (verified on a synthetic local demo only). A common real-world
  shape is a page whose only marking is `/Fm Do` while the painted colour spaces
  are declared in the FORM's OWN `/Resources /ColorSpace`; under page-scope 1a
  those form `cs`/`scn` operators surface as unresolved
  `ColorSpace::Resource(/CS0)`/`Resource(/CS1)` gaps. After this slice those
  form-declared spaces RESOLVE. Reproduced end-to-end through the real CLI on a
  synthetic local-only demo (a page that only does `/Fm Do`, whose form's OWN
  `/ColorSpace` declares `/CS0 [ /ICCBased … /N 4 ]` and `/CS1 [ /Separation
  /Spot1 /DeviceCMYK … ]` and whose content does `/CS0 cs … scn /CS1 cs … scn`):
  `presslint audit --json` reports `icc_based`:1 + `separation`:1 (spot `Spot1`),
  zero `resource` observations, zero coverage gaps — where the pre-1b form walk
  produced two unresolved `Resource(/CS…)` observations. Device-only and
  page-only-colour PDFs are byte-identical in inventory + audit (regression tests
  `page_without_form_invocations_is_unchanged`,
  `device_operators_are_unchanged_by_a_populated_environment`).

## T137 - Page Resource Colour-Space Tracking Wiring (F4-6 slice 1a)

- Both inventory bridges (`build_classic_pdf_inventory`, `build_pdf_inventory`) now
  run the `presslint-pdf` page colour-space resource pass alongside the `/XObject`
  pass and thread each page's classified colour spaces into the walker via a borrowed
  `ColorSpaceEnv`. `color_space_env_resources` maps a `presslint-pdf`
  `ClassifiedColorSpaceResource` (structural family) into the inventory-native
  `ColorSpaceResource` — the crate-layering seam. The alternate space is a recorded
  fact and is deliberately NOT substituted. New non-fatal `color_space_resource_error`
  + per-page `color_space_resource_skipped` mirror the existing `xobject_resource_*`
  fields. Shared `split_color_space_report` helper keeps both bridges under the
  100-line pedantic gate; `backend_lookup` factors the neutral backend dispatch.
- `audit_color_usage`: resource colours resolved to their real family
  (`IccBased`/`Separation`/`DeviceN`) are now HONEST observations, no longer
  `UnmodeledColorSpace` coverage gaps — only still-unresolvable spaces
  (`Lab`/`CalGray`/`CalRGB`/`Indexed`/`Pattern`/`Resource(_)`/non-image `Unknown`)
  remain gaps. New coverage-gap kinds `ColorSpaceResourceSkipped` (a present-but-
  unresolvable page colour space hides colour) and `ColorSpaceResourceInspectionError`
  (the document-level pass could not begin, e.g. a compressed root — honest, not a
  hard failure); `MissingColorSpaceResources`/`MissingColorSpace` are NOT gaps (no
  colour to miss).
- Read-only: the write/convert path is untouched and the converter's up-front
  selector rejection for resource colour spaces is preserved. Scope is PAGE content
  streams; form XObject resource colours are a counted skip (slice 1b). Existing
  audit tests updated for the corrected behaviour (an `IccBased` observation is no
  longer an unmodeled gap; the compressed-leaf audit tolerates the new colour-space
  inspection-error gap) — no test weakened.
- Resource colour visibility note: before this slice, `cs`/`scn` were `NoOp`
  and resource colour usage was invisible in the audit. After it, the audit sees
  and attributes-by-name page-scope resource colour usage. Colour spaces declared
  only in FORM XObject `/Resources /ColorSpace` remain outside page-scope 1a, so
  they honestly surface as `ColorSpace::Resource(name)` -> `UnmodeledColorSpace`
  gaps rather than resolved `IccBased`/`Separation`; completing that resolution is
  exactly slice 1b (form-scope resource tracking).

## T134 - Compressed-Leaf Content Inventory Wiring

- `decode_page_content` now handles the additive
  `DocumentPageContentExtentResult::CompressedLeafInspected { extents, .. }` variant
  by inventorying via its `extents` exactly like an `Inspected` page (merged into
  the same match arm). So `build_pdf_inventory`, and therefore `audit_color_usage`
  and `query_pdf_inventory`, now report REAL colour observations for a compressed
  leaf `/Page` whose `/Contents` was read from its resolved `/ObjStm` body and whose
  referenced content-stream object is an ordinary uncompressed object. The resolved-
  body provenance boundary is enforced entirely in `presslint-pdf`; this crate just
  consumes the source-valid `extents`.
- Honest compressed-content-target skip: when a reached compressed leaf's
  `/Contents` target is ITSELF compressed, the resolved `extents` carry a single
  located-skip, so `decode_page_content` surfaces
  `InventoryPageSkip::TargetSkipped` → `PdfInventoryPageResult::Skipped` → an
  `Incomplete` audit with a coverage gap and zero colour — reached but honestly not
  inventoried, never fabricated. The pre-existing `PdfInventorySkip::CompressedLeaf`
  path is retained for leaves whose body/`/Contents` is un-inspectable.
- READ-ONLY: no writer/edit change. The colour converter and content-edit pipeline
  still drive the offset-based extents bridge and never see the compressed-leaf
  variants; compressed-leaf CONVERSION is a separate, later, resolved + ownership-
  safe slice.
- Tests (SYNTHETIC only) live in a new sibling module
  `tests/pdf_inventory_compressed_leaf.rs` (keeps `tests/pdf_inventory.rs` under the
  1000-line gate): two compressed leaves referencing uncompressed RGB content now
  produce `Inventoried` pages and real `audit_color_usage` RGB findings with zero
  `SkippedPage` gaps; a compressed leaf whose `/Contents` target is itself compressed
  stays a `TargetSkipped` skip with an `Incomplete`/no-colour audit. The real
  corpus was not re-run here (not present in this tree); the operator should confirm
  the `SkippedPage`-gaps -> real-colour before/after on a LOCAL-only file.

## T133 - Compressed Page-Tree Navigation In The Inventory Bridge

- `build_pdf_inventory` now re-resolves the page-tree root to body-aware
  `ResolvedObjectData` via `resolve_object(input, lookup,
  access.page_tree_root.reference, max_decoded_stream_bytes)` and routes through
  the new `inspect_document_page_content_extents_resolved` bridge (extracted into
  a `resolved_page_content_extents` helper to keep the function within the
  line-count gate). `access.page_tree_root` is a `ResolvedStructuralObject`, which
  does NOT retain the decoded bytes of a compressed root, so the re-resolve is
  required. Exactly ONE `resolve_object` runs for the root; the resolved leaf
  enumeration owns any bounded object-stream decode. A root resolve failure maps
  into the existing `PdfInventoryRejection::DocumentAccess` /
  `DocumentAccessRejection::PagesObject` error, mirroring `inspect_document_page_boxes`.
- Effect: a real xref-stream PDF whose page-tree root or an intermediate `/Pages`
  node is a compressed object-stream member no longer hard-fails the whole
  `audit`/`query`/convert path. `audit_color_usage`/`query_pdf_inventory` inherit
  the fix because they build on `build_pdf_inventory`.
- Honest coverage semantics: a compressed leaf `/Page` flows
  `DocumentPageContentExtentResult::CompressedLeaf` → `decode_page_content` returns
  `Err(InventoryPageSkip::CompressedLeaf { object_stream_number,
  index_within_object_stream })` → `PdfInventoryPageResult::Skipped` →
  `CoverageGapKind::SkippedPage`. The page is COUNTED as enumerated, carries zero
  colour observations, and yields exactly one page-scoped coverage gap — it is
  neither dropped silently nor claimed as full coverage. The new variant was
  threaded through both `InventoryPageSkip`, `PdfInventorySkip`, and
  `ClassicPdfInventorySkip` (the classic offset-based path never emits it, but the
  shared skip conversion stays total). Uncompressed leaves inventory exactly as
  before (byte-identical regression) and the classic-xref path is unchanged.
- Deferred (follow-up): inventorying compressed-LEAF CONTENT (reading a compressed
  leaf `/Page` dict's `/Contents` from its `/ObjStm`) needs a resolved `/Contents`
  path since `inspect_page_contents` is offset-only. Also out of scope this slice:
  the document-level `/XObject` resource pass still uses the offset-only root, so a
  compressed root adds one honest `ResourceInspectionError` coverage gap (not a
  hard failure).
- `presslint-write`'s `content_edit_pipeline` needed no behaviour change: both its
  `content_object_owners` (`if let Inspected`) and `locate_single_stream` (`let …
  else`) already skip any non-`Inspected` result, so a `CompressedLeaf` falls into
  the existing `NoContentStream` skip path (comments added).
- Tests (synthetic fixtures only): `build_pdf_inventory` over a compressed-root
  fixture returns `Ok` with two `Skipped { CompressedLeaf }` pages and zero colour
  entries; the audit over it counts two pages with two `SkippedPage` gaps and no
  colour observations; a compressed-intermediate-node fixture inventories its two
  uncompressed vector leaves normally. Real before/after recorded on a LOCAL-only
  corpus path (never committed): `audit` moved from hard-fail to returning.

## T122 - Single-Filter DecodeParms Arrays End To End

- No public surface change. `build_pdf_inventory` now inventories Flate pages
  and Flate content in xref-stream PDFs whose `/DecodeParms` is the single-filter
  array form `[null]` or `[<< ... >>]`, because the underlying
  `presslint_pdf::resolve_flate_decode_parameters` now resolves those forms to
  the same `FlateDecodeParameters` as the direct `null`/dictionary values (T122
  in the presslint-pdf journal). The page-content decode path was already shaped
  to accept any `Resolved` resolution, so raw streams stay borrowed and only the
  existing bounded decoded buffer is allocated for Flate streams.
- Added umbrella end-to-end tests over `build_pdf_inventory` for a classic-xref
  Flate page and an xref-stream Flate page, each with `/DecodeParms [ null ]`.

## T115 - Read-Only Document Color-Usage Audit (CHK2)

- Added `audit_color_usage(input, max_decoded_stream_bytes) -> Result<ColorUsageAudit, PdfInventoryError>`
  in the new `color_audit.rs` module: the second report-only prepress
  deliverable. It builds the neutral inventory with `build_pdf_inventory`
  verbatim, then scans the merged, page-ordered inventory once by borrow and
  moves it into the report. This is CHK2 — a DESCRIPTIVE color-usage audit, NOT
  a print-safe/pass-fail preflight and NOT an ink-limit/TAC check. It is a
  companion to the fixed-policy `check_no_rgb_in_print`, not a replacement; it
  plans nothing and mutates nothing.
- New public surface: `ColorUsageAudit { status, document, pages, spot_names,
  rgb_findings, coverage_gaps, inventory }`, `ColorAuditStatus`
  (`Complete | Incomplete` only — no pass/fail or print-safety claim),
  `ColorUsageSummary` + `ColorSpaceCount` / `ColorUsageCount` / `ObjectKindCount`,
  `PageColorUsage`, `RgbFinding`, and `CoverageGap` + `CoverageGapKind`.
  `build_color_usage_audit(inventory)` is the module-internal pure
  inventory-to-report split (exercised by synthetic tests), not re-exported.
- Counts are counted in a documented way: `ColorSpace` and `ColorUsage` counts
  are per `ColorObservation`; `ObjectKind` counts are per `InventoryEntry`;
  per-page summaries use each inventoried page's contiguous entry run, matching
  the `preflight.rs` cursor discipline. Because `ColorSpace`/`ColorUsage`/
  `ObjectKind` do not implement `Ord`, counts are deterministic sorted
  `Vec<...Count>` records ordered by a fixed variant rank (color spaces break
  `Resource` ties by raw `PdfName` bytes), not maps. One `PageColorUsage` is
  emitted per enumerated page in document order (empty summary for a skipped
  page).
- Spot extraction is conservative: `ColorObservation.spot_name` is collected
  only when the observation space is `Separation` or `DeviceN`. A `BTreeSet`
  both deduplicates and sorts by raw `PdfName` bytes.
- `DeviceRgb` observations are reported as explicit `RgbFinding`s (page, object
  identity, entry index, object kind, usage). RGB is a MODELED device space, so
  an RGB-only document is still `Complete`: findings describe observed color,
  they are not coverage gaps.
- Completeness semantics: `status` is `Incomplete` iff at least one coverage gap
  exists, else `Complete`. Coverage-gap policy — gaps are recorded for skipped
  pages, image `Unknown` observations (image color undecoded), `form_skipped`
  per-form expansion skips, page `xobject_resource_skipped`, the top-level
  `xobject_resource_error`, and any unmodeled/unresolved color space (`IccBased`,
  `CalGray`, `CalRgb`, `Lab`, `Indexed`, `Separation`, `DeviceN`, `Pattern`,
  `Resource(_)`, and non-image `Unknown`). `DeviceCmyk`/`DeviceGray` are the
  modeled process spaces and produce neither a finding nor a gap.
- Resource-skip nuance: `MissingResources`/`MissingXObject` skips mean the page
  simply has no `/Resources` or no `/XObject` dictionary — there is no XObject
  color to miss — so they are NOT gaps; every other resource-skip reason
  (present-but-unclassifiable target) is a `PageResourceSkipped` gap. This keeps
  a clean CMYK/Gray page (whose minimal fixture has no `/Resources`) `Complete`
  instead of noisily `Incomplete`.
- Copy budget: the full `PdfInventory` is moved into `ColorUsageAudit.inventory`
  exactly once (scanned by borrow, never cloned). Counts, findings, and gaps
  own only `Copy`/enum discriminants plus cloned `ObjectId`, `ColorSpace`, and
  spot `PdfName` boundary values; `ColorObservation.components`, decoded
  streams, image samples, ICC/profile bytes, and PDF source bytes are never
  copied into the report. No new benchmark target: this is a build-once +
  scan-once aggregation over the already-timed `build_pdf_inventory` path, the
  same shape as `query_pdf_inventory` and `check_no_rgb_in_print`.
- Tests (`src/tests/color_audit.rs`) cover clean CMYK/Gray completeness through
  the real PDF entry point, RGB finding collection (and RGB-stays-`Complete`),
  per-page/document counts + fixed-order determinism, spot-name dedup/sorting
  with a dropped non-`Separation` name, coverage-gap incompleteness (skipped
  page, unmodeled space, image `Unknown`, form skip, page resource skip,
  resource-inspection error), the benign `MissingResources`/`MissingXObject`
  non-gap case, and serde round-trips of every new public report contract.

## T112 - Surface Image `XObject` Dictionary Metadata

- `PdfInventoryPage` now exposes `image_xobjects:
  Vec<presslint_pdf::PageXObjectResourceTarget>`: the top-level page-scope image
  `XObject` targets for the page, each carrying the resolved reference/offset
  and the structural `ImageXObjectMetadata` (dimensions + colour-space family)
  added on the `presslint-pdf` side. Callers can now read image `/Width`,
  `/Height`, `/BitsPerComponent`, and a mapped `/ColorSpace` device family
  without decoding any image samples. Form targets are not surfaced here.
- The bridge fills the new field from the already-computed page
  `XObject`-resource report (`resources.image_xobjects.clone()`); when the
  resource pass is absent for a page the vector is empty. No new object reads,
  resolutions, or stream decodes; the page inventory result and skip shapes are
  unchanged. The `image_xobjects` vector serde-round-trips with the rest of
  `PdfInventoryPage`.
- Copy budget: only the small structural target records (name bytes, indirect
  reference, offset, scalar image metadata) already produced by the resource
  pass are cloned into the report; no PDF bytes, object/stream bodies, resource
  dictionaries, or decoded image data are retained.
- Note: adding the required `image_xobjects` field to `PdfInventoryPage`
  rippled to the two `PdfInventoryPage { .. }` test constructors in
  `src/tests/preflight.rs` (mechanical `image_xobjects: Vec::new()` only, no
  behaviour change).

## T111 - Bounded Recursive Form Inventory + ACT Coverage Hardening

- Raised the bridge walk from one-level to bounded recursive descent:
  `FormWalkContext::bounded_default()` is depth 8 with a per-page total
  expansion budget, while `new(max_depth)`, `with_budget(max_depth,
  max_expansions)`, and `one_level()` remain available for focused tests. The
  recursive `expand` machinery, active-path `visited` set, and merge ordering
  already existed from T110; this slice raises the cap and wires it into both
  public bridges.
- `PdfInventoryPageResult::Inventoried` and
  `ClassicPdfInventoryPageResult::Inventoried` now carry `form_skipped`, moved
  from `expanded.form_skipped`, so per-form skips are visible in document
  inventory reports instead of only through the direct form-expansion helper.
- Nested form `/Resources /XObject` classification skips from
  `inspect_form_xobject_resources` are now surfaced as
  `SkippedFormInventoryReason::Resource { skip }`, attributed to the enclosing
  form. Content skips, cycle skips, max-depth skips, and per-page
  `BudgetExhausted` skips remain structured `SkippedFormInventory` records.
- `check_no_rgb_in_print` no longer emits blanket `CoverageIncomplete` findings
  for every `FormXObject` inventory entry. Coverage review now comes only from
  skipped pages, image observations still modeled as `Unknown`, and real
  `form_skipped` diagnostics. The ACT behavior is now: `DeviceRGB` anywhere in
  walked page/form content is a `Fail`; a fully walked CMYK-only page/form tree
  `Pass`es with no findings.
- Tests cover nested RGB failure, CMYK-only nested pass, A -> B -> A cycle
  termination, max-depth review, shared-form two-branch walking, per-page budget
  exhaustion, nested page attribution, and nested resource diagnostics.
- Performance: no new benchmark target. This remains build-once page inventory
  plus bounded nested descents, capped by depth 8, active-path cycle detection,
  and a per-page total form expansion budget consumed before any form decode,
  tokenize, assemble, or inventory work. Raw form streams stay borrowed; Flate
  forms use the existing bounded decoded buffer; `form_skipped` and preflight
  findings retain no source bytes, object bodies, resource dictionaries, or
  decoded form streams.
- Next queued slice: IMG, decoding image `/Width`/`/Height`/
  `/BitsPerComponent`/`/ColorSpace` so image `Unknown` observations can be
  replaced with structured image metadata.

## T110 - One-Level Form `XObject` Content Inventory (FORM 11a)

- Added `form_inventory` module owning the recursion and merge for one-level
  Form `XObject` content expansion. `build_page_inventory_with_forms(input,
  lookup, page, page_index, max_decoded_stream_bytes, page_image_names,
  page_form_names, form_targets, FormWalkContext) -> FormExpandedInventory {
  inventory, form_skipped }` decodes/tokenizes/assembles/inventories the page
  exactly like the page-only path, then for each page-level form invocation
  entry walks the form's OWN decoded content one level deep and merges the
  nested entries immediately after the invocation entry.
- Physical flow (all in `presslint`): take each page-level form invocation's
  `PageXObjectResourceTarget` -> locate the form stream via
  `inspect_content_stream_data_extent_with_lookup(input, Some(lookup),
  object_byte_offset)` -> decode through the SAME single-stream
  filter/`/DecodeParms`/`FlateDecode` machinery (the newly `pub` `decode_content`
  helper in `page_content.rs`, bounded by `max_decoded_stream_bytes`) ->
  tokenize + assemble -> inspect the form's OWN `/Resources /XObject` via the
  new `presslint-pdf` `inspect_form_xobject_resources` -> re-invoke
  `build_inventory` on the decoded form bytes in `ContentScope::FormXObject {
  name }` with the ORIGINAL invoking `page_index`.
- ORIGINAL page index: nested entries are built with the invoking page's
  `page_index`, so ACT/selectors see form-contained objects on the invoking
  page. Verified by test (nested `id.page` / `provenance.page` == invoking page).
- Sequence rebasing: page entries keep their content-order sequence `0..n-1`;
  nested form entries are rebased onto a page-global counter that continues at
  `n`, so nested sequences are monotonically increasing and never restart at 0.
  A page with no form invocations continues to assign `0..n-1` and is
  byte-for-byte unchanged (regression test). The nested-entry digest is still
  computed from the form-local sequence inside `build_inventory` (its signature
  is fixed and its digest helpers are private to `presslint-inventory`); only
  `id.sequence` is rebased for page-global identity, which stays unique and
  deterministic because the form name scope and form-local ranges disambiguate.
- `FormWalkContext { max_depth, visited }` bounds the walk. For 11a
  `max_depth = 1`. `visited` keys forms on the active descent path by resolved
  `(object_number, generation)` plus byte offset, so a self-referential or
  cyclic form is a `SkippedFormInventoryReason::Cycle` (checked before the depth
  guard) rather than a page failure, panic, or infinite loop; a legitimate
  nested form beyond the max depth is a `MaxDepth` skip. `visited` is inserted on
  descent and removed on ascent, so its length is the current descent depth
  (the depth guard reads `visited.len()`) and sibling re-invocations of the same
  form are not false cycles. 11b only needs to raise `max_depth`.
- Structured per-form skips: `SkippedFormInventory { name, reference,
  object_byte_offset, reason }`. `reason` is `Cycle`, `MaxDepth`, or
  `Content { skip: PdfInventorySkip }`, where the content path reuses the
  existing `From<InventoryPageSkip>` conversion so unresolved / type-2 /
  generation-mismatch (via the form stream extent), unsupported-filter, and
  decode/tokenize/assemble/graphics-walk failures all become structured skips.
  The page's own text/vector inventory is always produced even when a form is
  skipped (tests cover self-ref cycle and unsupported filter).
- Bridges: both `build_pdf_inventory` (neutral, derived `ObjectLookup`) and
  `build_classic_pdf_inventory` (`ObjectLookup::ClassicXref`) route each page
  through the shared `build_page_inventory_with_forms`, passing the page's
  already-derived `form_xobjects` targets. The old page-only `build_page_inventory`
  was folded into this path. The public `PdfInventory` / `ClassicPdfInventory`
  report shapes and serde are UNCHANGED: form expansion enriches the merged
  `inventory`; the per-form skip diagnostics are exposed through
  `build_page_inventory_with_forms` (re-exported) for the later 11b/ACT slices
  rather than added to the page report structs (which would have required
  editing out-of-scope tests). `check_no_rgb_in_print` therefore now sees
  DeviceRGB painted inside page-level forms without any `preflight.rs` change.
- Performance: no new Criterion target. This is a build-once page walk plus one
  bounded nested walk per page-level form that reuses the already-benchmarked
  `build_inventory` and FlateDecode paths; the page hot loop is unchanged. The
  form-name correlation does a second `walk_graphics_state` pass ONLY when the
  page/form declares form resources (`form_names` non-empty), so pages without
  forms keep a single walk. Raw form streams stay borrowed; a `/FlateDecode`
  form allocates only the existing bounded decoded buffer. Report records retain
  no PDF source bytes, object bodies, resource dictionaries, or decoded form
  bytes; `FormWalkContext.visited` is a bounded `BTreeSet` of small `Copy` keys.
- Deferred (out of scope here): 11b bounded recursive descent (raise
  `max_depth`, descend nested forms), ACT status aggregation / surfacing
  `form_skipped` in the page report, image pixel/dimension/color-space decode
  (a form's image `Do` stays an `Unknown` image observation), `/BBox` / `/Matrix`
  geometry, filter arrays/chains per form stream, object-stream/type-2
  resolution, and annotation appearance streams.

## T108 - Ablation

- Doc-accuracy only, no behavior change: the `PreflightReason::UnmodeledOrUnresolvedColorSpace`
  doc listed the review-severity color spaces but omitted `CalGray`, which the
  wildcard match arm already routes to review; added it so the doc matches the
  code. The `DeviceCmyk | DeviceGray => None` arm and every other arm are unchanged.
- Added `pure_gray_page_passes_with_no_findings`, a focused test that pins the
  previously-untested `DeviceGray` half of the `DeviceCmyk | DeviceGray => None`
  pass-compatible arm (the CMYK half was already covered), protecting that
  simplification from regression.

## T108 - Read-Only `check_no_rgb_in_print` Preflight Over PDF Inventory

- Added `check_no_rgb_in_print(input, max_decoded_stream_bytes) -> PreflightReport`
  in the new `preflight.rs` module: the first real user-facing prepress
  deliverable. It builds the neutral inventory with `build_pdf_inventory`
  verbatim, then scans `report.inventory.entries` once and applies a fixed
  color policy. This is READ-ONLY: it lives in the umbrella crate, NOT
  `presslint-actions`; it plans nothing and mutates nothing.
- New public surface: `PreflightReport { check, status, findings, inventory }`,
  `PreflightStatus` (`Pass | Fail | NeedsReview`), `PreflightCheck`
  (`NoRgbInPrint`), `PreflightSeverity` (`Error | Review`), `PreflightReason`
  (`RgbDeviceColor | UnmodeledOrUnresolvedColorSpace | CoverageIncomplete`),
  and `PreflightFinding`.
- Pass/review/fail partition (the check owns this policy, it is not a thin
  selector wrapper): `DeviceRgb` in any marking observation is the only
  `Error` (`RgbDeviceColor`) and forces `Fail`. `DeviceCmyk` and `DeviceGray`
  are pass-compatible and emit no finding. Every other observed `ColorSpace`
  (`IccBased`, `CalRgb`, `Lab`, `Indexed`, `Separation`, `DeviceN`, `Pattern`,
  `Resource(_)`, `Unknown` on a non-image observation) is a `Review`
  (`UnmodeledOrUnresolvedColorSpace`). A marking object with multiple
  observations (fill + stroke) is scanned per observation, one finding per
  offending observation, in observation order, so `usage`/`color_space` stay
  precise.
- Status aggregation is exactly: `Fail` if any `Error`; else `NeedsReview` if
  any `Review` finding OR coverage gap (all coverage gaps are `Review`
  severity); else `Pass`. A clean `Pass` means "no observed DeviceRGB in
  inventoried marking content AND no review/coverage blocker", subject to the
  recorded coverage limits — it does NOT claim "no RGB anywhere".
- Three coverage-honesty signals, all `CoverageIncomplete`/`Review`: (a) every
  page whose `PdfInventoryPageResult` is `Skipped`; (b) every image observation
  modeled as `Unknown` (image color is not decoded yet); (c) one signal per
  `FormXObject` entry, because nested form content is not walked so RGB inside a
  form is currently invisible.
- Coverage-finding representation: object-anchored fields
  (`object`, `entry_index`, `kind`, `usage`, `color_space`) are `Option` and
  populated only for entry-anchored findings. Per-object color findings and the
  image-`Unknown` coverage finding carry all of them; the form coverage finding
  carries `object`/`entry_index`/`kind` but no color observation
  (`usage`/`color_space` are `None`); a skipped-page coverage finding carries
  only the page.
- Determinism: `collect_findings` walks pages in document order and entries in
  content order in lockstep, using each `Inventoried { entry_count }` to bound a
  page's contiguous entry run, so skipped-page and per-object findings interleave
  in strict document/page/entry/observation order in a single pass.
- Copy budget: the full `PdfInventory` is moved into `PreflightReport.inventory`
  exactly once (scanned by borrow, never cloned; no matched-entry clones).
  Findings own only `Copy`/enum discriminants plus a cloned `ObjectId` (small,
  no source bytes) and a cloned `ColorSpace` (scalar, or the `Resource` name it
  already carries). `ColorObservation.components`, decoded streams, and PDF
  source bytes are never copied into findings.
- No new benchmark target: this is a build-once + scan-once aggregation over the
  already-timed inventory build path, the same shape as `query_pdf_inventory`;
  selector/inventory throughput is already covered by existing Criterion benches.
- Next queue after ACT: FORM (Form `XObject` recursion + ACT hardening so RGB
  inside forms is caught), then IMG (image `/Width`/`/Height`/`/BitsPerComponent`/
  `/ColorSpace`), then S-a, the Y2 design note + Y2, then F3 (#29) design notes.

## T107 - Page `XObject` Resources in Real-PDF Inventory

- `build_pdf_inventory` now runs the new page `XObject` resource inspector
  through the same `ObjectLookup` backend selected by `inspect_document_access`,
  then passes each page's classified image/form resource-name lists into the
  existing combined `presslint_inventory::build_inventory` path. Real page-scope
  `/Im Do` and `/Fm Do` operators now produce image and form inventory entries
  when the page resources classify them as `/Subtype /Image` or `/Subtype /Form`.
- `build_classic_pdf_inventory` gets the same behavior via the classic
  `inspect_document_page_xobject_resources` wrapper. A shared page helper still
  decodes/tokenizes/builds inventory once per page, now with caller-supplied
  image/form name slices.
- Resource inspection is non-fatal for text/vector inventory. If the document
  resource pass cannot begin, the report records `xobject_resource_error` and
  pages inventory with empty image/form lists. Per-page resource diagnostics are
  exposed as `xobject_resource_skipped` on each page report.
- Duplicate raw names in a page's direct `/XObject` dictionary are surfaced as
  page-local diagnostics by `presslint-pdf`; the bridge receives disjoint
  image/form resource-name lists, with the first duplicate occurrence winning
  deterministically.
- Copy budget: raw content streams stay borrowed, Flate streams allocate only
  bounded decoded buffers, multi-stream pages allocate only the bounded joined
  content buffer, and the new resource bridge converts small raw PDF resource
  names into shared inventory `PdfName` values. No PDF source bytes, object
  bodies, resource dictionaries, image streams, or decoded image data are
  retained.
- Deferred: no Form XObject content recursion, no image pixel/filter/color-space
  inspection, no indirect `/XObject` subdictionary support, and no object-stream
  resolution beyond the existing structural lookup behavior.

## T106 - Document-Level Selector Query Over PDF Inventory

- Added `query_pdf_inventory(input, selector, max_decoded_stream_bytes)` in the
  new `pdf_query.rs` module: the first end-to-end "query a real PDF" path. It
  reuses `build_pdf_inventory` verbatim for the neutral document/page path, then
  scans the merged, page-ordered `report.inventory.entries` once, calling the
  already-benchmarked `presslint_selectors::matches` per entry.
- New public report types `PdfInventoryQuery { report, matches }` and
  `PdfInventoryMatch { entry_index, page_index }`. `entry_index` is a stable
  index into `report.inventory.entries`; `page_index` is the matched entry's own
  `entry.id.page` (the zero-based document-order ordinal threaded by
  `build_pdf_inventory`). Both derive `Debug, Clone, PartialEq, Serialize,
  Deserialize`; `PdfInventoryMatch` additionally derives `Copy, Eq`. The query
  result stays `PartialEq`-only because `PdfInventory` carries float
  bounds/components and is not `Eq`.
- Index-not-clone contract: matched entries are never cloned into the result.
  `matches` holds only `PdfInventoryMatch { usize, PageIndex }` (both `Copy`) and
  the full report is moved into `PdfInventoryQuery.report` exactly once. No
  source bytes, decoded streams, or entry payloads are copied by the query.
- The query is a strict superset (build, then select), so top-level failures
  surface as the same `PdfInventoryError` as `build_pdf_inventory`, unchanged.
  Matches are pushed in ascending `entry_index` order.
- No new abstraction beyond the query result pair; no new selector predicate, no
  JSON parsing, no CLI. `build_pdf_inventory` / `build_classic_pdf_inventory`
  behavior and serde shapes are untouched.
- No new benchmark target: this slice is a build-once + scan-once composition
  with no new hot loop; selector-matching throughput is already covered by the
  `presslint-selectors` Criterion bench. Stated here per the performance note
  rather than adding a bench.

## T105 - Multi-Stream Page Content Inventory

- `build_page_inventory` now inventories pages with multiple located content
  streams when every stream is supported and decodable, so both
  `build_pdf_inventory` and `build_classic_pdf_inventory` get the behavior
  through the shared helper.
- Added the private `page_content` helper: a single raw stream still returns a
  borrowed source slice, while Flate streams allocate only their bounded decoded
  output and multi-stream pages allocate one bounded joined page-content buffer.
- Multi-stream joins insert an explicit whitespace byte between decoded streams
  before tokenization, and the remaining decode budget is enforced across the
  whole joined page content, including separators.
- Unsupported filters, target/extent failures, decode failures, tokenizer,
  assembler, and graphics-walk failures continue to surface as deterministic
  structured page skips. `MultipleContentStreams` remains in the public skip
  enums for serde compatibility, but decodable multi-stream pages no longer emit
  it.

## T104 - Classic Incremental-Update Inventory End-to-End

- `build_pdf_inventory` now inventories classic incrementally-updated PDFs
  end-to-end. The only change in this crate is one mechanical dispatch arm: the
  `match &access.backend` site maps the new
  `DocumentAccessBackend::ClassicXrefChain { chain }` to
  `ObjectLookup::ClassicXrefChain(chain)`, so a classic trailer carrying `/Prev`
  now navigates and inventories through the same neutral spine as the classic
  single-table, single-section xref-stream, and xref-stream `/Prev`-chain
  backends.
- A classic two-section fixture whose newest section redefines the page
  `/Contents` object is inventoried to the updated content stream, proving the
  newest-wins classic chain resolves the page content through the bridge.
- Copy budget is unchanged: raw streams stay borrowed, Flate streams allocate
  only the bounded decoded buffer, reports retain no PDF source or stream bytes,
  and no per-page object map/cache is built over `ObjectLookup`.
- Next queue: `#26` document-level inventory merge, then the Y2 design note (a
  third mixed-chain abstraction unifying the parallel classic and xref-stream
  `/Prev` chain builders as feeders).

## T095 - Classic PDF Inventory Bridge

- Added `build_classic_pdf_inventory`, the umbrella-crate bridge from borrowed
  classic-xref PDF bytes to combined page-object `Inventory`.
- Scope is deliberately narrow: a page is inventoried only when it has exactly
  one located content stream and that stream is raw or a single `/FlateDecode`
  with resolved non-array `/DecodeParms`.
- Unsupported page and stream shapes are reported as structured skips, including
  target/extent locate failures, unsupported filters, unsupported
  `/DecodeParms`, decode failures, tokenizer/assembler failures, and
  graphics-walk failures.
- Copy budget: raw streams remain borrowed slices; Flate streams allocate only
  the bounded decoded buffer returned by the existing decoder. The bridge does
  not concatenate multiple streams or retain source bytes in report records.

## T102 - Neutral PDF Inventory Bridge

- Added `build_pdf_inventory`, the umbrella-crate bridge from borrowed PDF bytes
  to combined page-object `Inventory` over either a classic xref table or one
  `/Type /XRef` stream section.
- The bridge calls `inspect_document_access`, selects `ObjectLookup` from the
  returned `DocumentAccessBackend`, and locates page content extents through
  `inspect_document_page_content_extents_with_lookup`.
- Shared the backend-independent page decode/tokenize/assemble/build path as a
  private helper used by both the classic and neutral bridges. The public
  `Classic*` report types and serde shapes are unchanged.
- Top-level neutral document-access failures are wrapped as structured
  `PdfInventoryRejection::DocumentAccess` errors, including the delegated
  `PrevPresentUnsupported` stop for xref-stream `/Prev`.
- Preserved the page-skip taxonomy for content failures, multi-stream pages,
  unresolved or compressed targets, unsupported filters, unsupported
  `/DecodeParms`, decode failures, tokenizer/assembler failures, and
  graphics-walk failures.
- Copy budget is unchanged from the classic bridge: raw streams stay borrowed,
  Flate streams allocate only the bounded decoded buffer, reports retain no PDF
  source or stream bytes, and no per-page object map/cache is built over
  `ObjectLookup`.
- Next queue after X: #28 TAIL (`/Prev` chaining plus multi-section merge),
  then #26, then F3 (#29, design-notes only).
