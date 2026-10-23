use super::PingId;
use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::COMPLETE_PING_CHECK,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CompletePingCheck {
    pub ping_id: PingId,
}

impl TypedMessage for CompletePingCheck {
    const ID: MessageId = COMPLETE_PING_CHECK;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { ping_id: PingId { ping_id: reader.u8()? } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.u8(self.ping_id.ping_id);
        Ok(())
    }
}
