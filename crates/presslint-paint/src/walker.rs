use std::rc::Rc;

use presslint_syntax::OperatorRecord;
use presslint_types::{ByteRange, ColorObservation, ColorSpace, ColorUsage, PdfName};
use serde::{Deserialize, Serialize};

use crate::color_space_env::{ColorSpaceEnv, ColorSpaceResource};
use crate::operands::{
    checked_source, color_operands, concat_matrix, device_color, expect_operands, integer_operand,
    name_operand, numeric_operands,
};

const IDENTITY_CTM: [f64; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Colour currently held by one side of the graphics state.
///
/// Generalised from device-only colour: it carries direct device colours
/// (`g`/`rg`/`k`) exactly as before AND colours set through resource colour
/// spaces (`cs`/`CS` + `sc`/`scn`/`SC`/`SCN`). For a resource colour, `space` is
/// the REAL source family (`IccBased`/`Separation`/`DeviceN`), never collapsed
/// to a device space; `resource_name` is the `/CS…` selector and `spot_name` is
/// the colorant for `Separation`/`DeviceN`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicsColor {
    /// Source colour space observed by the operator stream.
    pub space: ColorSpace,
    /// Components in source-space order.
    pub components: Vec<f64>,
    /// Resource name (`cs`/`CS` operand) that selected this colour space.
    ///
    /// `None` for direct device operators and the page-default colour;
    /// `Some(name)` once `cs`/`CS` selected a resource colour space.
    pub resource_name: Option<PdfName>,
    /// Spot colorant name for `Separation`/`DeviceN` colours.
    pub spot_name: Option<PdfName>,
    /// Record byte range of the operator that set this colour.
    ///
    /// `None` for the page-default/inherited colour; `Some(range)` once a
    /// colour operator in the walked stream established it. The range travels
    /// with the colour through `q`/`Q` save/restore.
    pub source: Option<ByteRange>,
}

impl GraphicsColor {
    /// Create a device-colour snapshot with no recorded source.
    ///
    /// The page-default colour and direct device operators use this
    /// constructor; the walker stamps `source` when a colour operator sets the
    /// colour. `resource_name`/`spot_name` stay `None`, so device colours are
    /// byte-identical to the pre-generalisation behaviour.
    #[must_use]
    pub const fn new(space: ColorSpace, components: Vec<f64>) -> Self {
        Self {
            space,
            components,
            resource_name: None,
            spot_name: None,
            source: None,
        }
    }

    /// Return this colour as an inventory colour observation.
    ///
    /// The observation carries the real source family, the spot colorant name
    /// when present, and the colour-setting operator's record range as its
    /// `source`, so callers can map the observed colour back to the bytes that
    /// established it.
    #[must_use]
    pub fn observation(&self, usage: ColorUsage) -> ColorObservation {
        ColorObservation {
            usage,
            space: self.space.clone(),
            components: self.components.clone(),
            spot_name: self.spot_name.clone(),
            source: self.source,
        }
    }
}

/// Graphics-state slots tracked by the initial content walker.
///
/// Whole snapshots are not serialized here; the walker shares them through
/// [`Rc`](std::rc::Rc) and emits references from each paint op.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicsStateSnapshot {
    /// Current transformation matrix in PDF `[a b c d e f]` layout.
    pub ctm: [f64; 6],
    /// Current stroking colour.
    pub stroking_color: GraphicsColor,
    /// Current nonstroking colour.
    pub nonstroking_color: GraphicsColor,
    /// Current text rendering mode.
    pub text_rendering_mode: TextRenderingMode,
}

impl GraphicsStateSnapshot {
    /// Return the page-initial graphics state for this walker slice.
    #[must_use]
    pub fn page_default() -> Self {
        Self {
            ctm: IDENTITY_CTM,
            stroking_color: GraphicsColor::new(ColorSpace::DeviceGray, vec![0.0]),
            nonstroking_color: GraphicsColor::new(ColorSpace::DeviceGray, vec![0.0]),
            text_rendering_mode: TextRenderingMode::Fill,
        }
    }

