# presslint-write Journal

## Current State

`presslint-write` is the first byte-writing crate: a deterministic classic-xref
and xref-stream **incremental append** writer. It offers the foundational
semantic **no-op** (`write_incremental_revision`), the first *semantic*
dictionary mutation built on it (`set_page_boxes_incremental`), the first
**whole-stream** mutation (a no-op content-stream re-encode,
`reencode_page_content_incremental`), the first content-operand rewrite
(`rewrite_rgb_black_to_cmyk_incremental`, an exact syntactic RGB-black rewrite),
and now the first REAL colour conversion of PDF content:
`convert_content_colors_incremental`, a DeviceLink-driven direct device-colour
conversion (F4-2), generalised to MULTI-LINK source-space routing (F4-5).

## T131 - Multi-Link Routing for DeviceLink Colour Conversion (F4-5)

`convert_content_colors_incremental` now carries a SET of DeviceLinks and routes
each direct device colour operator to the link whose SOURCE space equals the
operator's declared space. One call can carry, e.g., an RGB->CMYK link AND a
CMYK->CMYK link and convert both `rg` and `k` content correctly in a single
pass; content whose space matches no supplied link is left intact and honestly
reported.

### Multi-link request

`ConvertContentColorsRequest.device_link_bytes: Vec<u8>` is replaced by
`device_links: Vec<DeviceLinkInput>`, where
`DeviceLinkInput { id: Option<String>, bytes: Vec<u8> }`. The `id` is an OPAQUE
caller label echoed into the report only — this slice does NOT resolve names to
files or profiles (a later CLI concern). A single-link caller passes a
one-element vec and reproduces the exact F4-2..F4-4 behaviour.

### Routing map + duplicate-source rule (`link_routing.rs`)

`build_link_routing` inspects each link ONCE up front (before any page
traversal) and builds a deterministic `BTreeMap<DeviceColorSpace, RoutedLink>`
keyed by narrowed source space. Whole-op errors, all returned before a single
page is opened:

- `NoDeviceLinks` when `device_links` is empty.
- `DeviceLinkInspectFailed { index, id, error }` when a link's bytes are not a
  DeviceLink (carries the offending link index + label).
- `UnsupportedLinkSpace { index, id, source, destination }` when a link's source
  OR destination is Lab / unsupported.
- `AmbiguousLinkSource { space, first_index, second_index }` when two links
  declare the same source space — routing would be a silent guess, so it is a
  hard error, not a first-wins pick.

