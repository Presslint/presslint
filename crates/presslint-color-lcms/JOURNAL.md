# presslint-color-lcms Journal

## Current State

This crate opens an ICC **DeviceLink** profile from bytes and applies it to a
single scalar device colour via Little CMS. It is pure colour math: no PDF I/O,
no selectors, and no content-stream rewriting. The crate isolates the C
dependency and all `unsafe` FFI so `presslint-color` stays pure-Rust
`#![forbid(unsafe_code)]`.

## T172 - Prepared DeviceLink Handles

`PreparedDeviceLink` now owns the ICC bytes plus raw `DeviceLinkInfo` and lazily
retains one private native pair (`OpenProfile` + `OwnedTransform`) on first
successful `ColorEngine::apply_device_link`. Later applies through the same
prepared link reuse that transform request-locally; there is no global cache, no
new `Send`/`Sync` bound, and no public exposure of native handles.

The free `apply_device_link_f64` path now shares the same validation/build/apply
helpers. Validation order remains channel count, scalar-format/Lab support,
finite components, range components, then transform build/apply. LCMS flags and
intent remain unchanged (`RelativeColorimetric`, flags `0`).

### Public API

- `inspect_device_link(bytes: &[u8]) -> Result<DeviceLinkInfo, LcmsError>` — opens
  a profile, REQUIRES device class DeviceLink (`link`, else `NotADeviceLink`), and
  reports source/destination colour spaces + input/output channel counts.
- `apply_device_link_f64(bytes: &[u8], input: &[f64]) -> Result<Vec<f64>, LcmsError>`
  — applies the DeviceLink to ONE scalar colour (components normalized
  `0.0..=1.0`, ISO 32000 §8.6 scalar operand domain) and returns the raw `f64`
  output components. No rounding/quantisation here — the caller owns that.
- `DeviceLinkInfo { source_space, destination_space, input_channels, output_channels }`.
- `DeviceLinkSpace { Gray, Rgb, Cmyk, Lab, Unsupported(u32) }` — narrow map from the
  ICC/lcms colour-space signature. Unknown signatures are reported as
  `Unsupported(raw_u32)` by `inspect` (not an error); the caller uses source/dest
  to enforce its source-space gate.
- **Lab is inspect-only in this slice.** `inspect` reports a `Lab`-sided DeviceLink
  so the caller can see it, but `apply` REJECTS it with `UnsupportedColorSpace`.
  Rationale: lcms `TYPE_Lab_DBL` carries the `cmsCIELab` encoding (L in 0..100,
  signed a/b), which is NOT the public API's normalized `0.0..=1.0` scalar domain
  (ISO 32000 §8.6). Applying the normalized-domain validation + `Lab_DBL` format to
  a Lab side would either reject valid Lab inputs or silently transform wrongly
  scaled values. This crate defines no Lab encoding policy, so `apply` supports
  only Gray/RGB/CMYK, whose `TYPE_*_DBL` components genuinely live in
  `0.0..=1.0`.
- `LcmsError` (serde-tagged `#[serde(tag = "error", rename_all = "snake_case")]`,
  structured, no lcms internal strings): `InvalidProfile`, `NotADeviceLink`,
  `UnsupportedColorSpace`, `ChannelCountMismatch { expected, got }`,
  `NonFiniteComponent`, `ComponentOutOfRange`, `TransformBuildFailed`,
  `TransformFailed`. `apply` validation order: channel count first; then it maps
  BOTH sides to a `TYPE_*_DBL` format (`UnsupportedColorSpace` if either space has
  none — e.g. a Lab side); then per-component finite (NaN/∞ → `NonFiniteComponent`)
  and range (outside `0..=1` → `ComponentOutOfRange`). The format check precedes
  the range check on purpose so an unsupported (Lab) side is rejected as
  `UnsupportedColorSpace` regardless of the component values, since the range
  invariant only holds for the normalized-domain spaces. `TransformFailed` is
  reserved: `cmsDoTransform` is infallible in this configuration.

### Dependency: `lcms2-sys` (raw FFI, not the safe `lcms2` wrapper)

- Version pinned EXACT: `lcms2-sys = "=4.0.7"` (MIT; wraps Little CMS, MIT).
- Build feature: `default-features = false, features = ["static"]`. `static`
  compiles the **bundled** Little CMS C source with `cc` (build.rs short-circuits
  on `static` and never probes pkg-config), so output is reproducible across
  hosts regardless of any system lcms2. `default-features = false` also drops the
  `dynamic` and `parallel` defaults.