    /// Current stroking colour as an inventory observation.
    #[must_use]
    pub fn stroke_observation(&self) -> ColorObservation {
        self.stroking_color.observation(ColorUsage::Stroke)
    }

    /// Current nonstroking colour as an inventory observation.
    #[must_use]
    pub fn fill_observation(&self) -> ColorObservation {
        self.nonstroking_color.observation(ColorUsage::Fill)
    }
}

impl Default for GraphicsStateSnapshot {
    fn default() -> Self {
        Self::page_default()
    }
}

/// Text rendering mode relevant to first-slice text inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextRenderingMode {
    /// Fill glyph outlines with the nonstroking colour (`0 Tr`).
    Fill,
    /// Stroke glyph outlines with the stroking colour (`1 Tr`).
    Stroke,
    /// Fill and stroke glyph outlines (`2 Tr`).
    FillThenStroke,
    /// Neither fill nor stroke glyph outlines (`3 Tr`).
    Invisible,
    /// Rendering modes outside the first supported editable slice.
    Unsupported {
        /// Raw `Tr` mode value.
        value: i32,
    },
}

impl TextRenderingMode {
    /// Map a PDF `Tr` integer into this inventory slice.
    #[must_use]
    pub const fn from_pdf_value(value: i32) -> Self {
        match value {
            0 => Self::Fill,
            1 => Self::Stroke,
            2 => Self::FillThenStroke,
            3 => Self::Invisible,
            _ => Self::Unsupported { value },
        }
    }

    /// Whether this mode uses the stroking colour in the supported slice.
    #[must_use]
    pub const fn uses_stroke(self) -> bool {
        matches!(self, Self::Stroke | Self::FillThenStroke)
    }

    /// Whether this mode uses the nonstroking colour in the supported slice.
    #[must_use]
    pub const fn uses_fill(self) -> bool {
        matches!(self, Self::Fill | Self::FillThenStroke)
    }

    /// Whether this mode can be edited by first-slice text color actions.
    #[must_use]
    pub const fn has_supported_visible_paint(self) -> bool {
        matches!(self, Self::Fill | Self::Stroke | Self::FillThenStroke)
    }
}

/// Text-showing operator observed in a content stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextShowOperator {
    /// `Tj`.
    ShowText,
    /// `TJ`.
    ShowTextAdjusted,
    /// `'`.
    MoveNextLineAndShowText,
    /// `"`.
    SetSpacingMoveNextLineAndShowText,
}

/// Type of path paint operation observed in a content stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathPaintKind {
    /// `S`.
    Stroke,
    /// `s`.
    CloseAndStroke,
    /// `f`.
    FillNonzero,
    /// `F`.
    FillObsolete,
    /// `f*`.
    FillEvenOdd,
    /// `B`.
    FillAndStrokeNonzero,
    /// `B*`.
    FillAndStrokeEvenOdd,
    /// `b`.
    CloseFillAndStrokeNonzero,
    /// `b*`.
    CloseFillAndStrokeEvenOdd,
    /// `n`.
    EndPath,
}

impl PathPaintKind {
    /// Whether this path paint operation uses the stroking colour.
    #[must_use]
    pub const fn uses_stroke(self) -> bool {
        matches!(
            self,
            Self::Stroke
                | Self::CloseAndStroke
                | Self::FillAndStrokeNonzero
                | Self::FillAndStrokeEvenOdd
                | Self::CloseFillAndStrokeNonzero
                | Self::CloseFillAndStrokeEvenOdd
        )
    }

    /// Whether this path paint operation uses the nonstroking colour.
    #[must_use]
    pub const fn uses_fill(self) -> bool {
        matches!(
            self,
            Self::FillNonzero
                | Self::FillObsolete
                | Self::FillEvenOdd
                | Self::FillAndStrokeNonzero
                | Self::FillAndStrokeEvenOdd
                | Self::CloseFillAndStrokeNonzero
                | Self::CloseFillAndStrokeEvenOdd
        )
    }
}

