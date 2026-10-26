use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::BULK_UPDATE_INVENTORY,
};

use super::{
    blocks::{
        InventoryFolderBlock, InventoryItemBlock, ItemBlockOrder, add_item_capacity, read_folder, read_item, write_folder, write_item,
    },
    read_blocks, read_blocks_with_count, validate_agent_id, write_count,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BulkUpdateInventory {
    pub agent_id: Uuid,
    pub transaction_id: Uuid,
    pub folder_data: Vec<InventoryFolderBlock>,
    pub item_data: Vec<InventoryItemBlock>,
    pub invalid_blocks: usize,
}

impl TypedMessage for BulkUpdateInventory {
    const ID: MessageId = BULK_UPDATE_INVENTORY;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        validate_agent_id(agent_id)?;
        let transaction_id = reader.uuid()?;
        let folder_count = usize::from(reader.u8()?);
        let (folder_data, invalid_folders) = read_blocks_with_count(reader, folder_count, read_folder)?;
        let (item_data, invalid_items) = read_blocks(reader, |reader| read_item(reader, ItemBlockOrder::Bulk))?;
        Ok(Self { agent_id, transaction_id, folder_data, item_data, invalid_blocks: invalid_folders + invalid_items })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        validate_agent_id(self.agent_id)?;
        writer.uuid(self.agent_id);
        writer.uuid(self.transaction_id);
        write_count(writer, self.folder_data.len())?;
        for block in &self.folder_data {
            write_folder(writer, block)?;
        }
        write_count(writer, self.item_data.len())?;
        for block in &self.item_data {
            write_item(writer, block, ItemBlockOrder::Bulk)?;
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        crate::lludp::message::allocation::add_capacity(
            &mut bytes,
            self.folder_data.capacity(),
            std::mem::size_of::<InventoryFolderBlock>(),
        );
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.item_data.capacity(), std::mem::size_of::<InventoryItemBlock>());
        for block in &self.folder_data {
            crate::lludp::message::allocation::add_string_capacity(&mut bytes, &block.name);
        }
        self.item_data.iter().for_each(|block| add_item_capacity(&mut bytes, block));
        bytes
    }
}
