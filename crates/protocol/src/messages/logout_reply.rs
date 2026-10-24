use uuid::Uuid;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogoutReply {
    pub agent_data: AgentData,
    pub inventory_data: Vec<InventoryData>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InventoryData {
    pub item_id: Uuid,
}
