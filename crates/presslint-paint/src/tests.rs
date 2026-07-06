//! Focused tests for the replayable [`PaintProgram`](crate::PaintProgram) stream
//! and the typed provenance newtypes.
//!
//! These prove the two invariants the paint-program abstraction must hold to be a
//! faithful re-expression of the walker: it REPLAYS (iterating the same program
//! twice yields identical op sequences) and it AGREES with `walk_graphics_state`
//! (both the success case and the error-fusing short-circuit). The provenance
//! tests prove [`DecodedRange`] is serde-transparent: it serializes exactly like
//! the bare [`ByteRange`] it wraps and round-trips from the same wire shape.

use std::rc::Rc;

use presslint_syntax::{OperatorRecord, assemble_operators, tokenize};
use presslint_types::{
    ByteRange, ColorSpace, ContentScope, InvocationFrame, InvocationPath, PdfName,
};
use serde::Deserialize;
use serde::de::value::MapDeserializer;

use crate::{
    CallEvent, CallMachine, CallSite, ColorSpaceEnv, DecodedRange, FormResolver, GraphicsColor,
    GraphicsWalkError, MutationClass, PaintOp, PaintOpKind, PaintProgram, PaintSubProgram,
    ResolveForm, walk_graphics_state,
};

/// Tokenize + assemble a content stream into owned operator records for testing.
pub fn assemble(input: &[u8]) -> Result<Vec<OperatorRecord>, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    Ok(assembled.records)
}

/// Collect the program's ops as raw per-record results (no short-circuit), the
/// way a caller that wants every yielded item would.
fn raw_ops(program: PaintProgram<'_>) -> Vec<Result<PaintOp, GraphicsWalkError>> {
    program.into_iter().collect()
}

pub fn name(value: &[u8]) -> PdfName {
    PdfName(value.to_vec())
}

pub fn page_program<'a>(
    source: &'a [u8],
    records: &'a [OperatorRecord],
    images: &'a [PdfName],
    forms: &'a [PdfName],
) -> PaintSubProgram<'a> {
    PaintSubProgram {
        source,
        records,
        color_space_env: ColorSpaceEnv::empty(),
        image_xobject_names: images,
        form_xobject_names: forms,
        scope: ContentScope::Page,
    }
}

pub fn form_program<'a>(
    source: &'a [u8],
    records: &'a [OperatorRecord],
    images: &'a [PdfName],
    forms: &'a [PdfName],
    form_name: PdfName,
) -> PaintSubProgram<'a> {
    PaintSubProgram {
        source,
        records,
        color_space_env: ColorSpaceEnv::empty(),
        image_xobject_names: images,
        form_xobject_names: forms,
        scope: ContentScope::FormXObject { name: form_name },
    }
}

fn collect_xobject_paths(event: CallEvent<'_>, paths: &mut Vec<(InvocationPath, PdfName)>) {
    if let PaintOpKind::XObjectInvoke { name } = &event.op.kind {
        paths.push((event.path.clone(), name.clone()));
    }
}

#[test]
fn mutation_class_preserves_source_bytes_for_verbatim_routes() {
    assert!(MutationClass::PreserveBytes.preserves_source_bytes());
    assert!(!MutationClass::SurgicalRewrite.preserves_source_bytes());
    assert!(!MutationClass::AppearanceReplacement.preserves_source_bytes());
    assert!(MutationClass::UnsupportedSkip.preserves_source_bytes());
}

#[test]
fn mutation_class_may_emit_replacement_bytes_for_rewrite_routes() {
    assert!(!MutationClass::PreserveBytes.may_emit_replacement_bytes());
    assert!(MutationClass::SurgicalRewrite.may_emit_replacement_bytes());
    assert!(MutationClass::AppearanceReplacement.may_emit_replacement_bytes());
    assert!(!MutationClass::UnsupportedSkip.may_emit_replacement_bytes());
}

