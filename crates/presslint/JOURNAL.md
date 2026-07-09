# presslint Journal

Older accumulated journal history lives in [JOURNAL-archive.md](JOURNAL-archive.md).

## T168 - ICCBased Audit Findings

- Added the public `IccBasedFinding` / `IccBasedFindingKind` audit family and
  re-exported it at the crate root. The pass scans page colour-space resources
  and page default colour-space facts only; form-scope attribution is deferred.
- Findings cover missing/malformed `/N`, direct `/Range` arity mismatches,
  device-family `/Alternate` component mismatches, and present-but-unclassified
  alternates. They report parallel descriptor facts and do not arbitrate which
  side a renderer should trust.
- `ColorUsageAudit` now has additive `icc_based_findings`, omitted when empty
  and defaulted on deserialize. The audit status remains gap-driven only:
  ICCBased findings never create coverage gaps, and `ColorSpace::IccBased`
  stays in the modeled non-gap arm.
- The pass is read-only and uses shallow structural inspections only. It does
  not decode ICC streams, read profile headers, change inventory identities, or
  alter paint, conversion, selector, writer, or digest behavior.

## T167 - Colour-audit module split (zero behaviour change)

- Split `color_audit.rs` (964 lines) into a directory module with the file as
  facade root: `color_audit/report.rs` (serde DTOs, no logic),
  `color_audit/scan.rs` (the single deterministic inventory pass, `Scan`, the
  resource-skip predicates), `color_audit/classify.rs` (per-observation
  classification, `entry_gap`/`page_gap` constructors), and
  `color_audit/summary.rs` (`SummaryAccumulator`, the shared `bump` probe, the
  fixed variant orders). The root keeps `audit_color_usage`,
  `audit_color_usage_with_output_intent_policy`, the `#[cfg(test)]`
  `build_color_usage_audit`, and `build_audit`.
- Every existing path is preserved by root re-exports: the crate-root public
  set (`ColorAuditStatus` … `audit_color_usage_with_output_intent_policy`) and
  the crate-internal `crate::color_audit::{Scan, CoverageGap, CoverageGapKind,
  page_gap, build_color_usage_audit}` seams are unchanged, so `lib.rs`,
  `default_color_space_findings.rs`, `graphics_state_findings.rs`, and the
  sibling test modules needed no edits. `Scan` fields and
  `SummaryAccumulator`/`bump` became `pub(super)` so the root/scan can consume
  them without widening visibility beyond the module.
- Split the 991-line test module the same way: `tests/color_audit/mod.rs`
  holds the shared fixtures (serde harness include, synthetic-inventory
  builders, count helpers, page/ExtGState PDF builders) and the 23 tests moved
  unchanged into `scan_counts.rs`, `gaps_findings.rs`, `serde_shape.rs`, and
  `graphics_state.rs`. The relative serde-harness `#[path]` gained one `../`
  for the extra directory level. Same 23 test names, no expectation changes.
- No behaviour, serde-shape, or hot-path change: the pass structure,
  allocation profile, and report JSON are untouched; the compiled code is the
  moved code modulo symbol paths.
- Ablation: the two document-anchored inspection-error gap literals in
  `scan_inventory` now go through a `pub(super)` `pageless_gap` constructor in
  `classify.rs`, next to `entry_gap`/`page_gap`. Same construction, one
  definition inside the module. The identical private `pageless_gap` copies in
  `graphics_state_findings.rs`/`default_color_space_findings.rs` predate this
  task and were left alone to keep those files diff-free; unifying all three on
  the module-root export is a follow-up.

## T166 - Indexed colour-space reporting

- The umbrella colour-space mapping now resolves
  `ColorSpaceFamily::Indexed` to `presslint_types::ColorSpace::Indexed` in both
  the inventory bridge (`color_space_from_family`) and the default
  colour-space findings (`family_space`). `cs`/`scn` over a classified Indexed
  resource therefore reports `ColorSpace::Indexed` with the raw INDEX operands
  (initial colour `[0.0]` after `cs`), never `Resource(_)`, base-space
  components, or a resource-classification coverage gap.
