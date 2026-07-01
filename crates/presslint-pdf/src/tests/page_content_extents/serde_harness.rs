use std::{fmt, vec::IntoIter};

use serde::{
    Serialize, de,
    de::DeserializeOwned,
    forward_to_deserialize_any,
    ser::{self, Impossible, SerializeSeq, SerializeStruct},
};

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Value {
    Object(Vec<(String, Self)>),
    Array(Vec<Self>),
    String(String),
    U32(u32),
    Null,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct ValueError(String);

impl ValueError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

impl fmt::Display for ValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ValueError {}

impl ser::Error for ValueError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

impl de::Error for ValueError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

pub(super) fn serde_value<T: Serialize>(value: &T) -> Result<Value, ValueError> {
    value.serialize(ValueSerializer)
}

pub(super) fn from_serde_value<T: DeserializeOwned>(value: Value) -> Result<T, ValueError> {
    T::deserialize(value)
}

pub(super) struct ValueSerializer;

impl ser::Serializer for ValueSerializer {
    type Ok = Value;
    type Error = ValueError;
    type SerializeSeq = ArraySerializer;
    type SerializeTuple = Impossible<Value, ValueError>;
    type SerializeTupleStruct = Impossible<Value, ValueError>;
    type SerializeTupleVariant = Impossible<Value, ValueError>;
    type SerializeMap = Impossible<Value, ValueError>;
    type SerializeStruct = ObjectSerializer;
    type SerializeStructVariant = Impossible<Value, ValueError>;

    fn serialize_bool(self, _value: bool) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported bool value"))
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
        Ok(Value::U32(value))
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(u32::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.serialize_u32(u32::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        Ok(Value::U32(value))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        let value = u32::try_from(value).map_err(Self::Error::custom)?;
        Ok(Value::U32(value))
    }

    fn serialize_f32(self, _value: f32) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported f32 value"))
    }

    fn serialize_f64(self, _value: f64) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported f64 value"))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        self.serialize_str(&value.to_string())
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(Value::String(value.to_owned()))
    }

    fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Err(Self::Error::custom("unsupported bytes value"))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(Value::Null)
    }

    fn serialize_some<T: ?Sized + Serialize>(self, value: &T) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(Value::Null)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(Value::Null)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(Value::String(variant.to_owned()))
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
        Ok(Value::Object(vec![(
            variant.to_owned(),
            value.serialize(Self)?,
        )]))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(ArraySerializer { values: Vec::new() })
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Err(Self::Error::custom("unsupported tuple value"))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Err(Self::Error::custom("unsupported tuple struct value"))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Err(Self::Error::custom("unsupported tuple variant value"))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Err(Self::Error::custom("unsupported map value"))
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(ObjectSerializer { fields: Vec::new() })
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Err(Self::Error::custom("unsupported struct variant value"))
    }
}

pub(super) struct ArraySerializer {
    values: Vec<Value>,
}

impl SerializeSeq for ArraySerializer {
    type Ok = Value;
    type Error = ValueError;

    fn serialize_element<T: ?Sized + Serialize>(&mut self, value: &T) -> Result<(), Self::Error> {
        self.values.push(value.serialize(ValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(Value::Array(self.values))
    }
}

pub(super) struct ObjectSerializer {
    fields: Vec<(String, Value)>,
}

impl SerializeStruct for ObjectSerializer {
    type Ok = Value;
    type Error = ValueError;

    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.fields
            .push((key.to_owned(), value.serialize(ValueSerializer)?));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(Value::Object(self.fields))
    }
}

impl<'de> de::Deserializer<'de> for Value {
    type Error = ValueError;

    fn deserialize_any<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Object(fields) => visitor.visit_map(ObjectAccess {
                fields: fields.into_iter(),
                next_value: None,
            }),
            Self::Array(values) => visitor.visit_seq(ArrayAccess {
                values: values.into_iter(),
            }),
            Self::String(value) => visitor.visit_string(value),
            Self::U32(value) => visitor.visit_u32(value),
            Self::Null => visitor.visit_unit(),
        }
    }

    fn deserialize_option<V: de::Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
        match self {
            Self::Null => visitor.visit_none(),
            other => visitor.visit_some(other),
        }
    }

    fn deserialize_enum<V: de::Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error> {
        match self {
            Self::String(variant) => visitor.visit_enum(EnumAccess { variant }),
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
        byte_buf unit unit_struct seq tuple tuple_struct map struct
        identifier ignored_any
    }
}

struct ObjectAccess {
    fields: IntoIter<(String, Value)>,
    next_value: Option<Value>,
}

impl<'de> de::MapAccess<'de> for ObjectAccess {
    type Error = ValueError;

    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Self::Error> {
        let Some((key, value)) = self.fields.next() else {
            return Ok(None);
        };
        self.next_value = Some(value);
        seed.deserialize(Value::String(key)).map(Some)
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

struct ArrayAccess {
    values: IntoIter<Value>,
}

impl<'de> de::SeqAccess<'de> for ArrayAccess {
    type Error = ValueError;

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

struct EnumAccess {
    variant: String,
}

impl<'de> de::EnumAccess<'de> for EnumAccess {
    type Error = ValueError;
    type Variant = VariantAccess;

    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Self::Error> {
        let variant = seed.deserialize(Value::String(self.variant))?;
        Ok((variant, VariantAccess))
    }
}

struct VariantAccess;

impl<'de> de::VariantAccess<'de> for VariantAccess {
    type Error = ValueError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(
        self,
        _seed: T,
    ) -> Result<T::Value, Self::Error> {
        Err(Self::Error::custom("unsupported newtype enum variant"))
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
