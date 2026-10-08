use base::UUID;

/// Requests a simulator's region handle for a region ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionHandleRequest {
    pub request_block: RequestBlock,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestBlock {
    pub region_id: UUID,
}
