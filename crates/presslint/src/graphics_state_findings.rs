//! Page-scope declared graphics-state findings for the colour-usage audit.
//!
//! This module derives the audit's `graphics_state_findings` surface from the
//! classified page `/Resources /ExtGState` inspection (the same dictionary-only
//! pass the inventory bridges run to build the walker environments), and the
//! coverage gaps for what that pass could NOT see.
//!
//! SEMANTICS — declared-in-resources presence. A finding reports that a page
//! DECLARES overprint/transparency-relevant extended graphics state in its
//! effective `/ExtGState` resources (ISO 32000-1 §8.4.5; overprint control
//! §8.6.7, transparency §11). A resource that is declared but never selected by
//! any `gs` operator still counts. Per-`gs` usage precision (did the content
//! actually activate that state?) belongs to the convert-guard slice, which
//! reuses this derivation on the write side.
//!
//! Findings versus gaps: successful detection — including "this dictionary
//! carries keys we do not classify" — is a FINDING; only an inspection failure
//! the derivation cannot see through becomes a coverage gap. Never both for the
//! same fact: a named `/ExtGState` entry that could not be classified surfaces
//! in the environment with every parameter `Unresolved` and is reported through
//! the finding's `unresolved` flag, not as a gap.

use presslint_inventory::{
    AlphaClass, BlendModeClass, ExtGStateParams, ExtGStateResource, GsParam, OverprintMode,
    SoftMaskClass,
};
use presslint_pdf::{
    DocumentAccessBackend, ObjectLookup, SkippedExtGStateResource, SkippedExtGStateResourceReason,
    SkippedPageXObjectResourceReason, inspect_document_access,
    inspect_document_page_extgstate_resources_with_lookup,
};
use presslint_types::PageIndex;
use serde::{Deserialize, Serialize};

use crate::color_audit::{CoverageGap, CoverageGapKind, page_gap};
use crate::document_inventory::page_extgstate_env;

/// Which resource scope a graphics-state finding was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicsStateFindingSource {
    /// The page's effective `/Resources /ExtGState` dictionary (including
    /// page-tree inheritance).
    PageExtGState,
    /// A form `XObject`'s own `/Resources /ExtGState` dictionary.
    ///
    /// DECLARED CONTRACT ONLY: this slice derives page-scope findings and never
    /// emits this variant. Deep form-scope findings are a named residual for
    /// the transparency-group (`/Group`) slice era.
    FormExtGState,
}

/// One aggregated per-page declared graphics-state finding.
///
/// At most ONE finding is emitted per page per [`GraphicsStateFindingSource`]:
/// the booleans aggregate over every classified resource in that scope. Pages
/// whose `/ExtGState` resources are absent or all-default produce no finding.
///
/// These are DECLARED-in-resources facts: a resource declared but never used by
/// any `gs` operator still counts (see the module docs). The booleans are
/// classified, never simulated — no PDF default values are invented.
// The four aggregate booleans ARE the pinned public contract for this finding
// (one flag per declared-state family); a state machine would misrepresent
// facts that can hold simultaneously.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphicsStateFinding {
    /// Page the finding is anchored to.
    pub page: PageIndex,
    /// Resource scope the finding was derived from.
    pub source: GraphicsStateFindingSource,
    /// A resource sets `OP` or `op` to `true`, or `OPM` to `1`/another
    /// classified non-default token. `OPM` alone does not switch overprinting
    /// on (it modulates it, ISO 32000-1 §8.6.7) but it is declared
    /// overprint-relevant state.
    pub overprint: bool,
    /// A resource sets a non-opaque `CA`/`ca`, a non-`/Normal` blend mode
    /// (including unrecognised or array-form values), or a present `SMask`.
    pub transparency: bool,
    /// A named `/ExtGState` entry could not be classified at all (for example an
    /// unresolved indirect reference), so it surfaces with every parameter
    /// `Unresolved`: nothing is known about what selecting it would do.
    pub unresolved: bool,
    /// A resource carries a Phase-1 safety key with an unclassifiable value, or
    /// keys outside the classified safety set. Partial classification is worth
    /// surfacing even when every classified parameter is default.
    pub unclassified: bool,
}

/// Everything the graphics-state pass contributes to one audit run.
#[derive(Debug, Default)]
pub struct GraphicsStateScan {
    /// Page-scope findings in document page order, at most one per page.
    pub findings: Vec<GraphicsStateFinding>,
    /// Gaps for inspection failures the finding derivation cannot see, in
    /// document page order, with a document-level begin-failure last.
    pub coverage_gaps: Vec<CoverageGap>,
}

