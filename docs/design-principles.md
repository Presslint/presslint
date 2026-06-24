# Design Principles

- Preserve unmodified bytes whenever the parser/serializer pair owns the path.
- Make deterministic output a public contract.
- Treat malformed or unsupported shapes as structured skips, not silent partial
  conversions.
- Keep selectors and actions as data so they can be serialized, versioned, and
  driven by tools.
- Prove single-use before mutating shared indirect objects in place; otherwise
  use a private converted copy.
- Keep public APIs small until an end-to-end use case proves the abstraction.

