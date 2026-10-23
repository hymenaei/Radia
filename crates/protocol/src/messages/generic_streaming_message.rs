use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::GENERIC_STREAMING_MESSAGE,
};

pub const MAX_DATA_BYTES: usize = 7 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericStreamingMessage {
    pub method: u16,
    pub data: Vec<u8>,
}

impl TypedMessage for GenericStreamingMessage {
    const ID: MessageId = GENERIC_STREAMING_MESSAGE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::Specialized;
    const INBOUND_ONLY: bool = false;

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
        crate::lludp::message::allocation::capacity_bytes(self.data.capacity(), std::mem::size_of::<u8>())
    }
}

pub(crate) fn decode(reader: &mut Reader<'_>) -> Result<GenericStreamingMessage, LLUdpError> {
    let method = reader.u16()?;
    let data_len = usize::from(reader.u16()?);
    if data_len > MAX_DATA_BYTES {
        return Err(LLUdpError::InvalidMessage);
    }

    let data = reader.take(data_len)?.to_vec();
    Ok(GenericStreamingMessage { method, data })
}

pub(crate) fn encode(message: &GenericStreamingMessage, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
    if message.data.len() > MAX_DATA_BYTES {
        return Err(LLUdpError::InvalidMessage);
    }

    writer.u16(message.method);
    writer.u16(u16::try_from(message.data.len()).map_err(|_| LLUdpError::VariableFieldTooLong)?);
    writer.bytes(&message.data);
    Ok(())
}

pub(crate) fn preflight_allocation_bytes(payload: &[u8]) -> Result<usize, LLUdpError> {
    let mut reader = Reader::new(payload);
    reader.take(2)?;
    let data_len = usize::from(reader.u16()?);
    if data_len > MAX_DATA_BYTES {
        return Err(LLUdpError::InvalidMessage);
    }
    reader.take(data_len)?;
    Ok(data_len)
}
