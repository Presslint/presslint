//! Named page-XObject admission for alias-epoch proof and conversion.

use presslint_pdf::{
    DictionaryEntryByteRange, DictionaryValueKind, ImageColorSpaceMetadata, ImageIntegerMetadata,
    ImageMaskMetadata, ImageXObjectMetadata, IndirectRef, PageXObjectResourceTarget,
    PageXObjectResourcesInspection, PdfName as ResourceName, SkippedPageXObjectResource,
    SkippedPageXObjectResourceReason, encode_flate_stream,
};
use presslint_types::PdfName;

use crate::{
    BlackPreservationPolicy, ConvertContentColorsOutput, ConvertContentColorsRequest,
    DeviceLinkInput, PageSelection, convert_content_colors_incremental,
    page_xobject_policy::{PageXObjectEffect, PageXObjectPolicy},
};

use super::content_color_convert::{
    GRAY_TO_GRAY_LINK, assemble_classic, contains, link_bytes, occurrence_count,
    page_decoded_stream, stream_body,
};
use super::{reopen, xref_record};

const CATALOG: &[u8] = b"<< /Type /Catalog /Pages 2 0 R >>";
const PAGES: &[u8] = b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>";
const GRAY_ALIAS: &str = "/ColorSpace << /GrayAlias /DeviceGray >>";
const FLATE_LIMIT: usize = 1 << 20;

fn value(value: u32) -> ImageIntegerMetadata {
    ImageIntegerMetadata::Value { value }
}

fn metadata(
    mask: ImageMaskMetadata,
    width: ImageIntegerMetadata,
    height: ImageIntegerMetadata,
    bits_per_component: ImageIntegerMetadata,
    color_space: ImageColorSpaceMetadata,
) -> ImageXObjectMetadata {
    ImageXObjectMetadata {
        width,
        height,
        bits_per_component,
        color_space,
        image_mask: mask,
    }
}

fn target(name: &[u8], metadata: Option<ImageXObjectMetadata>) -> PageXObjectResourceTarget {
    PageXObjectResourceTarget {
        name: ResourceName(name.to_vec()),
        reference: IndirectRef {
            object_number: 10,
            generation: 0,
        },
        object_byte_offset: 100,
        image_metadata: metadata,
    }
}

fn report(
    image_xobjects: Vec<PageXObjectResourceTarget>,
    form_xobjects: Vec<PageXObjectResourceTarget>,
    skipped: Vec<SkippedPageXObjectResource>,
) -> PageXObjectResourcesInspection {
    PageXObjectResourcesInspection {
        ordinal: 0,
        page_reference: IndirectRef {
            object_number: 3,
            generation: 0,
        },
        page_object_byte_offset: 20,
        image_xobject_names: image_xobjects
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
        form_xobject_names: form_xobjects
            .iter()
            .map(|entry| entry.name.clone())
            .collect(),
        image_xobjects,
        form_xobjects,
        skipped,
    }
}

fn named_skip(name: &[u8]) -> SkippedPageXObjectResource {
    SkippedPageXObjectResource {
        page_object_byte_offset: 20,
        resource_name: Some(ResourceName(name.to_vec())),
        reason: SkippedPageXObjectResourceReason::MissingSubtype {
            object_byte_offset: 100,
        },
    }
}

