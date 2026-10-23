mod frame;

pub use self::frame::{PacketFrame, PacketPreflight, PreflightedPacketFrame};

use std::{mem::size_of, ops::BitOr};

use tracing::Span;

use crate::schema;

use super::{error::LLUdpError, message::Message, zero_code};

const FLAG_ZEROCODED: u8 = 0x80;
const FLAG_RELIABLE: u8 = 0x40;
const FLAG_RESENT: u8 = 0x20;
const FLAG_APPENDED_ACKS: u8 = 0x10;
const HEADER_BYTES: usize = 6;
const MAX_LLUDP_PACKET_SIZE: usize = 8 * 1024;

pub const MAX_PACKET_RETAINED_CAPACITY_BYTES: usize = 3 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PacketFlags(u8);

impl PacketFlags {
    pub const ZEROCODED: Self = Self(FLAG_ZEROCODED);
    pub const RELIABLE: Self = Self(FLAG_RELIABLE);

    pub const RESENT: Self = Self(FLAG_RESENT);
    pub const APPENDED_ACKS: Self = Self(FLAG_APPENDED_ACKS);

    pub const fn from_bits_retain(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, flags: Self) -> bool {
        self.0 & flags.0 == flags.0
    }

    fn insert(&mut self, flags: Self) {
        self.0 |= flags.0;
    }

    fn remove(&mut self, flags: Self) {
        self.0 &= !flags.0;
    }
}

impl BitOr for PacketFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PacketHeader {
    sequence: u32,
    flags: PacketFlags,
    extra_header: Vec<u8>,
}

impl PacketHeader {
    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    pub fn flags(&self) -> PacketFlags {
        self.flags
    }

