pub(crate) mod appearance;
pub(crate) mod avatar;
pub(crate) mod components;
pub(crate) mod media;
pub(crate) mod pose;
pub(crate) mod relationships;
pub(crate) mod shape;
pub(crate) mod storage;
pub(crate) mod transform;
pub(crate) mod view;

use std::num::NonZeroU32;

use uuid::Uuid;

use crate::{Entity, LocalAddress};

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
