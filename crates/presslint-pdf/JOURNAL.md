# presslint-pdf Journal

Older accumulated journal history lives in [JOURNAL-archive-4.md](JOURNAL-archive-4.md).

## Current State

### T181 - Inherited page-Font resource descriptor (read-only, no consumers)

- Three new modules mirror the ExtGState resource inspectors: `font_classify`
  (taxonomy + per-entry classifier), `page_font_resources` (page-tree walk
  with whole-dictionary `/Resources` inheritance producing per-page reports
  keyed by the identity triple ordinal/page_reference/page_object_byte_offset),
  and `form_font_resources` (own-scope form entry point via
  `ResourceContext::from_dictionary(..., None)`, no page inheritance). All
  reuse the existing walk (cycle guard, MAX depth/visited bounds),
  `ResourceContext`, `resolve_reference`, and `unique_entry`; the ONE new
  abstraction is the classified page-font report family.
- NAMING NOTE: this is a descriptor OF a page's `/Font` resource entries. It
  never resolves or inspects the PDF `/FontDescriptor` key, descendant fonts,
  encodings, CMap/Widths/ToUnicode data, CharProcs, or font program streams.
- `ClassifiedFontResource { name, dictionary_type, subtype, reference,
  object_byte_offset }`. `name` keeps RAW bytes without the leading slash and
  without `#xx` decoding (write-policy territory). `reference`/
  `object_byte_offset` carry the resolved indirect target when the entry was a
  reference (bind by resolved object, never by name).
- `FontSubtypeClass` (serde `tag = "kind"`, snake_case): the five legal `Tf`
  subtypes `Type1`/`MmType1`/`TrueType`/`Type0`/`Type3` as distinct variants
  (`/MMType1` is never collapsed into `Type1`), distinct `CidFontType0`/
  `CidFontType2` variants documented as invalid direct `Tf` operands (ISO
  32000-1 §9.7.4.1, never mapped to `Type0`), and fail-closed
  `OtherName { name }` (e.g. `/Type1C`), `Missing`, `Duplicate { ranges }`,
  `NonName { value_kind }`. Exact byte equality on a DIRECT name only; an
  indirect `/Subtype` is `NonName` and is never resolved.
  `FontDictionaryTypeFact` records the exact `/Type` fact (`Font`, `Missing`,
  `OtherName`, `Duplicate`, `NonName`) without guessing and without affecting
  the subtype class.
- Split rule: failures reaching a font dictionary use the 11-variant skip
  vocabulary mirroring ExtGState (`Resources` delegation,
  `MissingFontResources`, `MissingFont`, `DuplicateFont`, `NonDictionaryFont`,
  `FontDictionaryFailed`, `DuplicateFontName`, `NonDictionaryEntry`,
  `MalformedResourceReference`, `UnresolvedResourceReference`,
  `ResourceDictionaryFailed`). A REACHED dictionary with bad/absent `/Type` or
  `/Subtype` stays in `fonts` with explicit fail-closed classes.
- Inheritance is whole-dictionary nearest-ancestor replacement (Table 30,
  §7.7.3.4): a page owning `/Resources` without `/Font` is `MissingFont`, an
  empty `/Resources << >>` or `/Font << >>` is present-not-omitted, only an
  absent `/Resources` key inherits. Identity-triple parity with the ExtGState
  inspector is pinned by test.
- Read-only descriptor contract: NO consumers, NO behaviour change —
  presslint-write, presslint-paint, the umbrella, and the CLI have zero diff;
  conversion output is byte-identical. The descriptor claims no safety or
  admissibility judgement; the safe/unsafe mapping is deferred write-side
  policy. Reports retain only copied name bytes and byte ranges/offsets — no
  dictionaries, object bodies, streams, or font-program bytes; no stream
  decode. One bounded page-tree walk per document call, same cost profile as
  the ExtGState inspector.
