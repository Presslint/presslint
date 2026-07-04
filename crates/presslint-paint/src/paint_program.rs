//! Replayable paint-program stream over assembled operator records.
//!
//! [`PaintProgram`] is a cheap, `Copy` descriptor of everything the graphics-state
//! walker needs to run: the source bytes, the assembled operator records, and the
//! borrowed [`ColorSpaceEnv`]. It owns no walk state and materializes no `Vec`, so
//! the SAME program can be replayed: each `.ops()` (or `into_iter()`) constructs a
//! FRESH [`GraphicsStateWalker`] and re-walks `records` from index `0`.
//!
//! [`PaintOps`] is the iterator it hands out. It yields one
//! `Result<PaintOp, GraphicsWalkError>` per record, driving the walker's
//! `step` exactly as the earlier inline inventory loop did. On the first `Err` it
//! FUSES: it yields that `Err` once, then `None` forever, faithfully modelling the
//! current first-malformed-record short-circuit.
//!
//! This is a thin driver over the SAME `walker.step`; it allocates nothing per op
//! beyond what the walker already does (the per-event `state.clone()` hotspot is
//! unchanged). Replay re-runs the walk from scratch — callers replay only when they
//! need a fresh pass — so extra retained memory is O(1).

use presslint_syntax::OperatorRecord;

use crate::color_space_env::ColorSpaceEnv;
use crate::walker::{GraphicsStateWalker, GraphicsWalkError, PaintOp};

/// Cheap, replayable descriptor of a paint program.
///
/// Holds only borrowed `source`/`records` and the `Copy` [`ColorSpaceEnv`], so the
/// descriptor is itself `Copy` and carries no walk state. Iterating it (via
/// [`ops`](Self::ops) or `IntoIterator`) builds a fresh walker and re-runs from the
/// start every time, so the same program can be replayed as many times as needed
/// without materializing an event `Vec`.
#[derive(Debug, Clone, Copy)]
pub struct PaintProgram<'a> {
    source: &'a [u8],
    records: &'a [OperatorRecord],
    env: ColorSpaceEnv<'a>,
}

impl<'a> PaintProgram<'a> {
    /// Describe a paint program over `source`, its assembled `records`, and the
    /// borrowed page colour-space environment `env`.
    #[must_use]
    pub const fn new(
        source: &'a [u8],
        records: &'a [OperatorRecord],
        env: ColorSpaceEnv<'a>,
    ) -> Self {
        Self {
            source,
            records,
            env,
        }
    }

    /// Start a fresh walk of this program.
    ///
    /// Constructs a new [`GraphicsStateWalker`] from the program's
    /// [`ColorSpaceEnv`] and returns an iterator positioned at record `0`. Calling
    /// this again yields an independent iterator over the same input — this is what
    /// makes the program replayable.
    #[must_use]
    pub fn ops(&self) -> PaintOps<'a> {
        PaintOps {
            walker: GraphicsStateWalker::with_color_space_env(self.env),
            source: self.source,
            records: self.records,
            index: 0,
            done: false,
        }
    }

    /// Iterator convention alias for [`ops`](Self::ops).
    ///
    /// Mirrors [`ops`](Self::ops) under the standard `iter` name so
    /// `&PaintProgram`'s `IntoIterator` has a matching inherent method.
    #[must_use]
    pub fn iter(&self) -> PaintOps<'a> {
        self.ops()
    }
}

impl<'a> IntoIterator for PaintProgram<'a> {
    type Item = Result<PaintOp, GraphicsWalkError>;
    type IntoIter = PaintOps<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.ops()
    }
}

impl<'a> IntoIterator for &PaintProgram<'a> {
    type Item = Result<PaintOp, GraphicsWalkError>;
    type IntoIter = PaintOps<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.ops()
    }
}

/// Iterator over the paint ops of one [`PaintProgram`] walk.
///
/// Drives [`GraphicsStateWalker::step`] one record at a time. It FUSES on the first
/// `Err`: it yields that `Err` once, sets `done`, and thereafter returns `None`, so
/// the first malformed record short-circuits exactly as the materializing walk does.
#[derive(Debug, Clone)]
pub struct PaintOps<'a> {
    walker: GraphicsStateWalker<'a>,
    source: &'a [u8],
    records: &'a [OperatorRecord],
    index: usize,
    done: bool,
}

impl Iterator for PaintOps<'_> {
    type Item = Result<PaintOp, GraphicsWalkError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        let record = self.records.get(self.index)?;
        let result = self.walker.step(self.source, self.index, record);
        self.index += 1;
        if result.is_err() {
            self.done = true;
        }
        Some(result)
    }
}