`RoutedLink` borrows the request's ICC bytes (`&'a [u8]`) for
`apply_device_link_f64`; the only owned copies are the small optional `id`
labels (one per link).

### Per-operator order (extends F4-2..F4-4)

Per operator: classify direct device op -> operand count/number/range
validation -> selector check (F4-4; `selector_excluded`) -> **route lookup** ->
black-preservation (per the routed link, only when its destination is CMYK) ->
apply the routed link, rewrite the operator to the routed link's destination
space (stroking preserved), splice. Splices descend by start offset; everything
else is byte-verbatim, so `output.bytes[..input.len()] == input`.

Note the reorder vs the single-link slice: operand validation and the selector
now run BEFORE the route lookup, so `no_matching_link` is reserved for
well-formed, selector-included operators whose space matches no link's source (a
genuine coverage gap). A malformed off-space operator is attributed to its
precise operand skip; an off-space operator excluded by the target selector is
attributed to `selector_excluded`. The exact source-space gate itself (an
operator converts iff its space equals SOME link's source) is unchanged.

### Report + `no_matching_link` rename

`ConvertedPage` keeps `operators_converted` (total across all links) +
`black_preserved`, and adds `links: Vec<LinkConversionCounts>` — one entry per
supplied link in request order, each carrying
`{ link_index, link_id, source, destination, operators_converted }` for that
page. `OperatorSkipCounts` renames `source_space_mismatch` -> `no_matching_link`
(the other four skip counts are unchanged). `ConvertedPage` is no longer `Copy`
(it owns the `links` vec).

### Copy budget

The routing table is built ONCE per call (N link inspections + one BTreeMap);
per-operator routing is a single map lookup. Link ICC bytes are borrowed from
the request, never cloned into the table. One owned decoded output per edited
page (unchanged from F4-2). The per-page `link_converted` tally is a
`Vec<usize>` sized to the link count; the per-link report clones only the small
`id` labels.

### Scope-ripple note

The request-shape change rippled into two test files outside this task's
declared write scope: `src/tests/selector_match.rs` (mechanical
`device_link_bytes` -> `device_links` wiring, plus the
`selector_precedes_route_lookup_for_offspace_operators` test re-expressed for
the new selector-before-route order) — a required, honest consequence of the
brief's explicit reorder, not a weakening.

## T128 - DeviceLink Content-Colour Conversion (F4-2)

`convert_content_colors_incremental(input, &ConvertContentColorsRequest { pages,
device_link_bytes }) -> Result<ConvertContentColorsOutput,
ConvertContentColorsError>` generalises the T126 hardcoded
`0 0 0 rg -> 0 0 0 1 k` rewrite into a DeviceLink-driven conversion. For each
direct device colour-setting operator in a selected single-content-stream page
whose declared space equals the supplied DeviceLink's SOURCE space, it reads the
operands, applies the DeviceLink via `presslint_color_lcms::apply_device_link_f64`
(F4-1), and rewrites the operator to the DeviceLink's DESTINATION space with the
converted operands. It is built on the untouched T125/T126
`content_edit_pipeline` (`edit_page_content_incremental` + `EditedContent`) and
the T126 operator matcher; `content_edit_pipeline.rs` was NOT modified.

### DeviceLink inspected ONCE up front

`presslint_color_lcms::inspect_device_link(&device_link_bytes)` runs before any
page traversal. An invalid / non-DeviceLink profile is `DeviceLinkInspectFailed
{ error }`; a `Lab` or `Unsupported` source OR destination space is
`UnsupportedLinkSpace { source, destination }` — both whole-op errors returned
before a single page is opened. The convertible source/destination spaces are
narrowed once to an internal `DeviceColorSpace` (`Gray`/`Rgb`/`Cmyk`), which
fixes the expected source operator set and channel count.

### Source-space gate + operator/operand-count change

Per page, an edit closure over the decoded content `tokenize`s +
`assemble_operators`, then for each `OperatorRecord`:

- operator bytes → `(space, stroking)`: `g`/`G`=Gray(1), `rg`/`RG`=RGB(3),
  `k`/`K`=CMYK(4); lowercase = nonstroking, uppercase = stroking. A non-colour
  operator is left verbatim and not counted.
- a colour operator whose space != link source space is left verbatim and counted
  `source_space_mismatch` (so a CMYK->CMYK link never touches `rg`, an RGB->CMYK
  link never touches `k`).
- a source-space operator with the wrong operand count → `wrong_operand_count`,
  skip; a non-number / multi-token operand → `non_number_operand`, skip; an
  operand outside `[0.0, 1.0]` → `operand_out_of_range`, skip.
- otherwise `apply_device_link_f64` produces the destination components; the
  operator becomes the destination-space operator (Gray→`g`/`G`, RGB→`rg`/`RG`,
  CMYK→`k`/`K`, preserving stroking), and the replacement is
  `<serialized components joined by single spaces> <dest operator>`. The operand
  count and operator therefore change with the destination space (e.g.
  `1 0 0 rg` via an RGB->CMYK link → `<c> <m> <y> <k> k`).

Splices are applied DESCENDING by source offset (T126 precedent) so earlier
ranges stay valid; every non-converted byte is preserved verbatim, and
`output.bytes[..input.len()] == input`. A page with zero conversions returns
`EditedContent::Unchanged` (no revision object), and works on classic AND
xref-stream, raw AND single `/FlateDecode` inputs (inheriting the pipeline's
skips for multi-stream `/Contents`, indirect/duplicate `/Length`, predictor
Flate, unsupported filter, compressed content object, and unproven ownership).

### Deterministic colour serialization (src/pdf_number_serialize.rs)

`serialize_color_component(f64) -> String` clamps to `[0.0, 1.0]`, maps `-0.0`
and negative / non-finite values to `0`, formats `{:.8}` (never an exponent),
then trims trailing zeros and a trailing `.`. This is F4-2's quantisation policy
(F4-1 stays raw `f64`). It is a deliberately separate helper from
`page_box_serialize::format_number`, which serializes arbitrary finite
coordinates by shortest round-trip and has no `[0,1]` clamp — the two policies do
not share an implementation, so page-box was left untouched.

### Honest reporting

`ConvertContentColorsOutput { bytes, converted, skipped }`. `ConvertedPage {
page_index, content_object, operators_converted, operator_skips }` where
`OperatorSkipCounts { source_space_mismatch, wrong_operand_count,
non_number_operand, operand_out_of_range }` aggregates per page. Every ANALYSED
page (a conversion happened, or zero conversions after operator inspection) is
reported under `converted`, carrying its per-page skip counts; only STRUCTURAL
skips (multi-stream, ownership, filter, length, round-trip) go to `skipped`.
Per-page tallies are captured in a `RefCell<Vec<PageTally>>` the edit closure
owns (one push per analysed page, in ascending selected-page order) and are
associated back to pages after the pipeline returns by ordering the analysed
pages (edited + `Unchanged`-skip) ascending and positionally zipping — exact
because the pipeline invokes the closure once per analysed page in that order.

### Bounds / copy budget

Per analysed page: one decoded `Vec<u8>` (Flate), one edited `Vec<u8>` only when
a conversion happens, one re-encoded `Vec<u8>`, plus small per-splice replacement
`Vec<u8>`s and the final append-writer output. The DeviceLink transform is built
per converted operator inside `apply_device_link_f64` (F4-1 semantics); a bounded
`TransformCacheKey` cache is a later slice. No source PDF bytes or whole-document
object cache are retained.

### Dependency edge + license gate

New first-party dependency `presslint-color-lcms` (the F4-1 Little CMS DeviceLink
executor); direction stays `write -> {actions, color-lcms, pdf, syntax, types}`,
no cycle. `presslint-color-lcms` pulls `lcms2-sys` (MIT, wrapping Little CMS,
MIT), pinned EXACT with `default-features = false, features = ["static"]` so the
LGPL / LLVM-exception transitive deps stay out of the graph. `check_licenses.sh`
still passes (53 third-party packages). Because `presslint-write` is
`#![forbid(unsafe_code)]`, its unit tests cannot run the lcms FFI that builds a
synthetic link, so the tests embed tiny frozen synthetic DeviceLink bytes
(grid-2 CLUT placeholders built once out of band through the same pinned
`lcms2-sys`) and drive them through the public `inspect`/`apply` API; no
ECI/FOGRA profile is vendored. Note: lcms `TYPE_CMYK_DBL` is the 0..100 domain,
so a CMYK-destination link's raw components can exceed 1.0 and are clamped to
`[0,1]` by the serializer — colour fidelity of CMYK sides is F4-1 / real-profile
territory, out of scope here.

