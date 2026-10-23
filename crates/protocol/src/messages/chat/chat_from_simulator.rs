use math::Vec3;
use uuid::Uuid;

use crate::{
    Audible, ChatSourceType, ChatType, LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::CHAT_FROM_SIMULATOR,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ChatFromSimulator {
    pub chat_data: ChatData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatData {
    pub from_name: String,
    pub source_id: Uuid,
    pub owner_id: Uuid,
    pub source_type: ChatSourceType,
    pub chat_type: ChatType,
    pub audible: Audible,
    pub position: Vec3,
    pub message: String,
}

impl TypedMessage for ChatFromSimulator {
    const ID: MessageId = CHAT_FROM_SIMULATOR;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            chat_data: ChatData {
                from_name: reader.variable1_string()?,
                source_id: reader.uuid()?,
                owner_id: reader.uuid()?,
                source_type: crate::ChatSourceType::from_wire(reader.u8()?),
                chat_type: crate::ChatType::from_wire(reader.u8()?),
                audible: crate::Audible::from_wire(reader.u8()?),
                position: reader.vector3()?,
                message: reader.variable2_string()?,
            },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let block = &self.chat_data;
        writer.variable1_string(&block.from_name)?;
        writer.uuid(block.source_id);
        writer.uuid(block.owner_id);
        writer.u8(block.source_type.to_wire());
        writer.u8(block.chat_type.to_wire());
        writer.u8(block.audible.to_wire());
        writer.vector3(block.position);
        writer.variable2_string(&block.message)
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        crate::lludp::message::allocation::add_string_capacity(&mut bytes, &self.chat_data.from_name);
        crate::lludp::message::allocation::add_string_capacity(&mut bytes, &self.chat_data.message);
        bytes
    }
}
