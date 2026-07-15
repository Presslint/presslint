//! Depth-indexed nested-Form colour-effect lattice.
//!
//! [`BoundedFormLaneEffects`] is the ONE new domain abstraction of the bounded
//! nested-Form recursion: a per-form analysis outcome indexed by remaining
//! descent depth. Slot `d` is the two-bit/Unknown lane effect of the form when
//! `d` more nested-Form `Do` edges may still be followed; slot 0 refuses every
//! invoked nested Form while still admitting local colour, ordinary Images and
//! stencils; the maximum-depth slot is the public analyze result. Every
//! computed slot is a pure function of the form's OWN subtree — never of the
//! path that reached it — so one cached lattice serves every caller at every
//! depth in any analysis order.
//!
//! Because the traversal itself is bounded to [`MAX_NESTED_FORM_DEPTH`] edges
//! below the frame under analysis, a lattice may be PARTIAL: slot `d` demands
//! child slots down `d` further edges, so a frame entered deep on a path can
//! only compute its low slots. `computed_through` records the highest slot
//! actually computed; slots above it are UNCOMPUTED, not refused, and hold
//! `None` so an incomplete lattice always reads fail-closed at the public
//! seam. A later shallower re-entry recomputes the frame and extends the
//! entry; computed slot values never change, which is what keeps cached
//! outcomes pure and query results order-independent. Each successful
//! deepening re-decode charges its decoded bytes; a budget-dependent failed
//! raw read or Flate inflation exhausts the residual aggregate byte budget,
//! preserves the partial entry, and prevents the same attempt from recurring.
//! Intrinsic decode failures retain their deterministic refusal behavior.
//!
//! This module owns ONLY the lattice contract: the depth constant, the
//! neutral/all-Unknown constructors, the shared-state lane marking, and the
//! per-slot fold applied at a parent's nested-Form `Do`. Name resolution
//! (`xobjects`), colour validation (`color`), caching, budgets, decoding and
//! the active-path cycle set all stay with their existing owners in the parent
//! module.

use super::FormLaneEffect;

/// Maximum admitted nested-Form `Do` EDGES on one analysis path. The root form
/// is not counted, so a full-depth path holds nine form streams. Slot `d`
/// answers "`d` edges remaining"; a root analysis reads slot
/// [`MAX_NESTED_FORM_DEPTH`].
pub(super) const MAX_NESTED_FORM_DEPTH: usize = 8;

/// One analyzed form's colour-lane outcome for every remaining descent depth:
/// `slots[d]` is `Some(effect)` when the form's effect is proven with `d`
/// nested-Form edges remaining, `None` when it is Unknown at that depth. An
/// Unknown slot is absorbing: no later operation of the same walk can restore
/// it, so a positive prefix never survives an unsupported suffix per slot.
///
/// Slots above `computed_through` were never computed (the depth horizon cut
/// the demand chain) and are `None` by construction: distinct in MEANING from
/// a refused slot, identical in fail-closed effect wherever they are read.
#[derive(Clone, Copy)]
pub(super) struct BoundedFormLaneEffects {
    slots: [Option<FormLaneEffect>; MAX_NESTED_FORM_DEPTH + 1],
    /// Highest slot index this lattice has actually computed. A complete
    /// lattice reaches [`MAX_NESTED_FORM_DEPTH`]; a partial one stops where
    /// the traversal horizon cut demand below the frame.
    computed_through: usize,
}

impl BoundedFormLaneEffects {
    /// The all-Unknown COMPLETE lattice: every slot computed and refused.
    /// Cached unconditionally for intrinsic refusals and unaffordable frames,
    /// and returned for an active-path cycle re-encounter (a cycle closes
    /// from every entry that reaches it, so all-Unknown is the member's true
    /// value at every depth).
    pub(super) const fn all_unknown() -> Self {
        Self {
            slots: [None; MAX_NESTED_FORM_DEPTH + 1],
            computed_through: MAX_NESTED_FORM_DEPTH,
        }
    }

    /// The neutral walk accumulator: every slot proven with no consumption
    /// yet. Folds along the walk can only lower `computed_through`.
    pub(super) const fn neutral() -> Self {
        Self {
            slots: [Some([false, false]); MAX_NESTED_FORM_DEPTH + 1],
            computed_through: MAX_NESTED_FORM_DEPTH,
        }
    }

    /// Highest computed slot index. The analyzer recomputes a cached frame
    /// only when re-entered with more remaining edges than this.
    pub(super) const fn computed_through(self) -> usize {
        self.computed_through
    }

    /// The maximum-depth slot: the public analyze result of one cached form.
    /// On a partial lattice the top slot is uncomputed `None`, so an
    /// unfinished frame always reads Unknown at the public seam.
    pub(super) const fn max_depth_effect(self) -> Option<FormLaneEffect> {
        self.slots[MAX_NESTED_FORM_DEPTH]
    }

    /// Mark one lane consumed in every still-proven slot. Graphics state is
    /// shared across slots (ISO 32000-1 §8.10.1 restores state after every
    /// `Do`), so a sentinel-live path paint or stencil consumption applies to
    /// every depth identically; slots differ only at nested-Form folds.
    pub(super) fn mark_consumed(&mut self, lane: usize) {
        for effect in self.slots.iter_mut().flatten() {
            effect[lane] = true;
        }
    }

    /// Fold one invoked nested-Form child into this parent lattice at the
    /// parent's `Do` site.
    ///
    /// Parent slot 0 has no edge left and becomes Unknown. Each parent slot
    /// `d >= 1` folds the child's slot `d - 1`: an Unknown child slot refuses
    /// the parent slot, and a proven child bit propagates onto the parent
    /// exactly when the parent's matching lane still equals its inherited
    /// sentinel at the invocation (`lane_live`); a live local parent colour
    /// absorbs the child's consumption locally and no caller lane is touched.
    ///
    /// A partial child bounds the parent: parent slot `d` is computable only
    /// through child slot `d - 1`, so `computed_through` drops to the child's
    /// reach plus one and every higher parent slot stays uncomputed.
    pub(super) fn fold_nested_form(&mut self, child: Self, lane_live: [bool; 2]) {
        let reach = MAX_NESTED_FORM_DEPTH.min(child.computed_through + 1);
        let mut folded = [None; MAX_NESTED_FORM_DEPTH + 1];
        for (depth, slot) in folded.iter_mut().enumerate().skip(1).take(reach) {
            *slot = match (self.slots[depth], child.slots[depth - 1]) {
                (Some(mut parent), Some(child_bits)) => {
                    for (lane, live) in lane_live.iter().enumerate() {
                        if child_bits[lane] && *live {
                            parent[lane] = true;
                        }
                    }
                    Some(parent)
                }
                _ => None,
            };
        }
        self.slots = folded;
        self.computed_through = self.computed_through.min(reach);
    }

    /// Fold one invoked nested-Form `Do` whose child lattice is unavailable:
    /// the target sits past the traversal horizon with nothing cached. Slot 0
    /// is computed (an invoked nested Form with no edge left is Unknown —
    /// its true value at that depth); every deeper slot needed the child and
    /// stays uncomputed for a shallower re-entry to fill in.
    pub(super) const fn refuse_nested_descent(&mut self) {
        self.slots = [None; MAX_NESTED_FORM_DEPTH + 1];
        self.computed_through = 0;
    }
}
