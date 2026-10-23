use std::{mem::size_of, ops::BitOr};

use tracing::Span;

use super::{
    error::LLUdpError,
    message::{AllocationStrategy, Message},
    types::MessageId,
    zero_code,
};
use crate::schema;

const FLAG_ZEROCODED: u8 = 0x80;
const FLAG_RELIABLE: u8 = 0x40;
const FLAG_RESENT: u8 = 0x20;
const FLAG_APPENDED_ACKS: u8 = 0x10;
const MAX_LLUDP_PACKET_SIZE: usize = 0x2000;
/// Conservative limit for memory capacity retained by one decoded packet.
///
/// The Network crate limits concurrent decoders and uses this ceiling as the conservative bound for each in-flight decode. Keep it above the
/// largest schema expansion and handwritten decoder allocation envelope, including maximum count prefixes and compatibility padding.
pub const MAX_PACKET_RETAINED_CAPACITY_BYTES: usize = 3 * 1024 * 1024;

/// A validated view of packet framing that borrows the original datagram.
pub struct PacketFrame<'a> {
    sequence: u32,
    flags: PacketFlags,
    extra_header: &'a [u8],
    wire_body: &'a [u8],
    appended_ack_bytes: &'a [u8],
}

impl<'a> PacketFrame<'a> {
    /// Packet sequence number from the fixed header.
    pub fn sequence(&self) -> u32 {
        self.sequence
    }

    /// Message ID from the body prefix.
    ///
    /// This reads only enough bytes to identify the message. Call [`Self::preflight`] or decode the packet to validate its full body.
    pub fn message_id(&self) -> Result<MessageId, LLUdpError> {
        if self.flags.contains(PacketFlags::ZEROCODED) {
            let mut prefix = [0; 4];
            let prefix_len = zero_code::decode_prefix(self.wire_body, &mut prefix)?;
            MessageId::decode(&prefix[..prefix_len]).map(|(id, _)| id)
        } else {
            MessageId::decode(self.wire_body).map(|(id, _)| id)
        }
    }

    /// Whether the packet sets the reliable flag.
    pub fn is_reliable(&self) -> bool {
        self.flags.contains(PacketFlags::RELIABLE)
    }

    /// Whether the packet sets the resent flag.
    pub fn is_resent(&self) -> bool {
        self.flags.contains(PacketFlags::RESENT)
    }

    /// Appended ACK IDs in wire order.
    pub fn appended_acks(&self) -> impl ExactSizeIterator<Item = u32> + '_ {
        self.appended_ack_bytes.as_chunks::<4>().0.iter().map(|chunk| u32::from_be_bytes(*chunk))
    }

    /// Inspects message framing and estimates decoded heap capacity without allocating decoded fields.
    pub fn preflight(&self) -> Result<PacketPreflight, LLUdpError> {
        self.preflight_values().map(|(preflight, _, _)| preflight)
    }

    /// Produces a view that binds the allocation estimate to this datagram and can decode it without repeating preflight.
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
                    6 + self.extra_header.len()
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

/// Checked decoded-allocation bounds produced by [`PacketFrame::preflight`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PacketPreflight {
    message_id: MessageId,
    allocation_strategy: AllocationStrategy,
    retained_capacity_upper_bound: usize,
    peak_decode_allocation_bytes: usize,
}

impl PacketPreflight {
    /// Message ID carried by the datagram.
    pub fn message_id(&self) -> MessageId {
        self.message_id
    }

    /// Allocation preflight route selected for this message ID.
    pub fn allocation_strategy(&self) -> AllocationStrategy {
        self.allocation_strategy
    }

    /// Upper bound on packet-owned heap capacities after decoding.
    pub fn retained_capacity_upper_bound(&self) -> usize {
        self.retained_capacity_upper_bound
    }

    /// Bound including temporary decoded message bytes held during packet decode.
    pub fn peak_decode_allocation_bytes(&self) -> usize {
        self.peak_decode_allocation_bytes
    }
}

/// A packet frame whose allocation preflight is bound to the same borrowed datagram.
pub struct PreflightedPacketFrame<'a> {
    frame: PacketFrame<'a>,
    preflight: PacketPreflight,
    message_id_len: usize,
    decoded_body_len: usize,
}

