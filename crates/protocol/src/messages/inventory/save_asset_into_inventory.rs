use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::SAVE_ASSET_INTO_INVENTORY,
};

use super::validate_agent_id;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveAssetIntoInventory {
    pub agent_id: Uuid,
    pub item_id: Uuid,
    pub new_asset_id: Uuid,
}

impl TypedMessage for SaveAssetIntoInventory {
    const ID: MessageId = SAVE_ASSET_INTO_INVENTORY;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        validate_agent_id(agent_id)?;
        let item_id = reader.uuid()?;
        let new_asset_id = reader.uuid()?;
        if item_id.is_nil() || new_asset_id.is_nil() {
            return Err(LLUdpError::InvalidMessage);
        }
        Ok(Self { agent_id, item_id, new_asset_id })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        validate_agent_id(self.agent_id)?;
        if self.item_id.is_nil() || self.new_asset_id.is_nil() {
            return Err(LLUdpError::InvalidMessage);
        }
        writer.uuid(self.agent_id);
        writer.uuid(self.item_id);
        writer.uuid(self.new_asset_id);
        Ok(())
    }
}
