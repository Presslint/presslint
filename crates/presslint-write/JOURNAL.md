# presslint-write Journal

Earlier entries are preserved in [JOURNAL-archive.md](JOURNAL-archive.md).

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

## T186 - Page font policy and ordinary TextShow alias admission

### Mechanical split

The logical page-sequence pipeline moved from `content_edit_pipeline.rs` into a
new `content_sequence_pipeline.rs` with no behaviour change: `PageSequenceEdit`,
`PageSequenceOutput`, `PreparedSequenceObject`,
`edit_page_content_incremental_sequence`, the advisory report-index logic and
tests, sequence-object preparation, and the sequence-only advisory joins all
moved. The raw/indexed per-stream pipeline and the low-level write/encode
helpers stayed in the source module and are reused behind the smallest `pub`
seams in the private module (`select_indices`, `lookup_from_backend`,
`write_dirty_objects`, `merge_duplicate_dirty_objects`, `find_direct_length`,
`classify_filter`, `whole_stream_boundary`, `MAX_CONTENT_STREAM_BYTES`). The
three per-report indexes collapsed into one generic `PageReportIndex` over a
small `PageIdentityReport` trait; the exact XObject join behaviour is unchanged.
Post-ablation split sizes: source 607, destination 666, both well under the
gate. The destination no longer carries disabled preflight states that its only
entry point cannot construct, and the shared report-identity trait exposes one
exact identity tuple rather than three repeated accessors.

### One policy, one authority boundary

The single new abstraction is the private `PageFontPolicy`. It maps the exact
identity-matched page `/Font` and `/ExtGState` reports once into borrowed
`FontEnv`/`ExtGStateEnv` views plus an independently corroborated ordinary-font
admitted set keyed by `(object number, generation, reached object offset)`. The
page report is joined by full page identity (ordinal, leaf reference, page
object byte offset); a missing, duplicate, mismatched, or malformed report is
unknown/refusal, never a vector-index guess. Named `Tf` bindings represent every
selectable root exactly as the environment permits (Type3 included), but the
admitted set contains only exact indirect Type1/MMType1/TrueType/Type0
identities. Repeated safe facts converge; any safe/unsafe/conflicting fact for
one tuple (Type3, CID descendant, inadmissible target, non-`/Font` type) poisons
it fail-closed. The admitted-set check is corroboration only: it queries no
object lookup, reopens no font body, and reclassifies nothing. Content
ownership, sequence localization, root closure, and the page transaction remain
the only mutation authority. A failed font inspection is advisory — it makes the
policy unknown and refuses TextShow but adds no page-skip reason.

### All-environments walk and the Tr 0-7 truth table

The converter now builds the single logical-page walk with
`PaintProgram::with_all_envs`, feeding the policy's `ExtGStateEnv` (neutral seven
parameters, mapped `/Font` directive) and `FontEnv`. `AliasEpochPlan` requires a
`ResolvedIndirect` snapshot admitted by the policy before treating any of the
four text-show operators as a colour consumer, then consumes lanes by render
mode: `0`/`4` nonstroking, `1`/`5` stroking, `2`/`6` both, `3`/`7` neither (no
consumer, no splice). Modes 4-7 stay paint's raw `Unsupported { value }` and are
interpreted writer-locally only; every other unsupported value refuses. An
unadmitted font (Type3, CID, direct-without-identity, stale/missing tuple,
raw/unset/indeterminate) keeps the existing `TextShow` refusal, because a
non-ordinary current font may execute glyph programs that paint. TextShow is a
consumer only and is never spliced.

### ExtGState semantic-name correction

