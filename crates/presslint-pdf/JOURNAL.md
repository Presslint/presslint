# presslint-pdf Journal

## Current State

- Adds `inspect_document_page_content_extents`, the document-order aggregation
  layer for page content-stream data extents. Given caller bytes, a
  `&ClassicXrefTableInspection`, and the byte offset of an already-located root
  `/Pages` node, it first delegates to `inspect_page_tree_leaves`; a root
  leaf-enumeration failure is the helper's only top-level `Err` and is wrapped
  in `DocumentPageContentExtentsInspectionError` with the root offset and source
  length. On success, `DocumentPageContentExtentsInspection` carries the
  caller-visible `byte_len`, the full delegated `PageTreeLeavesInspection`, and
  exactly one document-ordered `DocumentPageContentExtentInspection` per
  enumerated `PageTreeLeaf`. Each per-page result stores the zero-based ordinal
  and original leaf metadata so callers can connect document order, page
  reference, and page object offset without relying on vector position alone.
  For each leaf, the helper delegates in order to `inspect_page_contents`,
  `inspect_page_content_targets`, and `inspect_page_content_extents`; successful
  pages carry those three delegated reports in
  `DocumentPageContentExtentResult::Inspected`. A leaf whose `/Contents`
  inspection fails is recorded as
  `DocumentPageContentExtentResult::ContentsFailed`, preserving the delegated
  `PageContentsInspectionError` while later leaves continue through the same
  pipeline. Skipped leaf-tree diagnostics and truncation markers remain only in
  the delegated `leaves` field and are not promoted into page results or
  reinterpreted by the aggregate. The report exposes `#[must_use]`
  `page_count()`, per-page `is_located()`, and `located_page_count()` helpers;
  a page is counted as located when `/Contents` inspection succeeded and every
  delegated content target has a located extent. The implementation reimplements
  no page-tree traversal, `/Contents` parsing, xref target resolution, `/Length`
  parsing, or `endstream` validation; it only assembles delegated reports in
  deterministic document order. It retains or copies no PDF bytes, object
  bodies, page dictionaries, stream dictionaries, stream bytes, decoded bytes,
  concatenated content buffers, or source slices; owned data is limited to
  delegated reports, offsets, ordinals, small enums, and the source-ordered
  per-page result vector. No benchmark was added because the only new allocation
  is deterministic public report materialization and the work is a bounded
  delegation loop over already-enumerated leaves. Non-goals for this slice:
  no filesystem opener or trailer-to-document pipeline, no decoding,
  decompression, concatenation, tokenization, `/Resources`/boxes/annotations or
  inherited-attribute inspection, `/Count` validation, `/Prev` traversal, xref
  stream/object stream parsing, caches, object maps, syntax/inventory/selectors/
  actions bridging, or mutation planning. Focused tests cover a multi-page
  classic-xref fixture with all pages located in document order, a missing
  `/Contents` page that fails per-page while a later leaf is still processed,
  preservation of delegated leaf skips plus cycle truncation separately from
  per-page content failure, and a serde round-trip with one inspected page and
  one per-page `/Contents` failure.

- Adds `inspect_page_tree_kid_targets`, a focused one-node page-tree expansion
  helper. Given caller bytes, a `&ClassicXrefTableInspection`, and the byte
  offset of an already-located page-tree node, it first delegates to
  `inspect_page_tree_kids` and carries the full `PageTreeKidsInspection` in the
  aggregate report. The helper then walks the delegated `kids.kids` vector in
  source order and delegates each original `PageTreeKidReference::reference` to
  `inspect_page_tree_reference_target`, producing exactly one
  `PageTreeKidTargetInspection` entry per direct kid reference without
  reordering or deduplicating. `Resolved` entries preserve the original kid
  reference metadata plus the delegated `PageTreeReferenceTargetInspection`, so
  callers can distinguish `/Pages`, `/Page`, and other-name targets through the
  delegated node-type report. `Failed` entries preserve the original kid
  reference metadata plus the delegated
  `PageTreeReferenceTargetInspectionError`, and a failed child does not abort
  later kids. Malformed or unsupported top-level `/Kids` entries remain only in
  the delegated `PageTreeKidsInspection::skipped` list; this helper does not
  reinterpret, drop, or promote them to target entries. The aggregate exposes
  caller-visible `byte_len`, the delegated kids report, the source-ordered
  child-result vector, and a `#[must_use] resolved_count()` accessor for the
  number of successful child resolutions. It retains or copies no PDF bytes,
  object bodies, stream bodies, page dictionaries, page-tree dictionaries,
  content streams, decoded streams, or source slices; owned data is limited to
  the delegated reports and the deterministic result vector. The helper lives in
  `page_tree_kid_targets.rs` as a sibling module and leaves `page_tree_kids.rs`
  and `page_tree_reference.rs` public fields and serde shapes unchanged. Non-
  goals for this slice: no recursive page-tree traversal, leaf-page enumeration,
  `/Count` validation, page `/Contents`/resources/boxes/annotations inspection,
  `/Parent` parsing, xref-stream/object-stream support, `/Prev` traversal,
  filesystem I/O, caches, indexes, or mutation.

