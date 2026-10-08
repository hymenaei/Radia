use base::UUID;

/// The simulator's response to a viewer logout request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LogoutReply {
    pub agent_data: AgentData,
    /// Inventory item IDs to update, in wire order; the block has a one-byte count.
    pub inventory_data: Vec<InventoryData>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InventoryData {
    /// Inventory item UUID to update; a null UUID is the legacy empty-list sentinel.
    pub item_id: UUID,
}
