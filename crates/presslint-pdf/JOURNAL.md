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
- Adds `inspect_indirect_object_header`, a bounded helper for caller-provided
  bytes and an expected indirect object byte offset. It skips optional PDF
  whitespace, reports the parsed `IndirectRef`, the resolved header start, the
  header byte range through the `obj` keyword, and the byte immediately after
  `obj`. It validates public numeric ranges and returns structured errors for
  out-of-bounds offsets, malformed headers, and object/generation range
  failures without retaining PDF bytes or parsing object bodies.
- Adds `inspect_indirect_object_body_token`, a pure report-only helper for
  caller-provided bytes and an expected indirect object body offset. It skips
  PDF whitespace, reports the resolved first-token byte offset, and classifies
  only the broad leading token family: dictionary open `<<`, hex-string open
  `<`, array open, name, literal string, number-like, boolean, or null. It
  returns structured errors for offsets at or beyond EOF, whitespace-only tails,
  and unclassified leading bytes, without copying object bodies, stream bodies,
  dictionaries, arrays, strings, names, or numeric values.
- Adds `inspect_dictionary_extent`, a bounded single-pass helper for
  caller-provided bytes and a byte offset that begins a dictionary (typically
  the `first_token_byte_offset` reported for a `DictionaryOpen`). It skips
  optional PDF whitespace, requires the first significant token to be the `<<`
  dictionary-open delimiter, and tracks `<<`/`>>` nesting depth so the reported
  close is the matching close of the outermost `<<`, not the first inner `>>`.
  It reports the open `<<` offset, the closing `>>` offset, the exclusive
  byte offset after the close, and the deepest observed nesting depth. Literal
  strings `( ... )` (honoring `\` escapes and balanced unescaped parentheses),
  hex strings `< ... >` (a `<` not followed by `<`), and `%` comments are
  skipped as opaque spans so `<<`, `>>`, `>`, and `<` bytes inside them never
  affect the depth count. A private `MAX_DICTIONARY_NESTING_DEPTH` constant
  bounds pathological inputs: exceeding it yields a structured
  `MaxNestingExceeded` rejection rather than unbounded work. The helper decodes
  no key, value, name, number, or string contents, and never retains or copies
  PDF bytes; it reports only byte offsets and the depth scalar. It returns
  structured errors for offsets at or beyond EOF, whitespace-only tails, a first
  token that is not `<<`, an unterminated literal or hex string, and an
  unterminated dictionary.
- Adds `inspect_array_extent`, a bounded single-pass helper for caller-provided
  bytes and a byte offset that begins an array (typically the
  `first_token_byte_offset` reported for an `ArrayOpen`). It skips optional PDF
  whitespace, requires the first significant token to be the `[` array-open
  delimiter, and tracks `[`/`]` nesting depth so the reported close is the
  matching close of the outermost `[`, not the first inner `]`. It reports the
  open `[` offset, the closing `]` offset, the exclusive byte offset after the
  close, and the deepest observed nesting depth. Literal strings `( ... )`
  (honoring `\` escapes and balanced unescaped parentheses), hex strings
  `< ... >` (a `<` not followed by `<`), and `%` comments are skipped as opaque
  spans so `]` bytes inside them never affect the depth count. A `<<` dictionary
  open is advanced past as a nested-dictionary delimiter so its leading `<` is
  not misread as a hex-string open; only `[`/`]` delimiters drive the depth
  count. A private `MAX_ARRAY_NESTING_DEPTH` constant bounds pathological
  inputs: exceeding it yields a structured `MaxNestingExceeded` rejection rather
  than unbounded work. The helper decodes no element, name, number, or string
  contents, and never retains or copies PDF bytes; it reports only byte offsets
  and the depth scalar. It returns structured errors for offsets at or beyond
  EOF, whitespace-only tails, a first token that is not `[`, an unterminated
  literal or hex string, and an unterminated array. It lives in its own
  `array_extent.rs` module and leaves `inspect_dictionary_extent` and
  `inspect_indirect_object_body_token` unchanged.
- Adds `inspect_classic_xref_trailer_dictionary`, a pure composition helper for
  caller-provided bytes and a classic xref `trailer` keyword offset. It skips
  optional PDF whitespace at the caller offset, validates the resolved
  `trailer` keyword with shared keyword-boundary rules, skips optional
  whitespace after it through `inspect_dictionary_extent`, and reports only the
  caller offset, resolved trailer keyword offset, dictionary open/close/after
  offsets, and maximum observed dictionary nesting depth. It returns structured
  public errors for offsets at or beyond EOF, missing `trailer` keywords, and
  delegated dictionary-extent rejections, without parsing trailer keys such as
  `/Size`, `/Root`, or `/Prev`.
- Adds `inspect_dictionary_entries`, a bounded shallow scanner for
  caller-provided bytes and a dictionary-open byte offset. It first validates
  and bounds the outer dictionary through `inspect_dictionary_extent`, then
  scans only between the outer `<<` and its matching `>>` for top-level
  `/Name value` pairs. Each entry reports key and value byte ranges plus a
  shallow value kind (`dictionary`, `array`, `name`, `string`, `number_like`,
  `boolean`, `null`, `indirect_reference_like`, or `other_scalar`) without
  retaining or copying key/value bytes. Nested dictionaries and arrays are
  skipped as opaque values through the existing extent helpers, while literal
  strings, hex strings, and comments are treated as opaque spans so delimiters
  inside them do not split top-level entries. Scalar scanning recognizes
  `N G R`-shaped values as one indirect-reference-like span but does not resolve
  or interpret references or any specific dictionary key semantics. The helper
  returns structured public errors for malformed non-name top-level keys,
  missing values, delegated dictionary/array extent failures, and unterminated
  string spans.
- Shares literal-string, hex-string, and `%`-comment opaque-span skip helpers
  through the internal `source_utils` module so the string/comment scanning
  rules live in one place alongside the existing whitespace/delimiter helpers.
- Reports malformed or unsupported source shape through structured public
  rejection and diagnostic enums without retaining or copying PDF bytes.
- The only owned allocations introduced by classic table inspection are the
  public report vectors for subsection and entry metadata; no PDF bytes, object
  bodies, stream bodies, or trailer dictionary bytes are copied into reports.
- Splits source inspection internals into focused modules for bounded source
  orchestration, final `startxref` inspection, xref-section classification,
  classic xref table inspection, and shared byte-scanning helpers. Public
  crate-root re-exports and report/error shapes remain unchanged.
- Reuses the shared byte-scanning helpers for indirect-object header inspection
  so whitespace, digit, and keyword boundary rules stay in one internal module.
- Moves source and classic xref table regression coverage into focused
  `src/tests/source.rs` and `src/tests/classic_xref.rs` modules.
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