The writer-local `gs` safety preflight now decodes the operand and the
classified/skipped resource names before comparison, requiring exactly one
classified semantic match and no matching skip. `/GS1` and `/GS#31` are the same
name, so an ambiguous duplicate, a matching skip, or a malformed operand fails
closed instead of first-winning a raw sibling. The policy applies the same
collision poisoning to the mapped `ExtGState` font directives. `presslint-pdf`
and `presslint-paint` are unchanged. A strictly undecodable classified or
skipped name retains its literal spelling as a poison key only when those bytes
equal the decoded operand; this covers permissive-reader ambiguity without
globally poisoning unrelated malformed declarations. A unique declaration
whose operand and declaration use different raw spellings passes the hardened
guard but misses paint's raw lookup, clearing font certainty — the deliberate
safe false-negative `Indeterminate` boundary.

### Review corrections (incomplete-coverage poisoning)

Two fail-closed gaps closed after review. First, the `gs` safety preflight only
consulted structural skips that carried a resource name; a namespace-level
(nameless) skip such as a duplicated `/ExtGState` key or a `/Resources`
inheritance diagnostic left the classified set partial while a `gs` naming a
surviving classified resource was still accepted. The preflight now treats a
nameless skip beside any classified resource as incomplete coverage and fails
closed as unclassified before matching; an empty classified set keeps its
existing unresolved outcome, so the locked guard results are unchanged. Second,
`PageFontPolicy` mapped its `ExtGState` `/Font` directives only from the
classified resources and never consulted the report's skips, so a same-name skip
left the surviving resource's directive as a positive `Select`. The policy now
poisons every mapped directive implicated by a skip: a named skip poisons its
decoded-semantic-name match, and a nameless skip poisons every mapped directive
so any `gs` clears font certainty. Both are the same fail-closed rule the
collision poisoning already applied, extended to the skip list; the policy
computes each mapped resource's semantic key once for both checks. Coverage was
also widened: all four text-show operators run under the full Tr 0-7 table,
plus `gs` `LeaveUnchanged`/uncertain directives, `Tf`/`gs` last-writer-wins and
convergence, `q`/`Q` restoration, `BT`/`ET` font persistence, an open-path
invalid-context refusal, end-to-end mode 4/7 byte preservation, and the two new
incomplete-coverage regression fixtures. The cross-stream test harness now gives
each physical content stream a distinct indirect object.

### Deferrals

Type3 CharProc/`d0`/`d1`/resource/recursion semantics and writer admission,
Form descent and Form-local fonts, ordinary Form `/Matrix`/`/BBox`/group resets,
and a global `presslint-pdf`/`presslint-paint` semantic `ExtGState` operand
change all remain out of scope. No public enum, report, serde, digest, identity,
capability, action, selector, CLI, or skip-reason surface changed.

## Paint `SetFont` enum adoption

`AliasEpochPlan` now handles semantic `SetFont` directly as colour-neutral text
state with the same graphics-object-context check previously applied to raw
`Tf`; `Tf` is no longer in the `NoOp` allowlist. `TextShow` remains an
unconditional live-epoch refusal. No conversion candidate, telemetry, policy,
resource interpretation, or admission reach changed.

## T180 - Page named-image and stencil alias admission

One bounded shallow page-XObject inspection now runs per conversion request
through the already-open object lookup. Its report pages are indexed by exact
leaf reference; duplicate references are poisoned, and a match must also agree
on object byte offset and document ordinal. The matched XObject report travels
as a separate callback fact and does not extend the page colour-facts model.

The new private `PageXObjectPolicy` builds one deterministic semantic-name map
per analysed page. PDF `#xx` escapes are decoded for both report keys and `Do`
operands; malformed escapes fail closed, names remain case-sensitive, decoded
collisions poison the semantic name, and a named structural skip overrides a
same-name target. Page-scoped inspection gaps make every name unknown, while an
unused malformed declaration affects no unrelated invoked name.

Named ordinary images (`/ImageMask` absent or false) are neutral to current
graphics-state colour. A `/ImageMask true` target consumes only the
nonstroking alias lane when width and height are positive, BitsPerComponent is
absent or exactly 1, and ColorSpace is absent. Invalid images, Forms, missing
or unknown names, and inline images remain refused; invocation inside a text
or open path object refuses before XObject classification.

