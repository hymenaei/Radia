//! World-scoped identities, simulator addresses, and region metadata.

use base::UUID;
use math::DVec3;
use std::{
    num::NonZeroU32,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_WORLD_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Entity(pub(super) hecs::Entity, pub(super) WorldId);

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct WorldId(u64);

impl WorldId {
    pub(super) fn fresh() -> Self {
        let id = NEXT_WORLD_ID
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| current.checked_add(1))
            .unwrap_or_else(|_| panic!("World identity space exhausted"));
        Self(id)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Region(u64);

impl Region {
    /// Wraps the network connection identifier used for this region lifetime.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A simulator-local object address scoped to one connection lifetime.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct LocalAddress {
    pub(super) region: Region,
    pub(super) local_id: NonZeroU32,
}

/// Identifies one object binding for asynchronous operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectBindingTarget {
    pub(super) entity: Entity,
    pub(super) id: UUID,
    pub(super) address: LocalAddress,
    pub(super) address_generation: u64,
}

impl ObjectBindingTarget {
    pub const fn entity(self) -> Entity {
        self.entity
    }

    pub const fn object_id(self) -> UUID {
        self.id
    }

    pub const fn address(self) -> LocalAddress {
        self.address
    }
}

impl LocalAddress {
    pub const fn new(region: Region, local_id: NonZeroU32) -> Self {
        Self { region, local_id }
    }

    pub const fn region(self) -> Region {
        self.region
    }

    pub const fn local_id(self) -> NonZeroU32 {
        self.local_id
    }
}

/// A packed Second Life region handle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegionHandle(u64);

impl RegionHandle {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }

    pub const fn origin(self) -> DVec3 {
        DVec3::new((self.0 >> 32) as u32 as f64, self.0 as u32 as f64, 0.0)
    }
}

/// Optional, non-authoritative metadata for a registered region.
///
/// When passed to [`crate::World::patch_region`], `None` leaves the stored value unchanged. Supplied values cannot conflict with metadata already
/// stored for the region.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RegionInfo {
    /// Second Life region UUID, when known.
    pub region_id: Option<UUID>,
    /// Global region handle, when known.
    pub handle: Option<RegionHandle>,
    /// Region name, when known.
    pub name: Option<String>,
    /// Region origin in global meters, when known.
    pub origin: Option<DVec3>,
}

/// An asset identity, distinct from the identity of an object using that asset.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AssetId(UUID);

impl AssetId {
    pub const fn new(uuid: UUID) -> Self {
        Self(uuid)
    }

    pub const fn uuid(self) -> UUID {
        self.0
    }
}
