//! Closed page-alias epoch proof and refusal plan.
//!
//! [`AliasEpochPlan`] is the ONE abstraction between the per-setter structural
//! alias classification and the first alias conversion. It observes EVERY
//! successfully walked [`PaintOp`] of the existing single logical-page
//! [`presslint_paint::PaintProgram`] walk — before the converter's colour-only
//! branch — and proves, symbolically and byte-neutrally, which lane-specific
//! alias epochs are CLOSED (every apply precondition holds structurally) and
//! which are REFUSED (first reason in document order). The plan itself rewrites
//! nothing and runs no transform; its finished outcome is consumed once by the
//! converter's root-atomic alias pass.
//!
//! An epoch starts when an exact eligible `/Alias cs` or `/Alias CS` selects a
//! classified page device alias on one lane (stroking or nonstroking). Per ISO
//! 32000-1 §8.6.3 the selection immediately resets the lane colour, so the
//! selection record itself is the epoch's FIRST conversion candidate carrying
//! the exact initial source tuple (`[0]`, `[0, 0, 0]`, or `[0, 0, 0, 1]`).
//! Each epoch pairs symbolic source state (alias identity, device family, the
//! walker-carried source tuple) with symbolic emitted state (the ONE prepared
//! route fixed at selection: destination family plus link identity). Exact
//! eligible numeric setters add candidates and become the lane's pending
//! source tuple; `q` copies both lane pairs — tuple included — `Q` restores
//! them exactly (a consumer after `Q` is proved against the restored value),
//! and every `q`-derived branch carrying a root epoch is proved
//! or refused atomically with that root. Native transform construction and the
//! LCMS apply are deliberately NOT proven here. The converter dry-runs every
//! retained candidate before authorizing any splice for the root.
//!
//! Text showing is admitted as a colour CONSUMER only for an exact ordinary
//! indirect font (Type1/MMType1/TrueType/Type0) present in the page policy's
//! unpoisoned admitted set: its render mode consumes the current nonstroking
//! and/or stroking colour (0/4 fill, 1/5 stroke, 2/6 both, 3/7 neither) exactly
//! like a fill/stroke paint, and its bytes stay verbatim. Every unproven font
//! state — Type3, CID descendant, direct-without-identity, stale/missing/
//! unadmitted tuple, raw/unset/indeterminate selection — and every unsupported
//! render-mode value keep the historical fail-closed `TextShow` refusal, because
//! a non-ordinary current font may execute glyph content programs that inherit
//! graphics state and paint with the current colour.
//!
//! The proof is otherwise FAIL-CLOSED at every boundary the walker cannot
//! certify: inline
//! images, Type3 `d0`/`d1`, `BX`/`EX` compatibility sections, Pattern-name or
//! otherwise ineligible setters inside an active epoch, malformed
//! graphics-object placement (§8.2: text-object and path-object lifecycles
//! are both tracked, so `q`/`Q`/`cm` inside `BT`/`ET` and any non-path
//! operator inside an open path refuse), unknown operators, and any
//! disagreement between raw operator bytes, plan state, and the paint
//! snapshot. A named `Do` is classified through the page's exact matched
//! [`PageXObjectPolicy`]: an ordinary image is colour-neutral, a structurally
//! valid `/ImageMask true` stencil consumes the current nonstroking colour
//! (§8.9.6.2) exactly like a fill paint, a demanded ordinary form proved by the
//! request analyzer consumes only the inherited stroking/nonstroking lanes its
//! path paints read (a neutral form consumes neither and leaves roots live), and
//! a structural/unknown/unproven form, an unknown name, or an invalid image
//! keeps the historical fail-closed refusal.
//! Refusal affects alias plans only; the neighbouring direct-device
//! shortcut conversion keeps its own guards, decision order, and counts.
//!
//! The plan is also the sole production owner of the EXISTING structural
//! alias-setter tallies: it classifies every exact `sc`/`SC`/`scn`/`SCN` event
//! through [`PageDeviceSpacePolicy::classify_alias_setter`] with unchanged
//! per-setter semantics, so `resource_alias_setters_eligible/ineligible` keep
//! their exact public meaning while epoch status stays private.
//!
//! Cost: one bounded O(1) observation per record, O(q-depth) saved lane
//! frames, and O(epochs + candidates + repeated-record decisions) retained
//! state. No second walk, no materialized event vector, no snapshot clones —
//! pre/post graphics state is reused by reference from the walker's shared
//! `Rc` snapshots.

use std::collections::{BTreeMap, BTreeSet};

use presslint_paint::{
    DecodedRange, GraphicsColor, GraphicsStateSnapshot, PaintOp, PaintOpKind, PathPaintKind,
    TextRenderingMode,
};
use presslint_pdf::{IndirectObjectEditDisposition, IndirectRef};
use presslint_selectors::Selector;
use presslint_types::{ByteRange, ColorUsage, PageIndex, PdfName};

use crate::{
    content_color_convert::{DeviceColorSpace, classify_operator},
    link_routing::LinkRouting,
    page_content_sequence::PageContentSequence,
    page_device_space_policy::{
        AliasSetterClass, AliasSetterEvent, PageDeviceSpacePolicy, paint_color_space,
    },
    page_font_policy::PageFontPolicy,
    page_xobject_policy::{PageXObjectEffect, PageXObjectPolicy},
    selector_match::selector_matches_operator,
};