#[test]
fn paint_program_replays_identical_op_sequences() -> Result<(), String> {
    // A mixed, well-formed stream exercising save/restore, cm, colour, path
    // paint, text show, and both XObject/ExtGState invocations.
    let input: &[u8] = b"q 1 0 0 1 5 5 cm 0.4 g f BT (Hi) Tj ET /Im1 Do /GS1 gs Q";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());

    // Replay: two independent walks of the same descriptor are identical.
    let first = raw_ops(program);
    let second = raw_ops(program);
    assert_eq!(first, second);
    // The descriptor is Copy, so it is unconsumed and re-iterable a third time.
    assert_eq!(raw_ops(program), first);
    Ok(())
}

#[test]
fn paint_program_ops_equal_walk_graphics_state() -> Result<(), String> {
    let input: &[u8] = b"q 1 0 0 1 5 5 cm 0.4 g f BT (Hi) Tj ET /Im1 Do /GS1 gs Q";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());

    // Collecting Result items short-circuits to Result<Vec, _> exactly like the
    // materializing `walk_graphics_state`, so the two must be equal.
    let collected: Result<Vec<_>, _> = program.into_iter().collect();
    let walked = walk_graphics_state(input, &records);
    assert_eq!(collected, walked);
    Ok(())
}

#[test]
fn paint_program_fuses_on_first_error_matching_walk() -> Result<(), String> {
    // `0.4 g f` is well-formed; the malformed `1 2 RG` (three operands expected,
    // two given) sits after it. The program must yield ops up to and including
    // the Err, then fuse to None forever.
    let input: &[u8] = b"0.4 g f 1 2 RG";
    let records = assemble(input)?;
    let program = PaintProgram::new(input, &records, ColorSpaceEnv::empty());

    let mut ops = program.into_iter();
    let mut yielded = Vec::new();
    for item in ops.by_ref() {
        let is_err = item.is_err();
        yielded.push(item);
        if is_err {
            break;
        }
    }

    // The last yielded item is the Err, and it matches what the materializing
    // walk surfaces for the same malformed record.
    let last = yielded.last().ok_or("at least one op should be yielded")?;
    assert!(last.is_err());
    let walked_err = walk_graphics_state(input, &records)
        .err()
        .ok_or("walk should fail on the malformed record")?;
    assert_eq!(last.as_ref().err(), Some(&walked_err));

    // Fused: every subsequent poll is None, forever.
    assert!(ops.next().is_none());
    assert!(ops.next().is_none());

    // And the short-circuiting collect agrees byte-for-byte with the walk.
    let collected: Result<Vec<_>, _> = program.into_iter().collect();
    assert_eq!(collected, walk_graphics_state(input, &records));
    Ok(())
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

/// Walk `input` into materialized ops, mapping any walker/assemble error to a
/// `String` so the `Rc`-sharing tests can use `?`.
fn walk(input: &[u8]) -> Result<Vec<PaintOp>, String> {
    let records = assemble(input)?;
    walk_graphics_state(input, &records).map_err(|error| format!("{error:?}"))
}

#[test]
fn no_state_change_ops_share_the_same_interned_state() -> Result<(), String> {
    // These operators emit paint ops without mutating the graphics state.
    let ops = walk(b"n /Im1 Do /GS1 gs (Hi) Tj")?;
    assert_eq!(ops.len(), 4);
    for window in ops.windows(2) {
        assert!(
            Rc::ptr_eq(&window[0].state, &window[1].state),
            "no-state-change ops must share one interned state"
        );
    }
    Ok(())
}

#[test]
fn save_restore_preserves_interned_state_identity() -> Result<(), String> {
    let ops = walk(b"q 1 0 0 1 5 5 cm Q n")?;
    assert_eq!(ops.len(), 4);
    let saved = &ops[0].state;
    let concat = &ops[1].state;
    let restored = &ops[2].state;
    let after = &ops[3].state;

    assert!(
        Rc::ptr_eq(saved, restored),
        "post-`Q` state must be the exact saved pre-`cm` `Rc`"
    );
    assert!(
        Rc::ptr_eq(restored, after),
        "`n` must not disturb the restored interned state"
    );
    assert!(
        !Rc::ptr_eq(concat, saved),
        "`cm` must copy-on-write to a distinct snapshot"
    );
    Ok(())
}

#[test]
fn decoded_range_serializes_exactly_like_the_bare_byte_range() -> Result<(), mini_json::JsonError> {
    let range = ByteRange { start: 3, end: 18 };

    // `#[serde(transparent)]`: the newtype and the bare range must produce the
    // SAME wire bytes — a plain `{"start":..,"end":..}` object.
    assert_eq!(mini_json::to_json(&range)?, r#"{"start":3,"end":18}"#);
    assert_eq!(
        mini_json::to_json(&DecodedRange::new(range))?,
        mini_json::to_json(&range)?
    );
    Ok(())
}

#[test]
fn graphics_color_with_decoded_source_keeps_the_prior_json_shape()
-> Result<(), mini_json::JsonError> {
    // The typed `source` field must serialize as the plain range object it was
    // before the newtype adoption — no wrapper, no extra nesting.
    let color = GraphicsColor {
        space: ColorSpace::DeviceCmyk,
        components: vec![0.0, 0.0, 0.0, 1.0],
        resource_name: None,
        spot_name: None,
        source: Some(DecodedRange::new(ByteRange { start: 3, end: 18 })),
    };

    assert_eq!(
        mini_json::to_json(&color)?,
        concat!(
            r#"{"space":"device_cmyk","components":[0,0,0,1],"#,
            r#""resource_name":null,"spot_name":null,"source":{"start":3,"end":18}}"#
        )
    );
    Ok(())
}

#[test]
fn decoded_range_round_trips_from_the_bare_byte_range_wire_shape() -> Result<(), String> {
    // Deserializing the newtype from the exact map shape a bare `ByteRange`
    // serializes to proves the round-trip is transparent in both directions.
    let entries = [("start", 3_usize), ("end", 18_usize)];
    let decoded = DecodedRange::deserialize(MapDeserializer::<_, serde::de::value::Error>::new(
        entries.into_iter(),
    ))
    .map_err(|error| error.to_string())?;
    let bare = ByteRange::deserialize(MapDeserializer::<_, serde::de::value::Error>::new(
        entries.into_iter(),
    ))
    .map_err(|error| error.to_string())?;

    assert_eq!(decoded, DecodedRange::new(ByteRange { start: 3, end: 18 }));
    assert_eq!(decoded, DecodedRange::new(bare));
    assert_eq!(decoded.into_byte_range(), bare);
    Ok(())
}

/// Minimal JSON-string serializer for the serde-transparency locks.
///
/// Dependency-free on purpose (the crate has no JSON dev-dependency): it
/// supports exactly the data shapes [`GraphicsColor`] and the range types
/// exercise — unsigned integers, `f64`, unit enum variants, options, sequences,
/// and structs — and rejects everything else.
mod mini_json {
    use std::fmt;

    use presslint_types::{InvocationFrame, InvocationPath, PdfName};
    use serde::{
        Serialize,
        ser::{self, Impossible},
    };

    /// Serialize `value` to a compact JSON string.
    pub(super) fn to_json<T: Serialize>(value: &T) -> Result<String, JsonError> {
        value.serialize(JsonWriter)
    }

    /// Parse the plain JSON shape emitted for [`InvocationPath`].
    pub(super) fn invocation_path_from_json(input: &str) -> Result<InvocationPath, JsonError> {
        let mut parser = Parser::new(input);
        let path = parser.invocation_path()?;
        parser.end()?;
        Ok(path)
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct JsonError(String);

    impl JsonError {
        fn custom<T: fmt::Display>(message: T) -> Self {
            Self(message.to_string())
        }

        fn unsupported(what: &str) -> Self {
            Self(format!("unsupported JSON value: {what}"))
        }
    }

    impl fmt::Display for JsonError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl std::error::Error for JsonError {}

    impl ser::Error for JsonError {
        fn custom<T: fmt::Display>(message: T) -> Self {
            Self::custom(message)
        }
    }

    struct Parser<'a> {
        bytes: &'a [u8],
        cursor: usize,
    }

    impl<'a> Parser<'a> {
        fn new(input: &'a str) -> Self {
            Self {
                bytes: input.as_bytes(),
                cursor: 0,
            }
        }

        fn invocation_path(&mut self) -> Result<InvocationPath, JsonError> {
            self.expect(b"{\"frames\":")?;
            let frames = self.frames()?;
            self.expect(b"}")?;
            Ok(InvocationPath { frames })
        }

        fn frames(&mut self) -> Result<Vec<InvocationFrame>, JsonError> {
            self.expect(b"[")?;
            let mut frames = Vec::new();
            if self.eat(b"]") {
                return Ok(frames);
            }
            loop {
                frames.push(self.frame()?);
                if self.eat(b"]") {
                    return Ok(frames);
                }
                self.expect(b",")?;
            }
        }

        fn frame(&mut self) -> Result<InvocationFrame, JsonError> {
            self.expect(b"{\"ordinal\":")?;
            let ordinal = self.u32()?;
            self.expect(b",\"name\":")?;
            let name = PdfName(self.byte_array()?);
            self.expect(b"}")?;
            Ok(InvocationFrame { ordinal, name })
        }

        fn byte_array(&mut self) -> Result<Vec<u8>, JsonError> {
            self.expect(b"[")?;
            let mut bytes = Vec::new();
            if self.eat(b"]") {
                return Ok(bytes);
            }
            loop {
                bytes.push(self.u8()?);
                if self.eat(b"]") {
                    return Ok(bytes);
                }
                self.expect(b",")?;
            }
        }

        fn u8(&mut self) -> Result<u8, JsonError> {
            let value = self.u32()?;
            u8::try_from(value).map_err(|_| JsonError::custom("byte value out of range"))
        }

        fn u32(&mut self) -> Result<u32, JsonError> {
            let start = self.cursor;
            while self.bytes.get(self.cursor).is_some_and(u8::is_ascii_digit) {
                self.cursor += 1;
            }
            if self.cursor == start {
                return Err(JsonError::custom("expected unsigned integer"));
            }
            let text = std::str::from_utf8(&self.bytes[start..self.cursor])
                .map_err(|error| JsonError::custom(error.to_string()))?;
            text.parse::<u32>()
                .map_err(|error| JsonError::custom(error.to_string()))
        }

        fn end(&self) -> Result<(), JsonError> {
            if self.cursor == self.bytes.len() {
                Ok(())
            } else {
                Err(JsonError::custom("trailing JSON content"))
            }
        }

        fn eat(&mut self, expected: &[u8]) -> bool {
            if self.bytes[self.cursor..].starts_with(expected) {
                self.cursor += expected.len();
                true
            } else {
                false
            }
        }

        fn expect(&mut self, expected: &[u8]) -> Result<(), JsonError> {
            if self.eat(expected) {
                Ok(())
            } else {
                Err(JsonError::custom(format!(
                    "expected `{}`",
                    String::from_utf8_lossy(expected)
                )))
            }
        }
    }

    struct JsonWriter;

    impl ser::Serializer for JsonWriter {
        type Ok = String;
        type Error = JsonError;
        type SerializeSeq = ArrayWriter;
        type SerializeTuple = Impossible<String, JsonError>;
        type SerializeTupleStruct = Impossible<String, JsonError>;
        type SerializeTupleVariant = Impossible<String, JsonError>;
        type SerializeMap = Impossible<String, JsonError>;
        type SerializeStruct = ObjectWriter;
        type SerializeStructVariant = Impossible<String, JsonError>;

        fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
            Err(JsonError::unsupported("bool"))
        }

        fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
            self.serialize_i64(i64::from(value))
        }

        fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
            self.serialize_i64(i64::from(value))
        }

        fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
            self.serialize_i64(i64::from(value))
        }

        fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
            Ok(value.to_string())
        }

        fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
            self.serialize_u64(u64::from(value))
        }

        fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
            self.serialize_u64(u64::from(value))
        }

        fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
            self.serialize_u64(u64::from(value))
        }

        fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
            Ok(value.to_string())
        }

        fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
            self.serialize_f64(f64::from(value))
        }

        fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
            Ok(value.to_string())
        }

        fn serialize_char(self, _value: char) -> Result<Self::Ok, Self::Error> {
            Err(JsonError::unsupported("char"))
        }

        fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
            // The locked shapes only emit plain identifier-like strings.
            Ok(format!("\"{value}\""))
        }

        fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
            Err(JsonError::unsupported("bytes"))
        }

        fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
            Ok("null".to_owned())
        }

        fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
            value.serialize(self)
        }

        fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
            Err(JsonError::unsupported("unit"))
        }

        fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
            Err(JsonError::unsupported("unit struct"))
        }

        fn serialize_unit_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            variant: &'static str,
        ) -> Result<Self::Ok, Self::Error> {
            self.serialize_str(variant)
        }

        fn serialize_newtype_struct<T: ?Sized + Serialize>(
            self,
            _name: &'static str,
            value: &T,
        ) -> Result<Self::Ok, Self::Error> {
            value.serialize(self)
        }

        fn serialize_newtype_variant<T: ?Sized + Serialize>(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _value: &T,
        ) -> Result<Self::Ok, Self::Error> {
            Err(JsonError::unsupported("newtype variant"))
        }

        fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
            Ok(ArrayWriter { items: Vec::new() })
        }

        fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
            Err(JsonError::unsupported("tuple"))
        }

        fn serialize_tuple_struct(
            self,
            _name: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleStruct, Self::Error> {
            Err(JsonError::unsupported("tuple struct"))
        }

        fn serialize_tuple_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeTupleVariant, Self::Error> {
            Err(JsonError::unsupported("tuple variant"))
        }

        fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
            Err(JsonError::unsupported("map"))
        }

        fn serialize_struct(
            self,
            _name: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeStruct, Self::Error> {
            Ok(ObjectWriter { fields: Vec::new() })
        }

        fn serialize_struct_variant(
            self,
            _name: &'static str,
            _variant_index: u32,
            _variant: &'static str,
            _len: usize,
        ) -> Result<Self::SerializeStructVariant, Self::Error> {
            Err(JsonError::unsupported("struct variant"))
        }
    }

    struct ArrayWriter {
        items: Vec<String>,
    }

    impl ser::SerializeSeq for ArrayWriter {
        type Ok = String;
        type Error = JsonError;

        fn serialize_element<T: ?Sized + Serialize>(
            &mut self,
            value: &T,
        ) -> Result<(), Self::Error> {
            self.items.push(value.serialize(JsonWriter)?);
            Ok(())
        }

        fn end(self) -> Result<Self::Ok, Self::Error> {
            Ok(format!("[{}]", self.items.join(",")))
        }
    }

    struct ObjectWriter {
        fields: Vec<String>,
    }

    impl ser::SerializeStruct for ObjectWriter {
        type Ok = String;
        type Error = JsonError;

        fn serialize_field<T: ?Sized + Serialize>(
            &mut self,
            key: &'static str,
            value: &T,
        ) -> Result<(), Self::Error> {
            let value = value.serialize(JsonWriter)?;
            self.fields.push(format!("\"{key}\":{value}"));
            Ok(())
        }

        fn end(self) -> Result<Self::Ok, Self::Error> {
            Ok(format!("{{{}}}", self.fields.join(",")))
        }
    }
}
