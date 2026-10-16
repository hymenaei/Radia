use base::UUID;
use protocol::{
    FieldValue, SchemaMessage,
    lludp::Message,
    types::{MAX_TEXTURE_FACES, decode_texture_entry},
};
use world::{
    AssetId, AvatarAppearanceUpdate, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex, MAX_AVATAR_TEXTURE_SLOTS,
    MAX_AVATAR_VISUAL_PARAMS, SitTarget,
};

use super::schema_field;

const MAX_AVATAR_TEXTURE_ENTRY_BYTES: usize = 4095;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SitTargetUpdate {
    pub(crate) object_id: UUID,
    pub(crate) target: SitTarget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AvatarAppearanceMessage {
    pub(crate) object_id: UUID,
    pub(crate) update: AvatarAppearanceUpdate,
}

pub(crate) fn decode_avatar_appearance_update(message: &Message) -> Option<AvatarAppearanceMessage> {
    let Message::Schema(message) = message else { return None };
    if message.schema().name != "AvatarAppearance" {
        return None;
    }

    let object_id = match schema_field(message, "Sender", "ID")? {
        FieldValue::UUID(id) if !id.is_nil() => *id,
        _ => return None,
    };
    let visual_param_values = schema_block(message, "VisualParam")?
        .instances
        .iter()
        .map(|instance| match instance.fields.first()? {
            FieldValue::U8(value) => Some(*value),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    if visual_param_values.len() <= 1 || visual_param_values.len() > MAX_AVATAR_VISUAL_PARAMS {
        return None;
    }
    let visual_params = visual_param_values
        .into_iter()
        .enumerate()
        .map(|(index, value)| Some((AvatarVisualParamIndex::new(index as u8)?, value)))
        .collect::<Option<Vec<_>>>()?;

    let texture_slots = match schema_field(message, "ObjectData", "TextureEntry") {
        Some(FieldValue::Bytes(bytes)) => decode_avatar_textures(bytes),
        _ => None,
    };
    let appearance_version = schema_optional_field(message, "AppearanceData", "AppearanceVersion").and_then(|field| match field {
        FieldValue::U8(version) => Some(*version),
        _ => None,
    });
    let appearance_version = appearance_version.map(|version| version.max(1));
    if appearance_version.is_some_and(|version| version > 1) {
        return None;
    }
    let cof_version = schema_optional_field(message, "AppearanceData", "CofVersion").and_then(|field| match field {
        FieldValue::S32(version) => Some(*version),
        _ => None,
    });

    let update = AvatarAppearanceUpdate::new(appearance_version, cof_version, visual_params, texture_slots).ok()?;
    Some(AvatarAppearanceMessage { object_id, update })
}

pub(crate) fn decode_sit_target_update(message: &Message) -> Option<SitTargetUpdate> {
    let Message::Schema(message) = message else { return None };
    if message.schema().name != "AvatarSitResponse" {
        return None;
    }

    let object_id = match schema_field(message, "SitObject", "ID")? {
        FieldValue::UUID(id) if !id.is_nil() => *id,
        _ => return None,
    };
    let position = match schema_field(message, "SitTransform", "SitPosition")? {
        FieldValue::Vector3(position) => *position,
        _ => return None,
    };
    let rotation = match schema_field(message, "SitTransform", "SitRotation")? {
        FieldValue::Quaternion(rotation) => *rotation,
        _ => return None,
    };

    Some(SitTargetUpdate { object_id, target: SitTarget { position, rotation } })
}

fn schema_optional_field<'a>(message: &'a SchemaMessage, block_name: &str, field_name: &str) -> Option<&'a FieldValue> {
    let block_index = message.schema().blocks.iter().position(|block| block.name == block_name)?;
    let block_schema = &message.schema().blocks[block_index];
    let field_index = block_schema.fields.iter().position(|field| field.name == field_name)?;
    let instance = message.blocks().get(block_index)?.instances.first()?;
    if !message.field_was_supplied(block_index, 0, field_index)? {
        return None;
    }
    instance.fields.get(field_index)
}

fn schema_block<'a>(message: &'a SchemaMessage, block_name: &str) -> Option<&'a protocol::BlockValues> {
    message.schema().blocks.iter().zip(message.blocks()).find(|(schema, _)| schema.name == block_name).map(|(_, values)| values)
}

fn decode_avatar_textures(bytes: &[u8]) -> Option<AvatarTextureSlots> {
    if bytes.len() > MAX_AVATAR_TEXTURE_ENTRY_BYTES {
        return None;
    }
    let entry = decode_texture_entry(bytes).ok().flatten()?;
    let mut values = [None; MAX_AVATAR_TEXTURE_SLOTS];
    for (index, value) in values.iter_mut().enumerate() {
        let slot = AvatarTextureSlot::new(index as u8)?;
        let texture_id = entry.face(slot.index())?.texture_id;
        if !texture_id.is_nil() {
            *value = Some(AssetId::new(texture_id));
        }
    }
    debug_assert_eq!(MAX_AVATAR_TEXTURE_SLOTS, MAX_TEXTURE_FACES);
    AvatarTextureSlots::new(values).ok()
}

#[cfg(test)]
mod tests {
    use base::UUID;
    use math::{Quat, Vec3};
    use protocol::{BlockValues, FieldValue, SchemaMessage, lludp::Message, types::decode_texture_entry};
    use world::{AssetId, AvatarTextureSlot, AvatarVisualParamIndex};

