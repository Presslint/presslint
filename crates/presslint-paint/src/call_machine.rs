//! Pure call/return traversal substrate for paint programs.
//!
//! This module provides the structural mechanics that later inventory slices can
//! use to migrate form expansion onto `presslint-paint`: a caller supplies
//! already-prepared sub-program descriptors and a resolver that decides whether
//! each form call descends or skips. The machine owns no PDF object lookup,
//! resource loading, depth limit, budget, or cycle policy; those decisions remain
//! entirely with the resolver.
//!
//! Traversal is depth-first over the existing replayable [`PaintProgram`]. Every
//! visited paint op is exposed to a visitor as a [`CallEvent`] carrying the
//! current [`InvocationPath`]. When a `Do` event is classified as a form
//! invocation (name in `form_xobject_names` and not in `image_xobject_names`),
//! the machine assigns the next zero-based ordinal for the calling program,
//! asks the resolver, and pushes an [`InvocationFrame`] only for `Descend`.
//! `Skip` is a normal resolver decision; an error aborts like any walker error.
//!
//! The machine also owns the normative Form graphics-state inheritance
//! (ISO 32000-1 §8.10.1): every descended callee walk is seeded from the exact
//! shared snapshot carried by the caller's `Do` event (`Rc::clone`, no deep
//! copy), with an empty callee-local `q`/`Q` stack. Caller and callee remain
//! isolated by the walker's copy-on-write snapshots. Form `/Matrix`
//! concatenation, `/BBox` clipping, and transparency-group entry resets are
//! deliberately NOT modelled here.

use std::rc::Rc;

use presslint_syntax::OperatorRecord;
use presslint_types::{ContentScope, InvocationFrame, InvocationPath, PdfName};

use crate::{
    ColorSpaceEnv, ExtGStateEnv, FontEnv, GraphicsStateSnapshot, GraphicsWalkError, PaintOp,
    PaintOpKind, PaintOps, PaintProgram,
};

/// Borrowed descriptor for one paint sub-program.
///
/// The caller owns the byte buffers, assembled records, resource-name lists, and
/// colour-space environment. The machine only borrows them and replays the
/// existing [`PaintProgram`] iterator.
#[derive(Debug, Clone)]
pub struct PaintSubProgram<'a> {
    /// Content-stream bytes for this sub-program.
    pub source: &'a [u8],
    /// Assembled operator records for `source`.
    pub records: &'a [OperatorRecord],
    /// Colour-space environment used by the paint walker.
    pub color_space_env: ColorSpaceEnv<'a>,
    /// `ExtGState` environment used by the paint walker for `gs`.
    pub extgstate_env: ExtGStateEnv<'a>,
    /// Font environment used by the paint walker for `Tf` and `gs`.
    pub font_env: FontEnv<'a>,
    /// Names classified by the caller as image `XObject`s.
    pub image_xobject_names: &'a [PdfName],
    /// Names classified by the caller as form `XObject`s.
    pub form_xobject_names: &'a [PdfName],
    /// Public content scope for this sub-program.
    pub scope: ContentScope,
}

/// Form call-site presented to a resolver.
#[derive(Debug, Clone, Copy)]
pub struct CallSite<'a> {
    /// Invocation path of the calling program.
    pub caller_path: &'a InvocationPath,
    /// Zero-based form invocation ordinal within the calling program.
    pub ordinal: u32,
    /// Invoked form resource name.
    pub name: &'a PdfName,
    /// Paint op that performed the `Do` invocation.
    pub event: &'a PaintOp,
}

/// Resolver decision for a form call-site.
#[derive(Debug, Clone)]
pub enum ResolveForm<'a> {
    /// Descend into a caller-supplied sub-program.
    Descend(PaintSubProgram<'a>),
    /// Continue in the caller without visiting callee ops.
    Skip,
}

/// Caller-owned policy hook for form invocation handling.
///
/// The resolver may enforce depth, cycle, budget, resource, or ownership policy
/// by returning [`ResolveForm::Skip`] or a [`GraphicsWalkError`]. The machine
/// itself only handles traversal mechanics and path construction.
pub trait FormResolver<'a> {
    /// Resolve one form-classified call-site.
    ///
    /// # Errors
    ///
    /// Returns a [`GraphicsWalkError`] when caller-owned policy refuses the
    /// traversal as a hard failure rather than a normal [`ResolveForm::Skip`].
    fn resolve_form(&mut self, call: CallSite<'_>) -> Result<ResolveForm<'a>, GraphicsWalkError>;

    /// Observe return from a descended form before its frame is popped.
    ///
    /// `path` is the full child invocation path that is about to be left. The
    /// hook is called once for every successful [`ResolveForm::Descend`],
    /// including empty callees. If a callee walk or resolver call yields an
    /// error, the machine unwinds every still-open descended frame in LIFO order
    /// before propagating the error, so resolver-owned active-path policy can
    /// clean up exactly once per completed or aborted descent.
    fn on_return(&mut self, _path: &InvocationPath) {}
}

