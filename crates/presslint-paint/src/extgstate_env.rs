//! Borrowed page/form `ExtGState` environment consumed by the graphics-state
//! walker, and the classified `ExtGState` state carried on the snapshot.
//!
//! This mirrors [`ColorSpaceEnv`](crate::ColorSpaceEnv): a thin borrowed view over
//! a per-scope slice of classified `ExtGState` resources, keyed by resource name.
//! The walker interprets `gs` against it. With a default-empty environment, `gs`
//! leaves these seven classified parameters untouched. In all-environment
//! mode, each resource also carries a compact mapped `/Font` directive; legacy
//! constructors retain the earlier every-`gs` invalidation behavior.
//!
//! The model is inventory-native: values are CLASSIFIED, never simulated. No PDF
//! defaults are invented, no alpha math is done, and no `PdfName` blend-mode name
//! is carried in the hot snapshot — only the coarse [`BlendModeClass`]. The
//! classified model here is produced by `presslint-pdf` and mapped into this form
//! by the umbrella crate, so `presslint-paint` keeps no dependency on the
//! structural PDF layer (the same layering as [`ColorSpaceResource`]).
//!
//! Crate layering: this module owns paint/graphics-state semantics only. It never
//! parses PDF dictionaries.

use presslint_types::PdfName;

use crate::font_env::ExtGStateFontDirective;

/// Per-parameter classification of one `ExtGState` graphics-state value.
///
/// Generic over the classified value type `T` for the parameter (for example
/// [`OverprintMode`] or [`AlphaClass`]). It is `Copy` for every `T` used here, so
/// an [`ExtGStateParams`] and a [`GraphicsExtGStateSnapshot`] are both `Copy`.
///
/// In a resource ([`ExtGStateParams`]) the variants mean:
/// - [`Default`](Self::Default): the dictionary does NOT carry this key, so a `gs`
///   that applies the resource leaves the snapshot's current value untouched
///   (`gs` layers over existing state; it never resets an absent key).
/// - [`Set`](Self::Set): the key is present with a classified value.
/// - [`Unresolved`](Self::Unresolved): the key is present but its value could not
///   be resolved (e.g. an unresolved indirect reference).
/// - [`Unclassified`](Self::Unclassified): the key is present with a value shape
///   outside the classified vocabulary.
///
/// In the snapshot ([`GraphicsExtGStateSnapshot`]) the same variants describe the
/// value currently in force: [`Default`](Self::Default) is the page-initial value
/// no `gs` has changed, and the others are copied from the applied resource (or
/// [`Unresolved`](Self::Unresolved) for every field after a `gs` whose name is not
/// in a non-empty environment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GsParam<T> {
    /// Page-initial value in the snapshot; absent key in a resource.
    Default,
    /// Present, classified value.
    Set(T),
    /// Present but unresolvable value.
    Unresolved,
    /// Present with an unclassified value shape.
    Unclassified,
}

/// Overprint mode (`OPM`) classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverprintMode {
    /// Numeric `0`.
    Zero,
    /// Numeric `1`.
    One,
    /// Any other numeric value.
    Other,
}

/// Alpha constant (`CA`/`ca`) classification. Exact numeric 1.0 is opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlphaClass {
    /// Alpha exactly equal to 1.0.
    Opaque,
    /// Any alpha other than 1.0.
    NonOpaque,
}

/// Blend mode (`BM`) classification. The full name stays out of the hot snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendModeClass {
    /// `/Normal` blend mode.
    Normal,
    /// A non-`/Normal` named blend mode.
    NonNormal,
    /// A blend-mode value that is a name outside the recognised set, or an
    /// array-form list; classified as present-but-other without carrying bytes.
    OtherNamed,
}

/// Soft mask (`SMask`) presence classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoftMaskClass {
    /// The soft mask is `/None`.
    None,
    /// A soft mask is present.
    Present,
}

