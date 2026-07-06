//! Shared public data types for `presslint`.
//!
//! This crate contains stable identifiers, page geometry, color observations,
//! and provenance records used by inventory, selectors, actions, and PDF write
//! planning. It performs no I/O.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

/// Zero-based page index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PageIndex(pub u32);

/// Stable identity for a marked page object.
///
/// This is not a PDF indirect reference. It identifies the object as observed
/// by the inventory pass: page, sequence, and a content-derived digest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectId {
    /// Page where the object was discovered.
    pub page: PageIndex,
    /// Deterministic sequence number within the page inventory.
    pub sequence: u32,
    /// Digest of canonical object evidence.
    pub digest: [u8; 32],
}

/// Byte range in a source stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ByteRange {
    /// Inclusive start offset.
    pub start: usize,
    /// Exclusive end offset.
    pub end: usize,
}

/// Source location that can be mapped back to an editable PDF scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Provenance {
    /// Page where the observation was made.
    pub page: PageIndex,
    /// Stable content scope identifier.
    pub scope: ContentScope,
    /// Byte range in the decoded content stream when available.
    pub range: Option<ByteRange>,
}

/// Content scope where an inventory object was discovered.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentScope {
    /// Direct page content stream.
    Page,
    /// Form `XObject` content invoked from a page or another form.
    FormXObject {
        /// Resource name used to invoke the form.
        name: PdfName,
    },
    /// Annotation appearance stream.
    AnnotationAppearance,
}

/// PDF name represented as raw bytes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PdfName(pub Vec<u8>);

/// One form invocation frame in a nested paint traversal.
///
/// `ordinal` is zero-based among form-classified `Do` invocations in the
/// calling program, after image-vs-form classification. The `name` is the
/// resource name used by that invocation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvocationFrame {
    /// Zero-based form invocation position within the caller program.
    pub ordinal: u32,
    /// Resource name used to invoke the form.
    pub name: PdfName,
}

/// Nested path from page-level content into form invocations.
///
/// An empty path means page-level content. This is shared provenance vocabulary
/// for future inventory contracts, but it is not yet referenced by
/// [`Provenance`], so existing serialized structs keep their prior shape.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvocationPath {
    /// Ordered call frames from outermost to innermost form invocation.
    pub frames: Vec<InvocationFrame>,
}

/// Axis-aligned bounds in default user space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoundingBox {
    /// Minimum x coordinate.
    pub x_min: f64,
    /// Minimum y coordinate.
    pub y_min: f64,
    /// Maximum x coordinate.
    pub x_max: f64,
    /// Maximum y coordinate.
    pub y_max: f64,
}

/// PDF color-space family observed by the inventory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorSpace {
    /// `/DeviceGray`.
    DeviceGray,
    /// `/DeviceRGB`.
    DeviceRgb,
    /// `/DeviceCMYK`.
    DeviceCmyk,
    /// `/ICCBased`.
    IccBased,
    /// `/Lab`.
    Lab,
    /// `/CalGray`.
    CalGray,
    /// `/CalRGB`.
    CalRgb,
    /// `/Indexed`.
    Indexed,
    /// `/Separation`.
    Separation,
    /// `/DeviceN`.
    DeviceN,
    /// `/Pattern`.
    Pattern,
    /// Named resource alias.
    Resource(PdfName),
    /// Unsupported or unresolved color-space shape.
    Unknown,
}

/// How a color observation was used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorUsage {
    /// Non-stroking paint.
    Fill,
    /// Stroking paint.
    Stroke,
    /// Image samples or image color space.
    Image,
    /// Shading color output.
    Shading,
}

/// Color metadata attached to an inventory entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ColorObservation {
    /// Paint use.
    pub usage: ColorUsage,
    /// Observed source color space.
    pub space: ColorSpace,
    /// Components in source-space order.
    pub components: Vec<f64>,
    /// Spot colorant name for `Separation` / `DeviceN` observations.
    pub spot_name: Option<PdfName>,
    /// Byte range of the content-stream operator that established this color.
    ///
    /// `Some(range)` points at the color-setting operator's record (e.g. the
    /// `rg`/`g`/`k` operator), not the paint or text-showing operator that
    /// observed the color. It is `None` for the page-default/inherited color
    /// and for synthesized observations that no color operator produced.
    pub source: Option<ByteRange>,
}

/// High-level class of page object.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    /// Text or glyph run.
    Text,
    /// Vector path paint operation.
    Vector,
    /// Image object.
    Image,
    /// Form `XObject` invocation.
    FormXObject,
    /// Shading paint.
    Shading,
    /// Tiling or shading pattern use.
    Pattern,
    /// Annotation appearance or annotation color entry.
    Annotation,
}

/// Edit capability advertised by the inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EditCapability {
    /// Color operands can be rewritten in a content stream.
    RewriteColorOperand,
    /// Image stream samples can be replaced.
    ReplaceImageStream,
    /// Text can be wrapped with an additional stroke operation.
    AddTextSpreadStroke,
    /// Vector stroke width can be adjusted.
    AdjustStrokeWidth,
    /// Object is read-only for the current implementation.
    ReadOnly,
}
