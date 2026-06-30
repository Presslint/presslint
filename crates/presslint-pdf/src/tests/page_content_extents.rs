use super::classic_inspection;

use crate::{
    ClassicXrefObjectLocation, ClassicXrefTableInspection, IndirectRef, IndirectReferenceByteRange,
    PageContentExtentInspection, PageContentExtentsInspection, PageContentReference,
    PageContentTargetInspection, PageContentTargetsInspection, SkippedPageContentTargetReason,
    inspect_catalog_pages, inspect_classic_xref_table, inspect_classic_xref_trailer_root,
    inspect_content_stream_data_extent, inspect_page_content_extents, inspect_page_content_targets,
    inspect_page_contents, inspect_page_tree_kids, inspect_page_tree_reference_target,
};

use serde_harness::{from_serde_value, serde_value};

fn empty_xref() -> ClassicXrefTableInspection {
    classic_inspection(Vec::new())
}

fn content_reference(object_number: u32) -> PageContentReference {
    PageContentReference {
        reference: IndirectRef {
            object_number,
            generation: 0,
        },
        reference_range: IndirectReferenceByteRange { start: 0, end: 0 },
    }
}

fn resolved_target(object_number: u32, object_byte_offset: usize) -> PageContentTargetInspection {
    PageContentTargetInspection::Resolved {
        content_reference: content_reference(object_number),
        object_byte_offset,
        xref_generation: 0,
    }
}

fn not_found_skip(object_number: u32) -> PageContentTargetInspection {
    PageContentTargetInspection::Skipped {
        content_reference: content_reference(object_number),
        reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
            location: ClassicXrefObjectLocation::NotFound { object_number },
        },
    }
}

fn make_targets(
    byte_len: usize,
    entries: Vec<PageContentTargetInspection>,
) -> PageContentTargetsInspection {
    PageContentTargetsInspection { byte_len, entries }
}

#[test]
fn single_reference_page_locates_one_direct_length_extent() {
    let source = b"5 0 obj\n<< /Length 12 >>\nstream\nhello world!\nendstream\nendobj\n";
    let xref = empty_xref();
    let targets = make_targets(source.len(), vec![resolved_target(5, 0)]);

    let report = inspect_page_content_extents(source, &xref, &targets);

    assert_eq!(report.byte_len, source.len());
    assert_eq!(report.located_count(), 1);
    let expected = inspect_content_stream_data_extent(source, Some(&xref), 0)
        .expect("direct-length extent should inspect");
    assert_eq!(
        &source[expected.stream_data_start_byte_offset()..expected.stream_data_end_byte_offset()],
        b"hello world!"
    );
    assert_eq!(
        report.entries,
        vec![PageContentExtentInspection::Located {
            content_reference: content_reference(5),
            object_byte_offset: 0,
            extent: expected,
        }]
    );
}

#[test]
fn multi_stream_array_page_locates_extents_in_content_order() {
    let first = b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n";
    let second = b"6 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(first);
    let second_offset = source.len();
    source.extend_from_slice(second);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![resolved_target(5, 0), resolved_target(6, second_offset)],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.located_count(), 2);
    let first_extent = inspect_content_stream_data_extent(&source, Some(&xref), 0)
        .expect("first direct-length extent should inspect");
    let second_extent = inspect_content_stream_data_extent(&source, Some(&xref), second_offset)
        .expect("second direct-length extent should inspect");
    assert_eq!(
        &source[first_extent.stream_data_start_byte_offset()
            ..first_extent.stream_data_end_byte_offset()],
        b"abc"
    );
    assert_eq!(
        &source[second_extent.stream_data_start_byte_offset()
            ..second_extent.stream_data_end_byte_offset()],
        b"hello"
    );
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Located {
                content_reference: content_reference(5),
                object_byte_offset: 0,
                extent: first_extent,
            },
            PageContentExtentInspection::Located {
                content_reference: content_reference(6),
                object_byte_offset: second_offset,
                extent: second_extent,
            },
        ]
    );
}

