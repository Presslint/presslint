# presslint-write Journal

Earlier entries are preserved in
[JOURNAL-archive-2.md](JOURNAL-archive-2.md) and
[JOURNAL-archive.md](JOURNAL-archive.md).

## T195 - Bounded iterative reached-Form closure with clone-set plan, no bytes

### The plan contract

`form_clone_set_plan.rs` + `form_clone_set_plan/walk.rs` add ONE private,
request-scoped `FormCloneSetPlan` family. Seeding consumes the T194 binding
witness report through the SAME borrowed document consumer index the
ownership adapter uses: the sequence pipeline now binds
`inspect_object_consumer_index` to a local, borrows it for the T194 binding
inspection + plan construction, then moves it into
`ContentObjectOwnershipIndex::new` — one index per request, unchanged
conversion/staging behaviour.

A selected-page witness seeds a clone set only when ALL hold: the binding walk was globally
complete (any page-tree truncation or skip leaves ZERO candidates,
fail-closed), decoded `/Subtype /Form`, an explicitly leaf-direct binding
path (direct leaf `/Resources`, direct `/Resources` and `/XObject`
dictionaries — asserted explicitly even though the verdict's fixed check
order implies them), and the exact `Unproven { TargetConsumersNotExclusive }`
verdict. `ProvenPageLocal` is counted as an in-place candidate (no clone);
wrong subtypes, inherited/indirect paths, and incomplete-index verdicts are
counted, never walked. Unselected page witnesses consume no closure or
reservation budget. Within one page, several qualifying witnesses binding one
target share one `(page, target)` set retaining those witnesses' key/value
ranges; the same source on different pages gets distinct sets.

The ONLY public change is the additive omit-when-empty
`ConvertedPage::form_clone_set_plan_counts`
(candidate/planned/refused-set, planned-object, and null-equivalent
counters, `FormXObjectRefusalCounts` house pattern, per-field zero omission).
Counts join pages by the exact identity triple (leaf reference, object byte
offset, ordinal) with duplicate-reference slot poisoning — the same
fail-closed join every other advisory report uses. Observe-only: no converter
skip, no change to the closed Form refusal-class taxonomy, and no emitted-byte
change (`output.bytes[..input.len()] == input` is asserted with the plan
active).

### The walker