- **Rationale for using raw `lcms2-sys` instead of the safe `lcms2` wrapper**: the
  safe `lcms2` wrapper CAN build a single-DeviceLink transform, but its dependency
  on `lcms2-sys` uses `lcms2-sys`' default features, which include `parallel`.
  `parallel` pulls
  `cc/parallel → jobserver → getrandom → r-efi (LGPL-2.1-or-later) / wasip2 /
  wit-bindgen (Apache-2.0 WITH LLVM-exception)`. Those licences fail the mandatory
  `scripts/check_licenses.sh` gate (it rejects any forbidden-prefix identifier
  even inside an `OR`, and cannot parse `WITH` exceptions). Cargo cannot SUBTRACT
  a feature that an intermediate crate turns on — feature resolution is additive —
  so there is no way to disable `parallel` while depending on the `lcms2` wrapper.
  Depending on `lcms2-sys` DIRECTLY with `default-features = false` is the only
  way to keep the licence gate green. Result: `check_licenses.sh` passes (53
  third-party packages; the only new runtime/build deps are `lcms2-sys`, `libc`,
  `cc`, `find-msvc-tools`, `shlex`, `dunce`, all MIT/Apache/CC0).

### Determinism policy

Same DeviceLink bytes + same input → bit-identical `f64` output. Guaranteed by:
EXACT-pinned `lcms2-sys` + bundled `static` build; fixed `TYPE_*_DBL` input/output
formats (`GRAY_DBL`/`RGB_DBL`/`CMYK_DBL`; Lab is inspect-only — see above); a
DeviceLink-only transform
built with `cmsCreateMultiprofileTransform` over the single link profile (no
profile-connection-space chaining, no second profile); minimal transform flags
(`0` — NOT `cmsFLAGS_NOCACHE`, NOT `HIGHRESPRECALC`); no global plugins; no mutable
global lcms state. The rendering intent passed to the builder
(`Intent::RelativeColorimetric`) is irrelevant to the result because the intent is
already baked into the DeviceLink LUT; a fixed value is used only for
reproducibility. The determinism test applies the same link to the same input
twice and compares raw IEEE-754 bit patterns (`f64::to_bits`), not just `==`.

### Bounds / copy budget

Per the performance-discipline guidance, this crate may build a transform per
call (a bounded cache can be added later using
`presslint-color::TransformCacheKey`).
Per call: one profile handle (`cmsOpenProfileFromMem`), one transform handle, and
one small output `Vec<f64>` sized to `output_channels`. The input slice is passed
to `cmsDoTransform` in place — no byte reinterpretation or input copy (native
`double` pixels). Both handles are released via RAII (`OpenProfile`/`OwnedTransform`
`Drop`) on every return path. One colour per call, no batching, no large-payload
copies.

### Test-fixture approach (SYNTHETIC only — never ECI/FOGRA)

Every fixture is a tiny SYNTHETIC DeviceLink built in-memory through the same
`lcms2-sys` FFI: create built-in / synthetic profiles (`cmsCreate_sRGBProfile`,
`cmsCreateGrayProfile` with a D50 white point + a gamma tone curve, or
`cmsCreateLab4Profile` for the Lab source side), build a source→destination
transform, serialize it to a DeviceLink with `cmsTransform2DeviceLink`, then
`cmsSaveProfileToMem`, and feed those bytes back through the public API. Nothing is
vendored to disk (no `tests/fixtures/*.icc`), and the safe `lcms2` wrapper is
deliberately NOT used even as a dev-dependency, because it would reintroduce the
licence-forbidden transitive deps. Coverage: inspect reports RGB source/dest + 3/3
channels; inspect reports distinct Gray→RGB spaces + 1/3 channels; inspect reports a
Lab→RGB source side; RGB scalar apply is bit-identical across repeated calls; Gray
apply returns 3 output components; a Lab-sided DeviceLink is rejected by `apply`
with `UnsupportedColorSpace` (even for in-range components); channel-count mismatch,
NaN, infinite, and out-of-range inputs each reject with the right structured error;
a non-DeviceLink (sRGB display-class) profile and garbage bytes reject. 11
integration tests.

