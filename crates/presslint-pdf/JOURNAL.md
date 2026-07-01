# presslint-pdf Journal

Older accumulated journal history lives in [JOURNAL-archive.md](JOURNAL-archive.md).

## Current State

### T114 - Resolved Object-Aware Document Access Spine

- Added `ResolvedObjectPosition` with `Uncompressed { object_byte_offset,
  xref_generation }` and `Compressed { object_stream_number,
  index_within_object_stream }`. The neutral spine reports catalog and
  page-tree-root objects through `ResolvedStructuralObject`, keeping `reference`
  separate while preserving legacy uncompressed offset fields.
- Added `inspect_catalog_pages_resolved`, `inspect_page_tree_node_resolved`,
  `inspect_page_tree_node_type_resolved`, `inspect_page_tree_reference_target_resolved`,
  `inspect_page_tree_kid_targets_resolved`, and
  `inspect_page_tree_leaves_resolved`. Uncompressed branches delegate to the
  existing offset inspectors; compressed branches inspect member-body-relative
  entries through `inspect_object_dictionary`.
- Rewired the neutral spine to resolve `/Root`, catalog `/Pages`, page-tree
  root, and child page-tree references through bounded `resolve_object`.
  Compressed-but-unresolvable root catalog or `/Pages` objects remain fatal
  structured spine errors; compressed child failures are non-fatal page-tree
  skips, so later siblings still enumerate.
- Page order, DFS traversal, leaf ordering, depth/visited guards, and cycle
  identity by object number are preserved. No `ObjectLookup` or
  `DocumentAccessBackend` variant or umbrella bridge change was added.
- Copy budget: the only owned structural byte buffer remains the bounded decoded
  object-stream body from `resolve_object`; no cache or whole-document object
  map was added. No Criterion target was added because this dispatches over
  existing bounded resolution.
- Tests cover compressed catalog/`/Pages`/leaf navigation, mixed uncompressed
  root with compressed leaves, fatal compressed-root extraction, non-fatal
  invalid compressed child skip, serde tags, and existing classic/xref-stream
  parity.
- Retry note: `inspect_page_tree_leaves_with_lookup` remains an offset-only
  legacy/content-extents path. Only `inspect_page_tree_leaves_resolved` expands
  compressed children through `resolve_object` with the bounded decode limit, so
  compressed leaf pages are not surfaced to `/Contents` inspection with a
  synthetic offset `0`.
- Retry ablation: body-aware `/Kids` scanning now reuses the single scanner in
  `page_tree_kids.rs`, and the catalog/page-tree dictionary helpers borrow
  their entry spans from the owned report instead of cloning the entries vector.
- Deferred: compressed page `/Contents` extents and inherited
  `/Resources`/`/XObject` discovery remain later. The queue continues with
  CHK2, F3 design notes, HYBRID `/XRefStm`, and SA/IMASK polish.

### Resolve Compressed Objects From Object Streams

- Added the `object_stream_objects` module with
  `extract_object_stream_member(input, object_stream_byte_offset,
  requested_object_number, index, max_decoded_object_stream_bytes) ->
  ExtractedObjectStreamMember`: the first OBJSTM slice. It resolves nothing
  itself; the caller supplies the already-resolved object-stream object offset.
  It validates the containing stream dictionary as exactly `/Type /ObjStm`,
  requires exactly one direct non-negative integer `/N` and `/First` (both
  fitting `usize`, with `/First <= decoded.len()`), decodes the body through the
  existing stream-extent / filter-classification / `/DecodeParms` / bounded
  `decode_flate_stream` helpers (unfiltered or a single `/FlateDecode` only),
  parses `decoded[..First]` as exactly `N` `(object number, offset)` integer
  pairs, requires offsets in range and strictly increasing, selects member
  `index` (requiring `index < N` and the pair's object number to equal the
  request), computes the body span `First + offset_i .. First + offset_{i+1}`
  (or `decoded.len()` for the last member), and rejects a member body that
  begins with an indirect-object header (compressed members are bare bodies).
  `/Extends` is recorded (`has_extends`) but never followed.
- Copy budget: the single intentional owned allocation is the bounded decoded
  `/ObjStm` buffer, necessary because compressed member bodies live only in
  decoded stream bytes, not in `input`. An unfiltered body is copied into an
  owned buffer bounded by `max_decoded_object_stream_bytes` (a
  `DecodedObjectStreamTooLarge` rejection over the limit) so the result never
  borrows source bytes. `ExtractedObjectStreamMember` owns only that buffer plus
  a `Range<usize>` span; no other source bytes are retained. No benchmark: one
  bounded resolution path with an explicit copy budget, not a traversal hot
  path, and no cache.
- Added the body-aware resolver `resolve_object(input, lookup, reference,
  max_decoded_object_stream_bytes) -> ResolvedObjectData` in `object_resolver`.
  `ResolvedObjectData::Uncompressed { resolved }` delegates to the unchanged
  `resolve_xref_object_offset`, so uncompressed and classic in-use objects stay
  byte-for-byte compatible with the offset-only path.
  `ResolvedObjectData::Compressed { reference, object_stream_number,
  index_within_object_stream, decoded_object_stream, object_body_span }` owns the
  bounded decoded buffer and the member body span. A compressed request requires
  generation `0`; the containing object stream is resolved through
  `resolve_xref_object_offset` and, if itself type-2 compressed, rejected as
  `ObjectStreamIsCompressed`; other container failures surface as
  `ObjectStreamObjectUnresolved`; member-body failures surface as
  `ObjectStreamMemberExtraction { extraction_reason }`.
- `resolve_xref_object_offset` is unchanged: it still reports type-2 compressed
  entries as `UnsupportedCompressedXrefStreamEntry` and remains the zero-copy
  uncompressed fast path. `resolve_object` is the opt-in superset.
