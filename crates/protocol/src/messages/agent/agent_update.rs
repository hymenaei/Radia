use math::{Quat, Vec3};
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::AGENT_UPDATE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentUpdate {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub body_rotation: Quat,
    pub head_rotation: Quat,
    pub state: u8,
    pub camera_center: Vec3,
    pub camera_at_axis: Vec3,
    pub camera_left_axis: Vec3,
    pub camera_up_axis: Vec3,
    pub far: f32,
    pub control_flags: u32,
    pub flags: u8,
}

impl TypedMessage for AgentUpdate {
    const ID: MessageId = AGENT_UPDATE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_data: AgentData {
                agent_id: reader.uuid()?,
                session_id: reader.uuid()?,
                body_rotation: reader.quaternion()?,
                head_rotation: reader.quaternion()?,
                state: reader.u8()?,
                camera_center: reader.vector3()?,
                camera_at_axis: reader.vector3()?,
                camera_left_axis: reader.vector3()?,
                camera_up_axis: reader.vector3()?,
                far: reader.f32()?,
                control_flags: reader.u32()?,
                flags: reader.u8()?,
            },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let block = &self.agent_data;
        writer.uuid(block.agent_id);
        writer.uuid(block.session_id);
        writer.quaternion_xyz(block.body_rotation)?;
        writer.quaternion_xyz(block.head_rotation)?;
        writer.u8(block.state);
        writer.vector3(block.camera_center);
        writer.vector3(block.camera_at_axis);
        writer.vector3(block.camera_left_axis);
        writer.vector3(block.camera_up_axis);
        writer.f32(block.far);
        writer.u32(block.control_flags);
        writer.u8(block.flags);
        Ok(())
    }
}