/// The seven classified `ExtGState` parameters this read model tracks.
///
/// One field per Phase-1 safety key: `OP`, `op`, `OPM`, `CA`, `ca`, `BM`, `SMask`.
/// As a resource's params, a [`GsParam::Default`] field marks a key the dictionary
/// does not carry (so `gs` layering leaves the snapshot's current value in place);
/// every other classification is copied through onto the snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtGStateParams {
    /// Stroking overprint flag (`OP`).
    pub overprint_stroke: GsParam<bool>,
    /// Non-stroking overprint flag (`op`).
    pub overprint_fill: GsParam<bool>,
    /// Overprint mode (`OPM`).
    pub overprint_mode: GsParam<OverprintMode>,
    /// Stroking alpha constant (`CA`).
    pub stroke_alpha: GsParam<AlphaClass>,
    /// Non-stroking alpha constant (`ca`).
    pub fill_alpha: GsParam<AlphaClass>,
    /// Blend mode (`BM`).
    pub blend_mode: GsParam<BlendModeClass>,
    /// Soft mask (`SMask`).
    pub soft_mask: GsParam<SoftMaskClass>,
}

impl ExtGStateParams {
    /// Params that set nothing: every field [`GsParam::Default`].
    ///
    /// Applying these to a snapshot is a no-op (each `Default` field layers over
    /// the current value without changing it).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            overprint_stroke: GsParam::Default,
            overprint_fill: GsParam::Default,
            overprint_mode: GsParam::Default,
            stroke_alpha: GsParam::Default,
            fill_alpha: GsParam::Default,
            blend_mode: GsParam::Default,
            soft_mask: GsParam::Default,
        }
    }
}

/// One classified `ExtGState` resource expressed in the inventory model.
///
/// Carries the resource name that selects it (the `gs` operand without the leading
/// slash), the classified [`ExtGStateParams`], and whether the dictionary held
/// keys outside the Phase-1 safety set. It holds no PDF bytes or dictionary data.
/// Its derive set mirrors [`ColorSpaceResource`](crate::ColorSpaceResource).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtGStateResource {
    /// Resource name (without the leading slash) selected by `gs`.
    pub name: PdfName,
    /// Classified Phase-1 safety parameters.
    pub params: ExtGStateParams,
    /// True when the dictionary carried keys outside the Phase-1 safety set.
    pub has_unclassified_keys: bool,
    /// Consumer-mapped `/Font` effect for all-environment walks.
    pub font: ExtGStateFontDirective,
}

/// Classified `ExtGState` state carried on the graphics-state snapshot.
///
/// The SAME seven fields as [`ExtGStateParams`] but interpreted as the value
/// currently in force (values only; NO names). It is `Copy`, so `q`/`Q`
/// save/restore it for free as part of the snapshot and `Rc::make_mut` pays no
/// extra allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GraphicsExtGStateSnapshot {
    /// Stroking overprint flag (`OP`).
    pub overprint_stroke: GsParam<bool>,
    /// Non-stroking overprint flag (`op`).
    pub overprint_fill: GsParam<bool>,
    /// Overprint mode (`OPM`).
    pub overprint_mode: GsParam<OverprintMode>,
    /// Stroking alpha constant (`CA`).
    pub stroke_alpha: GsParam<AlphaClass>,
    /// Non-stroking alpha constant (`ca`).
    pub fill_alpha: GsParam<AlphaClass>,
    /// Blend mode (`BM`).
    pub blend_mode: GsParam<BlendModeClass>,
    /// Soft mask (`SMask`).
    pub soft_mask: GsParam<SoftMaskClass>,
}

impl GraphicsExtGStateSnapshot {
    /// The page-initial `ExtGState` state: every field [`GsParam::Default`].
    #[must_use]
    pub const fn page_default() -> Self {
        Self {
            overprint_stroke: GsParam::Default,
            overprint_fill: GsParam::Default,
            overprint_mode: GsParam::Default,
            stroke_alpha: GsParam::Default,
            fill_alpha: GsParam::Default,
            blend_mode: GsParam::Default,
            soft_mask: GsParam::Default,
        }
    }