impl PreflightedPacketFrame<'_> {
    /// Allocation bounds validated against this frame's datagram.
    pub fn preflight(&self) -> PacketPreflight {
        self.preflight
    }

    /// Decodes the already-preflighted datagram without re-inspecting its framing or allocation bounds.
    #[tracing::instrument(
        name = "protocol.packet.decode",
        level = "debug",
        skip_all,
        fields(
            datagram_bytes = self.datagram_len(),
            sequence = tracing::field::Empty,
            reliable = tracing::field::Empty,
            message = tracing::field::Empty
        )
    )]
    pub fn decode(self) -> Result<Packet, LLUdpError> {
        let Self { frame, preflight, message_id_len, decoded_body_len } = self;
        let flags = frame.flags;
        let sequence = frame.sequence;
        Span::current().record("sequence", sequence);
        Span::current().record("reliable", flags.contains(PacketFlags::RELIABLE));

        let decode_payload = |message_bytes: &[u8]| Message::decode(preflight.message_id, &message_bytes[message_id_len..]);
        let (message, trailing_message_bytes) = if flags.contains(PacketFlags::ZEROCODED) {
            let mut decoded_body = Vec::with_capacity(decoded_body_len);
            zero_code::decode_into(frame.wire_body, &mut decoded_body, decoded_body_len)?;
            decode_payload(&decoded_body)?
        } else {
            decode_payload(frame.wire_body)?
        };
        let message_name = schema::find_by_id(preflight.message_id).map_or("Unknown", |metadata| metadata.name);
        Span::current().record("message", message_name);

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

    fn datagram_len(&self) -> usize {
        6 + self.frame.extra_header.len()
            + self.frame.wire_body.len()
            + self.frame.appended_ack_bytes.len()
            + usize::from(self.frame.flags.contains(PacketFlags::APPENDED_ACKS))
    }
}

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

impl BitOr for PacketFlags {
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
    /// Validates the datagram header, flags, and appended-ACK framing without decoding its message body.
    pub fn inspect(bytes: &[u8]) -> Result<PacketFrame<'_>, LLUdpError> {
        if bytes.len() > MAX_LLUDP_PACKET_SIZE {
            return Err(LLUdpError::DatagramTooLarge);
        }
        if bytes.len() < 6 {
            return Err(LLUdpError::TruncatedPacket);
        }

