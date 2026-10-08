use base::UUID;

use crate::ChatType;

/// A viewer-to-simulator nearby chat message.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatFromViewer {
    pub agent_data: AgentData,
    pub chat_data: ChatData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentData {
    pub agent_id: UUID,
    pub session_id: UUID,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatData {
    /// UTF-8 chat text, encoded as a NUL-terminated Variable 2 field.
    pub message: String,
    pub chat_type: ChatType,
    pub channel: i32,
}
