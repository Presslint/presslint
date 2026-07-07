//! Borrowed page colour-space environment consumed by the graphics-state walker.
//!
//! This is the ONE new abstraction for page resource colour-space tracking. It is
//! a thin borrowed view over a per-page slice of classified colour-space
//! resources, keyed by resource name. The walker interprets `cs`/`CS` and
//! `sc`/`scn`/`SC`/`SCN` against it. A default-empty environment reproduces the
//! device-only behaviour byte-for-byte: with no classified resources, no `cs`
//! name ever resolves, so a device-only content stream walks exactly as before.
//!
//! Crate layering: this module owns paint/graphics-state semantics only. It never
//! parses PDF dictionaries. The classified model
//! ([`ColorSpaceResource`]) is produced by `presslint-pdf` and mapped into this
//! inventory-native form by the umbrella crate, so `presslint-inventory` keeps no
//! dependency on the structural PDF layer.

use presslint_types::{ColorSpace, PdfName};

/// One page colour-space resource expressed in the inventory colour model.
///
/// Carries the resource name that selects the space (`cs`/`CS` operand, without
/// the leading slash), the observed source [`ColorSpace`] family, an optional
/// shallow component count, and the spot colorant names for
/// `Separation`/`DeviceN`. It holds no PDF bytes, dictionaries, or profile data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColorSpaceResource {
    /// Resource name (without the leading slash) that selects this space.
    pub name: PdfName,
    /// Observed source colour-space family.
    pub space: ColorSpace,
    /// Shallow component count when known.
    pub component_count: Option<usize>,
    /// Spot colorant names for `Separation` (one) or `DeviceN` (many).
    pub spot_names: Vec<PdfName>,
}

impl ColorSpaceResource {
    /// The legacy first colorant name reported for `Separation`/`DeviceN`.
    ///
    /// `Separation` has exactly one colorant; `DeviceN` may have several and the
    /// first is reported (a single-name field cannot carry them all). Other
    /// families have no spot name.
    #[must_use]
    pub fn spot_name(&self) -> Option<PdfName> {
        match self.space {
            ColorSpace::Separation | ColorSpace::DeviceN => self.spot_names.first().cloned(),
            _ => None,
        }
    }

    /// The complete colorant list reported for `Separation`/`DeviceN`.
    #[must_use]
    pub fn spot_names(&self) -> Vec<PdfName> {
        match self.space {
            ColorSpace::Separation | ColorSpace::DeviceN => self.spot_names.clone(),
            _ => Vec::new(),
        }
    }

    /// The implied initial colour when this space is selected by `cs`/`CS`.
    ///
    /// Per ISO 32000-1 §8.6.3, the initial colour is all-zero for the additive /
    /// subtractive device families (with `DeviceCMYK` black at `0 0 0 1`) and ICC
    /// profiles, and all-one for `Separation`/`DeviceN` (full tint). When the
    /// component count is unknown the initial colour is empty rather than
    /// fabricated.
    #[must_use]
    pub fn initial_color(&self) -> Vec<f64> {
        match &self.space {
            ColorSpace::DeviceGray => vec![0.0],
            ColorSpace::DeviceRgb => vec![0.0, 0.0, 0.0],
            ColorSpace::DeviceCmyk => vec![0.0, 0.0, 0.0, 1.0],
            ColorSpace::Separation | ColorSpace::DeviceN => {
                vec![1.0; self.component_count.unwrap_or(1)]
            }
            _ => self
                .component_count
                .map_or_else(Vec::new, |count| vec![0.0; count]),
        }
    }
}

/// Borrowed page colour-space environment the walker consumes.
///
/// The environment is a reference into a per-page classified colour-space slice,
/// not a per-operator clone. It is `Copy`, so passing it into the walker or the
/// inventory builders costs one pointer/length pair.
#[derive(Debug, Clone, Copy)]
pub struct ColorSpaceEnv<'a> {
    resources: &'a [ColorSpaceResource],
}

impl<'a> ColorSpaceEnv<'a> {
    /// Borrow a page's classified colour-space resources as an environment.
    #[must_use]
    pub const fn new(resources: &'a [ColorSpaceResource]) -> Self {
        Self { resources }
    }

    /// The empty environment: no `cs`/`CS` name resolves.
    ///
    /// This is the default and reproduces device-only behaviour byte-for-byte.
    #[must_use]
    pub const fn empty() -> Self {
        Self { resources: &[] }
    }

    /// Resolve a `cs`/`CS` resource name to its classified colour space.
    #[must_use]
    pub fn resolve(&self, name: &PdfName) -> Option<&'a ColorSpaceResource> {
        self.resources
            .iter()
            .find(|resource| &resource.name == name)
    }

    /// Whether the environment carries no classified resources.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

impl Default for ColorSpaceEnv<'_> {
    fn default() -> Self {
        Self::empty()
    }
}