`Do` creates no conversion candidate. The existing source selection and
setter candidates, root/shared-record closure, component dry-run, direct
conversion order and counts, reconciliation, edited-page validation, staging,
selector decisions, page atomicity, and append-only source-prefix invariant are
unchanged. Policy construction retains only small structural facts and name
buffers; no image stream or pixel data is copied or decoded.

## T179 - Root-Atomic Page-Alias Conversion

Finished `AliasEpochOutcome` values are now consumed directly after the one
logical-page `PaintProgram` walk. A root is initially executable exactly when
its status is `Closed`, it has a supported consumer, and it retains a route.
Every candidate in such a root is dry-run through one shared component path:
exact neutral-black preservation returns `[0,0,0,1]`, otherwise the already
prepared DeviceLink is applied once. The direct-device path uses the same
component seam and canonical serializer, preserving its bytes, identical
K-only no-splice rule, attribution, counts, and leave-verbatim apply failure.

No alias splice is staged until every candidate in its root succeeds. Both an
alias selection (including its PDF initial colour) and an explicit numeric
setter are replaced as a whole record by canonical destination `g/G`, `rg/RG`,
or `k/K` bytes. Candidate execution uses only retained family, lane, route,
components, occurrence, and local range; it does not tokenize, walk, resolve
resources or defaults, or evaluate selectors again.

Root atomicity is followed by physical-record atomicity. Candidates key by
content object plus local range. Structurally refused, closed-no-consumer,
missing-route, inconsistent, or transform-failed roots seed one deterministic
queue traversal over the root-record graph. Non-executability propagates to a
fixed point through co-tenant records, including transitive components. A
unique closed-no-consumer root stays a silent no-op; when it shares a record
with a consumed root, the shared component remains verbatim.

Only fully executable components append splices to the existing occurrence
plans. Repeated occurrences still require identical complete plans through
`PageContentSequence::reconcile`, and the unchanged edited-page validation,
temporary encoding/dirty-object staging, and append transaction remain the
page-atomic publication gate.

`ConvertedPage` adds zero-omitted
`resource_alias_candidates_converted` and
`resource_alias_candidates_refused`. Both count unique physical retained
candidate records. Successful non-black records also feed existing total/link
operator counts; black-overlay records feed `black_preserved`. Structural alias
setter counts keep their earlier per-setter meaning even inside refused roots.

Conservative refusals remain: font-unaware text showing, unclassified `Do`
invocations (forms, images, and stencils), inline images, Type3 operators,
patterns and other non-device resource spaces, and recursive form conversion.

## T178 - Closed Page-Alias Epoch Proof and Refusal Plan

One new private abstraction, `AliasEpochPlan`, sits between the per-setter
structural alias classification and the first alias conversion. It observes
EVERY successfully walked op of the existing single logical-page paint walk —
before the converter's colour-only branch — with no second tokenization,
assembly, replay, or paint API change. The pre-op snapshot is now seeded
explicitly from the exact PDF page-default graphics state; each later op
carries the previous shared post-op snapshot by `Rc` reference.

An epoch starts when an exact ELIGIBLE `/Alias cs`/`/Alias CS` selects a
classified page device alias on one lane. Selecting the space resets the lane
colour (ISO 32000-1 §8.6.3), so the selection record itself is the epoch's
first symbolic candidate carrying the exact initial source tuple (`[0]`,
`[0,0,0]`, `[0,0,0,1]`), retained untransformed even when a supported path
paint consumes it before any explicit setter. Each epoch pairs symbolic
source state (alias identity, device family, walker-carried tuple) with
symbolic emitted state (the ONE prepared route fixed at selection:
destination family plus link identity). Each live branch carries its own
pending source tuple; an explicit eligible setter becomes the branch's new
pending value. `q` copies both lane pairs — tuple included — and `Q` discards
frame-local branches and restores the saved pairs exactly, so a paint after
`Q` is proved against the RESTORED tuple, never against alias name and
family alone. Every `q`-derived branch carrying a root epoch is proved or
refused atomically with that root, a nested alias selection is an
independent epoch, and independent root epochs never poison each other (even
under the same alias name).