/// Visitor event carrying the current invocation path for one paint op.
#[derive(Debug, Clone, Copy)]
pub struct CallEvent<'a> {
    /// Path active while `op` was visited.
    pub path: &'a InvocationPath,
    /// Paint op yielded by the underlying [`PaintProgram`].
    pub op: &'a PaintOp,
}

/// Depth-first call/return traversal machine.
#[derive(Debug, Default, Clone, Copy)]
pub struct CallMachine;

impl CallMachine {
    /// Walk `root` depth-first, resolving form calls through `resolver`.
    ///
    /// The visitor is called for every paint op before any descent triggered by
    /// that op. A form invocation's ordinal is local to its calling program and
    /// counts only form-classified `Do` events; image names take precedence when
    /// a name appears in both resource lists.
    ///
    /// # Errors
    ///
    /// Returns the first walker error from any visited program, or the first
    /// resolver error.
    pub fn walk<'a>(
        root: PaintSubProgram<'a>,
        resolver: &mut impl FormResolver<'a>,
        mut visitor: impl FnMut(CallEvent<'_>),
    ) -> Result<(), GraphicsWalkError> {
        let mut path = InvocationPath { frames: Vec::new() };
        let mut stack = vec![WalkFrame::root(root)];

        while let Some(frame) = stack.last_mut() {
            let Some(next) = frame.ops.next() else {
                if stack
                    .pop()
                    .is_some_and(|finished| finished.pop_path_on_return)
                {
                    resolver.on_return(&path);
                    path.frames.pop();
                }
                continue;
            };

            let event = match next {
                Ok(event) => event,
                Err(error) => {
                    unwind_open_frames(&mut stack, &mut path, resolver);
                    return Err(error);
                }
            };
            visitor(CallEvent {
                path: &path,
                op: &event,
            });

            let Some(name) = form_invocation_name(
                &event,
                frame.program.image_xobject_names,
                frame.program.form_xobject_names,
            ) else {
                continue;
            };

            let ordinal = frame.next_form_ordinal;
            frame.next_form_ordinal = frame.next_form_ordinal.saturating_add(1);
            let decision = match resolver.resolve_form(CallSite {
                caller_path: &path,
                ordinal,
                name,
                event: &event,
            }) {
                Ok(decision) => decision,
                Err(error) => {
                    unwind_open_frames(&mut stack, &mut path, resolver);
                    return Err(error);
                }
            };

            if let ResolveForm::Descend(callee) = decision {
                path.frames.push(InvocationFrame {
                    ordinal,
                    name: name.clone(),
                });
                // Machine-owned Form-inheritance invariant (ISO 32000-1
                // §8.10.1): the callee starts from the EXACT caller state at
                // this `Do` event — a refcount bump, never a deep copy. The
                // resolver supplies only the sub-program; it cannot choose or
                // override the descent seed.
                stack.push(WalkFrame::callee(callee, Rc::clone(&event.state)));
            }
        }

        Ok(())
    }
}

fn unwind_open_frames<'a>(
    stack: &mut Vec<WalkFrame<'a>>,
    path: &mut InvocationPath,
    resolver: &mut impl FormResolver<'a>,
) {
    while let Some(frame) = stack.pop() {
        if frame.pop_path_on_return {
            resolver.on_return(path);
            path.frames.pop();
        }
    }
}

#[derive(Debug)]
struct WalkFrame<'a> {
    program: PaintSubProgram<'a>,
    ops: PaintOps<'a>,
    next_form_ordinal: u32,
    pop_path_on_return: bool,
}

impl<'a> WalkFrame<'a> {
    /// The root program starts from the page-default state, exactly as before.
    fn root(program: PaintSubProgram<'a>) -> Self {
        let seed = Rc::new(GraphicsStateSnapshot::page_default());
        Self::new(program, seed, false)
    }

    /// A descended callee starts from the caller's exact `Do`-event state with
    /// an empty local `q`/`Q` stack; its mutations copy-on-write away from the
    /// shared seed, so they can never leak back into the caller's walker.
    const fn callee(program: PaintSubProgram<'a>, seed: Rc<GraphicsStateSnapshot>) -> Self {
        Self::new(program, seed, true)
    }

    const fn new(
        program: PaintSubProgram<'a>,
        seed: Rc<GraphicsStateSnapshot>,
        pop_path_on_return: bool,
    ) -> Self {
        let ops = PaintProgram::with_all_envs(
            program.source,
            program.records,
            program.color_space_env,
            program.extgstate_env,
            program.font_env,
        )
        .ops_with_initial_state(seed);
        Self {
            program,
            ops,
            next_form_ordinal: 0,
            pop_path_on_return,
        }
    }
}

fn form_invocation_name<'a>(
    event: &'a PaintOp,
    image_xobject_names: &[PdfName],
    form_xobject_names: &[PdfName],
) -> Option<&'a PdfName> {
    let PaintOpKind::XObjectInvoke { name } = &event.kind else {
        return None;
    };
    (!image_xobject_names.contains(name) && form_xobject_names.contains(name)).then_some(name)
}