### T125/T126 unchanged

`reencode_page_content_incremental` (T125) and
`rewrite_rgb_black_to_cmyk_incremental` (T126) keep their exact public API,
behaviour, and skip taxonomy; the shared `content_edit_pipeline.rs` is untouched.

## WholeStream executor + no-op content re-encode (T125)

### Plan bridge now executes `WholeStream`

`write_incremental_revision_plan` previously rejected
`MutationBoundary::WholeStream` as `UnsupportedBoundaryKind::WholeStream`. It now
executes it exactly like `DictionaryEntry`: `validate_boundary` shares a
`validate_in_place_target` helper that checks the boundary `target` equals the
dirty object's `reference` (else `BoundaryTargetMismatch`) and that ownership is
`InPlaceMutation` (else `OwnershipNotInPlace`). The action already carries the
full replacement `body_bytes`, so the bridge only validates intent and never
re-derives the payload. `ContentStreamOperand` and `IndirectObjectClone` remain
`UnsupportedBoundaryKind` (the `WholeStream` enum variant is kept as a frozen
public contract but is no longer emitted).

### `reencode_page_content_incremental` (src/reencode_content.rs)

`reencode_page_content_incremental(input, &ReencodePageContentRequest { pages })
-> Result<ReencodePageContentOutput, ReencodePageContentError>` re-encodes each
selected single-content-stream page's stream as a **semantic no-op** and appends
one incremental revision. `PageSelection` is `All` or `Indices(Vec<PageIndex>)`;
`ReencodePageContentOutput { bytes, reencoded, skipped }` with
`ReencodedPage { page_index, content_object, filter_kind }`.

