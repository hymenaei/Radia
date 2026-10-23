use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::USE_CIRCUIT_CODE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct UseCircuitCode {
    pub circuit_code: CircuitCode,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CircuitCode {
    pub code: u32,
    pub session_id: Uuid,
    pub agent_id: Uuid,
}

impl TypedMessage for UseCircuitCode {
    const ID: MessageId = USE_CIRCUIT_CODE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { circuit_code: CircuitCode { code: reader.u32()?, session_id: reader.uuid()?, agent_id: reader.uuid()? } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.u32(self.circuit_code.code);
        writer.uuid(self.circuit_code.session_id);
        writer.uuid(self.circuit_code.agent_id);
        Ok(())
    }
}
