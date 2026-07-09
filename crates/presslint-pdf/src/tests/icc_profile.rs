//! Decoded-header parser, bounded stream inspection, truncation, filter and
//! decode-gap, and output-limit tests for the ICC profile descriptor slice.
//!
//! The decoded-header tests exercise [`parse_icc_profile_header`] over crafted
//! 128-byte headers directly; the stream tests build a minimal classic-xref PDF
//! whose object 1 is an ICC profile stream and drive
//! [`inspect_icc_profile_stream_with_lookup`] through the composed
//! resolve/extent/slice/filter/decode/parse path.

use crate::{
    IccProfileHeaderParse, IccProfileInspectionGap, IccProfileStreamInspection, IndirectRef,
    ObjectLookup, encode_flate_stream, inspect_classic_xref_table,
    inspect_icc_profile_stream_with_lookup, parse_icc_profile_header,
};

/// Build a 128-byte ICC header with the supplied fixed fields; other bytes stay
/// zero. When `acsp` is false the file-signature slot holds `junk` instead.
fn icc_header(
    size: u32,
    version: [u8; 4],
    class: &[u8],
    space: &[u8],
    pcs: &[u8],
    acsp: bool,
) -> Vec<u8> {
    let mut header = vec![0u8; 128];
    header[0..4].copy_from_slice(&size.to_be_bytes());
    header[8..12].copy_from_slice(&version);
    header[12..16].copy_from_slice(class);
    header[16..20].copy_from_slice(space);
    header[20..24].copy_from_slice(pcs);
    header[36..40].copy_from_slice(if acsp { b"acsp" } else { b"junk" });
    header
}

/// A conventional valid CMYK print profile header: version 4.4.0, `prtr` class,
/// `CMYK` data space, `Lab ` PCS, valid `acsp`, declared size 128.
fn valid_cmyk_header() -> Vec<u8> {
    icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"CMYK",
        b"Lab ",
        true,
    )
}

/// Build a minimal classic-xref PDF whose object `1 0` is a profile stream with
/// the supplied dictionary suffix and raw stream bytes. Returns the source and
/// the byte offset of its classic xref table.
fn profile_pdf(dict_extra: &str, stream: &[u8]) -> (Vec<u8>, usize) {
    let mut object = format!(
        "1 0 obj\n<< /Length {}{} >>\nstream\n",
        stream.len(),
        dict_extra
    )
    .into_bytes();
    object.extend_from_slice(stream);
    object.extend_from_slice(b"\nendstream\nendobj\n");

    let mut source = b"%PDF-1.7\n".to_vec();
    let object_offset = source.len();
    source.extend_from_slice(&object);

    let xref_offset = source.len();
    source.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
    source.extend_from_slice(format!("{object_offset:010} 00000 n \n").as_bytes());
    source.extend_from_slice(
        format!("trailer\n<< /Size 2 /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n").as_bytes(),
    );
    (source, xref_offset)
}

/// Inspect the profile stream object `1 0` of a `profile_pdf` fixture.
fn inspect_object_one(
    dict_extra: &str,
    stream: &[u8],
    output_limit: usize,
) -> IccProfileStreamInspection {
    inspect_reference(dict_extra, stream, output_limit, 1)
}

/// Inspect an arbitrary object number in a `profile_pdf` fixture, so a missing
/// reference can drive the unresolved-object gap.
fn inspect_reference(
    dict_extra: &str,
    stream: &[u8],
    output_limit: usize,
    object_number: u32,
) -> IccProfileStreamInspection {
    let (source, xref_offset) = profile_pdf(dict_extra, stream);
    let xref = inspect_classic_xref_table(&source, xref_offset).expect("xref table should inspect");
    inspect_icc_profile_stream_with_lookup(
        &source,
        ObjectLookup::ClassicXref(&xref),
        IndirectRef {
            object_number,
            generation: 0,
        },
        output_limit,
    )
}

fn expect_descriptor(inspection: IccProfileStreamInspection) -> crate::IccProfileHeaderDescriptor {
    match inspection {
        IccProfileStreamInspection::Parsed { descriptor } => descriptor,
        other => unreachable!("expected a parsed descriptor, got {other:?}"),
    }
}

#[test]
fn parses_valid_128_byte_header() {
    let IccProfileHeaderParse::Parsed { descriptor } =
        parse_icc_profile_header(&valid_cmyk_header())
    else {
        unreachable!("valid header should parse");
    };

    assert_eq!(descriptor.decoded_len, 128);
    assert_eq!(descriptor.declared_profile_size, 128);
    assert_eq!(descriptor.version_raw, [0x04, 0x40, 0x00, 0x00]);
    assert_eq!(descriptor.version_major, 4);
    assert_eq!(descriptor.version_minor, 4);
    assert_eq!(descriptor.version_bugfix, 0);
    assert_eq!(&descriptor.profile_class_signature, b"prtr");
    assert_eq!(&descriptor.data_color_space_signature, b"CMYK");
    assert_eq!(&descriptor.pcs_signature, b"Lab ");
    assert!(descriptor.acsp_present);
    assert_eq!(descriptor.data_space_component_count(), Some(4));
}