### `unsafe`

All `unsafe` FFI is contained in this crate (`#![allow(unsafe_code)]` overrides the
workspace `unsafe_code = "warn"` lint for this crate only). `presslint-color` is
untouched and remains `#![forbid(unsafe_code)]`.

### Gate result

`cargo fmt --all --check`, `./scripts/check_licenses.sh` (MIT `lcms2-sys` accepted),
`cargo test -p presslint-color-lcms` (11 pass), `cargo check --workspace
--all-targets`, and `./scripts/ci_check.sh` (clippy `-D warnings` incl. pedantic +
nursery) all pass.

### `ColorEngine` adapter (`engine.rs`)

`LcmsColorEngine` implements the `presslint-color` `ColorEngine` contract by
delegating verbatim to the existing free functions — a zero-state adapter that
introduces the contract WITHOUT moving any call site (the shipped converter keeps
calling the free functions this slice). `prepare_device_link` performs the same
single `inspect_device_link` the routing layer already runs and returns a
`PreparedDeviceLink` owning the link bytes plus the inspected `DeviceLinkInfo`;
`apply_device_link` delegates to `apply_device_link_f64(&link.bytes, input)`, so
the validation ORDER (channel-count → format/Lab-reject → finiteness → range) and
the output are identical. `device_link_shape` maps the inspected `DeviceLinkSpace`
sides to the shared `ColorSpace` vocabulary (Gray→DeviceGray, Rgb→DeviceRgb,
Cmyk→DeviceCmyk, Lab→Lab, Unsupported(_)→Unknown). NOTE: the raw 32-bit ICC
signature carried by `Unsupported(u32)` is LOST in that mapping — `Unknown` is
signature-free, so a caller needing the raw signature must read `DeviceLinkInfo`
directly. `type Error = LcmsError` (unchanged; its existing derives already satisfy
the contract's `Debug + Clone + PartialEq + Serialize + DeserializeOwned` bounds —
no `Display` added). No `Send`/`Sync`, matching the contract.

### New dependency edges + no keyed cache

Adds workspace-internal edges `presslint-color-lcms -> presslint-color` and `->
presslint-types` ONLY (the reverse edge stays forbidden: `presslint-color` is
contracts-only and `#![forbid(unsafe_code)]`). The crate re-exports the contract
(`pub use presslint_color::{ColorEngine, DeviceLinkShape}`) so future consumers
need no direct edge; the umbrella's `pub use presslint_color as color` exposes the
trait with no umbrella edit. Cargo.lock gains only these two internal edges (no new
third-party packages; the licence gate is unaffected). `PreparedDeviceLink` copies
the DeviceLink bytes — an intentional, bounded copy (one small profile per routed
link, off the hot path this slice does not touch); native-handle retention that
ends the per-`apply` re-parse is the NEXT slice. The keyed transform cache stays
DORMANT: `TransformCacheKey` is unwired because it is blocked on profile identity
(`DeviceLinkInput.id` is optional/non-unique and the key's doctrine forbids hashing
bytes) — a profile-registry-era concern, not this slice.

### Differential proof (`tests/engine.rs`)

A separate integration test rebuilds the same synthetic in-memory DeviceLinks the
executor suite builds and asserts, per fixture and input, that the adapter's
`prepare + apply` is BIT-IDENTICAL to `apply_device_link_f64` (`f64::to_bits`
equality) and ERROR-FOR-ERROR identical on every failure fixture (invalid profile,
not a devicelink, Lab-sided reject, channel mismatch, non-finite, out-of-range),
plus that `device_link_shape` agrees with `inspect_device_link`. The synthetic
builders are duplicated from `tests/device_link.rs` rather than shared through a
`tests/common` module, because a shared module would be a third file outside this
slice's write scope; no existing test was edited or weakened. 11 new tests.

## Follow-Ups

- Wire this executor into content-operand rewriting by calling it once per
  selected, source-space-matched colour operand (source-space gate via
  `DeviceLinkInfo::source_space`).
- Bounded transform cache keyed by `presslint-color::TransformCacheKey` before
  repeated real DeviceLink execution (do not add before cache ownership /
  invalidation boundaries are explicit — see the performance-discipline doc).
- No black-preservation overlay, no rounding/quantisation policy, no named-target
  resolution, no image/sample conversion, no multi-colour batch API in this slice.
