use uuid::Uuid;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegionHandleRequest {
    pub request_block: RequestBlock,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestBlock {
    pub region_id: Uuid,
}
