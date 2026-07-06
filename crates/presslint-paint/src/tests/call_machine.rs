//! Call/return machine + invocation-identity tests (Phase 0b-2..0b-4a).
//!
//! These exercise `CallMachine::walk`: caller-local form invocation ordinals,
//! depth-first descent with LIFO frame popping, the `on_return` hook, resolver
//! skip/error surfacing, image-name precedence, and the plain-JSON round trip of
//! the `InvocationPath` that identifies each expanded op.

use presslint_types::{InvocationFrame, InvocationPath, PdfName};

use super::{assemble, form_program, mini_json, name, page_program};
use crate::{
    CallEvent, CallMachine, CallSite, FormResolver, GraphicsWalkError, PaintOpKind,
    PaintSubProgram, ResolveForm,
};

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
