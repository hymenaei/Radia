use math::{Quat, Vec3};
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::AVATAR_SIT_RESPONSE,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvatarSitResponse {
    pub sit_object: SitObject,
    pub sit_transform: SitTransform,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SitObject {
    pub id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SitTransform {
    pub auto_pilot: Option<bool>,
    pub sit_position: Option<Vec3>,
    pub sit_rotation: Option<Quat>,
    pub camera_eye_offset: Option<Vec3>,
    pub camera_at_offset: Option<Vec3>,
    pub force_mouselook: Option<bool>,
}

impl TypedMessage for AvatarSitResponse {
    const ID: MessageId = AVATAR_SIT_RESPONSE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let id = reader.optional_fixed(16, |reader| reader.uuid())?;
        let auto_pilot = reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?;
        let sit_position = reader.optional_fixed(12, |reader| reader.vector3())?;
        let sit_rotation = reader.optional_fixed(12, |reader| reader.quaternion())?;
        let camera_eye_offset = reader.optional_fixed(12, |reader| reader.vector3())?;
        let camera_at_offset = reader.optional_fixed(12, |reader| reader.vector3())?;
        let force_mouselook = reader.optional_fixed(1, |reader| reader.u8().map(|value| value != 0))?;
        Ok(Self {
            sit_object: SitObject { id },
            sit_transform: SitTransform { auto_pilot, sit_position, sit_rotation, camera_eye_offset, camera_at_offset, force_mouselook },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let Some(id) = self.sit_object.id else { return Ok(()) };
        writer.uuid(id);
        let Some(auto_pilot) = self.sit_transform.auto_pilot else { return Ok(()) };
        writer.u8(u8::from(auto_pilot));
        let Some(sit_position) = self.sit_transform.sit_position else { return Ok(()) };
        writer.vector3(sit_position);
        let Some(sit_rotation) = self.sit_transform.sit_rotation else { return Ok(()) };
        writer.quaternion_xyz(sit_rotation)?;
        let Some(camera_eye_offset) = self.sit_transform.camera_eye_offset else { return Ok(()) };
        writer.vector3(camera_eye_offset);
        let Some(camera_at_offset) = self.sit_transform.camera_at_offset else { return Ok(()) };
        writer.vector3(camera_at_offset);
        let Some(force_mouselook) = self.sit_transform.force_mouselook else { return Ok(()) };
        writer.u8(u8::from(force_mouselook));
        Ok(())
    }
}
