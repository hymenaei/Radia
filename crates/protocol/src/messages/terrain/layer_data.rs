use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::LAYER_DATA,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerData {
    pub layer_type: u8,
    pub data: Vec<u8>,
}

impl TypedMessage for LayerData {
    const ID: MessageId = LAYER_DATA;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { layer_type: reader.u8()?, data: reader.variable2()?.to_vec() })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.u8(self.layer_type);
        writer.variable2(&self.data)
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.data.capacity(), std::mem::size_of::<u8>())
    }
}