#[test]
fn policy_classifies_ordinary_stencil_form_and_invalid_image_shapes() {
    let duplicate = ImageMaskMetadata::Duplicate {
        first_key_range: DictionaryEntryByteRange { start: 1, end: 2 },
        duplicate_key_range: DictionaryEntryByteRange { start: 3, end: 4 },
    };
    let images = vec![
        target(
            b"Missing",
            Some(metadata(
                ImageMaskMetadata::Missing,
                ImageIntegerMetadata::Missing,
                ImageIntegerMetadata::Malformed,
                ImageIntegerMetadata::Unsupported {
                    value_kind: DictionaryValueKind::Name,
                },
                ImageColorSpaceMetadata::Unsupported {
                    value_kind: DictionaryValueKind::Array,
                },
            )),
        ),
        target(
            b"False",
            Some(metadata(
                ImageMaskMetadata::False,
                value(1),
                value(1),
                value(8),
                ImageColorSpaceMetadata::DeviceRgb,
            )),
        ),
        target(
            b"StencilDefaultBpc",
            Some(metadata(
                ImageMaskMetadata::True,
                value(2),
                value(3),
                ImageIntegerMetadata::Missing,
                ImageColorSpaceMetadata::Missing,
            )),
        ),
        target(
            b"StencilOneBpc",
            Some(metadata(
                ImageMaskMetadata::True,
                value(2),
                value(3),
                value(1),
                ImageColorSpaceMetadata::Missing,
            )),
        ),
        target(
            b"ZeroWidth",
            Some(metadata(
                ImageMaskMetadata::True,
                value(0),
                value(3),
                value(1),
                ImageColorSpaceMetadata::Missing,
            )),
        ),
        target(
            b"MissingHeight",
            Some(metadata(
                ImageMaskMetadata::True,
                value(2),
                ImageIntegerMetadata::Missing,
                value(1),
                ImageColorSpaceMetadata::Missing,
            )),
        ),
        target(
            b"BadBpc",
            Some(metadata(
                ImageMaskMetadata::True,
                value(2),
                value(3),
                value(8),
                ImageColorSpaceMetadata::Missing,
            )),
        ),
        target(
            b"HasColorSpace",
            Some(metadata(
                ImageMaskMetadata::True,
                value(2),
                value(3),
                value(1),
                ImageColorSpaceMetadata::DeviceGray,
            )),
        ),
        target(
            b"DuplicateMask",
            Some(metadata(
                duplicate,
                value(2),
                value(3),
                value(1),
                ImageColorSpaceMetadata::Missing,
            )),
        ),
        target(
            b"UnsupportedMask",
            Some(metadata(
                ImageMaskMetadata::Unsupported {
                    value_kind: DictionaryValueKind::Name,
                },
                value(2),
                value(3),
                value(1),
                ImageColorSpaceMetadata::Missing,
            )),
        ),
        target(b"NoMetadata", None),
    ];
    let forms = vec![PageXObjectResourceTarget {
        name: ResourceName(b"Form".to_vec()),
        reference: IndirectRef {
            object_number: 11,
            generation: 0,
        },
        object_byte_offset: 110,
        image_metadata: None,
    }];
    let report = report(images, forms, Vec::new());
    let policy = PageXObjectPolicy::new(Some(&report));

    for name in [b"Missing".as_slice(), b"False"] {
        assert_eq!(
            policy.effect_of(&PdfName(name.to_vec())),
            PageXObjectEffect::OrdinaryImage
        );
    }
    for name in [b"StencilDefaultBpc".as_slice(), b"StencilOneBpc"] {
        assert_eq!(
            policy.effect_of(&PdfName(name.to_vec())),
            PageXObjectEffect::Stencil
        );
    }
    assert_eq!(
        policy.effect_of(&PdfName(b"Form".to_vec())),
        PageXObjectEffect::Form
    );
    for name in [
        b"ZeroWidth".as_slice(),
        b"MissingHeight",
        b"BadBpc",
        b"HasColorSpace",
        b"DuplicateMask",
        b"UnsupportedMask",
        b"NoMetadata",
        b"Absent",
    ] {
        assert_eq!(
            policy.effect_of(&PdfName(name.to_vec())),
            PageXObjectEffect::Unknown,
            "{}",
            String::from_utf8_lossy(name)
        );
    }
}

