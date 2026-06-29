# presslint-pdf Journal

## Current State

- Defines initial structural PDF access data contracts for indirect references
  and document info.
- Adds bounded source inspection over caller-provided bytes:
  `inspect_pdf_source` reports total byte length, `%PDF-M.N` header offset and
  version from a fixed leading window, and final `startxref` offset from a fixed
  trailing window when the marker, decimal offset, and `%%EOF` are present.
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
  this report into whole-file eager parsing.
- Use the ownership decision model before future write planning mutates any
  indirect object that may be referenced by more than one consumer.