/// Semantic event emitted for one assembled operator record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PaintOpKind {
    /// `q` saved the current graphics state.
    Save,
    /// `Q` restored the most recently saved graphics state.
    Restore,
    /// `cm` concatenated a matrix onto the CTM.
    ConcatMatrix {
        /// Operand matrix in PDF `[a b c d e f]` layout.
        matrix: [f64; 6],
    },
    /// A stroking colour operator changed state.
    ///
    /// Emitted by the direct device operators (`G`/`RG`/`K`) and by
    /// `CS` + `SC`/`SCN` resource colour selection/setting.
    SetStrokingColor {
        /// Updated stroking colour.
        color: GraphicsColor,
    },
    /// A nonstroking colour operator changed state.
    ///
    /// Emitted by the direct device operators (`g`/`rg`/`k`) and by
    /// `cs` + `sc`/`scn` resource colour selection/setting.
    SetNonstrokingColor {
        /// Updated nonstroking colour.
        color: GraphicsColor,
    },
    /// A path paint operator observed the current state.
    PathPaint {
        /// Path paint operation.
        paint: PathPaintKind,
    },
    /// `Tr` changed the active text rendering mode.
    SetTextRenderingMode {
        /// Updated text rendering mode.
        mode: TextRenderingMode,
    },
    /// A text-showing operator observed the current text state.
    TextShow {
        /// Text-showing operator.
        operator: TextShowOperator,
        /// Active text rendering mode for this text-showing operation.
        rendering_mode: TextRenderingMode,
    },
    /// `Do` invoked an `XObject` resource by name.
    XObjectInvoke {
        /// Resource name operand without the leading slash.
        name: PdfName,
    },
    /// `gs` invoked an `ExtGState` parameter dictionary by name.
    ///
    /// Carries the resource-name operand without the leading slash, mirroring
    /// [`XObjectInvoke`](Self::XObjectInvoke). This event only surfaces the
    /// invocation and its provenance; the graphics-state snapshot is left
    /// unchanged and no `ExtGState` parameter semantics (overprint, blend mode,
    /// alpha, soft mask, …) are modelled here.
    SetExtGState {
        /// Resource name operand without the leading slash.
        name: PdfName,
    },
    /// Operator outside this walker slice; state is unchanged.
    NoOp,
}

/// Ordered graphics-state event tied to source byte provenance.
///
/// `state` is shared so emitting an event is a refcount bump rather than a deep
/// snapshot copy. `PaintOpKind` keeps serde on its own.
#[derive(Debug, Clone, PartialEq)]
pub struct PaintOp {
    /// Zero-based operator-record index.
    pub index: usize,
    /// Source range for the operator token.
    pub operator_range: ByteRange,
    /// Source range for operands plus operator.
    pub record_range: ByteRange,
    /// Semantic event for this operator.
    pub kind: PaintOpKind,
    /// Shared graphics-state snapshot after the operator was applied.
    ///
    /// Consecutive no-mutation events share the same `Rc` (a refcount bump, not
    /// a deep copy); a copy-on-write happens only when an operator mutates a
    /// snapshot that is still shared with a prior event or the save stack.
    pub state: Rc<GraphicsStateSnapshot>,
}

/// Structured graphics-state walker failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphicsWalkError {
    /// Error class.
    pub kind: GraphicsWalkErrorKind,
    /// Source range to highlight for the failing operator record.
    pub range: ByteRange,
}

impl GraphicsWalkError {
    /// Create a walker error.
    #[must_use]
    pub const fn new(kind: GraphicsWalkErrorKind, range: ByteRange) -> Self {
        Self { kind, range }
    }
}