A root epoch closes only when every candidate passes: exact source
family/shape, canonical selector match on its SOURCE facts (one excluded
candidate refuses the complete root, never a prefix), one prepared route,
source+destination raw-device `/Default*` safety, whole-record localization
to one physical occurrence with in-place ownership, repeated-reference
consistency (identical candidate/no-candidate decisions and identical
symbolic facts for every occurrence of the same physical record — EVERY root
relying on a record is accumulated, one divergent occurrence refuses them
all, and the record stays permanently poisoned for any later root), strict
operator/consumer admission, and page-end closure at zero `q` depth. Path
paint consumers map exactly (`S`/`s` stroke; `f`/`F`/`f*` fill; `B`/`B*`/
`b`/`b*` both; `n` neither); `sh` is neutral to the current colour. A closed
epoch with no consumer is a private no-op candidate and authorizes no byte
change. Native transform construction and the LCMS apply are deliberately NOT
proven: the dry-run of every retained candidate plus the atomic conversion is
the next slice.

Fail-closed boundaries while an alias is live anywhere (current lanes or
saved frames): text showing regardless of `Tr` (no effective font/subtype in
the snapshot; Type3 glyphs inherit state), `Do`, `BI`/`ID`/`EI`, `d0`/`d1`,
`BX`/`EX`, Pattern-name or otherwise ineligible/unclassifiable setters inside
an active epoch, unknown/non-allowlisted operators, known-invalid
graphics-object placement per ISO 32000-1 §8.2 (both the text-object AND the
path-object lifecycle are tracked: unbalanced `BT`/`ET`, `EMC` underflow,
`q`/`Q`/`cm`/path/shading operators inside a text object, path continuation
or clipping without an open path, any non-path operator — colour selection
and setters included — inside an open `m`/`re` path object, and a text or
path object left open at page end), and any raw-operator/plan/snapshot
disagreement. The colour-neutral allowlist admits exactly: path
construction/clipping, `sh`, line/rendering parameters, `cm`, `gs` (after the
existing whole-page ExtGState/transparency-group preflight), `Tr`, text state
and positioning without showing, and marked-content delimiters/points — each
only in a graphics-object context that admits it. `Q`
underflow remains the existing whole-page walk failure; a non-empty shadow
`q` stack at page end refuses EVERY alias plan for the page (§8.4.2 requires
balance), while structural counts keep their per-setter meaning.

The plan replaces the isolated per-setter tally path as the SOLE production
source of `resource_alias_setters_eligible/ineligible`, delegating to the
unchanged `PageDeviceSpacePolicy::classify_alias_setter`, so both fields keep
exactly their prior structural per-setter meaning (a structurally eligible
setter may sit inside a refused epoch). The policy gains one crate-private
exact-name `alias_decision` lookup; classification and `/Default*` authority
are unchanged. Closed/refused epoch status stays crate-private (no public
count, refusal list, serde, CLI, or report change) until a converted alias
byte exists. Direct-device conversion bytes, decision order, skip/link
counts, page atomicity, selector vocabulary, and the append-only prefix are
byte-for-byte unchanged.

KNOWN CONSERVATIVE LIMITS (deliberate): every text show is a refusal
boundary (no font inspection), every `Do` refuses a live epoch (no
form/image classification), inline images and Type3 metrics always refuse,
and a boundary refuses a root even while its only live branch is suspended
in a saved frame. Later slices relax these case by case.

## T177 - Page Device-Space Policy and Default-Colour Interlock

