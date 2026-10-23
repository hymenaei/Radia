use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REGION_HANDSHAKE_REPLY,
};

#[derive(Clone, Debug, PartialEq)]
pub struct RegionHandshakeReply {
    pub agent_data: AgentData,
    pub region_info: RegionInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionInfo {
    pub flags: u32,
}

impl TypedMessage for RegionHandshakeReply {
    const ID: MessageId = REGION_HANDSHAKE_REPLY;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_data: AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            region_info: RegionInfo { flags: reader.u32()? },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.u32(self.region_info.flags);
        Ok(())
    }
}
