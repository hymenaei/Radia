use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REMOVE_INVENTORY_FOLDER,
};

use super::{
    blocks::{read_id, write_id},
    read_blocks, validate_agent_id, write_count,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoveInventoryFolder {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub folder_data: Vec<Uuid>,
    pub invalid_blocks: usize,
}

impl TypedMessage for RemoveInventoryFolder {
    const ID: MessageId = REMOVE_INVENTORY_FOLDER;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        validate_agent_id(agent_id)?;
        let session_id = reader.uuid()?;
        let (folder_data, invalid_blocks) = read_blocks(reader, read_id)?;
        Ok(Self { agent_id, session_id, folder_data, invalid_blocks })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        validate_agent_id(self.agent_id)?;
        writer.uuid(self.agent_id);
        writer.uuid(self.session_id);
        write_count(writer, self.folder_data.len())?;
        for id in &self.folder_data {
            write_id(writer, *id)?;
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.folder_data.capacity(), std::mem::size_of::<Uuid>())
    }
}
