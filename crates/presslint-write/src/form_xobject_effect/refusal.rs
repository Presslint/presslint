//! Form refusal-class taxonomy and per-page counting shape.
//!
//! [`FormXObjectRefusalClass`] names the stable terminal gate that blocked one
//! demanded Form identity's admission. It is observe-only: it never
//! authorizes anything, never describes epoch refusal, retains no PDF
//! identity, byte, or resource-name fact, and exposes no cache/lattice/budget
//! internal. [`FormXObjectRefusalCounts`] is the additive per-page shape a
//! caller folds one class into per refused demanded identity, mirroring the
//! `OperatorSkipCounts` house pattern: named `usize` fields, `is_empty`, and a
//! fold helper, `#[serde(default, skip_serializing_if = "..")]` so existing
//! JSON without refusals stays byte-identical.

use serde::{Deserialize, Serialize};

/// The stable terminal gate that refused one demanded Form identity.
///
/// Exactly eleven variants. A nested child's intrinsic failure bubbles up to
/// its invoking root classified as the child's OWN actionable cause; only a
/// genuine active-path cycle re-encounter or a traversal-horizon depth cutoff
/// classify as [`Self::RecursionCycle`]/[`Self::RecursionDepth`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FormXObjectRefusalClass {
    /// Exact identity corroboration (addressability, generation, reached
    /// offset) or the decoded-name Form-dictionary preflight (execution
    /// aliases, safety-key evasion) refused.
    StructuralPreflight,
    /// Filter/extent classification refused: an unsupported filter or filter
    /// chain, a non-default `FlateDecode` predictor, or an extent that could
    /// not be inspected.
    StreamFilterOrExtent,
    /// A transparency `/Group` was present, malformed, or unresolved.
    TransparencyGroup,
    /// Tokenization, operator assembly, the closed raw-record preflight
    /// grammar, or an intrinsic seeded-walk fallthrough refused.
    RawGrammar,
    /// The Form-local `/Resources /ColorSpace` authority, or a `CS`/`cs`/
    /// `SC`/`SCN`/`sc`/`scn` validation over it, refused.
    ColorAuthority,
    /// The Form-local `/Resources /XObject` authority refused to resolve an
    /// invoked `Do` name to an admissible target.
    XObjectAuthority,
    /// The Form-local `/Resources /ExtGState` proven-neutral `gs` gate
    /// refused.
    ExtGStateAuthority,
    /// A nested Form descent re-entered a reference already on the active
    /// descent path.
    RecursionCycle,
    /// A nested Form descent sat past the bounded traversal horizon with
    /// nothing cached.
    RecursionDepth,
    /// The fixed per-request first-seen exact Form target cap was spent.
    TargetBudget,
    /// The aggregate per-request decoded-byte budget was zero-entry, a raw
    /// body exceeded it, or a Flate inflation hit its output limit.
    DecodedByteBudget,
}

/// Per-page aggregate refusal-class counts.
///
/// Exhaustively counts a refused demanded Form identity once per exact
/// `(reference, reached_offset)` identity per page. Every field is additive
/// and independent; a new taxonomy class requires a new field here and in
/// every exhaustive match over [`FormXObjectRefusalClass`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormXObjectRefusalCounts {
    /// Count of [`FormXObjectRefusalClass::StructuralPreflight`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub structural_preflight: usize,
    /// Count of [`FormXObjectRefusalClass::StreamFilterOrExtent`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub stream_filter_or_extent: usize,
    /// Count of [`FormXObjectRefusalClass::TransparencyGroup`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub transparency_group: usize,
    /// Count of [`FormXObjectRefusalClass::RawGrammar`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub raw_grammar: usize,
    /// Count of [`FormXObjectRefusalClass::ColorAuthority`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub color_authority: usize,
    /// Count of [`FormXObjectRefusalClass::XObjectAuthority`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub xobject_authority: usize,
    /// Count of [`FormXObjectRefusalClass::ExtGStateAuthority`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub extgstate_authority: usize,
    /// Count of [`FormXObjectRefusalClass::RecursionCycle`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub recursion_cycle: usize,
    /// Count of [`FormXObjectRefusalClass::RecursionDepth`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub recursion_depth: usize,
    /// Count of [`FormXObjectRefusalClass::TargetBudget`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub target_budget: usize,
    /// Count of [`FormXObjectRefusalClass::DecodedByteBudget`] refusals.
    #[serde(default, skip_serializing_if = "count_is_zero")]
    pub decoded_byte_budget: usize,
}