- Adds `inspect_page_content_extents`, the page-level aggregator that locates the
  ordered content-stream data byte extents for a single leaf page. It takes the
  caller bytes, a `&ClassicXrefTableInspection`, and a
  `&PageContentTargetsInspection` (the resolved `/Contents` targets from
  `inspect_page_content_targets`), and walks `targets.entries` once in source
  order, producing exactly one source-ordered result per target and never
  reordering or deduplicating, so the page's content-stream order is preserved.
  Each `PageContentTargetInspection::Resolved` target is delegated to
  `inspect_content_stream_data_extent` at its resolved `object_byte_offset` with
  `Some(xref)`: on success the entry is `PageContentExtentInspection::Located`
  carrying the original `PageContentReference`, the resolved `object_byte_offset`,
  and the delegated `ContentStreamDataExtentInspection`; on failure it is
  `PageContentExtentInspection::Failed`, preserving the underlying
  `ContentStreamDataExtentInspectionError` and the resolved offset without
  aborting the remaining targets. Each `PageContentTargetInspection::Skipped`
  target is carried through unchanged as `PageContentExtentInspection::Skipped`,
  preserving the original `PageContentReference` and the existing
  `SkippedPageContentTargetReason`, with no extent inspection attempted. The
  aggregate `PageContentExtentsInspection` exposes the caller-visible source
  `byte_len`, the source-ordered `Vec` of per-target results, and a `#[must_use]`
  `located_count()` accessor returning the number of `Located` entries so callers
  can detect a fully-located page (`located_count() == entries.len()`) without
  re-matching variants. For a given input the per-target located extent equals
  byte-for-byte what `inspect_content_stream_data_extent` returns for the same
  resolved object offset and xref table; the aggregator reimplements no
  `/Length`, indirect-resolution, or `endstream` logic and only iterates targets
  and dispatches. It retains or copies no stream bytes, decoded bytes, object
  bodies, dictionaries, source slices, or concatenated content buffer; the only
  owned data is the source-ordered `Vec` of fixed-size delegated reports plus the
  copied small `PageContentReference`/offset metadata already present in the
  delegated reports. Each delegated `inspect_content_stream_data_extent` still
  re-inspects its own stream start (as documented for the combined extent helper);
  this aggregator adds no new redundant source scan of its own. It lives in its
  own `page_content_extents.rs` module and leaves `page_content_targets.rs`,
  `page_contents.rs`, and `content_stream_extent.rs` public surfaces, fields, and
  serde shapes unchanged (no `pub(crate)` factoring was needed). Non-goals for
  this slice: it does not decode, decompress, concatenate, or tokenize stream
  bytes, materialize a concatenated content buffer, follow `/Prev`, parse xref or
  object streams, build object maps/caches, resolve indirect-reference chains, or
  bridge content streams to `presslint-syntax`/`presslint-inventory`/selectors/
  actions/mutation planning. A composition test chains
  `inspect_page_contents -> inspect_page_content_targets -> inspect_page_content_extents`
  over a synthetic classic-xref/page fixture and confirms the located extents
  match calling `inspect_content_stream_data_extent` directly on each resolved
  offset.

- Defines initial structural PDF access data contracts for indirect references
  and document info.
- Adds `inspect_content_stream_data_extent`, a focused public aggregator for
  dictionary-bodied content stream objects whose top-level `/Length` is either
  a direct non-negative integer or a classic-xref indirect reference. It first
  uses `inspect_content_stream_start`, selects exactly one exact raw
  top-level `/Length` entry through the shared entry-selection helper, dispatches
  on the entry's `DictionaryValueKind`, and then delegates validation to
  `inspect_direct_length_content_stream_data_extent` for `NumberLike` values or
  to `inspect_indirect_length_content_stream_data_extent` for
  `IndirectReferenceLike` values when a `ClassicXrefTableInspection` is supplied.
  Success is the `ContentStreamDataExtentInspection` enum carrying the focused
  direct or indirect report plus common `length`,
  `stream_data_start_byte_offset`, and `stream_data_end_byte_offset` accessors.
- Combined content-stream extent inspection has dispatch-level structured
  rejections for stream-start failures, missing and duplicate exact `/Length`
  entries, `IndirectLengthRequiresXrefTable`, and
  `UnsupportedLengthValueKind` carrying the observed `DictionaryValueKind`.
  Delegated direct and indirect failures are surfaced through dedicated
  `DirectLength` and `IndirectLength` channels that preserve the focused
  helper's underlying rejection reason. For a valid direct or indirect input,
  the enum payload is byte-for-byte the same focused-helper report that callers
  would receive by invoking the matching helper directly.
- The combined helper does no `/Length` parsing, indirect resolution,
  checked data-end arithmetic, `endstream` validation, fallback scanning,
  stream-byte reading, decoding, decompression, concatenation, content
  tokenization, filter validation, page semantics, selector/action planning, or
  mutation itself. The focused public helper selected by dispatch remains
  authoritative for those extent checks. This implementation inspects stream
  start once for dispatch and the selected focused helper inspects stream start
  again while producing its canonical report; no private shortcut was added that
  would bypass the public focused helpers. The new report and rejections retain
  no stream bytes, decoded bytes, object bodies, dictionaries, source slices, or
  copied PDF payloads; owned data is limited to fixed-size enum/report metadata
  already present in the delegated reports.
