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
- Reports malformed or unsupported source shape through structured public
  rejection and diagnostic enums without retaining or copying PDF bytes.
- Models indirect-object edit ownership as proven single-use, shared, or
  unproven, with a pure helper that permits in-place mutation only for exactly
  one proven owning consumer.
- Does not yet open files, parse xref tables, trailers, objects, streams, page
  trees, or catalogs; it does not decode streams, mutate bytes, or connect to
  inventory/action planning.

## Follow-Ups

- Keep full PDF file parsing deferred until syntax, graphics-state, inventory,
  selectors, and planning/action slices have stable contracts.
- Build future object access on the source-inspection boundary without widening
  this report into whole-file eager parsing. Cross-reference parsing (table
  entries/subsections, the trailer dictionary, the `/Type /XRef`, `/W`,
  `/Index` stream dictionary, stream-body decoding, and `/Prev` incremental
  chains) stays deferred behind this style classification.
- Use the ownership decision model before future write planning mutates any
  indirect object that may be referenced by more than one consumer.