Pipeline per selected page (locate -> decode -> re-serialize byte-identical ->
re-encode -> `WholeStream`-replace -> reopen):

1. Open with `inspect_document_access`; locate each page's single content stream
   through `inspect_document_page_content_extents_with_lookup` at the resolved
   page-tree-root offset (a private `lookup_from_backend` maps the backend to an
   `ObjectLookup`).
2. Prove single-use ownership: `decide_indirect_object_edit(content_object,
   owning_leaves)` where the owning leaves are every document page that references
   the content object. A shared content stream is `OwnershipNotInPlace` (skip).
3. Read the stream dict via `inspect_indirect_object_dictionary`; find the one
   direct integer `/Length` value span.
4. Classify `/Filter` via `classify_content_stream_filter` (raw or single
   `/FlateDecode`); resolve `/DecodeParms` via `resolve_flate_decode_parameters`
   and skip a predictor Flate stream.
5. Slice stream data with `content_stream_data_slice`; decode Flate via
   `decode_flate_stream`.
6. **Round-trip proof**: because `serialize_unmodified` is still an identity
   placeholder, the byte-identical proof uses the real lexical round trip
   `tokenize` + `serialize_tokens_unmodified` — content that does not tokenize
   (e.g. an unterminated string) or does not reconstruct exactly is a
   `ContentRoundTripMismatch` skip, so no page whose syntax does not round-trip is
   written.
7. Re-encode: raw uses the re-serialized bytes verbatim (equal to the original
   stream data -> **true byte no-op**); Flate re-compresses via
   `encode_flate_stream` (`decode(reopened) == decode(original)` -> semantic
   no-op, stream bytes may differ).
8. Rebuild the object body `<< dict-with-/Length-replaced >>\nstream\n<data>\n
   endstream` (LF separators), replacing exactly the one `/Length` value span with
   the new data length and preserving every other dictionary byte (incl.
   `/Filter`, `/DecodeParms`) verbatim.
9. Route one `WholeStream` boundary + rebuilt body per edited page through
   `write_incremental_revision_plan`. When every selected page is skipped the plan
   would be empty, so the no-op revision delegates straight to
   `write_incremental_revision` (also where an encrypted/hybrid input is
   rejected on the all-skipped path).

### Skip / error taxonomy

`ReencodePageSkipReason` (structured, one per unsupported shape):
`MultipleContentStreams`, `NoContentStream`, `CompressedContentObject`,
`IndirectLength`, `MissingOrDuplicateLength`, `NonDirectNumericLength`,
`UnsupportedFilter`, `PredictorFlate`, `ContentRoundTripMismatch`,
`OwnershipNotInPlace`. Whole-op `ReencodePageContentError`: `EmptyRequest`,
`Open`, `PageIndexOutOfRange`, `Write` (all-skipped delegate), `Plan` (plan
bridge). Requested indices are deduplicated and ordered so a repeated index never
produces two dirty objects for the same stream.

### Copy budget