/// Serde helper: omit an additive scalar count while it is zero, so existing
/// zero-count JSON shapes stay byte-compatible.
#[allow(clippy::trivially_copy_pass_by_ref)]
const fn count_is_zero(count: &usize) -> bool {
    *count == 0
}

impl FormXObjectRefusalCounts {
    /// The all-zero counts (const constructor, used where `Default::default`
    /// is not `const`).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            structural_preflight: 0,
            stream_filter_or_extent: 0,
            transparency_group: 0,
            raw_grammar: 0,
            color_authority: 0,
            xobject_authority: 0,
            extgstate_authority: 0,
            recursion_cycle: 0,
            recursion_depth: 0,
            target_budget: 0,
            decoded_byte_budget: 0,
        }
    }

    /// Whether every count is zero (the omit-when-empty predicate).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.structural_preflight == 0
            && self.stream_filter_or_extent == 0
            && self.transparency_group == 0
            && self.raw_grammar == 0
            && self.color_authority == 0
            && self.xobject_authority == 0
            && self.extgstate_authority == 0
            && self.recursion_cycle == 0
            && self.recursion_depth == 0
            && self.target_budget == 0
            && self.decoded_byte_budget == 0
    }

    /// Count one refused demanded identity classified as `class`. Exhaustive
    /// over [`FormXObjectRefusalClass`] so a new variant breaks this match.
    pub(crate) const fn add(&mut self, class: FormXObjectRefusalClass) {
        match class {
            FormXObjectRefusalClass::StructuralPreflight => self.structural_preflight += 1,
            FormXObjectRefusalClass::StreamFilterOrExtent => self.stream_filter_or_extent += 1,
            FormXObjectRefusalClass::TransparencyGroup => self.transparency_group += 1,
            FormXObjectRefusalClass::RawGrammar => self.raw_grammar += 1,
            FormXObjectRefusalClass::ColorAuthority => self.color_authority += 1,
            FormXObjectRefusalClass::XObjectAuthority => self.xobject_authority += 1,
            FormXObjectRefusalClass::ExtGStateAuthority => self.extgstate_authority += 1,
            FormXObjectRefusalClass::RecursionCycle => self.recursion_cycle += 1,
            FormXObjectRefusalClass::RecursionDepth => self.recursion_depth += 1,
            FormXObjectRefusalClass::TargetBudget => self.target_budget += 1,
            FormXObjectRefusalClass::DecodedByteBudget => self.decoded_byte_budget += 1,
        }
    }

    /// Fold another page's counts into this one (field-wise sum). Used by
    /// tests that verify per-page counts sum to the analyzer's request-wide
    /// total.
    #[cfg(test)]
    pub(crate) const fn fold(&mut self, other: &Self) {
        self.structural_preflight += other.structural_preflight;
        self.stream_filter_or_extent += other.stream_filter_or_extent;
        self.transparency_group += other.transparency_group;
        self.raw_grammar += other.raw_grammar;
        self.color_authority += other.color_authority;
        self.xobject_authority += other.xobject_authority;
        self.extgstate_authority += other.extgstate_authority;
        self.recursion_cycle += other.recursion_cycle;
        self.recursion_depth += other.recursion_depth;
        self.target_budget += other.target_budget;
        self.decoded_byte_budget += other.decoded_byte_budget;
    }
}
