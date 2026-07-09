//! Contract-semantics tests for the [`ColorEngine`] trait.
//!
//! A tiny in-crate fake engine exercises the `prepare` / `shape` / `apply`
//! plumbing and pins that the associated `Error` type actually satisfies the
//! trait's `Debug + Clone + PartialEq + Serialize + DeserializeOwned` bounds.
//! The [`DeviceLinkShape`] serde-shape test locks its public JSON encoding.

use presslint_types::ColorSpace;
use serde::{Deserialize, Serialize};

use super::assert_json_round_trip;
use super::json::Json;
use crate::{ColorEngine, DeviceLinkShape};

/// A structured error for the in-crate fake engine. Its derives exercise the
/// trait's `Error` bounds; the externally-tagged serde encoding round-trips
/// through the dependency-free JSON harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FakeError {
    /// The synthetic profile bytes were empty.
    EmptyProfile,
    /// The input component count did not match the prepared link.
    ChannelCountMismatch {
        /// Channels the link expects.
        expected: usize,
        /// Channels supplied.
        got: usize,
    },
}

/// A prepared link for the fake engine: just its declared shape.
struct FakeLink {
    shape: DeviceLinkShape,
}

/// A trivial in-crate [`ColorEngine`] with no native backend. It pins the trait
/// plumbing only: `prepare` rejects empty bytes and otherwise yields a fixed
/// DeviceRGB→DeviceCMYK 3→4 link; `apply` performs a deterministic naive
/// inversion so the output is observable.
struct FakeEngine;

fn fake_shape() -> DeviceLinkShape {
    DeviceLinkShape {
        source: ColorSpace::DeviceRgb,
        destination: ColorSpace::DeviceCmyk,
        input_channels: 3,
        output_channels: 4,
    }
}

impl ColorEngine for FakeEngine {
    type DeviceLink = FakeLink;
    type Error = FakeError;

    fn prepare_device_link(&self, bytes: &[u8]) -> Result<Self::DeviceLink, Self::Error> {
        if bytes.is_empty() {
            return Err(FakeError::EmptyProfile);
        }
        Ok(FakeLink {
            shape: fake_shape(),
        })
    }

    fn device_link_shape(&self, link: &Self::DeviceLink) -> DeviceLinkShape {
        link.shape.clone()
    }

    fn apply_device_link(
        &self,
        link: &Self::DeviceLink,
        input: &[f64],
    ) -> Result<Vec<f64>, Self::Error> {
        if input.len() != link.shape.input_channels {
            return Err(FakeError::ChannelCountMismatch {
                expected: link.shape.input_channels,
                got: input.len(),
            });
        }
        // Naive RGB→CMYK inversion with zero black, purely to make the output
        // observable and deterministic.
        let mut output: Vec<f64> = input.iter().map(|component| 1.0 - component).collect();
        output.push(0.0);
        Ok(output)
    }
}

#[test]
fn prepare_then_shape_then_apply_round_trips() {
    let engine = FakeEngine;
    let link = engine
        .prepare_device_link(b"synthetic")
        .expect("prepare a non-empty synthetic link");

    assert_eq!(engine.device_link_shape(&link), fake_shape());

    let output = engine
        .apply_device_link(&link, &[0.25, 0.5, 0.75])
        .expect("apply to a matching-arity input");
    assert_eq!(output, vec![0.75, 0.5, 0.25, 0.0]);
}

#[test]
fn prepare_rejects_empty_profile() {
    let engine = FakeEngine;
    assert!(matches!(
        engine.prepare_device_link(&[]),
        Err(FakeError::EmptyProfile)
    ));
}

#[test]
fn apply_rejects_channel_count_mismatch() {
    let engine = FakeEngine;
    let link = engine
        .prepare_device_link(b"synthetic")
        .expect("prepare a non-empty synthetic link");
    assert_eq!(
        engine.apply_device_link(&link, &[0.1, 0.2]),
        Err(FakeError::ChannelCountMismatch {
            expected: 3,
            got: 2,
        })
    );
}

#[test]
fn engine_error_satisfies_clone_eq_and_serde_bounds() {
    // Clone + PartialEq: the trait requires both on `Error`.
    let error = FakeError::ChannelCountMismatch {
        expected: 3,
        got: 2,
    };
    assert_eq!(error.clone(), error);

    // Serialize + DeserializeOwned: a unit variant round-trips through the
    // dependency-free harness as its bare snake_case name.
    assert_json_round_trip(&FakeError::EmptyProfile, Json::string("empty_profile"));
}

#[test]
fn device_link_shape_json_shape_is_locked() {
    assert_json_round_trip(
        &fake_shape(),
        Json::object([
            ("source", Json::string("device_rgb")),
            ("destination", Json::string("device_cmyk")),
            ("input_channels", Json::U32(3)),
            ("output_channels", Json::U32(4)),
        ]),
    );
}