- The new `ObjectResolutionRejection` variants carry only small `Copy` data so
  `ObjectResolutionRejection` stays `Copy` (it is embedded in
  `LookupIndirectLengthRejection`), and the error stays under the
  large-error-size gate: the extraction-stage `StreamExtent` reason is a marker
  (offset carried by the outer error, direct-`/Length` decode path only) and the
  `ObjectStreamMemberExtraction` variant carries only the extraction reason
  (object-stream offset carried by `object_byte_offset`).
- Added the companion `inspect_object_dictionary(input, resolved:
  &ResolvedObjectData) -> ResolvedObjectDictionaryInspection` in
  `object_dictionary`. Uncompressed data delegates to
  `inspect_indirect_object_dictionary`; compressed data scans the extracted
  member body (requiring a leading `<<`, since members carry no indirect header)
  and reports `CompressedObjectDictionaryInspection` with entry spans relative to
  the member body, not to `input`.
- Deferred to the next OBJSTM wiring slice: threading `ResolvedObjectData`
  through catalog / page-tree / document-access navigation, adding an
  `ObjectLookup` / `DocumentAccessBackend` variant, following `/Extends` chains,
  and object-stream caching / whole-document object maps.

### T112 - Image `XObject` Dictionary Metadata

- Added the `image_xobject` module with
  `inspect_image_xobject_metadata(input, entries) -> ImageXObjectMetadata`, a
  pure structural scan over the shallow entries of an already-resolved
  `/Subtype /Image` dictionary. It reads only the four dictionary-level entries
  `/Width`, `/Height`, `/BitsPerComponent`, and `/ColorSpace`.
- `ImageIntegerMetadata` (for the three scalar dimensions/depth) maps to
  `Value { value: u32 }`, or one of the explicit shapes `Missing`,
  `Duplicate { .. }`, `Unsupported { value_kind }` (present but not a
  number-shaped scalar), or `Malformed` (number-shaped but not a non-negative
  32-bit integer: a real, a signed value, or an out-of-range magnitude).
- `ImageColorSpaceMetadata` maps the three direct device names to
  `DeviceGray` / `DeviceRgb` / `DeviceCmyk`. Every other shape stays explicit
  and is never guessed: `Missing`, `Duplicate { .. }`, `OtherName { name }` (a
  direct name other than the three devices, raw bytes including the leading
  slash), or `Unsupported { value_kind }` for any non-name value (an array such
  as `[/ICCBased ...]` / `[/Indexed ...]`, an indirect reference, a dictionary).
  Indirect and array colour spaces are deliberately not resolved in this slice.
- `PageXObjectResourceTarget` gained an `image_metadata:
  Option<ImageXObjectMetadata>` field: `Some(..)` for `/Subtype /Image`
  targets, `None` for `/Subtype /Form` targets. The metadata is computed inside
  the existing `classify_xobject_entry` path in both `page_xobject_resources`
  and `form_xobject_resources`, reusing the `target.entries` already inspected
  during subtype classification — no extra object read or resolution. Existing
  image/form resource-name classification and target ordering are unchanged.
- Image targets stay classified even when metadata is incomplete or
  unsupported: every unsupported shape is reported in-band rather than dropping
  the target.
- Copy budget: the metadata copies only small scalar values (`u32`,
  `DictionaryEntryByteRange`, `DictionaryValueKind`) plus, for a non-device
  colour space, the raw `/ColorSpace` name bytes already at the report
  boundary. It retains no PDF source bytes, object bodies, stream bodies,
  resource dictionaries, decoded image data, or ICC/profile bytes; a
  no-source-leak test asserts a `/Secret (..)` literal never reaches the debug
  report. No benchmark target: one bounded shallow scan per already-classified
  image target, no image-sample decode or buffer allocation.

### T110 - Single-Object Form `XObject` Resource Inspector

- Added `form_xobject_resources` module with
  `inspect_form_xobject_resources(input, lookup, object_byte_offset) ->
  FormXObjectResourcesInspection`: the single-object counterpart to the
  page-tree `inspect_document_page_xobject_resources`. It classifies exactly
  one Form `XObject`'s own `/Resources /XObject` dictionary into sorted,
  deduplicated `/Image` and `/Form` `PageXObjectResourceTarget`/`PdfName`
  vectors plus structured `SkippedPageXObjectResource` diagnostics.
- No page-resource inheritance: the form is scanned with
  `ResourceContext::from_dictionary(.., None)`, so a form paints against its
  OWN `/Resources` only. A missing form resource surfaces as `MissingResources`
  rather than borrowing the invoking page's resources. This is required because
  the T109 page-scope pass classifies page resources, not a form's own ones.
- Reuses the shared classification helpers from `page_resource_inheritance.rs`
  (`ResourceContext`, `unique_entry`, `resolve_reference`) and the
  `PageXObjectResourceTarget`/`PageXObjectResourceSubtype`/
  `SkippedPageXObjectResource` vocabulary. The subtype dispatch mirrors the
  page inspector but is kept in this sibling module so the 771-line
  `page_xobject_resources.rs` is not grown.
- Copy budget: the report stores only structural metadata, small owned name
  bytes, and byte-range skip records; it retains no PDF bytes, object bodies,
  resource dictionaries, stream bodies, or decoded data. A skip-diagnostic test
  asserts no source stream bytes leak into the debug report.
- This is a supporting inspector for the umbrella-crate one-level form-content
  inventory bridge (T110 `presslint` side); it opens nothing, resolves the
  form's `/XObject` targets one level via the existing lookup machinery, and
  mutates no bytes.

### T109 - Page `XObject` Resource Target Metadata

- `PageXObjectResourcesInspection` now exposes deterministic
  `image_xobjects` and `form_xobjects` vectors in addition to the existing
  `image_xobject_names` and `form_xobject_names` inventory bridge fields.
  Each `PageXObjectResourceTarget` carries the raw resource name, the indirect
  reference stored in the page-scope `/XObject` entry, and the resolved target
  object byte offset.
- Target vectors use the same sorted raw resource-name order as the
  legacy name vectors, preserving sorted/deduplicated behavior and disjoint
  Image/Form classification based on the resolved target `/Subtype`.
