//! Call/return machine + invocation-identity tests (Phase 0b-2..0b-4a), plus
//! the machine-owned caller-state inheritance invariant.
//!
//! These exercise `CallMachine::walk`: caller-local form invocation ordinals,
//! depth-first descent with LIFO frame popping, the `on_return` hook, resolver
//! skip/error surfacing, image-name precedence, and the plain-JSON round trip of
//! the `InvocationPath` that identifies each expanded op. The inheritance tests
//! prove every descended callee starts from the EXACT caller `Do`-event state
//! (`Rc::ptr_eq` for a non-mutating first callee op), that nesting inherits
//! from the IMMEDIATE caller, that callee mutations never leak back to the
//! caller or a sibling invocation, that `q`/`Do`/`Q` restores the pre-`q`
//! state, and that the callee's local `q`/`Q` stack starts EMPTY. Form
//! `/Matrix` concatenation, `/BBox` clipping, and transparency-group entry
//! resets are NOT modelled or tested here.

use std::rc::Rc;

use presslint_types::{ColorSpace, InvocationFrame, InvocationPath, PdfName};

use super::{assemble, form_program, mini_json, name, page_program};
use crate::{
    CallEvent, CallMachine, CallSite, FontBinding, FontBindingTarget, FontEnv, FontSelectionState,
    FormResolver, GraphicsStateSnapshot, GraphicsWalkError, PaintOp, PaintOpKind, PaintSubProgram,
    ResolveForm, ResolvedFont, TextRenderingMode,
};

fn resolved_font(object_number: u32, offset: usize, size: f64) -> FontSelectionState {
    FontSelectionState::ResolvedIndirect {
        object_number,
        generation: 0,
        object_byte_offset: offset,
        size,
    }
}

fn collect_xobject_paths(event: CallEvent<'_>, paths: &mut Vec<(InvocationPath, PdfName)>) {
    if let PaintOpKind::XObjectInvoke { name } = &event.op.kind {
        paths.push((event.path.clone(), name.clone()));
    }
}

#[derive(Debug)]
struct StaticResolver<'a> {
    callee: PaintSubProgram<'a>,
    calls: Vec<(InvocationPath, u32, PdfName)>,
}

impl<'a> FormResolver<'a> for StaticResolver<'a> {
    fn resolve_form(&mut self, call: CallSite<'_>) -> Result<ResolveForm<'a>, GraphicsWalkError> {
        self.calls
            .push((call.caller_path.clone(), call.ordinal, call.name.clone()));
        Ok(ResolveForm::Descend(self.callee.clone()))
    }
}

#[test]
fn call_machine_repeated_same_form_gets_local_ordinals_and_descends_twice() -> Result<(), String> {
    let root_source = b"/F Do /F Do";
    let form_source = b"0.1 g f";
    let root_records = assemble(root_source)?;
    let form_records = assemble(form_source)?;
    let form_name = name(b"F");
    let forms = [form_name.clone()];
    let no_images = [];
    let root = page_program(root_source, &root_records, &no_images, &forms);
    let callee = form_program(
        form_source,
        &form_records,
        &no_images,
        &[],
        form_name.clone(),
    );
    let mut resolver = StaticResolver {
        callee,
        calls: Vec::new(),
    };
    let mut paths = Vec::new();

    CallMachine::walk(root, &mut resolver, |event| {
        if matches!(event.op.kind, PaintOpKind::PathPaint { .. }) {
            paths.push(event.path.clone());
        }
    })
    .map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        resolver.calls,
        vec![
            (InvocationPath { frames: Vec::new() }, 0, form_name.clone()),
            (InvocationPath { frames: Vec::new() }, 1, form_name.clone()),
        ]
    );
    assert_eq!(
        paths,
        vec![
            InvocationPath {
                frames: vec![InvocationFrame {
                    ordinal: 0,
                    name: form_name.clone(),
                }],
            },
            InvocationPath {
                frames: vec![InvocationFrame {
                    ordinal: 1,
                    name: form_name,
                }],
            },
        ]
    );
    Ok(())
}

