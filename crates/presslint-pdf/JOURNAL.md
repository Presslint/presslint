# presslint-pdf Journal

## Current State

- Defines initial structural PDF access data contracts for indirect references
  and document info.
- Adds bounded source inspection over caller-provided bytes:
  `inspect_pdf_source` reports total byte length, `%PDF-M.N` header offset and
  version from a fixed leading window, and final `startxref` offset from a fixed
  trailing window when the marker, decimal offset, and `%%EOF` are present.
- Classifies the cross-reference section style at the resolved `startxref`
  offset: a bounded window (`XREF_SECTION_SCAN_LIMIT`) read at that offset is
  reported as a classic `xref` table (`XrefSection::Table`) when it begins,
  after optional PDF whitespace, with the `xref` keyword, or as a
  cross-reference stream (`XrefSection::Stream`) carrying the parsed object and
  generation numbers when it begins with an `N G obj` indirect object header.
  An out-of-bounds offset, an unrecognized shape, or object/generation numbers
  that do not fit `u32`/`u16` produce a non-fatal `XrefSectionUnclassified`
  diagnostic and leave the classification absent. This never reads table
  entries, the trailer dictionary, the stream dictionary, or any object body,
  and never follows `/Prev` chains or earlier xref sections.
- Adds `inspect_classic_xref_table`, a bounded helper for caller-provided bytes
  and an expected classic `xref` table offset. It reports the `xref` keyword
  byte offset, subsections in source order, fixed-width entries in deterministic
  object-number order within each subsection, each entry's object number,
  generation, byte offset, free/in-use state, and the byte offset where the
  following `trailer` keyword begins. The helper stops at `trailer` and does
  not parse or retain the trailer dictionary.
- Classic xref table inspection returns structured public errors for
  out-of-bounds offsets, non-table offsets, malformed subsection headers,
  malformed entries, numeric range failures, subsection object-number overflow,
  and missing trailers.
- Regression coverage pins exact-EOF keyword handling: a `trailer` keyword at
  EOF is accepted, while bare `xref` input with or without trailing whitespace
  reports `MissingTrailer`.
- Regression coverage also pins fixed-width entry termination: a short
  `offset generation state` entry followed by a blank line is still rejected as
  `MalformedEntry`.
- Adds `resolve_classic_xref_object`, a pure locate-only helper over an existing
  `ClassicXrefTableInspection`. It reports in-use, free, not-found, and
  ambiguous object-number results without reading source bytes, doing I/O,
  allocating a lookup map, or parsing object bodies.
- Reports malformed or unsupported source shape through structured public
  rejection and diagnostic enums without retaining or copying PDF bytes.
- The only owned allocations introduced by classic table inspection are the
  public report vectors for subsection and entry metadata; no PDF bytes, object
  bodies, stream bodies, or trailer dictionary bytes are copied into reports.
- Adds a Criterion benchmark target, `pdf_source`, covering synthetic classic
  xref table inspection throughput.
- Models indirect-object edit ownership as proven single-use, shared, or
  unproven, with a pure helper that permits in-place mutation only for exactly
  one proven owning consumer.
- Does not yet open files, parse trailer dictionaries, xref streams, objects,
  streams, page trees, or catalogs; it does not decode streams, mutate bytes,
  or connect to inventory/action planning.

## Follow-Ups

- Keep full PDF file parsing deferred until syntax, graphics-state, inventory,
  selectors, and planning/action slices have stable contracts.
- Build future object access on the source-inspection boundary without widening
  this report into whole-file eager parsing. Trailer dictionary parsing,
  `/Type /XRef`, `/W`, `/Index` stream dictionary handling, stream-body
  decoding, and `/Prev` incremental chains remain deferred.
- Use the ownership decision model before future write planning mutates any
  indirect object that may be referenced by more than one consumer.