- Duplicate raw names remain first occurrence wins. Later repeated names are
  still emitted as structured `DuplicateXObjectName` skips before any
  conflicting classification can add the duplicate target.
- Copy budget: the additive report records own only small structural metadata
  already derived during classification (`PdfName`, `IndirectRef`, and `usize`
  offsets). The inspector still retains no source bytes, object bodies,
  dictionaries, stream bodies, decoded streams, or per-page lookup caches.
- Deferred: this does not recurse into Form XObject content streams. It only
  exposes the resolved page-scope targets needed by a follow-up recursion slice.

### T107 - Page `XObject` Resource Classification

- Added `inspect_document_page_xobject_resources_with_lookup(input, lookup,
  root_node_object_offset)` plus the classic wrapper
  `inspect_document_page_xobject_resources(input, xref, root_node_object_offset)`.
  The walk is backend-neutral over the existing `ObjectLookup` variants and
  keeps classic behavior as a thin wrapper through `ObjectLookup::ClassicXref`.
- The inspector walks the page tree root-down in document order, carrying the
  effective inheritable `/Resources` dictionary. A child `/Pages` or `/Page`
  `/Resources` entry replaces the inherited dictionary for this slice; an absent
  child entry keeps the inherited dictionary.
- It classifies only direct `/XObject` resource dictionary entries whose values
  are indirect references resolving to dictionary-bodied objects with
  `/Subtype /Image` or `/Subtype /Form`. Per-page image/form name vectors are
  sorted and deduplicated for deterministic inventory input.
- Malformed, missing, unknown, non-reference, unresolved, generation-mismatched,
  duplicate-key, and non-dictionary resource shapes are reported as structured
  per-page `SkippedPageXObjectResource` diagnostics. Page-tree child failures
  remain ordered traversal skips rather than panics.
- Duplicate raw names inside a direct `/XObject` dictionary now emit
  `DuplicateXObjectName`; the first occurrence is the only one classified, so a
  conflicting repeated resource name cannot appear in both image and form name
  lists.
- Resource inheritance, replacement, reference resolution, and shared
  unique-entry helpers live in `page_resource_inheritance.rs`, keeping the
  public inspector module below the 800-line review gate while preserving the
  same public API re-exports.
- Copy budget: the source bytes, dictionaries, object bodies, stream bodies, and
  decoded data stay unretained. Owned output is limited to raw resource-name
  byte vectors, small skip records, copied dictionary-entry byte ranges, and
  delegated structural metadata. The traversal reuses the caller-provided
  `ObjectLookup` and builds no document object cache.
- Deferred: no recursion into Form XObject content streams, no image pixel or
  stream-parameter inspection, no object-stream/type-2 resolution beyond the
  existing lookup behavior, and no support for indirect `/XObject`
  subdictionaries in this slice.

### T104 - Classic-Table `/Prev` Chain Object Map

- Added `build_classic_xref_chain(input, startxref_byte_offset)`, the classic
  parallel of the T103 `XrefStreamChain`. It classifies and inspects one classic
  cross-reference table per section with the existing
  `inspect_classic_xref_table`, follows the classic trailer `/Prev` byte offset
  newest-to-oldest, and materializes one deterministic newest-wins
  `Vec<ClassicXrefEntry>` sorted ascending by object number.
- Contract / merge rule: the `startxref` section is newest. Merge precedence is
  **newest-wins including free-entry shadowing** — a newer entry (in-use or free)
  shadows any older entry for the same object number, and earlier sections only
  fill unseen numbers, implemented as a `BTreeMap<u32, ClassicXrefEntry>` with
  `or_insert` so the first insertion (newest) wins. Free-list fields are
  preserved exactly as parsed (a free entry's `byte_offset` is the next-free
  object number; object 0 is the head at generation 65535); the chain reports
  parsed structure only and validates no free-list integrity.
- Intra-section-duplicate choice: unlike the single-table lookup, which flags
  duplicate object numbers inside one table as ambiguous, the chain treats
  cross-section duplicates as expected (newest wins) and keeps the **first entry
  in source order** for an intra-section duplicate (first-in-section), because
  the newest section is processed first and `or_insert` keeps the first
  insertion. This is documented on `ClassicXrefChain`.
- Companion micro-inspector: `inspect_classic_xref_trailer_prev` is bundled as
  the chain's locator (too small to ship alone). It is a focused sibling of
  `inspect_classic_xref_trailer_root`, reusing the same trailer-dictionary and
  `inspect_dictionary_entries` scan plus the shared exact-key/duplicate-key
  `unique_entry` and `parse_non_negative_integer` helpers already used for the
  xref-stream trailer `/Prev`. It reports absent `/Prev` as `Ok(None)`, one
  direct non-negative integer as `Ok(Some(..))`, and duplicate/non-integer/
  overflow `/Prev` as distinct structured rejections.
- `/Root` is read from the newest section only. `effective_size` tracks the max
  direct `/Size` observed across section trailers; because classic object
  location uses byte offsets rather than `/Size`, a section trailer without a
  readable direct `/Size` simply does not contribute and does not gate the chain
  (best-effort, deliberately looser than the xref-stream chain, whose `/Size` is
  required geometry).
- `PrevSectionNotClassicXref` stop: a classic `/Prev` target that classifies as
  a cross-reference stream is a structured stop (mixed classic/xref-stream chains
  are deferred to Y2), never a silent drop and never a panic. Every other failure
  path (out-of-bounds offset, cycle, section/entry bounds, classification, table,
  trailer `/Root`, trailer `/Prev`) is a distinct structured rejection and no
  partial chain is returned.
- Bounds mirror T103: a visited-offset `BTreeSet` for cycles,
  `MAX_CLASSIC_XREF_CHAIN_SECTIONS = 64`, and
  `MAX_CLASSIC_XREF_CHAIN_ENTRIES = 1_000_000`, so a malformed `/Prev` graph
  cannot cause unbounded work or allocation.
