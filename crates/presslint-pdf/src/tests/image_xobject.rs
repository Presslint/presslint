use crate::{
    ImageColorSpaceMetadata, ImageIntegerMetadata, ImageXObjectMetadata,
    inspect_image_xobject_metadata, inspect_indirect_object_dictionary,
};

/// Inspect the structural image metadata of a synthetic image `XObject` whose
/// dictionary bytes are supplied verbatim. The object is placed after a short
/// header so the dictionary does not begin at offset zero.
fn image_metadata(dictionary: &[u8]) -> ImageXObjectMetadata {
    let mut source = b"%PDF-1.7\n".to_vec();
    let object_offset = source.len();
    source.extend_from_slice(dictionary);
    source.extend_from_slice(b"\nstream\nx\nendstream\nendobj\n");

    let dictionary = inspect_indirect_object_dictionary(&source, object_offset)
        .expect("image object dictionary should inspect");
    inspect_image_xobject_metadata(&source, &dictionary.entries)
}

#[test]
fn direct_device_gray_metadata_is_mapped() {
    let metadata = image_metadata(
        b"1 0 obj\n<< /Type /XObject /Subtype /Image /Width 100 /Height 50 /BitsPerComponent 8 /ColorSpace /DeviceGray >>",
    );

    assert_eq!(
        metadata,
        ImageXObjectMetadata {
            width: ImageIntegerMetadata::Value { value: 100 },
            height: ImageIntegerMetadata::Value { value: 50 },
            bits_per_component: ImageIntegerMetadata::Value { value: 8 },
            color_space: ImageColorSpaceMetadata::DeviceGray,
        }
    );
}

#[test]
fn direct_device_rgb_and_cmyk_names_are_mapped() {
    let rgb = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceRGB >>",
    );
    assert_eq!(rgb.color_space, ImageColorSpaceMetadata::DeviceRgb);

    let cmyk = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceCMYK >>",
    );
    assert_eq!(cmyk.color_space, ImageColorSpaceMetadata::DeviceCmyk);
}

#[test]
fn absent_entries_are_reported_missing() {
    let metadata = image_metadata(b"1 0 obj\n<< /Type /XObject /Subtype /Image >>");

    assert_eq!(
        metadata,
        ImageXObjectMetadata {
            width: ImageIntegerMetadata::Missing,
            height: ImageIntegerMetadata::Missing,
            bits_per_component: ImageIntegerMetadata::Missing,
            color_space: ImageColorSpaceMetadata::Missing,
        }
    );
}

#[test]
fn non_device_colorspace_name_stays_explicit() {
    let metadata = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /Cs1 >>",
    );

    assert_eq!(
        metadata.color_space,
        ImageColorSpaceMetadata::OtherName {
            name: b"/Cs1".to_vec(),
        }
    );
}

#[test]
fn array_colorspace_is_unsupported_not_guessed() {
    let metadata = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace [ /ICCBased 9 0 R ] >>",
    );

    assert_eq!(
        metadata.color_space,
        ImageColorSpaceMetadata::Unsupported {
            value_kind: crate::DictionaryValueKind::Array,
        }
    );
}

#[test]
fn indirect_colorspace_is_unsupported_not_resolved() {
    let metadata = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace 9 0 R >>",
    );

    assert_eq!(
        metadata.color_space,
        ImageColorSpaceMetadata::Unsupported {
            value_kind: crate::DictionaryValueKind::IndirectReferenceLike,
        }
    );
}

#[test]
fn non_integer_and_signed_dimensions_are_explicit() {
    let real_width = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2.5 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray >>",
    );
    assert_eq!(real_width.width, ImageIntegerMetadata::Malformed);

    let negative_height = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2 /Height -3 /BitsPerComponent 8 /ColorSpace /DeviceGray >>",
    );
    assert_eq!(negative_height.height, ImageIntegerMetadata::Malformed);

    let name_bpc = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2 /Height 2 /BitsPerComponent /Eight /ColorSpace /DeviceGray >>",
    );
    assert_eq!(
        name_bpc.bits_per_component,
        ImageIntegerMetadata::Unsupported {
            value_kind: crate::DictionaryValueKind::Name,
        }
    );
}

#[test]
fn duplicate_entries_are_reported_explicit() {
    let metadata = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2 /Width 3 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray /ColorSpace /DeviceRGB >>",
    );

    assert!(matches!(
        metadata.width,
        ImageIntegerMetadata::Duplicate { .. }
    ));
    assert!(matches!(
        metadata.color_space,
        ImageColorSpaceMetadata::Duplicate { .. }
    ));
}

#[test]
fn metadata_retains_no_source_stream_bytes() {
    let metadata = image_metadata(
        b"1 0 obj\n<< /Subtype /Image /Width 2 /Height 2 /BitsPerComponent 8 /ColorSpace /DeviceGray /Secret (do-not-copy) >>",
    );

    let debug = format!("{metadata:?}");
    assert!(!debug.contains("do-not-copy"));
    assert!(!debug.contains("Secret"));
}
