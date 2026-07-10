# presslint-write Journal

Earlier entries are preserved in [JOURNAL-archive.md](JOURNAL-archive.md).

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
