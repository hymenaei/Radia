use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::DISABLE_SIMULATOR,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DisableSimulator;

impl TypedMessage for DisableSimulator {
    const ID: MessageId = DISABLE_SIMULATOR;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = true;

    fn decode(_: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self)
    }

    fn encode(&self, _: &mut Writer<'_>) -> Result<(), LLUdpError> {
        Ok(())
    }
}