/// Which graphics-state colour lane an epoch lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaneSide {
    /// Stroking colour (`CS`/`SC`/`SCN`, consumed by stroke painting).
    Stroking,
    /// Nonstroking colour (`cs`/`sc`/`scn`, consumed by fill painting).
    Nonstroking,
}

impl LaneSide {
    /// Fixed lane-array slot: stroking first.
    const fn index(self) -> usize {
        match self {
            Self::Stroking => 0,
            Self::Nonstroking => 1,
        }
    }

    /// The selector usage this lane's candidates are evaluated under.
    const fn usage(self) -> ColorUsage {
        match self {
            Self::Stroking => ColorUsage::Stroke,
            Self::Nonstroking => ColorUsage::Fill,
        }
    }
}

/// How a symbolic conversion candidate entered its epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind {
    /// The alias selection record itself: selecting the space resets the lane
    /// colour to the exact PDF initial colour (ISO 32000-1 §8.6.3), so the
    /// selection is a replaceable candidate even with no explicit setter.
    SelectionInitial,
    /// An exact eligible numeric `sc`/`SC`/`scn`/`SCN` under the live alias.
    ExplicitSetter,
}

/// First structural reason an epoch was refused, in document order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochRefusalReason {
    /// No supplied link routes the alias's source device family.
    NoPreparedRoute,
    /// The fixed route's source or destination family is not proven raw-device
    /// safe under the matching effective `/Default*`.
    RouteDefaultUnsafe,
    /// The request selector excluded one candidate; the whole root refuses
    /// rather than selecting a prefix of an epoch.
    SelectorExcluded,
    /// A structurally ineligible setter (wrong shape, out-of-range component,
    /// trailing Pattern name, …) appeared inside the active epoch.
    IneligibleSetter,
    /// A colour setter inside the active epoch could not be classified at all;
    /// the paired state can no longer be trusted.
    UnclassifiedSetter,
    /// A required candidate record does not localize wholly to one physical
    /// occurrence of the page `/Contents`.
    CandidateNotLocalized,
    /// A required candidate record lives in an occurrence whose ownership
    /// decision vetoes in-place mutation.
    CandidateOwnershipVetoed,
    /// A repeated physical reference produced a different candidate decision
    /// or different symbolic facts for the same local record range.
    RepeatedReferenceMismatch,
    /// `Tj`/`TJ`/`'`/`"` while an alias was live and the effective font was not
    /// an admitted exact ordinary indirect font, or the render mode was an
    /// uninterpreted value outside 0-7. An admitted ordinary font consumes the
    /// mode's lanes instead of refusing.
    TextShow,
    /// A structural/unknown/unproven form, or an unknown or invalid named `Do`
    /// while an alias was live. Proven ordinary images are neutral, proven
    /// stencils consume the nonstroking lane, and an analyzed form consumes only
    /// its proven inherited lanes instead of refusing.
    XObjectInvoke,
    /// `BI`/`ID`/`EI` inline image or stencil semantics.
    InlineImage,
    /// Type3 glyph-metric operator `d0`/`d1`.
    Type3Operator,
    /// `BX`/`EX` compatibility section.
    CompatibilitySection,
    /// An operator outside the exact colour-neutral allowlist.
    UnknownOperator,
    /// A known-invalid graphics-object placement (ISO 32000-1 §8.2):
    /// unbalanced `BT`/`ET`, `EMC` underflow, `q`/`Q`/`cm`/path/shading
    /// operators inside a text object, path continuation or clipping without
    /// an open path, or any non-path operator inside an open path object.
    InvalidGraphicsObjectContext,
    /// Raw operator bytes, plan state, and the paint snapshot disagreed;
    /// disagreement is never permission.
    StateMismatch,
    /// A non-empty `q` stack remained at page end; ISO 32000-1 §8.4.2 requires
    /// balance across the logical page, so every alias plan refuses.
    UnbalancedSaveAtPageEnd,
}

/// Final private status of one root epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochStatus {
    /// Every structural apply precondition held through page-end closure.
    /// This does NOT claim native transform construction or apply success.
    Closed,
    /// The epoch refused; only the FIRST reason in document order is kept.
    Refused {
        /// Operator-record index of the refusing observation, when one exists
        /// (`None` for page-end closure refusals).
        record_index: Option<usize>,
        /// The structural refusal reason.
        reason: EpochRefusalReason,
    },
}

/// The ONE prepared route fixed for an epoch at alias selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochRoute {
    /// Request-order index of the routed device link.
    pub link_index: usize,
    /// The link's narrowed destination device family (the emitted family).
    pub destination: DeviceColorSpace,
}

/// One retained symbolic conversion candidate.
///
/// Fields are consumed directly by the converter's root dry-run and focused
/// proof tests; nothing here is published.
#[derive(Debug, Clone, PartialEq)]
pub struct EpochCandidate {
    /// How the candidate entered the epoch.
    pub kind: CandidateKind,
    /// Zero-based operator-record index in the logical sequence.
    pub record_index: usize,
    /// Physical occurrence holding the whole candidate record.
    pub occurrence_index: usize,
    /// Local record range inside that occurrence.
    pub local_range: ByteRange,
    /// Exact source components carried by the candidate (never transformed).
    pub components: Vec<f64>,
    /// Whether the canonical selector matcher accepted the candidate.
    pub selector_matched: bool,
}

