# presslint-selectors Journal

## Current State

- Defines serializable boolean selector expressions and leaf predicates.
- Current predicates cover object kind, observed color space, page index, and
  edit capability.
- Provides an in-memory matcher over `presslint_inventory::InventoryEntry`.
- Focused serde tests lock the public JSON shape for selector boolean variants
  and predicate fixtures.

## Follow-Ups

- Keep selector JSON compatibility explicit when adding future predicates or
  consumer-facing recipes.