- `/DefaultGray|RGB|CMYK` pointing at an Indexed definition now emits a
  default colour-space finding with `replacement_space = Indexed` and
  `replacement_component_count = Some(1)`; the matching device observations
  keep their original family and operands. No conversion, writer, selector, or
  action-planning behaviour changed, and `color_audit.rs` is untouched: an
  Indexed OBSERVATION still surfaces as the documented
  `UnmodeledColorSpace` gap because palette semantics stay unmodeled.

## T165 - Root form default colour-space attribution

- Extended `ColorUsageAudit.default_color_space_findings` to page-level Form
  `XObject` invocations. Form findings are correlated by page plus exact
  one-frame `InvocationPath` and carry optional `form_name`/`invocation`
  attribution; page findings omit those fields and keep the T164 JSON shape.
- Malformed present root-form defaults now surface as
  `CoverageGapKind::DefaultColorSpaceSkipped` when the root form invocation is
  observed. The audit remains read-only and does not rewrite
  `ColorObservation.space`, parse ICC profiles, or mutate inventory identity.

## T164 - Default colour-space audit findings

- Added additive `ColorUsageAudit.default_color_space_findings` with serde
  `default` and `skip_serializing_if = "Vec::is_empty"`. Empty reports keep the
  old JSON shape; old JSON deserializes with an empty vector.
- Added `DefaultColorSpaceFinding` and `DefaultColorSpaceFindingSource` at the
  umbrella crate root. Findings report page-scope `/DefaultGray`,
  `/DefaultRGB`, and `/DefaultCMYK` declarations only when the replacement is
  non-trivial and the same page has matching `Device*` colour observations.
- Malformed present default entries now surface as
  `CoverageGapKind::DefaultColorSpaceSkipped`; pass-level inspection failure
  surfaces as `DefaultColorSpaceInspectionError`. The audit does not apply
  defaults to `ColorObservation.space`, run ICC/profile parsing, or mutate PDF
  bytes.

## T163 - OutputIntent eligibility audit/report

- Added `OutputIntentEligibility` plus umbrella helpers
  `resolve_output_intent_eligibility` and
  `evaluate_pdf_output_intent_eligibility`, keeping PDF catalog observation in
  `presslint-pdf` and color-policy resolution in `presslint-color`.
- Added `audit_color_usage_with_output_intent_policy`, which attaches optional
  `ColorUsageAudit.output_intent_eligibility` when a caller supplies an
  `OutputIntentPolicy`. The existing `audit_color_usage` path leaves the field
  absent via serde `default` and `skip_serializing_if`.

## T162 - Audit full DeviceN colorant names

- `ColorUsageAudit.spot_names` now collects every name in a `Separation` or
  `DeviceN` observation's additive `spot_names` list, falling back to legacy
  `spot_name` when the list is empty for old/deserialized observations.
- The public audit surface is unchanged: `spot_names` remains one
  deduplicated, raw-byte-sorted `Vec<PdfName>`.

## T161a - OutputIntent Observation Bridge

- Added `observed_output_intents_from_pdf`, the umbrella-only bridge from
  neutral `presslint-pdf` output-intent facts into
  `presslint-color::ObservedOutputIntent`.
- Keeps crate boundaries intact: PDF observation remains in `presslint-pdf`,
  color policy stays in `presslint-color`, and unsupported PDF-side entries
  remain structured PDF diagnostics rather than color-layer inputs.

## T160 - Transparency Group Audit Findings

- `GraphicsStateFindingSource` gains additive page/form transparency-group
  variants. The audit emits the page variant when a page dictionary contains
  `/Group << /S /Transparency ... >>`, with `transparency=true` and
  `unclassified=true` only for malformed or out-of-scope group safety fields.
- Page `/Group` facts are derived from the new pdf-side page-group inspector and
  are emitted alongside each page's ExtGState findings in document order.
  Malformed or unknowable page `/Group` entries become additive transparency
  group coverage gaps instead of being reported as ExtGState facts.