    /// Layer an applied resource's params over the current state.
    ///
    /// For each field, a [`GsParam::Default`] resource param leaves the snapshot's
    /// current value untouched (the dictionary did not carry that key); every other
    /// classification ([`Set`](GsParam::Set), [`Unresolved`](GsParam::Unresolved),
    /// [`Unclassified`](GsParam::Unclassified)) replaces the current value. This is
    /// the `gs`-hit path: `gs` layers, it never resets an absent key.
    pub const fn apply(&mut self, params: &ExtGStateParams) {
        self.overprint_stroke = layer(self.overprint_stroke, params.overprint_stroke);
        self.overprint_fill = layer(self.overprint_fill, params.overprint_fill);
        self.overprint_mode = layer(self.overprint_mode, params.overprint_mode);
        self.stroke_alpha = layer(self.stroke_alpha, params.stroke_alpha);
        self.fill_alpha = layer(self.fill_alpha, params.fill_alpha);
        self.blend_mode = layer(self.blend_mode, params.blend_mode);
        self.soft_mask = layer(self.soft_mask, params.soft_mask);
    }

    /// Set every field to [`GsParam::Unresolved`].
    ///
    /// The `gs`-miss path on a NON-empty environment: the name did not resolve, so
    /// nothing is known about what that `gs` did to any parameter and no value is
    /// invented.
    pub const fn set_all_unresolved(&mut self) {
        *self = Self {
            overprint_stroke: GsParam::Unresolved,
            overprint_fill: GsParam::Unresolved,
            overprint_mode: GsParam::Unresolved,
            stroke_alpha: GsParam::Unresolved,
            fill_alpha: GsParam::Unresolved,
            blend_mode: GsParam::Unresolved,
            soft_mask: GsParam::Unresolved,
        };
    }
}

impl Default for GraphicsExtGStateSnapshot {
    fn default() -> Self {
        Self::page_default()
    }
}

/// Layer one incoming resource param over the current snapshot param.
///
/// [`GsParam::Default`] means the dictionary did not carry the key, so the current
/// value is kept; any other classification replaces it.
const fn layer<T: Copy>(current: GsParam<T>, incoming: GsParam<T>) -> GsParam<T> {
    match incoming {
        GsParam::Default => current,
        replacement => replacement,
    }
}

/// Borrowed page/form `ExtGState` environment the walker consumes.
///
/// The environment is a reference into a per-scope classified `ExtGState` slice,
/// not a per-operator clone. It is `Copy`, so passing it into the walker costs one
/// pointer/length pair. This mirrors [`ColorSpaceEnv`](crate::ColorSpaceEnv)
/// exactly.
#[derive(Debug, Clone, Copy)]
pub struct ExtGStateEnv<'a> {
    resources: &'a [ExtGStateResource],
}

impl<'a> ExtGStateEnv<'a> {
    /// Borrow a scope's classified `ExtGState` resources as an environment.
    #[must_use]
    pub const fn new(resources: &'a [ExtGStateResource]) -> Self {
        Self { resources }
    }

    /// The empty environment: no `gs` name resolves and the feature is off.
    ///
    /// This is the default; with it `gs` does not mutate the seven classified
    /// `ExtGState` parameters. Font certainty is invalidated separately.
    #[must_use]
    pub const fn empty() -> Self {
        Self { resources: &[] }
    }

    /// Resolve a `gs` resource name to its classified resource.
    #[must_use]
    pub fn resolve(&self, name: &PdfName) -> Option<&'a ExtGStateResource> {
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

impl Default for ExtGStateEnv<'_> {
    fn default() -> Self {
        Self::empty()
    }
}
