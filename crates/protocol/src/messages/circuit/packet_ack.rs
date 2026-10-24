use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::PACKET_ACK,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PacketAck {
    pub packets: Vec<Packets>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Packets {
    pub id: u32,
}

impl TypedMessage for PacketAck {
    const ID: MessageId = PACKET_ACK;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = false;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let count = reader.terminal_variable_count()?;
        let mut packets = Vec::with_capacity(count);
        for _ in 0..count {
            packets.push(Packets { id: reader.u32()? });
        }
        Ok(Self { packets })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        let count = u8::try_from(self.packets.len()).map_err(|_| LLUdpError::TooManyAcks)?;
        writer.u8(count);
        for packet in &self.packets {
            writer.u32(packet.id);
        }
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.packets.capacity(), std::mem::size_of::<Packets>())
    }
}
