use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::UPDATE_CREATE_INVENTORY_ITEM,
};

use super::{
    blocks::{InventoryItemBlock, ItemBlockOrder, add_item_capacity, read_item, write_item},
    read_blocks, read_bool, validate_agent_id, write_bool, write_count,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCreateInventoryItem {
    pub agent_id: Uuid,
    pub sim_approved: bool,
    pub transaction_id: Uuid,
    pub inventory_data: Vec<InventoryItemBlock>,
    pub invalid_blocks: usize,
}

impl TypedMessage for UpdateCreateInventoryItem {
    const ID: MessageId = UPDATE_CREATE_INVENTORY_ITEM;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        validate_agent_id(agent_id)?;
        let sim_approved = read_bool(reader)?;
        let transaction_id = reader.uuid()?;
        let (inventory_data, invalid_blocks) = read_blocks(reader, |reader| read_item(reader, ItemBlockOrder::UpdateCreate))?;
        Ok(Self { agent_id, sim_approved, transaction_id, inventory_data, invalid_blocks })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        validate_agent_id(self.agent_id)?;
        writer.uuid(self.agent_id);
        write_bool(writer, self.sim_approved);
        writer.uuid(self.transaction_id);
        write_count(writer, self.inventory_data.len())?;
        for block in &self.inventory_data {
            write_item(writer, block, ItemBlockOrder::UpdateCreate)?;
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        crate::lludp::message::allocation::add_capacity(
            &mut bytes,
            self.inventory_data.capacity(),
            std::mem::size_of::<InventoryItemBlock>(),
        );
        self.inventory_data.iter().for_each(|block| add_item_capacity(&mut bytes, block));
        bytes
    }
}
