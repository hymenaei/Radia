mod avatar;
mod chat;
mod object;
mod pbr;

pub(super) use avatar::{AvatarAppearanceMessage, SitTargetUpdate, decode_avatar_appearance_update, decode_sit_target_update};
pub(crate) use chat::{ChatMessage, display_message, format_avatar_name};
pub(super) use object::{ObjectBatch, ObjectMediaRequest, ObjectOperation, decode_object_message, object_media_requests};

use protocol::{FieldValue, SchemaMessage};

pub(super) fn schema_field<'a>(message: &'a SchemaMessage, block_name: &str, field_name: &str) -> Option<&'a FieldValue> {
    let (block_schema, block_values) =
        message.schema().blocks.iter().zip(message.blocks()).find(|(schema, _)| schema.name == block_name)?;
    let instance = block_values.instances.first()?;
    let field_index = block_schema.fields.iter().position(|field| field.name == field_name)?;
    instance.fields.get(field_index)
}
