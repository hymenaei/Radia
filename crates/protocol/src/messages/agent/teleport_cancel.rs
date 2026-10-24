use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::TELEPORT_CANCEL,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportCancel {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

impl TypedMessage for TeleportCancel {
    const ID: MessageId = TELEPORT_CANCEL;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        decode(reader)
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        encode(self, writer);
        Ok(())
    }
}

pub(crate) fn encode(message: &TeleportCancel, writer: &mut Writer) {
    writer.uuid(message.agent_id);
    writer.uuid(message.session_id);
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<TeleportCancel, LLUdpError> {
    Ok(TeleportCancel { agent_id: reader.uuid()?, session_id: reader.uuid()? })
}