/// Private closed/refused report for one root epoch.
///
/// Consumed directly by the converter; no field is published as an epoch API.
#[derive(Debug, Clone, PartialEq)]
pub struct AliasEpochReport {
    /// Alias resource name that selected the epoch.
    pub alias: PdfName,
    /// Lane the epoch lives on.
    pub side: LaneSide,
    /// The alias's exact source device family.
    pub source: DeviceColorSpace,
    /// The route fixed at selection, when one existed.
    pub route: Option<EpochRoute>,
    /// Retained symbolic candidates in document order.
    pub candidates: Vec<EpochCandidate>,
    /// Whether any supported paint consumer observed the epoch. A closed epoch
    /// with no consumer is a no-op candidate and authorizes no byte change.
    pub has_consumer: bool,
    /// Closed or first-refusal status.
    pub status: EpochStatus,
}

/// Everything the plan proves for one analysed page.
pub struct AliasEpochOutcome {
    /// Every root epoch in selection order with its private status.
    pub epochs: Vec<AliasEpochReport>,
    /// Structurally eligible setter tally per physical occurrence — exactly
    /// the per-setter classification the public counts have carried so far.
    pub eligible_setters: Vec<usize>,
    /// Structurally ineligible setter tally per physical occurrence.
    pub ineligible_setters: Vec<usize>,
}

/// One live lane branch: which root epoch the lane currently carries plus the
/// branch-local CURRENT source tuple (the pending candidate's identity).
///
/// The tuple is copied into every `q`-saved frame and restored exactly by
/// `Q`, so the plan itself proves the restored paired state — e.g. that a
/// paint after `q 0.8 sc Q` consumes the pre-frame value — instead of
/// trusting alias name and family alone. Components are tiny (1/3/4 floats).
#[derive(Debug, Clone)]
struct LaneBranch {
    root: usize,
    /// The walker-agreed source components currently pending on this branch.
    components: Vec<f64>,
    /// Identity of the record that established the pending value: the
    /// walker's colour-provenance range, which itself travels through
    /// `q`/`Q`. A consumer proves the restored colour is the exact pending
    /// candidate, not merely an equal-looking one.
    source: Option<DecodedRange>,
}

/// Accumulating state for one root epoch.
struct EpochState {
    alias: PdfName,
    side: LaneSide,
    source: DeviceColorSpace,
    route: Option<EpochRoute>,
    candidates: Vec<EpochCandidate>,
    has_consumer: bool,
    /// Live branches carrying this root: the selecting lane plus every
    /// `q`-saved copy. Zero means every branch terminated or was restored
    /// away; boundary refusals no longer reach the epoch.
    live_branches: usize,
    refusal: Option<(Option<usize>, EpochRefusalReason)>,
}

/// Symbolic facts recorded for one candidate decision on one physical record,
/// compared verbatim across repeated occurrences. Route, destination, and
/// `/Default*` status are page-constant functions of `(alias, side)` and need
/// no separate comparison.
#[derive(Debug, Clone, PartialEq)]
struct RecordFacts {
    kind: CandidateKind,
    side: LaneSide,
    alias: PdfName,
    source: DeviceColorSpace,
    components: Vec<f64>,
    selector_matched: bool,
}

/// Candidate/no-candidate decision for one physical record of a repeated
/// content object. `facts: None` records an observed NON-candidate. EVERY
/// root that relied on the decision is accumulated so a later divergent
/// occurrence refuses all of them, and a diverged record stays permanently
/// poisoned: no root that touches it afterwards can close.
struct RecordDecision {
    roots: Vec<usize>,
    facts: Option<RecordFacts>,
    poisoned: bool,
}

/// Streaming closed/refused proof over one logical page walk. See the module
/// docs for the full contract.
pub struct AliasEpochPlan<'a> {
    policy: &'a PageDeviceSpacePolicy,
    routing: &'a LinkRouting,
    /// Page-exact named `XObject` colour effects for `Do` classification.
    xobjects: &'a PageXObjectPolicy<'a>,
    /// Page-exact font policy: exact ordinary-font admission for `TextShow`.
    fonts: &'a PageFontPolicy,
    target: Option<&'a Selector>,
    page_index: PageIndex,
    /// Current lane pair: stroking then nonstroking.
    lanes: [Option<LaneBranch>; 2],
    /// Shadow `q` stack of saved lane pairs, mirroring the walker exactly.
    saved: Vec<[Option<LaneBranch>; 2]>,
    epochs: Vec<EpochState>,
    /// Whether a `BT` text object is currently open.
    in_text_object: bool,
    /// Whether a path object begun by `m`/`re` is still open (ISO 32000-1
    /// §8.2 admits only construction, clipping, and painting until the paint).
    in_path_object: bool,
    /// Open `BMC`/`BDC` nesting depth.
    marked_content_depth: usize,
    /// Content objects with more than one physical occurrence.
    repeated_objects: BTreeSet<IndirectRef>,
    /// Deterministic per-physical-record decisions for repeated objects.
    record_decisions: BTreeMap<(IndirectRef, usize, usize), RecordDecision>,
    eligible_setters: Vec<usize>,
    ineligible_setters: Vec<usize>,
}

