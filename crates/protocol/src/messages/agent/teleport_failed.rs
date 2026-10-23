use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::TELEPORT_FAILED,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TeleportFailed {
    pub agent_id: Uuid,
    pub reason: Vec<u8>,
    pub alert_info: Vec<AlertInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertInfo {
    pub message: Vec<u8>,
    pub extra_params: Vec<u8>,
}

impl TypedMessage for TeleportFailed {
    const ID: MessageId = TELEPORT_FAILED;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let agent_id = reader.uuid()?;
        let reason = reader.variable1()?.to_vec();
        let count = reader.terminal_variable_count()?;
        let mut alert_info = Vec::with_capacity(count);
        for _ in 0..count {
            alert_info.push(AlertInfo { message: reader.variable1()?.to_vec(), extra_params: reader.variable1()?.to_vec() });
        }
        Ok(Self { agent_id, reason, alert_info })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_id);
        writer.variable1(&self.reason)?;
        writer.u8(u8::try_from(self.alert_info.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        for info in &self.alert_info {
            writer.variable1(&info.message)?;
            writer.variable1(&info.extra_params)?;
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = crate::lludp::message::allocation::capacity_bytes(self.reason.capacity(), std::mem::size_of::<u8>());
        crate::lludp::message::allocation::add_capacity(&mut bytes, self.alert_info.capacity(), std::mem::size_of::<AlertInfo>());
        for info in &self.alert_info {
            crate::lludp::message::allocation::add_capacity(&mut bytes, info.message.capacity(), std::mem::size_of::<u8>());
            crate::lludp::message::allocation::add_capacity(&mut bytes, info.extra_params.capacity(), std::mem::size_of::<u8>());
        }
        bytes
    }
}