- Adds `inspect_direct_length_content_stream_data_extent`, a public,
  report-only helper for dictionary-bodied content stream objects whose
  top-level `/Length` value is a direct non-negative integer. It composes
  `inspect_content_stream_start` for object, dictionary, `stream` keyword, and
  stream-data-start validation, then scans the delegated dictionary entries for
  exactly one exact raw `/Length` key. The report carries the delegated
  `ContentStreamStartInspection`, `/Length` key/value byte ranges, the parsed
  byte length, the stream-data start offset, and the exclusive stream-data end
  offset computed with checked addition.
- Direct-length stream extent inspection accepts only `/Length` values whose
  delegated value span is a single ASCII-digit scalar that fits `usize`.
  Structured public rejections cover missing and duplicate `/Length`, indirect
  `N G R` length values, non-numeric direct values such as names/strings/arrays
  or dictionaries, malformed number-like values such as negative numbers and
  decimals, decimal values that do not fit `usize`, checked-addition overflow,
  computed data ends past EOF, invalid post-data EOL markers, and missing or
  misspelled `endstream` keywords at the computed position.
- The direct-length helper validates the bytes immediately after the declared
  data range structurally: LF and CRLF are accepted as the required
  stream-data terminator before `endstream`; EOF, a lone CR, non-EOL bytes, and
  an `endstream` spelling that fails the shared keyword-boundary rule are
  rejected. It performs no fallback scan for `endstream` when `/Length` is
  absent or malformed.
- The direct-length stream extent report retains no stream bytes, decoded
  bytes, object bodies, dictionaries, source slices, or copied PDF payloads.
  It adds no new owned byte buffers, caches, object maps, filesystem I/O, xref
  stream support, object stream support, indirect `/Length` resolution,
  `/Filter` or `/DecodeParms` validation, content-stream tokenization,
  decompression, concatenation, or page semantics.
- Adds `inspect_indirect_length_content_stream_data_extent`, the sibling
  report-only helper for dictionary-bodied content stream objects whose
  top-level `/Length` value is an indirect reference resolved through a
  caller-supplied `ClassicXrefTableInspection`. It composes
  `inspect_content_stream_start`, scans the delegated top-level dictionary
  entry metadata for exactly one exact raw `/Length` key, requires that value
  to be `DictionaryValueKind::IndirectReferenceLike`, delegates the value span
  to `resolve_classic_xref_integer_object`, and reports the delegated
  `ContentStreamStartInspection`, `/Length` key/value byte ranges, delegated
  `ClassicXrefIntegerObjectResolution`, resolved byte length, stream-data start
  offset, and exclusive stream-data end offset computed with checked addition.
- Indirect-length stream extent inspection rejects missing and duplicate
  `/Length` keys, direct or otherwise non-reference `/Length` values, delegated
  indirect-integer resolution failures while preserving the underlying
  `ClassicXrefIntegerObjectResolutionRejection`, checked-addition overflow,
  computed data ends past EOF, invalid post-data EOL markers, and missing or
  misspelled `endstream` keywords at the computed position. It accepts only the
  resolved one-level non-negative integer object; it does not follow reference
  chains.
- The indirect-length helper uses the same structural `endstream` policy as
  the direct helper: the computed data end must be in bounds, then LF or CRLF
  must appear immediately before an exact `endstream` keyword accepted by the
  shared keyword-boundary rule. There is no fallback scan when `/Length` is
  missing, malformed, unresolved, or inconsistent.
- The indirect-length stream extent report retains no stream bytes, decoded
  bytes, object bodies, dictionaries, source slices, or copied PDF payloads.
  It adds no copied stream buffers, decoded buffers, object maps, caches,
  filesystem I/O, `/Prev` traversal, xref stream parsing, object stream
  parsing, `/Filter` or `/DecodeParms` validation, content-stream tokenization,
  decompression, concatenation, page semantics, selectors, actions, or mutation
  planning. Private factoring is limited to exact `/Length` entry selection and
  fixed-position `endstream` validation shared with the direct helper, without
  changing the direct helper's public fields or serde shape.