    pub fn extra_header(&self) -> &[u8] {
        &self.extra_header
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ZeroCoding {
    Automatic(bool),
    Received,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    header: PacketHeader,
    message: Message,
    trailing_message_bytes: Vec<u8>,
    appended_acks: Vec<u32>,
    zero_coding: ZeroCoding,
}

impl Packet {
    pub fn new(sequence: u32, message: Message) -> Self {
        let zero_coding = ZeroCoding::Automatic(message.default_zerocoded());
        Self {
            header: PacketHeader { sequence, flags: PacketFlags::default(), extra_header: Vec::new() },
            message,
            trailing_message_bytes: Vec::new(),
            appended_acks: Vec::new(),
            zero_coding,
        }
    }

    pub fn header(&self) -> Result<PacketHeader, LLUdpError> {
        let _: u8 = self.header.extra_header.len().try_into().map_err(|_| LLUdpError::ExtraHeaderTooLong)?;
        let _: u8 = self.appended_acks.len().try_into().map_err(|_| LLUdpError::TooManyAcks)?;
        let flags = match self.zero_coding {
            ZeroCoding::Received => self.header.flags,
            ZeroCoding::Automatic(_) => self.encoded_message()?.1,
        };
        Ok(PacketHeader { sequence: self.header.sequence, flags, extra_header: self.header.extra_header.clone() })
    }

    pub fn sequence(&self) -> u32 {
        self.header.sequence
    }

    pub fn message(&self) -> &Message {
        &self.message
    }

    pub fn into_message(self) -> Message {
        self.message
    }

    pub fn with_message(mut self, message: Message) -> Self {
        if message.id() != self.message.id() {
            self.trailing_message_bytes.clear();
        }
        if let ZeroCoding::Automatic(_) = self.zero_coding {
            self.zero_coding = ZeroCoding::Automatic(message.default_zerocoded());
        }
        self.message = message;
        self
    }

    pub fn is_reliable(&self) -> bool {
        self.header.flags.contains(PacketFlags::RELIABLE)
    }

    pub fn is_resent(&self) -> bool {
        self.header.flags.contains(PacketFlags::RESENT)
    }

    pub fn is_zerocoded(&self) -> Result<bool, LLUdpError> {
        self.header().map(|header| header.flags.contains(PacketFlags::ZEROCODED))
    }

    pub fn appended_acks(&self) -> &[u32] {
        &self.appended_acks
    }

    pub fn retained_capacity_bytes(&self) -> usize {
        self.header
            .extra_header
            .capacity()
            .saturating_add(self.trailing_message_bytes.capacity())
            .saturating_add(self.appended_acks.capacity().saturating_mul(size_of::<u32>()))
            .saturating_add(self.message.retained_capacity_bytes())
    }

    pub fn extra_header(&self) -> &[u8] {
        &self.header.extra_header
    }

    pub fn trailing_message_bytes(&self) -> &[u8] {
        &self.trailing_message_bytes
    }

    pub fn with_reliable(mut self) -> Self {
        self.header.flags.insert(PacketFlags::RELIABLE);
        self
    }

    pub fn with_resent(mut self) -> Self {
        self.header.flags.insert(PacketFlags::RELIABLE | PacketFlags::RESENT);
        self
    }

    pub fn with_zero_coding(mut self, enabled: bool) -> Self {
        self.zero_coding = ZeroCoding::Automatic(enabled);
        self.header.flags.remove(PacketFlags::ZEROCODED);
        self
    }

    pub fn with_extra_header(mut self, bytes: Vec<u8>) -> Self {
        self.header.extra_header = bytes;
        self
    }

    pub fn with_appended_acks(mut self, packet_ids: Vec<u32>) -> Self {
        self.appended_acks = packet_ids;
        if self.appended_acks.is_empty() {
            self.header.flags.remove(PacketFlags::APPENDED_ACKS);
        } else {
            self.header.flags.insert(PacketFlags::APPENDED_ACKS);
        }
        self
    }

    pub fn encode(&self) -> Result<Vec<u8>, LLUdpError> {
        let mut packet = Vec::new();
        self.encode_into(&mut packet)?;
        Ok(packet)
    }

    #[tracing::instrument(
        name = "protocol.packet.encode", level = "debug", skip_all,
        fields(sequence = self.header.sequence, message = tracing::field::Empty, datagram_bytes = tracing::field::Empty)
    )]
    pub fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), LLUdpError> {
        let output_start = output.len();
        if self.is_resent() && !self.is_reliable() {
            return Err(LLUdpError::InvalidPacketFlags);
        }
        let extra_len = u8::try_from(self.header.extra_header.len()).map_err(|_| LLUdpError::ExtraHeaderTooLong)?;
        let ack_count = u8::try_from(self.appended_acks.len()).map_err(|_| LLUdpError::TooManyAcks)?;

        let (message_bytes, flags) = self.encoded_message()?;

        let packet_len = self.framed_len(message_bytes.len(), flags.contains(PacketFlags::APPENDED_ACKS))?;

        output.reserve(packet_len);
        output.push(flags.bits());
        output.extend_from_slice(&self.header.sequence.to_be_bytes());
        output.push(extra_len);
        output.extend_from_slice(&self.header.extra_header);
        output.extend_from_slice(&message_bytes);
        if flags.contains(PacketFlags::APPENDED_ACKS) {
            for packet_id in &self.appended_acks {
                output.extend_from_slice(&packet_id.to_be_bytes());
            }
            output.push(ack_count);
        }
        let message_name = schema::find_by_id(self.message.id()).map_or("Unknown", |metadata| metadata.name);
        Span::current().record("message", message_name);
        Span::current().record("datagram_bytes", output.len() - output_start);
        Ok(())
    }

    fn framed_len(&self, body_bytes: usize, has_ack_section: bool) -> Result<usize, LLUdpError> {
        let length = HEADER_BYTES
            .checked_add(self.header.extra_header.len())
            .and_then(|length| length.checked_add(body_bytes))
            .and_then(|length| self.appended_acks.len().checked_mul(4).and_then(|bytes| length.checked_add(bytes)))
            .and_then(|length| length.checked_add(usize::from(has_ack_section)))
            .ok_or(LLUdpError::DatagramTooLarge)?;
        if length > MAX_LLUDP_PACKET_SIZE {
            return Err(LLUdpError::DatagramTooLarge);
        }
        Ok(length)
    }

    fn encoded_message(&self) -> Result<(Vec<u8>, PacketFlags), LLUdpError> {
        let mut message_bytes = Vec::new();
        self.message.id().encode(&mut message_bytes)?;
        self.message.encode_payload(&mut message_bytes, MAX_LLUDP_PACKET_SIZE)?;
        if self.trailing_message_bytes.len() > MAX_LLUDP_PACKET_SIZE.saturating_sub(message_bytes.len()) {
            return Err(LLUdpError::DatagramTooLarge);
        }
        message_bytes.extend_from_slice(&self.trailing_message_bytes);

        let has_ack_section = !self.appended_acks.is_empty() || self.header.flags.contains(PacketFlags::APPENDED_ACKS);
        self.framed_len(message_bytes.len(), has_ack_section)?;

        let mut flags = self.header.flags;
        match self.zero_coding {
            ZeroCoding::Automatic(enabled) => {
                flags.remove(PacketFlags::ZEROCODED);
                if enabled {
                    let compressed = zero_code::encode(&message_bytes);
                    if compressed.len() < message_bytes.len() {
                        message_bytes = compressed;
                        flags.insert(PacketFlags::ZEROCODED);
                    }
                }
            }
            ZeroCoding::Received if flags.contains(PacketFlags::ZEROCODED) => {
                message_bytes = zero_code::encode(&message_bytes);
            }
            ZeroCoding::Received => {}
        }

        if self.appended_acks.is_empty() && !self.header.flags.contains(PacketFlags::APPENDED_ACKS) {
            flags.remove(PacketFlags::APPENDED_ACKS);
        } else {
            flags.insert(PacketFlags::APPENDED_ACKS);
        }

        Ok((message_bytes, flags))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        lludp::{AllocationStrategy, MessageId},
        messages::{CompletePingCheck, PacketAck, PingId, circuit::packet_ack::Packets},
    };

    use super::*;

    fn decode_preflighted(bytes: &[u8]) -> Result<Packet, LLUdpError> {
        Packet::inspect(bytes)?.preflighted()?.decode()
    }

    #[test]
    fn resent_requires_reliable() {
        let mut packet = Packet::new(1, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }));
        packet.header.flags.insert(PacketFlags::RESENT);
        assert_eq!(packet.encode(), Err(LLUdpError::InvalidPacketFlags));
    }

    #[test]
    fn packet_size_limit() {
        let id = MessageId::low(0xfff9);
        let payload = vec![0x55; MAX_LLUDP_PACKET_SIZE - 10];
        let valid = Packet::new(1, Message::Unknown { id, payload: payload.clone() }).encode().expect("maximum-sized packet");
        assert_eq!(valid.len(), MAX_LLUDP_PACKET_SIZE);
        assert_eq!(Packet::decode(&valid).expect("decode maximum-sized packet").message(), &Message::Unknown { id, payload });

        let oversized = Packet::new(1, Message::Unknown { id, payload: vec![0x55; MAX_LLUDP_PACKET_SIZE - 9] });
        assert_eq!(oversized.encode(), Err(LLUdpError::DatagramTooLarge));

        let mut oversized_datagram = valid;
        oversized_datagram.push(0x55);
        assert_eq!(Packet::decode(&oversized_datagram), Err(LLUdpError::DatagramTooLarge));
    }

    #[test]
    fn retained_capacity_counts_packet_and_message_buffers() {
        let mut payload = Vec::with_capacity(128);
        payload.extend_from_slice(&[0x55; 32]);
        let mut packet = Packet::new(1, Message::Unknown { id: MessageId::low(0xfff9), payload });
        packet.header.extra_header = Vec::with_capacity(7);
        packet.trailing_message_bytes = Vec::with_capacity(9);
        packet.appended_acks = Vec::with_capacity(3);

        assert_eq!(packet.retained_capacity_bytes(), 128 + 7 + 9 + 3 * size_of::<u32>());
    }

    #[test]
    fn oversized_unknown_payload() {
        let packet = Packet::new(1, Message::Unknown { id: MessageId::high(1), payload: vec![0x55; MAX_LLUDP_PACKET_SIZE * 128] });

        assert_eq!(packet.encode(), Err(LLUdpError::DatagramTooLarge));
    }

    #[test]
    fn encoded_expansion_limit() {
        let packet =
            Packet::new(1, Message::Unknown { id: MessageId::high(1), payload: vec![0; MAX_LLUDP_PACKET_SIZE] }).with_zero_coding(true);

        assert_eq!(packet.encode(), Err(LLUdpError::DatagramTooLarge));
    }

    #[test]
    fn bad_id_precedes_expansion() {
        let datagram = [0x80, 0, 0, 0, 1, 0, 0, 4, 0];
        assert_eq!(Packet::decode(&datagram), Err(LLUdpError::InvalidMessageId));
        assert_eq!(decode_preflighted(&datagram), Err(LLUdpError::InvalidMessageId));
    }

    #[test]
    fn preflighted_decode_matches_packet_decode_for_frame_variants() {
        let ordinary = Packet::new(1, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }))
            .encode()
            .expect("encode ordinary packet");
        let zero_coded = Packet::new(2, Message::Unknown { id: MessageId::high(0x71), payload: vec![0; 64] })
            .with_zero_coding(true)
            .encode()
            .expect("encode zero-coded packet");
        let appended_acks = Packet::new(3, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 2 } }))
            .with_appended_acks(vec![7, 0x0102_0304])
            .encode()
            .expect("encode packet with appended ACKs");

        assert_eq!(ordinary[0] & PacketFlags::ZEROCODED.bits(), 0);
        assert_ne!(zero_coded[0] & PacketFlags::ZEROCODED.bits(), 0);
        assert_eq!(
            Packet::inspect(&appended_acks).expect("inspect appended-ACK packet").appended_acks().collect::<Vec<_>>(),
            [7, 0x0102_0304]
        );

        for wire in [&ordinary, &zero_coded, &appended_acks] {
            let preflighted = Packet::inspect(wire).expect("inspect packet").preflighted().expect("preflight packet");
            assert_eq!(preflighted.preflight(), Packet::preflight(wire).expect("preflight same packet"));
            assert_eq!(preflighted.decode(), Packet::decode(wire));
        }
    }

    #[test]
    fn preflighted_decode_matches_packet_decode_for_malformed_packets() {
        let mut truncated_message = Packet::new(1, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }))
            .encode()
            .expect("encode CompletePingCheck");
        truncated_message.pop();
        assert!(Packet::preflight(&truncated_message).is_ok(), "no-allocation preflight does not decode typed fields");
        assert_eq!(Packet::decode(&truncated_message), Err(LLUdpError::TruncatedPacket));
        assert_eq!(decode_preflighted(&truncated_message), Packet::decode(&truncated_message));

        let malformed_zero_code = [0x80, 0, 0, 0, 1, 0, 0];
        assert_eq!(Packet::decode(&malformed_zero_code), Err(LLUdpError::InvalidZeroCode));
        assert_eq!(decode_preflighted(&malformed_zero_code), Packet::decode(&malformed_zero_code));

        let truncated_header = [0, 0, 0, 0, 1];
        assert_eq!(Packet::decode(&truncated_header), Err(LLUdpError::TruncatedPacket));
        assert_eq!(decode_preflighted(&truncated_header), Packet::decode(&truncated_header));
    }

    #[test]
    fn zerocode_ids_preflight() {
        for id in [MessageId::high(1), MessageId::medium(1), MessageId::low(0x0100), MessageId::fixed(0xffff_fffa)] {
            let mut encoded_id = Vec::new();
            id.encode(&mut encoded_id).expect("encode test message ID");
            let zerocoded_id = zero_code::encode(&encoded_id);
            let mut prefix = [0; 4];
            let prefix_len = zero_code::decode_prefix(&zerocoded_id, &mut prefix).expect("decode message ID prefix");

            assert_eq!(MessageId::decode(&prefix[..prefix_len]), Ok((id, encoded_id.len())));
        }
    }

    #[test]
    fn preflight_reports_routes_and_bounds_retained_capacity() {
        let typed = Packet::new(1, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }))
            .encode()
            .expect("encode typed packet");
        let typed_bound = Packet::preflight(&typed).expect("preflight typed packet");
        let decoded = Packet::decode(&typed).expect("decode typed packet");
        assert_eq!(typed_bound.allocation_strategy(), AllocationStrategy::NoAlloc);
        assert!(typed_bound.retained_capacity_upper_bound() >= decoded.retained_capacity_bytes());

        let unknown_id = MessageId::high(0x71);
        let opaque = Packet::new(2, Message::Unknown { id: unknown_id, payload: vec![0, 1, 0, 2, 3] })
            .with_zero_coding(true)
            .encode()
            .expect("encode opaque packet");
        let opaque_bound = Packet::preflight(&opaque).expect("preflight zerocoded opaque packet");
        let decoded = Packet::decode(&opaque).expect("decode zerocoded opaque packet");
        assert_eq!(opaque_bound.message_id(), unknown_id);
        assert_eq!(opaque_bound.allocation_strategy(), AllocationStrategy::Opaque);
        assert!(opaque_bound.retained_capacity_upper_bound() >= decoded.retained_capacity_bytes());
        assert!(opaque_bound.peak_decode_allocation_bytes() >= opaque_bound.retained_capacity_upper_bound());
    }

    #[test]
    fn object_shape_padding_is_preflighted() {
        let mut bytes = vec![0, 0, 0, 0, 1, 0, 0xff, 0xff, 0, 98];
        bytes.extend_from_slice(&[0; 32]);
        bytes.push(u8::MAX);
        assert_eq!(bytes.len(), 43);

        let preflight = Packet::preflight(&bytes).expect("bound omitted ObjectShape records before decoding");
        assert_eq!(preflight.message_id(), MessageId::low(98));
        assert!(preflight.retained_capacity_upper_bound() > bytes.len());
        let decoded = Packet::decode(&bytes).expect("preserve fixed-record compatibility padding");
        let Message::Schema(schema) = decoded.message() else { panic!("ObjectShape remains schema-backed") };

        assert_eq!(schema.blocks()[1].instances.len(), usize::from(u8::MAX));
        assert_eq!(schema.field_was_supplied(1, 0, 0), Some(false));
        assert!(preflight.retained_capacity_upper_bound() >= decoded.retained_capacity_bytes());
    }

    #[test]
    fn maximum_packet_ack_is_bounded() {
        let packets = (0..u8::MAX).map(|id| Packets { id: u32::from(id) }).collect();
        let bytes = Packet::new(1, Message::PacketAck(PacketAck { packets })).encode().expect("encode 255 valid ACK entries");
        let preflight = Packet::preflight(&bytes).expect("preflight 255 ACK entries");
        let decoded = Packet::decode(&bytes).expect("decode 255 ACK entries");
        assert!(preflight.retained_capacity_upper_bound() >= decoded.retained_capacity_bytes());
        assert!(matches!(decoded.message(), Message::PacketAck(message) if message.packets.len() == usize::from(u8::MAX)));
    }

    #[test]
    fn frame_reads_message_id() {
        let packet = Packet::new(1, Message::PacketAck(PacketAck { packets: vec![Packets { id: 0 }] })).with_zero_coding(true);
        let bytes = packet.encode().expect("encode zero-coded PacketAck");
        let frame = Packet::inspect(&bytes).expect("inspect PacketAck");

        assert_eq!(frame.message_id(), Ok(crate::schema::ids::PACKET_ACK));
    }

    #[test]
    fn typed_preflight_covers_packed_compatibility() {
        use test_support::{lludp::vectors, parse_hex_fixture};

        for fixture in [
            vectors::REGION_HANDSHAKE,
            vectors::REGION_HANDSHAKE_INFO4,
            vectors::REGION_HANDSHAKE_LEGACY,
            vectors::OBJECT_UPDATE,
            vectors::OBJECT_UPDATE_COMPRESSED_TYPED,
            vectors::IMPROVED_TERSE_OBJECT_UPDATE,
            vectors::IMPROVED_TERSE_OBJECT_UPDATE_AVATAR,
        ] {
            let bytes = parse_hex_fixture(fixture).expect("specialized fixture is valid hex");
            let preflight = Packet::preflight(&bytes).expect("specialized route bounds fixture allocations");
            let packet = Packet::decode(&bytes).unwrap_or_else(|error| panic!("decode specialized typed fixture {fixture:?}: {error}"));

            assert_eq!(preflight.allocation_strategy(), AllocationStrategy::Specialized);
            assert!(
                preflight.retained_capacity_upper_bound() >= packet.retained_capacity_bytes(),
                "specialized bound {} covers {} retained bytes for {:?}",
                preflight.retained_capacity_upper_bound(),
                packet.retained_capacity_bytes(),
                preflight.message_id()
            );
        }

        let truncated = parse_hex_fixture(vectors::OBJECT_UPDATE_COMPRESSED).expect("truncated fixture is valid hex");
        assert!(matches!(Packet::preflight(&truncated), Err(LLUdpError::TruncatedPacket)));
        assert_eq!(Packet::decode(&truncated), Err(LLUdpError::TruncatedPacket));

        for fixture in [vectors::OBJECT_UPDATE, vectors::IMPROVED_TERSE_OBJECT_UPDATE] {
            let mut truncated = parse_hex_fixture(fixture).expect("packed update fixture is valid hex");
            truncated.pop();

            assert!(
                matches!(Packet::preflight(&truncated), Err(LLUdpError::TruncatedPacket)),
                "preflight rejects a truncated record in {fixture:?}"
            );
        }
    }
}