/// Structured graphics-state walker error class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GraphicsWalkErrorKind {
    /// A source range from an operator record does not address the source bytes.
    InvalidSourceRange,
    /// `Q` appeared while the graphics-state stack was empty.
    GraphicsStateStackUnderflow,
    /// A supported operator had the wrong number of operands.
    MalformedOperandCount {
        /// Operator name as source bytes.
        operator: Vec<u8>,
        /// Expected operand count.
        expected: usize,
        /// Observed operand count.
        got: usize,
    },
    /// A supported operator operand was not a single numeric lexeme.
    MalformedNumericOperand {
        /// Operator name as source bytes.
        operator: Vec<u8>,
        /// Zero-based operand index.
        operand_index: usize,
    },
    /// A supported operator operand was not a single PDF name lexeme.
    MalformedNameOperand {
        /// Operator name as source bytes.
        operator: Vec<u8>,
        /// Zero-based operand index.
        operand_index: usize,
    },
    /// A supported operator numeric operand parsed as NaN or infinity.
    NonFiniteNumericOperand {
        /// Operator name as source bytes.
        operator: Vec<u8>,
        /// Zero-based operand index.
        operand_index: usize,
    },
}

/// Stateful walker over assembled content-stream operator records.
///
/// The walker borrows a page colour-space environment
/// ([`ColorSpaceEnv`]) so `cs`/`CS` + `sc`/`scn`/`SC`/`SCN` resolve resource
/// colour spaces. A default-empty environment reproduces the device-only
/// behaviour byte-for-byte.
#[derive(Debug, Clone)]
pub struct GraphicsStateWalker<'a> {
    state: Rc<GraphicsStateSnapshot>,
    stack: Vec<Rc<GraphicsStateSnapshot>>,
    color_space_env: ColorSpaceEnv<'a>,
}

impl<'a> GraphicsStateWalker<'a> {
    /// Create a walker with the page-initial graphics state and no colour-space
    /// environment (device-only behaviour).
    #[must_use]
    pub fn new() -> Self {
        Self::with_color_space_env(ColorSpaceEnv::empty())
    }

    /// Create a walker with the page-initial graphics state that resolves
    /// `cs`/`CS` names against a borrowed page colour-space environment.
    #[must_use]
    pub fn with_color_space_env(color_space_env: ColorSpaceEnv<'a>) -> Self {
        Self {
            state: Rc::new(GraphicsStateSnapshot::page_default()),
            stack: Vec::new(),
            color_space_env,
        }
    }

    /// Return the current graphics-state snapshot.
    ///
    /// The state is interned behind an [`Rc`], so this derefs to the shared
    /// snapshot.
    #[must_use]
    pub fn state(&self) -> &GraphicsStateSnapshot {
        self.state.as_ref()
    }

    /// Borrow the current graphics state for mutation, copying-on-write.
    ///
    /// [`Rc::make_mut`] clones only when the snapshot is still shared with a
    /// prior emitted event or the save stack.
    fn state_mut(&mut self) -> &mut GraphicsStateSnapshot {
        Rc::make_mut(&mut self.state)
    }

    /// Apply one operator record and emit its post-operator event.
    ///
    /// # Errors
    ///
    /// Returns a structured error for stack underflow, invalid source ranges,
    /// malformed operand counts, malformed numeric operands, or non-finite
    /// numeric operands in the supported operator set.
    pub fn step(
        &mut self,
        source: &[u8],
        index: usize,
        record: &OperatorRecord,
    ) -> Result<PaintOp, GraphicsWalkError> {
        checked_source(source, record.range, record.range)?;
        let operator = checked_source(source, record.operator.range, record.range)?;
        let kind = self.event_kind(source, operator, record)?;
        Ok(PaintOp {
            index,
            operator_range: record.operator.range,
            record_range: record.range,
            kind,
            state: Rc::clone(&self.state),
        })
    }

