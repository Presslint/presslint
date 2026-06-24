# Architecture

`presslint-core` is built around a small set of stable concepts:

1. **Structural PDF access** opens documents, exposes objects and streams, and
   provides deterministic write seams.
2. **Syntax preservation** tokenizes and serializes content streams without
   normalizing unrelated bytes.
3. **Graphics interpretation** walks content streams into a live graphics state.
4. **Inventory** projects marked content into queryable scene objects.
5. **Selectors** choose inventory entries through serializable predicates.
6. **Actions** plan deterministic mutations against selected objects.
7. **Patch commit** applies validated edits while respecting shared-object
   ownership and deterministic output ordering.

The engine is designed for surgical PDF edits, not full-document re-rendering.