#[test]
fn page_mixing_resolved_stream_and_skipped_target_preserves_skip() {
    let source = b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n";
    let xref = empty_xref();
    let targets = make_targets(source.len(), vec![resolved_target(5, 0), not_found_skip(6)]);

    let report = inspect_page_content_extents(source, &xref, &targets);

    assert_eq!(report.located_count(), 1);
    let extent = inspect_content_stream_data_extent(source, Some(&xref), 0)
        .expect("direct-length extent should inspect");
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Located {
                content_reference: content_reference(5),
                object_byte_offset: 0,
                extent,
            },
            PageContentExtentInspection::Skipped {
                content_reference: content_reference(6),
                reason: SkippedPageContentTargetReason::UnresolvedXrefLocation {
                    location: ClassicXrefObjectLocation::NotFound { object_number: 6 },
                },
            },
        ]
    );
}

#[test]
fn resolved_target_with_failing_extent_still_processes_later_targets() {
    let malformed = b"5 0 obj\n<< /Other 1 >>\nstream\nabc\nendstream\nendobj\n";
    let healthy = b"6 0 obj\n<< /Length 3 >>\nstream\nxyz\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(malformed);
    let healthy_offset = source.len();
    source.extend_from_slice(healthy);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![resolved_target(5, 0), resolved_target(6, healthy_offset)],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.located_count(), 1);
    let expected_error = inspect_content_stream_data_extent(&source, Some(&xref), 0)
        .expect_err("missing /Length should fail extent inspection");
    let healthy_extent = inspect_content_stream_data_extent(&source, Some(&xref), healthy_offset)
        .expect("healthy direct-length extent should inspect");
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Failed {
                content_reference: content_reference(5),
                object_byte_offset: 0,
                error: expected_error,
            },
            PageContentExtentInspection::Located {
                content_reference: content_reference(6),
                object_byte_offset: healthy_offset,
                extent: healthy_extent,
            },
        ]
    );
}

#[test]
fn located_count_reports_only_located_entries() {
    let healthy = b"7 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n";
    let malformed = b"5 0 obj\n<< /Other 1 >>\nstream\nz\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(healthy);
    let malformed_offset = source.len();
    source.extend_from_slice(malformed);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![
            resolved_target(7, 0),
            not_found_skip(6),
            resolved_target(5, malformed_offset),
        ],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.entries.len(), 3);
    assert_eq!(report.located_count(), 1);
}

#[test]
fn serde_round_trip_preserves_aggregate_report_and_failed_entry() {
    let healthy = b"6 0 obj\n<< /Length 1 >>\nstream\nq\nendstream\nendobj\n";
    let malformed = b"5 0 obj\n<< /Other 1 >>\nstream\nz\nendstream\nendobj\n";
    let mut source = Vec::new();
    source.extend_from_slice(healthy);
    let malformed_offset = source.len();
    source.extend_from_slice(malformed);
    let xref = empty_xref();
    let targets = make_targets(
        source.len(),
        vec![resolved_target(6, 0), resolved_target(5, malformed_offset)],
    );

    let report = inspect_page_content_extents(&source, &xref, &targets);
    assert_eq!(report.located_count(), 1);

    let value = serde_value(&report).expect("aggregate report should serialize");
    let restored: PageContentExtentsInspection =
        from_serde_value(value).expect("aggregate report should deserialize");
    assert_eq!(restored, report);

    let failed_entry = report.entries[1].clone();
    assert!(matches!(
        failed_entry,
        PageContentExtentInspection::Failed { .. }
    ));
    let entry_value = serde_value(&failed_entry).expect("failed entry should serialize");
    let restored_entry: PageContentExtentInspection =
        from_serde_value(entry_value).expect("failed entry should deserialize");
    assert_eq!(restored_entry, failed_entry);
}