    /// Dispatch the colour operators: `G`/`g`/`RG`/`rg`/`K`/`k` device colours,
    /// `CS`/`cs` colour-space selection, and `SC`/`SCN`/`sc`/`scn` colour setting.
    ///
    /// Returns `None` for any non-colour operator so
    /// [`event_kind`](Self::event_kind) handles the structural operators. Kept
    /// separate so each function stays bounded.
    fn color_event(
        &mut self,
        source: &[u8],
        operator: &[u8],
        record: &OperatorRecord,
    ) -> Option<Result<PaintOpKind, GraphicsWalkError>> {
        Some(match operator {
            b"G" => {
                self.set_stroking_device_color(source, operator, record, ColorSpace::DeviceGray, 1)
            }
            b"g" => self.set_nonstroking_device_color(
                source,
                operator,
                record,
                ColorSpace::DeviceGray,
                1,
            ),
            b"RG" => {
                self.set_stroking_device_color(source, operator, record, ColorSpace::DeviceRgb, 3)
            }
            b"rg" => self.set_nonstroking_device_color(
                source,
                operator,
                record,
                ColorSpace::DeviceRgb,
                3,
            ),
            b"K" => {
                self.set_stroking_device_color(source, operator, record, ColorSpace::DeviceCmyk, 4)
            }
            b"k" => self.set_nonstroking_device_color(
                source,
                operator,
                record,
                ColorSpace::DeviceCmyk,
                4,
            ),
            b"CS" => self.select_color_space(source, operator, record, ColorSide::Stroking),
            b"cs" => self.select_color_space(source, operator, record, ColorSide::Nonstroking),
            b"SC" | b"SCN" => self.set_color_value(source, operator, record, ColorSide::Stroking),
            b"sc" | b"scn" => {
                self.set_color_value(source, operator, record, ColorSide::Nonstroking)
            }
            _ => return None,
        })
    }

    fn event_kind(
        &mut self,
        source: &[u8],
        operator: &[u8],
        record: &OperatorRecord,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        if let Some(result) = self.color_event(source, operator, record) {
            return result;
        }
        match operator {
            b"q" => {
                expect_operands(operator, record, 0)?;
                self.stack.push(Rc::clone(&self.state));
                Ok(PaintOpKind::Save)
            }
            b"Q" => {
                expect_operands(operator, record, 0)?;
                let Some(previous) = self.stack.pop() else {
                    return Err(GraphicsWalkError::new(
                        GraphicsWalkErrorKind::GraphicsStateStackUnderflow,
                        record.range,
                    ));
                };
                self.state = previous;
                Ok(PaintOpKind::Restore)
            }
            b"cm" => {
                let matrix = numeric_operands(source, operator, record, 6)?;
                let ctm = concat_matrix(matrix, self.state.ctm);
                self.state_mut().ctm = ctm;
                Ok(PaintOpKind::ConcatMatrix { matrix })
            }
            b"Tr" => self.set_text_rendering_mode(source, operator, record),
            b"S" => Self::path_paint(operator, record, PathPaintKind::Stroke),
            b"s" => Self::path_paint(operator, record, PathPaintKind::CloseAndStroke),
            b"f" => Self::path_paint(operator, record, PathPaintKind::FillNonzero),
            b"F" => Self::path_paint(operator, record, PathPaintKind::FillObsolete),
            b"f*" => Self::path_paint(operator, record, PathPaintKind::FillEvenOdd),
            b"B" => Self::path_paint(operator, record, PathPaintKind::FillAndStrokeNonzero),
            b"B*" => Self::path_paint(operator, record, PathPaintKind::FillAndStrokeEvenOdd),
            b"b" => Self::path_paint(operator, record, PathPaintKind::CloseFillAndStrokeNonzero),
            b"b*" => Self::path_paint(operator, record, PathPaintKind::CloseFillAndStrokeEvenOdd),
            b"n" => Self::path_paint(operator, record, PathPaintKind::EndPath),
            b"Tj" => Self::text_show(
                operator,
                record,
                TextShowOperator::ShowText,
                1,
                self.state.text_rendering_mode,
            ),
            b"TJ" => Self::text_show(
                operator,
                record,
                TextShowOperator::ShowTextAdjusted,
                1,
                self.state.text_rendering_mode,
            ),
            b"'" => Self::text_show(
                operator,
                record,
                TextShowOperator::MoveNextLineAndShowText,
                1,
                self.state.text_rendering_mode,
            ),
            b"\"" => Self::text_show(
                operator,
                record,
                TextShowOperator::SetSpacingMoveNextLineAndShowText,
                3,
                self.state.text_rendering_mode,
            ),
            b"Do" => Ok(PaintOpKind::XObjectInvoke {
                name: name_operand(source, operator, record)?,
            }),
            b"gs" => Ok(PaintOpKind::SetExtGState {
                name: name_operand(source, operator, record)?,
            }),
            _ => Ok(PaintOpKind::NoOp),
        }
    }

