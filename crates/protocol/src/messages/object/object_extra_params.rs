use std::mem::size_of;

use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::OBJECT_EXTRA_PARAMS,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectExtraParams {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub object_data: Vec<ExtraParamUpdate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtraParamUpdate {
    pub local_id: u32,
    pub parameter_type: u16,
    pub in_use: bool,
    pub data: Vec<u8>,
}

impl TypedMessage for ObjectExtraParams {
    const ID: MessageId = OBJECT_EXTRA_PARAMS;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::Specialized;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        self::decode(reader)
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        self::encode(self, writer)
    }

    fn preflight_allocation_bound(payload: &[u8]) -> Result<usize, LLUdpError> {
        crate::lludp::message::allocation::with_payload(payload, preflight_allocation_bytes(payload)?)
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes =
            crate::lludp::message::allocation::capacity_bytes(self.object_data.capacity(), std::mem::size_of::<ExtraParamUpdate>());
        for update in &self.object_data {
            crate::lludp::message::allocation::add_capacity(&mut bytes, update.data.capacity(), std::mem::size_of::<u8>());
        }
        bytes
    }
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<ObjectExtraParams, LLUdpError> {
    let agent_id = reader.uuid()?;
    let session_id = reader.uuid()?;
    let count = reader.terminal_variable_count()?;
    let mut object_data = Vec::with_capacity(count);

    for _ in 0..count {
        let local_id = reader.u32()?;
        let parameter_type = reader.u16()?;
        let in_use = reader.u8()? != 0;
        let parameter_size = reader.u32()?;
        let data = reader.variable1()?;
        if usize::try_from(parameter_size).ok() != Some(data.len()) {
            return Err(LLUdpError::InvalidMessage);
        }

        object_data.push(ExtraParamUpdate { local_id, parameter_type, in_use, data: data.to_vec() });
    }

    Ok(ObjectExtraParams { agent_id, session_id, object_data })
}

pub(crate) fn encode(message: &ObjectExtraParams, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    writer.uuid(message.agent_id);
    writer.uuid(message.session_id);
    writer.u8(u8::try_from(message.object_data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);

    for update in &message.object_data {
        let parameter_size = u32::try_from(update.data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?;
        writer.u32(update.local_id);
        writer.u16(update.parameter_type);
        writer.u8(u8::from(update.in_use));
        writer.u32(parameter_size);
        writer.u8(u8::try_from(update.data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
        writer.bytes(&update.data);
    }

    Ok(())
}

pub(crate) fn preflight_allocation_bytes(payload: &[u8]) -> Result<usize, LLUdpError> {
    let mut reader = Reader::new(payload);
    reader.take(32)?;
    let count = reader.terminal_variable_count()?;
    let mut allocation_bytes = count.checked_mul(size_of::<ExtraParamUpdate>()).ok_or(LLUdpError::AllocationBoundOverflow)?;

    for _ in 0..count {
        reader.take(7)?;
        let parameter_size = reader.u32()?;
        let data = reader.variable1()?;
        if usize::try_from(parameter_size).ok() != Some(data.len()) {
            return Err(LLUdpError::InvalidMessage);
        }
        allocation_bytes = allocation_bytes.checked_add(data.len()).ok_or(LLUdpError::AllocationBoundOverflow)?;
    }

    Ok(allocation_bytes)
}
