//! Page-object inventory model and graphics-state observations.

#![forbid(unsafe_code)]

mod digest;
mod inventory;
#[cfg(test)]
mod tests;

pub use inventory::{
    Inventory, InventoryEntry, build_form_inventory, build_image_inventory, build_inventory,
    build_inventory_with_color_space_env, build_text_inventory, build_vector_inventory,
    expanded_entry_identity, form_inventory_from_graphics_events,
    image_inventory_from_graphics_events, inventory_from_graphics_events,
    text_inventory_from_graphics_events, vector_inventory_from_graphics_events,
};
pub use presslint_paint::{
    AlphaClass, BlendModeClass, ColorSpaceEnv, ColorSpaceResource, DecodedRange, ExtGStateEnv,
    ExtGStateParams, ExtGStateResource, GraphicsColor, GraphicsExtGStateSnapshot,
    GraphicsStateSnapshot, GraphicsStateWalker, GraphicsWalkError, GraphicsWalkErrorKind, GsParam,
    MutationClass, OverprintMode, PaintOp, PaintOpKind, PathPaintKind, SoftMaskClass, SourceRange,
    TextRenderingMode, TextShowOperator, walk_graphics_state,
};