One new private abstraction, `PageDeviceSpacePolicy`, is built once per
analysed page: exact page DeviceGray/RGB/CMYK alias facts from the page
`/Resources /ColorSpace` classification, plus one
`Absent | Identity | Replaced | Unknown` status per device family from the
`/Default*` classification. The two structural document inspections run ONCE
per conversion request through the already-open `ObjectLookup`; their failure
is advisory and never becomes a page or pipeline skip.

Report pages join to content pages by EXACT leaf `IndirectRef` equality,
corroborated by page object byte offset and document ordinal, never by
compacted report vector position (inspectors may omit failed leaves). A
missing, duplicate, or inconsistent match degrades to unknown facts.

The converter's single paint walk now receives the policy's borrowed
`ColorSpaceEnv` instead of the always-empty one, so exact page device aliases
resolve in graphics state across ordered `/Contents`, `q`/`Q`, and carried
colour state with no paint or PDF API change. Only aliases whose OWN
classified terminal family is exactly a device family with component count
1/3/4 enter the environment, and only while that family's default status is
`Absent` or `Identity`; built-in names, abbreviations, and the `/Default*`
keys are never alias candidates and can never shadow the built-ins. A
duplicate alias name is retained only as an ineligible decision and never
enters the environment.

Exact numeric `sc`/`SC`/`scn`/`SCN` setters under classified page device
aliases are counted READ-ONLY as `resource_alias_setters_eligible` /
`resource_alias_setters_ineligible` (additive serde fields, omitted at zero).
Eligibility requires the eligible alias decision, side/case agreement, the
resolved policy family, exact component count, single finite numeric operand
tokens in `[0,1]` with no trailing name/Pattern operand, and a record range
that localizes to one physical occurrence. No alias, selection, or setter
byte changes; no selector, routing, black-preservation, or LCMS dry-run is
performed for aliases. The selected alias comes from the existing
pre-operator paint snapshot, so a distinct trailing Pattern name is still
counted as an ineligible setter rather than becoming uncounted.

BEHAVIOUR CHANGE (fail-closed safety correction): a selected + routed direct
`g/G`, `rg/RG`, `k/K` conversion now proceeds only when BOTH the source and
the emitted destination device family are proven `Absent` or `Identity` for
their matching `/Default*`; otherwise the operator stays byte-verbatim and is
counted in the additive `OperatorSkipCounts::default_color_space_unsafe`.
`MissingResources` alone proves absence; a general `Resources` failure
poisons all families and takes precedence over it; a family-specific
malformed/duplicate/unresolved/unclassified default poisons only its family.
The interlock runs after selector and route (which keep their old counts) and
before black preservation and the LCMS apply; safe families on the same page
still convert. Public request, selector vocabulary, ownership, append-only
prefix, page atomicity, and all zero-count serde shapes are unchanged.

DEFERRED (closed-epoch contract for the next slices): no alias conversion, no
`cs/CS/sc/scn` rewrite, no synthetic initial-colour insertion, no zero-width
splice, no resource mutation or destination resource creation, and no form
descent - the read-only eligibility counts are the input for the future
closed alias-epoch proof/refusal abstraction and its dry-run/atomic apply.

## T176 - Exact Logical Page Content

The direct-device converter now treats an ordered page `/Contents` array as one
exact decoded byte sequence. Private `PageContentSequence` owns that logical
buffer, its single global token and operator-record vectors, and ordered
physical occurrence spans. The superseded per-stream `ParsedContent` path is
removed. No separator is synthesized: occurrence boundaries may cross
whitespace or comment trivia (including CRLF), but a boundary strictly inside
any other lexical token refuses the page.

Paint interpretation and the ExtGState guard consume the global records, so
operands, `q`/`Q`, graphics state, and `gs` may cross physical streams. Selected
replacement ranges must localize wholly to one occurrence. Complete occurrence
plans for a repeated indirect object must be identical, including edit versus
no edit; identical plans collapse to one physical rewrite and count.

