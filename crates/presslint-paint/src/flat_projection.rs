//! Exact depth-first flat projection over the call machine.
//!
//! [`flat_call_events`] composes [`crate::CallMachine::walk`] (it does not
//! re-implement traversal) and re-presents its depth-first visitor stream as ONE
//! flat, fused, path-annotated stream of every paint op of the root program AND of
//! every resolved sub-program, in exactly the umbrella's emission order:
//!
//! 1. the caller's ops in program order;
//! 2. when a form-classified `XObjectInvoke` resolves to
//!    [`crate::ResolveForm::Descend`], the callee's ops (recursively, depth-first)
//!    are delivered IMMEDIATELY AFTER that invocation op, before the caller's next
//!    op;
//! 3. a [`crate::ResolveForm::Skip`]ped (or resolver-refused) descent delivers NO
//!    callee ops and the caller continues;
//! 4. each `Ok` item carries its [`InvocationPath`] context (empty for root ops).
//!
//! The interleaving is a pure function of `(root, resolver decisions)` — there is
//! no hash-map iteration order anywhere — and nothing is materialized into a
//! `Vec`: items stream to the sink as they are visited, exactly one pass.
//!
//! # Sequence numbers are out of scope
//!
//! This projection reproduces POSITIONAL order only. It assigns NO sequence
//! numbers. Today's umbrella gives page entries their walk-local sequence and
//! seeds EXPANDED (form) entries a continuation at `page_inventory.len()` in splice
//! order, so positional order and sequence numbering DIVERGE by design in the flat
//! model. Sequence assignment is an inventory/adapter concern that stays in the
//! umbrella (pinned by its form-expanded goldens) until a later slice migrates it
//! onto this projection.
//!
//! # Errors and fusing
//!
//! A walk error in ANY program (root or callee), or a resolver error, is delivered
//! as the FINAL sink call wrapped in `Err`, after which nothing more is delivered —
//! the stream fuses exactly like [`crate::PaintOps`]. There is no resumption in the
//! caller after a callee error: this mirrors the umbrella's short-circuit on a
//! decode/walk failure inside a form expansion.

use presslint_types::InvocationPath;

use crate::{CallMachine, FormResolver, GraphicsWalkError, PaintOp, PaintSubProgram};

/// One item of the flat projection: a paint op with its invocation-path context.
///
/// Borrowed and `Copy`: `path` and `op` borrow the machine's live traversal state
/// and are valid only for the duration of the sink call. Nothing is cloned per op
/// — in particular the [`InvocationPath`] is handed out by reference, never
/// deep-copied, so the projection stays as allocation-light as the underlying
/// walker (the deep per-op path clone that would otherwise be a hotspot is avoided
/// exactly as the walker's per-event state clone was in Phase 0a-5).
///
/// The shape mirrors [`crate::CallEvent`]; the distinct name marks the
/// projection's stream item that later inventory/adapter slices consume.
#[derive(Debug, Clone, Copy)]
pub struct FlatPaintOp<'a> {
    /// Invocation path active while `op` was visited (empty for root ops).
    pub path: &'a InvocationPath,
    /// Paint op yielded by the underlying [`crate::PaintProgram`] walk.
    pub op: &'a PaintOp,
}

/// Drive the exact depth-first flat projection over the call machine.
///
/// `sink` is called once per stream item, in the umbrella's emission order (see
/// the [module docs](self)). Each `Ok` item is a [`FlatPaintOp`] borrowing the
/// active [`InvocationPath`] and the visited [`PaintOp`]; the trailing `Err`, if
/// any, is the walk/resolver error that fused the stream. The failing op itself is
/// not delivered as an `Ok` item — it surfaces only as the trailing `Err` (a
/// resolver error, by contrast, follows the `Ok` invocation op it refused).
pub fn flat_call_events<'a>(
    root: PaintSubProgram<'a>,
    resolver: &mut impl FormResolver<'a>,
    mut sink: impl FnMut(Result<FlatPaintOp<'_>, GraphicsWalkError>),
) {
    let result = CallMachine::walk(root, resolver, |event| {
        sink(Ok(FlatPaintOp {
            path: event.path,
            op: event.op,
        }));
    });
    if let Err(error) = result {
        sink(Err(error));
    }
}

#[cfg(test)]
mod tests {
    use presslint_types::{InvocationFrame, PdfName};

    use super::flat_call_events;
    use crate::tests::{assemble, form_program, name, page_program};
    use crate::{
        CallSite, ColorSpaceEnv, FormResolver, GraphicsWalkError, PaintOpKind, PaintProgram,
        PaintSubProgram, ResolveForm, walk_graphics_state,
    };

