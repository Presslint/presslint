# presslint Journal

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
