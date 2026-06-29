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
- Adds a report-only spot-resolution contract: `ObservedSpotColor`,
  `SpotRejection`, `SpotSkipReason`, `SkippedSpotConversion`, `SpotDecision`, and
  `resolve_spot_policy`. The helper resolves a `SpotPolicy` against a
  caller-supplied, ICC-free slice of `ObservedSpotColor` values (each an abstract
  colorant `name` plus the `alternate` color space its tint transform targets, no
  ICC data, no tint-transform function, no components). `Preserve` leaves as is;
  `Reject` is satisfied (`NoSpotColors`) when no spot is observed and otherwise
  yields a `SpotRejection::SpotConversionRequired`; `ConvertAlternate` partitions
  the observed spots in a single deterministic pass into `eligible` (alternate is
  a process device color space: `DeviceGray`/`DeviceRgb`/`DeviceCmyk`) and
  `skipped` (`SpotSkipReason::UnsupportedAlternate` for every other alternate),
  both lists preserving caller iteration order. Reporting skips rather than
  dropping unconvertible spots keeps unsupported shapes preserved or explicitly
  skipped, never silently converted partially wrong. The shared
  `is_process_device_color_space` private helper defines the process-device
  notion. The helper is pure: no PDF catalog inspection, no `Separation`/`DeviceN`
  reading, no ICC parsing, no tint-transform evaluation, no PDF byte mutation.
- Adds a report-only overprint-resolution contract:
  `ObservedOverprintInteraction`, `OverprintSensitivity`,
  `OverprintMitigation`, `OverprintMitigationAction`, `OverprintRejection`,
  `OverprintSkipReason`, `SkippedOverprintMitigation`, `OverprintDecision`, and
  `resolve_overprint_policy`. The helper resolves an `OverprintPolicy` against
  caller-supplied, PDF-free overprint observations. `Preserve` leaves as is;
  `RejectUnsafe` is satisfied (`NoUnsafeOverprint`) only when no unsafe
  interaction is observed and otherwise returns a structured
  `OverprintRejection::UnsafeOverprintSensitiveConversions` with unsafe
  observations in caller order; `Mitigate` ignores safe observations and
  partitions unsafe observations into supported report-only mitigations
  (`PreserveZeroProcessColorants` for process colorant expansion,
  `PreserveSpotOverprintAppearance` for spot colorant conversion) and explicit
  skips (`OverprintSkipReason::UnsupportedInteraction`), preserving caller order
  in both lists. The helper is pure: no graphics-state or ExtGState inspection,
  no overprint simulation, no transparency flattening, no color transform, no
  PDF byte mutation.
- Adds a combined color-policy resolution contract: `ColorPolicyDecision` and
  `resolve_color_policy`. The helper resolves a whole `ColorPolicy` (its `spot`
  and `overprint` members) against caller-supplied, PDF-free spot and overprint
  observations in one call and returns a serde-stable `ColorPolicyDecision`
  carrying the resolved `spot: SpotDecision` and `overprint: OverprintDecision`.
  It is a thin aggregator that delegates to the existing `resolve_spot_policy`
  and `resolve_overprint_policy`, reimplementing no spot or overprint logic: its
  `spot` field equals `resolve_spot_policy(policy.spot, spot_observed)` and its
  `overprint` field equals `resolve_overprint_policy(policy.overprint,
  overprint_observed)` for the same inputs, so caller iteration order is
  preserved unchanged in every nested list. `ColorPolicy.spot`/`overprint` are
  `Copy`, so the helper reads `&ColorPolicy` without cloning. This mirrors the
  combined page-object inventory builder: keep the focused sub-resolvers
  authoritative, then add a delegating aggregator for the whole policy. It lives
  in `color_policy.rs`, re-exported from the crate root next to the other
  resolvers. The helper is pure: no PDF catalog inspection, no graphics-state or
  ExtGState reading, no ICC parsing, no tint-transform evaluation, no overprint
  simulation, no transparency flattening, no color transform, no PDF byte
  mutation. Combined resolution and shape tests live in
  `src/tests/color_policy.rs`.
- Adds a report-only transform planning contract: `TransformPlanRequest`,
  `TransformPlanDecision`, and `resolve_transform_plan`. The request combines an
  existing `TransformRequest`, `DeviceLinkPolicy`, and `OutputIntentPolicy`; the
  decision carries the resolved `DeviceLinkDecision`, `OutputIntentDecision`, and
  `ColorPolicyDecision` without adding a new top-level rejection policy. The
  helper is a thin aggregator that delegates directly to
  `resolve_device_link_policy`, `resolve_output_intent_policy`, and
  `resolve_color_policy` for the same inputs, so caller iteration order and all
  focused resolver behavior are preserved unchanged. It remains report-only: no
  ICC parsing, PDF catalog inspection, graphics-state inspection, color
  transform execution, output-intent insertion, or PDF byte mutation. Focused
  delegation and serde shape tests live in `src/tests/transform_plan.rs`, with
  the request shape locked in `src/tests.rs`.