- Wired as a new backend: `ObjectLookup::ClassicXrefChain`,
  `DocumentAccessBackend::ClassicXrefChain`, and the
  `DocumentAccessRejection::ClassicXrefChain` / `TrailerPrev` stops.
  `inspect_document_access` selects the classic-chain backend when a classic
  trailer carries `/Prev` (an absent `/Prev` keeps the single-table
  `ClassicXref` backend). The `content_stream_extent.rs` exhaustive match and the
  umbrella `pdf_inventory.rs` `DocumentAccessBackend` dispatch learned the new
  variant; the chain resolves indirect `/Length` through the shared
  `resolve_xref_object_offset` path like the other non-classic-table backends.
- Copy budget: `input` stays borrowed. The only owned output is the bounded
  `Vec<ClassicXrefEntry>` (small `Copy` records), the bounded section-offset
  vector, and a `BTreeMap` used only during the merge. No PDF source bytes,
  trailer bytes, object bodies, or stream bodies are retained or copied. Object
  location binary-searches the already-sorted entry vector and builds no per-call
  map or cache.
- No new benchmark target: like T103, a same-type chain builder over bounded
  report materialization does not warrant a Criterion target; the work is bounded
  by the visited set and the two `MAX_*` constants, not a measured tight loop.
- Y2 deferral: the classic and xref-stream chains stay **parallel** same-type
  builders (classic and xref-stream entries have different currencies). A future
  Y2 unifies them via a third mixed-chain abstraction with these two builders as
  feeders; this task does not attempt a generic merged map, hybrid `/XRefStm`,
  object-stream/type-2 resolution, or byte mutation.

### T103 - Xref-Stream `/Prev` Chain Object Map

- Added a bounded same-type xref-stream `/Prev` chain builder that decodes each
  section with the existing single-section decoder, follows newest-to-oldest
  offsets, and materializes one deterministic newest-wins merged
  `XrefStreamEntry` map sorted by object number.
- Merge precedence is newest-wins for every entry type: an object redefined in
  the newest section resolves to the newest offset, and a newer type-0 free
  entry shadows an older type-1 in-use entry.
- The chain report reads `/Root` from the newest section only, tracks effective
  `/Size` as the maximum across sections, retains no source bytes, and owns only
  bounded section offsets plus the merged entry vector.
- Added structured chain stops for out-of-bounds offsets, repeated offsets
  (cycles), section-count bound, merged-entry bound, unclassified `/Prev`
  targets, mixed classic-table `/Prev` targets, and delegated section decode
  failures.
- Added `ObjectLookup::XrefStreamChain` and
  `DocumentAccessBackend::XrefStreamChain`. Single-section xref-stream PDFs
  with no `/Prev` still use the existing `XrefStreamSection` backend and serde
  shape; only a present `/Prev` selects the chain backend.
- The neutral document-access spine and the page-content extent path now resolve
  through the merged chain. The umbrella inventory bridge gets the benefit
  transitively by deriving the new lookup variant from the new backend; no new
  umbrella report type, opener, cache, or CLI behavior was added.
- Deferred as planned: classic-table `/Prev` chains need the companion classic
  trailer inspector slice, and mixed classic/xref chains plus `/XRefStm` hybrid
  references remain Y2 work.

### T088 - Bounded FlateDecode Stream Decoding

- Added a focused `/FlateDecode` stream helper that accepts borrowed compressed
  bytes plus explicit decode parameters and returns bounded owned decoded
  bytes for downstream tokenizer/inventory consumers.
- The helper uses pinned `miniz_oxide =0.9.1` with default features disabled
  and only the allocation feature enabled. Inflate uses the zlib-wrapped
  bounded API, so over-limit output becomes a structured rejection instead of
  unbounded allocation.
- Supported predictor cases are no predictor / `/Predictor 1`, TIFF Predictor
  2, and PNG predictors 10-15. Predictor failures are explicit for unsupported
  predictors, malformed parameters, row geometry mismatches, integer overflow,
  and unknown PNG filter bytes.
- The owned decoded buffer is intentional at this seam: decompression creates a
  new byte stream. Predictor reversal avoids an additional decoded copy; PNG
  rows are compacted in place after filter reversal.
- Non-goals remain unchanged: no xref streams, no filter arrays or chained
  filters, no additional PDF filters, no recompression or mutation, no object
  maps/caches/document opener, and no inventory/action/color work.

### T089 - Inspect Cross-Reference Stream Dictionary Geometry Fields

- Adds `inspect_xref_stream_dictionary(input, object_byte_offset)`, the first
  cross-reference-stream (`/Type /XRef`) slice. Given caller bytes and the byte
  offset of an indirect object (the offset `classify_xref_section` reports as
  `XrefSection::Stream`), it extracts the geometry fields a later step needs to
  slice the eventually-decoded entry table: `/Type` (must be `/XRef`), `/W` (the
  three field widths), `/Size`, and `/Index` (the subsection pairs).
- It delegates the object header and top-level entry spans to
  `inspect_indirect_object_dictionary`, reimplementing no header, body-token,
  dictionary-open, or entry-span scanning, and matches only the exact raw key
  bytes `/Type`, `/W`, `/Size`, and `/Index` the same way
  `inspect_classic_xref_trailer_root` matches `/Root` (one shared
  `unique_entry` helper reports missing as `Ok(None)`, exactly-one as
  `Ok(Some)`, and more-than-one as a duplicate-key error).
- `/Type` must be exactly one name value whose raw bytes are `/XRef`; a missing,
  duplicate, non-name, or non-`/XRef` `/Type` is a distinct rejection
  (`MissingType`, `DuplicateType`, `NonNameTypeValue`, `UnexpectedTypeName`).
- `/W` must be exactly one array value, located with `inspect_array_extent` and
  scanned by the one new bounded abstraction: a whitespace/comment-separated
  decimal-integer element scan over the located array extent. Exactly three
  non-negative integers are required and a width of `0` (omitted field) is
  accepted; missing, duplicate, non-array, malformed-array, malformed-element,
  width-overflow, and wrong-length cases are distinct rejections (`MissingW`,
  `DuplicateW`, `NonArrayWValue`, `MalformedWArray`, `MalformedWElement`,
  `WidthOutOfRange`, `WrongWLength`).