Per edited page: one decoded `Vec<u8>` (Flate) plus one re-encoded/rebuilt-body
`Vec<u8>`, bounded by `MAX_CONTENT_STREAM_BYTES` (64 MiB). Plus the single output
`Vec<u8>` from the append writer (owned-bytes API) and the plan bridge's one copy
of each body into `DirtyObjectBytes`. No source PDF bytes are retained, and no
whole-document object cache is built — structural facts are read once through
`presslint-pdf`. The raw path is fully copy-free of decode/encode work: its new
data equals the original stream slice. New dependency: `presslint-syntax` (for
`tokenize`/`serialize_tokens_unmodified`); direction stays `write -> {actions,
pdf, syntax, types}`, no cycle.

### `set_page_boxes_incremental` unchanged

The dictionary-entry semantic writer keeps its exact public behavior; only the
shared `validate_in_place_target` refactor touches its plan-bridge path, and its
tests are unchanged.

## Plan-to-writer bridge (T120)

`write_incremental_revision_plan(input: &[u8], plan:
&presslint_actions::IncrementalRevisionPlan) -> Result<Vec<u8>,
PlannedWriteError>` (in `src/planned.rs`) is the validating bridge from the
backend-agnostic action plan contract to the byte writer.

### Validation order (all before any bytes are assembled)

1. Empty plan → `EmptyPlan`.
2. Sort dirty objects deterministically by `IndirectRef`, then reject duplicate
   object numbers → `DuplicateDirtyObject { object_number }`.
3. For each dirty object: no boundaries → `EmptyBoundaries { reference }`; then
   validate every boundary. This slice executes only
   `MutationBoundary::DictionaryEntry`: its `target` must equal the dirty
   object's `reference` (else `BoundaryTargetMismatch`) and its ownership
   disposition must be `InPlaceMutation` (else `OwnershipNotInPlace`).
   `ContentStreamOperand`, `WholeStream`, and `IndirectObjectClone` are rejected
   as `UnsupportedBoundaryKind { reference, kind }`.
4. Only after full-plan validation are dirty objects converted to
   `DirtyObjectBytes` and passed to `write_incremental_revision`; a delegated
   failure is wrapped verbatim as `PlannedWriteError::Write { error }`.

`PlannedWriteError` is serde-tagged (`stage`) and `UnsupportedBoundaryKind` is a
serde `snake_case` enum. Two focused tests use non-existent dirty object numbers
to prove `OwnershipNotInPlace` and `BoundaryTargetMismatch` reject *before*
delegation (the byte writer would otherwise report `DirtyObjectNotInUse`).

### Boundary contract stays writer-owned vs plan-owned

The plan carries dirty-object intent only. The writer keeps sole ownership of
classic-vs-stream dispatch, `/Prev`, `/Size`, `/Root`, `/ID`, `/Info`,
encryption/hybrid rejection, and object-currency/header validation — the bridge
only adds the plan-layer checks the byte writer cannot express (boundary kind,
boundary target agreement, in-place ownership, duplicate object numbers).

### Copy budget

The bridge copies each `PlannedDirtyObject::body_bytes` once into the
`DirtyObjectBytes` it hands to the writer — the API-boundary replacement-body
payload the low-level contract already requires. It copies no source PDF bytes
and holds only borrowed references (`Vec<&PlannedDirtyObject>`) while validating
and ordering. New first-party dependencies: `presslint-actions` (the plan
contract) and `presslint-types` (`ByteRange` for the boundary value locators).
The dependency direction is `write -> {actions, pdf, types}`; no cycle.

### Page-box reroute

