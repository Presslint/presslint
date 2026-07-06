//! Typed-provenance serde-transparency tests (Phase 0a-6).
//!
//! These prove [`DecodedRange`] is serde-transparent: it serializes exactly like
//! the bare [`ByteRange`] it wraps and round-trips from the same wire shape.

use presslint_types::{ByteRange, ColorSpace};
use serde::Deserialize;
use serde::de::value::MapDeserializer;

use super::mini_json;
use crate::{DecodedRange, GraphicsColor};

#[test]
fn decoded_range_serializes_exactly_like_the_bare_byte_range() -> Result<(), mini_json::JsonError> {
    let range = ByteRange { start: 3, end: 18 };

    // `#[serde(transparent)]`: the newtype and the bare range must produce the
    // SAME wire bytes — a plain `{"start":..,"end":..}` object.
    assert_eq!(mini_json::to_json(&range)?, r#"{"start":3,"end":18}"#);
    assert_eq!(
        mini_json::to_json(&DecodedRange::new(range))?,
        mini_json::to_json(&range)?
    );
    Ok(())
}

#[test]
fn graphics_color_with_decoded_source_keeps_the_prior_json_shape()
-> Result<(), mini_json::JsonError> {
    // The typed `source` field must serialize as the plain range object it was
    // before the newtype adoption — no wrapper, no extra nesting.
    let color = GraphicsColor {
        space: ColorSpace::DeviceCmyk,
        components: vec![0.0, 0.0, 0.0, 1.0],
        resource_name: None,
        spot_name: None,
        source: Some(DecodedRange::new(ByteRange { start: 3, end: 18 })),
    };

    assert_eq!(
        mini_json::to_json(&color)?,
        concat!(
            r#"{"space":"device_cmyk","components":[0,0,0,1],"#,
            r#""resource_name":null,"spot_name":null,"source":{"start":3,"end":18}}"#
        )
    );
    Ok(())
}

#[test]
fn decoded_range_round_trips_from_the_bare_byte_range_wire_shape() -> Result<(), String> {
    // Deserializing the newtype from the exact map shape a bare `ByteRange`
    // serializes to proves the round-trip is transparent in both directions.
    let entries = [("start", 3_usize), ("end", 18_usize)];
    let decoded = DecodedRange::deserialize(MapDeserializer::<_, serde::de::value::Error>::new(
        entries.into_iter(),
    ))
    .map_err(|error| error.to_string())?;
    let bare = ByteRange::deserialize(MapDeserializer::<_, serde::de::value::Error>::new(
        entries.into_iter(),
    ))
    .map_err(|error| error.to_string())?;

    assert_eq!(decoded, DecodedRange::new(ByteRange { start: 3, end: 18 }));
    assert_eq!(decoded, DecodedRange::new(bare));
    assert_eq!(decoded.into_byte_range(), bare);
    Ok(())
}