#[test]
fn caller_font_inherits_exactly_and_form_tf_uses_only_local_environment() -> Result<(), String> {
    let root_source = b"/F1 8 Tf /A Do (P) Tj /F2 9 Tf /A Do";
    let form_source = b"n /Local 7 Tf n";
    let root_records = assemble(root_source)?;
    let form_records = assemble(form_source)?;
    let no_images = [];
    let forms = [name(b"A")];
    let root_bindings = [
        FontBinding::from_pdf_name_bytes(
            b"F1",
            FontBindingTarget::Resolved(ResolvedFont {
                object_number: 11,
                generation: 0,
                object_byte_offset: 101,
            }),
        )
        .ok_or("F1 binding")?,
        FontBinding::from_pdf_name_bytes(
            b"F2",
            FontBindingTarget::Resolved(ResolvedFont {
                object_number: 12,
                generation: 0,
                object_byte_offset: 202,
            }),
        )
        .ok_or("F2 binding")?,
    ];
    let local_bindings = [FontBinding::from_pdf_name_bytes(
        b"Local",
        FontBindingTarget::Resolved(ResolvedFont {
            object_number: 30,
            generation: 0,
            object_byte_offset: 303,
        }),
    )
    .ok_or("local binding")?];

    let mut root = page_program(root_source, &root_records, &no_images, &forms);
    root.font_env = FontEnv::known(&root_bindings);
    let mut callee = form_program(form_source, &form_records, &no_images, &[], name(b"A"));
    callee.font_env = FontEnv::known(&local_bindings);
    let mut resolver = StaticResolver {
        callee,
        calls: Vec::new(),
    };
    let mut form_states = Vec::new();
    let mut page_text_state = None;

    CallMachine::walk(root, &mut resolver, |event| {
        if event.path.frames.is_empty() {
            if matches!(event.op.kind, PaintOpKind::TextShow { .. }) {
                page_text_state = Some(event.op.state.font_selection.clone());
            }
        } else {
            form_states.push(event.op.state.font_selection.clone());
        }
    })
    .map_err(|error| format!("{error:?}"))?;

    assert_eq!(page_text_state, Some(resolved_font(11, 101, 8.0)));
    assert_eq!(
        form_states,
        vec![
            resolved_font(11, 101, 8.0),
            resolved_font(30, 303, 7.0),
            resolved_font(30, 303, 7.0),
            resolved_font(12, 202, 9.0),
            resolved_font(30, 303, 7.0),
            resolved_font(30, 303, 7.0),
        ]
    );
    Ok(())
}

#[derive(Debug)]
struct NestedResolver<'a> {
    a: PaintSubProgram<'a>,
    b: PaintSubProgram<'a>,
}

impl<'a> FormResolver<'a> for NestedResolver<'a> {
    fn resolve_form(&mut self, call: CallSite<'_>) -> Result<ResolveForm<'a>, GraphicsWalkError> {
        if call.name == &name(b"A") {
            Ok(ResolveForm::Descend(self.a.clone()))
        } else {
            Ok(ResolveForm::Descend(self.b.clone()))
        }
    }
}

#[test]
fn call_machine_nested_descent_is_depth_first_and_pops_on_return() -> Result<(), String> {
    let root_source = b"/A Do n";
    let a_source = b"/B Do n";
    let b_source = b"0.2 g f";
    let root_records = assemble(root_source)?;
    let a_records = assemble(a_source)?;
    let b_records = assemble(b_source)?;
    let no_images = [];
    let root_forms = [name(b"A")];
    let a_forms = [name(b"B")];
    let root = page_program(root_source, &root_records, &no_images, &root_forms);
    let a = form_program(a_source, &a_records, &no_images, &a_forms, name(b"A"));
    let b = form_program(b_source, &b_records, &no_images, &[], name(b"B"));
    let mut resolver = NestedResolver { a, b };
    let mut seen = Vec::new();

    CallMachine::walk(root, &mut resolver, |event| {
        seen.push((event.path.clone(), event.op.kind.clone()));
    })
    .map_err(|error| format!("{error:?}"))?;

    let a_frame = InvocationFrame {
        ordinal: 0,
        name: name(b"A"),
    };
    let b_frame = InvocationFrame {
        ordinal: 0,
        name: name(b"B"),
    };
    assert!(matches!(seen[0].1, PaintOpKind::XObjectInvoke { .. }));
    assert_eq!(seen[0].0.frames, Vec::new());
    assert!(matches!(seen[1].1, PaintOpKind::XObjectInvoke { .. }));
    assert_eq!(seen[1].0.frames, vec![a_frame.clone()]);
    assert_eq!(seen[2].0.frames, vec![a_frame.clone(), b_frame]);
    assert_eq!(seen[3].0.frames, seen[2].0.frames);
    assert_eq!(seen[4].0.frames, vec![a_frame]);
    assert_eq!(seen[5].0.frames, Vec::new());
    Ok(())
}

