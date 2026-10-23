use uuid::Uuid;

use crate::{
    ChatType, LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::CHAT_FROM_VIEWER,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ChatFromViewer {
    pub agent_data: AgentData,
    pub chat_data: ChatData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatData {
    pub message: String,
    pub chat_type: ChatType,
    pub channel: i32,
}

impl TypedMessage for ChatFromViewer {
    const ID: MessageId = CHAT_FROM_VIEWER;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = false;
    const ZEROCODED: Option<bool> = Some(false);

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_data: AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            chat_data: ChatData {
                message: reader.variable2_string()?,
                chat_type: ChatType::from_wire(reader.u8()?),
                channel: reader.i32()?,
            },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.variable2_string(&self.chat_data.message)?;
        writer.u8(self.chat_data.chat_type.to_wire());
        writer.i32(self.chat_data.channel);
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        crate::lludp::message::allocation::add_string_capacity(&mut bytes, &self.chat_data.message);
        bytes
    }
}
