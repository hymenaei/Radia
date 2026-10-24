use crate::lludp::{
    AllocationStrategy, LLUdpError, Message, MessageId,
    packet::{HEADER_BYTES, MAX_LLUDP_PACKET_SIZE, MAX_PACKET_RETAINED_CAPACITY_BYTES, Packet, PacketFlags, PacketHeader, ZeroCoding},
    zero_code,
};

pub struct PacketFrame<'a> {
    sequence: u32,
    flags: PacketFlags,
    extra_header: &'a [u8],
    wire_body: &'a [u8],
    appended_ack_bytes: &'a [u8],
}

impl<'a> PacketFrame<'a> {
    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    pub fn message_id(&self) -> Result<MessageId, LLUdpError> {
        if self.flags.contains(PacketFlags::ZEROCODED) {
            let mut prefix = [0; 4];
            let prefix_len = zero_code::decode_prefix(self.wire_body, &mut prefix)?;
            MessageId::decode(&prefix[..prefix_len]).map(|(id, _)| id)
        } else {
            MessageId::decode(self.wire_body).map(|(id, _)| id)
        }
    }

    pub fn is_reliable(&self) -> bool {
        self.flags.contains(PacketFlags::RELIABLE)
    }

    pub fn is_resent(&self) -> bool {
        self.flags.contains(PacketFlags::RESENT)
    }

