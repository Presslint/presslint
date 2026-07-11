//! Graphics-state walker and paint-program spine for presslint.
//!
//! This crate owns paint/graphics-state semantics: the source-preserving
//! graphics-state walker, its borrowed page colour-space environment, and the
//! private operand helpers it interprets. It depends only on `presslint-types`
//! and `presslint-syntax`, so consumers such as `presslint-inventory` (and, later,
//! the rewriter and `presslint-render`) can build on the same paint model without
//! a dependency cycle.

#![forbid(unsafe_code)]

mod call_machine;
mod color_space_env;
mod extgstate_env;
mod flat_projection;
mod mutation_class;
mod operands;
mod paint_program;
mod provenance;
mod walker;

#[cfg(test)]
mod tests;

pub use call_machine::{
    CallEvent, CallMachine, CallSite, FormResolver, PaintSubProgram, ResolveForm,
};
pub use color_space_env::{ColorSpaceEnv, ColorSpaceResource};
pub use extgstate_env::{
    AlphaClass, BlendModeClass, ExtGStateEnv, ExtGStateParams, ExtGStateResource,
    GraphicsExtGStateSnapshot, GsParam, OverprintMode, SoftMaskClass,
};
pub use flat_projection::{FlatPaintOp, flat_call_events};
pub use mutation_class::MutationClass;
pub use paint_program::{PaintOps, PaintProgram};
pub use provenance::{DecodedRange, SourceRange};
pub use walker::{
    FontSelectionState, GraphicsColor, GraphicsStateSnapshot, GraphicsStateWalker,
    GraphicsWalkError, GraphicsWalkErrorKind, PaintOp, PaintOpKind, PathPaintKind,
    TextRenderingMode, TextShowOperator, walk_graphics_state,
};