- Concepts cross-checked (never copied, no GPL/AGPL read): PDFBOX-5054 (no
  d0/d1 or CharProcs fact may upgrade a classification; renderers disagree on
  colour ops after d1), PDFBOX-1238 (real files reference font resources that
  the effective `/Font` dictionary does not define; absence stays a structured
  `MissingFont`/`MissingFontResources` fact, never a substituted fallback),
  pdf.js #19634 (Type3 resource lookup is multi-tier and renderer-divergent;
  this slice records the shallow `Type3` fact and models no CharProcs or
  Type3 resource lookup), pdf-issues #368 (text-state/graphics-state
  restoration semantics belong to the later paint `Tf` modelling slice, not
  to this structural descriptor), ISO 32000-1 §9.9 (`/Type1C` names an
  embedded font-program stream subtype under `/FontFile3`, never a legal font
  dictionary `/Subtype`, so it and every other unknown name stay `OtherName`,
  never collapsed), qpdf 10.1.0 lesson (bind by resolved object identity, not
  by resource name).

### T180 - Image XObject stencil-mask metadata

- `ImageXObjectMetadata` now carries additive `ImageMaskMetadata` with exact
  `Missing`, `False`, `True`, `Duplicate`, and `Unsupported` structural facts.
  `Missing` is the default and is omitted from serde output, so the earlier
  four-field JSON shape serializes unchanged and deserializes with the absent
  fact; explicit false remains distinct from absence.
- The shallow image dictionary pass recognizes only direct boolean
  `/ImageMask` values. Non-boolean scalars, containers, and indirect references
  remain explicit unsupported shapes, while duplicate keys retain their byte
  ranges. No sample, filter, stream, mask, decode-array, or profile data is
  decoded or retained.
- Page- and Form-scope XObject target reports carry the same generic image
  metadata. This is descriptor propagation only: it does not admit Form
  descent or attach image metadata to Form targets.

### T173 - Bounded ICCBased Profile Header Descriptor Facts (T168b)

- New `icc_profile` module. The ONE new abstraction is
  `IccProfileHeaderDescriptor`: byte-level facts from the fixed 128-byte ICC
  header (ICC.1:2022, big-endian) — `decoded_len`, `declared_profile_size`
  (bytes `0..4`), `version_raw` + decoded `version_major`/`version_minor`/
  `version_bugfix` (BCD of bytes `8..12`), `profile_class_signature`
  (`12..16`), `data_color_space_signature` (`16..20`), `pcs_signature`
  (`20..24`), and `acsp_present` (bytes `36..40` == `acsp`). Raw four-byte
  signatures are always preserved next to any decoded convenience field, so a
  signature with spaces or unknown bytes is a fact, not an error. No lcms/skcms,
  no tag-table parsing, no colorimetry.
- `parse_icc_profile_header(decoded) -> IccProfileHeaderParse` reads only the
  128-byte header. Its sole rejection is `Truncated { decoded_len }` (< 128
  bytes); a corrupt `acsp`, an anomalous version, and unknown signatures all
  yield a populated `Parsed { descriptor }`. `data_space_component_count()` is
  the conservative ICC mapping `GRAY`=1, `RGB `/`CMY `=3, `CMYK`=4,
  `2CLR`..`FCLR`=2..15, everything else `None`.
- `inspect_icc_profile_stream_with_lookup(input, lookup, reference,
  output_limit) -> IccProfileStreamInspection` is pure composition of existing
  seams: `resolve_xref_object_offset` (stream objects cannot be object-stream
  members, so the offset-only resolver suffices) → extent → slice → filter
  classify → single `/FlateDecode` or identity decode → header parse. The
  decoded buffer is dropped after facts are extracted. Result taxonomy:
  `Parsed { descriptor }`, `Truncated { decoded_len }`, or `Gap { reason }`.
- `IccProfileInspectionGap` (unit variants, plain snake_case strings):
  `ProfileObjectCompressed`, `ProfileObjectUnresolved`, `StreamExtent`,
  `StreamSlice`, `FilterClassification`, `UnsupportedFilter`,
  `DecodeParmsDeclared`, `UnsupportedDecodeParms`, `DecodeParmsMalformed`,
  `DecodeOutputLimitExceeded`, `FlateDecodeFailed`. `/DecodeParms` PITFALL: a
  declared `/DecodeParms` key (even `null`) is a gap, never a silent
  default-parameter decode that could produce a plausible-but-wrong header.
