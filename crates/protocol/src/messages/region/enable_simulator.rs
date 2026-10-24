use std::net::Ipv4Addr;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::ENABLE_SIMULATOR,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnableSimulator {
    pub simulator_info: SimulatorInfo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SimulatorInfo {
    pub handle: u64,
    pub ip: Ipv4Addr,
    pub port: u16,
}

impl TypedMessage for EnableSimulator {
    const ID: MessageId = ENABLE_SIMULATOR;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::NoAlloc;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        let handle = reader.u64()?;
        let ip = std::net::Ipv4Addr::from([reader.u8()?, reader.u8()?, reader.u8()?, reader.u8()?]);
        let port = u16::from_be_bytes(reader.take(2)?.try_into().map_err(|_| LLUdpError::TruncatedPacket)?);
        Ok(Self { simulator_info: SimulatorInfo { handle, ip, port } })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.u64(self.simulator_info.handle);
        writer.bytes(&self.simulator_info.ip.octets());
        writer.bytes(&self.simulator_info.port.to_be_bytes());
        Ok(())
    }
}
