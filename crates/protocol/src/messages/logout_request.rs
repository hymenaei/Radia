use base::UUID;

/// A viewer request to end its simulator session.
#[derive(Clone, Debug, PartialEq)]
pub struct LogoutRequest {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
}
