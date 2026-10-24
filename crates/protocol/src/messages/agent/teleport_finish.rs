use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::TELEPORT_FINISH,
};

#[derive(Clone, Debug, PartialEq)]
pub struct TeleportFinish {
    pub agent_id: Uuid,
    pub location_id: u32,
    pub sim_ip: [u8; 4],
    pub sim_port: [u8; 2],
    pub region_handle: u64,
    pub seed_capability: Vec<u8>,
    pub sim_access: Option<u8>,
    pub teleport_flags: Option<u32>,
}

impl TypedMessage for TeleportFinish {
    const ID: MessageId = TELEPORT_FINISH;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_id: reader.uuid()?,
            location_id: reader.u32()?,
            sim_ip: reader.take(4)?.try_into().map_err(|_| LLUdpError::TruncatedPacket)?,
            sim_port: reader.take(2)?.try_into().map_err(|_| LLUdpError::TruncatedPacket)?,
            region_handle: reader.u64()?,
            seed_capability: reader.variable2()?.to_vec(),
            sim_access: reader.optional_fixed(1, |reader| reader.u8())?,
            teleport_flags: reader.optional_fixed(4, |reader| reader.u32())?,
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_id);
        writer.u32(self.location_id);
        writer.bytes(&self.sim_ip);
        writer.bytes(&self.sim_port);
        writer.u64(self.region_handle);
        writer.variable2(&self.seed_capability)?;
        let Some(sim_access) = self.sim_access else { return Ok(()) };
        writer.u8(sim_access);
        let Some(teleport_flags) = self.teleport_flags else { return Ok(()) };
        writer.u32(teleport_flags);
        Ok(())
    }

    fn retained_capacity_bytes(&self) -> usize {
        crate::lludp::message::allocation::capacity_bytes(self.seed_capability.capacity(), std::mem::size_of::<u8>())
    }
}