        let flags = PacketFlags::from_bits_retain(bytes[0]);
        if flags.contains(PacketFlags::RESENT) && !flags.contains(PacketFlags::RELIABLE) {
            return Err(LLUdpError::InvalidPacketFlags);
        }
        let sequence = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        let extra_len = usize::from(bytes[5]);
        let body_start = 6usize.checked_add(extra_len).ok_or(LLUdpError::TruncatedPacket)?;
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
            extra_header: &bytes[6..body_start],
            wire_body: &bytes[body_start..body_end],
            appended_ack_bytes,
        })
    }

    /// Validates message framing and estimates decoded allocations before fields are materialized.
    pub fn preflight(bytes: &[u8]) -> Result<PacketPreflight, LLUdpError> {
        Self::inspect(bytes)?.preflight()
    }

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
    pub fn is_zerocoded(&self) -> Result<bool, LLUdpError> {
        self.header().map(|header| header.flags.contains(PacketFlags::ZEROCODED))
    }

    /// Returns ACK packet IDs in the order stored by this packet.
    /// The wire representation stores each ID as a big-endian `u32`.
    pub fn appended_acks(&self) -> &[u32] {
        &self.appended_acks
    }

    /// Returns the capacity of heap-backed data retained by this packet.
    ///
    /// This includes vector and string capacities in the packet, its message, and schema fields. It excludes inline values and allocator
    /// bookkeeping, so callers that use the result as a resource budget should account for their own queue or envelope storage separately.
    pub fn retained_capacity_bytes(&self) -> usize {
        self.header
            .extra_header
            .capacity()
            .saturating_add(self.trailing_message_bytes.capacity())
            .saturating_add(self.appended_acks.capacity().saturating_mul(size_of::<u32>()))
            .saturating_add(self.message.retained_capacity_bytes())
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
    /// [`LLUdpError::ExtraHeaderTooLong`] if they exceed 255 bytes.
    pub fn with_extra_header(mut self, bytes: Vec<u8>) -> Self {
        self.header.extra_header = bytes;
        self
    }

    /// Sets appended ACK IDs in wire order and returns this packet.
    ///
    /// The ACK flag is cleared when `packet_ids` is empty and set otherwise.
    /// Each ID is encoded as a big-endian `u32`. Encoding rejects more than
    /// 255 IDs with [`LLUdpError::TooManyAcks`].
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
    pub fn encode(&self) -> Result<Vec<u8>, LLUdpError> {
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
    pub fn encode_into(&self, output: &mut Vec<u8>) -> Result<(), LLUdpError> {
        let output_start = output.len();
        if self.is_resent() && !self.is_reliable() {
            return Err(LLUdpError::InvalidPacketFlags);
        }
        let extra_len = u8::try_from(self.header.extra_header.len()).map_err(|_| LLUdpError::ExtraHeaderTooLong)?;
        let ack_count = u8::try_from(self.appended_acks.len()).map_err(|_| LLUdpError::TooManyAcks)?;

        let (message_bytes, flags) = self.encoded_message()?;

        let packet_len = 6usize
            .checked_add(self.header.extra_header.len())
            .and_then(|length| length.checked_add(message_bytes.len()))
            .and_then(|length| length.checked_add(self.appended_acks.len() * 4))
            .and_then(|length| length.checked_add(usize::from(flags.contains(PacketFlags::APPENDED_ACKS))))
            .ok_or(LLUdpError::DatagramTooLarge)?;
        if packet_len > MAX_LLUDP_PACKET_SIZE {
            return Err(LLUdpError::DatagramTooLarge);
        }

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

    fn encoded_message(&self) -> Result<(Vec<u8>, PacketFlags), LLUdpError> {
        let mut message_bytes = Vec::new();
        self.message.id().encode(&mut message_bytes)?;
        self.message.encode_payload(&mut message_bytes, MAX_LLUDP_PACKET_SIZE)?;
        if self.trailing_message_bytes.len() > MAX_LLUDP_PACKET_SIZE.saturating_sub(message_bytes.len()) {
            return Err(LLUdpError::DatagramTooLarge);
        }
        message_bytes.extend_from_slice(&self.trailing_message_bytes);

        let has_ack_section = !self.appended_acks.is_empty() || self.header.flags.contains(PacketFlags::APPENDED_ACKS);
        let uncompressed_packet_len = 6usize
            .checked_add(self.header.extra_header.len())
            .and_then(|length| length.checked_add(message_bytes.len()))
            .and_then(|length| length.checked_add(self.appended_acks.len() * 4))
            .and_then(|length| length.checked_add(usize::from(has_ack_section)))
            .ok_or(LLUdpError::DatagramTooLarge)?;
        if uncompressed_packet_len > MAX_LLUDP_PACKET_SIZE {
            return Err(LLUdpError::DatagramTooLarge);
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
    pub fn decode(bytes: &[u8]) -> Result<Self, LLUdpError> {
        Self::inspect(bytes)?.preflighted()?.decode()
    }
}

#[cfg(test)]
mod tests {
    use crate::messages::{CompletePingCheck, PacketAck, PingId, packet_ack::Packets};

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

        assert!(!Packet::inspect(&ordinary).expect("inspect ordinary packet").flags.contains(PacketFlags::ZEROCODED));
        assert!(Packet::inspect(&zero_coded).expect("inspect zero-coded packet").flags.contains(PacketFlags::ZEROCODED));
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
    fn schema_preflight_bounds_schema_shaped_typed_decoders() {
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
                    - 6
                    - frame.extra_header.len()
                    - frame.appended_ack_bytes.len()
                    - usize::from(frame.flags.contains(PacketFlags::APPENDED_ACKS));
                zero_code::decode(frame.wire_body, max_size).expect("decode fixture zero coding")
            } else {
                frame.wire_body.to_vec()
            };
            let (id, id_len) = MessageId::decode(&message_bytes).expect("decode fixture message ID");
            let metadata = schema::find_by_id(id).expect("typed fixture has generated schema");
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
