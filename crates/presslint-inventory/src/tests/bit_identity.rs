//! Golden bit-identity locks for the combined inventory path.
//!
//! The streaming-vs-materialized differential below only catches divergence
//! between two inventory construction paths. It will not catch a uniform
//! regression in refactors that change the shared walker underneath both paths,
//! such as interned state or typed provenance work. These inline value locks
//! pin the exact entry identities before those refactors, so digest movement
//! fails loudly even when both paths still agree with each other.

use presslint_types::{ByteRange, ColorSpace, ContentScope, ObjectKind, PageIndex, PdfName};

use crate::{
    ColorSpaceEnv, ColorSpaceResource, DecodedRange, GraphicsStateWalker, GraphicsWalkError,
    GraphicsWalkErrorKind, Inventory, PaintOp, build_inventory,
    build_inventory_with_color_space_env, inventory_from_graphics_events,
};

const PAGE: PageIndex = PageIndex(2);

#[test]
fn corpus_inventory_identity_is_golden_locked() -> Result<(), GraphicsWalkError> {
    let page_scope = ContentScope::Page;
    let form_scope = ContentScope::FormXObject {
        name: name(b"FmNested"),
    };
    let im1 = [name(b"Im1")];
    let fm1 = [name(b"Fm1")];
    let dup = [name(b"Dup")];

    let inventory = assert_streaming_equals_materialized(
        b"0.4 g f /Im1 Do (Hi) Tj /Fm1 Do",
        PAGE,
        &page_scope,
        &im1,
        &fm1,
    )?;
    assert_golden(
        "mixed vector/text/image/form",
        &inventory,
        &[
            ObjectKind::Vector,
            ObjectKind::Image,
            ObjectKind::Text,
            ObjectKind::FormXObject,
        ],
        MIXED_DIGESTS,
    );

    let inventory = assert_streaming_equals_materialized(
        b"q 1 0 0 1 5 5 cm 10 20 m 30 40 l 50 60 l h W n /GS1 gs \
          0.4 g f BT /F1 12 Tf (Hi) Tj ET /Im1 Do 5 5 10 10 re n /Fm1 Do Q",
        PAGE,
        &page_scope,
        &im1,
        &fm1,
    )?;
    assert_golden(
        "many no-op few entry",
        &inventory,
        &[
            ObjectKind::Vector,
            ObjectKind::Text,
            ObjectKind::Image,
            ObjectKind::FormXObject,
        ],
        MANY_NOOP_DIGESTS,
    );

    let inventory =
        assert_streaming_equals_materialized(b"/Dup Do", PAGE, &page_scope, &dup, &dup)?;
    assert_golden(
        "shared Do name image wins",
        &inventory,
        &[ObjectKind::Image],
        SHARED_DO_DIGESTS,
    );

    let inventory =
        assert_streaming_equals_materialized(b"0 0 0 rg f", PAGE, &page_scope, &[], &[])?;
    assert_golden(
        "colour source provenance changes digest",
        &inventory,
        &[ObjectKind::Vector],
        COLOR_SOURCE_DIGESTS,
    );

    let inventory =
        assert_streaming_equals_materialized(b"0.2 g f (Inner) Tj", PAGE, &form_scope, &[], &[])?;
    assert_golden(
        "form scope",
        &inventory,
        &[ObjectKind::Vector, ObjectKind::Text],
        FORM_SCOPE_DIGESTS,
    );
    Ok(())
}

#[test]
fn resource_colour_identity_is_golden_locked() -> Result<(), GraphicsWalkError> {
    let resources = [ColorSpaceResource {
        name: name(b"CS0"),
        space: ColorSpace::Separation,
        component_count: Some(1),
        spot_names: vec![name(b"Spot")],
    }];
    let input = b"/CS0 cs 0.5 scn 0 0 1 1 re f";
    let inventory = assert_streaming_equals_materialized_with_env(
        input,
        PAGE,
        &ContentScope::Page,
        &[],
        &[],
        ColorSpaceEnv::new(&resources),
    )?;
    assert_golden(
        "resource colour via cs/scn",
        &inventory,
        &[ObjectKind::Vector],
        RESOURCE_COLOR_DIGESTS,
    );
    Ok(())
}