`set_page_boxes_incremental` now builds an `IncrementalRevisionPlan` from its
already-proven leaf edits and routes through `write_incremental_revision_plan`.
Each edited leaf produces one `PlannedDirtyObject` whose boundaries come from
`presslint_actions::plan_set_page_box_boundaries` over the same normalized
rectangles and located value locators that drive the byte edits, so the planned
mutation intent and the rewritten body always agree. All existing page-box
behavior — request validation, rectangle normalization, crop containment, skip
taxonomy, body serialization, and document-order edited reports — is unchanged.
When every requested page is skipped the plan would be empty, which the bridge
rejects by contract, so that no-op case delegates straight to
`write_incremental_revision` (surfacing as `SetPageBoxesError::Write`); the
plan-routed case surfaces as the new `SetPageBoxesError::Plan`. Low-level
page-box serialization and the dictionary body splice were split into
`src/page_box_serialize.rs` to keep `page_boxes.rs` under the file-size gate.

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
- Executing non-dictionary boundaries through the plan bridge: this slice writes
  only `MutationBoundary::DictionaryEntry`; `ContentStreamOperand`,
  `WholeStream`, and `IndirectObjectClone` are rejected as unsupported execution
  shapes.

## T121 - Xref-Stream Incremental Append Backend

Added a second append backend for inputs whose final `startxref` points at a
cross-reference stream. `write_incremental_revision` now dispatches by the
input's newest xref section kind:

- classic table inputs continue through the existing classic table/trailer path;
- xref-stream inputs build the same-type xref-stream `/Prev` chain and append a
  raw, unfiltered `/Type /XRef` stream revision;
- hybrid classic trailers carrying `/XRefStm` remain rejected.

The xref-stream backend allocates a fresh indirect object for the appended xref
stream (`whole-chain max object number + 1`, generation 0) and includes a
self-entry pointing at the offset recorded before that object's own header. It
packs only type-1 entries, computes `/W` minimally as `[1 w1 w2]` with big-endian
fields, derives `/Index` from the dirty objects plus the new xref object as
ascending run-length subsections, and sets `/Length` to the exact packed byte
count. The stream dictionary order is deterministic: `/Type`, `/Size`, `/Index`,
`/W`, `/Root`, `/Prev`, `/Length`, optional `/ID`, optional `/Info`.

Dirty-object validation mirrors the classic gate but uses
`ObjectLookup::XrefStreamChain`: each dirty reference must resolve through
`resolve_xref_object_offset` to an uncompressed in-use object whose header
matches the requested reference. Type-2 compressed xref-stream entries now return
`CompressedDirtyObject`; reserved/future entry types return
`ReservedDirtyObject`. Encrypted xref-stream dictionaries are rejected with the
same `EncryptedInput` variant as classic inputs.

Tests cover verbatim prefix preservation, reopen through
`inspect_document_access` selecting `XrefStreamChain`, self-reference
correctness, newest-wins dirty-object resolution, `/Prev` and `startxref`
offsets, whole-chain `/Size` including the newly allocated xref object, `/Index`
and `/W` bytes, second append chain growth, plan-bridge routing, and
`set_page_boxes_incremental` end-to-end on an xref-stream input. A classic
dispatch regression compares the public dispatch output with the classic backend
bytes.

Deferred deliberately: Flate-compressed xref stream output, hybrid `/XRefStm`
write/merge support, type-2 compressed-object mutation, object-stream writing,
clone/private-copy routing, deletion/free-list repair, and encryption
preservation.

## T126 - Direct RGB-Black Content Operator Rewrite

Added the first semantic content-operand mutation on top of the T125
WholeStream substrate: `rewrite_rgb_black_to_cmyk_incremental`. The operation is
an exact page-content operator rewrite only, not color conversion and not a
visual/colorimetric equivalence claim. It rewrites direct DeviceRGB black color
operators in eligible page content streams:

- `0 0 0 rg`-class records become canonical `0 0 0 1 k`;
- `0 0 0 RG`-class records become canonical `0 0 0 1 K`;
- all other decoded bytes outside the spliced operator records are preserved
  verbatim.

The T125 re-encode mechanics now live in a shared crate-private edit pipeline:
locate selected single-stream page content, prove single-use in-place ownership,
read the stream dictionary and direct `/Length`, decode raw or single
`/FlateDecode` streams without predictors, prove byte-identical
`tokenize` + `serialize_tokens_unmodified` round-trip on the original decoded
bytes, call an edit callback, re-check the edited decoded bytes, re-encode, build
the stream object body with only `/Length` changed, and execute one
`MutationBoundary::WholeStream` plan. `reencode_page_content_incremental` is now
a thin identity-edit wrapper over that pipeline; its public skip enum remains a
separate T125-owned type and is mapped from the internal skip enum rather than
aliased.

