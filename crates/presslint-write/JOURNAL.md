# presslint-write Journal

## Current State

`presslint-write` is the first byte-writing crate: a deterministic classic-xref
**incremental append** writer. It offers the foundational semantic **no-op**
(`write_incremental_revision`) and the first *semantic* mutation built on it,
`set_page_boxes_incremental`.

## Semantic page-box writing (T119)

### Public API

- `set_page_boxes_incremental(input: &[u8], request: &SetPageBoxesRequest)
  -> Result<SetPageBoxesOutput, SetPageBoxesError>` — sets `/MediaBox` and/or
  `/CropBox` on selected uncompressed leaf pages and appends exactly one classic
  incremental revision through `write_incremental_revision`.
- `SetPageBoxesRequest { pages: Vec<PageBoxEdit> }`, where
  `PageBoxEdit { page_index, media_box: Option<PageRectangle>,
  crop_box: Option<PageRectangle> }`. Rectangles reuse
  `presslint_pdf::PageRectangle`; no second rectangle type is introduced.
- `SetPageBoxesOutput { bytes, edited: Vec<EditedPage>,
  skipped: Vec<SkippedPageEdit> }`. `EditedPage` carries the page index, leaf
  reference, and per-box `AppliedBox { kind, rectangle, op }` where
  `DictionaryEntryWrite` is `Replace` or `Insert`.
- `SetPageBoxSkipReason` (document conditions → structured skips) and
  `SetPageBoxesError` (request/geometry/inspection/write failures).

### Behavior proven

- **Verbatim prefix**: `output.bytes[..input.len()] == input`; only the edited
  leaf objects are appended (multi-page tests assert unselected leaves are never
  rewritten).
- **Replace vs insert**: a direct leaf entry is replaced from
  `key_range.start .. value_range.end` (provenance from
  `inspect_document_page_boxes`); an absent/inherited/defaulted entry is inserted
  immediately after the leaf dictionary `<<`. Inherited boxes become explicit
  leaf entries; ancestor `/Pages` dictionaries are never mutated.
- **Multiple edits per body** are applied in descending start-offset order;
  equal-start inserts are ordered so `/MediaBox` precedes `/CropBox`.
- **Minimal serialization**: `/MediaBox [0 0 612 792]`-style literals via `f64`
  `Display` (shortest round-trip, no exponent, no trailing zeros), with `0` for
  negative zero. Every unrelated byte inside the leaf dictionary body is
  preserved.
- **Normalization/validation**: requested rectangles are ordered lower-left /
  upper-right; non-finite, zero-area, and crop-outside-(effective/requested)-media
  requests are hard errors (no auto-intersect). Duplicate request page indexes
  and empty page edits are also errors.
- **Ownership**: only a leaf enumerated exactly once whose ownership decision is
  `InPlaceMutation` is rewritten. Ownership is decided with
  `decide_indirect_object_edit` over the leaf's single `/Parent`; a leaf reached
  by more than one page-tree slot (occurrence count > 1) or lacking a provable
  `/Parent` is an `OwnershipNotProven` skip. Compressed leaves, duplicate/
  malformed/indirect box entries, and missing pages map from the inspector's
  own skip taxonomy.
- **Reopen + idempotence**: output reopens through `inspect_document_access` and
  `inspect_document_page_boxes` reports the requested boxes on edited pages; a
  second identical append re-replaces the now-direct entry and keeps the first
  output as a verbatim prefix.
- **Report ordering**: `SetPageBoxesOutput::edited` is returned in ascending
  document page-index order regardless of the order pages appear in the request;
  a reversed multi-page request (`[1, 0]`) still reports `[0, 1]`. `skipped`
  stays in request order. Sorting `edited` is independent of the `dirty` object
  vector, which the append writer keys by object.

### Skip reasons (unsupported shapes)

`PageNotFound`, `CompressedLeafDictionary`, `OwnershipNotProven`,
`DuplicateBoxKey`, `UnsupportedBoxValue` (e.g. indirect box value),
`MalformedBoxValue`, `MissingEffectiveMediaBox`, `LeafUnreadable`. Because
compressed leaves only occur behind object streams / xref streams (which the
classic append writer rejects), the compressed skip mapping is covered by a
focused unit test rather than an end-to-end classic-doc write.

### Design notes

- Leaf references, box provenance, and skip reasons are read once through
  `presslint_pdf::inspect_document_page_boxes`; the leaf `<<` opener and
  `/Parent` are read through `inspect_indirect_object_dictionary` /
  `parse_indirect_reference`. No whole-document object cache is added.
- Copy budget: one rewritten body `Vec<u8>` per edited leaf plus the full output
  `Vec<u8>` from the append writer (owned-bytes API). Reports carry page indexes,
  references, small rectangles, ranges, and structured reasons only — never PDF
  source bytes, decoded streams, or page-tree dictionaries.

## Append writer (no-op foundation)

`write_incremental_revision` copies the caller's input verbatim as the output
prefix, then appends one classic incremental revision that rewrites selected
existing uncompressed indirect objects with caller-supplied body bytes. It is a
semantic **no-op** when the bodies are byte-identical, and the byte-assembly
substrate `set_page_boxes_incremental` builds on.

### Public API

