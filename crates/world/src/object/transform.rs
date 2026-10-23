use std::{
    error::Error,
    fmt::{self, Display, Formatter},
    num::NonZeroU8,
};

use math::{DVec3, Quat, Vec3};
use uuid::Uuid;

use crate::{Entity, LocalAddress, Region};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldPose {
    pub position: DVec3,
    pub rotation: Quat,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldTransform {
    pub pose: WorldPose,
    pub scale: Vec3,
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SitTarget {
    pub position: Vec3,
    pub rotation: Quat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoordinateFrame {
    Unknown,
    PendingParent { address: LocalAddress, expected_uuid: Option<Uuid> },
    RegionRelative { region: Region },
    LinksetRoot { root: Entity },
    AttachmentPoint { avatar: Entity, point: Option<AttachmentPoint> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PoseState {
    Resolved(WorldPose),
    Unresolved(UnresolvedReason),
    Provisional { pose: WorldPose, reason: UnresolvedReason },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnresolvedReason {
    PendingParent,
    PoseNotValidForCurrentFrame,
    MissingLocalPose,
    MissingAttachmentJoint,
    MissingRegionOrigin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoseError {
    InvalidRotation { entity: Entity },
    TraversalLimitExceeded { max_edges: usize },
}

impl Display for PoseError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRotation { entity } => write!(f, "invalid rotation for entity {entity:?}"),
            Self::TraversalLimitExceeded { max_edges } => write!(f, "parent traversal exceeds {max_edges} edges"),
        }
    }
}

impl Error for PoseError {}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WorldTransformState {
    Resolved(WorldTransform),
    MissingScale(WorldPose),
    Provisional { pose: WorldPose, reason: UnresolvedReason },
    Unresolved(UnresolvedReason),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transform {
    pub(crate) position: Option<Vec3>,
    pub(crate) rotation: Option<Quat>,
    pub(crate) scale: Option<Vec3>,
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
