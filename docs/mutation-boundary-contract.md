# Mutation Boundary Contract

`presslint-actions` emits mutation boundaries as report-only planning metadata.
The boundary says which source region or indirect object a future executor may
edit; it does not write bytes, serialize objects, encode streams, repair PDFs,
or create an incremental revision.

This contract is the first half of the F3 writer model. It freezes the public
shape a later incremental-update planner can consume while keeping current
action planning read-only.

## Purpose

A mutation boundary is the narrowest public description of an intended edit:

- content-stream operand replacement;
- dictionary key/value replacement or insertion;
- whole-stream replacement;
- private clone of a shared indirect object, plus the consumer reference patch.

The model carries stable source ranges, indirect references where they are
known, and the ownership decision required before touching a shareable indirect
object. It retains no PDF source bytes, decoded stream bytes, object bodies, or
profile payloads.

## Model

`MutationBoundary` is a serde-tagged enum using `kind` and `snake_case` variant
names.

`content_stream_operand` identifies an operand in a decoded content stream
operator record. It carries the page, content scope, record range, optional
operand/operator ranges, optional indirect-object ownership, and planned value
provenance. Live planning currently emits only this variant. Its ownership is
optional because inventory `ObjectId` values are content-derived identities, not
PDF indirect references.

`dictionary_entry` identifies a dictionary edit in a concrete indirect object.
It carries the target `IndirectRef`, key, replace/insert operation, value
locator, non-optional ownership decision, and planned value provenance. The
stored `PdfName` key bytes are slashless; slash-prefixed names are literal PDF
syntax only.

The `SetPageBox` action plans `dictionary_entry` boundaries through
`plan_set_page_box_boundaries`. Given the action, a leaf page `IndirectRef`, a
proven ownership decision, and an optional value locator per box, it emits one
`dictionary_entry` per requested-and-located box (`MediaBox` before `CropBox`).
The operation is derived from the locator: `existing_value` plans a
`replace`, `insertion_point` plans an `insert`. This is planning metadata only:
the helper never calls `presslint-write`, reads PDF bytes, or applies an edit.
The actual byte writing lives in `presslint-write::set_page_boxes_incremental`,
which independently re-derives the same boundaries from
`presslint_pdf::inspect_document_page_boxes` and appends a classic incremental
revision.

`whole_stream` identifies replacement of an indirect stream object's payload. It
carries the target `IndirectRef`, optional existing stream-data range,
non-optional ownership decision, and planned value provenance.

`indirect_object_clone` identifies a private-copy rewrite for a shared indirect
object. It carries the shared source object, the consumer being redirected, the
new-object allocation intent, a boxed boundary describing the consumer-side
reference patch, the ownership decision for the source, and value provenance.

The supporting contracts are:

- `DictionaryEntryOp`: `replace` or `insert`;
- `DictionaryValueLocator`: `existing_value` with key/value ranges, or
  `insertion_point` with the dictionary range;
- `PlannedValueProvenance`: `action_generated`, `derived_from_object`, or
  `external_policy`;
- `PlannedObjectAllocation`: `append_new` with an object number, or `deferred`.

## Ownership Rule

Indirect objects may be shared by multiple consumers. A future executor may
mutate an indirect object in place only when ownership is proven to be
single-use for the consumer being edited. If the object is shared, or ownership
cannot be proven, the executor must leave the original object untouched and
route the edited consumer to a private copy.

That rule is encoded by requiring
`presslint_pdf::IndirectObjectEditDecision` on every indirect-object boundary:
`dictionary_entry`, `whole_stream`, and `indirect_object_clone`. The decision
records the target object, the proven ownership state, and whether the edit is
in-place or private-copy.

For `content_stream_operand`, ownership is optional. Current inventory planning
knows page/scope/range and a content-derived `ObjectId`, but it does not yet
carry a concrete `IndirectRef` for the containing stream. The planner must not
synthesize an indirect reference or fake an ownership decision.

## Non-Goals

This contract does not define an executor or byte writer. It does not define
object serialization, stream encoding, PDF repair, xref writing, trailer
writing, revision ordering, or incremental-update output.

Those contracts are intentionally deferred to F3b:
`IncrementalRevisionPlan`, dirty-object intent, xref/trailer append intent, and
the incremental-update writer design note. F3b will build on this boundary
model and on the PDF incremental-update concepts around indirect objects,
dictionaries, streams, cross-reference sections, and trailers.
