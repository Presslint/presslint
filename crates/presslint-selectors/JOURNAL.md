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
- `Selector` and `Predicate` expose `PartialEq` but not `Eq`, because the
  serde-stable color-components payload can contain constructible `f64` values
  such as `NaN` even though matching rejects non-finite predicate values.
- The color-components matcher borrows predicate and entry component slices,
  performs a bounded linear scan over `entry.colors`, and allocates nothing per
  `matches` call.
- Focused serde tests lock the public JSON shape for selector boolean variants
  and predicate fixtures, including page, page-match (parity/range/set),
  named form-XObject, and annotation appearance scope fixtures. Matcher tests
  cover parity (odd/even), inclusive range (including the empty `start > end`
  case), and set membership (including the empty set).
- Tests live in a `tests` submodule split across files: `tests.rs` holds the
  shape and matcher tests, and `tests/json.rs` holds the test-only in-memory
  JSON serde harness, keeping `lib.rs` focused on production code and under the
  file-size gate.

## Follow-Ups

- Keep selector JSON compatibility explicit when adding future predicates or
  consumer-facing recipes.
- A categorical "any form regardless of name" scope matcher is intentionally
  deferred to avoid a premature shared scope-kind discriminant; revisit only
  when a consumer needs it.