    /// Stable one-token tag per paint-op kind, for order-equivalence assertions
    /// that compare `(path, op-kind)` without constructing full colour/text
    /// payloads.
    fn kind_tag(kind: &PaintOpKind) -> &'static str {
        match kind {
            PaintOpKind::Save => "q",
            PaintOpKind::Restore => "Q",
            PaintOpKind::ConcatMatrix { .. } => "cm",
            PaintOpKind::SetStrokingColor { .. } => "SC",
            PaintOpKind::SetNonstrokingColor { .. } => "sc",
            PaintOpKind::PathPaint { .. } => "paint",
            PaintOpKind::SetTextRenderingMode { .. } => "Tr",
            PaintOpKind::SetFont { .. } => "Tf",
            PaintOpKind::TextShow { .. } => "show",
            PaintOpKind::XObjectInvoke { .. } => "Do",
            PaintOpKind::SetExtGState { .. } => "gs",
            PaintOpKind::NoOp => "noop",
        }
    }

    fn frame(ordinal: u32, value: &[u8]) -> InvocationFrame {
        InvocationFrame {
            ordinal,
            name: name(value),
        }
    }

    /// Resolver that descends into a caller-supplied program keyed by form name
    /// and skips any name it does not know. The linear scan keeps resolution
    /// deterministic (no hash-map iteration order).
    #[derive(Debug)]
    struct MapResolver<'a> {
        programs: Vec<(PdfName, PaintSubProgram<'a>)>,
    }

    impl<'a> FormResolver<'a> for MapResolver<'a> {
        fn resolve_form(
            &mut self,
            call: CallSite<'_>,
        ) -> Result<ResolveForm<'a>, GraphicsWalkError> {
            for (candidate, program) in &self.programs {
                if candidate == call.name {
                    return Ok(ResolveForm::Descend(program.clone()));
                }
            }
            Ok(ResolveForm::Skip)
        }
    }

    /// Drive the flat projection and collect each `Ok` item as `(frames, kind-tag)`
    /// plus the trailing error, if any. Cloning the frames is a test convenience;
    /// the projection itself hands out the path by reference.
    fn collect_flat<'a>(
        root: PaintSubProgram<'a>,
        resolver: &mut impl FormResolver<'a>,
    ) -> (
        Vec<(Vec<InvocationFrame>, &'static str)>,
        Option<GraphicsWalkError>,
    ) {
        let mut items = Vec::new();
        let mut error = None;
        flat_call_events(root, resolver, |item| match item {
            Ok(flat) => items.push((flat.path.frames.clone(), kind_tag(&flat.op.kind))),
            Err(walk_error) => error = Some(walk_error),
        });
        (items, error)
    }

    #[test]
    fn flat_projection_interleaves_calls_depth_first_in_emission_order() -> Result<(), String> {
        // Root has two form call sites (A then B); A itself descends into C. The
        // flat stream must be: caller op, invocation op, callee ops (recursively),
        // the caller's next op, ...
        let root_source = b"q /A Do /B Do Q";
        let a_source = b"/C Do 0.1 g f";
        let c_source = b"0.2 g f";
        let b_source = b"0.3 g f";
        let root_records = assemble(root_source)?;
        let a_records = assemble(a_source)?;
        let c_records = assemble(c_source)?;
        let b_records = assemble(b_source)?;
        let no_images = [];
        let root_forms = [name(b"A"), name(b"B")];
        let a_forms = [name(b"C")];
        let root = page_program(root_source, &root_records, &no_images, &root_forms);
        let a = form_program(a_source, &a_records, &no_images, &a_forms, name(b"A"));
        let c = form_program(c_source, &c_records, &no_images, &[], name(b"C"));
        let b = form_program(b_source, &b_records, &no_images, &[], name(b"B"));
        let mut resolver = MapResolver {
            programs: vec![(name(b"A"), a), (name(b"B"), b), (name(b"C"), c)],
        };

        let (items, error) = collect_flat(root, &mut resolver);
        assert!(error.is_none());
        assert_eq!(
            items,
            vec![
                (vec![], "q"),
                (vec![], "Do"),
                (vec![frame(0, b"A")], "Do"),
                (vec![frame(0, b"A"), frame(0, b"C")], "sc"),
                (vec![frame(0, b"A"), frame(0, b"C")], "paint"),
                (vec![frame(0, b"A")], "sc"),
                (vec![frame(0, b"A")], "paint"),
                (vec![], "Do"),
                (vec![frame(1, b"B")], "sc"),
                (vec![frame(1, b"B")], "paint"),
                (vec![], "Q"),
            ]
        );
        Ok(())
    }

    #[test]
    fn flat_projection_skip_yields_no_callee_ops_and_caller_continues() -> Result<(), String> {
        // `S` is form-classified but unknown to the resolver, so it skips: no
        // callee ops, and the caller's following `0.5 g f` still streams.
        let root_source = b"/S Do 0.5 g f";
        let root_records = assemble(root_source)?;
        let no_images = [];
        let root_forms = [name(b"S")];
        let root = page_program(root_source, &root_records, &no_images, &root_forms);
        let mut resolver = MapResolver {
            programs: Vec::new(),
        };

        let (items, error) = collect_flat(root, &mut resolver);
        assert!(error.is_none());
        assert_eq!(
            items,
            vec![(vec![], "Do"), (vec![], "sc"), (vec![], "paint")]
        );
        Ok(())
    }

    #[test]
    fn flat_projection_double_invocation_yields_callee_twice_with_distinct_paths()
    -> Result<(), String> {
        // The same form `F` is invoked twice; each descent yields F's ops under its
        // own caller-local ordinal path: [(0,F)] then [(1,F)].
        let root_source = b"/F Do /F Do";
        let form_source = b"0.1 g f";
        let root_records = assemble(root_source)?;
        let form_records = assemble(form_source)?;
        let no_images = [];
        let root_forms = [name(b"F")];
        let root = page_program(root_source, &root_records, &no_images, &root_forms);
        let callee = form_program(form_source, &form_records, &no_images, &[], name(b"F"));
        let mut resolver = MapResolver {
            programs: vec![(name(b"F"), callee)],
        };

        let (items, error) = collect_flat(root, &mut resolver);
        assert!(error.is_none());
        assert_eq!(
            items,
            vec![
                (vec![], "Do"),
                (vec![frame(0, b"F")], "sc"),
                (vec![frame(0, b"F")], "paint"),
                (vec![], "Do"),
                (vec![frame(1, b"F")], "sc"),
                (vec![frame(1, b"F")], "paint"),
            ]
        );
        Ok(())
    }

    #[test]
    fn flat_projection_callee_walk_error_fuses_the_stream() -> Result<(), String> {
        // `F` descends into a stream whose trailing `1 2 RG` is malformed (RG wants
        // three operands). The error must surface as the final item AND fuse: the
        // caller's post-descent `0.9 g f` must NOT stream (no resumption).
        let root_source = b"/F Do 0.9 g f";
        let form_source = b"0.4 g f 1 2 RG";
        let root_records = assemble(root_source)?;
        let form_records = assemble(form_source)?;
        let no_images = [];
        let root_forms = [name(b"F")];
        let root = page_program(root_source, &root_records, &no_images, &root_forms);
        let callee = form_program(form_source, &form_records, &no_images, &[], name(b"F"));
        let mut resolver = MapResolver {
            programs: vec![(name(b"F"), callee)],
        };

        let (items, error) = collect_flat(root, &mut resolver);
        assert_eq!(
            items,
            vec![
                (vec![], "Do"),
                (vec![frame(0, b"F")], "sc"),
                (vec![frame(0, b"F")], "paint"),
            ]
        );
        // The surfaced error is exactly the callee walk's short-circuit error.
        let expected = walk_graphics_state(form_source, &form_records)
            .err()
            .ok_or("callee walk should fail on the malformed record")?;
        assert_eq!(error, Some(expected));
        Ok(())
    }

    #[test]
    fn flat_projection_no_form_root_equals_plain_paint_program() -> Result<(), String> {
        // With no form call sites the projection degenerates to the plain
        // `PaintProgram` op stream, every item at the empty root path.
        let root_source = b"q 0.4 g f Q /Im1 Do";
        let root_records = assemble(root_source)?;
        let images = [name(b"Im1")];
        let no_forms = [];
        let root = page_program(root_source, &root_records, &images, &no_forms);
        let mut resolver = MapResolver {
            programs: Vec::new(),
        };

        let mut flat_ops = Vec::new();
        let mut error = None;
        flat_call_events(root, &mut resolver, |item| match item {
            Ok(flat) => {
                assert!(flat.path.frames.is_empty(), "root ops carry the empty path");
                flat_ops.push(flat.op.clone());
            }
            Err(walk_error) => error = Some(walk_error),
        });
        assert!(error.is_none());

        let expected = PaintProgram::new(root_source, &root_records, ColorSpaceEnv::empty())
            .ops()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|walk_error| format!("{walk_error:?}"))?;
        assert_eq!(flat_ops.len(), expected.len());
        for (got, want) in flat_ops.iter().zip(&expected) {
            assert_eq!(got, want);
        }
        Ok(())
    }
}
