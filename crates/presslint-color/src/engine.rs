//! The colour-EXECUTION contract: applying a prepared `DeviceLink`.
//!
//! `presslint-color` already owns the LINK-SELECTION policy layer — which
//! `DeviceLink` to use, resolved by the pure
//! [`resolve_device_link_policy`](crate::resolve_device_link_policy) /
//! [`resolve_transform_plan`](crate::resolve_transform_plan) functions. This
//! module adds the complementary LINK-APPLICATION contract: a narrow
//! [`ColorEngine`] that an out-of-crate backend (e.g. `presslint-color-lcms`)
//! implements to turn `DeviceLink` bytes into a prepared handle and apply that
//! handle to one scalar device colour.
//!
//! The `prepare` / `apply` split is deliberate: it lets a backend retain parsed
//! profile/transform state behind [`ColorEngine::DeviceLink`] as a pure
//! implementation detail, without changing this contract.
//!
//! # What is deliberately absent
//!
//! Rendering intent, black-point compensation (BPC), and K-preservation are
//! creation-time properties of a `DeviceLink` LUT, NOT apply-time parameters: a
//! precompiled `DeviceLink` bakes the intent in, and Little CMS explicitly
//! ignores runtime intent/BPC for device links. Black preservation is a
//! rule-based operand overlay the caller applies BEFORE the link, so
//! K-preserved operands never reach the engine. Quantisation lives downstream
//! (the caller owns rounding), so [`ColorEngine::apply_device_link`] returns
//! raw `f64`. None of these appear on any method here, by design; if such
//! metadata is ever needed it becomes a SELECTION-side descriptive field, never
//! an engine input.
//!
//! # No `Send` / `Sync` bounds
//!
//! A real backend's prepared `DeviceLink` will retain native profile/transform
//! handles that are `!Send + !Sync`. This trait therefore adds no `Send` /
//! `Sync` supertrait bounds, and consumers must not assume them.

use core::fmt::Debug;

use presslint_types::ColorSpace;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

/// The source/destination colour spaces and channel counts of a prepared
/// `DeviceLink`.
///
/// This is the engine-side view of a `DeviceLink`'s two sides, expressed in the
/// shared [`ColorSpace`] vocabulary so a caller can gate operands by space
/// without depending on any backend's native colour-space signature type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLinkShape {
    /// The `DeviceLink`'s A-side (source / input) colour space.
    pub source: ColorSpace,
    /// The `DeviceLink`'s B-side (destination / output) colour space.
    pub destination: ColorSpace,
    /// Number of input components the `DeviceLink` consumes.
    pub input_channels: usize,
    /// Number of output components the `DeviceLink` produces.
    pub output_channels: usize,
}

/// A colour-execution backend that prepares and applies `DeviceLink`s.
///
/// This is the LINK-APPLICATION half of the colour engine; the LINK-SELECTION
/// half stays in the pure resolvers of this crate. An implementor parses
/// `DeviceLink` bytes once via
/// [`prepare_device_link`](Self::prepare_device_link), inspects the prepared
/// link's shape via [`device_link_shape`](Self::device_link_shape), and applies
/// it to one scalar colour via [`apply_device_link`](Self::apply_device_link).
///
/// The two associated types make the trait intentionally NOT object-safe:
/// future consumers are generic or concrete, never `dyn ColorEngine`. See the
/// module docs for why there are no `Send` / `Sync` supertrait bounds.
pub trait ColorEngine {
    /// A prepared, ready-to-apply `DeviceLink`. A backend may retain parsed
    /// profile state (and, in a later slice, native handles) behind this type.
    type DeviceLink;

    /// The structured failure type. Its bounds match this crate's serde
    /// convention so a caller can inspect, compare, and round-trip engine
    /// errors without requiring a `Display` / `std::error::Error` impl.
    type Error: Debug + Clone + PartialEq + Serialize + DeserializeOwned;

    /// Prepare a `DeviceLink` from its ICC bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if the bytes are not a usable `DeviceLink`
    /// (backend-defined; e.g. not a valid ICC profile, or not a device-link
    /// class).
    fn prepare_device_link(&self, bytes: &[u8]) -> Result<Self::DeviceLink, Self::Error>;

    /// Report the source/destination spaces and channel counts of a prepared
    /// `DeviceLink`.
    fn device_link_shape(&self, link: &Self::DeviceLink) -> DeviceLinkShape;

    /// Apply a prepared `DeviceLink` to ONE scalar colour and return its raw
    /// output components. No rounding is performed — the caller owns
    /// quantisation.
    ///
    /// # Errors
    ///
    /// Returns [`Self::Error`] if `input` is not applicable to `link`
    /// (backend-defined; e.g. channel-count mismatch, non-finite or
    /// out-of-range components, or an unsupported colour space).
    fn apply_device_link(
        &self,
        link: &Self::DeviceLink,
        input: &[f64],
    ) -> Result<Vec<f64>, Self::Error>;
}
