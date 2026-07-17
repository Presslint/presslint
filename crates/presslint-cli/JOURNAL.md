# presslint-cli Journal

## T196: staged-export counter serde locks (test surface only)

- No CLI production change. The report tests extend the
  `form_clone_set_plan_counts` serde locks to the additive staged-export
  counters (`staged_sets`, `staged_objects`, `staged_body_bytes`,
  `export_refused_sets`): nonzero values serialize, zero values are omitted
  (existing JSON shapes stay byte-identical), and older/partial JSON without
  the new fields deserializes them to zero.
- Human/JSON rendering, command syntax, warnings, and exit policy are
  unchanged: the counts ride the wrapped library report as-is.

## T195: convert report clone-set plan counts (test surface only)

- No CLI production change. The public `ConvertedPage` struct literals in the
  report tests add the new additive `form_clone_set_plan_counts` field
  (`FormCloneSetPlanCounts::default()`), locking the public construction
  shape.
- New serde locks: the field is omitted from JSON when every counter is zero
  (existing zero-count JSON shapes stay byte-identical), nonzero counters
  serialize with zero inner counters omitted, older `ConvertedPage` JSON
  without the field deserializes to the empty default, and partial counts
  JSON defaults missing counters to zero.
- Human/JSON rendering, command syntax, warnings, and exit policy are
  unchanged: the counts ride the wrapped library report as-is.

## T179: convert report alias-candidate outcomes

- Human conversion totals add `alias candidates converted` and
  `alias candidates refused`; per-page lines add `alias_converted=` and
  `alias_refused=` in deterministic order.
- Candidate refusals participate in the existing coverage-gap warning as
  `alias_candidates_refused=`. Command syntax, selector help, and exit policy
  are unchanged.
- JSON continues to carry the library page report directly. Both additive
  candidate fields are omitted at zero and default to zero when older JSON is
  deserialized; nonzero values are additive.

## T177: convert report alias-eligibility and Default-safety counts

- Human `presslint convert` totals now include three deterministic lines:
  `alias setters eligible`, `alias setters ineligible`, and
  `default color-space unsafe`; each per-page line appends
  `alias_eligible=`, `alias_ineligible=`, and `default_unsafe=`.
- JSON output stays the wrapped library report. The additive per-page fields
  `resource_alias_setters_eligible` / `resource_alias_setters_ineligible` and
  `operator_skips.default_color_space_unsafe` are omitted at zero, so every
  existing zero-count JSON shape is byte-compatible.
- Command names, flags, and selector help are unchanged: the converter still
  converts only the six direct device shortcut operators and merely REPORTS
  resource-alias eligibility and Default-safety refusals. The existing
  coverage warning deterministically includes the nonzero/zero
  `default_color_space_unsafe` tally alongside its other skip counters.

## T164: audit report default colour-space count

- Human `presslint audit` output now includes the
  `default color-space findings` count from `ColorUsageAudit`.
- JSON output remains the wrapped library report; the library field is omitted
  when empty and present only when the audit emits findings.

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

## T168: ICCBased audit finding counter

- Human audit reports now include an `icc-based findings` counter beside the
  existing finding and coverage counters.
- JSON output needs no CLI-specific shaping: it carries the library
  `ColorUsageAudit` unchanged, including the additive `icc_based_findings`
  vector when present and omitting it when empty.
