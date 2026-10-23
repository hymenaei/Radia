use crate::{Entity, LocalAddress, Region, UpdateAuthority};
use std::num::NonZeroU32;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PropertyProvenance {
    pub region: Region,
    pub authority: UpdateAuthority,
    pub order: u64,
    pub block_ordinal: u32,
    pub source_sequence: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertiesProvenance {
    pub owner: Option<PropertyProvenance>,
    pub creator: Option<PropertyProvenance>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Kind {
    Avatar,
    Object,
    Unknown(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParentUpdate {
    Root,
    Local(NonZeroU32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Parent {
    Unknown,
    Root,
    Pending { address: LocalAddress, expected_uuid: Option<Uuid> },
    Resolved(Entity),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Properties {
    pub owner_id: Option<Uuid>,
    pub creator_id: Option<Uuid>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertiesPatch {
    pub owner_id: Option<Uuid>,
    pub creator_id: Option<Uuid>,
}
