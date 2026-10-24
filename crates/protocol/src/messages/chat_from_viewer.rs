use uuid::Uuid;

use crate::ChatType;

#[derive(Clone, Debug, PartialEq)]
pub struct ChatFromViewer {
    pub agent_data: AgentData,
    pub chat_data: ChatData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AgentData {
    pub agent_id: Uuid,
    pub session_id: Uuid,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatData {
    pub message: String,
    pub chat_type: ChatType,
    pub channel: i32,
}
