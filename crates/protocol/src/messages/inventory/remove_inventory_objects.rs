use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REMOVE_INVENTORY_OBJECTS,
};

use super::{
    blocks::{read_id, write_id},
    read_blocks, read_blocks_with_count, validate_agent_id, write_count,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveInventoryObjects {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub folder_data: Vec<Uuid>,
    pub item_data: Vec<Uuid>,
    pub invalid_blocks: usize,
}

impl TypedMessage for RemoveInventoryObjects {
    const ID: MessageId = REMOVE_INVENTORY_OBJECTS;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        validate_agent_id(agent_id)?;
        let session_id = reader.uuid()?;
        let folder_count = usize::from(reader.u8()?);
        let (folder_data, invalid_folders) = read_blocks_with_count(reader, folder_count, read_id)?;
        let (item_data, invalid_items) = read_blocks(reader, read_id)?;
        Ok(Self { agent_id, session_id, folder_data, item_data, invalid_blocks: invalid_folders + invalid_items })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        validate_agent_id(self.agent_id)?;
        writer.uuid(self.agent_id);
        writer.uuid(self.session_id);
        write_count(writer, self.folder_data.len())?;
        for id in &self.folder_data {
            write_id(writer, *id)?;
        }
        write_count(writer, self.item_data.len())?;
        for id in &self.item_data {
            write_id(writer, *id)?;
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(
            self.folder_data.capacity().saturating_add(self.item_data.capacity()),
            std::mem::size_of::<Uuid>(),
        )
    }
}
