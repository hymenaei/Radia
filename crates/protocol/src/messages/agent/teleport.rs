use math::Vec3;
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::{TELEPORT_LANDMARK_REQUEST, TELEPORT_LOCAL, TELEPORT_LOCATION_REQUEST, TELEPORT_PROGRESS, TELEPORT_START},
};

#[derive(Clone, Debug, PartialEq)]
pub struct TeleportLocationRequest {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub region_handle: u64,
    pub position: Vec3,
    pub look_at: Vec3,
}

impl TypedMessage for TeleportLocationRequest {
    const ID: MessageId = TELEPORT_LOCATION_REQUEST;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;
    const ZEROCODED: Option<bool> = Some(false);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_id: reader.uuid()?,
            session_id: reader.uuid()?,
            region_handle: reader.u64()?,
            position: reader.vector3()?,
            look_at: reader.vector3()?,
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_id);
        writer.uuid(self.session_id);
        writer.u64(self.region_handle);
        writer.vector3(self.position);
        writer.vector3(self.look_at);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportLandmarkRequest {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub landmark_id: Uuid,
}

impl TypedMessage for TeleportLandmarkRequest {
    const ID: MessageId = TELEPORT_LANDMARK_REQUEST;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;
    const ZEROCODED: Option<bool> = Some(true);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { agent_id: reader.uuid()?, session_id: reader.uuid()?, landmark_id: reader.uuid()? })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_id);
        writer.uuid(self.session_id);
        writer.uuid(self.landmark_id);
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct TeleportLocal {
    pub agent_id: Uuid,
    pub location_id: u32,
    pub position: Vec3,
    pub look_at: Vec3,
    pub teleport_flags: u32,
}

impl TypedMessage for TeleportLocal {
    const ID: MessageId = TELEPORT_LOCAL;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = true;
    const ZEROCODED: Option<bool> = Some(false);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_id: reader.uuid()?,
            location_id: reader.u32()?,
            position: reader.vector3()?,
            look_at: reader.vector3()?,
            teleport_flags: reader.u32()?,
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_id);
        writer.u32(self.location_id);
        writer.vector3(self.position);
        writer.vector3(self.look_at);
        writer.u32(self.teleport_flags);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportStart {
    pub teleport_flags: u32,
}

impl TypedMessage for TeleportStart {
    const ID: MessageId = TELEPORT_START;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = true;
    const ZEROCODED: Option<bool> = Some(false);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { teleport_flags: reader.u32()? })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.u32(self.teleport_flags);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportProgress {
    pub agent_id: Uuid,
    pub teleport_flags: u32,
    pub message: Vec<u8>,
}

impl TypedMessage for TeleportProgress {
    const ID: MessageId = TELEPORT_PROGRESS;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;
    const ZEROCODED: Option<bool> = Some(false);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { agent_id: reader.uuid()?, teleport_flags: reader.u32()?, message: reader.variable1()?.to_vec() })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_id);
        writer.u32(self.teleport_flags);
        writer.variable1(&self.message)
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.message.capacity(), std::mem::size_of::<u8>())
    }
}
