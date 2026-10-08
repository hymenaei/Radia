use base::UUID;

use crate::{Audible, ChatSourceType, ChatType, Vector3};

/// Nearby chat and source metadata received from a simulator.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatFromSimulator {
    pub chat_data: ChatData,
}

/// Chat content, source identity, audibility, and position.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatData {
    pub from_name: String,
    pub source_id: UUID,
    pub owner_id: UUID,
    pub source_type: ChatSourceType,
    pub chat_type: ChatType,
    pub audible: Audible,
    /// Region-local source position.
    pub position: Vector3,
    /// UTF-8 chat text, encoded as a NUL-terminated Variable 2 field.
    pub message: String,
}
