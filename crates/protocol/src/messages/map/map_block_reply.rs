use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::MAP_BLOCK_REPLY,
};

#[derive(Clone, Debug, PartialEq)]
pub struct MapBlockReply {
    pub agent_data: AgentData,
    pub data: Vec<Data>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub flags: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Data {
    pub x: u16,
    pub y: u16,
    pub name: String,
    pub access: u8,
    pub region_flags: u32,
    pub water_height: u8,
    pub agents: u8,
    pub map_image_id: Uuid,
}

impl TypedMessage for MapBlockReply {
    const ID: MessageId = MAP_BLOCK_REPLY;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        let flags = reader.u32()?;
        let count = reader.terminal_variable_count()?;
        let mut data = Vec::with_capacity(count);
        for _ in 0..count {
            data.push(Data {
                x: reader.u16()?,
                y: reader.u16()?,
                name: reader.variable1_string()?,
                access: reader.u8()?,
                region_flags: reader.u32()?,
                water_height: reader.u8()?,
                agents: reader.u8()?,
                map_image_id: reader.uuid()?,
            });
        }
        Ok(Self { agent_data: AgentData { agent_id, flags }, data })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let count = u8::try_from(self.data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
        writer.uuid(self.agent_data.agent_id);
        writer.u32(self.agent_data.flags);
        writer.u8(count);
        for block in &self.data {
            writer.u16(block.x);
            writer.u16(block.y);
            writer.variable1_string(&block.name)?;
            writer.u8(block.access);
            writer.u32(block.region_flags);
            writer.u8(block.water_height);
            writer.u8(block.agents);
            writer.uuid(block.map_image_id);
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.data.capacity(), std::mem::size_of::<Data>());
        for block in &self.data {
            crate::lludp::message::allocation::add_string_capacity(&mut bytes, &block.name);
        }
        bytes
    }
}
