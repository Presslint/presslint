# Inventory, Selectors, Actions

The core abstraction is:

```text
Inventory -> Selectors -> Actions -> Recipes
```

Inventory entries describe page objects with stable identities, provenance,
geometry, paint attributes, and edit capabilities.

Selectors are serializable predicates over inventory entries.

Actions are serializable mutation requests. They do not mutate immediately;
they first produce a patch plan that can be reviewed, journaled, tested for
shared-object safety, and committed deterministically.