    fn set_stroking_device_color(
        &mut self,
        source: &[u8],
        operator: &[u8],
        record: &OperatorRecord,
        space: ColorSpace,
        count: usize,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        let color = sourced_device_color(source, operator, record, space, count)?;
        self.state_mut().stroking_color = color.clone();
        Ok(PaintOpKind::SetStrokingColor { color })
    }

    fn set_nonstroking_device_color(
        &mut self,
        source: &[u8],
        operator: &[u8],
        record: &OperatorRecord,
        space: ColorSpace,
        count: usize,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        let color = sourced_device_color(source, operator, record, space, count)?;
        self.state_mut().nonstroking_color = color.clone();
        Ok(PaintOpKind::SetNonstrokingColor { color })
    }

    /// Handle `cs`/`CS`: select the current colour space by resource name and
    /// reset the current colour to that space's implied initial colour.
    ///
    /// The name is resolved against the borrowed page colour-space environment.
    /// A resolved resource space adopts its REAL source family (never collapsed
    /// to a device space); an unresolved name is reported honestly as
    /// [`ColorSpace::Resource`] (never a bare `Unknown`), so the audit surfaces
    /// it as a coverage gap rather than a fabricated device colour.
    fn select_color_space(
        &mut self,
        source: &[u8],
        operator: &[u8],
        record: &OperatorRecord,
        side: ColorSide,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        let name = name_operand(source, operator, record)?;
        let color = self.selected_color(&name, record.range);
        Ok(self.apply_color(side, color))
    }

    /// Handle `sc`/`scn`/`SC`/`SCN`: set a colour value in the current space.
    ///
    /// The numeric operands become the colour components in the current space;
    /// the space, resource name, and spot colorant carried by `cs`/`CS` are
    /// preserved. A trailing name operand (pattern colour) is recorded as the
    /// resource name but not otherwise modelled in this slice.
    fn set_color_value(
        &mut self,
        source: &[u8],
        operator: &[u8],
        record: &OperatorRecord,
        side: ColorSide,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        let (components, pattern_name) = color_operands(source, operator, record)?;
        let current = self.side_color(side);
        let mut color = current.clone();
        color.components = components;
        if let Some(name) = pattern_name {
            color.resource_name = Some(name);
        }
        color.source = Some(record.range);
        Ok(self.apply_color(side, color))
    }

    /// Build the [`GraphicsColor`] a `cs`/`CS` selection establishes.
    fn selected_color(&self, name: &PdfName, range: ByteRange) -> GraphicsColor {
        self.color_space_env.resolve(name).map_or_else(
            || GraphicsColor {
                space: ColorSpace::Resource(name.clone()),
                components: Vec::new(),
                resource_name: Some(name.clone()),
                spot_name: None,
                source: Some(range),
            },
            |resource| resource_initial_color(resource, name.clone(), range),
        )
    }

