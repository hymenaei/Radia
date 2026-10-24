use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct CompleteAgentMovement {
    pub agent_data: AgentData,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub circuit_code: u32,
}
