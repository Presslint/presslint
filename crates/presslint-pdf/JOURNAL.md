# presslint-pdf Journal

Older accumulated journal history lives in [JOURNAL-archive.md](JOURNAL-archive.md).

## Current State

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

## Follow-Ups

- Next C slice: follow `/Prev` to chain `decode_xref_stream_section` over
  incremental sections and merge them into a whole-document object map while
  preserving the `ResolvedObject` API now shared by classic and single-section
  xref-stream document access.
