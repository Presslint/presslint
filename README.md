# PressLint Core

Programmable PDF prepress core written in Rust.

> Status: pre-alpha scaffold. APIs are intentionally unstable while the
> object inventory, selector, action, and byte-preserving edit model are
> established.

## Goal

PressLint Core is an Apache-2.0 licensed engine for inspecting and applying
deterministic, structure-preserving prepress operations to PDFs.

The core model is:

```text
PDF bytes -> inventory -> selectors -> actions -> patch plan -> deterministic output
```

The first public milestones focus on:

- byte-preserving content-stream parsing and serialization;
- page-object inventory for text, vector, image, form, pattern, and shading uses;
- serializable selectors and action plans;
- deterministic color conversion through ICC / DeviceLink profiles.

## Non-goals

This repository is not the commercial preflight product, not a GUI, not a RIP,
and not a full PDF renderer. Product workflows, private test corpus notes,
automation prompts, and commercial strategy live outside the public repository.

## Workspace

```text
crates/presslint-core        Shared public data types.
crates/presslint-pdf         Structural PDF access and write seams.
crates/presslint-syntax      Content stream tokenization and serialization.
crates/presslint-inventory   Page object inventory model.
crates/presslint-selectors   Serializable selector DSL.
crates/presslint-actions     Serializable action and recipe model.
crates/presslint-color       Color policy and transform interfaces.
```

## Development

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo test --workspace
./scripts/ci_check.sh
```

## License

Licensed under the Apache License, Version 2.0. See `LICENSE`.
