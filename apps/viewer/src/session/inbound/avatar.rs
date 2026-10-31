use protocol::{
    lludp::{FieldValue, Message},
    types::{MAX_TEXTURE_FACES, decode_texture_entry},
};
use utils::AssetId;
use uuid::Uuid;
use world::{
    AvatarAppearanceUpdate, AvatarTextureSlot, AvatarTextureSlots, AvatarVisualParamIndex, MAX_AVATAR_TEXTURE_SLOTS,
    MAX_AVATAR_VISUAL_PARAMS, SitTarget,
};

use crate::session::agent_control::SitCameraOverride;

const MAX_AVATAR_TEXTURE_ENTRY_BYTES: usize = 4095;
const SIT_GROUND_CONSTRAINED_ANIMATION: Uuid = Uuid::from_u128(0x1a2b_d58e_87ff_0df8_0b4c_53e0_47b0_bb6e);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SitAnimationUpdate {
    pub(crate) agent_id: Uuid,
    pub(crate) ground_sitting: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SitTargetUpdate {
    pub(crate) object_id: Uuid,
    pub(crate) target: SitTarget,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct SitResponseUpdate {
    pub(crate) object_id: Uuid,
    pub(crate) target: Option<SitTarget>,
    pub(crate) auto_pilot: bool,
    pub(crate) force_mouselook: bool,
    pub(crate) camera_override: Option<SitCameraOverride>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AvatarAppearanceMessage {
    pub(crate) object_id: Uuid,
    pub(crate) update: AvatarAppearanceUpdate,
    pub(crate) invalid_fields: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodeError {
    InvalidMessage,
}

pub(crate) fn decode_avatar_appearance_update(message: &Message) -> Result<Option<AvatarAppearanceMessage>, DecodeError> {
    let Message::AvatarAppearance(message) = message else { return Ok(None) };
    let object_id = message.sender.id;
    if object_id.is_nil() {
        return Err(DecodeError::InvalidMessage);
    }
    let visual_param_values = message.visual_params.iter().map(|value| value.param_value).collect::<Vec<_>>();
    if visual_param_values.len() <= 1 || visual_param_values.len() > MAX_AVATAR_VISUAL_PARAMS {
        return Err(DecodeError::InvalidMessage);
    }
    let visual_params = visual_param_values
        .into_iter()
        .enumerate()
        .map(|(index, value)| Some((AvatarVisualParamIndex::new(index as u8)?, value)))
        .collect::<Option<Vec<_>>>()
        .ok_or(DecodeError::InvalidMessage)?;

    let mut invalid_fields = 0;
    let texture_slots = match decode_avatar_textures(&message.object_data.texture_entry) {
        Ok(slots) => slots,
        Err(()) => {
            invalid_fields += 1;
            None
        }
    };
    let appearance_data = message.appearance_data.first();
    let appearance_version = appearance_data.map(|data| data.appearance_version);
    let appearance_version = appearance_version.map(|version| version.max(1));
    if appearance_version.is_some_and(|version| version > 1) {
        return Err(DecodeError::InvalidMessage);
    }
    let cof_version = appearance_data.map(|data| data.cof_version);

    let update = AvatarAppearanceUpdate::new(appearance_version, cof_version, visual_params, texture_slots)
        .map_err(|_| DecodeError::InvalidMessage)?;
    Ok(Some(AvatarAppearanceMessage { object_id, update, invalid_fields }))
}

pub(crate) fn decode_sit_target_update(message: &Message) -> Option<SitTargetUpdate> {
    let response = decode_sit_response(message)?;
    let target = response.target?;
    Some(SitTargetUpdate { object_id: response.object_id, target })
}

pub(crate) fn decode_sit_response(message: &Message) -> Option<SitResponseUpdate> {
    let Message::AvatarSitResponse(message) = message else { return None };
    let object_id = message.sit_object.id?;
    if object_id.is_nil() {
        return None;
    }
    let transform = &message.sit_transform;
    let target = transform.sit_position.zip(transform.sit_rotation).and_then(|(position, rotation)| {
        (position.is_finite() && rotation.is_finite() && rotation.length_squared() > 0.0).then_some(SitTarget { position, rotation })
    });
    let camera_override = transform.camera_eye_offset.zip(transform.camera_at_offset).and_then(|(eye_offset, at_offset)| {
        (eye_offset.is_finite() && at_offset.is_finite() && eye_offset.distance(at_offset) > 0.001)
            .then_some(SitCameraOverride { eye_offset, at_offset })
    });

    Some(SitResponseUpdate {
        object_id,
        target,
        auto_pilot: transform.auto_pilot.unwrap_or(false),
        force_mouselook: transform.force_mouselook.unwrap_or(false),
        camera_override,
    })
}

pub(crate) fn decode_sit_animation(message: &Message) -> Option<SitAnimationUpdate> {
    if message.name() != "AvatarAnimation" {
        return None;
    }
    let Message::Schema(message) = message else { return None };
    let sender = message.blocks().first()?.instances.first()?.fields.first()?;
    let FieldValue::Uuid(agent_id) = sender else { return None };
    if agent_id.is_nil() {
        return None;
    }
    let ground_sitting = message.blocks().get(1)?.instances.iter().try_fold(false, |sitting, animation| {
        let [FieldValue::Uuid(animation_id), FieldValue::S32(_)] = animation.fields.as_slice() else { return None };
        Some(sitting || *animation_id == SIT_GROUND_CONSTRAINED_ANIMATION)
    })?;
    Some(SitAnimationUpdate { agent_id: *agent_id, ground_sitting })
}

fn decode_avatar_textures(bytes: &[u8]) -> Result<Option<AvatarTextureSlots>, ()> {
    if bytes.len() > MAX_AVATAR_TEXTURE_ENTRY_BYTES {
        return Err(());
    }
    let Some(entry) = decode_texture_entry(bytes).map_err(|_| ())? else { return Ok(None) };
    let mut values = [None; MAX_AVATAR_TEXTURE_SLOTS];
    for (index, value) in values.iter_mut().enumerate() {
        let slot = AvatarTextureSlot::new(index as u8).ok_or(())?;
        let texture_id = entry.face(slot.index()).ok_or(())?.texture_id;
        if !texture_id.is_nil() {
            *value = Some(AssetId::new(texture_id));
        }
    }
    debug_assert_eq!(MAX_AVATAR_TEXTURE_SLOTS, MAX_TEXTURE_FACES);
    AvatarTextureSlots::new(values).map(Some).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use math::{Quat, Vec3};
    use protocol::{
        lludp::{BlockValues, FieldValue, Message, Packet, SchemaMessage},
        messages::{
            AvatarAppearance, AvatarSitResponse,
            agent::{
                avatar_appearance::{AppearanceData, AttachmentBlock, ObjectData, Sender, VisualParam},
                avatar_sit_response::{SitObject, SitTransform},
            },
        },
        types::decode_texture_entry,
    };
    use utils::AssetId;
    use uuid::Uuid;
    use world::{AvatarTextureSlot, AvatarVisualParamIndex};

    use super::{decode_avatar_appearance_update, decode_sit_animation, decode_sit_response, decode_sit_target_update};

    fn avatar_message(texture_entry: Vec<u8>, params: &[u8], appearance: Option<(u8, i32)>) -> Message {
        Message::AvatarAppearance(AvatarAppearance {
            sender: Sender { id: Uuid::from_u128(7), is_trial: false },
            object_data: ObjectData { texture_entry },
            visual_params: params.iter().copied().map(|param_value| VisualParam { param_value }).collect(),
            appearance_data: appearance
                .map(|(appearance_version, cof_version)| vec![AppearanceData { appearance_version, cof_version, flags: 0 }])
                .unwrap_or_default(),
            appearance_hover: Vec::new(),
            attachment_blocks: Some(Vec::<AttachmentBlock>::new()),
        })
    }

    fn texture_entry(texture_id: Uuid) -> Vec<u8> {
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

    fn avatar_animation(agent_id: Uuid, animations: &[Uuid]) -> Message {
        let schema = SchemaMessage::schema_by_name("AvatarAnimation").expect("AvatarAnimation schema");
        let animations = animations
            .iter()
            .enumerate()
            .map(|(sequence, animation)| {
                protocol::lludp::BlockInstance::new(vec![FieldValue::Uuid(*animation), FieldValue::S32(sequence as i32)])
            })
            .collect();
        Message::Schema(
            SchemaMessage::new(
                schema,
                vec![
                    BlockValues::single(vec![FieldValue::Uuid(agent_id)]),
                    BlockValues::new(animations),
                    BlockValues::new(Vec::new()),
                    BlockValues::new(Vec::new()),
                ],
            )
            .expect("valid AvatarAnimation message"),
        )
    }

    #[test]
    fn sit_response_maps_target() {
        let message = AvatarSitResponse {
            sit_object: SitObject { id: Some(Uuid::from_u128(7)) },
            sit_transform: SitTransform {
                auto_pilot: Some(true),
                sit_position: Some(Vec3::new(1.0, 2.0, 3.0)),
                sit_rotation: Some(Quat::IDENTITY),
                camera_eye_offset: Some(Vec3::ZERO),
                camera_at_offset: Some(Vec3::ZERO),
                force_mouselook: Some(false),
            },
        };
        let update = decode_sit_target_update(&Message::AvatarSitResponse(message)).expect("sit target update");

        assert_eq!(update.object_id, Uuid::from_u128(7));
        assert_eq!(update.target.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(update.target.rotation, Quat::IDENTITY);

        let response = decode_sit_response(&Message::AvatarSitResponse(AvatarSitResponse {
            sit_object: SitObject { id: Some(Uuid::from_u128(7)) },
            sit_transform: SitTransform {
                auto_pilot: Some(true),
                sit_position: Some(Vec3::new(1.0, 2.0, 3.0)),
                sit_rotation: Some(Quat::IDENTITY),
                camera_eye_offset: Some(Vec3::X),
                camera_at_offset: Some(Vec3::Y),
                force_mouselook: Some(true),
            },
        }))
        .expect("sit response");
        assert!(response.auto_pilot);
        assert!(response.force_mouselook);
        let camera = response.camera_override.unwrap();
        assert_eq!(camera.eye_offset, Vec3::X);
        assert_eq!(camera.at_offset, Vec3::Y);
    }

    #[test]
    fn ground_sit_animation_decodes_from_wire() {
        let agent_id = Uuid::from_u128(9);
        let ground_sit = Uuid::from_u128(0x1a2b_d58e_87ff_0df8_0b4c_53e0_47b0_bb6e);
        let sit = Uuid::from_u128(0x1a5f_e8ac_a804_8a5d_7cbd_56bd_8318_4568);
        let sit_ground = Uuid::from_u128(0x1c76_00d6_661f_b87b_efe2_d742_1eb9_3c86);
        let sit_to_stand = Uuid::from_u128(0xa8de_e56f_2eae_9e7a_05a2_6fb9_2b97_e21e);
        let bytes = Packet::new(1, avatar_animation(agent_id, &[sit, sit_ground, sit_to_stand, ground_sit]))
            .encode()
            .expect("encode AvatarAnimation packet");
        let packet = Packet::decode(&bytes).expect("decode AvatarAnimation packet");
        let update = decode_sit_animation(packet.message()).expect("ground sit animation update");

        assert_eq!(update.agent_id, agent_id);
        assert!(update.ground_sitting);
        let packet = Packet::decode(
            &Packet::new(2, avatar_animation(agent_id, &[sit, sit_ground, sit_to_stand])).encode().expect("encode standing packet"),
        )
        .expect("decode standing packet");
        assert!(!decode_sit_animation(packet.message()).expect("standing animation update").ground_sitting);
    }

    #[test]
    fn non_sit_messages_are_ignored() {
        assert_eq!(decode_sit_target_update(&Message::StartPingCheck(Default::default())), None);
    }

    #[test]
    fn visual_params_keep_positions() {
        let message = avatar_message(texture_entry(Uuid::from_u128(88)), &[12, 34], Some((1, 50)));
        let update = decode_avatar_appearance_update(&message).expect("valid avatar message").expect("avatar appearance update");

        assert_eq!(update.object_id, Uuid::from_u128(7));
        assert_eq!(
            update.update.visual_params().collect::<Vec<_>>(),
            [(AvatarVisualParamIndex::new(0).unwrap(), 12), (AvatarVisualParamIndex::new(1).unwrap(), 34)]
        );
        assert_eq!(update.update.appearance_version(), Some(1));
        assert_eq!(update.update.cof_version(), Some(50));
        assert_eq!(
            update.update.texture_slots().and_then(|slots| slots.texture(AvatarTextureSlot::HEAD_BAKED)),
            Some(AssetId::new(Uuid::from_u128(88)))
        );
    }

    #[test]
    fn bad_texture_keeps_visual_params() {
        let message = avatar_message(vec![0], &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("valid visual params").expect("avatar update");
        assert_eq!(update.update.texture_slots(), None);
        assert_eq!(update.update.visual_params().count(), 2);
        assert_eq!(update.invalid_fields, 1);
    }

    #[test]
    fn empty_texture_is_omitted() {
        let message = avatar_message(Vec::new(), &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("valid avatar message").expect("avatar update");
        assert_eq!(update.update.texture_slots(), None);
        assert_eq!(update.invalid_fields, 0);
    }

    #[test]
    fn nil_texture_clears_slots() {
        let message = avatar_message(texture_entry(Uuid::nil()), &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("valid avatar message").expect("avatar update");
        let slots = update.update.texture_slots().expect("TextureEntry replaces avatar slots");
        assert_eq!(slots.texture(AvatarTextureSlot::HEAD_BAKED), None);
    }

    #[test]
    fn oversized_texture_is_omitted() {
        let mut bytes = texture_entry(Uuid::from_u128(88));
        while bytes.len() <= super::MAX_AVATAR_TEXTURE_ENTRY_BYTES {
            bytes.push(1);
            bytes.extend_from_slice(&[0; 16]);
        }
        assert!(decode_texture_entry(&bytes).expect("valid TextureEntry").is_some());

        let message = avatar_message(bytes, &[12, 34], None);
        let update = decode_avatar_appearance_update(&message).expect("valid visual params").expect("avatar update");
        assert_eq!(update.update.texture_slots(), None);
        assert_eq!(update.update.visual_params().count(), 2);
        assert_eq!(update.invalid_fields, 1);
    }

    #[test]
    fn short_param_list_is_ignored() {
        assert_eq!(decode_avatar_appearance_update(&avatar_message(Vec::new(), &[12], None)), Err(super::DecodeError::InvalidMessage));
    }

    #[test]
    fn unsupported_version_is_ignored() {
        assert_eq!(
            decode_avatar_appearance_update(&avatar_message(Vec::new(), &[12, 34], Some((2, 50)))),
            Err(super::DecodeError::InvalidMessage)
        );
    }
}