- Read-only and bounded: no CMM, no tag-table access, no writer/converter
  impact. Every malformed/uninspectable outcome is a structured value, never a
  panic.

### T170 - Document-Wide Object Consumer Index (Read-Only Snapshot)

- New `object_consumer_index` module (facade + `traversal` submodule):
  `inspect_object_consumer_index(input, &DocumentAccess)` builds the
  document-wide reverse-reference map as a deterministic SNAPSHOT report: sorted
  `ObjectConsumersEntry { target, referrers }` entries, unresolved-edge
  facts, structured skips, truncation facts, cache facts, and an
  `unreferenced` diff of in-use xref entries no user ever reached.
- Referrer taxonomy (serde `tag = "referrer"`, snake_case): `TrailerKey`
  (one per NEWEST-trailer key except `/Root`; chains re-inspect their
  newest section, xref-stream backends read the stream object's own
  dictionary), `Root` (the catalog object itself, registration only),
  `RootKey` (one per catalog key incl. `/Pages`), `Page { page_index,
  page }`. The report retains key NAME bytes only. No `/Thumb` or
  `/Annots` machinery: page-subtree and generic edges cover them.
- Traversal is iterative with a PER-USER visited set (`BTreeSet<u32>`),
  never global — a global set would classify shared subtrees as
  single-use (unsafe in-place mutation later). Page rules are membership
  checks, never `/Type` sniffing: the top-level `/Parent` edge of
  page/page-tree-node dictionaries is skipped; an edge to a known page
  other than the user's own page is registered but not expanded, which
  makes `RootKey(/Pages)` own tree intermediates while stopping at every
  page dict. The page match compares the FULL leaf reference (number AND
  generation), so a generation-mismatched reference to a page's object
  number resolves like any other edge and lands as an unresolved-edge
  fact, never a consumer edge (regression-tested). Page dicts are therefore INHERENTLY multi-user
  (`RootKey(/Pages)` + own `Page` user) — pinned by test as the safety
  property. Reference extraction reuses the T169 scanner, so
  stream-parameter edges (indirect `/Length` etc.) are consumer edges
  and stream data is never scanned.
- Object streams are transparent: member references are MEMBER edges;
  the container is never a consumer and shows up only via direct
  referrers or the unreferenced diff. A per-container once-decode cache
  (64 MiB budget) re-derives `/First`/`/N` + header pairs defensively
  after the first canonical `resolve_object`; later members slice the
  cached buffer, any mismatch falls back to the canonical (re-decoding)
  path so the error taxonomy stays identical. Budget overflow drops the
  whole cache (slower but correct) and is a report fact.
- Dangling/free/generation-mismatch/reserved/classic-ambiguous edges are
  `ObjectConsumerUnresolvedEdge` facts wrapping the existing
  `ObjectResolutionRejection` (ISO 32000-1 7.3.10 null semantics), never
  consumer edges, never fatal. Bounds (per-user depth 64, per-user
  visited 65_536, global expanded 1_048_576, recorded pairs 1_000_000,
  8 MiB per-container decode) each surface as structured truncation
  facts; the module docs PIN that a truncated index must never feed
  ownership decisions.
- Review fix: a compressed member whose `/ObjStm` container exceeds the
  existing 8 MiB decoded-byte cap now records
  `MaxDecodedObjectStreamBytes { decoded_length, max_decoded_object_stream_bytes }`
  in `truncations` in addition to the unresolved-edge fact, so skipped member
  outgoing edges cannot be mistaken for a complete ownership proof.
- `DocumentAccessBackend::object_lookup()` is now the public
  backend-to-lookup projection (presslint-write still carries a private
  duplicate; migrating it is a later slice). Performance: cost is
  O(sum of per-user reachable subtrees) — an accepted industry trade-off;
  shared-subtree closure memoization is a recorded future optimization.
  The only owned buffers are the bounded cached decoded object streams.
