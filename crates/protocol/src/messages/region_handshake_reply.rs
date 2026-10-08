use base::UUID;

/// Confirms the viewer's region initialization to the simulator.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionHandshakeReply {
    pub agent_data: AgentData,
    pub region_info: RegionInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionInfo {
    pub flags: u32,
}
