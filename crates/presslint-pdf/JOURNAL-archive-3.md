# presslint-pdf Journal Archive 3

Newer active history lives in [JOURNAL.md](JOURNAL.md). Older accumulated
history lives in [JOURNAL-archive-2.md](JOURNAL-archive-2.md).

## Archived Entries

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

### T166 - Indexed Colour-Space Structural Classification

- The colour-space classifier now models shallow `[/Indexed base hival lookup]`
  (and the `/I` alias) definitions instead of skipping them as
  `UnsupportedIndexedColor`. A classified Indexed fact carries family
  `Indexed`, `component_count = Some(1)` (one index paint operand), an optional
  shallowly classified `base_space` family, the direct non-negative integer
  `indexed_hival`, and a descriptor-only `indexed_lookup` shape (hex-string
  decoded byte length by digit counting, literal string, indirect reference, or
  unknown). No palette expansion, lookup decoding/retention, or profile parsing
  happens (ISO 32000-1 §8.6.6.3 shapes; length-vs-base validation deliberately
  deferred).
- `ClassifiedColorSpaceDefinition`/`ClassifiedColorSpaceResource` gained the
  three fields additively with `serde(default, skip_serializing_if)`, so
  existing JSON shapes are unchanged for non-Indexed families and old JSON
  deserializes. `ColorSpaceFamily::Indexed` is appended after `DeviceN`. The
  new `IndexedLookupDescriptor` enum lives in `page_color_space_resources` and
  is root-re-exported from `lib.rs` alongside the other colour-space report
  types (review fix: the public `indexed_lookup` field would otherwise carry an
  unnameable type downstream).
- Review fix: `hival` accepts an optional leading `+` sign; ISO 32000-1 §7.3.3
  integer syntax permits a sign, so `+15` is a valid non-negative integer token.
  Negative-signed tokens stay `MalformedColorSpaceOperand`; both sides are
  locked by form-resource regressions.
- Malformed Indexed shapes stay structured skips: fewer than four array
  elements or a non-integer/non-direct `hival` are
  `MalformedColorSpaceOperand`; an unresolved indirect base is the existing
  `UnresolvedResourceReference` skip. An unmodeled (Pattern/Lab/Cal) base
  leaves `base_space = None` while the Indexed fact still classifies.
  `UnsupportedIndexedColor` is retained for report compatibility but no longer
  emitted. Pattern/Lab/Cal top-level spaces and image `ColorSpace` handling are
  unchanged.
