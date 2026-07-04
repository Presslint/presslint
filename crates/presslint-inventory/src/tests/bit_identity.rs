//! Golden bit-identity locks for the combined inventory path.
//!
//! The streaming-vs-materialized differential below only catches divergence
//! between two inventory construction paths. It will not catch a uniform
//! regression in refactors that change the shared walker underneath both paths,
//! such as interned state or typed provenance work. These inline value locks
//! pin the exact entry identities before those refactors, so digest movement
//! fails loudly even when both paths still agree with each other.

use presslint_syntax::{assemble_operators, tokenize};
use presslint_types::{ByteRange, ColorSpace, ContentScope, ObjectKind, PageIndex, PdfName};

use crate::{
    ColorSpaceEnv, ColorSpaceResource, GraphicsStateEvent, GraphicsStateWalker, GraphicsWalkError,
    GraphicsWalkErrorKind, Inventory, build_inventory, build_inventory_with_color_space_env,
    inventory_from_graphics_events,
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
            ByteRange { start: 8, end: 14 },
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
    let records = records(input)?;
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
    let records = records(input)?;
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
    events: Result<Vec<GraphicsStateEvent>, GraphicsWalkError>,
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

fn records(input: &[u8]) -> Result<Vec<presslint_syntax::OperatorRecord>, GraphicsWalkError> {
    let tokens = tokenize(input).map_err(|error| {
        GraphicsWalkError::new(
            crate::GraphicsWalkErrorKind::InvalidSourceRange,
            error.range,
        )
    })?;
    let assembled = assemble_operators(&tokens).map_err(|error| {
        let range = match error {
            presslint_syntax::AssembleError::InvalidTokenRange { range, .. }
            | presslint_syntax::AssembleError::TrailingOperands { range, .. }
            | presslint_syntax::AssembleError::UnmatchedArrayClose { range, .. }
            | presslint_syntax::AssembleError::UnmatchedDictionaryClose { range, .. }
            | presslint_syntax::AssembleError::MismatchedDelimiter { range, .. }
            | presslint_syntax::AssembleError::UnterminatedCompositeOperand { range, .. }
            | presslint_syntax::AssembleError::OperatorInsideCompositeOperand { range, .. }
            | presslint_syntax::AssembleError::UnexpectedKeyword { range, .. } => range,
        };
        GraphicsWalkError::new(crate::GraphicsWalkErrorKind::InvalidSourceRange, range)
    })?;
    Ok(assembled.records)
}

fn walk_graphics_state_with_env(
    input: &[u8],
    records: &[presslint_syntax::OperatorRecord],
    color_space_env: ColorSpaceEnv<'_>,
) -> Result<Vec<GraphicsStateEvent>, GraphicsWalkError> {
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
        20, 52, 11, 171, 169, 61, 97, 254, 205, 147, 59, 125, 88, 255, 176, 175, 3, 184, 207, 31,
        162, 236, 246, 52, 56, 25, 226, 180, 83, 9, 158, 191,
    ],
    [
        201, 250, 74, 248, 104, 192, 210, 107, 99, 84, 162, 213, 58, 176, 12, 201, 114, 53, 95, 8,
        16, 126, 72, 226, 188, 28, 89, 226, 38, 238, 91, 103,
    ],
    [
        18, 206, 29, 114, 36, 163, 167, 71, 146, 25, 7, 107, 133, 206, 120, 79, 56, 93, 84, 222,
        95, 1, 252, 181, 120, 127, 179, 185, 90, 82, 68, 33,
    ],
    [
        24, 147, 201, 141, 140, 163, 212, 190, 185, 27, 20, 43, 71, 172, 79, 216, 136, 19, 27, 47,
        206, 78, 124, 79, 134, 78, 161, 194, 241, 105, 46, 40,
    ],
];
const MANY_NOOP_DIGESTS: &[[u8; 32]] = &[
    [
        189, 3, 62, 98, 252, 231, 85, 244, 114, 234, 108, 251, 179, 172, 118, 11, 79, 150, 187,
        184, 136, 201, 209, 80, 157, 80, 27, 29, 176, 6, 179, 24,
    ],
    [
        144, 93, 150, 13, 58, 121, 120, 151, 245, 48, 39, 246, 166, 25, 136, 154, 224, 86, 2, 152,
        195, 111, 171, 128, 66, 49, 107, 127, 92, 250, 63, 113,
    ],
    [
        170, 198, 138, 155, 145, 248, 115, 46, 199, 177, 1, 11, 206, 153, 141, 253, 214, 249, 213,
        206, 133, 183, 212, 46, 235, 55, 25, 0, 189, 19, 117, 61,
    ],
    [
        28, 85, 246, 99, 23, 17, 156, 124, 243, 92, 218, 10, 92, 180, 99, 198, 121, 214, 99, 63,
        87, 70, 105, 78, 61, 8, 107, 1, 40, 242, 10, 145,
    ],
];
const SHARED_DO_DIGESTS: &[[u8; 32]] = &[[
    46, 179, 149, 4, 231, 212, 130, 48, 150, 147, 12, 225, 211, 18, 177, 146, 111, 27, 53, 234,
    214, 37, 157, 62, 29, 166, 72, 49, 146, 33, 97, 181,
]];
const COLOR_SOURCE_DIGESTS: &[[u8; 32]] = &[[
    207, 245, 54, 88, 236, 217, 3, 66, 252, 211, 201, 103, 187, 242, 206, 39, 31, 55, 247, 121,
    250, 21, 49, 215, 103, 194, 87, 118, 186, 0, 176, 132,
]];
const RESOURCE_COLOR_DIGESTS: &[[u8; 32]] = &[[
    68, 214, 118, 223, 104, 255, 235, 218, 239, 59, 200, 132, 254, 91, 199, 159, 22, 98, 138, 247,
    135, 254, 205, 129, 6, 87, 146, 149, 64, 143, 245, 81,
]];
const FORM_SCOPE_DIGESTS: &[[u8; 32]] = &[
    [
        62, 139, 193, 123, 17, 186, 251, 59, 127, 54, 177, 88, 76, 25, 67, 115, 79, 159, 214, 106,
        127, 221, 215, 46, 240, 226, 120, 200, 154, 148, 59, 24,
    ],
    [
        166, 182, 34, 61, 114, 15, 79, 232, 188, 156, 57, 159, 245, 43, 186, 207, 24, 178, 105,
        204, 72, 135, 193, 171, 123, 9, 116, 210, 224, 220, 46, 218,
    ],
];