    use super::{decode_avatar_appearance_update, decode_sit_target_update};

    fn avatar_message(texture_entry: Vec<u8>, params: &[u8], appearance: Option<(u8, i32)>) -> Message {
        let schema = SchemaMessage::schema_by_name("AvatarAppearance").expect("avatar appearance schema");
        let appearance_data = appearance.map_or_else(
            || BlockValues::new(Vec::new()),
            |(version, cof)| BlockValues::single(vec![FieldValue::U8(version), FieldValue::S32(cof), FieldValue::U32(0)]),
        );
        let mut blocks = vec![
            BlockValues::single(vec![FieldValue::UUID(UUID::from_u128(7)), FieldValue::Bool(false)]),
            BlockValues::single(vec![FieldValue::Bytes(texture_entry)]),
            BlockValues::new(params.iter().copied().map(|value| protocol::BlockInstance::new(vec![FieldValue::U8(value)])).collect()),
            appearance_data,
            BlockValues::new(Vec::new()),
            BlockValues::new(Vec::new()),
        ];
        Message::Schema(SchemaMessage::new(schema, std::mem::take(&mut blocks)).expect("valid avatar appearance message"))
    }

    fn texture_entry(texture_id: UUID) -> Vec<u8> {
        let mut bytes = texture_id.as_bytes().to_vec();
        bytes.push(0);
        bytes.extend_from_slice(&[255; 4]);
        bytes.push(0);
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&1.0_f32.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0_i16.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0_i16.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&0_i16.to_le_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&[0; 6]);
        bytes.extend_from_slice(&[0; 16]);
        bytes
    }

    #[test]
    fn sit_response_maps_target() {
        let schema = SchemaMessage::schema_by_name("AvatarSitResponse").expect("sit response schema");
        let message = SchemaMessage::new(
            schema,
            vec![
                BlockValues::single(vec![FieldValue::UUID(UUID::from_u128(7))]),
                BlockValues::single(vec![
                    FieldValue::Bool(true),
                    FieldValue::Vector3(Vec3::new(1.0, 2.0, 3.0)),
                    FieldValue::Quaternion(Quat::IDENTITY),
                    FieldValue::Vector3(Vec3::ZERO),
                    FieldValue::Vector3(Vec3::ZERO),
                    FieldValue::Bool(false),
                ]),
            ],
        )
        .expect("valid schema message");
        let update = decode_sit_target_update(&Message::Schema(message)).expect("sit target update");

        assert_eq!(update.object_id, UUID::from_u128(7));
        assert_eq!(update.target.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(update.target.rotation, Quat::IDENTITY);
    }

    #[test]
    fn non_sit_messages_are_ignored() {
        assert_eq!(decode_sit_target_update(&Message::StartPingCheck(Default::default())), None);
    }

    #[test]
    fn visual_params_keep_positions() {
        let message = avatar_message(texture_entry(UUID::from_u128(88)), &[12, 34], Some((1, 50)));
        let update = decode_avatar_appearance_update(&message).expect("avatar appearance update");

        assert_eq!(update.object_id, UUID::from_u128(7));
        assert_eq!(
            update.update.visual_params().collect::<Vec<_>>(),
            [(AvatarVisualParamIndex::new(0).unwrap(), 12), (AvatarVisualParamIndex::new(1).unwrap(), 34)]
        );
        assert_eq!(update.update.appearance_version(), Some(1));
        assert_eq!(update.update.cof_version(), Some(50));
        assert_eq!(
            update.update.texture_slots().and_then(|slots| slots.texture(AvatarTextureSlot::HEAD_BAKED)),
            Some(AssetId::new(UUID::from_u128(88)))
        );
    }

    #[test]
    fn bad_texture_keeps_visual_params() {
        let message = avatar_message(vec![0], &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("avatar update without textures");
        assert_eq!(update.update.texture_slots(), None);
        assert_eq!(update.update.visual_params().count(), 2);
    }

    #[test]
    fn empty_texture_is_omitted() {
        let message = avatar_message(Vec::new(), &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("avatar update without textures");
        assert_eq!(update.update.texture_slots(), None);
    }

    #[test]
    fn nil_texture_clears_slots() {
        let message = avatar_message(texture_entry(UUID::nil()), &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("avatar update with empty texture slots");
        let slots = update.update.texture_slots().expect("TextureEntry replaces avatar slots");
        assert_eq!(slots.texture(AvatarTextureSlot::HEAD_BAKED), None);
    }

    #[test]
    fn oversized_texture_is_omitted() {
        let mut bytes = texture_entry(UUID::from_u128(88));
        while bytes.len() <= super::MAX_AVATAR_TEXTURE_ENTRY_BYTES {
            bytes.push(1);
            bytes.extend_from_slice(&[0; 16]);
        }
        assert!(decode_texture_entry(&bytes).expect("valid TextureEntry").is_some());

        let message = avatar_message(bytes, &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("avatar update without textures");
        assert_eq!(update.update.texture_slots(), None);
        assert_eq!(update.update.visual_params().count(), 2);
    }

    #[test]
    fn short_param_list_is_ignored() {
        assert_eq!(decode_avatar_appearance_update(&avatar_message(Vec::new(), &[12], None)), None);
    }

    #[test]
    fn unsupported_version_is_ignored() {
        assert_eq!(decode_avatar_appearance_update(&avatar_message(Vec::new(), &[12, 34], Some((2, 50)))), None);
    }
}
