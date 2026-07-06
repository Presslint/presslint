# presslint-types Journal

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
