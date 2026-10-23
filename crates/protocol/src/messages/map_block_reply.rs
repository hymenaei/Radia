use uuid::Uuid;

#[derive(Clone, Debug, PartialEq)]
pub struct MapBlockReply {
    pub agent_data: AgentData,
    pub data: Vec<Data>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub flags: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Data {
    pub x: u16,
    pub y: u16,
    pub name: String,
    pub access: u8,
    pub region_flags: u32,
    pub water_height: u8,
    pub agents: u8,
    pub map_image_id: Uuid,
}