#[derive(Debug)]
struct ReturnTrackingResolver<'a> {
    a: PaintSubProgram<'a>,
    b: PaintSubProgram<'a>,
    returns: Vec<InvocationPath>,
}

impl<'a> FormResolver<'a> for ReturnTrackingResolver<'a> {
    fn resolve_form(&mut self, call: CallSite<'_>) -> Result<ResolveForm<'a>, GraphicsWalkError> {
        if call.name == &name(b"A") {
            Ok(ResolveForm::Descend(self.a.clone()))
        } else {
            Ok(ResolveForm::Descend(self.b.clone()))
        }
    }

    fn on_return(&mut self, path: &InvocationPath) {
        self.returns.push(path.clone());
    }
}

#[test]
fn call_machine_return_hook_fires_lifo_and_for_empty_callee() -> Result<(), String> {
    let root_source = b"/A Do /B Do";
    let a_source = b"/B Do";
    let b_source = b"";
    let root_records = assemble(root_source)?;
    let a_records = assemble(a_source)?;
    let b_records = assemble(b_source)?;
    let no_images = [];
    let root_forms = [name(b"A"), name(b"B")];
    let a_forms = [name(b"B")];
    let root = page_program(root_source, &root_records, &no_images, &root_forms);
    let a = form_program(a_source, &a_records, &no_images, &a_forms, name(b"A"));
    let b = form_program(b_source, &b_records, &no_images, &[], name(b"B"));
    let mut resolver = ReturnTrackingResolver {
        a,
        b,
        returns: Vec::new(),
    };

    CallMachine::walk(root, &mut resolver, |_| {}).map_err(|error| format!("{error:?}"))?;

    assert_eq!(
        resolver.returns,
        vec![
            InvocationPath {
                frames: vec![
                    InvocationFrame {
                        ordinal: 0,
                        name: name(b"A"),
                    },
                    InvocationFrame {
                        ordinal: 0,
                        name: name(b"B"),
                    },
                ],
            },
            InvocationPath {
                frames: vec![InvocationFrame {
                    ordinal: 0,
                    name: name(b"A"),
                }],
            },
            InvocationPath {
                frames: vec![InvocationFrame {
                    ordinal: 1,
                    name: name(b"B"),
                }],
            },
        ]
    );
    Ok(())
}

#[test]
fn call_machine_return_hook_fires_before_callee_error_unwinds() -> Result<(), String> {
    let root_source = b"/B Do";
    let b_source = b"0.4 g f 1 2 RG";
    let root_records = assemble(root_source)?;
    let b_records = assemble(b_source)?;
    let no_images = [];
    let root_forms = [name(b"B")];
    let root = page_program(root_source, &root_records, &no_images, &root_forms);
    let b = form_program(b_source, &b_records, &no_images, &[], name(b"B"));
    let mut resolver = ReturnTrackingResolver {
        a: b.clone(),
        b,
        returns: Vec::new(),
    };

    let error = CallMachine::walk(root, &mut resolver, |_| {})
        .err()
        .ok_or("callee error should abort the walk")?;

    assert!(matches!(
        error.kind,
        crate::GraphicsWalkErrorKind::MalformedOperandCount { .. }
    ));
    assert_eq!(
        resolver.returns,
        vec![InvocationPath {
            frames: vec![InvocationFrame {
                ordinal: 0,
                name: name(b"B"),
            }],
        }]
    );
    Ok(())
}

#[derive(Debug)]
struct SkipResolver {
    calls: usize,
}