The RGB-black matcher uses `presslint_syntax::tokenize` and
`assemble_operators`. A match requires operator token source bytes exactly
`rg` or `RG`, exactly three operands, and each operand to be exactly one
`TokenKind::Number` token whose source bytes parse as a finite `f64` equal to
`0.0` (`0`, `0.0`, `.0`, `+0`, `-0`, `00.00`, etc.). Matching records are
collected in source order, then applied in descending source offset to an owned
decoded buffer so earlier byte ranges are not invalidated by later replacements.
A page with no matching operators is reported as
`NoMatchingOperators` and its content stream is not rewritten.

The inherited skip taxonomy is preserved for unsupported shapes: multi-stream
`/Contents`, missing/no stream, compressed object-stream members, indirect or
non-direct `/Length`, missing/duplicate `/Length`, unsupported filter/filter
chain, predictor Flate, content round-trip mismatch, and unproven ownership.
The color rewrite has its own public skip enum and adds only
`NoMatchingOperators`.

Bounds and copy budget: per edited page the hot path holds one decoded
`Vec<u8>`, a transient serializer `Vec<u8>` for each byte-identical round-trip
check, one edited decoded `Vec<u8>` only when a match exists, and one encoded
`Vec<u8>` for the replacement stream data, plus the final append-writer output.
The matcher does not retain source bytes or build a whole-document cache; it
keeps only token/operator records and small splice ranges for the current stream.
The stream-object-body builder is split into `stream_object_body.rs`, keeping the
shared pipeline comfortably below the 800-line file-size gate.

## T129 - Black-Preservation Overlay for DeviceLink Conversion

Added an opt-in `BlackPreservationPolicy` to
`convert_content_colors_incremental`. The default policy is `None`, preserving
the F4-2 DeviceLink behavior byte-for-byte: matching source-space operators are
validated and then converted through `presslint-color-lcms`.

With `NeutralBlackToK`, matching source-space operators are processed in this
order: source-space gate, operand count / number / range validation, exact
neutral-black preservation, then DeviceLink conversion for everything else. The
overlay fires only when the DeviceLink destination is CMYK. Exact neutral black
means RGB `[0, 0, 0]`, Gray `[0]`, or CMYK `[0, 0, 0, 1]` after the existing
single-number operand parse; there is no tolerance and no neutral-gray mapping.
Preserved operators serialize canonically as `0 0 0 1 k` or `0 0 0 1 K`.

`ConvertedPage` now reports `black_preserved` separately from
`operators_converted`. The latter remains the count of operators actually sent
through the DeviceLink. A canonical CMYK K-only operator under a CMYK-to-CMYK
link is counted as preserved, but if the canonical replacement is byte-identical
to the original operator record no splice is recorded, so a page with only those
passthroughs produces no revision object.

Bounds and performance: the overlay adds only a small fixed operand comparison
before the heavier DeviceLink call and can skip that call for preserved black.
Memory shape is unchanged from F4-2: one decoded stream, token/operator records,
small replacement buffers only for real splices, and the normal re-encoded
stream / append-writer output when a page is actually dirtied.

## T130 - Selector-Targeted DeviceLink Colour Conversion (F4-4)

Added an optional `target: Option<Selector>` to `ConvertContentColorsRequest`
(`#[serde(default, skip_serializing_if = "Option::is_none")]`), so a caller can
narrow WHICH matching-source colour operators are converted, e.g. "only DeviceRGB
fills", "odd pages", "pure-red operands". `None` (default) is F4-2/F4-3 behaviour
byte-for-byte: every matching-source operator is converted. The struct dropped
its `Eq` derive (keeps `PartialEq`) because a `Selector` may carry `f64` colour
components.

### Operator-local evaluator (src/selector_match.rs)

