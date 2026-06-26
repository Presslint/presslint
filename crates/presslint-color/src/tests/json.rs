//! Minimal in-memory JSON model and serde adapters for color shape tests.
//!
//! This harness keeps the focused serde shape tests dependency-free: it
//! reflects values into a structural [`Json`] tree and back, so tests can lock
//! byte-stable field order without pulling in a JSON crate. It mirrors the
//! `presslint-selectors`/`presslint-actions` harnesses.
//!
//! The locked color contracts carry no `bool` or `f64` fields, so this model
//! intentionally rejects those scalars. It does support the externally-tagged
//! newtype enum variant `presslint-core::ColorSpace::Resource(PdfName)`, whose
//! round-trip is pinned by the transform shape tests.

#![allow(clippy::expect_used, clippy::missing_errors_doc)]

use std::{fmt, vec::IntoIter};

use serde::{
    Serialize, de, forward_to_deserialize_any,
    ser::{self, SerializeSeq, SerializeStruct},
};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Json {
    Object(Vec<(String, Self)>),
    Array(Vec<Self>),
    String(String),
    U32(u32),
}

impl Json {
    pub(super) fn object(fields: impl IntoIterator<Item = (&'static str, Self)>) -> Self {
        Self::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value))
                .collect(),
        )
    }

    pub(super) fn array(values: impl IntoIterator<Item = Self>) -> Self {
        Self::Array(values.into_iter().collect())
    }

    pub(super) fn string(value: &'static str) -> Self {
        Self::String(value.to_owned())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct JsonError(String);

impl JsonError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
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

impl de::Error for JsonError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

pub(super) struct JsonSerializer;

impl ser::Serializer for JsonSerializer {
    type Ok = Json;
    type Error = JsonError;
    type SerializeSeq = JsonArraySerializer;
    type SerializeTuple = JsonArraySerializer;
    type SerializeTupleStruct = JsonArraySerializer;
    type SerializeTupleVariant = JsonArraySerializer;
    type SerializeMap = JsonObjectSerializer;
    type SerializeStruct = JsonObjectSerializer;
    type SerializeStructVariant = JsonObjectSerializer;

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported bool JSON value"))
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
        let value = u32::try_from(value).map_err(Self::Error::custom)?;
        Ok(Json::U32(value))
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(u32::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(u32::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        Ok(Json::U32(value))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        let value = u32::try_from(value).map_err(Self::Error::custom)?;
        Ok(Json::U32(value))
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported float JSON value"))
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported float JSON value"))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(&value.to_string())
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(Json::String(value.to_owned()))
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        // Reachable only via `serde_bytes`-style byte serialization. None of the
        // locked color contracts use it: `PdfName` and `EmbeddedBytes` wrap
        // `Vec<u8>`, which serde serializes element-by-element through
        // `serialize_seq`. Rejecting it keeps the harness to exactly what the
        // fixtures exercise, matching the sibling crates.
        Err(Self::Error::custom("unsupported bytes JSON value"))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported null JSON value"))
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported unit JSON value"))
    }

    fn serialize_unit_struct(self, name: &'static str) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom(format!(
            "unsupported unit struct {name}"
        )))
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(Json::String(variant.to_owned()))
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
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(Json::object([(variant, value.serialize(Self)?)]))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(JsonArraySerializer { values: Vec::new() })
    }

    fn serialize_tuple(self, len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        self.serialize_seq(Some(len))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(JsonObjectSerializer {
            fields: Vec::new(),
            next_key: None,
        })
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        self.serialize_map(None)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        self.serialize_map(None)
    }
}

pub(super) struct JsonArraySerializer {
    values: Vec<Json>,
}

