use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::LOGOUT_REPLY,
};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogoutReply {
    pub agent_data: AgentData,
    pub inventory_data: Vec<InventoryData>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InventoryData {
    pub item_id: Uuid,
}

impl TypedMessage for LogoutReply {
    const ID: MessageId = LOGOUT_REPLY;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_data = AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? };
        let count = reader.terminal_variable_count()?;
        let mut inventory_data = Vec::with_capacity(count);
        for _ in 0..count {
            inventory_data.push(InventoryData { item_id: reader.uuid()? });
        }
        Ok(Self { agent_data, inventory_data })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let count = u8::try_from(self.inventory_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.u8(count);
        for item in &self.inventory_data {
            writer.uuid(item.item_id);
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.inventory_data.capacity(), std::mem::size_of::<InventoryData>())
    }
}
