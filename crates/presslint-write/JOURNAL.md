# presslint-write Journal

Earlier entries are preserved in [JOURNAL-archive.md](JOURNAL-archive.md).

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
