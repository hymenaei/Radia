use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::PARCEL_OVERLAY,
};

#[derive(Clone, Debug, PartialEq)]
pub struct ParcelOverlay {
    pub sequence_id: i32,
    pub data: Vec<u8>,
}

impl TypedMessage for ParcelOverlay {
    const ID: MessageId = PARCEL_OVERLAY;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self { sequence_id: reader.i32()?, data: reader.variable2()?.to_vec() })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.i32(self.sequence_id);
        writer.variable2(&self.data)
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.data.capacity(), std::mem::size_of::<u8>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lludp::{codec::Reader, error::LLUdpError};

    #[test]
    fn decodes_official_chunk_index_and_variable_data() {
        let payload = [2, 0, 0, 0, 4, 0, 0, 0x11, 0x20, 0xc0];
        let mut reader = Reader::new(&payload);

        let message = ParcelOverlay::decode(&mut reader).unwrap();

        assert_eq!(message.sequence_id, 2);
        assert_eq!(message.data, [0, 0x11, 0x20, 0xc0]);
        assert!(reader.remaining().is_empty());
    }

    #[test]
    fn rejects_truncated_variable_data() {
        let payload = [0, 0, 0, 0, 4, 0, 1];
        let mut reader = Reader::new(&payload);

        std::assert_matches!(ParcelOverlay::decode(&mut reader), Err(LLUdpError::TruncatedPacket));
    }
}
