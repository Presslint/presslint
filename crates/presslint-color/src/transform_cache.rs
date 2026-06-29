//! Deterministic, report-only transform cache-key contract.
//!
//! A future color executor needs to recognise identical abstract color
//! transforms before any real ICC/`DeviceLink` execution exists, so it can reuse a
//! built transform instead of rebuilding it. This module models only the key
//! that identifies one such transform; it does not implement a cache store,
//! eviction, invalidation, parsing, or any transform execution.
//!
//! The key names a transform by the conversion path the crate already resolves
//! (see [`DeviceLinkDecision`]):
//!
//! - the `DeviceLink` path — keyed by the selected `DeviceLink`'s stable `id` plus
//!   the abstract source and destination [`ColorSpace`];
//! - the profile-connection-space (PCS) path — keyed by caller-supplied,
//!   bytes-free stable source and destination profile references plus the
//!   abstract source and destination [`ColorSpace`].
//!
//! Every profile and `DeviceLink` is referenced by a stable caller-supplied id
//! only. The key never carries, hashes, or copies ICC/profile bytes, mirroring
//! the `DeviceLinkDescription.id` and `OutputProfileSource::OpaqueId` patterns
//! and directly answering the performance-discipline watch-item against carrying
//! embedded profile bytes through report-only decisions.
//!
//! When a deterministic, bounded key cannot be formed — a rejected `DeviceLink`
//! decision, or a PCS path missing a stable source or destination profile id —
//! the helper returns a structured [`TransformCacheKeyRejection`] instead of
//! fabricating a key.

use presslint_core::ColorSpace;
use serde::{Deserialize, Serialize};

use crate::devicelink::DeviceLinkDecision;
use crate::policy::TransformRequest;

/// Bytes-free, stable reference to a color profile for the PCS path.
///
/// This mirrors the `OutputProfileSource::OpaqueId` pattern: it names a profile
/// by a stable caller-supplied id only. It carries no ICC data and no profile
/// bytes, and the derivation never hashes or copies profile bytes through it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProfileReference {
    /// Stable caller-defined profile identifier.
    pub id: String,
}

/// Deterministic, bytes-free identity of one abstract color transform.
///
/// This is the key a future bounded cache can use directly: it derives `Eq` and
/// `Hash`, so equal transforms (same path, ids, and color spaces) compare and
/// hash equal, and any differing source, destination, or id yields a different
/// key. It contains only stable ids and small abstract [`ColorSpace`] values;
/// it never contains ICC/profile bytes, decoded streams, or operands.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "path", rename_all = "snake_case")]
pub enum TransformCacheKey {
    /// Transform resolved through a selected `DeviceLink`.
    DeviceLink {
        /// Stable id of the selected `DeviceLink`.
        device_link_id: String,
        /// Abstract source color space of the transform.
        source: ColorSpace,
        /// Abstract destination color space of the transform.
        destination: ColorSpace,
    },
    /// Transform resolved through the profile connection space.
    ProfileConnectionSpace {
        /// Stable, bytes-free source profile reference.
        source_profile: ProfileReference,
        /// Stable, bytes-free destination profile reference.
        destination_profile: ProfileReference,
        /// Abstract source color space of the transform.
        source: ColorSpace,
        /// Abstract destination color space of the transform.
        destination: ColorSpace,
    },
}

/// Reason a deterministic, bounded cache key could not be formed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum TransformCacheKeyRejection {
    /// The `DeviceLink` decision was itself rejected, so there is no transform to
    /// key.
    RejectedDeviceLink,
    /// The PCS path lacks a stable source profile id.
    MissingSourceProfileId,
    /// The PCS path lacks a stable destination profile id.
    MissingDestinationProfileId,
}

/// Outcome of deriving a transform cache key.
///
/// This decision is report-only. Producing it parses no ICC bytes, inspects no
/// PDF catalog or graphics state, executes no color transform, and mutates no
/// PDF bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum TransformCacheKeyDecision {
    /// A deterministic, bounded cache key was formed.
    Keyed {
        /// Derived cache key.
        key: TransformCacheKey,
    },
    /// No deterministic cache key could be formed.
    NoKey {
        /// Structured no-key reason.
        reason: TransformCacheKeyRejection,
    },
}

/// Derive a [`TransformCacheKey`] from an already-resolved [`DeviceLinkDecision`]
/// and the originating [`TransformRequest`], plus caller-supplied, bytes-free
/// profile references used only by the PCS path.
///
/// This function is pure: it performs no I/O, reads no PDF bytes, parses no ICC
/// profile, inspects no PDF catalog or graphics state, executes no transform,
/// and mutates no PDF bytes. It only re-keys an already-made path decision.
///
/// Derivation rules:
///
/// - [`DeviceLinkDecision::UseDeviceLink`] yields a
///   [`TransformCacheKey::DeviceLink`] keyed by the selected link's stable `id`
///   and the request's abstract source/destination color spaces.
/// - [`DeviceLinkDecision::UseProfileConnectionSpace`] yields a
///   [`TransformCacheKey::ProfileConnectionSpace`] keyed by the caller-supplied
///   source and destination profile references and the request's abstract
///   source/destination color spaces. A missing source reference yields
///   [`TransformCacheKeyRejection::MissingSourceProfileId`]; a present source but
///   missing destination reference yields
///   [`TransformCacheKeyRejection::MissingDestinationProfileId`].
/// - [`DeviceLinkDecision::Rejected`] yields
///   [`TransformCacheKeyRejection::RejectedDeviceLink`]: there is no bounded
///   transform to key.
///
/// The `source_profile`/`destination_profile` references are consulted only on
/// the PCS path; they are ignored when a `DeviceLink` was selected.
#[must_use]
pub fn derive_transform_cache_key(
    decision: &DeviceLinkDecision,
    request: &TransformRequest,
    source_profile: Option<&ProfileReference>,
    destination_profile: Option<&ProfileReference>,
) -> TransformCacheKeyDecision {
    match decision {
        DeviceLinkDecision::Rejected { .. } => TransformCacheKeyDecision::NoKey {
            reason: TransformCacheKeyRejection::RejectedDeviceLink,
        },
        DeviceLinkDecision::UseDeviceLink { device_link } => TransformCacheKeyDecision::Keyed {
            key: TransformCacheKey::DeviceLink {
                device_link_id: device_link.id.clone(),
                source: request.source.clone(),
                destination: request.destination.clone(),
            },
        },
        DeviceLinkDecision::UseProfileConnectionSpace => {
            let Some(source_profile) = source_profile else {
                return TransformCacheKeyDecision::NoKey {
                    reason: TransformCacheKeyRejection::MissingSourceProfileId,
                };
            };
            let Some(destination_profile) = destination_profile else {
                return TransformCacheKeyDecision::NoKey {
                    reason: TransformCacheKeyRejection::MissingDestinationProfileId,
                };
            };
            TransformCacheKeyDecision::Keyed {
                key: TransformCacheKey::ProfileConnectionSpace {
                    source_profile: source_profile.clone(),
                    destination_profile: destination_profile.clone(),
                    source: request.source.clone(),
                    destination: request.destination.clone(),
                },
            }
        }
    }
}
