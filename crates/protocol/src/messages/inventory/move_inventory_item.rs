use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::MOVE_INVENTORY_ITEM,
};

use super::{
    blocks::{MoveInventoryItemBlock, read_move, write_move},
    read_blocks, read_bool, validate_agent_id, write_bool, write_count,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MoveInventoryItem {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub stamp: bool,
    pub inventory_data: Vec<MoveInventoryItemBlock>,
    pub invalid_blocks: usize,
}

impl TypedMessage for MoveInventoryItem {
    const ID: MessageId = MOVE_INVENTORY_ITEM;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        validate_agent_id(agent_id)?;
        let session_id = reader.uuid()?;
        let stamp = read_bool(reader)?;
        let (inventory_data, invalid_blocks) = read_blocks(reader, read_move)?;
        Ok(Self { agent_id, session_id, stamp, inventory_data, invalid_blocks })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        validate_agent_id(self.agent_id)?;
        writer.uuid(self.agent_id);
        writer.uuid(self.session_id);
        write_bool(writer, self.stamp);
        write_count(writer, self.inventory_data.len())?;
        for block in &self.inventory_data {
            write_move(writer, block)?;
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        crate::lludp::message::allocation::add_capacity(
            &mut bytes,
            self.inventory_data.capacity(),
            std::mem::size_of::<MoveInventoryItemBlock>(),
        );
        for block in &self.inventory_data {
            crate::lludp::message::allocation::add_string_capacity(&mut bytes, &block.new_name);
        }
        bytes
    }
}