- Adds a report-only, deterministic transform cache-key contract:
  `ProfileReference`, `TransformCacheKey`, `TransformCacheKeyRejection`,
  `TransformCacheKeyDecision`, and `derive_transform_cache_key`, in their own
  `transform_cache.rs` module. The key identifies one abstract color transform so
  a future executor can recognise identical transforms before any real
  ICC/DeviceLink execution exists; this slice models only the key contract and
  its derivation, not a cache store, eviction, invalidation, or LRU policy. The
  key names a transform by the conversion path the crate already resolves: the
  DeviceLink path (`TransformCacheKey::DeviceLink`) keyed by the selected
  DeviceLink's stable `id` plus the abstract source/destination `ColorSpace`, and
  the profile-connection-space path (`TransformCacheKey::ProfileConnectionSpace`)
  keyed by caller-supplied, bytes-free source/destination `ProfileReference`
  values plus the abstract source/destination `ColorSpace`. `ProfileReference`
  mirrors the `OutputProfileSource::OpaqueId` pattern: it names a profile by a
  stable id only and carries no ICC/profile bytes; the key never carries, hashes,
  or copies profile bytes, and no `EmbeddedBytes`-style variant is accepted into
  it. The key derives `Eq`/`Hash` so a future bounded cache can use it directly:
  equal inputs (same path, ids, color spaces) yield equal keys and any differing
  link id, profile reference, source, or destination yields a different key.
  `derive_transform_cache_key` is a pure single match over an already-resolved
  `DeviceLinkDecision` plus the originating `TransformRequest` and the two
  bytes-free PCS profile references (consulted only on the PCS path):
  `UseDeviceLink` yields a keyed DeviceLink key, `UseProfileConnectionSpace`
  yields a keyed PCS key or, when a profile reference is absent, a structured
  `MissingSourceProfileId`/`MissingDestinationProfileId` no-key reason (source
  checked first), and `Rejected` yields `RejectedDeviceLink`. It fabricates no
  placeholder key for unbounded inputs. The helper is pure: no ICC parsing, no
  PDF catalog or graphics-state inspection, no transform execution, no PDF byte
  mutation. Derivation, equality/inequality, no-key-reason, and serde shape tests
  live in `src/tests/transform_cache.rs`; the dependency-free JSON harness needed
  no change because the contract uses only `String` and `ColorSpace`.
- Focused serde shape tests lock the public JSON encoding of `ColorPolicy`,
  `SpotPolicy`, `OverprintPolicy`, `TransformRequest`, and the output-intent
  contracts plus the DeviceLink selection, spot-resolution, and
  overprint-resolution contracts. The transform fixture pins the nested
  `presslint-core::ColorSpace` encoding for both a unit variant (`device_cmyk`)
  and the `Resource(PdfName)` newtype variant. DeviceLink tests live in
  `src/tests/devicelink.rs`, spot resolution tests in `src/tests/spot.rs`, and
  overprint resolution tests in `src/tests/overprint.rs`; the dependency-free
  JSON harness lives in
  `src/tests/json.rs`. The harness rejects `bool`, float, and
  `serde_bytes`-style byte scalars: none of the locked color contracts use them
  (`PdfName` and `EmbeddedBytes` wrap `Vec<u8>`, which serializes
  element-by-element as a sequence), so the harness stays scoped to exactly what
  the fixtures exercise.
- Does not yet include ICC parsing, DeviceLink execution, transform caching, or
  PDF write logic.
- The crate is split into focused modules mirroring `presslint-inventory` and
  `presslint-syntax`: `policy.rs` (the shared policy/request input contracts:
  `ColorPolicy`, `SpotPolicy`, `OverprintPolicy`, the output-intent policy/target
  contracts, `DeviceLinkPolicy`, `TransformRequest`), `output_intent.rs`
  (`ObservedOutputIntent`/`OutputIntentRejection`/`OutputIntentDecision`, the
  private `target_identity` helper, and `resolve_output_intent_policy`), `spot.rs`
  (the spot-resolution contracts, the private `is_process_device_color_space`
  helper, and `resolve_spot_policy`), `overprint.rs` (the overprint-resolution
  contracts and `resolve_overprint_policy`), and `devicelink.rs` (the DeviceLink
  selection contracts and `resolve_device_link_policy`). `lib.rs` is a small
  public facade that keeps `#![forbid(unsafe_code)]`, declares the modules, and
  re-exports every previously-public item, so the public API is source-identical.
  The split was a mechanical move with no contract, signature, or behavior change;
  the `target_identity` and `is_process_device_color_space` helpers stay private to
  their modules.

## Follow-Ups

- Keep color conversion as an action over inventory entries, not as parser
  orchestration.
- Keep output-intent insertion and replacement decisions in future planning and
  writing layers, separate from content operand conversion.
