use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REGION_ID_AND_HANDLE_REPLY,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegionIdAndHandleReply {
    pub reply_block: ReplyBlock,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReplyBlock {
    pub region_id: Uuid,
    pub region_handle: u64,
}

impl TypedMessage for RegionIdAndHandleReply {
    const ID: MessageId = REGION_ID_AND_HANDLE_REPLY;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { reply_block: ReplyBlock { region_id: reader.uuid()?, region_handle: reader.u64()? } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.reply_block.region_id);
        writer.u64(self.reply_block.region_handle);
        Ok(())
    }
}