/// Run the page-scope `/ExtGState` graphics-state pass over PDF bytes.
///
/// The inventory bridges already run this exact inspection to build the walker
/// environments but do not carry the reports on the public
/// [`PdfInventory`](crate::PdfInventory) shape, so the audit re-runs it here:
/// dictionary-only, one
/// bounded page-tree walk, no content-stream work. Any failure to BEGIN the
/// pass (document access or root page-tree expansion) is one document-anchored
/// [`CoverageGapKind::ExtGStateResourceInspectionError`] gap — after a
/// successful inventory build this is defensive only, since the same access
/// spine already succeeded.
#[must_use]
pub fn scan_document_graphics_state(input: &[u8]) -> GraphicsStateScan {
    let Ok(access) = inspect_document_access(input) else {
        return inspection_error_scan();
    };
    let lookup = backend_lookup(&access.backend);
    let Ok(report) = inspect_document_page_extgstate_resources_with_lookup(
        input,
        lookup,
        access.page_tree_root.object_byte_offset,
    ) else {
        return inspection_error_scan();
    };

    let mut scan = GraphicsStateScan::default();
    for page in &report.pages {
        // The env mapping is the same one the walker consumes: named
        // skips surface as all-`Unresolved` entries, so the derivation sees
        // them; only skips invisible to the env become gaps below.
        let env = page_extgstate_env(page);
        let Ok(ordinal) = u32::try_from(page.ordinal) else {
            // Unreachable after a successful inventory build, which rejects
            // out-of-range ordinals first; recorded page-less for honesty.
            scan.coverage_gaps
                .push(pageless_gap(CoverageGapKind::ExtGStateResourceSkipped));
            continue;
        };
        let page_index = PageIndex(ordinal);
        if let Some(finding) = derive_page_extgstate_finding(page_index, &env) {
            scan.findings.push(finding);
        }
        for skip in &page.skipped {
            if is_hidden_extgstate_skip(skip, &env) {
                scan.coverage_gaps.push(page_gap(
                    CoverageGapKind::ExtGStateResourceSkipped,
                    page_index,
                ));
            }
        }
    }
    scan
}

/// Aggregate one page's classified `ExtGState` environment into at most one
/// [`GraphicsStateFinding`] with source
/// [`GraphicsStateFindingSource::PageExtGState`].
///
/// Returns `None` when every boolean stays `false`: a page with no `/ExtGState`
/// resources, or only resources whose classified parameters are absent or set
/// to the trigger-free values (`OP`/`op` `false`, `OPM 0`, opaque alpha,
/// `/Normal` blend mode, `/SMask /None`), reports nothing.
#[must_use]
fn derive_page_extgstate_finding(
    page: PageIndex,
    resources: &[ExtGStateResource],
) -> Option<GraphicsStateFinding> {
    let mut finding = GraphicsStateFinding {
        page,
        source: GraphicsStateFindingSource::PageExtGState,
        overprint: false,
        transparency: false,
        unresolved: false,
        unclassified: false,
    };
    for resource in resources {
        finding.overprint |= sets_overprint(resource.params);
        finding.transparency |= sets_transparency(resource.params);
        finding.unresolved |= is_all_unresolved(resource.params);
        finding.unclassified |=
            resource.has_unclassified_keys || has_unclassified_param(resource.params);
    }
    (finding.overprint || finding.transparency || finding.unresolved || finding.unclassified)
        .then_some(finding)
}

/// Whether a resource declares overprint-relevant state: `OP`/`op` written
/// `true`, or `OPM` written `1` or another classified non-default token (ISO
/// 32000-1 §8.6.7).
const fn sets_overprint(params: ExtGStateParams) -> bool {
    matches!(params.overprint_stroke, GsParam::Set(true))
        || matches!(params.overprint_fill, GsParam::Set(true))
        || matches!(
            params.overprint_mode,
            GsParam::Set(OverprintMode::One | OverprintMode::Other)
        )
}

/// Whether a resource declares transparency-relevant state: a non-opaque
/// `CA`/`ca`, a non-`/Normal` blend mode (including present-but-other shapes),
/// or a present `SMask` (ISO 32000-1 §11.3.5, §11.6.4).
const fn sets_transparency(params: ExtGStateParams) -> bool {
    matches!(params.stroke_alpha, GsParam::Set(AlphaClass::NonOpaque))
        || matches!(params.fill_alpha, GsParam::Set(AlphaClass::NonOpaque))
        || matches!(
            params.blend_mode,
            GsParam::Set(BlendModeClass::NonNormal | BlendModeClass::OtherNamed)
        )
        || matches!(params.soft_mask, GsParam::Set(SoftMaskClass::Present))
}

