use base::UUID;

/// Returns a region UUID and its global handle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RegionIDAndHandleReply {
    pub reply_block: ReplyBlock,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReplyBlock {
    pub region_id: UUID,
    pub region_handle: u64,
}
