//! Pure mappings from decoded LLUDP messages to viewer-domain values.

use protocol::{FieldValue, SchemaMessage};

mod avatar;
mod chat;
mod gltf;
mod object;

pub(super) fn schema_field<'a>(message: &'a SchemaMessage, block_name: &str, field_name: &str) -> Option<&'a FieldValue> {
    let (block_schema, block_values) =
        message.schema().blocks.iter().zip(message.blocks()).find(|(schema, _)| schema.name == block_name)?;
    let instance = block_values.instances.first()?;
    let field_index = block_schema.fields.iter().position(|field| field.name == field_name)?;
    instance.fields.get(field_index)
}

pub(super) use avatar::{AvatarAppearanceMessage, SitTargetUpdate, decode_avatar_appearance_update, decode_sit_target_update};
pub(crate) use chat::{ChatMessage, display_message, format_avatar_name};
pub(super) use object::{ObjectBatch, ObjectMediaRequest, ObjectOperation, decode_object_message, object_media_requests};
