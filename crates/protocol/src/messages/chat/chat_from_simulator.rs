use uuid::Uuid;

use crate::{Audible, ChatSourceType, ChatType};
use math::Vec3;

#[derive(Clone, Debug, PartialEq)]
pub struct ChatFromSimulator {
    pub chat_data: ChatData,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatData {
    pub from_name: String,
    pub source_id: Uuid,
    pub owner_id: Uuid,
    pub source_type: ChatSourceType,
    pub chat_type: ChatType,
    pub audible: Audible,
    pub position: Vec3,
    pub message: String,
}
