# presslint-color Journal

## Current State

- Defines color policy data contracts for spot handling and overprint handling.
- Defines abstract transform requests over `presslint-core::ColorSpace`.
- Defines output-intent policy contracts for preserving existing intents,
  requiring an existing intent, or requesting a named/profile-backed target for
  a later writer.
- Output-intent contracts are planning inputs only. They do not inspect PDF
  catalogs, parse ICC profiles, embed streams, or mutate PDF bytes.
- Focused serde shape tests lock the public JSON encoding of `ColorPolicy`,
  `SpotPolicy`, `OverprintPolicy`, `TransformRequest`, and the output-intent
  contracts. The transform fixture pins the nested `presslint-core::ColorSpace`
  encoding for both a unit variant (`device_cmyk`) and the `Resource(PdfName)`
  newtype variant. The dependency-free JSON harness lives in `src/tests/json.rs`
  and the shape tests in `src/tests.rs`, mirroring `presslint-selectors` and
  `presslint-actions`. The harness rejects `bool`, float, and `serde_bytes`-style
  byte scalars: none of the locked color contracts use them (`PdfName` and
  `EmbeddedBytes` wrap `Vec<u8>`, which serializes element-by-element as a
  sequence), so the harness stays scoped to exactly what the fixtures exercise.
- Does not yet include ICC parsing, DeviceLink execution, transform caching, or
  PDF write logic.

## Follow-Ups

- Keep color conversion as an action over inventory entries, not as parser
  orchestration.
- Keep output-intent insertion and replacement decisions in future planning and
  writing layers, separate from content operand conversion.
