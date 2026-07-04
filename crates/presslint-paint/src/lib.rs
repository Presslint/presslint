//! Graphics-state walker and paint-program spine for presslint.
//!
//! This crate owns paint/graphics-state semantics: the source-preserving
//! graphics-state walker, its borrowed page colour-space environment, and the
//! private operand helpers it interprets. It depends only on `presslint-types`
//! and `presslint-syntax`, so consumers such as `presslint-inventory` (and, later,
//! the rewriter and `presslint-render`) can build on the same paint model without
//! a dependency cycle.

#![forbid(unsafe_code)]

mod color_space_env;
mod operands;
mod walker;

pub use color_space_env::{ColorSpaceEnv, ColorSpaceResource};
pub use walker::{
    GraphicsColor, GraphicsStateEvent, GraphicsStateEventKind, GraphicsStateSnapshot,
    GraphicsStateWalker, GraphicsWalkError, GraphicsWalkErrorKind, PathPaintKind,
    TextRenderingMode, TextShowOperator, walk_graphics_state,
};