impl<'a> AliasEpochPlan<'a> {
    /// Build the plan for one parsed logical page sequence.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        policy: &'a PageDeviceSpacePolicy,
        routing: &'a LinkRouting,
        xobjects: &'a PageXObjectPolicy<'a>,
        fonts: &'a PageFontPolicy,
        target: Option<&'a Selector>,
        page_index: PageIndex,
        sequence: &PageContentSequence,
    ) -> Self {
        let occurrence_count = sequence.occurrence_count();
        let mut seen = BTreeSet::new();
        let mut repeated_objects = BTreeSet::new();
        for index in 0..occurrence_count {
            if let Some(object) = sequence.occurrence_object(index) {
                if !seen.insert(object) {
                    repeated_objects.insert(object);
                }
            }
        }
        Self {
            policy,
            routing,
            xobjects,
            fonts,
            target,
            page_index,
            lanes: [None, None],
            saved: Vec::new(),
            epochs: Vec::new(),
            in_text_object: false,
            in_path_object: false,
            marked_content_depth: 0,
            repeated_objects,
            record_decisions: BTreeMap::new(),
            eligible_setters: vec![0; occurrence_count],
            ineligible_setters: vec![0; occurrence_count],
        }
    }

    /// Observe one successfully walked paint op, in walk order, BEFORE the
    /// converter's colour-only branch. `operator` is the exact raw bytes at
    /// the op's operator range; `state_before` is the shared pre-op snapshot
    /// (seeded from [`GraphicsStateSnapshot::page_default`] for the first op).
    pub fn observe(
        &mut self,
        op: &PaintOp,
        operator: &[u8],
        state_before: &GraphicsStateSnapshot,
        sequence: &PageContentSequence,
    ) {
        match &op.kind {
            // `q`/`Q` and `cm` are valid at page-description level only:
            // ISO 32000-1 §8.2 excludes them from text AND path objects. The
            // shadow stack still mirrors the walker (which tolerates them) so
            // later frames never diverge — refusal is not permission to drift.
            PaintOpKind::Save => {
                if self.in_text_object || self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
                self.save();
            }
            PaintOpKind::Restore => {
                if self.in_text_object || self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
                self.restore(op);
            }
            PaintOpKind::SetStrokingColor { color } => {
                self.color_operator(
                    LaneSide::Stroking,
                    color,
                    op,
                    operator,
                    state_before,
                    sequence,
                );
            }
            PaintOpKind::SetNonstrokingColor { color } => {
                self.color_operator(
                    LaneSide::Nonstroking,
                    color,
                    op,
                    operator,
                    state_before,
                    sequence,
                );
            }
            PaintOpKind::PathPaint { paint } => self.path_paint(*paint, op),
            PaintOpKind::TextShow { rendering_mode, .. } => self.text_show(op, *rendering_mode),
            // An external object is its own graphics object (§8.2): valid at
            // page-description level only, so invalid placement refuses BEFORE
            // any classification. A proven ordinary image never touches the
            // current colour; a proven stencil paints its 1-samples with the
            // current nonstroking colour (§8.9.6.2), so it consumes the fill
            // lane exactly like a fill paint — the stroking lane is untouched.
            // Everything unproven keeps the historical fail-closed refusal.
            PaintOpKind::XObjectInvoke { name } => {
                if self.in_text_object || self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                } else {
                    match self.xobjects.effect_of(name) {
                        PageXObjectEffect::OrdinaryImage => {}
                        PageXObjectEffect::Stencil => self.consume(LaneSide::Nonstroking, op),
                        // A demanded ordinary Form proved a bounded inherited-
                        // lane effect: it consumes ONLY the lanes its path paints
                        // read from the caller. A neutral analyzed Form (neither
                        // lane) leaves alias roots live and is not a refusal.
                        PageXObjectEffect::AnalyzedForm {
                            consumes_stroking,
                            consumes_nonstroking,
                        } => {
                            if consumes_stroking {
                                self.consume(LaneSide::Stroking, op);
                            }
                            if consumes_nonstroking {
                                self.consume(LaneSide::Nonstroking, op);
                            }
                        }
                        PageXObjectEffect::Form | PageXObjectEffect::Unknown => {
                            self.refuse_open(op, EpochRefusalReason::XObjectInvoke);
                        }
                    }
                }
            }
            PaintOpKind::ConcatMatrix { .. } => {
                if self.in_text_object || self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
            }
            // `Tr` and `Tf` are fully modelled text state (valid in text
            // objects and at page level); `gs` is colour-neutral here because
            // unsafe ExtGState activation already skipped the whole page in
            // the converter preflight. None belongs inside an open path.
            PaintOpKind::SetTextRenderingMode { .. }
            | PaintOpKind::SetFont { .. }
            | PaintOpKind::SetExtGState { .. } => {
                if self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
            }
            PaintOpKind::NoOp => self.unmodelled_operator(operator, op),
        }
    }

    /// Close the plan at page end and surrender the outcome.
    ///
    /// Root epochs close only when the `q` stack is empty; a leftover frame
    /// refuses EVERY alias plan for the page (the walker tolerates trailing
    /// unmatched `q`, ISO 32000-1 §8.4.2 does not). An unterminated text,
    /// path, or marked-content object refuses the epochs still open.
    pub fn finish(mut self) -> AliasEpochOutcome {
        if self.saved.is_empty() {
            if self.in_text_object || self.in_path_object || self.marked_content_depth > 0 {
                self.refuse_live(None, EpochRefusalReason::InvalidGraphicsObjectContext);
            }
        } else {
            for epoch in &mut self.epochs {
                if epoch.refusal.is_none() {
                    epoch.refusal = Some((None, EpochRefusalReason::UnbalancedSaveAtPageEnd));
                }
            }
        }
        AliasEpochOutcome {
            epochs: self
                .epochs
                .into_iter()
                .map(|epoch| AliasEpochReport {
                    alias: epoch.alias,
                    side: epoch.side,
                    source: epoch.source,
                    route: epoch.route,
                    candidates: epoch.candidates,
                    has_consumer: epoch.has_consumer,
                    status: epoch
                        .refusal
                        .map_or(EpochStatus::Closed, |(record_index, reason)| {
                            EpochStatus::Refused {
                                record_index,
                                reason,
                            }
                        }),
                })
                .collect(),
            eligible_setters: self.eligible_setters,
            ineligible_setters: self.ineligible_setters,
        }
    }

    /// `q`: copy BOTH lane pairs — root identity AND the pending source tuple
    /// — so `Q` restores the exact paired state; each copied branch is a live
    /// branch of its root epoch and is proved atomically with it.
    fn save(&mut self) {
        let frame = self.lanes.clone();
        for branch in frame.iter().flatten() {
            self.epochs[branch.root].live_branches += 1;
        }
        self.saved.push(frame);
    }

    /// `Q`: discard the branches introduced inside the frame and restore both
    /// saved pairs exactly. Underflow cannot reach here — the walker fails the
    /// whole page first — but disagreement is still refusal, never permission.
    fn restore(&mut self, op: &PaintOp) {
        let Some(frame) = self.saved.pop() else {
            self.refuse_open(op, EpochRefusalReason::StateMismatch);
            return;
        };
        self.drop_branch(LaneSide::Stroking);
        self.drop_branch(LaneSide::Nonstroking);
        self.lanes = frame;
    }

    /// Terminate the lane's current branch, if any.
    fn drop_branch(&mut self, side: LaneSide) {
        if let Some(branch) = self.lanes[side.index()].take() {
            self.epochs[branch.root].live_branches -= 1;
        }
    }

    /// Dispatch one colour event by its exact raw operator bytes.
    fn color_operator(
        &mut self,
        side: LaneSide,
        color: &GraphicsColor,
        op: &PaintOp,
        operator: &[u8],
        state_before: &GraphicsStateSnapshot,
        sequence: &PageContentSequence,
    ) {
        // Colour operators are admitted inside a text object (§8.2) but never
        // inside an open path. The refusal fires both BEFORE dispatch
        // (reaching a branch this op terminates) and AFTER (reaching an epoch
        // this op just selected).
        let invalid_here = self.in_path_object;
        if invalid_here {
            self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
        }
        // A direct device shortcut terminates the lane branch; the shortcut
        // itself remains the direct converter's candidate, untouched here.
        if classify_operator(operator).is_some() {
            self.drop_branch(side);
        } else {
            match operator {
                b"cs" | b"CS" => self.select_color_space(side, color, op, operator, sequence),
                b"sc" | b"SC" | b"scn" | b"SCN" => {
                    self.set_color_value(side, color, op, operator, state_before, sequence);
                }
                // The walker emits colour events only for the operators above;
                // anything else is a state disagreement, refused fail-closed.
                _ => self.refuse_open(op, EpochRefusalReason::StateMismatch),
            }
        }
        if invalid_here {
            self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
        }
    }

    /// `cs`/`CS`: terminate the lane's branch; an exact ELIGIBLE page device
    /// alias starts a new independent root epoch whose selection record is its
    /// first candidate.
    fn select_color_space(
        &mut self,
        side: LaneSide,
        color: &GraphicsColor,
        op: &PaintOp,
        operator: &[u8],
        sequence: &PageContentSequence,
    ) {
        if (operator == b"CS") != (side == LaneSide::Stroking) {
            self.refuse_open(op, EpochRefusalReason::StateMismatch);
            return;
        }
        self.drop_branch(side);
        let Some(name) = color.resource_name.as_ref() else {
            self.refuse_open(op, EpochRefusalReason::StateMismatch);
            return;
        };
        let decision = self
            .policy
            .alias_decision(name)
            .filter(|decision| decision.eligible);
        let Some(decision) = decision else {
            self.note_record_decision(op, sequence, None, None);
            return;
        };
        let source = decision.space;
        let route = self.routing.route(source).map(|link| EpochRoute {
            link_index: link.index,
            destination: link.destination,
        });
        let root = self.epochs.len();
        self.epochs.push(EpochState {
            alias: name.clone(),
            side,
            source,
            route,
            candidates: Vec::new(),
            has_consumer: false,
            live_branches: 1,
            refusal: None,
        });
        // The branch tuple always mirrors the walker's own components so the
        // paired state stays bit-consistent across `q`/`Q` restoration.
        self.lanes[side.index()] = Some(LaneBranch {
            root,
            components: color.components.clone(),
            source: color.source,
        });
        // Paired-state agreement: an eligible alias resolved in the paint
        // environment, so the walker must report the alias family and the
        // exact PDF initial colour (§8.6.3).
        let initial = initial_components(source);
        if color.space != paint_color_space(source) || !components_match(&color.components, initial)
        {
            self.refuse(root, Some(op.index), EpochRefusalReason::StateMismatch);
        }
        match route {
            None => self.refuse(root, Some(op.index), EpochRefusalReason::NoPreparedRoute),
            Some(route) if !self.policy.route_is_raw_device(source, route.destination) => {
                self.refuse(root, Some(op.index), EpochRefusalReason::RouteDefaultUnsafe);
            }
            Some(_) => {}
        }
        self.add_candidate(
            root,
            CandidateKind::SelectionInitial,
            op,
            initial.to_vec(),
            sequence,
        );
    }

    /// `sc`/`SC`/`scn`/`SCN`: classify the setter structurally (the unchanged
    /// public tallies), then bind it to the lane's live epoch as a candidate
    /// or a refusal.
    fn set_color_value(
        &mut self,
        side: LaneSide,
        color: &GraphicsColor,
        op: &PaintOp,
        operator: &[u8],
        state_before: &GraphicsStateSnapshot,
        sequence: &PageContentSequence,
    ) {
        if matches!(operator, b"SC" | b"SCN") != (side == LaneSide::Stroking) {
            self.refuse_open(op, EpochRefusalReason::StateMismatch);
            return;
        }
        let class = self.tally_setter(side, color, op, operator, state_before, sequence);
        let Some(root) = self.lanes[side.index()].as_ref().map(|branch| branch.root) else {
            // No live epoch on this lane: the structural tally above is the
            // whole effect, but the no-candidate decision still participates
            // in repeated-reference consistency.
            self.note_record_decision(op, sequence, None, None);
            return;
        };
        let reason = match class {
            Some(AliasSetterClass::Eligible) => {
                let agrees = color.resource_name.as_ref() == Some(&self.epochs[root].alias)
                    && color.space == paint_color_space(self.epochs[root].source);
                if agrees {
                    // The setter becomes the branch's new pending source
                    // value; a saved outer frame keeps the pre-`q` pair.
                    if let Some(branch) = self.lanes[side.index()].as_mut() {
                        branch.components.clone_from(&color.components);
                        branch.source = color.source;
                    }
                    self.add_candidate(
                        root,
                        CandidateKind::ExplicitSetter,
                        op,
                        color.components.clone(),
                        sequence,
                    );
                    return;
                }
                EpochRefusalReason::StateMismatch
            }
            Some(AliasSetterClass::Ineligible) => EpochRefusalReason::IneligibleSetter,
            None => EpochRefusalReason::UnclassifiedSetter,
        };
        self.refuse(root, Some(op.index), reason);
        self.note_record_decision(op, sequence, Some(root), None);
    }

    /// Classify one setter with EXACT T177 per-setter semantics and attribute
    /// the structural tally to the occurrence holding the operator token.
    fn tally_setter(
        &mut self,
        side: LaneSide,
        color: &GraphicsColor,
        op: &PaintOp,
        operator: &[u8],
        state_before: &GraphicsStateSnapshot,
        sequence: &PageContentSequence,
    ) -> Option<AliasSetterClass> {
        let stroking = side == LaneSide::Stroking;
        let selected_resource_name = if stroking {
            state_before.stroking_color.resource_name.as_ref()
        } else {
            state_before.nonstroking_color.resource_name.as_ref()
        };
        let record = sequence.records().get(op.index)?;
        let event = AliasSetterEvent {
            operator,
            stroking,
            selected_resource_name,
            color,
            record,
            tokens: sequence.tokens(),
            localized: sequence.localize(op.record_range).is_some(),
        };
        let class = self.policy.classify_alias_setter(&event)?;
        let location = sequence.localize(op.operator_range)?;
        match class {
            AliasSetterClass::Eligible => self.eligible_setters[location.occurrence_index] += 1,
            AliasSetterClass::Ineligible => self.ineligible_setters[location.occurrence_index] += 1,
        }
        Some(class)
    }

    /// Add one symbolic candidate to `root`: selector, localization,
    /// ownership, and repeated-reference checks all run here.
    fn add_candidate(
        &mut self,
        root: usize,
        kind: CandidateKind,
        op: &PaintOp,
        components: Vec<f64>,
        sequence: &PageContentSequence,
    ) {
        let side = self.epochs[root].side;
        let source = self.epochs[root].source;
        // Every candidate — selection-initial included — passes the canonical
        // selector matcher on its SOURCE facts; one excluded candidate refuses
        // the complete root (never a prefix of an epoch).
        let selector_matched = self.target.is_none_or(|selector| {
            selector_matches_operator(selector, self.page_index, source, side.usage(), &components)
        });
        if !selector_matched {
            self.refuse(root, Some(op.index), EpochRefusalReason::SelectorExcluded);
        }
        let Some(location) = sequence.localize(op.record_range) else {
            self.refuse(
                root,
                Some(op.index),
                EpochRefusalReason::CandidateNotLocalized,
            );
            return;
        };
        if location.disposition != IndirectObjectEditDisposition::InPlaceMutation {
            self.refuse(
                root,
                Some(op.index),
                EpochRefusalReason::CandidateOwnershipVetoed,
            );
        }
        let facts = RecordFacts {
            kind,
            side,
            alias: self.epochs[root].alias.clone(),
            source,
            components: components.clone(),
            selector_matched,
        };
        self.epochs[root].candidates.push(EpochCandidate {
            kind,
            record_index: op.index,
            occurrence_index: location.occurrence_index,
            local_range: location.local_range,
            components,
            selector_matched,
        });
        self.note_record_decision(op, sequence, Some(root), Some(facts));
    }

    /// Record (and cross-check) one candidate/no-candidate decision for a
    /// physical record of a REPEATED content object. Identical facts prove
    /// once and every relying root is accumulated; any divergence refuses ALL
    /// of them and permanently poisons the record, so no root touching it
    /// later can close — a conflicting future rewrite is never authorized.
    fn note_record_decision(
        &mut self,
        op: &PaintOp,
        sequence: &PageContentSequence,
        root: Option<usize>,
        facts: Option<RecordFacts>,
    ) {
        let Some(location) = sequence.localize(op.record_range) else {
            return;
        };
        if !self.repeated_objects.contains(&location.content_object) {
            return;
        }
        let key = (
            location.content_object,
            location.local_range.start,
            location.local_range.end,
        );
        let Some(previous) = self.record_decisions.get_mut(&key) else {
            self.record_decisions.insert(
                key,
                RecordDecision {
                    roots: root.into_iter().collect(),
                    facts,
                    poisoned: false,
                },
            );
            return;
        };
        if let Some(root) = root {
            if !previous.roots.contains(&root) {
                previous.roots.push(root);
            }
        }
        if !previous.poisoned && previous.facts == facts {
            return;
        }
        previous.poisoned = true;
        let targets = previous.roots.clone();
        for target in targets {
            self.refuse(
                target,
                Some(op.index),
                EpochRefusalReason::RepeatedReferenceMismatch,
            );
        }
    }

    /// A path paint consumed the lanes its kind uses (ISO 32000-1 §8.5.3);
    /// `n` consumes neither. Painting ends any open path object; paints are
    /// invalid inside a text object.
    fn path_paint(&mut self, paint: PathPaintKind, op: &PaintOp) {
        if self.in_text_object {
            self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
        }
        self.in_path_object = false;
        if paint.uses_stroke() {
            self.consume(LaneSide::Stroking, op);
        }
        if paint.uses_fill() {
            self.consume(LaneSide::Nonstroking, op);
        }
    }

    /// A text-showing operator (`Tj`/`TJ`/`'`/`"`) consumes the current colour
    /// selected by its text rendering mode — but ONLY when the effective font
    /// is an exact ordinary indirect font in the page policy's admitted set.
    ///
    /// Text showing is valid only inside a text object and never inside an open
    /// path (§8.2); invalid placement refuses the structural way. An unadmitted
    /// font (Type3, CID descendant, direct-without-identity, stale offset,
    /// missing/malformed/duplicate/unsupported, or a raw/unset/indeterminate
    /// selection) keeps the historical fail-closed `TextShow` refusal, because
    /// a non-ordinary current font may execute glyph content programs that
    /// paint with the current colour. For an admitted font the render mode
    /// selects the consumed lanes (ISO 32000-1 §9.3.6, Table 106): 0/4
    /// nonstroking, 1/5 stroking, 2/6 both, 3/7 neither. Modes 3/7 mark no root
    /// used and authorize no colour splice; every other unsupported mode value
    /// refuses. `TextShow` is a consumer only — no byte, string, spacing, font,
    /// matrix, or clipping operand is ever a conversion candidate.
    fn text_show(&mut self, op: &PaintOp, mode: TextRenderingMode) {
        // An unadmitted font is the primary reason and keeps the historical
        // `TextShow` refusal regardless of context or mode: a non-ordinary
        // current font may execute glyph programs that paint. Only an admitted
        // exact ordinary indirect font is scrutinized for context and mode.
        if !self.fonts.admits(&op.state.font_selection) {
            self.refuse_open(op, EpochRefusalReason::TextShow);
            return;
        }
        if !self.in_text_object || self.in_path_object {
            self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
            return;
        }
        let Some((uses_fill, uses_stroke)) = text_show_lanes(mode) else {
            self.refuse_open(op, EpochRefusalReason::TextShow);
            return;
        };
        if uses_stroke {
            self.consume(LaneSide::Stroking, op);
        }
        if uses_fill {
            self.consume(LaneSide::Nonstroking, op);
        }
    }

    /// Mark the lane's live epoch as consumed, cross-checking that the post-op
    /// snapshot still carries the epoch's paired source state — alias
    /// identity, device family, AND the branch's pending source tuple (the
    /// value a `Q` restoration must have brought back exactly).
    fn consume(&mut self, side: LaneSide, op: &PaintOp) {
        let Some(branch) = &self.lanes[side.index()] else {
            return;
        };
        let root = branch.root;
        let color = match side {
            LaneSide::Stroking => &op.state.stroking_color,
            LaneSide::Nonstroking => &op.state.nonstroking_color,
        };
        let epoch = &self.epochs[root];
        let agrees = color.resource_name.as_ref() == Some(&epoch.alias)
            && color.space == paint_color_space(epoch.source)
            && components_match(&color.components, &branch.components)
            && color.source == branch.source;
        if agrees {
            self.epochs[root].has_consumer = true;
        } else {
            self.refuse(root, Some(op.index), EpochRefusalReason::StateMismatch);
        }
    }

    /// Classify one walker-unmodelled operator by its exact raw bytes: an
    /// exact colour-neutral allowlist is admitted, everything else refuses
    /// fail-closed while an alias is live. This is a colour-proof boundary,
    /// not a general validator: context tracking exists only to refuse known
    /// invalid placement, never to certify malformed semantics.
    fn unmodelled_operator(&mut self, operator: &[u8], op: &PaintOp) {
        match operator {
            // `m`/`re` BEGIN (or continue) a path object: geometry only,
            // neutral to the current colour, invalid inside a text object
            // (§8.2).
            b"m" | b"re" => {
                if self.in_text_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                } else {
                    self.in_path_object = true;
                }
            }
            // Path continuation and clipping are valid ONLY inside an open
            // path object (§8.2 admits them between `m`/`re` and the paint).
            b"l" | b"c" | b"v" | b"y" | b"h" | b"W" | b"W*" => {
                if self.in_text_object || !self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
            }
            // A shading owns its colour space (§8.7.4.2): neutral to the
            // CURRENT colour, valid at page-description level only.
            b"sh" => {
                if self.in_text_object || self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
            }
            // Line/rendering parameters, text state/positioning WITHOUT text
            // showing, and marked-content points never touch the current
            // colour — but none of them belongs inside an open path object.
            b"w" | b"J" | b"j" | b"M" | b"d" | b"ri" | b"i" | b"Tc" | b"Tw" | b"Tz" | b"TL"
            | b"Ts" | b"Td" | b"TD" | b"Tm" | b"T*" | b"MP" | b"DP" => {
                if self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
            }
            b"BT" => {
                if self.in_text_object || self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
                self.in_text_object = true;
            }
            b"ET" => {
                if self.in_text_object {
                    self.in_text_object = false;
                } else {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
            }
            // Marked-content sections are valid in page and text contexts,
            // never inside an open path; depth still tracks on refusal so the
            // shadow nesting never drifts from the byte stream.
            b"BMC" | b"BDC" => {
                if self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
                self.marked_content_depth += 1;
            }
            b"EMC" => {
                if self.in_path_object {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
                if let Some(depth) = self.marked_content_depth.checked_sub(1) {
                    self.marked_content_depth = depth;
                } else {
                    self.refuse_open(op, EpochRefusalReason::InvalidGraphicsObjectContext);
                }
            }
            b"BI" | b"ID" | b"EI" => self.refuse_open(op, EpochRefusalReason::InlineImage),
            b"d0" | b"d1" => self.refuse_open(op, EpochRefusalReason::Type3Operator),
            b"BX" | b"EX" => self.refuse_open(op, EpochRefusalReason::CompatibilitySection),
            _ => self.refuse_open(op, EpochRefusalReason::UnknownOperator),
        }
    }

    /// Refuse every root epoch that still has a live branch anywhere (current
    /// lanes or saved frames): a boundary the plan cannot certify poisons all
    /// alias state it could still reach, including branches that resume after
    /// a later `Q`.
    fn refuse_open(&mut self, op: &PaintOp, reason: EpochRefusalReason) {
        self.refuse_live(Some(op.index), reason);
    }

    /// The shared live-branch refusal pass behind [`Self::refuse_open`] and
    /// the page-end closure (`record_index: None`).
    fn refuse_live(&mut self, record_index: Option<usize>, reason: EpochRefusalReason) {
        for epoch in &mut self.epochs {
            if epoch.live_branches > 0 && epoch.refusal.is_none() {
                epoch.refusal = Some((record_index, reason));
            }
        }
    }

    /// Record the FIRST refusal of one root epoch, in document order.
    fn refuse(&mut self, root: usize, record_index: Option<usize>, reason: EpochRefusalReason) {
        let epoch = &mut self.epochs[root];
        if epoch.refusal.is_none() {
            epoch.refusal = Some((record_index, reason));
        }
    }
}

