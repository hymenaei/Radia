use base::UUID;

use math::Vec3;

/// The simulator's completed movement update for this agent.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentMovementComplete {
    pub agent_data: AgentData,
    pub data: Data,
    pub sim_data: SimData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
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
