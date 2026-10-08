use base::UUID;

/// Notifies neighboring simulators that a viewer enabled this simulator.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfirmEnableSimulator {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
}