#[test]
fn malformed_after_last_entry_surfaces_identical_error() -> Result<(), String> {
    let input = b"0.4 g f 1 2 RG";
    let result = assert_streaming_equals_materialized(input, PAGE, &ContentScope::Page, &[], &[]);
    let Err(error) = result else {
        return Err("malformed record after last entry should fail".to_string());
    };

    assert_eq!(
        error,
        GraphicsWalkError::new(
            GraphicsWalkErrorKind::MalformedOperandCount {
                operator: b"RG".to_vec(),
                expected: 3,
                got: 2,
            },
            DecodedRange::new(ByteRange { start: 8, end: 14 }),
        )
    );
    Ok(())
}

pub(super) fn assert_streaming_equals_materialized(
    input: &[u8],
    page: PageIndex,
    scope: &ContentScope,
    images: &[PdfName],
    forms: &[PdfName],
) -> Result<Inventory, GraphicsWalkError> {
    let records = super::assembled_records(input)?;
    let streamed = build_inventory(input, &records, page, scope, images, forms);
    let events = walk_graphics_state_with_env(input, &records, ColorSpaceEnv::empty());
    compare_streaming_and_materialized(streamed, events, page, scope, images, forms)
}

fn assert_streaming_equals_materialized_with_env(
    input: &[u8],
    page: PageIndex,
    scope: &ContentScope,
    images: &[PdfName],
    forms: &[PdfName],
    color_space_env: ColorSpaceEnv<'_>,
) -> Result<Inventory, GraphicsWalkError> {
    let records = super::assembled_records(input)?;
    let streamed = build_inventory_with_color_space_env(
        input,
        &records,
        page,
        scope,
        images,
        forms,
        color_space_env,
    );
    let events = walk_graphics_state_with_env(input, &records, color_space_env);
    compare_streaming_and_materialized(streamed, events, page, scope, images, forms)
}

fn compare_streaming_and_materialized(
    streamed: Result<Inventory, GraphicsWalkError>,
    events: Result<Vec<PaintOp>, GraphicsWalkError>,
    page: PageIndex,
    scope: &ContentScope,
    images: &[PdfName],
    forms: &[PdfName],
) -> Result<Inventory, GraphicsWalkError> {
    match (streamed, events) {
        (Ok(streamed), Ok(events)) => {
            let materialized = inventory_from_graphics_events(page, scope, &events, images, forms);
            assert_eq!(streamed, materialized);
            Ok(streamed)
        }
        (Err(streamed_err), Err(events_err)) => {
            assert_eq!(streamed_err, events_err);
            Err(streamed_err)
        }
        (Ok(_), Err(events_err)) => {
            let streamed_marker: Result<(), GraphicsWalkError> = Ok(());
            let materialized_marker = Err(events_err.clone());
            assert_eq!(
                streamed_marker, materialized_marker,
                "streaming succeeded but materialized failed"
            );
            Err(events_err)
        }
        (Err(streamed_err), Ok(_)) => {
            let streamed_marker = Err(streamed_err.clone());
            let materialized_marker: Result<(), GraphicsWalkError> = Ok(());
            assert_eq!(
                streamed_marker, materialized_marker,
                "streaming failed but materialized succeeded"
            );
            Err(streamed_err)
        }
    }
}

fn walk_graphics_state_with_env(
    input: &[u8],
    records: &[presslint_syntax::OperatorRecord],
    color_space_env: ColorSpaceEnv<'_>,
) -> Result<Vec<PaintOp>, GraphicsWalkError> {
    let mut walker = GraphicsStateWalker::with_color_space_env(color_space_env);
    records
        .iter()
        .enumerate()
        .map(|(index, record)| walker.step(input, index, record))
        .collect()
}

fn assert_golden(
    name: &str,
    inventory: &Inventory,
    expected_kinds: &[ObjectKind],
    expected_digests: &[[u8; 32]],
) {
    let kinds: Vec<ObjectKind> = inventory.entries.iter().map(|entry| entry.kind).collect();
    let digests: Vec<[u8; 32]> = inventory
        .entries
        .iter()
        .map(|entry| entry.id.digest)
        .collect();

    assert_eq!(inventory.len(), expected_kinds.len(), "{name}: entry count");
    assert_eq!(kinds, expected_kinds, "{name}: kinds");
    assert_eq!(digests, expected_digests, "{name}: digests");
}

