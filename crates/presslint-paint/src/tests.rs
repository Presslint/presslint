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
use presslint_types::{ByteRange, ColorSpace};
use serde::Deserialize;
use serde::de::value::MapDeserializer;

use crate::{
    ColorSpaceEnv, DecodedRange, GraphicsColor, GraphicsWalkError, PaintOp, PaintProgram,
    walk_graphics_state,
};

/// Tokenize + assemble a content stream into owned operator records for testing.
fn assemble(input: &[u8]) -> Result<Vec<OperatorRecord>, String> {
    let tokens = tokenize(input).map_err(|error| format!("{error:?}"))?;
    let assembled = assemble_operators(&tokens).map_err(|error| format!("{error:?}"))?;
    Ok(assembled.records)
}

/// Collect the program's ops as raw per-record results (no short-circuit), the
/// way a caller that wants every yielded item would.
fn raw_ops(program: PaintProgram<'_>) -> Vec<Result<PaintOp, GraphicsWalkError>> {
    program.into_iter().collect()
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

    use serde::{
        Serialize,
        ser::{self, Impossible},
    };

    /// Serialize `value` to a compact JSON string.
    pub(super) fn to_json<T: Serialize>(value: &T) -> Result<String, JsonError> {
        value.serialize(JsonWriter)
    }

    #[derive(Debug, PartialEq, Eq)]
    pub(super) struct JsonError(String);

    impl JsonError {
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
            Self(message.to_string())
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