- Adds `resolve_classic_xref_integer_object`, a focused report-only composition
  helper for caller-provided bytes, an existing `ClassicXrefTableInspection`, and
  the byte offset where an `N G R` value begins (typically a `DictionaryEntrySpan`
  `value_range.start` classified `IndirectReferenceLike`, such as an indirect
  `/Length`). It composes existing bounded inspectors only: `parse_indirect_reference`
  for the `N G R` value, `resolve_classic_xref_object` to locate the referenced
  object, `inspect_indirect_object_header` to validate the resolved object header,
  and `inspect_indirect_object_body_token` to require a `NumberLike` leading
  token; it then parses the leading ASCII-digit run as the integer value. Only a
  single `ClassicXrefObjectLocation::InUse` entry is accepted; `Free`, `NotFound`,
  and `Ambiguous` locations produce the structured `FreeObject`, `ObjectNotFound`,
  and `AmbiguousObject` rejections, each carrying the resolved object number in
  the error's `object_number` field. The resolved object header's parsed
  `IndirectRef` is checked against the reference's object and generation numbers;
  a mismatch is a `ReferenceMismatch` rejection carrying the header's `IndirectRef`,
  not a silent acceptance. The integer body is accepted only as a non-negative
  ASCII-digit run terminated by PDF whitespace, a delimiter, or the `endobj`
  keyword: `-1`, `1.0`, `+1`, an empty run, and trailing non-delimiter garbage
  (and an end-of-file with no terminator) are `MalformedInteger`, a value that
  does not fit `usize` is the distinct `IntegerOutOfRange`, and a non-number-like
  body leading token (name, string, array, dictionary, boolean, null, etc.) is
  `NonIntegerBody` carrying the classified `IndirectObjectBodyLeadingTokenKind`.
  Delegated reference, header, and body-token failures surface through the
  dedicated `Reference`, `Header`, and `BodyToken` channels with the underlying
  rejection reason preserved. The report carries the parsed `IndirectRef`, the
  resolved in-use object byte offset, the integer value byte range, and the parsed
  `usize`; it retains or copies no PDF bytes, object bodies, stream bodies,
  dictionaries, or source slices. It resolves exactly one reference one level
  deep: it does not follow chains of indirect references, read `/Prev`, parse
  object streams, or resolve anything beyond the single referenced integer object.
  The only owned allocation is the fixed-size public report; the work is one
  reference parse, one allocation-free xref-table scan, one delegated header
  inspection, one body-token classification, and a fixed-size checked digit-run
  parse, so no benchmark was added. It lives in its own `integer_object.rs`
  module (sibling to `object_stream.rs` / `object_dictionary.rs`). A composition
  test parses a `<< /Length 7 0 R >>` dictionary's `IndirectReferenceLike`
  `/Length` value span with `inspect_dictionary_entries` and resolves it to an
  integer through this helper. This primitive is now also used by
  `inspect_indirect_length_content_stream_data_extent` for the deferred indirect
  case of content-stream `/Length` handling.
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
- Adds `parse_indirect_reference`, a bounded report-only helper for
  caller-provided bytes and a byte offset that begins an `N G R` indirect
  reference (typically the `value_range.start` of a `DictionaryEntrySpan`
  classified as `IndirectReferenceLike`). It skips optional PDF whitespace over
  a fixed leading window (`INDIRECT_REFERENCE_SCAN_LIMIT`), parses
  `object-number generation R`, and reports the parsed `IndirectRef`, the
  resolved reference start, the byte range through the `R` keyword, and the byte
  immediately after `R`. It is the direct sibling of
  `inspect_indirect_object_header`: same `N G <keyword>` discipline, but the
  keyword is `R`, validated with the shared keyword-boundary rule
  (`consume_keyword`) so `Robot` or `R0` is not accepted and an `N G obj` header
  (or any other trailing keyword) is rejected as malformed rather than parsed as
  a reference. It validates public numeric ranges and returns structured errors
  for offsets at or beyond EOF (`OffsetOutOfBounds`), malformed `N G R` shape
  (`MalformedReference`), an object number that does not fit `u32`
  (`ObjectNumberOutOfRange`), and a generation that does not fit `u16`
  (`GenerationOutOfRange`), mirroring the header helper's rejection style. It
  retains or copies no PDF bytes, allocates nothing beyond the fixed-size public
  report, and does not resolve or follow the reference, inspect the referenced
  object's header or body, or read any dictionary, array, stream, trailer, or
  `/Prev` chain. It lives in its own `indirect_reference.rs` module. A
  composition test chains `inspect_classic_xref_table ->
  inspect_classic_xref_trailer_dictionary -> inspect_dictionary_entries ->
  parse_indirect_reference` over synthetic classic-xref bytes, parsing the
  `/Root` entry's `IndirectReferenceLike` value span into an `IndirectRef` and
  feeding its object number to `resolve_classic_xref_object` to locate the
  catalog object's byte offset without inspecting the catalog body.
- Adds `inspect_classic_xref_trailer_root`, a focused composition helper for
  caller-provided bytes and a classic xref `trailer` keyword offset. It
  delegates trailer dictionary location to `inspect_classic_xref_trailer_dictionary`,
  scans top-level trailer entries with `inspect_dictionary_entries`, matches
  only the exact raw top-level key bytes `/Root`, and parses the selected value
  with `parse_indirect_reference`. The report carries the delegated trailer
  dictionary offsets, `/Root` key and value byte ranges, and the parsed
  `IndirectRef`; it does not retain or copy trailer bytes, object bodies,
  stream bodies, catalog dictionaries, page trees, or referenced-object bytes,
  and it does not resolve the root reference. Structured public rejections cover
  delegated trailer dictionary failures, delegated dictionary entry failures,
  missing `/Root`, duplicate exact `/Root` keys, direct non-reference values
  such as dictionaries/names/numbers, and malformed scalar reference attempts
  such as `1 0 obj`.