    fn side_color(&self, side: ColorSide) -> &GraphicsColor {
        match side {
            ColorSide::Stroking => &self.state.stroking_color,
            ColorSide::Nonstroking => &self.state.nonstroking_color,
        }
    }

    fn apply_color(&mut self, side: ColorSide, color: GraphicsColor) -> PaintOpKind {
        match side {
            ColorSide::Stroking => {
                self.state_mut().stroking_color = color.clone();
                PaintOpKind::SetStrokingColor { color }
            }
            ColorSide::Nonstroking => {
                self.state_mut().nonstroking_color = color.clone();
                PaintOpKind::SetNonstrokingColor { color }
            }
        }
    }

    fn set_text_rendering_mode(
        &mut self,
        source: &[u8],
        operator: &[u8],
        record: &OperatorRecord,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        let value = integer_operand(source, operator, record)?;
        let mode = TextRenderingMode::from_pdf_value(value);
        self.state_mut().text_rendering_mode = mode;
        Ok(PaintOpKind::SetTextRenderingMode { mode })
    }

    fn path_paint(
        operator: &[u8],
        record: &OperatorRecord,
        paint: PathPaintKind,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        expect_operands(operator, record, 0)?;
        Ok(PaintOpKind::PathPaint { paint })
    }

    fn text_show(
        operator: &[u8],
        record: &OperatorRecord,
        show_operator: TextShowOperator,
        expected_operands: usize,
        rendering_mode: TextRenderingMode,
    ) -> Result<PaintOpKind, GraphicsWalkError> {
        expect_operands(operator, record, expected_operands)?;
        Ok(PaintOpKind::TextShow {
            operator: show_operator,
            rendering_mode,
        })
    }
}

impl Default for GraphicsStateWalker<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Which side of the graphics state a colour operator targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorSide {
    /// Stroking colour (`CS`/`SC`/`SCN`).
    Stroking,
    /// Nonstroking colour (`cs`/`sc`/`scn`).
    Nonstroking,
}

/// Resolve a device-colour operator and stamp its own record range as the
/// colour source.
///
/// Both the stroking and nonstroking setters share this: the colour-setting
/// operator records where it was set so paint/text-show observations can map
/// the colour back to those bytes.
fn sourced_device_color(
    source: &[u8],
    operator: &[u8],
    record: &OperatorRecord,
    space: ColorSpace,
    count: usize,
) -> Result<GraphicsColor, GraphicsWalkError> {
    let mut color = device_color(source, operator, record, space, count)?;
    color.source = Some(record.range);
    Ok(color)
}

/// Build the colour a `cs`/`CS` selection establishes for a resolved resource.
///
/// The observed space is the resource's REAL source family (`ICCBased` with `N=4`
/// stays `IccBased`, not `DeviceCmyk`; `Separation`/`DeviceN` keep the special
/// space, never their alternate). The current colour is the space's implied
/// initial colour so a paint/text-show observed before any `sc`/`scn` reports a
/// real colour rather than a stale device colour.
fn resource_initial_color(
    resource: &ColorSpaceResource,
    name: PdfName,
    range: ByteRange,
) -> GraphicsColor {
    GraphicsColor {
        space: resource.space.clone(),
        components: resource.initial_color(),
        resource_name: Some(name),
        spot_name: resource.spot_name(),
        source: Some(range),
    }
}

/// Walk assembled operator records into ordered graphics-state events.
///
/// Unsupported operators emit explicit no-op events and leave state unchanged.
///
/// # Errors
///
/// Returns a structured walker error for malformed records in the supported
/// operator set or invalid source ranges.
pub fn walk_graphics_state(
    source: &[u8],
    records: &[OperatorRecord],
) -> Result<Vec<PaintOp>, GraphicsWalkError> {
    let mut walker = GraphicsStateWalker::new();
    records
        .iter()
        .enumerate()
        .map(|(index, record)| walker.step(source, index, record))
        .collect()
}
