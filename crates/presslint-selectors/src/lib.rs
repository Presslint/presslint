//! Serializable selectors for inventory entries.

#![forbid(unsafe_code)]

use presslint_core::{ColorSpace, ObjectKind, PageIndex};
use presslint_inventory::InventoryEntry;
use serde::{Deserialize, Serialize};

/// Boolean selector expression.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Selector {
    /// Match every entry.
    All,
    /// Match no entries.
    None,
    /// Negate an expression.
    Not {
        /// Expression to negate.
        expr: Box<Self>,
    },
    /// Match when every child matches.
    And {
        /// Child expressions evaluated with logical AND.
        exprs: Vec<Self>,
    },
    /// Match when any child matches.
    Or {
        /// Child expressions evaluated with logical OR.
        exprs: Vec<Self>,
    },
    /// Leaf predicate.
    Predicate {
        /// Predicate to evaluate.
        predicate: Predicate,
    },
}

/// Selector leaf predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Predicate {
    /// Match object kind.
    ObjectKind {
        /// Object kind to match.
        object_kind: ObjectKind,
    },
    /// Match observed color space.
    ColorSpace {
        /// Color space to match.
        space: ColorSpace,
    },
    /// Match zero-based page index.
    Page {
        /// Page index to match.
        page: PageIndex,
    },
    /// Match entries that advertise an edit capability.
    Editable {
        /// Required edit capability.
        capability: presslint_core::EditCapability,
    },
}

/// Evaluate a selector against one inventory entry.
#[must_use]
pub fn matches(selector: &Selector, entry: &InventoryEntry) -> bool {
    match selector {
        Selector::All => true,
        Selector::None => false,
        Selector::Not { expr } => !matches(expr, entry),
        Selector::And { exprs } => exprs.iter().all(|expr| matches(expr, entry)),
        Selector::Or { exprs } => exprs.iter().any(|expr| matches(expr, entry)),
        Selector::Predicate { predicate } => matches_predicate(predicate, entry),
    }
}