- `/Size` must be exactly one direct non-negative integer that fits `usize`;
  missing, duplicate, non-integer (any non-pure-digit value span, including an
  indirect `N G R` or a decimal), and out-of-range cases are distinct
  rejections (`MissingSize`, `DuplicateSize`, `NonIntegerSizeValue`,
  `SizeOutOfRange`).
- `/Index` is optional: when absent it defaults to a single `(0, Size)`
  subsection with `index_value_range` `None`; when present it must be one array
  of an even count of non-negative integers parsed as
  `(first_object_number, entry_count)` pairs, and a duplicate, non-array,
  malformed-array, malformed-element, odd-length, or integer-overflow `/Index`
  is a distinct rejection (`DuplicateIndex`, `NonArrayIndexValue`,
  `MalformedIndexArray`, `MalformedIndexElement`, `OddIndexLength`,
  `IndexIntegerOutOfRange`). Geometry is never fabricated when the key is
  present but malformed.
- `XrefStreamDictionaryInspection` carries the delegated
  `IndirectObjectDictionaryInspection`, the `/Type` key/value byte ranges, the
  `/W` value byte range and parsed `widths`, the `/Size` value byte range and
  parsed `size`, and the `/Index` value byte range (when present) plus the
  ordered `index_subsections`. It retains or copies no PDF bytes, object bodies,
  stream bodies, decoded bytes, or source slices; the only owned allocations are
  the three-element `widths` vector and the small `index_subsections` pair
  vector (the acceptable copy budget for bounded report materialization), so no
  benchmark was added.
- Every failure path is a distinct structured rejection variant and the helper
  never returns partial geometry on error. It lives in the new focused
  `xref_stream.rs` module, re-exported from `lib.rs`; tests live in
  `src/tests/xref_stream.rs`.
- A composition test chains `inspect_startxref -> classify_xref_section
  (== XrefSection::Stream) -> inspect_xref_stream_dictionary` over a synthetic
  xref-stream fixture and confirms `/Type /XRef`, the three `/W` widths,
  `/Size`, and the defaulted/explicit `/Index` subsections; a serde round-trip
  test pins the public JSON shape of the report and rejection enum.
- Non-goals for this slice: no decoding/inflating/reading of the cross-reference
  stream body bytes, no `/W`-width entry-record parsing or object offset map, no
  `/Root` or `/Prev` parsing, no `/Prev` following, incremental-section merging,
  or hybrid-reference (`/XRefStm`) support, no indirect-reference resolution,
  catalog/page-tree/`/Contents` reading, no stream-data extent location or
  `endstream` validation, and no filesystem I/O, document opener, caches, object
  maps, or whole-document eager parsing.
- Ablation (behavior-preserving): the four field-requirement helpers
  (`require_type`/`require_widths`/`require_size`/`require_index`) no longer take
  a generic `E: Fn(..) -> Error + Copy` error closure with the same four-line
  bound repeated verbatim; they take a small `Copy` `ErrorContext` struct whose
  `error(reason, offset)` method builds the rejection. This removes the generic
  parameter from every helper, the closure built in
  `inspect_xref_stream_dictionary`, the free `xref_stream_error` constructor, and
  the four single-use `duplicate_*` variant wrappers (their duplicate-key ranges
  are now destructured inline at each call site). No public type, field, serde
  shape, rejection variant, error offset, or behavior changed; all `xref_stream`
  tests, the full `presslint-pdf` suite, `cargo check --workspace --all-targets`,
  clippy, and `./scripts/ci_check.sh` pass unchanged.

### T090 - Inspect Cross-Reference Stream Trailer Navigation Fields

- Adds `inspect_xref_stream_trailer(input, object_byte_offset)`, the next
  cross-reference-stream slice. Given caller bytes and the byte offset of an
  xref-stream indirect object (the offset `classify_xref_section` reports as
  `XrefSection::Stream`), it reads the trailer-style navigation fields that let
  the structural path continue from an xref stream: `/Root` (the catalog) and
  optional `/Prev` (the previous cross-reference byte offset).
- It delegates `/Type /XRef`, `/W`, `/Size`, and `/Index` geometry validation to
  `inspect_xref_stream_dictionary`, reimplementing none of it, and scans only the
  exact raw top-level keys `/Root` and `/Prev` over the entries that geometry
  inspection already materialized (via `inspect_indirect_object_dictionary`). The
  exact-key, missing/duplicate semantics reuse the shared `unique_entry` helper
  from `xref_stream`, and the `/Prev` byte offset reuses the same
  `parse_non_negative_integer` helper that module applies to `/Size` (both `pub`,
  crate-internal because the module is private).
- `/Root` is required and must be exactly one top-level `N G R` indirect
  reference that covers its entire value span, parsed with
  `parse_indirect_reference`; it is reported as an `IndirectRef` plus its key and
  value byte ranges. Missing, duplicate, non-reference (name/number/dict/array),
  and malformed-reference (`obj` keyword or trailing scalar) cases are distinct
  rejections (`MissingRoot`, `DuplicateRoot`, `NonReferenceRootValue`,
  `MalformedRootReference`).
- `/Prev` is optional and must be at most one top-level direct non-negative
  decimal-integer byte offset that fits `usize`, reported with its value byte
  range and parsed `prev_byte_offset` when present. Duplicate, non-integer
  (indirect reference, decimal, signed, name, array, dictionary), and
  integer-overflow cases are distinct rejections (`DuplicatePrev`,
  `NonIntegerPrevValue`, `PrevOutOfRange`); when `/Prev` is absent both report
  fields are `None`.
- `XrefStreamTrailerInspection` carries the delegated
  `XrefStreamDictionaryInspection`, the `/Root` key/value byte ranges and parsed
  `root_reference`, and the optional `/Prev` value byte range and parsed
  `prev_byte_offset`. It retains or copies no PDF bytes, object bodies, stream
  bodies, or source slices; its only owned data are the delegated inspection's
  bounded vectors, byte ranges, an `IndirectRef`, and small `usize` values, so no
  benchmark was added. Every failure path is a distinct structured rejection and
  the helper never returns partial navigation fields on error.