- Deferred to the write-integration slice: referrer→consumer mapping for
  `decide_indirect_object_edit` (`TrailerKey` has no owning object),
  `content_object_owners` absorption, snapshot-invalidation contract.
- Final-review hardening: the per-user visited set is keyed by FULL
  `IndirectRef`, not object number — a generation-mismatched edge sharing a
  visited object's number (e.g. `3 1 R` inside page `3 0 R`) must reach
  resolution and surface as an unresolved-edge fact instead of being
  silently suppressed; pinned by a same-number mismatch regression.

### T169 - Object-Body Indirect-Reference Scanner

- New `object_body_references` module with three read-only entry points:
  `inspect_object_body_references(input, object_byte_offset)` validates the
  object header, classifies the leading body token, and scans the matching
  extent; `inspect_object_body_references_resolved(input, resolved)` delegates
  uncompressed data to the offset path and scans a compressed member's
  `object_body_span` inside the decoded buffer; and
  `scan_indirect_references_in_span(buffer, range)` is the reusable span
  primitive for future callers (the consumer-index slice).
- The scan is LINEAR, not recursive: a sliding window of the last two unsigned
  digit-only integer tokens plus a boundary-checked bare `R` keyword finds
  every `N G R` reference at any nesting depth in one bounded pass. Names,
  delimiters, signed numbers, reals, and keywords reset the window; literal
  strings, hex strings, and comments are skipped via the existing
  `source_utils` skippers and never produce references; a comment between
  tokens counts as whitespace (ISO 32000-1 7.2.4), so it cannot hide a real
  reference. The skippers run over a slice truncated at the span end, so the
  pass is span-bounded end to end: a comment or string that terminates only
  past the span end never causes a read beyond it (sibling object-stream
  members and suffix bytes stay untouched).
- Stream data is never scanned by construction: dictionary-led bodies bound
  the scan to the balanced dictionary extent, array-led bodies to the array
  extent, and a number-like scalar body gets the bounded three-token
  reference-shape check through `parse_indirect_reference` (so a `2 0 R`
  body is captured, not dropped). Other scalar bodies yield empty reports.
- `ObjectBodyReferencesInspection` carries `references: Vec<IndirectRef>` in
  source order without dedup, structured `SkippedObjectBodyReference` markers
  for `u32`/`u16` overflow shapes, and a structured
  `ObjectBodyReferencesTruncation::MaxReferences` fact when the per-body
  65_536-reference cap stops the scan. No byte ranges in v1 (compressed spans
  address a droppable decoded buffer) and no PDF bytes are retained.
- Copy budget: one pass per body over an extent-bounded borrowed slice; the
  only allocations are the output vectors. No decoding is performed; the
  resolved path consumes the caller-owned decoded buffer.
- Deferred to the consumer-index slice: referrer taxonomy and ownership
  semantics, objstm once-decode caching, dangling/free/generation validation,
  and byte ranges for the uncompressed path.


### T168 - ICCBased Descriptor Facts

- `ClassifiedColorSpaceDefinition` and `ClassifiedColorSpaceResource` now carry
  additive ICCBased facts: profile stream ref, direct `/Range` arity, and
  tri-state `/Alternate` presence.
- The classifier remains decode-free: no stream bytes, ICC headers, `/Range`
  values, or profile payloads are read or retained.
- Serde compatibility is additive: all three fields default and are omitted when
  absent, preserving older JSON shapes.


### T161a - Colour Environment Descriptor Facts

- Added companion, read-only default colour-space inspectors for page and form
  scopes. Page facts use inherited effective `/Resources`; form facts read only
  the form object's own `/Resources`. Defaults are reported separately from
  selectable `/ColorSpace` names.
- Added a catalog `/OutputIntents` observer that records supported `/S`,
  simple ASCII `/OutputConditionIdentifier`, and structural
  `/DestOutputProfile` presence/reference without reading profile streams.
- Refactored the colour-space classifier to expose a name-free
  `ClassifiedColorSpaceDefinition` shared by named resources and defaults.

### T160 - Transparency Group Inspectors