- `DirtyObjectBytes { reference: presslint_pdf::IndirectRef, body_bytes: Vec<u8> }`
  — one existing uncompressed object to rewrite. `body_bytes` is the object body
  only (the bytes between the `N G obj` header and `endobj`); the writer wraps
  it but never inspects, decodes, or edits it.
- `write_incremental_revision(input: &[u8], dirty_objects: &[DirtyObjectBytes])
  -> Result<Vec<u8>, WriteError>` — copies `input` verbatim as the output prefix
  and appends exactly one classic incremental revision.
- `WriteError` — serde-tagged (`stage`) rejection enum, and `ActiveTrailerError`
  for the trailer-scan sub-failure. The fixed-width xref-entry formatter and
  offset limit are crate-internal details, covered by focused format tests.

### Invariants proven

- **Verbatim prefix**: `output[..input.len()] == input`. The output `Vec<u8>` is
  seeded with the input bytes; every later push only appends writer-owned bytes
  (LF end-of-line). A single leading `\n` is inserted before the first appended
  object only when the input does not already end in an EOL byte.
- **Offset accounting**: each appended object's offset is recorded before its
  `N G obj` header is written, and the classic xref entry for that object carries
  that offset. Verified by newest-wins resolution landing on `output[offset..]`
  starting with the object header.
- **Classic xref entry width**: entries are the fixed 20-byte
  `{offset:010} {generation:05} n \n`; offsets above `9_999_999_999` are rejected
  (`XrefOffsetTooLarge`) rather than truncated. Consecutive dirty object numbers
  are grouped into subsections.
- **Trailer**: preserves `/Root` from the newest trailer, preserves `/ID`
  verbatim when present, preserves the active trailer `/Info` value verbatim
  when present, sets `/Prev` to exactly the previous `startxref` target, and
  sets `/Size` from the **whole classic `/Prev` chain** (max object number + 1,
  floored by the max declared `/Size`) — not the newest section or dirty set.
  This is the concrete `PDFBOX-5945` pitfall, and a two-revision fixture whose
  newest section understates `/Size` proves the older section's higher object
  number still counts.
- **Reopen + newest-wins**: appended output reopens through
  `presslint_pdf::inspect_document_access` (selecting the `ClassicXrefChain`
  backend), resolves the rewritten object to the appended offset, preserves page
  leaf count/order, and supports a second append that lengthens the `/Prev`
  chain. Output is independent of dirty-object ordering (deterministic sort by
  indirect reference).

### Rejections

Cross-reference-stream inputs, hybrid-reference classic trailers (`/XRefStm` in
the active trailer), encrypted inputs (`/Encrypt` in the active trailer),
duplicate dirty object numbers, dirty objects that do not resolve to an existing
in-use uncompressed object, generation mismatches, dirty objects whose resolved
xref offset does not point at a matching indirect object header
(`DirtyObjectHeaderMismatch`), and fixed-width xref offset overflow.

Dirty-object currency is validated through `resolve_xref_object_offset` with the
`ClassicXrefChain` backend, **not** the locate-only chain lookup: a newest-wins
`InUse` entry is accepted only after the indirect object header at its resolved
offset parses and its object/generation match the dirty reference. This closes
the header-validation gap: a stale, corrupt, or mis-pointed xref entry can no
longer be accepted and then silently shadowed by the appended object. The
delegated `ObjectResolutionError` is mapped back to the existing
`GenerationMismatch`/`DirtyObjectNotInUse` cases, with every header-validation
failure folded into `DirtyObjectHeaderMismatch { reference, error }`.

### Design notes

- Structural facts are read through `presslint-pdf` (`inspect_pdf_source` for the
  final `startxref` + section classification, `inspect_classic_xref_table` for
  the active trailer offset, `build_classic_xref_chain` for the newest-wins chain
  and whole-chain `/Size`, `resolve_xref_object_offset` for header-validated
  object currency), never reparsed here. The one internal abstraction,
  `AppendRevisionWriter`, owns byte assembly and offset accounting.
- Copy budget: the returned `Vec<u8>` necessarily contains a full copy of the
  input (owned-bytes API) plus the caller-supplied bodies and small trailer/xref
  metadata. `/ID` and `/Info` trailer values are borrowed from the input while
  assembling the appended trailer and copied only into the returned output
  buffer. No PDF source bytes, object bodies, stream bodies, decoded streams, or
  whole-document object cache are copied. Depends only on `presslint-pdf` (plus
  `serde` for the error contract); it does **not** depend on the umbrella
  `presslint` crate.

## Deferred (future semantic writer)

- Other page-box keys (`TrimBox`/`BleedBox`/`ArtBox`), `/Rotate` compensation,
  and page-geometry normalization beyond rectangle ordering.
- Private-copy cloning / `/Kids` reference rewiring for shared or unproven leaf
  pages (currently a structured skip); editing ancestor `/Pages` dictionaries.
- Content-operand rewriting, whole-stream re-encoding, and color conversion.
- Xref-stream and hybrid `/XRefStm` incremental-revision support; object-stream
  / compressed-object (type-2) mutation.
- Object deletion, free-list repair, garbage collection, encryption
  preservation, full rewrite, and PDF repair.
- Plan-to-writer wiring from `presslint-actions` (the `MutationBoundary` /
  `IncrementalRevisionPlan` bridge); `set_page_boxes_incremental` and the
  `SetPageBox` planner currently re-derive boundaries independently.
