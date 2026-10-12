use base::UUID;

/// The simulator's response to a viewer logout request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogoutReply {
    pub agent_data: AgentData,
    pub inventory_data: Vec<InventoryData>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InventoryData {
    pub item_id: UUID,
}
