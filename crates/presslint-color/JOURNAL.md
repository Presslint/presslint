# presslint-color Journal

## Current State

- Defines color policy data contracts for spot handling and overprint handling.
- Defines abstract transform requests over `presslint-core::ColorSpace`.
- Defines output-intent policy contracts for preserving existing intents,
  requiring an existing intent, or requesting a named/profile-backed target for
  a later writer.
- Output-intent contracts are planning inputs only. They do not inspect PDF
  catalogs, parse ICC profiles, embed streams, or mutate PDF bytes.
- Adds a pure `resolve_output_intent_policy` decision helper that resolves an
  `OutputIntentPolicy` against a caller-supplied, ICC-free slice of
  `ObservedOutputIntent` values and returns a serde-stable
  `OutputIntentDecision`. It mirrors the `presslint-pdf::decide_indirect_object_edit`
  pure-decision pattern: `Preserve` leaves as-is; `RequireExisting` is satisfied
  when any intent is observed and otherwise yields an `OutputIntentRejection`;
  `EnsureTarget` resolves to already-satisfied on a matching target identity, a
  conflict on a same-subtype/different-identifier intent, or requires-ensure-target
  otherwise. Target identity is compared only by subtype and output-condition
  identifier (never `registry_name`, `info`, or profile bytes), with match taking
  priority over conflict and conflict over requires-ensure-target. The helper is
  pure: no PDF catalog inspection, no ICC parsing, no PDF byte mutation.
- Adds a report-only DeviceLink selection contract:
  `DeviceLinkPolicy`, `DeviceLinkDescription`, `DeviceLinkRejection`,
  `DeviceLinkDecision`, and `resolve_device_link_policy`. The helper resolves a
  policy against a `TransformRequest` and caller-supplied abstract DeviceLink
  descriptions. It never parses ICC bytes or executes transforms; matching is
  exact `ColorSpace` equality for both source and destination. `Require` rejects
  when no link matches, `Prefer` uses the first matching link and otherwise
  falls back to profile-connection-space planning, and `Forbid` always plans
  profile-connection-space conversion while ignoring supplied links.
- Focused serde shape tests lock the public JSON encoding of `ColorPolicy`,
  `SpotPolicy`, `OverprintPolicy`, `TransformRequest`, and the output-intent
  contracts plus the DeviceLink selection contracts. The transform fixture pins
  the nested `presslint-core::ColorSpace` encoding for both a unit variant
  (`device_cmyk`) and the `Resource(PdfName)` newtype variant. DeviceLink tests
  live in `src/tests/devicelink.rs`; the dependency-free JSON harness lives in
  `src/tests/json.rs`. The harness rejects `bool`, float, and
  `serde_bytes`-style byte scalars: none of the locked color contracts use them
  (`PdfName` and `EmbeddedBytes` wrap `Vec<u8>`, which serializes
  element-by-element as a sequence), so the harness stays scoped to exactly what
  the fixtures exercise.
- Does not yet include ICC parsing, DeviceLink execution, transform caching, or
  PDF write logic.

## Follow-Ups

- Keep color conversion as an action over inventory entries, not as parser
  orchestration.
- Keep output-intent insertion and replacement decisions in future planning and
  writing layers, separate from content operand conversion.
