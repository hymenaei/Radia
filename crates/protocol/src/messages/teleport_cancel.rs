use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportCancel {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

pub(crate) fn encode(message: &TeleportCancel, writer: &mut Writer) {
    writer.uuid(message.agent_id);
    writer.uuid(message.session_id);
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<TeleportCancel, LLUdpError> {
    Ok(TeleportCancel { agent_id: reader.uuid()?, session_id: reader.uuid()? })
}
