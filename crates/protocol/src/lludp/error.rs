use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LLUdpError {
    TruncatedPacket,
    InvalidMessageId,
    InvalidMessage,
    InvalidZeroCode,
    InvalidAcknowledgementList,
    InvalidPacketFlags,
    DatagramTooLarge,
    DecodedPacketTooLarge,
    AllocationBoundOverflow,
    ExtraHeaderTooLong,
    TooManyAcks,
    VariableFieldTooLong,
    InvalidUtf8,
    InvalidSchemaMessage,
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