- Adds `inspect_indirect_object_dictionary`, a focused composition helper for
  caller-provided bytes and an indirect object byte offset (typically a
  `ClassicXrefObjectLocation::InUse` offset). It is the object-level sibling of
  `inspect_classic_xref_trailer_dictionary`: where that helper bridges a
  `trailer` keyword to a dictionary, this one bridges an `N G obj` header to a
  dictionary-bodied object's top-level entries. It composes existing bounded
  inspectors only: it resolves the header with `inspect_indirect_object_header`,
  classifies the body's leading token with `inspect_indirect_object_body_token`,
  requires that token to be the dictionary-open `<<`, then scans top-level
  `/Name value` spans with `inspect_dictionary_entries` at the reported
  first-token offset. The report carries the resolved header byte range and
  parsed `IndirectRef`, the dictionary open/close/after offsets, the maximum
  observed dictionary nesting depth, and the delegated `Vec<DictionaryEntrySpan>`;
  it retains or copies no PDF bytes, object bodies, stream bodies, key bytes, or
  value bytes (keys and values stay addressed by range). Structured public
  rejections distinguish a delegated header-inspection failure (`Header`), a
  delegated body-token classification failure including offsets at or beyond EOF
  surfaced by that helper (`BodyToken`), a non-dictionary body leading token such
  as an array/name/number/string/boolean/null (`NonDictionaryBody` carrying the
  classified `IndirectObjectBodyLeadingTokenKind`), and a delegated
  dictionary-entry inspection failure (`DictionaryEntries`). It interprets no
  keys such as `/Type`, `/Pages`, `/Kids`, `/Count`, or `/Contents`, resolves no
  indirect references found in values, decodes no name escapes or key/value
  bytes, and locates no `stream`/`endstream`/`endobj`/`/Length` or object body
  end beyond the dictionary extent. It lives in its own `object_dictionary.rs`
  module. A composition test chains `inspect_classic_xref_table ->
  resolve_classic_xref_object -> inspect_indirect_object_dictionary` over
  synthetic classic-xref bytes, locating a catalog-shaped object and a page-tree
  root object and reporting their top-level entry keys (`/Type`, `/Pages`,
  `/Kids`, `/Count`) as spans without copying their bytes.
- Adds `inspect_catalog_pages`, a focused composition helper for
  caller-provided bytes and an already-located catalog object byte offset. It
  delegates catalog-shaped object inspection to
  `inspect_indirect_object_dictionary`, matches only the exact raw top-level
  key bytes `/Pages`, and parses the selected value through
  `parse_indirect_reference`. The report carries the delegated object
  dictionary inspection, `/Pages` key and value byte ranges, and the parsed
  page-tree root `IndirectRef`; it retains or copies no PDF bytes, object
  bodies, stream bodies, page-tree dictionaries, page dictionaries, contents
  streams, or referenced-object bytes. Structured public rejections cover
  delegated catalog dictionary failures, missing `/Pages`, duplicate exact
  `/Pages` keys, direct non-reference values such as dictionaries/names/numbers,
  and malformed scalar reference attempts such as `2 0 obj`. This slice does
  not validate `/Type /Catalog`, decode name escapes, resolve the parsed
  `/Pages` reference, traverse `/Kids`, inspect `/Count`, page dictionaries,
  `/Contents`, resources, annotations, inherited attributes, streams, xref
  streams, object streams, encryption, linearization, incremental updates, or
  `/Prev` chains.
- Adds `inspect_page_tree_node`, a focused composition helper for
  caller-provided bytes and an already-located page-tree node object byte offset
  (typically the target of a catalog `/Pages` reference). It delegates
  page-tree-node object inspection to `inspect_indirect_object_dictionary`,
  matches only the exact raw top-level key bytes `/Kids` and `/Count` from each
  entry's `key_range`, validates the shallow `/Kids` value kind as `array`, and
  bounds that array value with `inspect_array_extent` at the value range start.
  The report carries the delegated `IndirectObjectDictionaryInspection`, the
  `/Kids` key/value byte ranges, the delegated `ArrayExtentInspection` for the
  `/Kids` value (open/close/after offsets and observed nesting depth), and the
  `/Count` key/value byte ranges; it retains or copies no PDF bytes, object
  bodies, stream bodies, page dictionaries, contents streams, `/Kids` array
  elements, or referenced-object bytes, reporting only byte ranges, offsets, and
  the depth scalar. Structured public rejections distinguish a delegated
  object-dictionary failure (`NodeDictionary`), missing `/Kids` (`MissingKids`),
  duplicate exact `/Kids` (`DuplicateKids`), a non-array `/Kids` value
  (`NonArrayKidsValue`), a delegated `/Kids` array-extent failure
  (`KidsArrayExtent`), missing `/Count` (`MissingCount`), duplicate exact
  `/Count` (`DuplicateCount`), and a non-number-like `/Count` value
  (`NonNumberCountValue`). The standalone `inspect_array_extent` call is what
  populates the report's `kids_array_extent`; because
  `inspect_indirect_object_dictionary` already bounds array values through
  `inspect_dictionary_entries`, an unterminated `/Kids` array currently fails at
  that delegated step and surfaces its array-extent rejection reason through the
  `NodeDictionary` channel, while `KidsArrayExtent` remains the dedicated error
  channel for the bounding call. Non-goals for this slice: it does not scan
  `/Kids` array elements into indirect references (it only bounds the array),
  parse the `/Count` integer (it confirms only the shallow number-like value
  kind and reports its byte range), descend into child page-tree nodes or page
  objects, resolve `/Kids` references, follow `/Parent`, require or decode
  `/Type /Pages`, decode name escapes, inspect page dictionaries, `/Contents`,
  `/MediaBox`, resources, annotations, or inherited attributes, or follow
  `/Prev` chains, xref streams, object streams, encryption, linearization, or
  incremental updates. It lives in its own `page_tree_node.rs` module. A
  composition test chains `inspect_classic_xref_table ->
  inspect_classic_xref_trailer_root -> resolve_classic_xref_object ->
  inspect_catalog_pages -> resolve_classic_xref_object -> inspect_page_tree_node`
  over synthetic bytes to bound the `/Kids` array and locate `/Count` without
  scanning array elements.
