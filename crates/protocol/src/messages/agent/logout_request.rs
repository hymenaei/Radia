use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct LogoutRequest {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}
