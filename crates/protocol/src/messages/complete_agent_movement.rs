use base::UUID;

/// Tells the simulator that the viewer is ready to begin agent movement.
#[derive(Clone, Debug, PartialEq)]
pub struct CompleteAgentMovement {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
    pub circuit_code: u32,
}