/// Map a text rendering mode to `(uses_fill, uses_stroke)` lane consumption, or
/// `None` when the mode value is not one of the interpreted 0-7 lanes.
///
/// Modes 4-7 remain paint's raw `Unsupported { value }` representation and are
/// interpreted writer-locally only: 4/5/6 add clipping to 0/1/2's colour
/// consumption, and 7 is clip-only. This adds no claim that `PressLint` models
/// the accumulated clipping path (§9.3.6). Modes 3/7 consume neither lane.
const fn text_show_lanes(mode: TextRenderingMode) -> Option<(bool, bool)> {
    match mode {
        TextRenderingMode::Fill | TextRenderingMode::Unsupported { value: 4 } => {
            Some((true, false))
        }
        TextRenderingMode::Stroke | TextRenderingMode::Unsupported { value: 5 } => {
            Some((false, true))
        }
        TextRenderingMode::FillThenStroke | TextRenderingMode::Unsupported { value: 6 } => {
            Some((true, true))
        }
        TextRenderingMode::Invisible | TextRenderingMode::Unsupported { value: 7 } => {
            Some((false, false))
        }
        TextRenderingMode::Unsupported { .. } => None,
    }
}

/// The exact PDF initial colour a device-family selection establishes
/// (ISO 32000-1 §8.6.3): Gray `[0]`, RGB `[0, 0, 0]`, CMYK `[0, 0, 0, 1]`.
const fn initial_components(space: DeviceColorSpace) -> &'static [f64] {
    match space {
        DeviceColorSpace::Gray => &[0.0],
        DeviceColorSpace::Rgb => &[0.0, 0.0, 0.0],
        DeviceColorSpace::Cmyk => &[0.0, 0.0, 0.0, 1.0],
    }
}

/// Exact bitwise component-tuple agreement (both sides come from the same
/// deterministic initial-colour construction, so bit equality is the honest
/// comparison and avoids float tolerance).
fn components_match(observed: &[f64], expected: &[f64]) -> bool {
    observed.len() == expected.len()
        && observed
            .iter()
            .zip(expected)
            .all(|(a, b)| a.to_bits() == b.to_bits())
}
