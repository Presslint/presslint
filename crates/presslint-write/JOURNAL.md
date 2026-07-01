# presslint-write Journal

## Current State

`presslint-write` is the first byte-writing crate: a deterministic classic-xref
**incremental append** writer whose only capability is a semantic **no-op**.

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

- Semantic dictionary editing, `SetMediaBox`/`SetPageBox`, content-operand
  rewriting, whole-stream re-encoding, and color conversion.
- Xref-stream and hybrid `/XRefStm` incremental-revision support; object-stream
  / compressed-object mutation.
- Private-copy cloning of shared objects, object deletion, free-list repair,
  garbage collection, encryption preservation, full rewrite, and PDF repair.
- Plan-to-writer wiring from `presslint-actions` (the `MutationBoundary` /
  `IncrementalRevisionPlan` bridge).
