use crate::{
    LLUdpError,
    lludp::codec::{Reader, Writer},
};

pub const MAX_DATA_BYTES: usize = 7 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericStreamingMessage {
    pub method: u16,
    pub data: Vec<u8>,
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