#[test]
fn composition_chains_page_contents_targets_and_aggregator() {
    let prefix = b"%PDF-1.7\n";
    let catalog = b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n";
    let pages = b"2 0 obj\n<< /Type /Pages /Kids [ 3 0 R ] /Count 1 >>\nendobj\n";
    let page = b"3 0 obj\n<< /Type /Page /Parent 2 0 R /Contents [ 5 0 R 6 0 R ] >>\nendobj\n";
    let content_five = b"5 0 obj\n<< /Length 3 >>\nstream\nabc\nendstream\nendobj\n";
    let content_six = b"6 0 obj\n<< /Length 5 >>\nstream\nhello\nendstream\nendobj\n";
    let catalog_offset = prefix.len();
    let pages_offset = prefix.len() + catalog.len();
    let page_offset = pages_offset + pages.len();
    let content_five_offset = page_offset + page.len();
    let content_six_offset = content_five_offset + content_five.len();
    let xref_offset = content_six_offset + content_six.len();
    let source = format!(
        "{}{}{}{}{}{}xref\n0 7\n0000000000 65535 f \n{catalog_offset:010} 00000 n \n{pages_offset:010} 00000 n \n{page_offset:010} 00000 n \n0000000000 00000 f \n{content_five_offset:010} 00000 n \n{content_six_offset:010} 00000 n \ntrailer\n<< /Size 7 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
        String::from_utf8_lossy(prefix),
        String::from_utf8_lossy(catalog),
        String::from_utf8_lossy(pages),
        String::from_utf8_lossy(page),
        String::from_utf8_lossy(content_five),
        String::from_utf8_lossy(content_six),
    )
    .into_bytes();

    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref should inspect");
    let root = inspect_classic_xref_trailer_root(&source, xref.trailer_byte_offset)
        .expect("trailer root should inspect");
    let catalog_target = inspect_page_tree_reference_target(&source, &xref, root.root_reference)
        .expect("catalog reference should resolve");
    let catalog_pages = inspect_catalog_pages(&source, catalog_target.object_byte_offset)
        .expect("catalog pages should inspect");
    let page_tree =
        inspect_page_tree_reference_target(&source, &xref, catalog_pages.pages_reference)
            .expect("page tree should resolve");
    let kids =
        inspect_page_tree_kids(&source, page_tree.object_byte_offset).expect("kids should inspect");
    let page_target = inspect_page_tree_reference_target(&source, &xref, kids.kids[0].reference)
        .expect("page should resolve");
    let contents = inspect_page_contents(&source, page_target.object_byte_offset)
        .expect("page contents should inspect");
    let targets = inspect_page_content_targets(&source, &xref, &contents);

    let report = inspect_page_content_extents(&source, &xref, &targets);

    assert_eq!(report.located_count(), 2);
    let expected_five =
        inspect_content_stream_data_extent(&source, Some(&xref), content_five_offset)
            .expect("first resolved extent should inspect");
    let expected_six = inspect_content_stream_data_extent(&source, Some(&xref), content_six_offset)
        .expect("second resolved extent should inspect");
    assert_eq!(
        &source[expected_five.stream_data_start_byte_offset()
            ..expected_five.stream_data_end_byte_offset()],
        b"abc"
    );
    assert_eq!(
        &source[expected_six.stream_data_start_byte_offset()
            ..expected_six.stream_data_end_byte_offset()],
        b"hello"
    );
    assert_eq!(
        report.entries,
        vec![
            PageContentExtentInspection::Located {
                content_reference: contents.contents[0],
                object_byte_offset: content_five_offset,
                extent: expected_five,
            },
            PageContentExtentInspection::Located {
                content_reference: contents.contents[1],
                object_byte_offset: content_six_offset,
                extent: expected_six,
            },
        ]
    );
}

/// Minimal dependency-free serde value tree and adapters for shape round-trip
/// tests, mirroring the focused harness used by the content-stream-extent tests.
mod serde_harness {
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

        fn serialize_element<T: ?Sized + Serialize>(
            &mut self,
            value: &T,
        ) -> Result<(), Self::Error> {
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

        fn deserialize_option<V: de::Visitor<'de>>(
            self,
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
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
}
