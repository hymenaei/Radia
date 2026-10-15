use std::num::{NonZeroU8, NonZeroU32};
use std::sync::atomic::{AtomicU64, Ordering};

use base::UUID;
use math::{DVec3, Quat, Vec3};

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

/// A world-space position and orientation in meters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldPose {
    pub position: DVec3,
    pub rotation: Quat,
}

/// A world-space pose with this object's accepted size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTransform {
    pub pose: WorldPose,
    pub scale: Vec3,
}

/// A resolved point on an avatar skeleton.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AttachmentPoint(NonZeroU8);

impl AttachmentPoint {
    pub const fn new(value: NonZeroU8) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u8 {
        self.0.get()
    }
}

/// Optional sit-position evidence expressed relative to the owning object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SitTarget {
    pub position: Vec3,
    pub rotation: Quat,
}

/// The local frame used to interpret an object's accepted position and rotation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinateFrame {
    Unknown,
    PendingParent { address: LocalAddress, expected_uuid: Option<UUID> },
    RegionRelative { region: Region },
    LinksetRoot { root: Entity },
    AttachmentPoint { avatar: Entity, point: Option<AttachmentPoint> },
}

/// A pose query result, including unresolved and accepted-detach states.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PoseState {
    Resolved(WorldPose),
    Unresolved(UnresolvedReason),
    Provisional { pose: WorldPose, reason: UnresolvedReason },
}

/// The first missing requirement in a pose's dependency chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnresolvedReason {
    PendingParent,
    PoseNotValidForCurrentFrame,
    MissingLocalPose,
    MissingAttachmentJoint,
    MissingRegionOrigin,
}

/// A defensive failure while resolving an internally inconsistent parent chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoseError {
    InvalidRotation { entity: Entity },
    ParentCycle,
    TraversalLimitExceeded { max_edges: usize },
}

/// A full transform query can be unavailable even when its pose is valid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorldTransformState {
    Resolved(WorldTransform),
    Unresolved(PoseState),
    MissingScale(WorldPose),
}

/// A non-nil world identity.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Identity {
    pub(super) id: UUID,
}

impl Identity {
    pub const fn id(self) -> UUID {
        self.id
    }
}

/// The protocol object category. Unknown PCode values remain available to callers.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Kind {
    Avatar,
    Object,
    Unknown(u8),
}

/// The object's shape category, independent from its protocol kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shape {
    Prim,
    Sculpt,
    Mesh,
    Avatar,
}

/// The object's appearance category, independent from its protocol kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Appearance {
    Surface,
    Avatar,
}

/// Position, rotation, and scale can be learned independently.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    pub(super) position: Option<Vec3>,
    pub(super) rotation: Option<Quat>,
    pub(super) scale: Option<Vec3>,
}

impl Transform {
    pub const fn position(self) -> Option<Vec3> {
        self.position
    }

    pub const fn rotation(self) -> Option<Quat> {
        self.rotation
    }

    pub const fn scale(self) -> Option<Vec3> {
        self.scale
    }

    /// Returns a local transform only after all three source-relative values are known.
    pub const fn local_transform(self) -> Option<LocalTransform> {
        match (self.position, self.rotation, self.scale) {
            (Some(position), Some(rotation), Some(scale)) => Some(LocalTransform { position, rotation, scale }),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalTransform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose {
    pub position: Option<Vec3>,
    pub rotation: Option<Quat>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LinearMotion {
    pub velocity: Option<Vec3>,
    pub acceleration: Option<Vec3>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MotionPatch {
    pub velocity: Option<Vec3>,
    pub acceleration: Option<Vec3>,
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