#[test]
fn semantic_name_decoding_collisions_and_skip_poisoning_are_fail_closed() {
    let ordinary = metadata(
        ImageMaskMetadata::Missing,
        value(1),
        value(1),
        value(8),
        ImageColorSpaceMetadata::DeviceRgb,
    );
    let decoded = report(
        vec![target(b"Im#31", Some(ordinary.clone()))],
        vec![],
        vec![],
    );
    let policy = PageXObjectPolicy::new(Some(&decoded));
    assert_eq!(
        policy.effect_of(&PdfName(b"Im1".to_vec())),
        PageXObjectEffect::OrdinaryImage
    );
    assert_eq!(
        policy.effect_of(&PdfName(b"im1".to_vec())),
        PageXObjectEffect::Unknown
    );
    assert_eq!(
        policy.effect_of(&PdfName(b"Im#3G".to_vec())),
        PageXObjectEffect::Unknown
    );

    let collision = report(
        vec![
            target(b"Im1", Some(ordinary.clone())),
            target(b"Im#31", Some(ordinary.clone())),
        ],
        vec![],
        vec![],
    );
    assert_eq!(
        PageXObjectPolicy::new(Some(&collision)).effect_of(&PdfName(b"Im1".to_vec())),
        PageXObjectEffect::Unknown
    );

    let same_name_skip = report(
        vec![target(b"Good", Some(ordinary.clone()))],
        vec![],
        vec![named_skip(b"Go#6Fd"), named_skip(b"Unrelated")],
    );
    let policy = PageXObjectPolicy::new(Some(&same_name_skip));
    assert_eq!(
        policy.effect_of(&PdfName(b"Good".to_vec())),
        PageXObjectEffect::Unknown
    );
    assert_eq!(
        policy.effect_of(&PdfName(b"Unrelated".to_vec())),
        PageXObjectEffect::Unknown
    );

    let unrelated_skip = report(
        vec![target(b"Good", Some(ordinary))],
        vec![],
        vec![named_skip(b"Bad")],
    );
    assert_eq!(
        PageXObjectPolicy::new(Some(&unrelated_skip)).effect_of(&PdfName(b"Good".to_vec())),
        PageXObjectEffect::OrdinaryImage
    );

    let page_skip = SkippedPageXObjectResource {
        page_object_byte_offset: 20,
        resource_name: None,
        reason: SkippedPageXObjectResourceReason::MissingXObject,
    };
    let poisoned = report(
        vec![target(
            b"Good",
            Some(metadata(
                ImageMaskMetadata::Missing,
                value(1),
                value(1),
                value(8),
                ImageColorSpaceMetadata::DeviceRgb,
            )),
        )],
        vec![],
        vec![page_skip],
    );
    assert_eq!(
        PageXObjectPolicy::new(Some(&poisoned)).effect_of(&PdfName(b"Good".to_vec())),
        PageXObjectEffect::Unknown
    );
    assert_eq!(
        PageXObjectPolicy::new(None).effect_of(&PdfName(b"Good".to_vec())),
        PageXObjectEffect::Unknown
    );
}

fn page_body(contents: &str, resources: &str) -> Vec<u8> {
    format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents {contents} /Resources << {resources} >> >>"
    )
    .into_bytes()
}

fn image_body(entries: &str) -> Vec<u8> {
    stream_body(&format!(" /Type /XObject /Subtype /Image {entries}"), b"x")
}

/// A demanded Form whose body uses a resource colour operator (`cs`), so its
/// exact analysis is Unknown and the outer `Do` keeps the fail-closed refusal.
fn form_body() -> Vec<u8> {
    stream_body(
        " /Type /XObject /Subtype /Form /BBox [0 0 1 1]",
        b"/CS0 cs 0 0 m 1 1 l f",
    )
}

fn resource_pdf(content: &[u8], resources: &str, xobjects: Vec<Vec<u8>>) -> Vec<u8> {
    let mut bodies = vec![
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("4 0 R", resources),
        stream_body("", content),
    ];
    bodies.extend(xobjects);
    assemble_classic(&bodies)
}

fn convert(input: &[u8]) -> ConvertContentColorsOutput {
    convert_content_colors_incremental(
        input,
        &ConvertContentColorsRequest {
            pages: PageSelection::All,
            device_links: vec![DeviceLinkInput {
                id: None,
                bytes: link_bytes(GRAY_TO_GRAY_LINK),
            }],
            black_preservation: BlackPreservationPolicy::None,
            target: None,
        },
    )
    .expect("conversion succeeds")
}

#[test]
fn ordinary_images_are_neutral_but_do_not_create_a_consumer() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Im 5 0 R >>");
    let image = image_body("/Width 1 /Height 1 /BitsPerComponent 8 /ImageMask false");

    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Im Do 0 0 m 1 1 l f\n",
        &resources,
        vec![image.clone()],
    );
    let output = convert(&input);
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 2);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert!(!contains(
        &page_decoded_stream(&output.bytes, false),
        b"GrayAlias"
    ));

    let input = resource_pdf(b"/GrayAlias cs 0.5 sc /Im Do\n", &resources, vec![image]);
    let output = convert(&input);
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
}

#[test]
fn stencil_consumes_only_nonstroking_and_save_restore_preserves_the_tuple() {
    let resources = format!("{GRAY_ALIAS} /XObject << /St 5 0 R >>");
    let stencil = image_body("/Width 2 /Height 2 /ImageMask true /BitsPerComponent 1");
    let input = resource_pdf(
        b"/GrayAlias CS 0.6 SC /GrayAlias cs 0.2 sc q 0.8 sc /St Do Q /St Do\n",
        &resources,
        vec![stencil],
    );
    let output = convert(&input);
    let page = &output.converted[0];

    assert_eq!(page.resource_alias_candidates_converted, 3);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    let decoded = page_decoded_stream(&output.bytes, false);
    assert!(contains(&decoded, b"/GrayAlias CS 0.6 SC"));
    assert!(!contains(&decoded, b"/GrayAlias cs"));
    assert!(contains(&decoded, b"q"));
    assert!(contains(&decoded, b"Q"));
}