Iterative FIFO only (`VecDeque`), never recursion (PDFBOX-2574/-5232,
pypdf #2098/#2839 class). The visited map is keyed by exact `IndirectRef`
identity — object number AND generation, never value equality
(PDFBOX-4477) — with insertion BEFORE enqueue; edges are examined in body
source order; members are ordered deterministically by source reference.
Existing public primitives only: `resolve_object`/`ResolvedObjectData`,
`inspect_object_body_references_resolved`, `inspect_object_dictionary`, and
the span scanner behind them. A self-referential indirect `/Length`
(pypdf #3112) is an ordinary visited-map cycle; an indirect `/Length` target
is an ordinary member; explicit `null` is an ordinary value.

Free/not-found identities are terminal `NullEquivalent` verdicts
(ISO 32000-1 §7.3.9–7.3.10): the original token is preserved, nothing is
allocated, nothing is nulled out or dropped. A generation mismatch, header
mismatch, or malformed extraction is a `ResolutionFailed` refusal, and an
ambiguous/reserved/out-of-range xref answer refuses as `AmbiguousLookup`
(the qpdf 11.6.0/11.6.1 page-tree-predicate lesson: free, absent, mismatched
and spoofed identities are all distinct cases, tested separately).

### The frontier table

Fail-closed, NO drop lane, NO null-out rewrite; the whole set refuses with a
typed class, and the exact FIRST refusal is retained per set:

- MEMBERS: resolved nested forms, fonts, descriptors/programs, colour
  spaces, ICC profiles, `ExtGState`, shadings, patterns, metadata streams,
  groups, `/Filter`, `/DecodeParms`, indirect `/Length`, resource
  dictionaries — anything reached by reference that passes the preflight.
- `GraphEscape`: exact known catalog, page-tree-root, and leaf identities
  refuse even when `/Type` is absent or spoofed; exact AcroForm, structure,
  and `OCProperties` graph membership is derived from the SAME borrowed
  complete consumer index (decoded catalog-key names, no second walk). As an
  independent backstop, a member's decoded `/Type` naming
  `Catalog`/`Pages`/`Page`/`StructTreeRoot`/`StructElem`/`OCG`/`OCMD` refuses.
  Full-reference identity keeps generation mismatches on the ordinary
  resolution-failure path rather than misclassifying them as known pages.
- `AncillaryKey`: `/OC`, `/Ref`, external-file `/F`, `/OPI`, `/PieceInfo`,
  `/StructParent`, `/StructParents` on ANY dictionary-bodied member. The
  `/StructParent(s)` keys are Table-95 INTEGERS invisible to a reference
  walker (PDFBOX-5372 class), so the preflight is a decoded-name,
  duplicate-sensitive top-level key scan: `/O#43` and `/StructP#61rent`
  spellings cannot bypass, and ANY duplicated decoded key refuses as
  `MalformedStructuralKeys` (ambiguous dictionary).
- `MalformedStructuralKeys`: undecodable key escapes, non-name `/Type`
  values, unscannable dictionaries.
- `BodyScanIncomplete`: per-body scanner truncation (the retained
  65,536-reference cap) or a skipped out-of-range reference shape.
- Budget classes below; `ReservationRefused` for the all-or-nothing
  reservation stage.

The refuse-everything policy is deliberate: this slice writes no bytes, so a
refusal costs nothing, and the typed classes are real-corpus telemetry for a
future deliberate drop/repair slice. Dropping `PieceInfo`/`OPI` silently is
unacceptable in prepress; optional-content and structure imports need
companion plans this slice does not own.

### Bounds and honesty

Per clone set: reference depth 64, 4,096 unique resolved clone members, 4,096
cumulative accepted reference occurrences, and 1 MiB cumulative object-stream
decode work. Null-equivalent terminals allocate no clone identity and do not
spend the member budget; their retained verdicts and visited identities remain
bounded by the occurrence budget. Like the T193
floor proof, the occurrence budget is an accepted-accumulation bound, not a
scanner-work cap: one delegated body scan may first discover up to 65,536
references before the budget refuses. The RESIDUAL decode budget is passed
into every `resolve_object` call, so a later container decode is cut off the
instant it would exceed the cumulative cap; a rejection caused by that
reduced bound is classified as decode-budget exhaustion, and every
budget-dependent failure exhausts its residual before refusing (T190
lesson) — recorded usage reads fully spent, nothing looks retryable.

Worst-case complexity per set: O(members × per-body scan) delegated scanner
work plus O(occurrences) accepted accumulation, with visited-map lookups
O(log(members + null terminals)) per edge. The live walk retains at most
4,096 member records, 4,096 outgoing-reference records in total, 4,096 null
verdicts, and 4,097 visited identities (root plus accepted occurrences), plus
the FIFO queue. One delegated scan may transiently return 65,536 references
before completeness/budget rejection, and one compressed resolution may own
up to the residual 1 MiB decoded-container budget; neither transient buffer is
retained in the plan. Compressed-member resolution decodes containers per
member without caching (the `resolve_object` no-cache contract), so two
members in one container pay its decode twice; that amplification is exactly
what the 1 MiB cumulative budget bounds and counts.

Across a request with `S` candidate sets, accepted retained accumulation is
O(S + total retarget sites + total members + total occurrences + total null
terminals), where each per-set component has the limits above; the single
reservation vector adds `total members` compact identities at peak. The
temporary structural-frontier map is O(G), where `G` is the
catalog/page-tree/page and catalog-companion identity projection of the
already-built bounded consumer index. Building it iterates that borrowed index
once and scans no PDF body again; it is skipped entirely when incomplete
binding coverage already prevents every closure walk.

### Reservation

Exactly ONE `reserve_fresh_object_references(input, total)` call per request,
over the total member count, after all non-refused sets are known. An empty
plan calls it with zero; the reservation seam itself returns immediately
without a whole-document floor scan. The partition is
deterministic: sets in (page ordinal, root source reference) order, members
in source-reference order, consecutive identities per set. All-or-nothing: a
floor-proof failure (whose own whole-document 4,096-reference budget may
refuse a tiny closure — reported honestly as `ReservationRefused`, never as
a closure fact and never as a public `WriteError`), a returned-count contract
mismatch, or the ISO 32000-1 Annex C Table C.1 indirect-object-limit check
(8,388,607, run before the plan is declared ready) refuses EVERY would-be-
planned set with the typed reservation class. Each set owns its private
source-to-fresh pairs; no map outlives one clone set and none is reused
across pages or separate clone operations (pikepdf#271 class).

### Retained for the clone-body export and one-page retarget slices

Crate-private per set: the input byte length and plan ordering; the page
identity triple; every qualifying witnessed retarget entry's name and key/value
byte ranges with the expected old target; the root source reference and
corroborated offset; ordered members with uncompressed offset or compressed
container/member coordinates; duplicate-preserving outgoing-reference lists;
null-equivalent verdicts; the source-to-fresh mapping; budget usage; and the
exact first refusal. T196 (clone-body export, which must add a materialized-body
byte budget — qpdf#219) and T197 (one-page retarget) consume these without
re-walking; nothing here authorizes them.

## T193 - Deterministic fresh-object reservation and incremental append

### The one new public shape

`FreshObjectBytes { reference: IndirectRef, body_bytes: Vec<u8> }` mirrors
`DirtyObjectBytes` structurally but carries the opposite identity contract: it
names a NEW, uncompressed, generation-zero object that must never rewrite an
existing identity. It derives no serde (matching `DirtyObjectBytes`). Two free
functions round out the public surface:

- `reserve_fresh_object_references(input, count) -> Result<Vec<IndirectRef>, WriteError>`
  proves a complete, bounded, collision-free floor over the effective
  newest-wins object set and returns exactly `count` contiguous ascending
  generation-zero references starting there. `count == 0` returns an empty
  vector without any whole-document scan.
- `write_incremental_revision_with_fresh_objects(input, dirty_objects, fresh_objects) -> Result<Vec<u8>, WriteError>`
  recomputes and re-validates that same floor for the supplied live bytes,
  requires the caller's sorted fresh references to match it exactly (a
  reservation is not a capability token — the same input must still be
  supplied unchanged), validates dirty objects through the pre-existing path,
  and emits dirty and fresh objects merged in deterministic ascending-
  reference order on both backends. `fresh_objects=[]` delegates straight to
  the pre-existing `write_incremental_revision`, so it is byte-for-byte
  identical and performs no new allocation/reference discovery.

`crates/presslint-write/src/fresh_objects.rs` is the private mechanic behind
both: no second public domain abstraction, only the floor proof and its
bounded accounting helpers.

### The floor proof

The floor is one past every identity that could collide. Both backends prove:
the whole-`/Prev`-chain effective `/Size`; every effective xref entry object
number, including high free entries (classic) and, for xref streams, each
type-2 entry's object-stream container number and every observed section's
own indirect-object header number (so a defective section whose entry map
omits itself still raises the floor above its physical identity); every
indirect-reference target in the active trailer/xref-stream dictionary
(scanned over the dictionary's own balanced extent, never the trailing stream
bytes for the xref-stream case); and every indirect-reference target in every
effective live (in-use/uncompressed/compressed) object body, resolved through
the existing `resolve_object` + `inspect_object_body_references_resolved`
pair — including xref-defined objects with no other referrer and compressed
object-stream members. A classic table can never carry a type-2 entry, so the
classic proof never touches compressed-member decode work.

Any incomplete proof fails CLOSED rather than returning a partial ceiling:
an unresolvable object, a body/trailer reference scan that found an
out-of-range reference shape or hit the per-body truncation cap, a
reserved/future xref-stream entry type (its semantics cannot be proven
complete, so it stops the whole computation rather than being skipped), a
numeric conversion overflow, or either of the two writer-local cumulative
caps below.

`reserve_fresh_object_references` re-runs the same `scan_active_trailer`
(classic) / `scan_active_xref_stream` (xref-stream) `/Encrypt`/`/XRefStm`
check the write path already ran before it starts the floor proof, so a
nonzero reservation over an encrypted or classic-hybrid input refuses with
the same `EncryptedInput`/`HybridXrefStmInput` tags `write_incremental_revision`
already used — a fixed gap in this slice's initial cut, where the reservation
entry point bypassed that check and could otherwise "prove" a floor over an
unsupported source it should never have scanned at all.

### Bounds

Two writer-local cumulative caps, both in `fresh_objects.rs` and both
exercised by a dedicated exhaustion test:

- `MAX_FRESH_FLOOR_REFERENCES = 4096` is the accepted accumulation budget
  across the whole proof (trailer/dictionary scan plus every live body), not
  a hard cap on scanner work. Each delegated scan may first materialize up to
  `MAX_OBJECT_BODY_REFERENCES = 65,536`; after 4,096 discoveries accepted by
  earlier scans, the rejecting scan can therefore bring worst-case cumulative
  valid-reference discovery to 69,632 before the proof refuses. The two-tier
  bound is deliberate because the existing `presslint-pdf` scanner API has a
  per-scan cap and this slice does not add a budget-aware parser API.
- `MAX_FRESH_FLOOR_DECODE_WORK_BYTES = 1_048_576` (1 MiB) bounds the total
  decoded bytes from compressed-member container resolution. `resolve_for_floor`
  passes the *remaining* budget (`FloorAccumulator::decode_work_budget`), not
  the whole-cap constant, as each call's `max_decoded_object_stream_bytes`
  bound, so a second or later container decode is itself cut off the instant
  it would exceed the cumulative cap rather than paying its full decode cost
  and only being rejected afterward by `record_decode_work`. A rejection
  caused by that reduced bound (`DecodedObjectStreamTooLarge` or a Flate
  `OutputLimitExceeded`) is reported as `FreshFloorDecodeWorkCapExceeded`, the
  same tag a completed-but-over-budget decode would have produced.
  `resolve_object` does not cache decoded containers, so two compressed
  members sharing one container each pay decode cost again; this cap makes
  that repeated-decode amplification bounded and counted rather than assumed
  away, with worst-case actual decoded work never exceeding the documented
  cap (a fixed prior version bounded every call by the whole constant instead
  of the remaining budget, letting worst-case work exceed the cap before the
  cumulative check fired — fixed for this slice's public release).

### Xref-stream self-object avoidance

The internal xref-stream self-object number is placed above the whole fresh
reservation AND above every indirect-reference target parsed from the newly
appended dirty/fresh bodies. That second part is not part of the floor proof
(which only sees the EXISTING document): after appending dirty+fresh objects
to the output buffer, `next_xref_object_number_with_fresh` re-scans each
appended object's own bounded extent on the ALREADY-ASSEMBLED output with the
same `inspect_object_body_references` used elsewhere, so stream payload bytes
are never mistaken for object syntax. This lets a caller intentionally
reference a fresh object from a dirty/fresh body while guaranteeing the
self-object can never accidentally satisfy an otherwise-dangling reference —
proven by a dedicated test that references the naive "chain max + 1" self-slot
from a fresh body and asserts the self-object skips above it while that
number stays absent from the reopened chain.

### Additive `WriteError` tags

Sixteen new flat `WriteError` variants (their `stage` names extend the public
serde-tag vocabulary): fresh-object shape validation
(`NonZeroFreshGeneration`, `DuplicateFreshObject`, `FreshDirtyObjectCollision`,
`FreshReservationNotContiguous`, `FreshReservationFloorMismatch`,
`FreshReservationNumberOverflow`) and floor-proof/self-avoidance failures
(`FreshFloorNumericOverflow`, `FreshFloorResolution`,
`FreshFloorBodyReferences`, `FreshFloorTrailerReferencesIncomplete`,
`FreshFloorObjectReferencesIncomplete`, `FreshFloorReservedEntry`,
`FreshFloorReferenceCapExceeded`, `FreshFloorDecodeWorkCapExceeded`,
`FreshFloorSectionHeader`, `FreshXrefSelfObjectOverflow`).
`FreshFloorBodyReferences`/`FreshFloorObjectReferencesIncomplete` are shared
between the floor proof (existing bodies) and the self-avoidance scan (newly
appended bodies); the carried `reference` disambiguates which.

### Legacy compatibility, provably

`write_incremental_revision` and the classic/xref-stream backend functions it
calls are byte-for-byte UNCHANGED. The new fresh-aware backend functions
(`write_classic_incremental_revision_with_fresh`,
`write_xref_stream_incremental_revision_with_fresh`) are separate functions
that reuse the same private helpers (`order_dirty_objects`,
`validate_dirty_objects`, `AppendRevisionWriter`/`XrefStreamRevisionWriter`);
`AppendRevisionWriter::new` and `XrefStreamRevisionWriter::new` were refactored
to delegate to a new `with_capacity_estimate` constructor with the identical
headroom formula, a zero-behavior-change internal reuse. A dedicated test
matrix asserts `write_incremental_revision_with_fresh_objects(..., &[])`
byte-matches `write_incremental_revision` across the existing
encrypted/hybrid/duplicate/generation-mismatch/not-in-use/non-PDF rejection
fixtures, plus zero/multiple/reversed-order dirty objects on both backends.

### Test matrix additions (fail-closed and legacy-parity gaps)

Beyond the vertical/floor/fail-closed cases above, the matrix also proves:
nonempty reservation refusal for encrypted and classic-hybrid inputs, and for
an xref-stream input whose own dictionary declares `/Encrypt`; xref-stream
fresh-only append and full classic-style legacy parity (zero/reversed dirty
objects) on the xref-stream backend; a floor raised by a high in-use (not
just free) entry despite an understated `/Size`; a floor raised by a dangling
reference inside an object genuinely reachable via the page `/Parent` edge,
not just an orphaned unreferenced body; two compressed members resolved in
either physical container packing order producing an identical floor; a
broken classic/xref-stream `/Prev` chain, a malformed (unclosed) trailer
dictionary, a compressed member whose `/ObjStm` header declares the wrong
object number, a single body over the per-body reference-scan truncation cap,
and an xref-stream self-object overflow after an otherwise-valid fresh
reservation, all refusing before output assembly; and the vertical round-trip
tests resolving and byte-comparing every appended fresh/dirty body through
the real header-inspection API, not just locating its header. The
floor-proof's `FreshFloorSectionHeader` branch (an xref-stream section whose
own indirect-object header cannot be parsed) is intentionally left untested
by a black-box fixture: `build_xref_stream_chain` already re-parses that
exact header at that exact offset through the identical
`inspect_indirect_object_header` call while building the chain the floor
proof consumes, so a chain that built successfully cannot reach this branch
from any externally-constructed input; it stays as defense in depth.

### Deferred clone wiring

No production planner calls the new API in this slice. `IncrementalRevisionPlan`,
`PlannedObjectAllocation`, `MutationBoundary::IndirectObjectClone`, and the
`write_incremental_revision_plan` bridge stay frozen — this is the lower-level
identity/serialization prerequisite only, not a Form clone, consumer-edge
choice, or paint-mutation authorization. `presslint-pdf`'s existing exported
resolver/reference-inspection seams (`resolve_object`,
`inspect_object_body_references_resolved`, `ClassicXrefChain`/`XrefStreamChain`,
`inspect_indirect_object_header`) proved sufficient; no new `presslint-pdf`
public API was needed.

## T192 - Form refusal-class instrumentation, serialized per-page counts

### The taxonomy

Every `FormXObjectEffectAnalyzer` refusal is now classified into exactly one
of eleven stable `FormXObjectRefusalClass` variants
(`form_xobject_effect/refusal.rs`, the ONE new public taxonomy this slice
adds): `StructuralPreflight` (exact-identity corroboration or the decoded-name
Form-dictionary preflight), `StreamFilterOrExtent` (unsupported filter/chain/
predictor, extent inspection failure, or an intrinsic — non-budget — Flate
decode failure), `TransparencyGroup`, `RawGrammar` (tokenize/assemble/raw-
preflight failure or an intrinsic seeded-walk fallthrough), `ColorAuthority`
(the Form-local `/Resources /ColorSpace` authority or a `CS`/`cs`/`SC`/`SCN`/
`sc`/`scn` validation over it), `XObjectAuthority` (the Form-local
`/Resources /XObject` authority failing to resolve an invoked `Do`),
`ExtGStateAuthority`, `RecursionCycle` (an active-path re-encounter),
`RecursionDepth` (a nested descent past the bounded traversal horizon with
nothing cached), `TargetBudget`, and `DecodedByteBudget` (zero-entry budget,
raw over-budget, or a Flate output-limit hit). The taxonomy authorizes
nothing, retains no PDF identity/byte/resource-name fact, and is exposed only
as counts.

### Counting semantics

`ConvertedPage` gains one additive `form_xobject_refusal_counts` field
(`FormXObjectRefusalCounts`, named `usize` fields, `is_empty`, `add`/`fold`
helpers, `#[serde(default, skip_serializing_if = "..")]` so existing JSON
without refusals stays byte-identical). It counts a refused DEMANDED Form
identity once per exact `(reference, reached_offset)` identity per page:
repeated `Do` and aliases of one identity count once; the same identity
demanded on another page counts once there too, including on an analyzer
cache hit (zero decode/walk/budget charge). Counting attaches only to the
public `analyze()` entry point — the one page demand actually calls — so
nested descents never count independently; a nested child's intrinsic
failure bubbles up to the root demand classified as the CHILD's own
actionable cause, and only a genuine cycle re-encounter or a depth cutoff
classify as `RecursionCycle`/`RecursionDepth`.

### Mechanics: a per-compute first-wins class, a per-identity cache, a
### per-page tally

`compute`/`analyze_bytes`/`walk_lane_effect` return
`Result<CachedFormAnalysis, FormXObjectRefusalClass>`: `Err` names every
whole-Form refusal at the gate that fired, while `Ok` carries the computed
lattice and the optional first nested-fold class. This makes the impossible
"refused without a class" state unrepresentable. `fold_xobject_invoke` returns
`Result<Option<FormXObjectRefusalClass>, FormXObjectRefusalClass>` — `Err` is a
whole-Form intrinsic refusal (an unresolved `Do` name); `Ok(Some(_))` is a
nested-Form fold that damaged this walk's lattice (a cycle, a depth cutoff, or
the invoked child's own bubbled class, read from the child's inline cache
attribution); `Ok(None)` is a clean fold. The walk keeps only the FIRST such
class in walk order.
`analyze_nested` stores one private `CachedFormAnalysis` value — the existing
`BoundedFormLaneEffects` lattice plus an optional refusal class — per existing
cache key. There is no second keyed map, duplicated key, allocation, or lookup
for attribution: cache hits return the lattice and class from that one value.
The class is present exactly while the lattice's maximum-depth slot is Unknown;
a lattice that later proves an effect there clears any
stale class. The analyzer additionally holds a per-page identity-seen
`BTreeSet` and a `FormXObjectRefusalCounts` accumulator, reset by
`begin_page_refusal_tally` (called once per analysed page, before its Form
demands begin) and read by `take_page_refusal_counts` (called once per page,
after its content sequence is edited) — both `pub(crate)`, called only from
`content_color_convert.rs`; `page_xobject_policy.rs` needed no change.

A failed DEEPENING recompute (a cached partial entry re-entered with more
remaining edges than it has computed, whose fresh recompute attempt still
fails) keeps the prior lattice unchanged — its computed slots must never be
destroyed — but now replaces the cached value's inline attribution with the
fresh attempt's class. The lattice is a pure function of the form's own subtree
and stays untouched either way (observe-only for the actual admission result);
only the tallied CLASS is corrected, since it must reflect why THIS query's
deepening just failed (e.g. the aggregate byte budget running out elsewhere in
the interim), not whatever a much earlier, shallower compute happened to record.
Review caught this as a stale-class bug (a `RecursionDepth`-classified partial
entry could keep reporting `RecursionDepth` even after a later deepening
attempt failed on `DecodedByteBudget` instead); the fix updates the class on
every failed recompute, first or deepening alike.

### Observe-only, provably

No admission decision, byte output, cache/lattice/budget behavior, public
enum, or epoch tally changed: the full pre-existing `presslint-write` suite
passes unmodified, and a dedicated fixture locks identical bytes,
`operators_converted`, and operator-skip counts for an unrelated device-colour
operator on the SAME page as an actually-invoked, refused Form `Do` — the
refusal tally is observed alongside that operator's own conversion, not
substituted for a fixture with no Form at all. The no-analysis-context refusal
at `page_xobject_policy.rs:195` (the structural-policy path with no attached
analyzer) stays uninstrumented by design — left for a future sweep to add its
own class, not coded around.

**Correction (T193):** `resource_alias_candidates_refused` and
`form_xobject_refusal_counts` are independent event domains — the former
counts alias-conversion refusals, the latter counts Form-effect-analysis
refusals, over different (overlapping but not nested) event sets. No
subtraction between them has semantic meaning; a prior draft of this entry
suggested `resource_alias_candidates_refused − Σclasses` as a way to infer the
uninstrumented `page_xobject_policy.rs:195` count. That arithmetic is wrong
and has been removed. Report the two tallies separately.

### Performance

Classification at an already-executing refusal gate is constant work and no
new tokenization/decode/walk pass runs. The fixed-size
`FormXObjectRefusalCounts` itself allocates nothing. Per-page identity
deduplication uses an allocating `BTreeSet<(IndirectRef, usize)>`: each refused
root demand performs an `O(log n)` lookup/insert in the number of distinct
identities already seen on that page, and `begin_page_refusal_tally` resets the
set at page boundaries so it does not accumulate across pages.

Attribution adds one small `Option<FormXObjectRefusalClass>` to the existing
cache value. It therefore adds no second tree node, duplicated key, keyed
allocation, or separate attribution lookup. `MAX_FORM_TARGETS` bounds charged
first-seen COMPUTATIONS, not cache cardinality: after the charge budget reaches
zero, each later unseen demanded identity still publishes its all-Unknown
`TargetBudget` cache entry, preserving the pre-existing cache behavior. Inline
attribution shares that existing cardinality instead of doubling it or
claiming a 256-entry bound. No hot-path byte/profile/stream copy was added.

### Tests

One fixture per class asserts exactly that counter is 1 and all others 0
via a shared exhaustive-match helper (`assert_only_class`, `tests/
alias_epoch_form.rs`, reused by every sibling test module) so a twelfth
variant fails to compile until a new lock is written. Per-file locks: general
gates plus `TargetBudget`/`DecodedByteBudget` (including the raw-vs-Flate
`StreamFilterOrExtent`-not-`DecodedByteBudget` distinction) in
`alias_epoch_form.rs`; `ColorAuthority` in `alias_epoch_form_color_resources.rs`;
`ExtGStateAuthority` in `alias_epoch_form_extgstate.rs`; `XObjectAuthority` and
a root-corroboration `StructuralPreflight` in `alias_epoch_form_xobjects.rs`;
`RecursionCycle` (self and mutual cycles), `RecursionDepth` (the nine-edge
horizon), explicit bubbled-actionable-cause locks — `RawGrammar`,
`TransparencyGroup`, and `XObjectAuthority` each bubbling from a grandchild
through two folds without ever classifying as a recursion class — a
raw-vs-Flate `RawGrammar` parity lock, an exact-budget cross-page cache-hit-
recounts-at-zero-further-charge lock, and a byte-exhaustion-cascade lock
pinning `DecodedByteBudget` for a second, otherwise-unrelated unseen target,
in `alias_epoch_form_recursion.rs` (chosen for file-size headroom under the
1400-line gate; these locks exercise the ROOT analyzer/cache mechanics, not
nested recursion specifically). `tests/content_color_convert.rs` adds a
two-page fixture proving alias/repeated-`Do` dedup and cross-page cache-hit
recount, a same-page two-distinct-identities-of-the-SAME-class lock, and an
observe-only lock pairing an actually-invoked, refused Form `Do` with an
unrelated device-colour operator on the same page (never a fixture with no
Form at all). CLI JSON-shape ripple (`crates/presslint-cli/src/tests/
report.rs`): omit-when-empty, nonzero serialization, and old-JSON
default-on-missing-field.

## T191 - Form-local proven-neutral gs/ExtGState admission

### The authority contract

The Form analyzer now admits `gs` operations whose decoded operand resolves,
through ONE bounded own-scope authority, to exactly one classified `ExtGState`
resource proven colour-lane neutral and font-inert; every other `gs` refuses
the whole Form as the cached all-Unknown lattice, symmetric with every other
intrinsic refusal. The ONE new private domain abstraction is
`FormLocalExtGStateAuthority` (`form_xobject_effect/extgstate.rs`), mirroring
the T189 `FormLocalXObjectAuthority` shape: a decoded-name `BTreeMap` of
per-name neutrality verdicts, a literal-poison set for undecodable spellings,
and a namespace poison flag. It is a GATE, not a state machine — it never
tracks which `gs` is in force where. Neutrality of every activated entry makes
activation order irrelevant, which is exactly what keeps the single
sentinel-seeded walk on the empty `ExtGState` environment with no walker,
`PaintProgram`, lattice, cache-key, or budget change. The authority retains
only owned decoded names, neutrality booleans, and poison state; no source
bytes, dictionaries, tokens, streams, or classifier reports survive the
compute.

### The exact neutrality predicate

Admission delegates entirely to the shipped classifier facts
(`presslint-pdf/src/extgstate_classify.rs`); no parameter semantics are
re-derived. ALL of: `is_overprint_active() == false` (ANY set `/OPM`,
including `0`, is active; explicit `false` overprint flags admit),
`is_transparency_active() == false` (`/CA`/`/ca` absent or exactly opaque,
`/BM` absent or Normal/Compatible, `/SMask` absent or exactly `/None`),
`has_unresolved_or_unclassified_safety_param() == false`, `font_effect`
exactly `Unset`, AND `has_unclassified_keys == false` — the classifier
documents that `Unset` proves `/Font` absence only while the aggregate flag is
false, so a benign `/LW`-style dictionary deliberately refuses in this slice;
widening requires a distinct proved-no-Font fact later. The parameter matrix
matches the page-level `gs` guard entry-for-entry. The shared classifier now
decodes safety-key spellings and requires semantic uniqueness for all seven
keys: any duplicate, including a raw-plus-escaped or identical-value pair,
surfaces through the existing unresolved/unclassified-safety predicate rather
than first- or last-winning. No report field, enum variant, or serde shape was
added, and the page guard retains its deliberate `/LW` precision.

### Demand, scope, poisoning and caps

The gate runs in `analyze_bytes` after the raw preflight and BEFORE the walk
(the walker's empty environment is compatibility-neutral and never stands in
for validation), and ONLY when a syntactically valid `gs` record is present —
the exact demand pattern of the `Do`/`CS` authorities. A Form without `gs`
never inspects its own `/Resources /ExtGState`, even a malformed one. The
authority resolves ONLY from the Form's own canonical, semantically unique
`/Resources /ExtGState`, corroborated via
`has_canonical_form_resource_dictionary(.., b"ExtGState")` before the shipped
`inspect_form_extgstate_resources` report is trusted: escaped, duplicate,
malformed, unresolved, indirect-`/ExtGState`, or ambiguous authority is
Unknown, and page/caller fallback is never consulted — a `gs` naming an entry
absent from the Form's own resources refuses even when the page defines a
neutral entry of the same name. Matching reuses the page guard's crate-private
`resource_name_match` seam for the malformed literal-poison case while the
bounded decoded-name map handles semantic equality: a semantic collision
poisons the decoded name (never first-win), an undecodable relevant spelling
retains literal poison, a matching named skip overrides a same-name classified
entry, and a nameless skip poisons the namespace; proven-absent `/Resources`/
`/ExtGState` skips are exact absence, not uncertainty, and an unused unsafe
declaration does not poison a used safe one. Classified entries plus skips are capped at 256
(mirroring `MAX_XOBJECT_FACTS`) and distinct raw `gs` operand spellings are
separately capped at 256; either overflow is namespace Unknown. Generation
mismatches, unresolvable references, and compressed object-stream entry
targets refuse through the shipped classifier's named skips.

An INDIRECT `/ExtGState` entry additionally requires exact target
header-identity corroboration before its classified facts are trusted — the
same discipline the `XObject` authority applies to its targets. The shipped
classifier resolves an indirect entry through the xref and classifies
whatever dictionary sits at the resolved offset without checking that the
object header there identifies the requested object, so a malformed xref
binding `N G R` to a DIFFERENT object's neutral body would fabricate a
false-neutral verdict while a repairing reader may locate — and activate —
the real, unsafe object (wrong-offset object-key repair is a known
interoperability pitfall). Because the classified report does not carry the
resolved reference, the authority re-scans the canonical raw `/ExtGState`
entries once per build: each `IndirectReferenceLike` value must re-resolve
through the current lookup to an in-use source-addressable object with a
matching generation whose reinspected header reference equals the requested
reference. A failing entry poisons only its own decoded name (an unused
mispointed declaration does not block a used corroborated one); unscannable
authority poisons the namespace. Direct dictionary entries involve no xref
binding and are never re-scanned.

### Raw grammar and recursion composition

The closed raw preflight admits `gs` by extending the existing
single-name-operand arm (`CS`/`cs`/`Do`/`gs`): no open path, exactly one
syntactically valid name operand; wrong arity, non-name operands, and
open-path placement refuse in the preflight. Everything else about the
T184-T190 boundary is byte-for-byte unchanged: cache key, unconditional
post-compute publication, lattice/deepening contract, aggregate decoded-byte
budget with failure-exhaustion, semantic preflights, filter rules, recursion
depth/cycle/charging behavior, sentinel seeding, and fold semantics. Gates are
built per-identity from each Form's OWN resources: a child never sees the
root's entries (a child activating a name only the root defines refuses), a
neutral-`gs` child composes its lattice normally at any depth, an unsafe-`gs`
child makes the root Unknown, and repeated invocation keeps T190 charge-once
behavior. Only existing eligible page setter bytes change end-to-end; every
Form/resource/ExtGState object byte and every public report/schema/digest/
selector/CLI surface is unchanged.

## T190 - Bounded recursive nested ordinary Form colour-effect analysis

### The depth-slot lattice and why it is intrinsically cacheable

The analyzer now descends into invoked, retained, root-admissible nested
ordinary Form XObjects. The ONE new domain abstraction is private
`BoundedFormLaneEffects` (`form_xobject_effect/recurse.rs`): conceptually
`[Option<FormLaneEffect>; MAX_NESTED_FORM_DEPTH + 1]`, with
`MAX_NESTED_FORM_DEPTH = 8` counted in nested-Form `Do` EDGES (the root is not
counted; a full-depth path holds nine form streams). Slot `d` is the
two-bit/Unknown result with `d` edges remaining; slot 0 refuses every invoked
nested Form while still admitting local colour, ordinary Images and stencils;
the public `analyze` result is the maximum-depth slot, so the
`Option<FormLaneEffect>` seam and everything downstream
(`page_xobject_policy.rs`, `alias_epoch_plan.rs`) are unchanged.

The lattice is what makes every cached outcome a PURE function of the form's
own subtree: a single cached two-bit value cannot be both path-depth-aware and
order-independent, and a transient never-cached depth refusal would make cache
publication conditional (the bug class earlier turns caught) and re-charge
decoded bytes on recompute. One walk computes every slot the traversal horizon
lets it demand (graphics state is shared across slots — ISO 32000-1 §8.10.1
restores state after every `Do` — so slots differ only at nested-Form folds),
and publication stays unconditional post-compute. Because slot `d` demands
child slots down `d` further edges, a frame entered deep on a path can only
compute its low slots: the lattice therefore carries `computed_through`, the
highest slot actually computed. Slots above it are UNCOMPUTED — held `None`,
so they read fail-closed everywhere — never refused: a computed slot's value
is final and pure, and a shallower re-entry only ever EXTENDS an entry. An
intrinsic refusal (grammar, preflight, poisoned name, unsupported target or
malformed authority), as well as an unaffordable first compute, refuses ALL
slots and caches the complete all-Unknown lattice. `recurse.rs` owns only the
lattice contract, the depth constant, the constructors, lane marking, the fold
and the unavailable-descent refusal; resolution, colour validation, caching,
budgets and decoding stay with their existing owners.

### Recursion, cycles, and charging

Public `analyze` allocates one `IndirectRef`-keyed active-path `BTreeSet`
(bundled with the request bytes/lookup in a private descent context) and
delegates to one internal recursive entry; recursion happens on `&mut self`
under the caller's single outer `RefCell::borrow_mut` and never re-enters
through `PageXObjectPolicy`. At an invoked nested Form target, in order: (1)
an active-path re-encounter is a cycle — the child is all-Unknown for that
fold and nothing is charged or published (each cycle member still closes its
own cycle from its own compute, so all members cache all-Unknown
order-independently); (2) the cache serves whenever a compute from this depth
could not reach past what the entry already holds — every complete entry, and
a partial entry at or past the horizon; (3) within the horizon, an unseen
identity runs the full root-identical compute, and a partial entry
re-entered with more remaining edges than it has computed runs a DEEPENING
recompute, each with its reference held in the active set; (4) the computed
lattice publishes unconditionally, extending — never shrinking — a prior
partial entry. The active set is keyed by `IndirectRef` only (exact
corroboration admits at most one reached offset per reference per request, so
ref-only is strictly conservative); the cache key stays
`(IndirectRef, reached_offset)`. Depth is the recursion path length — no
second counter.

The TRAVERSAL is bounded to the same horizon as the lattice: an unseen frame
is entered only while the path stays within eight nested edges, so any
analysis — provable or adversarially deep — holds at most nine form streams
on the active path and at most nine lean native compute frames (decoded
buffers and tokens are heap-owned and dropped per frame). A deeper acyclic
chain is CUT, not followed: the frame at the horizon computes slot 0 (an
invoked nested Form with no edge left is Unknown — its true value), leaves
its higher slots uncomputed, and nothing past the horizon is decoded, charged
or entered. Every cut frame's cached slots stay pure, so public results are
order-independent: a horizon-cut mid form later queried as a root (depth 0
can always demand the full slot range within the horizon) deepens its entry
and proves or refuses exactly as on a fresh analyzer.

First-seen TARGETS are charged once per unique cache key, root or child;
aliases, sibling DAG reuse, repeated `Do` and cycle re-encounters recharge
nothing. Decoded BYTES follow the actual work instead: every successfully
read raw body or decoded Flate body charges on a first compute and on every
horizon-cut deepening recompute, while a budget-dependent failed attempt
exhausts the residual aggregate allowance. In particular, a raw extent larger
than the remaining allowance and a Flate `OutputLimitExceeded` rejection both
set the byte budget to zero; later deepening stops at the existing zero-budget
guard before preflight or inflation. Intrinsic decoder failures remain
deterministic and leave the residual budget unchanged. This keeps the
analyzer-owned aggregate budget an honest bound on total decode work and
closes the repeated bounded-inflation amplification vector. The per-form
256-fact caps apply per frame unchanged. Budget exhaustion mid-descent caches
all-Unknown for an unaffordable first-seen frame while cached forms keep
serving; an unaffordable deepening keeps the previously published partial
entry and every already-computed pure slot instead of overwriting it.

### Child admission and the fold

A child enters through the complete existing analyzer path verbatim:
target/byte budgets, exact reference/generation/reached-offset corroboration,
the semantic dictionary preflight, filter/extent classification, transparency
`/Group` refusal, the raw grammar with balanced q/Q, and its OWN demand-built
colour and `XObject` authorities. `canonical_form_resources_entries ==
Ok(None)` (proven-absent `/Resources`) stays admissible exactly as for a
root: nothing consults caller/page resources, so whatever remains admissible
is resource-independent by construction (a resource-less child invoking an
alias or a `Do` fails to resolve on its own and refuses). Children are ALWAYS
analyzed sentinel-seeded exactly like roots — never seeded from the parent's
live colour — so summaries stay state-independent.

At a proven nested-Form `Do`, each parent slot `d >= 1` folds the child's
slot `d - 1` and parent slot 0 becomes Unknown. Per lane: a child bit
propagates only while the parent's lane still equals its inherited sentinel
at the invocation (read from `op.state`, exactly like the shipped stencil
arm — the walker is state-neutral for `XObjectInvoke`, so post-op equals
pre-op); a live local parent colour absorbs the consumption; an Unknown child
slot refuses the parent slot. Child writes never alter subsequent parent
state (implicit q/Q of §8.10.1; a net-popping child stream is malformed per
§8.4.2 and refuses through its own raw preflight). A child invoked at several
sites or under aliases reuses one cached lattice with independent folds. The
authority's single `pub(super)` resolution returns the retained exact target
tuple with its effect, so each `Do` spelling is decoded and looked up once.

### Boundary and tests

The entire T184-T189 boundary is unchanged: cache key shape, unconditional
publication, cached Unknown, the 256 first-seen target cap, the aggregate
decoded-byte budget, semantic preflights, the raw grammar's admitted operator
set, one sentinel-seeded walk per compute (a deepening recompute is one more
such walk, never a second interpreter), the Form-local Device colour
and `XObject` authorities, `/Default*` rules, exact identity corroboration,
and page-only byte mutation. `/Matrix`/`/BBox` stay unmodelled and the
conservative colour-lane argument extends transitively to children. Form,
resource and image objects remain read-only, byte-identical, and appear once
in output; no public enum, report, serde, digest, selector or CLI surface
changed. Substantive coverage lives in `tests/alias_epoch_form_recursion.rs`
(composition/state independence, cycles, diamond/alias charge-once with tight
`with_bounds` sums, the eight/nine-edge depth boundary with cache-order
independence, the deeper-than-nine horizon with exact-sum/one-short charging
and pure deepening of horizon-cut lattices, deterministic Flate/raw failed-
attempt exhaustion with partial-cache preservation, admission parity, budget
exhaustion, end-to-end transaction boundary); the two historical
nested-refusal pins in
`alias_epoch_form_xobjects.rs` were assertion-adapted to keep a still-refusing
variant (transparency-`/Group` child) and pin the new proven outcome.

## T189 - Root Form local Image and stencil colour-effect admission

### Mechanical split before growth

The 1,144-line `form_xobject_effect.rs` was split BEFORE the new behaviour
landed, zero-behavior-change: the cohesive T188 Device-colour implementation
(the decoded-name `ColorAuthority`, `/Default*` and literal poisoning, the
ephemeral `ColorSpaceResource` projection, `CS`/`cs` selection and named-setter
validation, and the `/ColorSpace` half of the canonical-authority proof) moved
into private submodule `form_xobject_effect/color.rs` behind small `pub(super)`
seams. The parent retains the analyzer orchestration, the exact-identity
cache/budgets, the semantic dictionary preflight, the closed raw grammar, the
single seeded paint walk, and the GENERIC semantic-name/dictionary helpers
(`canonical_unique_authority_entry`, `malformed_name_may_hide`, and the
generalized `canonical_form_resources_entries` both authorities now share). All
existing T187/T188 assertions pass unchanged; no public module or re-export was
added.

### One new private authority family

`form_xobject_effect/xobjects.rs` adds the slice's ONE new domain abstraction:
private `FormLocalXObjectAuthority`, a deterministic
`BTreeMap<decoded name, Option<(PageXObjectResourceTarget, PageXObjectEffect)>>`
plus a literal-poison set and a namespace poison flag. It is built lazily, at
most once per analyzed Form, and ONLY when a syntactically valid `Do` is
present (the raw grammar now admits `Do` outside an open path with exactly one
name operand; everything else still refuses). `Some((target, effect))` is one
unambiguous exact typed binding; per-name `None` is collision/named-skip/
uncertain-target poison; a nameless uncertain skip, an ambiguous
`/Resources`/`/XObject` authority, or more than 256 classified-plus-skipped
facts poisons the whole namespace before the writer map is trusted.
`/Subtype /Form` targets are RETAINED with their full target tuple for the
later bounded-recursion slice — but only after the SAME exact-identity
corroboration the Image path applies (reference/generation/reached-offset
re-resolution, an exact reinspected object header, canonical semantically
unique `/Subtype /Form`); an uncorroborated Form target degrades to per-name
poison, and every invoked nested Form refuses in this slice either way. The
authority keeps no source bytes, dictionaries, streams, tokens, or image data
and is dropped once the fixed two-bit/Unknown result is cached.

### Intrinsic Image/stencil dependency rule

Per ISO 32000-1 §8.9.5, an ordinary `/Subtype /Image` (`/ImageMask` absent or
direct `false`) interprets its OWN samples and never reads the current
graphics-state colour: it is neutral to both lanes. Per §8.9.6.2, an
`/ImageMask true` stencil uses the CURRENT nonstroking colour for marking. At a
proven stencil `Do`, the walk reads the live pre-invocation nonstroking lane,
consuming the inherited root only while that lane still equals the source-less
sentinel — a prior direct setter, Form-local `cs`/`sc`
selection, or unrestored local frame kills the consumption, and `q`/`Q`
restoration re-exposes it exactly. Stroking is never consumed by a stencil.
`/Mask`, `/SMask`, `/Decode`, `/Interpolate`, `/Intent`, and a JPX-style
missing `/ColorSpace` affect coverage/sample interpretation only and are
neither decoded nor promoted to a colour-lane read.

### Authority boundary and bounds

Structural facts come from the existing `inspect_form_xobject_resources`
inspector (no new or changed public PDF API); its raw-key result is never
writer authority by itself. Before any name resolves, the Form's own
`/Resources` and `/XObject` keys must be canonical and semantically unique in
source-addressable dictionaries (direct or one exact in-use indirect
`/Resources` target; never page/caller fallback, never merged scopes). Every
Image target is re-corroborated before trust: reference/generation/reached
offset through the same exact-identity check the root uses, an exact
reinspected target dictionary, canonical semantically unique `/Subtype /Image`
and `/ImageMask` (including exact absence), and proven semantic absence of
`/Alternates`, `/OPI`, `/OC`, `/F`, and `/Ref`. Retained Form targets share
the identity corroboration and the canonical-unique-subtype proof (the
inspector compares neither the parsed object header with the resource
reference nor escaped subtype aliases); their deeper dictionary preflight
stays with the recursion slice's analysis entry point. The shipped
`page_xobject_policy::classify_image` ordinary/stencil classifier is reused
through crate-private visibility widening rather than forked; because its
metadata is raw-key structural, a `Stencil` verdict additionally corroborates
canonical semantic uniqueness of `/Width`, `/Height`, `/BitsPerComponent`, and
`/ColorSpace`. Decoded-name equality uses the shared bounded writer-local PDF
name decoder; malformed relevant spellings keep literal poison and unrelated
malformed names stay isolated. Declared-but-uninvoked targets never force
analysis or poison unrelated names; demand stays at valid `Do` operations.

### Recursion deferral and unchanged boundary

No recursion, active/in-progress set, cycle detection, depth budget,
child-effect composition, or cache-publication change landed: an invoked
nested Form is Unknown for the entire root Form, and a positive prefix never
survives an unsupported suffix. The `(IndirectRef, reached_offset)` cache key,
post-`compute` publication, cached Unknown, 256 first-seen target cap,
aggregate decoded-byte budget, root proxy/safety-key preflight, raw/single-
Flate decode, Group absence, q/Q/path truth, and the T188 colour rules are all
unchanged. Only existing eligible page setter bytes may change; Form, resource,
and image objects remain read-only, byte-identical, and appear exactly once in
output. Focused coverage lives in `tests/alias_epoch_form_xobjects.rs`
(intrinsic lane effects, authority/poisoning/cap matrices, cache/parity, and
end-to-end page-only mutation); the shared classifier's page-level behaviour is
locked by the existing xobject policy tests.

## T188 - Root Form local-device colour effect admission

### Scope and one abstraction

This slice extends the EXISTING request-scoped `FormXObjectEffectAnalyzer`
(`form_xobject_effect.rs`) across a narrow set of explicit Form-LOCAL device
colour operations; it adds NO new domain abstraction and changes NO public
surface. The analyzer's `analyze(input, lookup, reference, reached_offset)`
shape, its `(IndirectRef, reached_offset)` cache identity, cached Unknown, the
256 first-seen target cap and the aggregate decoded-byte budget are all
preserved, and cache entries remain the fixed two-lane effect only. A Form
without any `CS`/`cs` resource-colour operator is byte-for-byte the T187 result:
the empty `ColorSpaceEnv` walk is unchanged and no resource inspection, decode,
or projection runs. `PageXObjectPolicy` remains the sole page XObject-name
authority; the outer `Do` still folds the two-bit effect through its existing
collision/skip poisoning and the already-live `AnalyzedForm` consumer, so the
only possible public-byte difference is an already-eligible page setter newly
authorized by a proven effect.

### ISO initial-colour and the two-layer machine

Per ISO 32000-1 Table 74, `CS`/`cs` selects the colour space AND sets that
space's initial current colour (DeviceGray `[0]`, DeviceRGB `[0 0 0]`,
DeviceCMYK `[0 0 0 1]`). A proven local selection therefore kills ONLY its
selected inherited lane even without a following setter, and the resulting local
colour carries concrete operator provenance so it can never equal T187's
source-less inherited sentinel. `q`/`Q` still saves and restores both lanes
exactly, so `q; cs; fill; Q; fill` consumes the RESTORED inherited lane.

The raw preflight stays grammar/refusal only: it now ADMITS `CS`/`cs` (outside an
open path, exactly one syntactic name operand) and `SC`/`SCN`/`sc`/`scn` (outside
an open path, at least one finite numeric operand, no trailing Pattern name), but
proves neither semantic authority nor arity. The single
`PaintProgram::ops_with_initial_state` walk remains the sole colour-state
interpreter. Because `PaintOp.state` is post-operator, the walk compares each
setter's PRIOR snapshot lane against its post lane: a named setter is admitted
only over a proven local supported Device lane (a `Device*` space carrying a
resource name — never the resource-name-less inherited sentinel or a direct
device setter) with exact arity Gray 1 / RGB 3 / CMYK 4. Selection corroboration
requires the projected Device family, exact raw operand spelling, ISO initial
components, and the selecting record's concrete source. Setter corroboration
rejects the source-less inherited sentinel first, then requires the prior local
lane's family/name to survive unchanged and the setter to stamp its own record
source. The CMYK-shaped inherited sentinel therefore never makes a
four-component `SC` admissible. A `CS`/`cs` whose resolved post lane is not a
supported local Device lane (an unresolved `Resource(name)`), any
inherited/unsupported/wrong-arity/Pattern setter, and every unsupported suffix
all return Unknown.

### Decoded-name resource authority

When a `CS`/`cs` operator is present, a bounded analyzer-private decoded-name
projection is built ONCE from the Form's OWN `inspect_form_color_space_resources`
(never page fallback). Before a missing `/Resources` or `/ColorSpace` fact may
prove `/Default*` absence, a raw authority gate requires any present authority
key to be canonical and semantically unique; direct resources and exact
source-addressable indirect resource dictionaries are covered, while escaped,
duplicate, malformed, unresolved or otherwise ambiguous authority is Unknown.
The existing inspector continues to own colour-space value classification.

Two separate 256 limits apply: total reported colour-space-plus-skip facts are
capped before any writer authority map is allocated, and distinct raw `CS`/`cs`
operand spellings are capped before the ephemeral environment can grow beyond
256 entries. Exceeding either is Unknown. `ColorSpaceEnv` uses raw-name equality
and is never the semantic authority: each distinct raw operand spelling actually
used is decoded and resolved through the projection, and only a proven supported
Device family yields an ephemeral `ColorSpaceResource` whose name matches that
raw spelling for the one walk. Supported selections are the
canonical direct `/DeviceGray`/`/DeviceRGB`/`/DeviceCMYK` (reserved selectors
that cannot be shadowed by same-named resource keys) and unique Form-local
aliases whose value classifies DIRECTLY as one of those families (no
alias-to-alias chains). The matching `/DefaultGray`/`/DefaultRGB`/`/DefaultCMYK`
binding must be proven absent; presence, an unclassifiable skip, or uncertainty
makes that family Unknown, while canonically absent `/ColorSpace` proves
absence. Decoded semantic duplicates poison the invoked name. Undecodable
classified/skipped resource names retain their literal spelling as poison for a
decoded operand that collides with it; unrelated malformed names remain
isolated, and a malformed prefix that could hide a `/Default*` poisons only the
possible matching families. A named skip poisons its decoded name; a nameless
uncertain skip (or the fact cap) poisons every selection. Cal/ICC/Lab/Indexed/
Separation/DeviceN and Pattern all refuse.

### Retained boundary and deferral

The entire T187 boundary is load-bearing and unchanged: exact identity, the
`/F` `/Ref` `/OC` `/OPI` and canonical safety-key dictionary preflight, proven
Group absence, raw/single-Flate decode and byte budget, and `/Matrix`/`/BBox`
non-modelling (this makes only a colour-lane claim). Nested `Do`, ordinary
images, stencil/image masks, inline images, shading, Pattern execution, `gs`,
text/Type3, and every other resource operation still refuse. Recursion, image and
stencil composition remain a deliberate later medium slice.

## T187 - Root Form inherited-colour effect admission

### Colour-dependency authority boundary

This slice extends the page alias-epoch proof across a narrow class of root-page
Form `XObject` invocations WITHOUT writing a single Form byte. It makes only a
colour-lane-dependency claim: does painting inside a demanded root Form consume
the caller's inherited stroking and/or nonstroking colour? A proven consuming
Form lets the outer `/Fm Do` close only the matching page alias lanes; every
Form dictionary/stream byte, page ownership, root closure, dry-run, transaction,
append-only prefix and reopen behaviour stay byte-identical. No Form object,
resource binding, clone, or allocation is ever authored, so the future Form
ownership/mutation problem is deliberately untouched.

`/Matrix` and `/BBox` are unmodelled and make no conformance claim: a
non-identity `/Matrix` cannot change which colour lane a path paint reads, and
`/BBox` clipping can only suppress paint, so ignoring both may at worst
over-report consumption (a visually inert positive under the existing colour
route) and can never fabricate an unsafe false Neutral. Matrix/BBox need a later
geometry/bounds/render consumer; a Type3 substrate needs its own
string/Encoding/CharProc execution vertical.

### One abstraction: the request-scoped analyzer

The single new domain abstraction is the private `FormXObjectEffectAnalyzer`
(`form_xobject_effect.rs`). It is request-scoped and shared across every selected
page. It caches `Option<[bool; 2]>` (stroking-first lanes; `None` = Unknown)
keyed by the EXACT tuple `(object number, generation, reached object byte
offset)`; map presence distinguishes a cached Unknown from an unseen target, and
both positive and refused results are cached. A fixed 256 first-seen target cap
and ONE aggregate decoded-byte budget (`MAX_CONTENT_STREAM_BYTES`) bound the
whole request, not each page; exhaustion deterministically caches Unknown.
Corroboration re-resolves the reference through the current request
`ObjectLookup` and requires an in-use, source-addressable object whose
generation and byte offset match; compressed, free, missing, ambiguous or
out-of-range targets are Unknown.

### Semantic dictionary gate, then two content layers

Immediately after exact identity corroboration, and before filter
classification, extent discovery, slicing or decoding, the analyzer performs one
top-level dictionary preflight using decoded PDF-name equality. Undecodable keys
are Unknown. Any semantic `/F`, `/Ref`, `/OC` or `/OPI` refuses under canonical
or escaped spelling: external stream data, imported-page substitution, optional
visibility and OPI substitution are all outside the admitted execution model.
The raw-key-delegated safety keys `/Group`, `/Length`, `/Filter` and
`/DecodeParms` must be canonical and semantically unique, so an escaped alias or
duplicate cannot evade the existing group, stream-extent or filter inspectors.
Those inspectors continue to own the corresponding value semantics.

Neither layer may be omitted. Layer 1 tokenizes/assembles the demanded Form once
and runs a strict CLOSED raw-record preflight: the allowlist is `q Q`, `cm`,
direct `G g RG rg K k`, path construction `m l c v y h re`, clipping `W W*`, and
path paint/end `S s f F f* B B* b b* n`, each with exact arity, finite `f64`
numeric operands, and a path/`q`-stack context grammar (§8.2); `m`/`re` open or continue,
`l/c/v/y/h/W/W*` require an open path, paint/`n` require and close it, and the
stream end requires no open path and balanced `q/Q`. Every other operator —
resource colours `CS cs SC SCN sc scn`, line/text state, `ri`, `gs`, text/show,
`Do`, `sh`, inline images, `BX/EX`, marked content, `d0/d1`, unknowns — refuses,
so a positive prefix never survives a later unsupported record. The raw pass is
the ONLY validator for the path-construction/clipping operators the walker
collapses to no-ops.

Layer 2 runs exactly one `PaintProgram::ops_with_initial_state` walk seeded with
distinct source-less inherited stroking/nonstroking sentinels (`source: None`).
The walker owns `q`/`Q`, direct setter lane kills, and finiteness; each PathPaint
consumes a lane only when the live colour still equals its sentinel. Because
every valid direct setter stamps a concrete source range, even a numerically
sentinel-valued setter cannot recreate inheritance. Transparency grouping is
proven absent through the existing `inspect_form_transparency_group` (accepting
only `group.is_none()` AND `skipped.is_empty()`); only raw or single
default-predictor `/FlateDecode` bodies decode (bounded by the remaining
aggregate budget, dropped after caching the two-bit effect); everything else is
Unknown.

### Decoded-name integration and outer Do

`PageXObjectPolicy` stays the SOLE decoded semantic-name/collision/skip map. Its
private effect family gains `AnalyzedForm { consumes_stroking,
consumes_nonstroking }`; the existing `new(report)` structural constructor still
refuses every Form and is retained for focused tests and fail-closed callers.
The production constructor stores exact unresolved Form targets in that same
map. There is no demand-name collection pass or `BTreeSet<Vec<u8>>`: after the
single page walk reaches an outer `Do` and passes the invalid graphics-object
context check, the policy resolves that one entry through the request analyzer
and replaces it with the proven effect or refusal. Repeated names and aliases
reuse the exact analyzer cache; declared-but-uninvoked Forms are never analyzed;
decoded-name collision, malformed-name, named-skip and page-wide poisoning all
win before resolution and can never be overridden. A neutral analyzed Form
leaves alias roots live; known lane effects call the existing
`AliasEpochPlan::consume` for only those lanes; structural/unknown Forms keep the
historical `XObjectInvoke` refusal; ordinary image/stencil behaviour is
unchanged.

### Plumbing

The private sequence edit callback changed from `Fn` to `FnMut` and now receives
the existing Copy `ObjectLookup`; every sequence argument/order, the preflight
`Fn`, and the sole production caller are otherwise preserved. The conversion
request builds ONE analyzer and threads `input`/`lookup` into the page
conversion closure, which builds the lazy production policy around that shared
request cache. No public request/output/report shape, `EpochRefusalReason`, skip
reason, `CandidateKind`, serde, digest, or PDF/paint API changed. File sizes stay
well under the gate.
