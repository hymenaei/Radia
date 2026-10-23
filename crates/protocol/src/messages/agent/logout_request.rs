use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::LOGOUT_REQUEST,
};

#[derive(Clone, Debug, PartialEq)]
pub struct LogoutRequest {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

impl TypedMessage for LogoutRequest {
    const ID: MessageId = LOGOUT_REQUEST;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { agent_data: AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        Ok(())
    }
}
