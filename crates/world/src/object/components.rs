use math::Vec3;
use uuid::Uuid;

use crate::update::{FieldStamp, Stamp};
use crate::{AttachmentPoint, Kind, LinearMotion, LocalAddress, Parent, Region, Transform, UpdateAuthority, UpdateSource, WorldPose};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Binding {
    pub(crate) address: LocalAddress,
    pub(crate) address_generation: u64,
    fence: Stamp,
    pub(crate) parent_resolution_fence: Option<Stamp>,
}

impl Binding {
    pub(crate) fn new(address: LocalAddress, address_generation: u64, fence: Stamp, parent_resolution_fence: Option<Stamp>) -> Self {
        Self { address, address_generation, fence, parent_resolution_fence }
    }

    pub(crate) fn fence(self) -> Stamp {
        self.fence
    }

    pub(crate) fn advance_fence(&mut self, source: UpdateSource, stamp: Stamp) {
        if source.authority() == UpdateAuthority::Tracked {
            self.fence = self.fence.max(stamp);
        }
    }

    pub(crate) fn admits_parent_link(self, expected_uuid: Option<Uuid>, declared_at: Option<FieldStamp>, parent_uuid: Uuid) -> bool {
        match (expected_uuid, self.parent_resolution_fence) {
            (Some(expected), _) if expected == parent_uuid => true,
            (_, Some(fence)) => declared_at.is_some_and(|stamp| stamp.authority == UpdateAuthority::Tracked && stamp.stamp > fence),
            (_, None) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Fences {
    pub(crate) pose: Option<FieldStamp>,
    pub(crate) frame: Option<FieldStamp>,
    pub(crate) linear_motion: Option<FieldStamp>,
    pub(crate) angular_velocity: Option<FieldStamp>,
    pub(crate) scale: Option<FieldStamp>,
    pub(crate) parent: Option<FieldStamp>,
    pub(crate) attachment_point: Option<FieldStamp>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Fenced<T> {
    pub(crate) value: Option<T>,
    pub(crate) fence: Option<RegionalStamp>,
}

impl<T> Fenced<T> {
    pub(crate) fn is_stored(&self) -> bool {
        self.value.is_some() || self.fence.is_some()
    }
}

impl<T> Default for Fenced<T> {
    fn default() -> Self {
        Self { value: None, fence: None }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegionalStamp {
    pub(crate) region: Region,
    pub(crate) stamp: FieldStamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ParentRejectionBarrier {
    pub(crate) region: Region,
    pub(crate) stamp: Stamp,
}

impl ParentRejectionBarrier {
    pub(crate) fn admits(self, source: UpdateSource, stamp: Stamp) -> bool {
        self.region != source.region || (source.authority() == UpdateAuthority::Tracked && stamp > self.stamp)
    }
}

#[derive(Debug)]
pub(crate) struct ObjectRecord {
    pub(crate) id: Uuid,
    pub(crate) kind: Kind,
    pub(crate) state: ObjectState,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ObjectState {
    pub(crate) transform: Transform,
    pub(crate) binding: Binding,
    pub(crate) parent: Parent,
    pub(crate) linear_motion: LinearMotion,
    pub(crate) angular_velocity: Option<Vec3>,
    pub(crate) attachment_point: Option<AttachmentPoint>,
    pub(crate) frame_generation: u64,
    pub(crate) position_frame_generation: Option<u64>,
    pub(crate) rotation_frame_generation: Option<u64>,
    pub(crate) provisional_pose: Option<WorldPose>,
}

impl ObjectState {
    pub(crate) fn new(binding: Binding) -> Self {
        Self {
            transform: Transform::default(),
            binding,
            parent: Parent::Unknown,
            linear_motion: LinearMotion::default(),
            angular_velocity: None,
            attachment_point: None,
            frame_generation: 0,
            position_frame_generation: None,
            rotation_frame_generation: None,
            provisional_pose: None,
        }
    }

    pub(crate) fn has_current_local_pose(&self) -> bool {
        self.transform.position().is_some()
            && self.transform.rotation().is_some()
            && self.position_frame_generation == Some(self.frame_generation)
            && self.rotation_frame_generation == Some(self.frame_generation)
    }

    pub(crate) fn reset_region_relative(&mut self) {
        self.transform = Transform { scale: self.transform.scale, ..Transform::default() };
        self.parent = Parent::Unknown;
        self.linear_motion = LinearMotion::default();
        self.angular_velocity = None;
        self.attachment_point = None;
        self.position_frame_generation = None;
        self.rotation_frame_generation = None;
        self.provisional_pose = None;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ObjectOrdering {
    pub(crate) fences: Fences,
    pub(crate) parent_rejection_barrier: Option<ParentRejectionBarrier>,
}

impl ObjectOrdering {
    pub(crate) fn protects_parent(&self) -> bool {
        self.fences.parent.is_some_and(|fence| fence.authority == UpdateAuthority::Tracked) || self.parent_rejection_barrier.is_some()
    }

    pub(crate) fn admits_parent(&self, source: UpdateSource, stamp: Stamp) -> bool {
        self.parent_rejection_barrier.is_none_or(|barrier| barrier.admits(source, stamp))
    }

    pub(crate) fn reset_region_relative(&mut self) {
        self.fences.pose = None;
        self.fences.frame = None;
        self.fences.linear_motion = None;
        self.fences.angular_velocity = None;
        self.fences.scale = None;
        self.fences.parent = None;
        self.fences.attachment_point = None;
        self.parent_rejection_barrier = None;
    }
}

pub(crate) fn newer_field(stamp: FieldStamp, fence: Option<FieldStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.authority) {
        (UpdateAuthority::Tracked, UpdateAuthority::Untracked) => true,
        (UpdateAuthority::Untracked, UpdateAuthority::Tracked) => false,
        _ => stamp.stamp > fence.stamp,
    })
}

pub(crate) fn newer_region_field(region: Region, stamp: FieldStamp, fence: Option<RegionalStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.stamp.authority) {
        (UpdateAuthority::Tracked, UpdateAuthority::Untracked) => true,
        (UpdateAuthority::Untracked, UpdateAuthority::Tracked) => false,
        _ if fence.region != region => true,
        _ => stamp.stamp > fence.stamp.stamp,
    })
}
