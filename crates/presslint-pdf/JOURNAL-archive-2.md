# presslint-pdf Journal Archive 2

Newer active history lives in [JOURNAL.md](JOURNAL.md). Older accumulated
history lives in [JOURNAL-archive.md](JOURNAL-archive.md).

## Archived Entries

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

### T124 - Add Deterministic Flate Encode

- Adds `encode_flate_stream(input, input_limit)`, a pure byte transform that
  rejects inputs over the caller bound with `InputLimitExceeded`, then emits a
  zlib-wrapped `/FlateDecode` payload via
  `miniz_oxide::deflate::compress_to_vec_zlib`.
- Pins `FLATE_ENCODE_LEVEL` to `6`; no date, random, dictionary, predictor, or
  platform-dependent option is introduced. Output is one owned `Vec<u8>` and
  the borrowed input is not retained.
- Tests cover empty, small, large, high-entropy, already-compressed, and real
  decoded content-stream bodies, deterministic repeat encode, default-parameter
  decode round-trip, bounded rejection, and the structured error shape.