- Adds `inspect_page_tree_kids`, a focused composition helper for
  caller-provided bytes and an already-located page-tree node object byte
  offset. It delegates node boundary discovery to `inspect_page_tree_node`, then
  scans only the already-bounded `/Kids` array body between the outer `[` and
  matching `]` in a single forward pass. Direct top-level `N G R` entries are
  parsed through the existing `parse_indirect_reference` helper, preserving its
  keyword-boundary and numeric-range checks. The report carries the delegated
  `PageTreeNodeInspection`, direct kid `IndirectRef` values with
  `IndirectReferenceByteRange`s, and shallow skipped-entry diagnostics; it
  retains or copies no PDF bytes, array bytes, child object bytes, page
  dictionaries, contents streams, or referenced-object bytes. Nested arrays,
  dictionaries, literal strings, hex strings, comments, names, numbers,
  booleans, nulls, and other direct scalar values are not descended into or
  interpreted as child references. Malformed direct reference-shaped candidates
  are reported as `SkippedPageTreeKidKind::MalformedIndirectReference` with the
  delegated `IndirectReferenceInspectionRejection` rather than disappearing or
  panicking. The only owned allocations are the public report vectors for direct
  kid references and skipped direct entries; no benchmark was added because this
  remains a bounded single-array scan rather than a broader traversal.
- Adds `inspect_page_tree_node_type`, a focused composition helper for
  caller-provided bytes and an already-located page-tree object byte offset. It
  delegates top-level entry discovery to `inspect_indirect_object_dictionary`,
  matches the single exact raw top-level key bytes `/Type` from each entry's
  `key_range`, validates the shallow `/Type` value kind as `name`, and
  classifies the value's exact raw bytes into a `PageTreeNodeType`: `Pages` for
  `/Pages` (intermediate node), `Page` for `/Page` (leaf page), or `Other` for
  any other name value. The report carries the delegated
  `IndirectObjectDictionaryInspection`, the `/Type` key and value byte ranges,
  and the classified node kind; it retains or copies no PDF bytes, object
  bodies, stream bodies, page dictionaries, contents streams, or `/Type` name
  bytes, and an `Other` value stays addressed by `type_value_range`. The
  classifier compares only exact raw bytes and decodes no PDF name escapes, so
  an escaped form such as `/Page#73` classifies as `Other`, not `Page`. It never
  resolves references, follows `/Kids`, `/Parent`, or `/Contents`, descends into
  child page-tree nodes or page dictionaries, or reads any key other than
  `/Type`. Structured public rejections distinguish a delegated object-dictionary
  failure (`ObjectDictionary`), missing `/Type` (`MissingType`), duplicate exact
  `/Type` (`DuplicateType`), and a non-name `/Type` value (`NonNameTypeValue`),
  mirroring the `inspect_page_tree_node` / `inspect_catalog_pages` rejection
  style. It lives in its own `page_tree_node_type.rs` module with a private
  module-local `find_unique_entry` helper rather than widening the existing
  private one. A composition test chains `inspect_classic_xref_table ->
  inspect_classic_xref_trailer_root -> resolve_classic_xref_object ->
  inspect_catalog_pages -> resolve_classic_xref_object ->
  inspect_page_tree_node_type` over synthetic bytes to classify a located
  page-tree root as `/Pages`.
- Adds `inspect_page_tree_reference_target`, a focused composition helper for
  caller-provided bytes, an existing `ClassicXrefTableInspection`, and one
  page-tree `IndirectRef`. It resolves the requested object number through
  `resolve_classic_xref_object`, accepts only a single in-use xref entry whose
  generation matches the requested reference, and delegates classification to
  `inspect_page_tree_node_type` at the resolved byte offset. The report carries
  the requested reference, resolved object byte offset, xref generation, and the
  delegated node-type inspection; it retains or copies no PDF bytes, object
  bodies, stream bodies, page dictionaries, contents streams, or referenced
  object bytes. Structured public rejections distinguish free/not-found/
  ambiguous xref outcomes (`UnresolvedXrefLocation`), generation mismatch, and
  delegated node-type inspection failure with the underlying node-type rejection
  reason preserved. The helper does not traverse `/Kids`, recurse into page-tree
  children, parse `/Count`, inspect page contents/resources/annotations, or add
  caches/indexes around classic xref lookup. A composition test chains trailer
  root, catalog `/Pages`, page-tree `/Kids`, and one selected kid-reference
  classification without implementing full page-tree traversal.
