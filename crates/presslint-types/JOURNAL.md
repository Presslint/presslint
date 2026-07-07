# presslint-types Journal

## T162 - Additive multi-colorant observations

- `ColorObservation` gains additive `spot_names: Vec<PdfName>` for complete
  `Separation`/`DeviceN` colorant reporting. The legacy `spot_name` field stays
  unchanged as the first colorant compatibility field.
- `spot_names` uses serde `default` and `skip_serializing_if = "Vec::is_empty"`:
  old observation JSON still deserializes and non-spot observations keep their
  prior JSON shape.

## T153 - Declare ObjectId.digest identity (entry identity v3)

- Documentation only, no shape change. `ObjectId.digest`'s doc comment now states
  the identity is POSITIONAL within the page's paint order (not content-addressed)
  and invocation-aware: the digest folds the page-global sequence, the lexical
  scope, and — for form-painted content — the ordered form-invocation path (the
  same chain published in `Provenance.invocation`). Distinct invocations of one
  shared form receive distinct digests, and an edit that renumbers earlier paint
  operations renumbers the digests that follow. The digest remains an opaque
  handle; `ObjectId`/`Provenance`/`ContentScope` serde shapes are unchanged.

## T152 - Optional invocation provenance

- Added additive `Provenance::invocation: Option<InvocationPath>` metadata with
  serde defaulting and `skip_serializing_if`, so page-level and older serialized
  records keep their prior shape. `scope` remains the lexical source scope; the
  invocation path records which form paint instance produced an entry and is not
  part of entry identity yet.

## T148 - Invocation path vocabulary

- Added additive public `InvocationFrame` and `InvocationPath` types for future
  form-call provenance. They derive the crate's standard serde/public-data
  traits and are not yet referenced by `Provenance`, so existing serialized
  structs keep their prior shape.

## Current State

- Defines shared page, object identity, byte range, provenance, content scope,
  color observation, object kind, and edit capability types.
- `ColorObservation` carries an additive optional `source` byte range pointing
  at the color-setting operator that established the color (`None` for the
  page-default/inherited color and for synthesized observations).
- Provides the common data vocabulary used by inventory, selectors, actions,
  PDF access, syntax, and color crates.
- Performs no I/O.

## Follow-Ups

- Extend shared types only when a downstream slice needs a stable public
  contract.