fn name(bytes: &[u8]) -> PdfName {
    PdfName(bytes.to_vec())
}

const MIXED_DIGESTS: &[[u8; 32]] = &[
    [
        65, 212, 137, 105, 142, 115, 179, 168, 251, 211, 86, 137, 110, 162, 128, 150, 252, 103, 24,
        97, 166, 252, 214, 102, 71, 98, 113, 97, 146, 69, 199, 115,
    ],
    [
        46, 161, 20, 74, 76, 62, 180, 192, 121, 155, 151, 163, 160, 34, 247, 150, 159, 207, 209,
        164, 229, 34, 149, 70, 250, 22, 233, 237, 66, 136, 65, 10,
    ],
    [
        6, 162, 104, 144, 149, 3, 197, 92, 64, 199, 236, 249, 20, 144, 136, 180, 128, 149, 117,
        183, 7, 124, 3, 61, 73, 147, 232, 224, 148, 244, 67, 178,
    ],
    [
        93, 241, 210, 114, 9, 98, 211, 91, 101, 128, 45, 107, 246, 216, 90, 115, 131, 79, 38, 60,
        194, 29, 138, 173, 179, 198, 227, 61, 133, 250, 197, 226,
    ],
];
const MANY_NOOP_DIGESTS: &[[u8; 32]] = &[
    [
        44, 197, 56, 4, 32, 32, 135, 32, 70, 147, 43, 188, 146, 230, 38, 92, 89, 240, 72, 158, 84,
        158, 22, 242, 124, 1, 25, 138, 141, 223, 5, 172,
    ],
    [
        11, 157, 32, 98, 220, 33, 217, 23, 63, 173, 9, 115, 172, 2, 18, 136, 75, 74, 54, 56, 183,
        57, 53, 43, 167, 226, 230, 249, 255, 77, 193, 107,
    ],
    [
        105, 214, 162, 255, 212, 119, 40, 215, 132, 176, 10, 146, 214, 11, 196, 1, 130, 217, 86,
        37, 142, 8, 36, 173, 150, 151, 209, 185, 50, 132, 167, 133,
    ],
    [
        203, 130, 137, 252, 52, 211, 20, 210, 83, 39, 104, 43, 4, 147, 246, 32, 43, 191, 110, 191,
        145, 220, 227, 233, 200, 28, 84, 2, 125, 53, 41, 181,
    ],
];
const SHARED_DO_DIGESTS: &[[u8; 32]] = &[[
    57, 145, 67, 168, 246, 114, 159, 252, 248, 106, 100, 207, 119, 183, 144, 105, 98, 9, 143, 225,
    109, 59, 145, 154, 205, 117, 72, 53, 154, 234, 42, 177,
]];
const COLOR_SOURCE_DIGESTS: &[[u8; 32]] = &[[
    235, 207, 124, 201, 8, 225, 143, 207, 58, 140, 193, 107, 66, 115, 136, 13, 184, 5, 49, 111,
    216, 164, 20, 152, 1, 25, 215, 217, 126, 105, 125, 196,
]];
const RESOURCE_COLOR_DIGESTS: &[[u8; 32]] = &[[
    236, 169, 46, 155, 83, 247, 19, 67, 122, 5, 124, 93, 84, 107, 155, 82, 160, 7, 235, 186, 104,
    101, 12, 20, 51, 67, 176, 193, 141, 12, 107, 160,
]];
const FORM_SCOPE_DIGESTS: &[[u8; 32]] = &[
    [
        44, 41, 242, 9, 238, 88, 159, 200, 127, 72, 200, 125, 231, 23, 225, 163, 133, 123, 125,
        143, 188, 121, 99, 108, 157, 175, 101, 170, 147, 207, 73, 225,
    ],
    [
        205, 51, 184, 242, 68, 55, 175, 105, 237, 17, 57, 230, 81, 40, 173, 44, 204, 179, 106, 200,
        45, 28, 133, 215, 247, 126, 225, 210, 52, 27, 94, 61,
    ],
];