- It lives in the new focused `xref_stream_trailer.rs` module, re-exported from
  `lib.rs`; tests live in `src/tests/xref_stream_trailer.rs` and cover `/Root`
  success, `/Root` + `/Prev` success, missing/duplicate/non-reference/malformed
  `/Root`, duplicate/non-integer/overflow `/Prev`, delegated-geometry-failure
  propagation, a `inspect_startxref -> classify_xref_section
  (== XrefSection::Stream) -> inspect_xref_stream_trailer` composition case, a
  no-retained-bytes check, and a serde round-trip pinning the report and
  rejection JSON shape.
- Non-goals for this slice: no cross-reference stream body decoding or
  entry-record parsing, no `/W`-width record slicing or object-offset map, no
  `/Prev` following, incremental-section merging, or hybrid-reference
  (`/XRefStm`) support, no `/Root` resolution or catalog/page-tree traversal, and
  no filesystem I/O, document opener, caches, or whole-document eager parsing.

### T091 - Decode Cross-Reference Stream Entry Records

- Adds `parse_xref_stream_entries(decoded, widths, subsections)`, a pure helper
  that consumes caller-supplied already-decoded xref-stream body bytes plus the
  validated `/W` widths and ordered `/Index` subsections, then slices the body
  into fixed-width records and returns typed entries with derived object
  numbers.
- Each record is decoded as three explicit big-endian unsigned fields whose
  byte widths come from `/W`. Missing fields use the PDF xref-stream defaults:
  `W[0] == 0` makes the entry type default to `1`, while omitted fields 2 and 3
  default to `0`.
- Known entry types are reported as `Free`, `Uncompressed`, or `Compressed`.
  Unknown or future entry type values are surfaced as `Reserved { entry_type,
  field2, field3 }` with the raw decoded fields, so they are never silently
  fabricated into byte offsets or object-stream references.
- The decoder is all-or-nothing. Zero record width, a field width wider than the
  fixed integer accumulator, total-entry or decoded-length arithmetic overflow,
  decoded-length mismatch, known-entry field values that do not fit `usize`, and
  derived object-number overflow each produce distinct structured rejections
  without returning partial entries.
- The copy budget is intentionally bounded: `decoded` stays borrowed and no PDF
  bytes are retained or copied. The only owned output is a
  `Vec<XrefStreamEntry>` of small `Copy` records bounded by the declared total
  entry count, matching the deterministic report-materialization budget used by
  the T089/T090 xref-stream slices.
- No benchmark was added for this isolated pure decoder. The realistic
  Criterion target is deferred to the end-to-end structural slice that locates
  the xref-stream data extent, runs the T088 FlateDecode helper as needed, feeds
  this decoder, and builds the object-offset map over large xref inputs.
- Non-goals for this slice: no stream-data extent location, `/Length`
  resolution, `stream`/`endstream` scanning, FlateDecode/predictor invocation,
  object-offset map construction, `/Root` resolution, `/Prev` chain following,
  incremental-section merging, hybrid-reference (`/XRefStm`) support,
  object-stream body reading, or compressed-object extraction.

### T092 - Classic-Xref Document Access Spine

- Adds a small, backend-neutral object-resolution model in the new
  `object_resolver.rs` module: `ResolvedObject` (the success currency) plus
  `resolve_classic_xref_object_offset(input, xref, reference)`. The classic
  backend resolves an `IndirectRef` to an in-use object byte offset only when all
  checks hold: `resolve_classic_xref_object` reports exactly one in-use entry
  (free, not-found, and ambiguous results are rejected as
  `UnresolvedXrefLocation`), the in-use entry generation matches the requested
  reference (`GenerationMismatch`), and the indirect object header at the
  resolved offset both parses (`ObjectHeader`) and matches the requested
  object/generation (`ObjectHeaderReferenceMismatch`). Generation is therefore
  validated twice: against the xref entry and against the object header.
- The model is deliberately neutral so a later cross-reference-stream backend can
  return the same `ResolvedObject`/`ObjectResolutionError` without changing
  consumers. It is report-only metadata, not a document cache, object map, or
  opener; it reads only the short header at the resolved offset and the
  already-parsed xref table.
- Adds the first composing document-access spine in the new
  `document_access.rs` module: `inspect_classic_document_access(input)` threads
  the existing low-level inspectors in document order — `inspect_startxref`,
  `classify_xref_section`, `inspect_classic_xref_table`,
  `inspect_classic_xref_trailer_root`, root-reference resolution via the new
  resolver, `inspect_catalog_pages`, pages-reference resolution, and
  `inspect_page_tree_leaves`.
- `ClassicDocumentAccess` reports the classic xref table, trailer `/Root`, the
  resolved catalog `ResolvedObject`, catalog `/Pages`, the resolved page-tree-root
  `ResolvedObject`, and the document-ordered `PageTreeLeavesInspection` (leaves
  plus non-fatal skips/truncation). It retains or copies no PDF bytes, object
  bodies, stream bodies, dictionaries, decoded streams, or source slices; owned
  data is limited to the metadata and bounded vectors the delegated inspectors
  already produced, so no benchmark was added (no new scan beyond the delegated
  inspectors).
- Every failure path is a deterministic, structured `ClassicDocumentAccessError`
  whose `ClassicDocumentAccessRejection` names the spine stage that stopped and
  preserves the delegated failure verbatim. A cross-reference-stream section is a
  structured `UnsupportedXrefStream { object_number, generation }` result, not a
  success, and no xref-stream object-map work is attempted. Per-kid leaf-walk
  problems (other-typed kids, per-kid resolution failures, bound-stopped descents)
  stay as structured skips inside `page_leaves`, not spine failures.
