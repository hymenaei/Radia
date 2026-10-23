use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapBlockRequest {
    pub agent_data: AgentData,
    pub position_data: PositionData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
    pub flags: u32,
    pub estate_id: u32,
    pub godlike: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionData {
    pub min_x: u16,
    pub max_x: u16,
    pub min_y: u16,
    pub max_y: u16,
}
