//! Minimal JSON-string serializer for the serde-transparency locks.
//!
//! Dependency-free on purpose (the crate has no JSON dev-dependency): it
//! supports exactly the data shapes [`GraphicsColor`](crate::GraphicsColor) and
//! the range types exercise — unsigned integers, `f64`, unit enum variants,
//! options, sequences, and structs — and rejects everything else.

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

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
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