- Tests live in `src/tests/object_resolver.rs` (unique in-use success, two header
  checks, both generation checks, free/not-found/header-parse/header-mismatch
  rejections, no-retained-bytes, serde round-trip) and
  `src/tests/document_access.rs` (synthetic multi-page success, unsupported
  xref-stream classification, missing `startxref`, trailer `/Root` resolution
  failure, catalog `/Pages` resolution failure, xref generation mismatch, object
  header mismatch at the resolved offset, preserved leaf skips, no-retained-bytes,
  serde round-trip).
- Non-goals for this slice: no xref-stream object-map backend, no `/Prev`
  traversal or incremental-section merging, no object-stream extraction, no stream
  decoding, content tokenization, or inventory building, no whole-document object
  map or cache, no document opener, and no PDF byte mutation or writer work.

### T093 - Classify Content Stream Filter Chains

- Adds `classify_content_stream_filter(input, object_offset)`, a public
  decode-path classifier for dictionary-bodied content stream objects. It
  delegates object/dictionary/`stream` validation to
  `inspect_content_stream_start`, then inspects only the delegated top-level
  dictionary entries for the exact raw `/Filter` key.
- The success classification is intentionally small: `Uncompressed` for a
  missing `/Filter` or empty filter array, `Flate` for exactly one
  `/FlateDecode` filter, `UnsupportedFilter { filter_name_range }` for one
  non-Flate name, and `UnsupportedFilterChain { filter_value_range,
  filter_count }` for arrays with two or more name filters. Unsupported filters
  are structured skip results, not errors.
- Malformed structure is reported through
  `ContentStreamFilterClassificationError` with distinct rejection variants for
  delegated stream-start failure, duplicate `/Filter`, non-name/non-array filter
  values, malformed filter arrays, and non-name array elements. A malformed
  top-level `/Filter [` discovered during delegated dictionary entry inspection
  is remapped to the classifier's `MalformedFilterArray` rejection while
  unrelated stream-start failures remain delegated `StreamStart` failures.
  Indirect reference-like `/Filter` values remain a non-name/non-array
  rejection; this slice does not resolve them.
- The helper retains or copies no PDF bytes, object bodies, stream bodies,
  decoded bytes, filter names, or source slices. Reports carry only byte ranges,
  small counts, and enums; `/FlateDecode` is matched by comparing the caller's
  source range in place, preserving the zero-copy dispatch lesson from the
  earlier filter/decode work.
- Tests cover the no-filter identity path, single name filters, single-element
  arrays, empty arrays, multi-filter chains, duplicate keys, malformed value
  kinds, non-name array elements, serde shape, and composition from
  `inspect_classic_document_access`/page-content resolution to a resolved Flate
  content stream.
- Non-goals for this slice: no `/DecodeParms` or `/DP` parsing, no stream-body
  decode/inflate/decompress work, no content tokenization or assembly, no
  additional supported filters beyond classifying them as unsupported skips, no
  indirect `/Filter` resolution, and no inventory, selector, action, patch
  writer, filesystem I/O, document opener, cache, or whole-document eager
  parsing work.

### T098 - Decode A Single Cross-Reference Stream Into An Object Entry Map

- Adds `decode_xref_stream_section(input, object_byte_offset,
  max_decoded_stream_bytes)`, the first composing slice of the
  cross-reference-stream backend. Given caller bytes and the byte offset of one
  `/Type /XRef` stream object (the offset `classify_xref_section` reports as
  `XrefSection::Stream`), it returns an `XrefStreamSection`: the object byte
  offset, the three `/W` widths, `/Size`, the ordered `/Index` subsections, the
  `/Root` `IndirectRef`, the optional `/Prev` byte offset, and the entries in
  ascending object-number order.
- The composed pipeline reimplements none of its parts. It threads
  `inspect_xref_stream_trailer` (which itself composes
  `inspect_xref_stream_dictionary`, so one call supplies the `/W`/`/Size`/
  `/Index` geometry plus `/Root` and optional `/Prev`),
  `inspect_content_stream_data_extent` + `content_stream_data_slice` for the body
  bytes, the `classify_content_stream_filter` -> `resolve_flate_decode_parameters`
  -> `decode_flate_stream` decode path mirroring the T095 classic PDF inventory
  bridge, and `parse_xref_stream_entries` for the records.
- The decode path accepts exactly two stream shapes, like the inventory bridge: a
  raw (uncompressed) body passes through borrowed, and a single `/FlateDecode`
  with resolved non-array `/DecodeParms` is decoded into the bounded buffer
  `decode_flate_stream` returns under the caller's `max_decoded_stream_bytes`.
  The extent is located with no classic xref table, so an indirect `/Length`
  surfaces as a structured `StreamExtent` rejection rather than a partial read.
- Single-section scope: `/Prev` and `/Root` are surfaced but never followed or
  resolved; incremental sections are not merged; `Compressed` entries are
  reported as the typed record `parse_xref_stream_entries` already produces and
  never extracted; hybrid-reference (`/XRefStm`), `/Filter` arrays, chained
  filters, and non-Flate filters are reported as the unsupported-filter
  rejection.
- Duplicate object numbers across `/Index` subsections resolve deterministically
  by **last subsection wins** (matching how a later xref section overrides an
  earlier one, PDF 32000 §7.5.8). The entries from `parse_xref_stream_entries`
  (already in `/Index` traversal order) are folded through a
  `BTreeMap<usize, XrefStreamEntryRecord>`, so a later subsection overwrites an
  earlier record and the ordered iteration yields ascending object-number order.
- Every failure mode is a distinct structured `XrefStreamSectionRejection` that
  carries the delegated error/classification and never returns partial entries:
  `DictionaryGeometry`, `TrailerNavigation`, `StreamExtent`, `Slice`,
  `FilterClassification`, `UnsupportedFilter`, `DecodeParms`,
  `UnsupportedDecodeParms`, `FlateDecode`, and `EntryParse`. The single trailer
  call's nested geometry failure is split losslessly into the distinct
  `DictionaryGeometry` rejection (the trailer error builds 1:1 from the dictionary
  error), so both stages keep separate, delegated-error-carrying variants.
