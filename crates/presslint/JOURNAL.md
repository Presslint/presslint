# presslint Journal

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
