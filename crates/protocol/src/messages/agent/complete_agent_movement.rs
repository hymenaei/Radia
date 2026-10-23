use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::COMPLETE_AGENT_MOVEMENT,
};

#[derive(Clone, Debug, PartialEq)]
pub struct CompleteAgentMovement {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub circuit_code: u32,
}

impl TypedMessage for CompleteAgentMovement {
    const ID: MessageId = COMPLETE_AGENT_MOVEMENT;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { agent_data: AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()?, circuit_code: reader.u32()? } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.u32(self.agent_data.circuit_code);
        Ok(())
    }
}
