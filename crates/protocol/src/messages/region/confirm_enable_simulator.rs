use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::CONFIRM_ENABLE_SIMULATOR,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmEnableSimulator {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

impl TypedMessage for ConfirmEnableSimulator {
    const ID: MessageId = CONFIRM_ENABLE_SIMULATOR;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { agent_data: AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        Ok(())
    }
}
