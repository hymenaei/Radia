use base::UUID;

use crate::{Audible, ChatSourceType, ChatType};
use math::Vec3;

#[derive(Clone, Debug, PartialEq)]
pub struct ChatFromSimulator {
    pub chat_data: ChatData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatData {
    pub from_name: String,
    pub source_id: UUID,
    pub owner_id: UUID,
    pub source_type: ChatSourceType,
    pub chat_type: ChatType,
    pub audible: Audible,
    pub position: Vec3,
    pub message: String,
}