impl SerializeSeq for JsonArraySerializer {
    type Ok = Json;
    type Error = JsonError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.values.push(value.serialize(JsonSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(Json::Array(self.values))
    }
}

impl ser::SerializeTuple for JsonArraySerializer {
    type Ok = Json;
    type Error = JsonError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for JsonArraySerializer {
    type Ok = Json;
    type Error = JsonError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleVariant for JsonArraySerializer {
    type Ok = Json;
    type Error = JsonError;

    fn serialize_field<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        SerializeSeq::serialize_element(self, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeSeq::end(self)
    }
}

pub(super) struct JsonObjectSerializer {
    fields: Vec<(String, Json)>,
    next_key: Option<String>,
}

impl ser::SerializeMap for JsonObjectSerializer {
    type Ok = Json;
    type Error = JsonError;

    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Self::Error> {
        match key.serialize(JsonSerializer)? {
            Json::String(key) => {
                self.next_key = Some(key);
                Ok(())
            }
            other => Err(Self::Error::custom(format!(
                "unsupported object key {other:?}"
            ))),
        }
    }

    fn serialize_value<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        let key = self
            .next_key
            .take()
            .ok_or_else(|| Self::Error::custom("missing object key"))?;
        self.fields.push((key, value.serialize(JsonSerializer)?));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(Json::Object(self.fields))
    }
}

impl SerializeStruct for JsonObjectSerializer {
    type Ok = Json;
    type Error = JsonError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.fields
            .push((key.to_owned(), value.serialize(JsonSerializer)?));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(Json::Object(self.fields))
    }
}

impl ser::SerializeStructVariant for JsonObjectSerializer {
    type Ok = Json;
    type Error = JsonError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        SerializeStruct::serialize_field(self, key, value)
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        SerializeStruct::end(self)
    }
}

impl de::IntoDeserializer<'_, JsonError> for Json {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self::Deserializer {
        self
    }
}

impl<'de> de::Deserializer<'de> for Json {
    type Error = JsonError;

    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Object(fields) => visitor.visit_map(JsonMapAccess {
                fields: fields.into_iter(),
                next_value: None,
            }),
            Self::Array(values) => visitor.visit_seq(JsonSeqAccess {
                values: values.into_iter(),
            }),
            Self::String(value) => visitor.visit_string(value),
            Self::U32(value) => visitor.visit_u32(value),
        }
    }

    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            // Unit variant: an externally-tagged enum encodes as the bare
            // variant name string.
            Self::String(variant) => visitor.visit_enum(JsonEnumAccess {
                variant,
                value: None,
            }),
            // Newtype variant: an externally-tagged enum encodes as a single
            // `{ "variant": value }` object (e.g. `ColorSpace::Resource`).
            Self::Object(fields) if fields.len() == 1 => {
                let (variant, value) = fields.into_iter().next().expect("length checked above");
                visitor.visit_enum(JsonEnumAccess {
                    variant,
                    value: Some(value),
                })
            }
            other => other.deserialize_any(visitor),
        }
    }

    fn deserialize_newtype_struct<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        visitor.visit_newtype_struct(self)
    }

    forward_to_deserialize_any! {
        bool i8 i16 i32 i64 u8 u16 u32 u64 f32 f64 char str string bytes
        byte_buf option unit unit_struct seq tuple tuple_struct map struct
        identifier ignored_any
    }
}

struct JsonMapAccess {
    fields: IntoIter<(String, Json)>,
    next_value: Option<Json>,
}

impl<'de> de::MapAccess<'de> for JsonMapAccess {
    type Error = JsonError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let Some((key, value)) = self.fields.next() else {
            return Ok(None);
        };
        self.next_value = Some(value);
        seed.deserialize(Json::String(key)).map(Some)
    }

    fn next_value_seed<V: de::DeserializeSeed<'de>>(
        &mut self,
        seed: V,
    ) -> Result<V::Value, Self::Error> {
        let value = self
            .next_value
            .take()
            .ok_or_else(|| Self::Error::custom("missing object value"))?;
        seed.deserialize(value)
    }
}

struct JsonSeqAccess {
    values: IntoIter<Json>,
}

impl<'de> de::SeqAccess<'de> for JsonSeqAccess {
    type Error = JsonError;

    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Self::Error> {
        self.values
            .next()
            .map(|value| seed.deserialize(value))
            .transpose()
    }
}

struct JsonEnumAccess {
    variant: String,
    value: Option<Json>,
}

impl<'de> de::EnumAccess<'de> for JsonEnumAccess {
    type Error = JsonError;
    type Variant = JsonVariantAccess;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(Json::String(self.variant))?;
        Ok((variant, JsonVariantAccess { value: self.value }))
    }
}

struct JsonVariantAccess {
    value: Option<Json>,
}

impl<'de> de::VariantAccess<'de> for JsonVariantAccess {
    type Error = JsonError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<T::Value, Self::Error> {
        let value = self
            .value
            .ok_or_else(|| Self::Error::custom("missing newtype enum variant value"))?;
        seed.deserialize(value)
    }

    fn tuple_variant<V: de::Visitor<'de>>(
        self,
        _len: usize,
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(Self::Error::custom("unsupported tuple enum variant"))
    }

    fn struct_variant<V: de::Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        _visitor: V,
    ) -> Result<V::Value, Self::Error> {
        Err(Self::Error::custom("unsupported struct enum variant"))
    }
}