- Added shallow page/form `/Group` classifiers for Phase-1 safety: recognise
  `/S /Transparency`, record `/CS` as name/array shape, classify `/I` and `/K`
  booleans when present, and preserve absence without inventing defaults.
- Page inspection walks leaf pages in document order but reads `/Group` only
  from each page dictionary; it is not inherited and is not a resource entry.
  Form inspection reads only the form stream dictionary.
- Malformed, unresolved, non-dictionary, and non-transparency group shapes are
  structured diagnostics. The inspectors retain only small classified values and
  byte-range diagnostics, not stream bodies or compositing state.

### T159 - ExtGState Safety Predicates

- Added small predicate helpers on `ClassifiedExtGStateResource` for the Phase-1
  safety decisions: active overprint, active transparency, and malformed/unknown
  safety parameters. These helpers preserve the classifier's written-value model:
  `op` is not defaulted from `OP`, while `OP` true alone is overprint-active.
- Harmless unclassified dictionary keys such as `/LW` remain report metadata
  only; they do not make the safety predicate unknown when all seven safety
  parameters are unset or default.
- `/BM /Compatible` is classified as normal-equivalent while preserving the raw
  name, so the precision guard does not skip a page for that safe blend mode.
- The helpers are additive and read-only. The existing audit umbrella derivation
  still has not been refactored onto these predicates; that remains a follow-up.

### T155 - ExtGState Resource Inspector (Phase 1-1)

- Added read-only page and form `/Resources /ExtGState` inspectors. Page scope
  walks the page tree with the existing inherited `ResourceContext`; form scope
  uses `ResourceContext::from_dictionary(..., None)` and reads only the form's
  own resources.
- Added a pdf-crate-local ExtGState classifier/report vocabulary for the Phase-1
  safety keys only: `OP`, `op`, `OPM`, `CA`, `ca`, `BM`, and `SMask`, plus
  `has_unclassified_keys` for any other dictionary key except `/Type`.
- The classifier records per-parameter `Unset`/`Set`/`Malformed` states and does
  not resolve runtime defaults such as `op` inheriting `OP`. Unresolved indirect
  entries, duplicate names, missing/duplicate `/ExtGState`, non-dictionary
  containers, non-dictionary entries, and wrong-typed parameter values are
  reported structurally.
- Scope remains read-only and consumer-free: paint, inventory, umbrella, write,
  and CLI paths are untouched. Reports retain only small classified values and
  byte-range diagnostics, not PDF bodies or streams.

### T138 - Form-Scope Resource Colour-Space Inspector (F4-6 slice 1b)

- New read-only inspector `inspect_form_color_space_resources(input, lookup,
  object_byte_offset) -> FormColorSpaceResourcesInspection { object_byte_offset,
  color_spaces, skipped }` classifies ONE form stream object's OWN
  `/Resources /ColorSpace` into the same `ClassifiedColorSpaceResource` /
  `SkippedColorSpaceResource` model as the page pass. It mirrors
  `inspect_form_xobject_resources` (single object,
  `ResourceContext::from_dictionary(..., None)`, never a hard error).
- NO PAGE INHERITANCE (ISO 32000-1 §7.8.3 + §8.10.2 Table 95): a form paints
  against its OWN `/Resources` only. A form that omits `/ColorSpace` (or omits
  `/Resources`) gets an EMPTY colour-space environment — the honest
  prepress-audit behaviour, so its `cs CS0` stays an unresolved `Resource(CS0)`
  downstream. The obsolete PDF 1.1 page→form resource fallback is deliberately
  NOT implemented.
- REUSE, no classifier fork: the per-entry classifier
  `classify_color_space_entry` (name / array / indirect definition → family,
  shallow `/ICCBased /N`, `/Alternate`, spot names) and the whole
  `ClassifiedColorSpaceResource` / `SkippedColorSpaceResource` taxonomy are reused
  verbatim from `page_color_space_classify.rs` — the form inspector calls the same
  `pub` classifier and never reimplements it. The effective-`/ColorSpace` loop
  (resolve `/Resources`, `unique_entry(b"/ColorSpace")`,
  `inspect_dictionary_entries`, per-name dedup, sort) mirrors the page inspector's
  `inspect_effective_color_spaces` but lives self-contained in
  `form_color_space_resources.rs`, taking a `None`-inherited `ResourceContext`
  (the page files are left untouched, so the page-scope walk is byte-identical).
