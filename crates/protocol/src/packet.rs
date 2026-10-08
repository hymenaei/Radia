use crate::{error::UdpError, message::Message, schema, types::MessageId, zero_code};

const FLAG_ZEROCODED: u8 = 0x80;
const FLAG_RELIABLE: u8 = 0x40;
const FLAG_RESENT: u8 = 0x20;
const FLAG_APPENDED_ACKS: u8 = 0x10;
const MAX_UDP_PAYLOAD_SIZE: usize = 0x2000;

/// The bit flags carried in an LLUDP packet header.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PacketFlags(u8);

impl PacketFlags {
    pub const ZEROCODED: Self = Self(FLAG_ZEROCODED);
    pub const RELIABLE: Self = Self(FLAG_RELIABLE);
    /// The packet is a retransmission; LLUDP also requires [`Self::RELIABLE`].
    pub const RESENT: Self = Self(FLAG_RESENT);
    pub const APPENDED_ACKS: Self = Self(FLAG_APPENDED_ACKS);

    /// Retains all bits, including bits this version does not interpret.
    pub const fn from_bits_retain(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the raw flag byte, including bits this version does not interpret.
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

impl std::ops::BitOr for PacketFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

/// A decoded or encoded LLUDP packet header.
///
/// The sequence number is represented as a host-order `u32`. The extra header
/// borrows its meaning from the message protocol and appears before the message
/// ID on the wire.
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

/// One LLUDP datagram with a typed message and preserved packet metadata.
///
/// Encoded and decoded datagrams are limited to 8 KiB. New packets choose their
/// default zero-coding behavior from the message template; decoded packets
/// retain the received flags, header extension, appended ACKs, and unparsed
/// message bytes.
#[derive(Clone, Debug, PartialEq)]
pub struct Packet {
    header: PacketHeader,
    message: Message,
    trailing_message_bytes: Vec<u8>,
    appended_acks: Vec<u32>,
    zero_coding: ZeroCoding,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ZeroCoding {
    Automatic(bool),
    Received,
}

impl Packet {
    /// Creates a packet with the given sequence number and message.
    ///
    /// The sequence number is written as a big-endian `u32`. Reliability is
    /// unset, and zero coding follows the message template's default.
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

    /// Returns the header that encoding this packet will write.
    ///
    /// For packets with automatic zero coding, this encodes the message to
    /// determine whether zero coding saves bytes.
    ///
    /// # Errors
    ///
    /// With automatic zero coding, returns an error if encoding the message
    /// fails or the packet exceeds LLUDP limits. Also returns an error when
    /// the extra header or appended ACK count cannot be represented on wire.
    pub fn header(&self) -> Result<PacketHeader, UdpError> {
        let _: u8 = self.header.extra_header.len().try_into().map_err(|_| UdpError::ExtraHeaderTooLong)?;
        let _: u8 = self.appended_acks.len().try_into().map_err(|_| UdpError::TooManyAcks)?;
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

    /// Retains the header and flags. Trailing bytes are retained only when the message ID stays the same.
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

    /// Whether encoding this packet will set the wire zero-code flag.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::header`].
    pub fn is_zerocoded(&self) -> Result<bool, UdpError> {
        self.header().map(|header| header.flags.contains(PacketFlags::ZEROCODED))
    }

    /// Returns ACK packet IDs in the order stored by this packet.
    /// The wire representation stores each ID as a little-endian `u32`.
    pub fn appended_acks(&self) -> &[u32] {
        &self.appended_acks
    }

    /// Packet header extension bytes between the fixed header and message ID.
    pub fn extra_header(&self) -> &[u8] {
        &self.header.extra_header
    }

    /// Message bytes remaining after the fields recognized by this protocol version.
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

    /// Attempts zero coding when enabled; the wire flag is set only when it saves bytes.
    pub fn with_zero_coding(mut self, enabled: bool) -> Self {
        self.zero_coding = ZeroCoding::Automatic(enabled);
        self.header.flags.remove(PacketFlags::ZEROCODED);
        self
    }

    /// Sets bytes between the fixed header and message ID.
    ///
    /// The bytes are preserved as supplied. Encoding returns
    /// [`UdpError::ExtraHeaderTooLong`] if they exceed 255 bytes.
    pub fn with_extra_header(mut self, bytes: Vec<u8>) -> Self {
        self.header.extra_header = bytes;
        self
    }

    /// Sets appended ACK IDs in wire order and returns this packet.
    ///
    /// The ACK flag is cleared when `packet_ids` is empty and set otherwise.
    /// Each ID is encoded as a little-endian `u32`. Encoding rejects more than
    /// 255 IDs with [`UdpError::TooManyAcks`].
    pub fn with_appended_acks(mut self, packet_ids: Vec<u32>) -> Self {
        self.appended_acks = packet_ids;
        if self.appended_acks.is_empty() {
            self.header.flags.remove(PacketFlags::APPENDED_ACKS);
        } else {
            self.header.flags.insert(PacketFlags::APPENDED_ACKS);
        }
        self
    }

    /// Encodes one LLUDP datagram, limited to 8 KiB.
    ///
    /// # Errors
    ///
    /// Returns an error if packet flags conflict, header or ACK limits are
    /// exceeded, the message cannot be encoded, or the datagram exceeds 8 KiB.
    pub fn encode(&self) -> Result<Vec<u8>, UdpError> {
        let mut packet = Vec::new();
        self.encode_into(&mut packet)?;
        Ok(packet)
    }

    /// Appends one LLUDP datagram to `output` without changing it on error.
    /// The 8 KiB limit applies to this datagram, not existing output bytes.
    ///
    /// # Errors
    ///
    /// Returns the same errors as [`Self::encode`].
    #[tracing::instrument(
        name = "protocol.packet.encode", level = "debug", skip_all,
        fields(sequence = self.header.sequence, message = tracing::field::Empty, datagram_bytes = tracing::field::Empty)
    )]
    pub fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), UdpError> {
        let output_start = output.len();
        if self.is_resent() && !self.is_reliable() {
            return Err(UdpError::InvalidPacketFlags);
        }
        let extra_len = u8::try_from(self.header.extra_header.len()).map_err(|_| UdpError::ExtraHeaderTooLong)?;
        let ack_count = u8::try_from(self.appended_acks.len()).map_err(|_| UdpError::TooManyAcks)?;

        let (message_bytes, flags) = self.encoded_message()?;

        let packet_len = 6usize
            .checked_add(self.header.extra_header.len())
            .and_then(|length| length.checked_add(message_bytes.len()))
            .and_then(|length| length.checked_add(self.appended_acks.len() * 4))
            .and_then(|length| length.checked_add(usize::from(flags.contains(PacketFlags::APPENDED_ACKS))))
            .ok_or(UdpError::DatagramTooLarge)?;
        if packet_len > MAX_UDP_PAYLOAD_SIZE {
            return Err(UdpError::DatagramTooLarge);
        }

        output.reserve(packet_len);
        output.push(flags.bits());
        output.extend_from_slice(&self.header.sequence.to_be_bytes());
        output.push(extra_len);
        output.extend_from_slice(&self.header.extra_header);
        output.extend_from_slice(&message_bytes);
        if flags.contains(PacketFlags::APPENDED_ACKS) {
            for packet_id in &self.appended_acks {
                output.extend_from_slice(&packet_id.to_le_bytes());
            }
            output.push(ack_count);
        }
        let message_name = schema::find_by_id(self.message.id()).map_or("Unknown", |metadata| metadata.name);
        tracing::Span::current().record("message", message_name);
        tracing::Span::current().record("datagram_bytes", output.len() - output_start);
        Ok(())
    }

    fn encoded_message(&self) -> Result<(Vec<u8>, PacketFlags), UdpError> {
        let mut message_bytes = Vec::new();
        self.message.id().encode(&mut message_bytes)?;
        self.message.encode_payload(&mut message_bytes, MAX_UDP_PAYLOAD_SIZE)?;
        if self.trailing_message_bytes.len() > MAX_UDP_PAYLOAD_SIZE.saturating_sub(message_bytes.len()) {
            return Err(UdpError::DatagramTooLarge);
        }
        message_bytes.extend_from_slice(&self.trailing_message_bytes);

        let has_ack_section = !self.appended_acks.is_empty() || self.header.flags.contains(PacketFlags::APPENDED_ACKS);
        let uncompressed_packet_len = 6usize
            .checked_add(self.header.extra_header.len())
            .and_then(|length| length.checked_add(message_bytes.len()))
            .and_then(|length| length.checked_add(self.appended_acks.len() * 4))
            .and_then(|length| length.checked_add(usize::from(has_ack_section)))
            .ok_or(UdpError::DatagramTooLarge)?;
        if uncompressed_packet_len > MAX_UDP_PAYLOAD_SIZE {
            return Err(UdpError::DatagramTooLarge);
        }

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

    #[tracing::instrument(
        name = "protocol.packet.decode", level = "debug", skip_all,
        fields(datagram_bytes = bytes.len(), sequence = tracing::field::Empty, reliable = tracing::field::Empty, message = tracing::field::Empty)
    )]
    /// Decodes one complete LLUDP datagram.
    ///
    /// The input is limited to 8 KiB. Unknown message IDs are retained as
    /// [`Message::Unknown`]. The decoder preserves unrecognized flag bits,
    /// header extension bytes, appended ACKs, and bytes after fields known to
    /// this protocol version.
    ///
    /// # Errors
    ///
    /// Returns an error for a truncated or malformed header, message ID,
    /// payload, zero-coded body, or appended ACK list.
    pub fn decode(bytes: &[u8]) -> Result<Self, UdpError> {
        if bytes.len() > MAX_UDP_PAYLOAD_SIZE {
            return Err(UdpError::DatagramTooLarge);
        }
        if bytes.len() < 6 {
            return Err(UdpError::TruncatedPacket);
        }
        let flags = PacketFlags::from_bits_retain(bytes[0]);
        if flags.contains(PacketFlags::RESENT) && !flags.contains(PacketFlags::RELIABLE) {
            return Err(UdpError::InvalidPacketFlags);
        }
        let sequence = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        tracing::Span::current().record("sequence", sequence);
        tracing::Span::current().record("reliable", flags.contains(PacketFlags::RELIABLE));
        let extra_len = usize::from(bytes[5]);
        let body_start = 6usize.checked_add(extra_len).ok_or(UdpError::TruncatedPacket)?;
        if bytes.len() < body_start {
            return Err(UdpError::TruncatedPacket);
        }

        let mut body_end = bytes.len();
        let mut appended_acks = Vec::new();
        if flags.contains(PacketFlags::APPENDED_ACKS) {
            let count = usize::from(*bytes.last().ok_or(UdpError::InvalidAcknowledgementList)?);
            let trailer_len = count.checked_mul(4).and_then(|length| length.checked_add(1)).ok_or(UdpError::InvalidAcknowledgementList)?;
            if bytes.len() < body_start + trailer_len {
                return Err(UdpError::InvalidAcknowledgementList);
            }
            body_end -= trailer_len;
            let ids = &bytes[body_end..bytes.len() - 1];
            let (chunks, trailing) = ids.as_chunks::<4>();
            if !trailing.is_empty() {
                return Err(UdpError::InvalidAcknowledgementList);
            }
            for chunk in chunks {
                appended_acks.push(u32::from_le_bytes(*chunk));
            }
        }

        let wire_body = &bytes[body_start..body_end];
        let (id, id_len, message_bytes) = if flags.contains(PacketFlags::ZEROCODED) {
            let mut message_id_prefix = [0; 4];
            let prefix_len = zero_code::decode_prefix(wire_body, &mut message_id_prefix)?;
            let (id, id_len) = MessageId::decode(&message_id_prefix[..prefix_len])?;
            let packet_overhead = body_start + (bytes.len() - body_end);
            let max_decoded_body_size = MAX_UDP_PAYLOAD_SIZE.saturating_sub(packet_overhead);
            (id, id_len, zero_code::decode(wire_body, max_decoded_body_size)?)
        } else {
            let (id, id_len) = MessageId::decode(wire_body)?;
            (id, id_len, wire_body.to_vec())
        };
        let (message, trailing_message_bytes) = Message::decode(id, &message_bytes[id_len..])?;
        let message_name = schema::find_by_id(id).map_or("Unknown", |metadata| metadata.name);
        tracing::Span::current().record("message", message_name);

        Ok(Self {
            header: PacketHeader { sequence, flags, extra_header: bytes[6..body_start].to_vec() },
            message,
            trailing_message_bytes,
            appended_acks,
            zero_coding: ZeroCoding::Received,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::messages::{CompletePingCheck, complete_ping_check::PingID};

    use super::*;

    #[test]
    fn resent_requires_reliable() {
        let mut packet = Packet::new(1, Message::CompletePingCheck(CompletePingCheck { ping_id: PingID { ping_id: 1 } }));
        packet.header.flags.insert(PacketFlags::RESENT);
        assert_eq!(packet.encode(), Err(UdpError::InvalidPacketFlags));
    }

    #[test]
    fn packet_size_limit() {
        let id = MessageId::low(0xfff9);
        let payload = vec![0x55; MAX_UDP_PAYLOAD_SIZE - 10];
        let valid = Packet::new(1, Message::Unknown { id, payload: payload.clone() }).encode().expect("maximum-sized packet");
        assert_eq!(valid.len(), MAX_UDP_PAYLOAD_SIZE);
        assert_eq!(Packet::decode(&valid).expect("decode maximum-sized packet").message(), &Message::Unknown { id, payload });

        let oversized = Packet::new(1, Message::Unknown { id, payload: vec![0x55; MAX_UDP_PAYLOAD_SIZE - 9] });
        assert_eq!(oversized.encode(), Err(UdpError::DatagramTooLarge));

        let mut oversized_datagram = valid;
        oversized_datagram.push(0x55);
        assert_eq!(Packet::decode(&oversized_datagram), Err(UdpError::DatagramTooLarge));
    }

    #[test]
    fn oversized_unknown_payload() {
        let packet = Packet::new(1, Message::Unknown { id: MessageId::high(1), payload: vec![0x55; MAX_UDP_PAYLOAD_SIZE * 128] });

        assert_eq!(packet.encode(), Err(UdpError::DatagramTooLarge));
    }

    #[test]
    fn encoded_expansion_limit() {
        let packet =
            Packet::new(1, Message::Unknown { id: MessageId::high(1), payload: vec![0; MAX_UDP_PAYLOAD_SIZE] }).with_zero_coding(true);

        assert_eq!(packet.encode(), Err(UdpError::DatagramTooLarge));
    }

    #[test]
    fn bad_id_precedes_expansion() {
        let datagram = [0x80, 0, 0, 0, 1, 0, 0, 4, 0];
        assert_eq!(Packet::decode(&datagram), Err(UdpError::InvalidMessageId));
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
}
