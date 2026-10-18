//! Object classification, relationships, ownership, and their observed provenance.

use crate::{Entity, LocalAddress, Region, UpdateAuthority};
use base::UUID;
use std::num::NonZeroU32;

/// The accepted source that supplied one owner or creator value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PropertyProvenance {
    pub region: Region,
    pub authority: UpdateAuthority,
    pub order: u64,
    pub block_ordinal: u32,
    pub source_sequence: u32,
}

/// Provenance for independently refreshed owner and creator values.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertiesProvenance {
    pub owner: Option<PropertyProvenance>,
    pub creator: Option<PropertyProvenance>,
}

/// The protocol object category. Unknown PCode values remain available to callers.
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
    Pending { address: LocalAddress, expected_uuid: Option<UUID> },
    Resolved(Entity),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Properties {
    pub owner_id: Option<UUID>,
    pub creator_id: Option<UUID>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertiesPatch {
    pub owner_id: Option<UUID>,
    pub creator_id: Option<UUID>,
}