- Adds `inspect_page_contents`, a focused composition helper for
  caller-provided bytes and an already-located leaf page-object byte offset
  (typically the in-use xref target of a `/Page` kid resolved by
  `inspect_page_tree_reference_target`). It delegates page-object inspection to
  `inspect_indirect_object_dictionary`, matches only the exact raw top-level key
  bytes `/Contents` from each entry's `key_range`, and reports the direct
  content-stream indirect reference(s) a future page-content reader will
  resolve. A single-reference value (`N G R`, classified `IndirectReferenceLike`
  or `OtherScalar`) is parsed through `parse_indirect_reference` and reported as
  one content reference, requiring the parsed reference to consume the whole
  value span. An array value (`[ ... ]`) is bounded with `inspect_array_extent`
  and scanned for direct top-level `N G R` references in source order, reusing
  the same scan discipline as `inspect_page_tree_kids` and the shared
  `source_utils` skip helpers: nested arrays, dictionaries, literal/hex strings,
  comments, names, numbers, booleans, nulls, and other direct scalars become
  shallow `SkippedPageContentEntry` diagnostics instead of failing the whole
  inspection or being silently dropped, and malformed reference-shaped
  candidates carry the delegated `IndirectReferenceInspectionRejection`. The
  report carries the delegated `IndirectObjectDictionaryInspection`, the
  `/Contents` key/value byte ranges, a `PageContentsValueShape` marker
  (`single_reference` vs `array`), the ordered `PageContentReference`s with their
  `IndirectReferenceByteRange`s, and the array skip diagnostics; it retains or
  copies no PDF bytes, object bodies, array bytes, content-stream bodies, or
  referenced-object bytes, addressing references and ranges by offset only. The
  only owned allocations are the public report vectors for content references
  and skipped array entries; no benchmark was added because this remains one
  delegated object-dictionary inspection plus, for the array case, a single
  bounded array-extent scan over the `/Contents` array body rather than a
  whole-document traversal. Structured public rejections distinguish a delegated
  page-object dictionary failure (`PageDictionary`), missing `/Contents`
  (`MissingContents`), duplicate exact `/Contents` (`DuplicateContents`,
  matching the `inspect_catalog_pages` duplicate-key policy), a
  non-reference/non-array scalar value (`NonReferenceOrArrayContentsValue`), and
  a malformed single-reference value (`MalformedContentsReference`). A defensive
  `ContentsArrayExtent` channel mirrors `inspect_page_tree_node`'s
  `KidsArrayExtent`: because an array-classified value was already bounded by the
  delegated dictionary-entry scan, the re-bounding `inspect_array_extent` call
  cannot currently fail for a well-formed array value, so this variant is the
  dedicated reporting channel rather than a reachable failure. This slice does
  not resolve, fetch, decode, or concatenate content streams, parse
  `stream`/`endstream`/`/Length`, validate `/Type /Page`, inspect `/Resources`,
  page boxes, `/Annots`, or inherited attributes, recurse into `/Kids`, decode
  name escapes, or treat an absent `/Contents` as a valid empty page. It lives
  in its own `page_contents.rs` module and currently mirrors the
  `inspect_page_tree_kids` array scanner (whose helpers are module-private)
  rather than sharing one scanner, a duplication a later ablation could factor
  into a shared internal helper. A composition test chains
  `inspect_classic_xref_table -> inspect_classic_xref_trailer_root ->
  inspect_catalog_pages -> inspect_page_tree_reference_target ->
  inspect_page_tree_kids -> inspect_page_tree_reference_target ->
  inspect_page_contents` over synthetic bytes, reporting a resolved leaf page's
  single-reference contents and a second leaf's source-ordered array contents
  without copying PDF bytes.
- Adds `inspect_page_content_targets`, a locate-only composition helper for
  caller-provided bytes, an existing `ClassicXrefTableInspection`, and the
  direct `/Contents` references already reported by `inspect_page_contents`.
  The report carries the caller-visible source length and one source-ordered
  `PageContentTargetInspection` entry per direct content reference. Resolved
  entries carry the original `PageContentReference`, the matching in-use object
  byte offset, and the xref generation. Free, missing, ambiguous, and
  generation-mismatched xref outcomes become structured skipped entries instead
  of being silently dropped, and later references continue to resolve. The
  helper reuses `resolve_classic_xref_object` for each object number and mirrors
  `inspect_page_tree_reference_target`'s generation policy: only one in-use xref
  entry with the requested generation is accepted. It retains or copies no PDF
  bytes, object bodies, stream bodies, decoded streams, content bytes, or source
  slices; the only owned allocation is the public source-ordered report vector.
  Non-goals for this slice: it does not inspect content stream dictionaries,
  locate `stream`/`endstream`, parse `/Length`, decode or concatenate streams,
  tokenize content bytes, mutate PDF bytes, traverse `/Prev`, add xref stream or
  object stream support, build caches/indexes, or connect resolved contents to
  syntax or inventory crates. A composition test chains catalog `/Pages`, page
  tree `/Kids`, leaf page `/Contents`, and classic-xref content target
  resolution over synthetic bytes.