/// Whether an env entry is the all-`Unresolved` stand-in for a named
/// `/ExtGState` slot that could not be classified at all.
const fn is_all_unresolved(params: ExtGStateParams) -> bool {
    matches!(params.overprint_stroke, GsParam::Unresolved)
        && matches!(params.overprint_fill, GsParam::Unresolved)
        && matches!(params.overprint_mode, GsParam::Unresolved)
        && matches!(params.stroke_alpha, GsParam::Unresolved)
        && matches!(params.fill_alpha, GsParam::Unresolved)
        && matches!(params.blend_mode, GsParam::Unresolved)
        && matches!(params.soft_mask, GsParam::Unresolved)
}

/// Whether any single classified parameter carries an unclassifiable value
/// (a present key whose value shape is outside the classified vocabulary).
const fn has_unclassified_param(params: ExtGStateParams) -> bool {
    matches!(params.overprint_stroke, GsParam::Unclassified)
        || matches!(params.overprint_fill, GsParam::Unclassified)
        || matches!(params.overprint_mode, GsParam::Unclassified)
        || matches!(params.stroke_alpha, GsParam::Unclassified)
        || matches!(params.fill_alpha, GsParam::Unclassified)
        || matches!(params.blend_mode, GsParam::Unclassified)
        || matches!(params.soft_mask, GsParam::Unclassified)
}

/// Decide whether a page `/ExtGState` skip is HIDDEN from the finding
/// derivation and must therefore surface as a coverage gap.
///
/// Mirrors the colour-space `should_report`-style split, with one extra rule
/// for named skips:
/// - A NAMED skip that the env surfaces as an all-`Unresolved` entry is already
///   reported through the finding's `unresolved` flag — not a gap (a fact is a
///   finding or a gap, never both).
/// - A NAMED skip whose name the env only carries as a CLASSIFIED entry (a
///   duplicate-name diagnostic shadowed by the first occurrence) hides state
///   the derivation cannot see — a gap.
/// - An UNNAMED skip that merely records absence (no `/Resources`, no
///   `/ExtGState`) hides nothing — not a gap.
/// - Every other unnamed skip (duplicate or non-dictionary `/ExtGState`, a
///   dictionary that could not be scanned, a `/Resources` resolution failure)
///   hides the whole scope — a gap.
fn is_hidden_extgstate_skip(skip: &SkippedExtGStateResource, env: &[ExtGStateResource]) -> bool {
    use SkippedExtGStateResourceReason as Reason;
    use SkippedPageXObjectResourceReason as ResourcesReason;
    if let Some(name) = &skip.resource_name {
        return !env
            .iter()
            .any(|resource| resource.name.0 == name.0 && is_all_unresolved(resource.params));
    }
    match &skip.reason {
        Reason::MissingExtGStateResources | Reason::MissingExtGState => false,
        Reason::Resources { resources_reason } => !matches!(
            resources_reason,
            ResourcesReason::MissingResources | ResourcesReason::MissingXObject
        ),
        _ => true,
    }
}

/// One-gap scan for a pass that could not begin at all.
fn inspection_error_scan() -> GraphicsStateScan {
    GraphicsStateScan {
        findings: Vec::new(),
        coverage_gaps: vec![pageless_gap(
            CoverageGapKind::ExtGStateResourceInspectionError,
        )],
    }
}

/// Build a coverage gap with no page or object anchor.
const fn pageless_gap(kind: CoverageGapKind) -> CoverageGap {
    CoverageGap {
        kind,
        page: None,
        object: None,
        entry_index: None,
        kind_of_object: None,
        usage: None,
        color_space: None,
    }
}

/// Select the unified object-lookup backend for a resolved document access.
///
/// A local copy of the inventory bridge's private dispatch (a four-arm
/// mechanical match) so this pass does not widen `pdf_inventory`'s surface.
const fn backend_lookup(backend: &DocumentAccessBackend) -> ObjectLookup<'_> {
    match backend {
        DocumentAccessBackend::ClassicXref { xref_table, .. } => {
            ObjectLookup::ClassicXref(xref_table)
        }
        DocumentAccessBackend::ClassicXrefChain { chain } => ObjectLookup::ClassicXrefChain(chain),
        DocumentAccessBackend::XrefStreamSection { section } => {
            ObjectLookup::XrefStreamSection(section)
        }
        DocumentAccessBackend::XrefStreamChain { chain } => ObjectLookup::XrefStreamChain(chain),
    }
}
