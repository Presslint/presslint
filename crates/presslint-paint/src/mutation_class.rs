//! Per-paint-act routing classes for future mutation decisions.
//!
//! [`MutationClass`] is a contract vocabulary for one paint-site mutation
//! decision. It is not decidability for shared indirect objects: whether bytes
//! may be changed in place still needs the document ownership graph and the
//! relevant mutation boundary. This type reconciles the existing public
//! taxonomies without replacing them, and this crate currently has no live
//! producer or consumer for it.
//!
//! The relation to existing taxonomies is intentionally partial:
//!
//! | Mutation class | Inventory `EditCapability` relation | Actions `MutationBoundary` / `SkipReason` relation | Write pipeline relation |
//! |---|---|---|---|
//! | [`MutationClass::PreserveBytes`] | **KEEPS-DISTINCT** from `EditCapability::ReadOnly` and from an empty capability list: this is a routing decision for one paint act, not an advertised per-entry ability. | **KEEPS-DISTINCT** from `MutationBoundary`, which answers where bytes may change; no boundary is needed when bytes are preserved. **KEEPS-DISTINCT** from `SkipReason`, which remains action-planning eligibility. | **KEEPS-DISTINCT** from skip reasons; corresponds to byte-verbatim outcomes such as `EditedContent::Unchanged` or a guarded no-op without encoding why. |
//! | [`MutationClass::SurgicalRewrite`] | **SUBSUMES** narrow edit abilities such as `EditCapability::RewriteColorOperand`, `EditCapability::AddTextSpreadStroke`, and `EditCapability::AdjustStrokeWidth`. | **WRAPS** natural locality boundaries such as `MutationBoundary::ContentStreamOperand` and narrow `MutationBoundary::DictionaryEntry`; `SkipReason` remains outside this class. | **WRAPS** rewritten decoded content such as `EditedContent::Rewritten` when the replacement is a localized byte edit. |
//! | [`MutationClass::AppearanceReplacement`] | **WRAPS** `EditCapability::ReplaceImageStream` and future rendered-appearance replacement paths. | **WRAPS** broader byte-change boundaries such as `MutationBoundary::WholeStream` and `MutationBoundary::IndirectObjectClone`; `SkipReason` remains outside this class. | **WRAPS** stream/object replacement paths that preserve semantics by replacing appearance data rather than a narrow operand. |
//! | [`MutationClass::UnsupportedSkip`] | **KEEPS-DISTINCT** from `EditCapability::ReadOnly`; it can result from missing capability, an empty capability list, or later byte/provenance guards. | **KEEPS-DISTINCT** from `SkipReason`: concrete action-planning reasons stay in `presslint-actions`, and mutation locality stays in `MutationBoundary`. | **KEEPS-DISTINCT** from `PipelineSkipReason` and the public write skip enums; concrete physical reasons stay in the write taxonomy. |
//!
//! `MutationBoundary` remains the where-may-bytes-change locality and provenance
//! contract, which is a distinct question from this routing class.
//! `presslint-actions` `SkipReason` remains the action-planning eligibility
//! taxonomy and is not folded into [`MutationClass::UnsupportedSkip`].
//!
//! Phase-4 retirement target: the write crate currently re-derives public skip
//! labels (`ConvertPageSkipReason`, `ContentColorRewriteSkipReason`,
//! `ReencodePageSkipReason`) from one non-serde `PipelineSkipReason` through
//! three `map_skip_reason` functions for the same physical causes. The
//! paint-driven engine should centralize that mapping or make the split
//! intentional.

/// Routing class for a paint-site mutation decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationClass {
    /// Leave the sourced bytes untouched for this paint act.
    PreserveBytes,
    /// Emit localized replacement bytes for a specific operand or narrow entry.
    SurgicalRewrite,
    /// Replace a rendered appearance carrier, such as a stream or cloned object.
    AppearanceReplacement,
    /// Skip this paint act without carrying a reason payload.
    UnsupportedSkip,
}

impl MutationClass {
    /// Return true when this routing class leaves source bytes unchanged.
    #[must_use]
    pub const fn preserves_source_bytes(self) -> bool {
        matches!(self, Self::PreserveBytes | Self::UnsupportedSkip)
    }

    /// Return true when this routing class may emit replacement bytes.
    #[must_use]
    pub const fn may_emit_replacement_bytes(self) -> bool {
        matches!(self, Self::SurgicalRewrite | Self::AppearanceReplacement)
    }
}
