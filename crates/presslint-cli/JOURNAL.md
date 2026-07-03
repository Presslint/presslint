# presslint-cli Journal

## T132: thin convert and audit CLI

- Added the `presslint` binary with two commands:
  - `presslint convert <IN.pdf> --device-link <path>... [--select '<json>'|@file] [--pages <spec>] [--preserve-black] [--json] -o <OUT.pdf>|-`
  - `presslint audit <IN.pdf> [--json]`
- Kept the CLI as a thin driver: argument parsing, whole-file input reads,
  DeviceLink file reads, request assembly, output routing, and report rendering
  only. PDF syntax, colour conversion, selector evaluation, ICC inspection, and
  audit logic remain in the library crates.
- Introduced the `RunReport` boundary in `report.rs`. Command execution returns
  structured data plus warnings; rendering owns human-vs-JSON stream placement.
- Stream policy:
  - human reports go to stderr;
  - JSON reports go to stdout;
  - converted PDF bytes go to `-o <path>` through temp-file + rename, or to
    stdout with `-o -`;
  - `--json -o -` is rejected so JSON and PDF bytes never share stdout;
  - input and output paths must be distinct.
- Exit policy: success with honest warnings remains exit code 0; usage, parse,
  I/O, selector JSON, converter, and audit errors map to exit code 1.
- Memory note: this slice intentionally reads the whole input PDF and holds the
  library output/report in memory. For convert, peak memory is roughly 2-3x the
  PDF size because the library API is `&[u8] -> Vec<u8>`. Streaming and bounded
  memory are deferred.
- Deferred: named target registry, link directory/environment resolution,
  selector vocabulary enrichment, streaming I/O, in-place editing, and CI-policy
  failure flags.
- Private material and private profile paths are not referenced by public files.
  No competitor names are present.
- Test coverage includes pure page/selector/report routing tests plus a tiny
  synthetic end-to-end convert run that writes a PDF through the CLI execution
  path and renders the report. The synthetic DeviceLink bytes are public test
  material only; no licensed profile is vendored.

Verification status to update before hand-back:

- `cargo fmt --all --check`: passed
- `./scripts/check_licenses.sh`: passed, including the new direct `clap` and
  `serde_json` dependencies.
- `cargo test -p presslint-cli`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `./scripts/ci_check.sh`: passed.

Implementation note: the environment could not fetch new crates, so the CLI
uses `clap`'s builder API with the already-locked `std` feature instead of the
derive macro crate. The command surface and parsing behavior remain the same.

## T135: CLI coarse wall-clock timing

- Added `--timing` to `presslint convert` and `presslint audit`.
- Timing is measured with `std::time::Instant` at CLI phase boundaries only:
  - convert: `read_input`, `convert`, `write_output`, plus `total`;
  - audit: `read_input`, `audit`, plus `total`.
- Timing is observational and stderr-only. It is never included in converted
  PDF bytes and never included in the JSON machine report on stdout, so
  deterministic artifacts remain byte-reproducible with or without `--timing`.
- Without `--timing`, the command path does not collect or render timing lines;
  existing output and exit behavior are preserved.
- The timing renderer is a pure function over supplied labels and `Duration`
  values, with unit tests using injected durations rather than wall-clock
  assertions.
- The CLI remains a thin driver: no library API changed, and no dependency was
  added.
- Follow-up: internal per-stage library timing should use tracing spans or an
  explicit returned timing structure when the library is instrumented.
