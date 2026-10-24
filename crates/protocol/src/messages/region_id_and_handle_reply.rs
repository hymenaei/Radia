use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegionIdAndHandleReply {
    pub reply_block: ReplyBlock,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReplyBlock {
    pub region_id: Uuid,
    pub region_handle: u64,
}
