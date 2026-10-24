use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::MAP_BLOCK_REQUEST,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapBlockRequest {
    pub agent_data: AgentData,
    pub position_data: PositionData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub flags: u32,
    pub estate_id: u32,
    pub godlike: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionData {
    pub min_x: u16,
    pub max_x: u16,
    pub min_y: u16,
    pub max_y: u16,
}

impl TypedMessage for MapBlockRequest {
    const ID: MessageId = MAP_BLOCK_REQUEST;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_data: AgentData {
                agent_id: reader.uuid()?,
                session_id: reader.uuid()?,
                flags: reader.u32()?,
                estate_id: reader.u32()?,
                godlike: reader.u8()? != 0,
            },
            position_data: PositionData { min_x: reader.u16()?, max_x: reader.u16()?, min_y: reader.u16()?, max_y: reader.u16()? },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.u32(self.agent_data.flags);
        writer.u32(self.agent_data.estate_id);
        writer.u8(u8::from(self.agent_data.godlike));
        writer.u16(self.position_data.min_x);
        writer.u16(self.position_data.max_x);
        writer.u16(self.position_data.min_y);
        writer.u16(self.position_data.max_y);
        Ok(())
    }
}