- Copy budget: the decoded body buffer is the same justified copy the inventory
  bridge documents (decompression necessarily materializes a new byte stream,
  bounded by `max_decoded_stream_bytes`) and is dropped before the report is
  built; raw bodies stay borrowed and are handed to `parse_xref_stream_entries`
  without a copy. The report retains no PDF source bytes; its only owned
  allocations are the bounded `index_subsections` and `entries` vectors of small
  `Copy` records. This is not a per-object hot path (one decode per section), so
  no benchmark was added, matching the deferred T091 Criterion note.
- The helper and public types live in the focused new `xref_stream_map.rs`
  module, re-exported from `lib.rs`; tests live in `src/tests/xref_stream_map.rs`
  and cover: a `inspect_startxref -> classify_xref_section (== Stream) ->
  decode_xref_stream_section` chain over a FlateDecode + PNG Up-predictor fixture
  (built with a hand-rolled stored-block zlib helper so no deflate encoder
  dependency is added), a raw no-filter composition with `/Prev`, an overlapping
  `/Index` pinning the last-subsection-wins rule, each failure path (unsupported
  filter, array `/DecodeParms`, Flate decode failure, bad geometry, missing
  `/Root`, stream-extent, entry-parse length mismatch), a no-retained-bytes
  check, and serde round-trips pinning the report and every rejection variant.

### T099 - Add Unified Xref Object Lookup

- Adds the public borrowing `ObjectLookup<'a>` abstraction over
  `ClassicXrefTableInspection` and one decoded `XrefStreamSection`, plus
  `locate_xref_object` and the serde-pinned `ObjectLookupLocation` locate-only
  result. The locate shape distinguishes classic in-use/free/not-found/ambiguous
  entries from xref-stream uncompressed/free/compressed/reserved/not-found
  entries, and reports xref-stream object numbers or generations that cannot fit
  the existing `IndirectRef` widths without truncating them.
- The xref-stream lookup uses the sorted `XrefStreamSection.entries` vector
  directly with binary search. It builds no per-call map, copies no source bytes,
  and never fabricates byte offsets for type-2 compressed or reserved/future
  entry types.
- Adds `resolve_xref_object_offset(input, lookup, reference)` as the
  backend-neutral resolver. It returns the existing `ResolvedObject` success
  currency for classic in-use entries and xref-stream type-1 uncompressed entries
  after checking the xref generation and validating the indirect object header at
  the resolved offset.
- Keeps `resolve_classic_xref_object_offset` as a thin compatibility wrapper over
  `ObjectLookup::ClassicXref`, preserving the classic double validation:
  requested generation vs xref generation first, then requested reference vs the
  parsed object header reference.
- Xref-stream type-2 compressed entries reject with
  `UnsupportedCompressedXrefStreamEntry`, carrying the object-stream number and
  index inside that object stream. Reserved/future entry types reject with
  `UnsupportedReservedXrefStreamEntry`, carrying the raw decoded fields. Neither
  path is treated as not-found, and neither attempts object-stream extraction.
- Copy budget: lookup and resolution reports retain only structural metadata
  (`usize` offsets/fields, references, and small enums). They retain no PDF source
  bytes, decoded stream bytes, object bodies, dictionaries, or stream bodies.
- Deferred: this slice does not thread the new lookup through page-tree/document
  access, follow `/Prev`, merge incremental xref sections, support hybrid
  references, or extract object streams. That remains future spine wiring.

### T100 - Navigate Single-Section Xref Streams Through Document Access

- Threads the T099 `ObjectLookup<'_>` boundary through page-tree traversal with
  `_with_lookup` variants for `inspect_page_tree_reference_target`,
  `inspect_page_tree_kid_targets`, and `inspect_page_tree_leaves`. The generic
  path resolves references via `locate_xref_object`, accepts only classic in-use
  or xref-stream uncompressed entries, preserves the generation check and node
  type classification flow, and reports compressed/reserved xref-stream entries
  as structured unresolved skips rather than fabricated offsets.
- Keeps the classic helpers as compatibility wrappers over
  `ObjectLookup::ClassicXref`, preserving their report and error shapes. Classic
  unresolved locate results are mapped back to the existing
  `UnresolvedXrefLocation` rejection while xref-stream-only failures use the new
  backend-neutral `UnresolvedLookupLocation` variant.
- Adds the neutral `inspect_document_access(input)` spine. It selects the
  backend from `startxref` section classification: classic tables parse the
  matching classic xref/trailer, while `/Type /XRef` sections decode exactly one
  xref stream and read `/Root` from that section's trailer data. Both paths then
  resolve the catalog, catalog `/Pages`, page-tree root, and document-ordered
  leaves through `resolve_xref_object_offset` and the lookup-backed page-tree
  walk.
- Adds `DocumentAccess`, `DocumentAccessBackend`, `DocumentAccessError`, and
  `DocumentAccessRejection` as the backend-neutral report and rejection taxonomy.
  The report retains only structural metadata from delegated inspections and no
  PDF source bytes, object bodies, stream bodies, decoded stream buffers, or
  source slices.
- A decoded xref-stream section with `/Prev` now stops with
  `PrevPresentUnsupported { prev_byte_offset }`. The spine never follows the
  offset, decodes a previous section, merges incremental entries, or consults
  hybrid-reference `/XRefStm` data.
- Focused tests cover classic-delegation parity for the new lookup-backed
  helpers, xref-stream-backed page-tree leaf enumeration, neutral classic and
  `/FlateDecode` xref-stream document navigation, the `/Prev` stop,
  backend-selection and per-stage failures, compressed xref-stream entries as
  non-leaf skips, no-retained-bytes checks, and serde round-trips for the new
  neutral report and rejection shapes.
- Deferred: `/Prev` chaining, multi-section merging, `/XRefStm` hybrid-reference
  support, object-stream extraction, type-2 compressed-object resolution,
  document-level object maps/caches/openers, and filesystem I/O remain separate
  future work.