fn matches_predicate(predicate: &Predicate, entry: &InventoryEntry) -> bool {
    match predicate {
        Predicate::ObjectKind { object_kind } => entry.kind == *object_kind,
        Predicate::ColorSpace { space } => entry.colors.iter().any(|color| color.space == *space),
        Predicate::Page { page } => entry.id.page == *page,
        Predicate::Editable { capability } => entry.capabilities.contains(capability),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::missing_errors_doc)]

    use std::{fmt, vec::IntoIter};

    use presslint_core::{ColorSpace, EditCapability, ObjectKind, PageIndex};
    use serde::{
        Deserialize, Serialize, de, forward_to_deserialize_any,
        ser::{self, SerializeSeq, SerializeStruct},
    };

    use super::{Predicate, Selector};

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Json {
        Object(Vec<(String, Self)>),
        Array(Vec<Self>),
        String(String),
        U32(u32),
    }

    impl Json {
        fn object(fields: impl IntoIterator<Item = (&'static str, Self)>) -> Self {
            Self::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| (key.to_owned(), value))
                    .collect(),
            )
        }

        fn array(values: impl IntoIterator<Item = Self>) -> Self {
            Self::Array(values.into_iter().collect())
        }

        fn string(value: &'static str) -> Self {
            Self::String(value.to_owned())
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    struct JsonError(String);

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

    struct JsonSerializer;

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

        fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
            Err(Self::Error::custom(format!(
                "unsupported boolean JSON value {value}"
            )))
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

        fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
            Err(Self::Error::custom(format!(
                "unsupported f32 JSON value {value}"
            )))
        }

        fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
            Err(Self::Error::custom(format!(
                "unsupported f64 JSON value {value}"
            )))
        }

        fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
            self.serialize_str(&value.to_string())
        }

        fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
            Ok(Json::String(value.to_owned()))
        }

        fn serialize_bytes(self, _value: &[u8]) -> Result<Self::Ok, Self::Error> {
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

    struct JsonArraySerializer {
        values: Vec<Json>,
    }

    impl SerializeSeq for JsonArraySerializer {
        type Ok = Json;
        type Error = JsonError;

        fn serialize_element<T: ?Sized + Serialize>(
            &mut self,
            value: &T,
        ) -> Result<(), Self::Error> {
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

        fn serialize_element<T: ?Sized + Serialize>(
            &mut self,
            value: &T,
        ) -> Result<(), Self::Error> {
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

    struct JsonObjectSerializer {
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
                Self::String(variant) => visitor.visit_enum(JsonEnumAccess { variant }),
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
    }

    impl<'de> de::EnumAccess<'de> for JsonEnumAccess {
        type Error = JsonError;
        type Variant = JsonVariantAccess;

        fn variant_seed<V: de::DeserializeSeed<'de>>(
            self,
            seed: V,
        ) -> Result<(V::Value, Self::Variant), Self::Error> {
            let variant = seed.deserialize(Json::String(self.variant))?;
            Ok((variant, JsonVariantAccess))
        }
    }

    struct JsonVariantAccess;

    impl<'de> de::VariantAccess<'de> for JsonVariantAccess {
        type Error = JsonError;

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

    fn assert_selector_json(selector: &Selector, expected_json: Json) {
        let encoded = selector
            .serialize(JsonSerializer)
            .expect("serialize selector");
        assert_eq!(encoded, expected_json);

        let decoded = Selector::deserialize(expected_json).expect("deserialize selector fixture");
        assert_eq!(&decoded, selector);
    }

    fn assert_predicate_json(predicate: &Predicate, expected_json: Json) {
        let encoded = predicate
            .serialize(JsonSerializer)
            .expect("serialize predicate");
        assert_eq!(encoded, expected_json);

        let decoded = Predicate::deserialize(expected_json).expect("deserialize predicate fixture");
        assert_eq!(&decoded, predicate);
    }

    #[test]
    fn selector_boolean_variants_have_stable_json_shape() {
        assert_selector_json(&Selector::All, Json::object([("op", Json::string("all"))]));
        assert_selector_json(
            &Selector::None,
            Json::object([("op", Json::string("none"))]),
        );
        assert_selector_json(
            &Selector::Not {
                expr: Box::new(Selector::All),
            },
            Json::object([
                ("op", Json::string("not")),
                ("expr", Json::object([("op", Json::string("all"))])),
            ]),
        );
        assert_selector_json(
            &Selector::And {
                exprs: vec![Selector::All, Selector::None],
            },
            Json::object([
                ("op", Json::string("and")),
                (
                    "exprs",
                    Json::array([
                        Json::object([("op", Json::string("all"))]),
                        Json::object([("op", Json::string("none"))]),
                    ]),
                ),
            ]),
        );
        assert_selector_json(
            &Selector::Or {
                exprs: vec![Selector::None, Selector::All],
            },
            Json::object([
                ("op", Json::string("or")),
                (
                    "exprs",
                    Json::array([
                        Json::object([("op", Json::string("none"))]),
                        Json::object([("op", Json::string("all"))]),
                    ]),
                ),
            ]),
        );
    }

    #[test]
    fn predicate_variants_have_stable_json_shape() {
        assert_predicate_json(
            &Predicate::ObjectKind {
                object_kind: ObjectKind::Vector,
            },
            Json::object([
                ("kind", Json::string("object_kind")),
                ("object_kind", Json::string("vector")),
            ]),
        );
        assert_predicate_json(
            &Predicate::ColorSpace {
                space: ColorSpace::DeviceCmyk,
            },
            Json::object([
                ("kind", Json::string("color_space")),
                ("space", Json::string("device_cmyk")),
            ]),
        );
        assert_predicate_json(
            &Predicate::Page { page: PageIndex(3) },
            Json::object([("kind", Json::string("page")), ("page", Json::U32(3))]),
        );
        assert_predicate_json(
            &Predicate::Editable {
                capability: EditCapability::RewriteColorOperand,
            },
            Json::object([
                ("kind", Json::string("editable")),
                ("capability", Json::string("rewrite_color_operand")),
            ]),
        );
    }

    #[test]
    fn selector_predicate_fixtures_deserialize_to_expected_values() {
        assert_selector_json(
            &Selector::Predicate {
                predicate: Predicate::ObjectKind {
                    object_kind: ObjectKind::Image,
                },
            },
            Json::object([
                ("op", Json::string("predicate")),
                (
                    "predicate",
                    Json::object([
                        ("kind", Json::string("object_kind")),
                        ("object_kind", Json::string("image")),
                    ]),
                ),
            ]),
        );
        assert_selector_json(
            &Selector::Predicate {
                predicate: Predicate::ColorSpace {
                    space: ColorSpace::IccBased,
                },
            },
            Json::object([
                ("op", Json::string("predicate")),
                (
                    "predicate",
                    Json::object([
                        ("kind", Json::string("color_space")),
                        ("space", Json::string("icc_based")),
                    ]),
                ),
            ]),
        );
        assert_selector_json(
            &Selector::Predicate {
                predicate: Predicate::Page { page: PageIndex(0) },
            },
            Json::object([
                ("op", Json::string("predicate")),
                (
                    "predicate",
                    Json::object([("kind", Json::string("page")), ("page", Json::U32(0))]),
                ),
            ]),
        );
        assert_selector_json(
            &Selector::Predicate {
                predicate: Predicate::Editable {
                    capability: EditCapability::AdjustStrokeWidth,
                },
            },
            Json::object([
                ("op", Json::string("predicate")),
                (
                    "predicate",
                    Json::object([
                        ("kind", Json::string("editable")),
                        ("capability", Json::string("adjust_stroke_width")),
                    ]),
                ),
            ]),
        );
    }
}