impl<'a> FormResolver<'a> for SkipResolver {
    fn resolve_form(&mut self, _call: CallSite<'_>) -> Result<ResolveForm<'a>, GraphicsWalkError> {
        self.calls += 1;
        Ok(ResolveForm::Skip)
    }
}

#[test]
fn call_machine_resolver_skip_yields_no_callee_ops() -> Result<(), String> {
    let root_source = b"/F Do";
    let root_records = assemble(root_source)?;
    let forms = [name(b"F")];
    let no_images = [];
    let root = page_program(root_source, &root_records, &no_images, &forms);
    let mut resolver = SkipResolver { calls: 0 };
    let mut paths = Vec::new();

    CallMachine::walk(root, &mut resolver, |event| {
        paths.push(event.path.clone());
    })
    .map_err(|error| format!("{error:?}"))?;

    assert_eq!(resolver.calls, 1);
    assert_eq!(paths, vec![InvocationPath { frames: Vec::new() }]);
    Ok(())
}

#[test]
fn call_machine_image_name_wins_over_form_name_conflict() -> Result<(), String> {
    let root_source = b"/X Do";
    let root_records = assemble(root_source)?;
    let x_name = name(b"X");
    let images = [x_name.clone()];
    let forms = [x_name];
    let root = page_program(root_source, &root_records, &images, &forms);
    let mut resolver = SkipResolver { calls: 0 };
    let mut invoked = Vec::new();

    CallMachine::walk(root, &mut resolver, |event| {
        collect_xobject_paths(event, &mut invoked);
    })
    .map_err(|error| format!("{error:?}"))?;

    assert_eq!(resolver.calls, 0);
    assert_eq!(invoked.len(), 1);
    assert_eq!(invoked[0].0.frames, Vec::new());
    Ok(())
}

#[derive(Debug, Default)]
struct ErrorResolver;

impl<'a> FormResolver<'a> for ErrorResolver {
    fn resolve_form(&mut self, call: CallSite<'_>) -> Result<ResolveForm<'a>, GraphicsWalkError> {
        Err(GraphicsWalkError::new(
            crate::GraphicsWalkErrorKind::InvalidSourceRange,
            call.event.record_range,
        ))
    }
}

#[test]
fn call_machine_resolver_error_surfaces_as_walk_error() -> Result<(), String> {
    let root_source = b"/F Do";
    let root_records = assemble(root_source)?;
    let forms = [name(b"F")];
    let no_images = [];
    let root = page_program(root_source, &root_records, &no_images, &forms);
    let mut resolver = ErrorResolver;

    let error = CallMachine::walk(root, &mut resolver, |_| {})
        .err()
        .ok_or("resolver error should abort the walk")?;
    assert_eq!(error.kind, crate::GraphicsWalkErrorKind::InvalidSourceRange);
    Ok(())
}

/// Collect every visited `(path, op)` pair through a `StaticResolver` descent.
fn collect_events<'a>(
    root: PaintSubProgram<'a>,
    resolver: &mut impl FormResolver<'a>,
) -> Result<Vec<(InvocationPath, PaintOp)>, String> {
    let mut seen = Vec::new();
    CallMachine::walk(root, resolver, |event| {
        seen.push((event.path.clone(), event.op.clone()));
    })
    .map_err(|error| format!("{error:?}"))?;
    Ok(seen)
}

fn nonstroking(state: &GraphicsStateSnapshot) -> (ColorSpace, Vec<f64>) {
    (
        state.nonstroking_color.space.clone(),
        state.nonstroking_color.components.clone(),
    )
}

#[test]
fn descended_callee_starts_from_exact_caller_do_state() -> Result<(), String> {
    // The caller establishes non-default state before `Do`; the callee's first
    // op `n` does not mutate, so it must carry the caller `Do` event's snapshot
    // by POINTER — proving the machine seeded the descended walk itself.
    let root_source = b"0 0 1 RG 1 0 0 rg /F Do f";
    let form_source = b"n 0 1 0 rg f";
    let root_records = assemble(root_source)?;
    let form_records = assemble(form_source)?;
    let no_images = [];
    let forms = [name(b"F")];
    let root = page_program(root_source, &root_records, &no_images, &forms);
    let callee = form_program(form_source, &form_records, &no_images, &[], name(b"F"));
    let mut resolver = StaticResolver {
        callee,
        calls: Vec::new(),
    };

    let seen = collect_events(root, &mut resolver)?;

    let do_state = seen
        .iter()
        .find(|(path, op)| {
            path.frames.is_empty() && matches!(op.kind, PaintOpKind::XObjectInvoke { .. })
        })
        .map(|(_, op)| Rc::clone(&op.state))
        .ok_or("caller Do event")?;
    let callee_first = seen
        .iter()
        .find(|(path, _)| path.frames.len() == 1)
        .map(|(_, op)| op)
        .ok_or("first callee op")?;

    // Exact inheritance: pointer-equal until the first callee mutation.
    assert!(Rc::ptr_eq(&callee_first.state, &do_state));
    assert_eq!(
        nonstroking(&callee_first.state),
        (ColorSpace::DeviceRgb, vec![1.0, 0.0, 0.0])
    );
    assert_eq!(
        callee_first.state.stroking_color.components,
        vec![0.0, 0.0, 1.0]
    );

    // Callee mutation isolation: the caller's `f` after the return still sees
    // the caller's own red, not the callee's green.
    let caller_paint = seen
        .iter()
        .filter(|(path, op)| {
            path.frames.is_empty() && matches!(op.kind, PaintOpKind::PathPaint { .. })
        })
        .map(|(_, op)| op)
        .next_back()
        .ok_or("caller paint after return")?;
    assert_eq!(
        nonstroking(&caller_paint.state),
        (ColorSpace::DeviceRgb, vec![1.0, 0.0, 0.0])
    );
    Ok(())
}

#[test]
// Exact CTM transport is part of the inheritance contract: strict compare.
#[allow(clippy::float_cmp)]
fn nested_callee_inherits_from_immediate_caller_not_page() -> Result<(), String> {
    // Page sets 0.3 g, A sets 0.6 g and a CTM, then invokes B: B must inherit
    // A's exact `Do` state (0.6 grey + A's CTM), not the page state or default.
    let root_source = b"0.3 g /A Do";
    let a_source = b"0.6 g 2 0 0 2 0 0 cm /B Do";
    let b_source = b"f";
    let root_records = assemble(root_source)?;
    let a_records = assemble(a_source)?;
    let b_records = assemble(b_source)?;
    let no_images = [];
    let root_forms = [name(b"A")];
    let a_forms = [name(b"B")];
    let root = page_program(root_source, &root_records, &no_images, &root_forms);
    let a = form_program(a_source, &a_records, &no_images, &a_forms, name(b"A"));
    let b = form_program(b_source, &b_records, &no_images, &[], name(b"B"));
    let mut resolver = NestedResolver { a, b };
    let mut b_states = Vec::new();

    CallMachine::walk(root, &mut resolver, |event| {
        if event.path.frames.len() == 2 {
            b_states.push(Rc::clone(&event.op.state));
        }
    })
    .map_err(|error| format!("{error:?}"))?;

    let b_first = b_states.first().ok_or("B should be visited")?;
    assert_eq!(nonstroking(b_first), (ColorSpace::DeviceGray, vec![0.6]));
    assert_eq!(b_first.ctm, [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    Ok(())
}

#[test]
// Exact CTM transport is part of the inheritance contract: strict compare.
#[allow(clippy::float_cmp)]
fn callee_mutations_do_not_leak_to_caller_or_sibling_invocation() -> Result<(), String> {
    // `F` mutates colour, CTM, text rendering mode, and font selection. The
    // caller op between/after the two invocations and the SIBLING invocation's
    // inherited state must all keep the caller's own state.
    let root_source = b"1 0 0 rg /F Do f /F Do f";
    let form_source = b"n 0 1 0 rg 3 3 3 3 3 3 cm 2 Tr /F1 8 Tf f";
    let root_records = assemble(root_source)?;
    let form_records = assemble(form_source)?;
    let no_images = [];
    let forms = [name(b"F")];
    let root = page_program(root_source, &root_records, &no_images, &forms);
    let callee = form_program(form_source, &form_records, &no_images, &[], name(b"F"));
    let mut resolver = StaticResolver {
        callee,
        calls: Vec::new(),
    };

    let seen = collect_events(root, &mut resolver)?;

    // Both caller paints keep the caller's red, identity CTM, Fill mode, and
    // Unset font — none of the callee's mutations leaked back.
    let caller_paints: Vec<&PaintOp> = seen
        .iter()
        .filter(|(path, op)| {
            path.frames.is_empty() && matches!(op.kind, PaintOpKind::PathPaint { .. })
        })
        .map(|(_, op)| op)
        .collect();
    assert_eq!(caller_paints.len(), 2);
    for paint in &caller_paints {
        assert_eq!(
            nonstroking(&paint.state),
            (ColorSpace::DeviceRgb, vec![1.0, 0.0, 0.0])
        );
        assert_eq!(paint.state.ctm, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        assert_eq!(paint.state.text_rendering_mode, TextRenderingMode::Fill);
        assert_eq!(paint.state.font_selection, FontSelectionState::Unset);
    }

    // The sibling (second) invocation inherits from ITS OWN `Do` state — the
    // caller's red again, untouched by the first callee's green/CTM/text edits.
    let sibling_first = seen
        .iter()
        .find(|(path, _)| path.frames.first().is_some_and(|frame| frame.ordinal == 1))
        .map(|(_, op)| op)
        .ok_or("second invocation first op")?;
    assert_eq!(
        nonstroking(&sibling_first.state),
        (ColorSpace::DeviceRgb, vec![1.0, 0.0, 0.0])
    );
    assert_eq!(sibling_first.state.ctm, [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    assert_eq!(
        sibling_first.state.font_selection,
        FontSelectionState::Unset
    );
    Ok(())
}

#[test]
fn q_scoped_state_seeds_the_form_and_restores_after_return() -> Result<(), String> {
    // q / state change / Do / Q: the form starts from the INNER (0.8) state;
    // the caller paint after `Q` sees the pre-`q` (0.2) state.
    let root_source = b"0.2 g q 0.8 g /F Do Q f";
    let form_source = b"n";
    let root_records = assemble(root_source)?;
    let form_records = assemble(form_source)?;
    let no_images = [];
    let forms = [name(b"F")];
    let root = page_program(root_source, &root_records, &no_images, &forms);
    let callee = form_program(form_source, &form_records, &no_images, &[], name(b"F"));
    let mut resolver = StaticResolver {
        callee,
        calls: Vec::new(),
    };

    let seen = collect_events(root, &mut resolver)?;

    let callee_op = seen
        .iter()
        .find(|(path, _)| path.frames.len() == 1)
        .map(|(_, op)| op)
        .ok_or("callee op")?;
    assert_eq!(
        nonstroking(&callee_op.state),
        (ColorSpace::DeviceGray, vec![0.8])
    );

    let caller_paint = seen
        .iter()
        .filter(|(path, op)| {
            path.frames.is_empty() && matches!(op.kind, PaintOpKind::PathPaint { .. })
        })
        .map(|(_, op)| op)
        .next_back()
        .ok_or("caller paint after Q")?;
    assert_eq!(
        nonstroking(&caller_paint.state),
        (ColorSpace::DeviceGray, vec![0.2])
    );
    Ok(())
}

#[test]
fn callee_local_stack_starts_empty_so_unmatched_q_underflows() -> Result<(), String> {
    // The caller holds an open `q` save, but the callee's local stack starts
    // EMPTY: its unmatched `Q` must underflow instead of popping the caller's
    // save across the form boundary.
    let root_source = b"q /F Do Q";
    let form_source = b"Q";
    let root_records = assemble(root_source)?;
    let form_records = assemble(form_source)?;
    let no_images = [];
    let forms = [name(b"F")];
    let root = page_program(root_source, &root_records, &no_images, &forms);
    let callee = form_program(form_source, &form_records, &no_images, &[], name(b"F"));
    let mut resolver = StaticResolver {
        callee,
        calls: Vec::new(),
    };

    let error = CallMachine::walk(root, &mut resolver, |_| {})
        .err()
        .ok_or("unmatched callee Q should abort the walk")?;
    assert_eq!(
        error.kind,
        crate::GraphicsWalkErrorKind::GraphicsStateStackUnderflow
    );
    Ok(())
}

#[test]
fn invocation_path_plain_json_round_trip() -> Result<(), mini_json::JsonError> {
    let path = InvocationPath {
        frames: vec![
            InvocationFrame {
                ordinal: 0,
                name: name(b"A"),
            },
            InvocationFrame {
                ordinal: 3,
                name: name(b"Nested"),
            },
        ],
    };

    let json = mini_json::to_json(&path)?;
    assert_eq!(
        json,
        r#"{"frames":[{"ordinal":0,"name":[65]},{"ordinal":3,"name":[78,101,115,116,101,100]}]}"#
    );
    assert_eq!(mini_json::invocation_path_from_json(&json)?, path);
    assert_eq!(
        mini_json::invocation_path_from_json(r#"{"frames":[]}"#)?,
        InvocationPath { frames: Vec::new() }
    );
    Ok(())
}
