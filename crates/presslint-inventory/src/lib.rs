//! Page-object inventory model and graphics-state observations.

#![forbid(unsafe_code)]

mod digest;
mod inventory;
#[cfg(test)]
mod tests;

pub use inventory::{
    Inventory, InventoryEntry, build_form_inventory, build_image_inventory, build_inventory,
    build_inventory_with_color_space_env, build_text_inventory, build_vector_inventory,
    form_inventory_from_graphics_events, image_inventory_from_graphics_events,
    inventory_from_graphics_events, text_inventory_from_graphics_events,
    vector_inventory_from_graphics_events,
};
pub use presslint_paint::{
    ColorSpaceEnv, ColorSpaceResource, GraphicsColor, GraphicsStateEvent, GraphicsStateEventKind,
    GraphicsStateSnapshot, GraphicsStateWalker, GraphicsWalkError, GraphicsWalkErrorKind,
    PathPaintKind, TextRenderingMode, TextShowOperator, walk_graphics_state,
};