    pub fn appended_acks(&self) -> impl ExactSizeIterator<Item = u32> + '_ {
        self.appended_ack_bytes.as_chunks::<4>().0.iter().map(|chunk| u32::from_be_bytes(*chunk))
    }

    pub fn preflight(&self) -> Result<PacketPreflight, LLUdpError> {
        self.preflight_values().map(|(preflight, _, _)| preflight)
    }

    pub fn preflighted(self) -> Result<PreflightedPacketFrame<'a>, LLUdpError> {
        let (preflight, message_id_len, decoded_body_len) = self.preflight_values()?;
        Ok(PreflightedPacketFrame { frame: self, preflight, message_id_len, decoded_body_len })
    }

    fn preflight_values(&self) -> Result<(PacketPreflight, usize, usize), LLUdpError> {
        let (id, allocation_bytes, decoded_body_len, id_len) = if self.flags.contains(PacketFlags::ZEROCODED) {
            let mut id_prefix = [0; 4];
            let prefix_len = zero_code::decode_prefix(self.wire_body, &mut id_prefix)?;
            let (id, id_len) = MessageId::decode(&id_prefix[..prefix_len])?;
            let max_decoded_body_size = MAX_LLUDP_PACKET_SIZE
                .checked_sub(
                    HEADER_BYTES
                        + self.extra_header.len()
                        + self.appended_ack_bytes.len()
                        + usize::from(self.flags.contains(PacketFlags::APPENDED_ACKS)),
                )
                .ok_or(LLUdpError::DatagramTooLarge)?;
            let mut decoded_body = [0; MAX_LLUDP_PACKET_SIZE];
            let decoded_len = zero_code::decode_to_slice(self.wire_body, &mut decoded_body[..max_decoded_body_size])?;
            let payload = &decoded_body[id_len..decoded_len];
            let (_, allocation_bytes) = Message::allocation_bound(id, payload)?;
            (id, allocation_bytes, decoded_len, id_len)
        } else {
            let (id, id_len) = MessageId::decode(self.wire_body)?;
            let payload = &self.wire_body[id_len..];
            let (_, allocation_bytes) = Message::allocation_bound(id, payload)?;
            (id, allocation_bytes, self.wire_body.len(), id_len)
        };

        let packet_allocation_bytes = allocation_bytes
            .checked_add(self.extra_header.len())
            .and_then(|bytes| bytes.checked_add(self.appended_ack_bytes.len()))
            .ok_or(LLUdpError::AllocationBoundOverflow)?;
        let retained_capacity_upper_bound = packet_allocation_bytes;
        if retained_capacity_upper_bound > MAX_PACKET_RETAINED_CAPACITY_BYTES {
            return Err(LLUdpError::DecodedPacketTooLarge);
        }
        let temporary_decode_bytes = if self.flags.contains(PacketFlags::ZEROCODED) { decoded_body_len } else { 0 };
        let peak_decode_allocation_bytes =
            retained_capacity_upper_bound.checked_add(temporary_decode_bytes).ok_or(LLUdpError::AllocationBoundOverflow)?;

        Ok((
            PacketPreflight {
                message_id: id,
                allocation_strategy: Message::allocation_strategy(id),
                retained_capacity_upper_bound,
                peak_decode_allocation_bytes,
            },
            id_len,
            decoded_body_len,
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketPreflight {
    message_id: MessageId,
    allocation_strategy: AllocationStrategy,
    retained_capacity_upper_bound: usize,
    peak_decode_allocation_bytes: usize,
}

impl PacketPreflight {
    pub fn message_id(&self) -> MessageId {
        self.message_id
    }

    pub fn allocation_strategy(&self) -> AllocationStrategy {
        self.allocation_strategy
    }

    pub fn retained_capacity_upper_bound(&self) -> usize {
        self.retained_capacity_upper_bound
    }

    pub fn peak_decode_allocation_bytes(&self) -> usize {
        self.peak_decode_allocation_bytes
    }
}

pub struct PreflightedPacketFrame<'a> {
    frame: PacketFrame<'a>,
    preflight: PacketPreflight,
    message_id_len: usize,
    decoded_body_len: usize,
}

impl PreflightedPacketFrame<'_> {
    pub fn preflight(&self) -> PacketPreflight {
        self.preflight
    }

    pub fn decode(self) -> Result<Packet, LLUdpError> {
        let Self { frame, preflight, message_id_len, decoded_body_len } = self;
        let flags = frame.flags;
        let sequence = frame.sequence;
        let decode_payload = |message_bytes: &[u8]| Message::decode(preflight.message_id, &message_bytes[message_id_len..]);
        let (message, trailing_message_bytes) = if flags.contains(PacketFlags::ZEROCODED) {
            let mut decoded_body = Vec::with_capacity(decoded_body_len);
            zero_code::decode_into(frame.wire_body, &mut decoded_body, decoded_body_len)?;
            decode_payload(&decoded_body)?
        } else {
            decode_payload(frame.wire_body)?
        };
        let mut appended_acks = Vec::with_capacity(frame.appended_ack_bytes.len() / 4);
        appended_acks.extend(frame.appended_acks());
        let packet = Packet {
            header: PacketHeader { sequence, flags, extra_header: frame.extra_header.to_vec() },
            message,
            trailing_message_bytes,
            appended_acks,
            zero_coding: ZeroCoding::Received,
        };
        if packet.retained_capacity_bytes() > MAX_PACKET_RETAINED_CAPACITY_BYTES {
            return Err(LLUdpError::DecodedPacketTooLarge);
        }
        Ok(packet)
    }
}

impl Packet {
    pub fn inspect(bytes: &[u8]) -> Result<PacketFrame<'_>, LLUdpError> {
        if bytes.len() > MAX_LLUDP_PACKET_SIZE {
            return Err(LLUdpError::DatagramTooLarge);
        }
        if bytes.len() < HEADER_BYTES {
            return Err(LLUdpError::TruncatedPacket);
        }

        let flags = PacketFlags::from_bits_retain(bytes[0]);
        if flags.contains(PacketFlags::RESENT) && !flags.contains(PacketFlags::RELIABLE) {
            return Err(LLUdpError::InvalidPacketFlags);
        }
        let sequence = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let extra_len = usize::from(bytes[5]);
        let body_start = HEADER_BYTES.checked_add(extra_len).ok_or(LLUdpError::TruncatedPacket)?;
        if bytes.len() < body_start {
            return Err(LLUdpError::TruncatedPacket);
        }

        let body_end = if flags.contains(PacketFlags::APPENDED_ACKS) {
            let count = usize::from(*bytes.last().ok_or(LLUdpError::InvalidAcknowledgementList)?);
            let trailer_len =
                count.checked_mul(4).and_then(|length| length.checked_add(1)).ok_or(LLUdpError::InvalidAcknowledgementList)?;
            let end = bytes.len().checked_sub(trailer_len).ok_or(LLUdpError::InvalidAcknowledgementList)?;
            if end < body_start {
                return Err(LLUdpError::InvalidAcknowledgementList);
            }
            end
        } else {
            bytes.len()
        };
        let appended_ack_bytes = if flags.contains(PacketFlags::APPENDED_ACKS) { &bytes[body_end..bytes.len() - 1] } else { &[] };

        Ok(PacketFrame {
            sequence,
            flags,
            extra_header: &bytes[HEADER_BYTES..body_start],
            wire_body: &bytes[body_start..body_end],
            appended_ack_bytes,
        })
    }

    pub fn preflight(bytes: &[u8]) -> Result<PacketPreflight, LLUdpError> {
        Self::inspect(bytes)?.preflight()
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, LLUdpError> {
        Self::inspect(bytes)?.preflighted()?.decode()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{PacketAck, circuit::packet_ack::Packets};

    #[test]
    fn schema_bounds_cover_typed_messages() {
        use test_support::{lludp::vectors, parse_hex_fixture};

        for fixture in [
            vectors::AGENT_MOVEMENT_COMPLETE,
            vectors::PACKET_ACK,
            vectors::LOGOUT_REPLY,
            vectors::LOGOUT_REPLY_EMPTY,
            vectors::CHAT_FROM_VIEWER,
            vectors::CHAT_FROM_SIMULATOR,
            vectors::MAP_BLOCK_REPLY,
            vectors::OBJECT_UPDATE_CACHED,
            vectors::KILL_OBJECT,
            vectors::REQUEST_MULTIPLE_OBJECTS,
        ] {
            let bytes = parse_hex_fixture(fixture).expect("typed fixture is valid hex");
            let frame = Packet::inspect(&bytes).expect("inspect schema-shaped typed fixture");
            let message_bytes = if frame.flags.contains(PacketFlags::ZEROCODED) {
                let max_size = MAX_LLUDP_PACKET_SIZE
                    - HEADER_BYTES
                    - frame.extra_header.len()
                    - frame.appended_ack_bytes.len()
                    - usize::from(frame.flags.contains(PacketFlags::APPENDED_ACKS));
                zero_code::decode(frame.wire_body, max_size).expect("decode fixture zero coding")
            } else {
                frame.wire_body.to_vec()
            };
            let (id, id_len) = MessageId::decode(&message_bytes).expect("decode fixture message ID");
            let metadata = crate::schema::find_by_id(id).expect("typed fixture has generated schema");
            let schema_bound = crate::lludp::schema_message::SchemaMessage::allocation_bound(metadata, &message_bytes[id_len..])
                .expect("generated schema bounds typed message body");
            let packet = Packet::decode(&bytes).expect("decode typed fixture");
            let retained_message_bytes = packet.message.retained_capacity_bytes() + packet.trailing_message_bytes.capacity();

            assert_eq!(Message::allocation_strategy(id), AllocationStrategy::SchemaDerived, "schema route for {id:?}");
            assert!(
                schema_bound >= retained_message_bytes,
                "schema bound {schema_bound} covers {} retained bytes for {id:?}",
                retained_message_bytes
            );
        }

        let zero_coded = Packet::new(10, Message::PacketAck(PacketAck { packets: vec![Packets { id: 0 }] }))
            .with_zero_coding(true)
            .encode()
            .expect("encode zero-coded schema-shaped typed message");
        assert!(Packet::inspect(&zero_coded).expect("inspect zero-coded packet").flags.contains(PacketFlags::ZEROCODED));
        let preflight = Packet::preflight(&zero_coded).expect("preflight zero-coded schema-shaped typed message");
        assert_eq!(preflight.allocation_strategy(), AllocationStrategy::SchemaDerived);
        assert!(
            preflight.retained_capacity_upper_bound()
                >= Packet::decode(&zero_coded).expect("decode zero-coded packet").retained_capacity_bytes()
        );
    }
}
