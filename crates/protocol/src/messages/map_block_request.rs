use base::UUID;

/// Requests map data for a range of grid cells.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MapBlockRequest {
    pub agent_data: AgentData,
    pub position_data: PositionData,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
    pub flags: u32,
    pub estate_id: u32,
    pub godlike: bool,
}

/// Requested grid-cell bounds measured in region widths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PositionData {
    pub min_x: u16,
    pub max_x: u16,
    pub min_y: u16,
    pub max_y: u16,
}