The selector is evaluated PER COLOUR OPERATOR against a synthetic borrowed
`OperatorView { page_index, color_space: DeviceColorSpace, usage: ColorUsage,
components: &[f64] }` — page index, the operator's declared device space,
Fill (lowercase `g`/`rg`/`k`) vs Stroke (uppercase `G`/`RG`/`K`), and the
already-parsed operand components. `selector_matches` walks the boolean tree
directly (`All`=true, `None`=false, `Not`, `And`=all, `Or`=any, `Predicate`=leaf).
It builds NO `InventoryEntry`, tracks NO graphics state, and does NOT call
`presslint_selectors::matches`; the private page-match and component-match
semantics of the selector crate are reimplemented locally (parity on the
one-based page number, exact/tolerant component compare) so the evaluator stays a
cheap per-operator boolean with no inventory dependency.

### Supported vs rejected leaves (up-front rejection)

BEFORE any page traversal, `collect_unsupported_leaves` walks the selector tree;
any unsupported leaf makes the whole call fail with
`ConvertContentColorsError::UnsupportedTargetSelector { unsupported:
Vec<UnsupportedTargetLeaf> }` — never a silent non-match, because silently
under-converting is bad prepress behaviour. SUPPORTED (operator-local) leaves:
`ColorSpace` for Device Gray/RGB/CMYK only, `Page` + `PageMatch`
(parity/range/set/exact), `ColorUsage` for Fill/Stroke only, and `ColorComponents`
over the operand components (device space + usage None/Fill/Stroke). REJECTED
leaves (need graphics-state association, a later slice): `ObjectKind`, `Editable`,
`Scope`, a `ColorUsage`/`ColorComponents` usage of Image/Shading, and a
`ColorSpace`/`ColorComponents` over a non-device space (ICCBased, Lab, spot,
resource, ...).

### Ordering + report

Per operator the order EXTENDS F4-2/F4-3: source-space gate → operand validation
(count/number/range) → **selector check (only when `target` is `Some`)** →
black-preservation → DeviceLink apply. A valid source-space operator the selector
does NOT match is left byte-verbatim and counted as the new
`OperatorSkipCounts::selector_excluded`. Note the ordering means an off-source
operator (e.g. `g`/`k` under an RGB link) is counted `source_space_mismatch` and
never reaches the selector check.

### Page-aware pipeline (src/content_edit_pipeline.rs)

Page predicates need the page index per operator, so the internal core became
`edit_page_content_incremental_indexed(input, pages, Fn(PageIndex, &[u8]) ->
EditedContent)`. `edit_page_content_incremental` is kept as a thin
index-ignoring wrapper, so `reencode_content.rs` (T125) and
`content_color_rewrite.rs` (T126) are byte-for-byte UNCHANGED; only the convert
action switched to the page-aware entry. All decode / round-trip / re-encode /
write mechanics are shared and identical between the two entries.

### Dependency edge + license gate

New first-party dependency `presslint-selectors` (pulls `presslint-inventory`
transitively — acceptable; only the `Selector`/`Predicate` data model is used,
never the inventory matcher). Direction stays acyclic:
`write -> {actions, color-lcms, pdf, selectors, syntax, types}`.
`check_licenses.sh` still passes (53 third-party packages, unchanged — both new
crates are first-party).

### Bounds / performance

The selector check is a cheap per-operator boolean eval BEFORE the heavier
DeviceLink apply, with no inventory build and no allocation beyond the small
`OperatorView` (which borrows the already-parsed operand slice). Up-front leaf
rejection is a single pre-order walk of the selector tree. Memory shape is
otherwise unchanged from F4-2/F4-3.

### Bounds / unchanged

Every changed public source file stays < 800 lines (content_color_convert.rs 641,
content_edit_pipeline.rs 600, selector_match.rs 222; the selector integration
tests live in the tests/selector_match.rs module to keep tests/content_color_convert.rs
at 729). T125/T126/T128/T129 public behaviour and the `target = None` path are
unchanged; the pipeline wrapper keeps the old callers intact.
