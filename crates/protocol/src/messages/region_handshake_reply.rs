use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct RegionHandshakeReply {
    pub agent_data: AgentData,
    pub region_info: RegionInfo,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RegionInfo {
    pub flags: u32,
}
