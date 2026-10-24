use math::Vec3;
use uuid::Uuid;

use crate::{
    LLUdpError,
    lludp::{
        codec::{Reader, Writer},
        message::{AllocationStrategy, TypedMessage},
        types::MessageId,
    },
    schema::ids::AGENT_MOVEMENT_COMPLETE,
};

#[derive(Clone, Debug, PartialEq)]
pub struct AgentMovementComplete {
    pub agent_data: AgentData,
    pub data: Data,
    pub sim_data: SimData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Data {
    pub position: Vec3,
    pub look_at: Vec3,
    pub region_handle: u64,
    pub timestamp: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SimData {
    pub channel_version: String,
}

impl TypedMessage for AgentMovementComplete {
    const ID: MessageId = AGENT_MOVEMENT_COMPLETE;
    const ALLOCATION: AllocationStrategy = AllocationStrategy::SchemaDerived;
    const INBOUND_ONLY: bool = true;

    fn decode(reader: &mut Reader<'_>) -> Result<Self, LLUdpError> {
        Ok(Self {
            agent_data: AgentData { agent_id: reader.uuid()?, session_id: reader.uuid()? },
            data: Data { position: reader.vector3()?, look_at: reader.vector3()?, region_handle: reader.u64()?, timestamp: reader.u32()? },
            sim_data: SimData { channel_version: reader.variable2_string()? },
        })
    }

    fn encode(&self, writer: &mut Writer<'_>) -> Result<(), LLUdpError> {
        writer.uuid(self.agent_data.agent_id);
        writer.uuid(self.agent_data.session_id);
        writer.vector3(self.data.position);
        writer.vector3(self.data.look_at);
        writer.u64(self.data.region_handle);
        writer.u32(self.data.timestamp);
        writer.variable2_string(&self.sim_data.channel_version)
    }

    fn retained_capacity_bytes(&self) -> usize {
        let mut bytes = 0;
        crate::lludp::message::allocation::add_string_capacity(&mut bytes, &self.sim_data.channel_version);
        bytes
    }
}
