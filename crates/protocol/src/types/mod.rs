//! Typed payload values embedded in protocol messages.

pub mod object_shape;
pub mod render_material;
pub mod texture_entry;

pub use object_shape::ObjectShape;
pub use render_material::{MAX_RENDER_MATERIAL_BINDINGS, RenderMaterialBinding, RenderMaterialError, decode_render_material_bindings};
pub use texture_entry::{MAX_TEXTURE_FACES, TextureEntry, TextureEntryError, TextureFace, decode_texture_entry};