The converter path is page-atomic. It decodes every unique physical object once,
checks the aggregate logical size including repeated occurrences, stages local
splices and reports, rebuilds and globally parses/walks the edited sequence,
then encodes and constructs all dirty objects before publishing the page.
Readable ownership-vetoed objects still participate in interpretation while
their own edits and tallies remain suppressed. The public request, report,
selector, serde, CLI, append-only prefix, and ownership policy shapes are
unchanged. Cross-occurrence replacement itself remains intentionally unsupported
and fails closed.

## T175 - Parse-Once Paint-Driven Direct Converter

The shipped direct-device colour converter now discovers candidates exclusively
through the shared `presslint-paint` interpreter. Its private tokenize/assemble
pass and numeric operand interpreter are removed; the crate gains direct
dependencies on `presslint-paint` and `presslint-inventory`.

One new private abstraction, `ParsedContent`, borrows the decoded stream bytes
and owns the tokens plus assembled operator records, produced once with a
byte-identical token-serialization check before the converter callback runs.
The pipeline keeps its raw byte callbacks source-compatible behind the same
loop; only the converter-facing path receives parsed data. The independent
post-edit round-trip validation of edited bytes is unchanged.

Eligibility is exact-shortcut-bytes only: a colour event converts only when the
bytes at its operator range are `g`/`G`, `rg`/`RG`, or `k`/`K`, and the splice
uses the event's record range and already-parsed components. Resource colour
operators (`cs`/`CS`, `sc`/`scn`, `SC`/`SCN`) and payload text resembling a
shortcut stay byte-verbatim; no resource-space conversion is added. Route
selection, `[0,1]` range validation, black preservation, number serialization,
descending splices, prepared-link reuse, ownership veto, and the whole-page
ExtGState/transparency-group guards are behaviorally unchanged for successfully
walked streams.

FAIL-CLOSED TIGHTENING: any graphics-walk error (malformed operands of ANY
supported operator, stack underflow) now refuses the entire physical stream
through the existing round-trip mismatch skip, discarding every candidate found
before the error. The per-operator `wrong_operand_count` / `non_number_operand`
skip counts remain in the report shape but stay zero.

The write-local recursive selector evaluator is removed. After the existing
total unsupported-leaf precheck, targeting delegates to the canonical
`presslint-selectors` matcher over a private, ephemeral single-observation
entry whose non-colour fields are inert sentinels the accepted selector subset
cannot observe; a differential truth table pins adapter == canonical == the
prior semantics for every supported leaf.

KNOWN LIMITATION (pinned): multiple `/Contents` streams are still walked
independently, so a `q` in one stream with its `Q` in the next conservatively
refuses the second stream; explicit shortcuts remain independently convertible.
A logical concatenated page-stream walk is a follow-up slice.

## T174 - Document-Wide Content Ownership Veto

Content-stream edits now build one bounded object-consumer index from the same
immutable document-access snapshot used by the request. Exact direct page
`/Contents` occurrences remain the positive ownership proof; typed document
users are a separate completeness and exclusivity veto and are never treated as
immediate owners.

The snapshot fails closed for any truncation, unresolved edge, or scan skip that
can hide reachable edges: newest-trailer dictionary, catalog dictionary, body
scan, and reference-shape skips. Unreferenced-entry resolution diagnostics,
unreferenced objects, and object-stream cache drop/redecode facts do not poison
the proof. A missing target entry or any root, root-key, trailer-key, or other
page user refuses in-place mutation through the existing ownership skip.

The index deduplicates traversal paths per typed page user. The proof therefore
establishes confinement to one page user, not strict edge multiplicity within
that page subtree. Duplicate direct `/Contents` occurrences retain their direct
occurrence count and existing unique-owner behavior.

The traversal runs once per edit request and retains only direct owners, page
identities, typed referrers, and a completeness bit. Successful append-only
outputs and deterministic ordering are unchanged; focused fixtures cover the
new second-page-subtree and root-key vetoes plus the global completeness matrix.