- Adds `inspect_content_stream_start`, a focused report-only composition helper
  for caller-provided bytes and a dictionary-bodied stream object byte offset
  (typically a `PageContentTargetInspection::Resolved` `object_byte_offset`). It
  delegates object-dictionary validation to
  `inspect_indirect_object_dictionary`, then from the reported
  `after_dictionary_close_byte_offset` skips optional PDF whitespace and `%`
  comments through the shared `skip_whitespace_and_comments`, requires the exact
  `stream` keyword via the shared `consume_keyword` boundary rule (so `streams`
  or `stream0` is rejected as malformed), and validates the PDF 32000 §7.3.8.1
  end-of-line rule immediately after the keyword: only a CRLF pair or a single
  LF is accepted, and a lone CR is rejected. The report carries the delegated
  `IndirectObjectDictionaryInspection` (which already exposes the parsed
  `IndirectRef` and the dictionary open/close/after offsets), the `stream`
  keyword start/after byte offsets, the accepted `StreamKeywordEol`
  (`line_feed` or `carriage_return_line_feed`, with a `byte_len()` of 1 or 2),
  and the stream-data start byte offset immediately after the EOL. It retains or
  copies no PDF bytes, object bodies, stream bodies, decoded streams, or source
  slices; offsets and ranges only. Structured public rejections distinguish a
  delegated object-dictionary failure (`ObjectDictionary`), a non-dictionary
  object body surfaced as a dedicated `NonDictionaryBody` carrying the classified
  `IndirectObjectBodyLeadingTokenKind`, a post-dictionary offset at or beyond EOF
  before any `stream` keyword (`OffsetOutOfBounds`), a missing or malformed
  `stream` keyword (`MissingStreamKeyword`), and an invalid post-`stream`
  end-of-line marker (`InvalidStreamEol` carrying a `StreamEolIssue` of
  `lone_carriage_return`, `end_of_file`, or `not_end_of_line`). This slice
  locates only the *start* of the stream body: it does not locate `endstream`,
  read/parse/resolve `/Length` (direct or indirect), compute the stream-data end
  offset, read/decode/decompress/tokenize stream bytes, validate `/Filter`,
  `/Type`, or `/DecodeParms`, connect streams to `presslint-syntax`/
  `presslint-inventory`, or mutate PDF bytes — each deliberately deferred. It is
  allocation-light: one delegated dictionary inspection plus a fixed-size
  post-`>>` whitespace/keyword/EOL check, with no copied byte buffers, source
  slices, caches, or object maps, so no benchmark was added. It lives in its own
  `object_stream.rs` module (sibling to `object_dictionary.rs`). A composition
  test chains `inspect_classic_xref_table -> inspect_classic_xref_trailer_root ->
  inspect_catalog_pages -> inspect_page_tree_reference_target ->
  inspect_page_tree_kids -> inspect_page_tree_reference_target ->
  inspect_page_contents -> inspect_page_content_targets ->
  inspect_content_stream_start` over synthetic bytes to reach a resolved content
  stream's data start offset without copying PDF bytes.
- Ablation T077: replaces the page-content-target report assembly loop with a
  direct source-ordered iterator collection over the already reported
  `/Contents` references. Runtime behavior, public API, serde shape, skip
  semantics, and allocation shape are unchanged.
- Ablation T075: removes duplicate page-tree-reference test fixture builders and
  reuses the existing shared test helpers for indirect references and synthetic
  classic xref inspections. Runtime behavior, public APIs, and coverage are
  unchanged.
- Ablation T073: removes an unreachable `OffsetOutOfBounds` remapping from the
  page-tree-kids scanner's successful `parse_indirect_reference` branch. The
  shared indirect-reference parser already bounds successful reports to the
  input slice, so a parsed reference that extends past the bounded `/Kids` array
  body remains the existing `MalformedReference` skipped-entry diagnostic.
- Promotes the shared `parse_u64_decimal` decimal parser into `source_utils` so
  the indirect-reference and object-header helpers reuse one bounded
  decimal-to-`u64` routine instead of duplicating it.
- Ablation: factors the identical `N G <keyword>` scan shared by
  `inspect_indirect_object_header` and `parse_indirect_reference` into a single
  internal `parse_object_reference_shape` helper in `source_utils` (parametrized
  by scan window and trailing keyword). Each public helper now only maps the
  shared `ObjectReferenceShape`/rejection into its own report and error types, so
  the bounded single-pass scan, whitespace/digit/keyword-boundary discipline, and
  numeric-range checks live in one place instead of two near-verbatim copies.
  Behavior, public types, field names, serde shapes, and error offsets are
  unchanged; this only removes duplication.
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