#[test]
fn structural_constructor_refuses_forms_that_analysis_would_admit() {
    // The structural constructor never analyzes: any Form target classifies as a
    // fail-closed `Form` refusal, even one whose body an analyzer would prove
    // neutral. Only the production `analyzed` constructor folds a proven effect.
    let forms = vec![PageXObjectResourceTarget {
        name: ResourceName(b"Fm".to_vec()),
        reference: IndirectRef {
            object_number: 5,
            generation: 0,
        },
        object_byte_offset: 100,
        image_metadata: None,
    }];
    let report = report(Vec::new(), forms, Vec::new());
    assert_eq!(
        PageXObjectPolicy::new(Some(&report)).effect_of(&PdfName(b"Fm".to_vec())),
        PageXObjectEffect::Form
    );

    // End-to-end, the SAME empty Form is demanded, analyzed neutral, and leaves
    // the alias root live: no consumer, no refusal, alias bytes verbatim.
    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        &resources,
        vec![stream_body(
            " /Type /XObject /Subtype /Form /BBox [0 0 1 1]",
            b"",
        )],
    );
    let output = convert(&input);
    let page = &output.converted[0];
    assert_eq!(page.resource_alias_candidates_converted, 0);
    assert_eq!(page.resource_alias_candidates_refused, 0);
    assert!(contains(
        &page_decoded_stream(&output.bytes, false),
        b"/GrayAlias cs 0.5 sc"
    ));
}

#[test]
fn invalid_stencil_gates_form_and_unknown_targets_refuse() {
    let cases = [
        (
            "/Width 0 /Height 2 /ImageMask true /BitsPerComponent 1",
            "zero width",
        ),
        (
            "/Width 2 /Height 2 /ImageMask true /BitsPerComponent 8",
            "bad bpc",
        ),
        (
            "/Width 2 /Height 2 /ImageMask true /BitsPerComponent 1 /ColorSpace /DeviceGray",
            "colour space",
        ),
        ("/Width 2 /Height 2 /ImageMask /true", "nonboolean mask"),
    ];
    for (entries, label) in cases {
        let resources = format!("{GRAY_ALIAS} /XObject << /Bad 5 0 R >>");
        let input = resource_pdf(
            b"/GrayAlias cs 0.5 sc /Bad Do\n",
            &resources,
            vec![image_body(entries)],
        );
        let output = convert(&input);
        assert_eq!(
            output.converted[0].resource_alias_candidates_refused, 2,
            "{label}"
        );
    }

    let resources = format!("{GRAY_ALIAS} /XObject << /Fm 5 0 R >>");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Fm Do\n",
        &resources,
        vec![form_body()],
    );
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_refused,
        2
    );

    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Missing Do\n",
        &format!("{GRAY_ALIAS} /XObject << /Im 5 0 R >>"),
        vec![image_body("/Width 1 /Height 1")],
    );
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_refused,
        2
    );
}

#[test]
fn only_invoked_bad_names_poison_and_semantic_collisions_refuse() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Good 5 0 R /Bad 6 0 R >>");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Good Do 0 0 m 1 1 l f\n",
        &resources,
        vec![
            image_body("/Width 1 /Height 1 /BitsPerComponent 8"),
            stream_body(" /Type /XObject", b""),
        ],
    );
    let output = convert(&input);
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);

    let resources = format!("{GRAY_ALIAS} /XObject << /Im#31 5 0 R >>");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Im1 Do 0 0 m 1 1 l f\n",
        &resources,
        vec![image_body("/Width 1 /Height 1 /BitsPerComponent 8")],
    );
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_converted,
        2
    );

    let resources = format!("{GRAY_ALIAS} /XObject << /Im1 5 0 R /Im#31 6 0 R >>");
    let image = image_body("/Width 1 /Height 1 /BitsPerComponent 8");
    let input = resource_pdf(
        b"/GrayAlias cs 0.5 sc /Im1 Do\n",
        &resources,
        vec![image.clone(), image],
    );
    assert_eq!(
        convert(&input).converted[0].resource_alias_candidates_refused,
        2
    );
}