- Copy budget: one `/Resources /ColorSpace` inspection per expanded form (bounded
  by the caller's existing `FormWalkContext` budget), reusing the same
  lookup/inheritance machinery. The report retains only the classified family
  model + small skip records — no decoded object bytes, dictionaries, or stream
  bodies are held (a `report_retains_no_source_bytes` test pins this). No new
  per-operator allocation.
- Concepts cross-checked (never copied, no GPL/AGPL read): Apache PDFBox
  `PDFormXObject.getResources()` (may be null when a form omits `/Resources`),
  pdf.js swapping to the form stream's own resources while preserving graphics
  state, pikepdf resolving `/Do` + colour through the form's own `/Resources`;
  ISO 32000-1 §7.8.3 (other content streams carry their own `/Resources`),
  §8.10.1/§8.10.2 Table 95 (form XObject resource dictionary is self-contained,
  not promoted to the outer stream).

### T137 - Page Resource Colour-Space Classification (F4-6 slice 1a)

- New read-only inspector `inspect_document_page_color_space_resources[_with_lookup]`
  walks the page tree root-down (same inheritance/lookup machinery as the `/XObject`
  resource pass — `ResourceContext::from_dictionary`, `resolve_reference`,
  `unique_entry`) and classifies each page's effective `/Resources /ColorSpace`
  entries into `ClassifiedColorSpaceResource { name, family, component_count,
  spot_names, alternate_space }`. It resolves resource SHAPES only — no paint
  semantics, colourimetry, tint-transform evaluation, or profile parsing beyond the
  shallow `/ICCBased /N`.
- `ColorSpaceFamily` covers device families + `IccBased`/`Separation`/`DeviceN`.
  HONESTY rules enforced by classification: an `ICCBased` space with `N=4` is
  reported as `IccBased` (component count tracked separately), NEVER `DeviceCmyk`;
  a `Separation`/`DeviceN` alternate space is recorded as a FACT (`alternate_space`)
  but is never substituted as the painted source.
- Unsupported/unresolvable shapes become structured `SkippedColorSpaceResource`
  diagnostics (never a bare unknown): `MissingColorSpaceResources`, `MissingColorSpace`,
  `Duplicate*`, `NonDictionaryColorSpace`, `UnknownColorSpaceName`,
  `MalformedColorSpaceOperand`, `WrongComponentCount`, `UnsupportedPatternColor`,
  `UnsupportedIndexedColor`, `UnsupportedLabOrCalSpace`, `UnresolvedResourceReference`,
  `UnsupportedTintTransform`, plus a delegated `Resources` bucket for inherited
  `/Resources` failures.
- Scope: PAGE-scope resources only (form XObject resource environments are slice 1b).
  Read-only: no byte mutation; a `/Resources /ColorSpace` value read via `lookup`
  (including from a resolved compressed object) contributes family/shape facts only —
  no resolved-object member span is ever surfaced as an original-PDF source offset
  (reuses the T134 resolved-body provenance boundary).
- Module split for the 1000-line gate: `page_color_space_resources.rs` owns the
  page-tree walk, report types, and skip taxonomy; `page_color_space_classify.rs`
  owns the per-entry classification (name / array / indirect definition → family,
  shallow array element scanning, `/ICCBased /N`, `/Alternate`).
- Concepts cross-checked (never copied, no GPL/AGPL read): Apache PDFBox
  `PDColorSpace` (factory from name/array + `getNumberOfComponents`), pdf.js graphics-
  state fill/stroke colour-space resolution through resources, pikepdf's operator-
  interpretation guidance; ISO 32000-1 §7.8.3 (resource dictionaries + inheritance),
  §8.6.5 (ICCBased is profile-based, not device), §8.6.6 (Separation/DeviceN/Indexed/
  Pattern), §8.6.8 (`CS/cs`, `SC/SCN/sc/scn`).