#[test]
fn raw_signatures_preserve_spaces_and_unknown_bytes() {
    let header = icc_header(
        128,
        [0x02, 0x10, 0x00, 0x00],
        b"scnr",
        b"RGB ",
        b"XYZ ",
        true,
    );
    let IccProfileHeaderParse::Parsed { descriptor } = parse_icc_profile_header(&header) else {
        unreachable!("header should parse");
    };

    assert_eq!(&descriptor.data_color_space_signature, b"RGB ");
    assert_eq!(&descriptor.pcs_signature, b"XYZ ");
    // An anomalous BCD version still decodes byte-for-byte.
    assert_eq!(descriptor.version_major, 2);
    assert_eq!(descriptor.version_minor, 1);
    assert_eq!(descriptor.version_bugfix, 0);
    assert_eq!(descriptor.data_space_component_count(), Some(3));

    // An unknown four-byte data space is a fact, not a parse error, and offers
    // no recognized component count.
    let unknown = icc_header(
        128,
        [0x04, 0x00, 0x00, 0x00],
        b"mntr",
        b"HSV ",
        b"XYZ ",
        true,
    );
    let IccProfileHeaderParse::Parsed { descriptor } = parse_icc_profile_header(&unknown) else {
        unreachable!("header should parse");
    };
    assert_eq!(&descriptor.data_color_space_signature, b"HSV ");
    assert_eq!(descriptor.data_space_component_count(), None);
}

#[test]
fn nclr_data_space_maps_hex_digit_to_component_count() {
    for (space, expected) in [
        (b"2CLR", 2usize),
        (b"9CLR", 9),
        (b"ACLR", 10),
        (b"FCLR", 15),
    ] {
        let header = icc_header(128, [0x04, 0x40, 0x00, 0x00], b"spac", space, b"Lab ", true);
        let descriptor = match parse_icc_profile_header(&header) {
            IccProfileHeaderParse::Parsed { descriptor } => descriptor,
            IccProfileHeaderParse::Truncated { .. } => {
                unreachable!("header should parse")
            }
        };
        assert_eq!(descriptor.data_space_component_count(), Some(expected));
    }
}

#[test]
fn payload_shorter_than_header_is_truncated() {
    let short = vec![0u8; 64];
    assert_eq!(
        parse_icc_profile_header(&short),
        IccProfileHeaderParse::Truncated { decoded_len: 64 }
    );
}

#[test]
fn corrupt_acsp_still_yields_descriptor_facts() {
    let header = icc_header(
        128,
        [0x04, 0x40, 0x00, 0x00],
        b"prtr",
        b"CMYK",
        b"Lab ",
        false,
    );
    let IccProfileHeaderParse::Parsed { descriptor } = parse_icc_profile_header(&header) else {
        unreachable!("header should parse even with a corrupt acsp marker");
    };
    assert!(!descriptor.acsp_present);
    assert_eq!(&descriptor.profile_class_signature, b"prtr");
    assert_eq!(&descriptor.data_color_space_signature, b"CMYK");
}

#[test]
fn declared_size_equal_smaller_or_larger_than_decoded_len_is_representable() {
    for declared in [64u32, 128, 4096] {
        let header = icc_header(
            declared,
            [0x04, 0x40, 0x00, 0x00],
            b"prtr",
            b"CMYK",
            b"Lab ",
            true,
        );
        let IccProfileHeaderParse::Parsed { descriptor } = parse_icc_profile_header(&header) else {
            unreachable!("header should parse");
        };
        assert_eq!(descriptor.declared_profile_size, declared);
        assert_eq!(descriptor.decoded_len, 128);
    }
}

#[test]
fn raw_unfiltered_stream_parses() {
    let descriptor = expect_descriptor(inspect_object_one("", &valid_cmyk_header(), 4096));
    assert_eq!(&descriptor.data_color_space_signature, b"CMYK");
    assert!(descriptor.acsp_present);
}

#[test]
fn single_flate_stream_parses() {
    let compressed = encode_flate_stream(&valid_cmyk_header(), 4096).expect("encode");
    let descriptor = expect_descriptor(inspect_object_one(
        " /Filter /FlateDecode",
        &compressed,
        4096,
    ));
    assert_eq!(&descriptor.data_color_space_signature, b"CMYK");
    assert_eq!(descriptor.decoded_len, 128);
}

#[test]
fn unsupported_filter_is_a_gap() {
    let inspection = inspect_object_one(" /Filter /LZWDecode", &valid_cmyk_header(), 4096);
    assert_eq!(
        inspection,
        IccProfileStreamInspection::Gap {
            reason: IccProfileInspectionGap::UnsupportedFilter,
        }
    );
}

#[test]
fn declared_decode_parms_is_a_gap() {
    let compressed = encode_flate_stream(&valid_cmyk_header(), 4096).expect("encode");
    let inspection = inspect_object_one(
        " /Filter /FlateDecode /DecodeParms << /Predictor 1 >>",
        &compressed,
        4096,
    );
    assert_eq!(
        inspection,
        IccProfileStreamInspection::Gap {
            reason: IccProfileInspectionGap::DecodeParmsDeclared,
        }
    );
}

#[test]
fn low_output_limit_is_an_output_limit_gap() {
    let compressed = encode_flate_stream(&valid_cmyk_header(), 4096).expect("encode");
    // The decoded header is 128 bytes; a 64-byte cap forces the bounded inflate
    // to stop with a structured output-limit gap rather than allocating.
    let inspection = inspect_object_one(" /Filter /FlateDecode", &compressed, 64);
    assert_eq!(
        inspection,
        IccProfileStreamInspection::Gap {
            reason: IccProfileInspectionGap::DecodeOutputLimitExceeded,
        }
    );
}

#[test]
fn short_raw_stream_is_truncated() {
    let inspection = inspect_object_one("", &[0u8; 40], 4096);
    assert_eq!(
        inspection,
        IccProfileStreamInspection::Truncated { decoded_len: 40 }
    );
}

#[test]
fn unresolved_reference_is_a_gap() {
    let inspection = inspect_reference("", &valid_cmyk_header(), 4096, 9);
    assert_eq!(
        inspection,
        IccProfileStreamInspection::Gap {
            reason: IccProfileInspectionGap::ProfileObjectUnresolved,
        }
    );
}