#[test]
fn ordinary_do_in_text_or_an_open_path_refuses_before_classification() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Im 5 0 R >>");
    let image = image_body("/Width 1 /Height 1 /BitsPerComponent 8");
    for content in [
        b"/GrayAlias cs 0.5 sc BT /Im Do ET\n".as_slice(),
        b"/GrayAlias cs 0.5 sc 0 0 m /Im Do 1 1 l f\n",
    ] {
        let input = resource_pdf(content, &resources, vec![image.clone()]);
        assert_eq!(
            convert(&input).converted[0].resource_alias_candidates_refused,
            2
        );
    }
}

#[test]
fn inherited_and_page_specific_same_names_join_to_the_exact_page() {
    let bodies = vec![
        CATALOG.to_vec(),
        b"<< /Type /Pages /Kids [3 0 R 5 0 R] /Count 2 /Resources << /ColorSpace << /GrayAlias /DeviceGray >> /XObject << /Im 7 0 R >> >> >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 4 0 R >>".to_vec(),
        stream_body("", b"/GrayAlias cs 0.5 sc /Im Do 0 0 m 1 1 l f\n"),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 6 0 R /Resources << /ColorSpace << /GrayAlias /DeviceGray >> /XObject << /Im 8 0 R >> >> >>".to_vec(),
        stream_body("", b"/GrayAlias cs 0.5 sc /Im Do\n"),
        image_body("/Width 1 /Height 1 /BitsPerComponent 8"),
        image_body("/Width 1 /Height 1 /ImageMask true"),
    ];
    let input = assemble_classic(&bodies);
    let output = convert(&input);

    assert_eq!(output.converted.len(), 2);
    assert!(
        output
            .converted
            .iter()
            .all(|page| page.resource_alias_candidates_converted == 2)
    );
}

#[test]
fn flate_multistream_and_repeated_physical_content_preserve_transaction_rules() {
    let first = b"/GrayAlias cs 0.5 sc /Im Do 0 0 m 1 1 l f\n";
    let compressed = encode_flate_stream(first, FLATE_LIMIT).expect("encode");
    let resources = format!("{GRAY_ALIAS} /XObject << /Im 6 0 R >>");
    let input = assemble_classic(&[
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("[4 0 R 4 0 R 5 0 R]", &resources),
        stream_body(" /Filter /FlateDecode", &compressed),
        stream_body("", b"1 0 0 rg\n"),
        image_body("/Width 1 /Height 1 /BitsPerComponent 8"),
    ]);
    let output = convert(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert_eq!(output.converted[0].operators_converted, 2);
    assert_eq!(occurrence_count(&output.bytes, b"4 0 obj"), 2);
    reopen(&output.bytes);
}

#[test]
fn xref_stream_admission_appends_reopens_and_preserves_the_source_prefix() {
    let resources = format!("{GRAY_ALIAS} /XObject << /Im 5 0 R >>");
    let object_bodies = [
        CATALOG.to_vec(),
        PAGES.to_vec(),
        page_body("4 0 R", &resources),
        stream_body("", b"/GrayAlias cs 0.5 sc /Im Do 0 0 m 1 1 l f\n"),
        image_body("/Width 1 /Height 1 /BitsPerComponent 8"),
    ];
    let mut input = b"%PDF-1.5\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in object_bodies.iter().enumerate() {
        offsets.push(input.len());
        input.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        input.extend_from_slice(body);
        input.extend_from_slice(b"\nendobj\n");
    }
    let xref_offset = input.len();
    let mut xref_body = Vec::new();
    xref_body.extend_from_slice(&xref_record(0, 0, 0));
    for offset in offsets {
        xref_body.extend_from_slice(&xref_record(1, offset, 0));
    }
    xref_body.extend_from_slice(&xref_record(1, xref_offset, 0));
    input.extend_from_slice(
        format!(
            "6 0 obj\n<< /Type /XRef /Size 7 /Index [0 7] /W [1 2 1] /Root 1 0 R /Length {} >>\nstream\n",
            xref_body.len()
        )
        .as_bytes(),
    );
    input.extend_from_slice(&xref_body);
    input.extend_from_slice(b"\nendstream\nendobj\n");
    input.extend_from_slice(format!("startxref\n{xref_offset}\n%%EOF").as_bytes());

    let output = convert(&input);

    assert_eq!(&output.bytes[..input.len()], input.as_slice());
    assert_eq!(output.converted[0].resource_alias_candidates_converted, 2);
    assert!(matches!(
        reopen(&output.bytes).backend,
        presslint_pdf::DocumentAccessBackend::XrefStreamChain { .. }
    ));
}
