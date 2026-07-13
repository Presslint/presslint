//! Golden bit-identity locks for the combined inventory path.
//!
//! The streaming-vs-materialized differential below only catches divergence
//! between two inventory construction paths. It will not catch a uniform
//! regression in refactors that change the shared walker underneath both paths,
//! such as interned state or typed provenance work. These inline value locks
//! pin the exact entry identities before those refactors, so digest movement
//! fails loudly even when both paths still agree with each other.

use std::rc::Rc;

use presslint_types::{
    ByteRange, ColorSpace, ContentScope, EditCapability, ObjectKind, PageIndex, PdfName,
};

use crate::{
    ColorSpaceEnv, ColorSpaceResource, DecodedRange, ExtGStateEnv, FontSelectionState,
    GraphicsColor, GraphicsStateSnapshot, GraphicsStateWalker, GraphicsWalkError,
    GraphicsWalkErrorKind, Inventory, PaintOp, TextRenderingMode, build_inventory,
    build_inventory_with_color_space_env, build_inventory_with_initial_state_and_envs,
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
    assert_eq!(inventory.entries[0].id.digest, MIXED_DIGESTS[0]);
    assert_eq!(inventory.entries[1].id.digest, MIXED_DIGESTS[1]);
    assert_eq!(inventory.entries[3].id.digest, MIXED_DIGESTS[3]);
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
    assert_eq!(inventory.entries[0].id.digest, MANY_NOOP_DIGESTS[0]);
    assert_eq!(inventory.entries[2].id.digest, MANY_NOOP_DIGESTS[2]);
    assert_eq!(inventory.entries[3].id.digest, MANY_NOOP_DIGESTS[3]);
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
    assert_eq!(inventory.entries[0].id.digest, FORM_SCOPE_DIGESTS[0]);
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

/// A seed where every tracked snapshot field differs from `page_default()`.
///
/// The seeded tests below cover ordinary Form caller-state inheritance only:
/// Form `/Matrix` concatenation, `/BBox` clipping, and transparency-group entry
/// resets are NOT applied by the seeded builder and are not tested here.
fn seeded_snapshot() -> GraphicsStateSnapshot {
    GraphicsStateSnapshot {
        ctm: [2.0, 0.0, 0.0, 2.0, 7.0, 7.0],
        stroking_color: GraphicsColor::new(ColorSpace::DeviceRgb, vec![1.0, 0.0, 0.0]),
        nonstroking_color: GraphicsColor::new(ColorSpace::DeviceCmyk, vec![0.0, 0.0, 0.0, 1.0]),
        text_rendering_mode: TextRenderingMode::FillThenStroke,
        font_selection: FontSelectionState::Selected {
            name: name(b"F7"),
            size: 7.0,
        },
        extgstate: crate::GraphicsExtGStateSnapshot::page_default(),
    }
}

fn sourced_seeded_snapshot() -> GraphicsStateSnapshot {
    let mut state = seeded_snapshot();
    state.nonstroking_color.source = Some(DecodedRange::new(ByteRange {
        start: 100,
        end: 109,
    }));
    state
}

#[test]
fn seeded_builder_with_page_default_and_empty_envs_matches_legacy_builders()
-> Result<(), GraphicsWalkError> {
    // Page-default/empty-environment equivalence: the seeded builder reduces
    // byte-for-byte to the legacy builders, so existing digest locks above stay
    // authoritative for the default root path.
    let im1 = [name(b"Im1")];
    let fm1 = [name(b"Fm1")];
    for input in [
        b"0.4 g f /Im1 Do (Hi) Tj /Fm1 Do".as_slice(),
        b"q 1 0 0 1 5 5 cm 0.4 g f BT /F1 12 Tf (Hi) Tj ET /GS1 gs f Q".as_slice(),
    ] {
        let records = super::assembled_records(input)?;
        let legacy = build_inventory(input, &records, PAGE, &ContentScope::Page, &im1, &fm1)?;
        let seeded = build_inventory_with_initial_state_and_envs(
            input,
            &records,
            PAGE,
            &ContentScope::Page,
            &im1,
            &fm1,
            Rc::new(GraphicsStateSnapshot::page_default()),
            ColorSpaceEnv::empty(),
            ExtGStateEnv::empty(),
        )?;
        assert_eq!(legacy, seeded);
    }
    Ok(())
}

#[test]
fn seeded_streaming_matches_seeded_materialized_walk() -> Result<(), GraphicsWalkError> {
    // The form paints BEFORE any local colour operator (`f` under the inherited
    // seed), shows text under the inherited font/rendering mode, then sets a
    // local colour: entries, observations, capabilities, and text identity
    // inputs must agree between the streaming seeded builder and a materialized
    // walk from the SAME source-free seed and environments. A sourced seed is
    // covered separately because only the seeded builder receives the seed
    // needed to withhold caller-owned rewrite provenance.
    let input: &[u8] = b"f (Hi) Tj 0.5 g f";
    let scope = ContentScope::FormXObject {
        name: name(b"FmSeed"),
    };
    let records = super::assembled_records(input)?;
    let streamed = build_inventory_with_initial_state_and_envs(
        input,
        &records,
        PAGE,
        &scope,
        &[],
        &[],
        Rc::new(seeded_snapshot()),
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
    )?;

    let mut walker = GraphicsStateWalker::with_initial_state_and_envs(
        Rc::new(seeded_snapshot()),
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
    );
    let events: Vec<PaintOp> = records
        .iter()
        .enumerate()
        .map(|(index, record)| walker.step(input, index, record))
        .collect::<Result<_, _>>()?;
    let materialized = inventory_from_graphics_events(PAGE, &scope, &events, &[], &[]);
    assert_eq!(streamed, materialized);

    // The inherited seed is observable: the first fill reports the seeded CMYK
    // colour; the FillThenStroke text show reports BOTH seeded colours; the
    // later local `0.5 g` fill reports grey. Identity stays deterministic.
    assert_eq!(streamed.entries[0].colors.len(), 1);
    assert_eq!(streamed.entries[0].colors[0].space, ColorSpace::DeviceCmyk);
    assert_eq!(
        streamed.entries[0].colors[0].components,
        vec![0.0, 0.0, 0.0, 1.0]
    );
    assert_eq!(streamed.entries[1].kind, ObjectKind::Text);
    assert_eq!(streamed.entries[1].colors.len(), 2);
    assert_eq!(streamed.entries[1].colors[0].space, ColorSpace::DeviceRgb);
    assert_eq!(streamed.entries[2].colors[0].space, ColorSpace::DeviceGray);
    let replay = build_inventory_with_initial_state_and_envs(
        input,
        &records,
        PAGE,
        &scope,
        &[],
        &[],
        Rc::new(seeded_snapshot()),
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
    )?;
    assert_eq!(streamed, replay);
    Ok(())
}

#[test]
fn seeded_inherited_source_withholds_only_rewrite_until_local_color_reset()
-> Result<(), GraphicsWalkError> {
    // The first vector and text entry inherit a sourced caller colour. The bare
    // range has no owning-stream identity, so it is observation/digest evidence
    // but cannot authorize a Form-local rewrite. Text spread remains unrelated
    // and available. A local `g` establishes callee-owned provenance and
    // restores the existing rewrite capability for later vector and text paint.
    let input: &[u8] = b"f (Inherited) Tj 0.5 g f (Local) Tj";
    let records = super::assembled_records(input)?;
    let inventory = build_inventory_with_initial_state_and_envs(
        input,
        &records,
        PAGE,
        &ContentScope::FormXObject {
            name: name(b"FmSeed"),
        },
        &[],
        &[],
        Rc::new(sourced_seeded_snapshot()),
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
    )?;

    assert_eq!(inventory.entries.len(), 4);
    assert_eq!(inventory.entries[0].kind, ObjectKind::Vector);
    assert!(inventory.entries[0].colors[0].source.is_some());
    assert!(
        !inventory.entries[0]
            .capabilities
            .contains(&EditCapability::RewriteColorOperand)
    );
    assert_eq!(inventory.entries[1].kind, ObjectKind::Text);
    assert_eq!(
        inventory.entries[1].capabilities,
        vec![EditCapability::AddTextSpreadStroke]
    );

    for entry in &inventory.entries[2..] {
        assert!(
            entry
                .capabilities
                .contains(&EditCapability::RewriteColorOperand)
        );
    }
    assert_eq!(
        inventory.entries[3].capabilities,
        vec![
            EditCapability::RewriteColorOperand,
            EditCapability::AddTextSpreadStroke,
        ]
    );
    Ok(())
}

#[test]
fn seeded_builder_surfaces_identical_error_to_materialized_walk() -> Result<(), GraphicsWalkError> {
    // Errors agree too: the same malformed record short-circuits both paths
    // with the same structured error under a non-default seed.
    let input: &[u8] = b"f 1 2 RG";
    let records = super::assembled_records(input)?;
    let streamed = build_inventory_with_initial_state_and_envs(
        input,
        &records,
        PAGE,
        &ContentScope::Page,
        &[],
        &[],
        Rc::new(seeded_snapshot()),
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
    );
    let mut walker = GraphicsStateWalker::with_initial_state_and_envs(
        Rc::new(seeded_snapshot()),
        ColorSpaceEnv::empty(),
        ExtGStateEnv::empty(),
    );
    let materialized: Result<Vec<PaintOp>, GraphicsWalkError> = records
        .iter()
        .enumerate()
        .map(|(index, record)| walker.step(input, index, record))
        .collect();
    assert!(streamed.is_err());
    assert_eq!(streamed.err(), materialized.err());
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
        97, 200, 184, 89, 218, 165, 14, 19, 64, 187, 124, 19, 61, 98, 174, 150, 32, 18, 69, 169,
        47, 242, 77, 14, 154, 174, 137, 189, 155, 232, 211, 237,
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
        247, 118, 85, 37, 207, 163, 252, 251, 133, 108, 193, 215, 149, 158, 78, 185, 69, 222, 197,
        103, 3, 170, 124, 194, 192, 83, 19, 17, 112, 190, 119, 200,
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
        204, 124, 230, 97, 253, 67, 138, 89, 174, 44, 177, 27, 92, 61, 156, 249, 68, 103, 145, 240,
        150, 169, 172, 132, 252, 84, 170, 249, 125, 147, 180, 34,
    ],
];
