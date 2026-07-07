use std::fmt;

use presslint_types::{
    BoundingBox, ByteRange, ColorObservation, ColorSpace, ColorUsage, ContentScope, EditCapability,
    InvocationFrame, InvocationPath, ObjectId, ObjectKind, PageIndex, PdfName, Provenance,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::Inventory;
use super::json::{Json, JsonError, JsonSerializer};
use crate::InventoryEntry;

fn assert_json_round_trip<T>(value: &T, expected: Json) -> Result<(), JsonError>
where
    T: Serialize + DeserializeOwned + PartialEq + fmt::Debug,
{
    let encoded = value.serialize(JsonSerializer)?;
    assert_eq!(encoded, expected);

    let decoded = T::deserialize(expected)?;
    assert_eq!(&decoded, value);
    Ok(())
}

#[test]
fn inventory_entry_has_stable_json_shape() -> Result<(), JsonError> {
    assert_json_round_trip(&bounded_vector_entry(), bounded_vector_entry_json())
}

#[test]
fn inventory_has_stable_json_shape() -> Result<(), JsonError> {
    assert_json_round_trip(&inventory_fixture(), inventory_fixture_json())
}

#[test]
fn inventory_entry_with_invocation_has_stable_json_shape() -> Result<(), JsonError> {
    let mut entry = bounded_vector_entry();
    entry.provenance.invocation = Some(InvocationPath {
        frames: vec![InvocationFrame {
            ordinal: 0,
            name: PdfName(b"Fm".to_vec()),
        }],
    });

    assert_json_round_trip(&entry, bounded_vector_entry_with_invocation_json())
}

#[test]
fn old_color_observation_json_without_spot_names_deserializes() -> Result<(), JsonError> {
    let decoded = ColorObservation::deserialize(Json::object([
        ("usage", Json::string("fill")),
        ("space", Json::string("separation")),
        ("components", Json::array([Json::F64(0.5)])),
        ("spot_name", pdf_name_json(b"Spot")),
        ("source", Json::Null),
    ]))?;

    assert_eq!(decoded.usage, ColorUsage::Fill);
    assert_eq!(decoded.space, ColorSpace::Separation);
    assert_eq!(decoded.components, vec![0.5]);
    assert_eq!(decoded.spot_name, Some(PdfName(b"Spot".to_vec())));
    assert!(decoded.spot_names.is_empty());
    assert_eq!(decoded.source, None);
    Ok(())
}

#[test]
fn non_spot_color_observation_omits_spot_names() -> Result<(), JsonError> {
    let observation = ColorObservation {
        usage: ColorUsage::Image,
        space: ColorSpace::DeviceCmyk,
        components: vec![0.0, 0.0, 0.0, 1.0],
        spot_name: None,
        spot_names: Vec::new(),
        source: None,
    };

    assert_json_round_trip(
        &observation,
        Json::object([
            ("usage", Json::string("image")),
            ("space", Json::string("device_cmyk")),
            (
                "components",
                Json::array([
                    Json::F64(0.0),
                    Json::F64(0.0),
                    Json::F64(0.0),
                    Json::F64(1.0),
                ]),
            ),
            ("spot_name", Json::Null),
            ("source", Json::Null),
        ]),
    )
}

#[test]
fn spot_color_observations_serialize_full_spot_names_when_present() -> Result<(), JsonError> {
    assert_json_round_trip(
        &ColorObservation {
            usage: ColorUsage::Fill,
            space: ColorSpace::Separation,
            components: vec![0.5],
            spot_name: Some(PdfName(b"Spot".to_vec())),
            spot_names: vec![PdfName(b"Spot".to_vec())],
            source: None,
        },
        Json::object([
            ("usage", Json::string("fill")),
            ("space", Json::string("separation")),
            ("components", Json::array([Json::F64(0.5)])),
            ("spot_name", pdf_name_json(b"Spot")),
            ("spot_names", Json::array([pdf_name_json(b"Spot")])),
            ("source", Json::Null),
        ]),
    )?;

    assert_json_round_trip(
        &ColorObservation {
            usage: ColorUsage::Fill,
            space: ColorSpace::DeviceN,
            components: vec![0.2, 0.8],
            spot_name: Some(PdfName(b"Cut".to_vec())),
            spot_names: vec![PdfName(b"Cut".to_vec()), PdfName(b"Varnish".to_vec())],
            source: None,
        },
        Json::object([
            ("usage", Json::string("fill")),
            ("space", Json::string("device_n")),
            ("components", Json::array([Json::F64(0.2), Json::F64(0.8)])),
            ("spot_name", pdf_name_json(b"Cut")),
            (
                "spot_names",
                Json::array([pdf_name_json(b"Cut"), pdf_name_json(b"Varnish")]),
            ),
            ("source", Json::Null),
        ]),
    )
}

fn inventory_fixture() -> Inventory {
    Inventory {
        entries: vec![
            bounded_vector_entry(),
            sourced_text_entry(),
            read_only_form_entry(),
        ],
    }
}

fn bounded_vector_entry() -> InventoryEntry {
    InventoryEntry {
        id: object_id(1, 0),
        kind: ObjectKind::Vector,
        provenance: Provenance {
            page: PageIndex(1),
            scope: ContentScope::Page,
            range: Some(ByteRange { start: 20, end: 31 }),
            invocation: None,
        },
        bounds: Some(BoundingBox {
            x_min: 10.25,
            y_min: 20.5,
            x_max: 110.75,
            y_max: 220.125,
        }),
        colors: vec![ColorObservation {
            usage: ColorUsage::Stroke,
            space: ColorSpace::DeviceCmyk,
            components: vec![0.1, 0.2, 0.3, 0.4],
            spot_name: None,
            spot_names: Vec::new(),
            source: Some(ByteRange { start: 3, end: 18 }),
        }],
        capabilities: vec![
            EditCapability::RewriteColorOperand,
            EditCapability::AdjustStrokeWidth,
        ],
    }
}

fn sourced_text_entry() -> InventoryEntry {
    InventoryEntry {
        id: object_id(1, 1),
        kind: ObjectKind::Text,
        provenance: Provenance {
            page: PageIndex(1),
            scope: ContentScope::FormXObject {
                name: PdfName(b"FmText".to_vec()),
            },
            range: Some(ByteRange { start: 40, end: 52 }),
            invocation: None,
        },
        bounds: None,
        colors: vec![
            ColorObservation {
                usage: ColorUsage::Fill,
                space: ColorSpace::Resource(PdfName(b"BrandSpot".to_vec())),
                components: vec![0.65],
                spot_name: Some(PdfName(b"BrandSpot".to_vec())),
                spot_names: Vec::new(),
                source: Some(ByteRange { start: 32, end: 39 }),
            },
            ColorObservation {
                usage: ColorUsage::Shading,
                space: ColorSpace::Lab,
                components: vec![50.0, -2.5, 3.25],
                spot_name: None,
                spot_names: Vec::new(),
                source: None,
            },
        ],
        capabilities: vec![
            EditCapability::RewriteColorOperand,
            EditCapability::AddTextSpreadStroke,
        ],
    }
}

fn read_only_form_entry() -> InventoryEntry {
    InventoryEntry {
        id: object_id(1, 2),
        kind: ObjectKind::FormXObject,
        provenance: Provenance {
            page: PageIndex(1),
            scope: ContentScope::AnnotationAppearance,
            range: Some(ByteRange { start: 60, end: 68 }),
            invocation: None,
        },
        bounds: None,
        colors: Vec::new(),
        capabilities: vec![EditCapability::ReadOnly],
    }
}

fn object_id(page: u32, sequence: u32) -> ObjectId {
    let mut digest = [0; 32];
    for (offset, byte) in digest.iter_mut().enumerate() {
        *byte = u8::try_from(sequence * 32 + u32::try_from(offset).unwrap_or(0)).unwrap_or(0);
    }

    ObjectId {
        page: PageIndex(page),
        sequence,
        digest,
    }
}

fn inventory_fixture_json() -> Json {
    Json::object([(
        "entries",
        Json::array([
            bounded_vector_entry_json(),
            sourced_text_entry_json(),
            read_only_form_entry_json(),
        ]),
    )])
}

fn bounded_vector_entry_json() -> Json {
    Json::object([
        ("id", object_id_json(1, 0)),
        ("kind", Json::string("vector")),
        (
            "provenance",
            Json::object([
                ("page", Json::U32(1)),
                ("scope", Json::object([("kind", Json::string("page"))])),
                ("range", byte_range_json(20, 31)),
            ]),
        ),
        (
            "bounds",
            Json::object([
                ("x_min", Json::F64(10.25)),
                ("y_min", Json::F64(20.5)),
                ("x_max", Json::F64(110.75)),
                ("y_max", Json::F64(220.125)),
            ]),
        ),
        (
            "colors",
            Json::array([Json::object([
                ("usage", Json::string("stroke")),
                ("space", Json::string("device_cmyk")),
                (
                    "components",
                    Json::array([
                        Json::F64(0.1),
                        Json::F64(0.2),
                        Json::F64(0.3),
                        Json::F64(0.4),
                    ]),
                ),
                ("spot_name", Json::Null),
                ("source", byte_range_json(3, 18)),
            ])]),
        ),
        (
            "capabilities",
            Json::array([
                Json::string("rewrite_color_operand"),
                Json::string("adjust_stroke_width"),
            ]),
        ),
    ])
}

fn bounded_vector_entry_with_invocation_json() -> Json {
    Json::object([
        ("id", object_id_json(1, 0)),
        ("kind", Json::string("vector")),
        (
            "provenance",
            Json::object([
                ("page", Json::U32(1)),
                ("scope", Json::object([("kind", Json::string("page"))])),
                ("range", byte_range_json(20, 31)),
                ("invocation", invocation_path_json(0, b"Fm")),
            ]),
        ),
        (
            "bounds",
            Json::object([
                ("x_min", Json::F64(10.25)),
                ("y_min", Json::F64(20.5)),
                ("x_max", Json::F64(110.75)),
                ("y_max", Json::F64(220.125)),
            ]),
        ),
        (
            "colors",
            Json::array([Json::object([
                ("usage", Json::string("stroke")),
                ("space", Json::string("device_cmyk")),
                (
                    "components",
                    Json::array([
                        Json::F64(0.1),
                        Json::F64(0.2),
                        Json::F64(0.3),
                        Json::F64(0.4),
                    ]),
                ),
                ("spot_name", Json::Null),
                ("source", byte_range_json(3, 18)),
            ])]),
        ),
        (
            "capabilities",
            Json::array([
                Json::string("rewrite_color_operand"),
                Json::string("adjust_stroke_width"),
            ]),
        ),
    ])
}

fn sourced_text_entry_json() -> Json {
    Json::object([
        ("id", object_id_json(1, 1)),
        ("kind", Json::string("text")),
        (
            "provenance",
            Json::object([
                ("page", Json::U32(1)),
                (
                    "scope",
                    Json::object([
                        ("kind", Json::string("form_x_object")),
                        ("name", pdf_name_json(b"FmText")),
                    ]),
                ),
                ("range", byte_range_json(40, 52)),
            ]),
        ),
        ("bounds", Json::Null),
        (
            "colors",
            Json::array([
                Json::object([
                    ("usage", Json::string("fill")),
                    (
                        "space",
                        Json::object([("resource", pdf_name_json(b"BrandSpot"))]),
                    ),
                    ("components", Json::array([Json::F64(0.65)])),
                    ("spot_name", pdf_name_json(b"BrandSpot")),
                    ("source", byte_range_json(32, 39)),
                ]),
                Json::object([
                    ("usage", Json::string("shading")),
                    ("space", Json::string("lab")),
                    (
                        "components",
                        Json::array([Json::F64(50.0), Json::F64(-2.5), Json::F64(3.25)]),
                    ),
                    ("spot_name", Json::Null),
                    ("source", Json::Null),
                ]),
            ]),
        ),
        (
            "capabilities",
            Json::array([
                Json::string("rewrite_color_operand"),
                Json::string("add_text_spread_stroke"),
            ]),
        ),
    ])
}

fn read_only_form_entry_json() -> Json {
    Json::object([
        ("id", object_id_json(1, 2)),
        ("kind", Json::string("form_x_object")),
        (
            "provenance",
            Json::object([
                ("page", Json::U32(1)),
                (
                    "scope",
                    Json::object([("kind", Json::string("annotation_appearance"))]),
                ),
                ("range", byte_range_json(60, 68)),
            ]),
        ),
        ("bounds", Json::Null),
        ("colors", Json::array([])),
        ("capabilities", Json::array([Json::string("read_only")])),
    ])
}

fn object_id_json(page: u32, sequence: u32) -> Json {
    Json::object([
        ("page", Json::U32(page)),
        ("sequence", Json::U32(sequence)),
        ("digest", digest_json(sequence)),
    ])
}

fn digest_json(sequence: u32) -> Json {
    Json::array((0..32).map(|offset| Json::U32(sequence * 32 + offset)))
}

fn byte_range_json(start: u32, end: u32) -> Json {
    Json::object([("start", Json::U32(start)), ("end", Json::U32(end))])
}

fn pdf_name_json(bytes: &[u8]) -> Json {
    Json::array(bytes.iter().copied().map(|byte| Json::U32(u32::from(byte))))
}

fn invocation_path_json(ordinal: u32, name: &[u8]) -> Json {
    Json::object([(
        "frames",
        Json::array([Json::object([
            ("ordinal", Json::U32(ordinal)),
            ("name", pdf_name_json(name)),
        ])]),
    )])
}
