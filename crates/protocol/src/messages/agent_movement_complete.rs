use base::UUID;

use crate::Vector3;

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
    /// Agent position in region-local coordinates.
    pub position: Vector3,
    /// Region-local look-at vector.
    pub look_at: Vector3,
    /// Global handle of the region containing the agent.
    pub region_handle: u64,
    pub timestamp: u32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SimData {
    /// Simulator channel and version string, encoded as a NUL-terminated Variable 2 field.
    pub channel_version: String,
}
