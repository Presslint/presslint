# presslint-selectors Journal

## Current State

- Defines serializable boolean selector expressions and leaf predicates.
- Current predicates cover object kind, observed color space, page index,
  expressive page match (parity/range/set), edit capability, content scope, and
  observed color usage.
- Provides an in-memory matcher over `presslint_inventory::InventoryEntry`.
- The exact `Page { page }` predicate is unchanged: it still matches
  `entry.id.page` by `PageIndex` equality and keeps its locked JSON shape
  `{ "kind": "page", "page": <u32> }`.
- The `PageMatch { matcher }` predicate matches `entry.id.page` through a
  serde-stable `PageMatcher` sum type (internally tagged on `match`,
  snake_case):
  - `Parity { parity }` matches by one-based-page-number parity. Parity is
    defined on the one-based page number (`PageIndex` value + 1): `Odd` matches
    pages 1, 3, 5 (indices 0, 2, 4) and `Even` matches pages 2, 4, 6
    (indices 1, 3, 5). It is computed directly on the zero-based index (which
    has the opposite low bit) to stay panic-free at `u32::MAX`.
  - `Range { start, end }` matches an inclusive zero-based index range on both
    ends and matches nothing when `start > end`.
  - `Set { pages }` matches membership by `PageIndex` equality via a borrowed
    linear scan over the caller-owned `Vec<PageIndex>`, independent of order and
    duplicates. The owned `Vec` is a stable public serde contract at the
    selector boundary; matching adds no per-call allocation.
  - `PageParity` is a unit-variant enum serializing as the strings `"odd"` and
    `"even"`.
- The content-scope predicate matches by full `ContentScope` equality against
  `entry.provenance.scope`, including the form `XObject` resource name.
- The color-usage predicate matches when any `ColorObservation` on the entry
  carries the requested `ColorUsage`, mirroring the color-space predicate's
  any-observation semantics.
- The color-components predicate has serde shape
  `{ "kind": "color_components", "space": <ColorSpace>, "usage"?: <ColorUsage>,
  "components": [<f64>...], "tolerance"?: <f64> }`. It scans `entry.colors`
  and matches only when one `ColorObservation` supplies the requested color
  space, optional usage, and component vector together. It does not combine
  color space from one observation with usage or components from another.
- Component matching requires equal vector lengths. With no tolerance, matching
  uses exact `f64` equality; with `tolerance`, every absolute per-component
  difference must be less than or equal to that non-negative tolerance. Non-finite
  predicate components or tolerance values are clean non-matches.
- The component-compare predicate (`ComponentCompare`) is the first NUMERIC
  colour target: serde shape
  `{ "kind": "component_compare", "space": <ColorSpace>, "usage"?: <ColorUsage>,
  "component_index": <usize>, "op": <CompareOp>, "value": <f64> }`. It matches
  when any `ColorObservation` supplies the requested space, the optional usage,
  and a component at `component_index` satisfying `component op value`. `CompareOp`
  is a unit-variant enum serializing as `"ge" | "gt" | "le" | "lt" | "eq"`,
  applied as `component op value` (so `Ge` with `value: 0.85` is `component >=
  0.85`).
  - VALUE CONVENTION: `value` and the observed components are PDF fractions in
    `0.0..=1.0`, so `K >= 85%` is `value: 0.85`. The `%`->fraction conversion is
    the CLI/API caller's job and is NOT encoded in the AST.
  - BANDS reuse the boolean selector: `K >= 0.2 and K < 0.8` is a
    `Selector::And` of two `ComponentCompare` predicates; there is deliberately
    no dedicated band/range variant.
  - ROBUSTNESS (no panics): a `component_index` past the observed components is a
    clean NON-MATCH (`slice::get`, never a panic, never `Unsupported`); a
    non-finite `value` or a non-finite observed component is a NON-MATCH. `Eq`
    uses exact `f64` equality (narrow `#[allow(clippy::float_cmp)]` on the
    `compare_op` helper) with NO tolerance field in this slice. Finite
    out-of-`[0,1]` values are mathematically valid and are NOT special-cased.
  - It is O(observations) with a single indexed component read and one float
    compare per observation; the matcher borrows the entry component slice and
    allocates nothing per `matches` call (no copy budget added).
- `Selector` and `Predicate` expose `PartialEq` but not `Eq`, because the
  serde-stable color-components and component-compare payloads can contain
  constructible `f64` values such as `NaN` even though matching rejects
  non-finite predicate values.
- The color-components matcher borrows predicate and entry component slices,
  performs a bounded linear scan over `entry.colors`, and allocates nothing per
  `matches` call.
- Focused serde tests lock the public JSON shape for selector boolean variants
  and predicate fixtures, including page, page-match (parity/range/set),
  named form-XObject, and annotation appearance scope fixtures. Matcher tests
  cover parity (odd/even), inclusive range (including the empty `start > end`
  case), and set membership (including the empty set).
- Tests live in a `tests` submodule split across files: `tests.rs` holds the
  shape and matcher tests, `tests/json.rs` holds the test-only in-memory JSON
  serde harness, and `tests/component_compare.rs` holds the `ComponentCompare` /
  `CompareOp` matcher and serde shape-lock tests (each `CompareOp` around a K
  boundary, usage gating, out-of-range index, non-finite value/component, a band
  via `And`, and the locked JSON shapes), keeping `lib.rs` focused on production
  code and under the file-size gate.

## Follow-Ups

- Keep selector JSON compatibility explicit when adding future predicates or
  consumer-facing recipes.
- A categorical "any form regardless of name" scope matcher is intentionally
  deferred to avoid a premature shared scope-kind discriminant; revisit only
  when a consumer needs it.
