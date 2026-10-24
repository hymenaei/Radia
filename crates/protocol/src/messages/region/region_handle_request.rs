use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::REGION_HANDLE_REQUEST,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionHandleRequest {
    pub request_block: RequestBlock,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestBlock {
    pub region_id: Uuid,
}

impl TypedMessage for RegionHandleRequest {
    const ID: MessageId = REGION_HANDLE_REQUEST;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { request_block: RequestBlock { region_id: reader.uuid()? } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.request_block.region_id);
        Ok(())
    }
}
