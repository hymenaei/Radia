use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// An LLUDP packet or message could not be decoded or encoded.
pub enum LLUdpError {
    /// The packet header, message ID, or typed message body ended early.
    TruncatedPacket,
    /// The message ID's frequency prefix or ID value is invalid.
    InvalidMessageId,
    /// A typed message has malformed fields or violates its wire constraints.
    InvalidMessage,
    /// A zero-coded body is malformed or expands past the 8 KiB packet limit.
    InvalidZeroCode,
    /// The appended-ACK trailer is truncated or its count does not match its bytes.
    InvalidAcknowledgementList,
    /// The resent flag is set without the reliable flag.
    InvalidPacketFlags,
    /// An encoded or decoded LLUDP datagram exceeds 8 KiB.
    DatagramTooLarge,
    /// Decoding would retain more memory capacity than the packet limit.
    DecodedPacketTooLarge,
    /// The checked decoded-allocation estimate overflowed `usize`.
    AllocationBoundOverflow,
    /// The extra header contains more than 255 bytes.
    ExtraHeaderTooLong,
    /// An ACK block contains more than 255 packet IDs.
    TooManyAcks,
    /// A variable field cannot fit its wire length prefix or count.
    VariableFieldTooLong,
    /// A typed string field contains invalid UTF-8 before its terminator.
    InvalidUtf8,
    /// A schema-backed message's block count or field values do not match its schema.
    InvalidSchemaMessage,
    /// Encoding was requested for a typed message defined as receive-only.
    InboundMessage,
}

impl Display for LLUdpError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::TruncatedPacket => "truncated LLUDP packet or message",
            Self::InvalidMessageId => "invalid LLUDP message ID",
            Self::InvalidMessage => "invalid LLUDP message fields",
            Self::InvalidZeroCode => "invalid LLUDP zero-coded body",
            Self::InvalidAcknowledgementList => "invalid LLUDP appended ACK list",
            Self::InvalidPacketFlags => "resent LLUDP packets must also be reliable",
            Self::DatagramTooLarge => "LLUDP datagram exceeds the maximum UDP payload size",
            Self::DecodedPacketTooLarge => "decoded LLUDP packet exceeds the retained-memory limit",
            Self::AllocationBoundOverflow => "decoded LLUDP allocation estimate overflowed",
            Self::ExtraHeaderTooLong => "LLUDP extra header exceeds 255 bytes",
            Self::TooManyAcks => "LLUDP packet cannot carry more than 255 ACKs",
            Self::VariableFieldTooLong => "LLUDP variable field exceeds its length prefix",
            Self::InvalidUtf8 => "LLUDP string field contains invalid UTF-8",
            Self::InvalidSchemaMessage => "LLUDP message does not match its schema",
            Self::InboundMessage => "this typed LLUDP message is receive-only",
        })
    }
}

impl std::error::Error for LLUdpError {}
