use std::sync::Arc;

use math::Vec3;
use uuid::Uuid;

use crate::update::{FieldStamp, Stamp};
use crate::{
    Appearance, AttachmentPoint, AvatarAppearance, GltfMaterialOverrides, Kind, LinearMotion, LocalAddress, ObjectMedia, ObjectObservation,
    Parent, Properties, PropertiesProvenance, PropertyProvenance, Region, RenderMaterials, Shape, SitTarget, SurfaceAppearance, Transform,
    UpdateAuthority, UpdateSource, WorldPose,
};

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
    pub(crate) shape: Option<RegionalStamp>,
    pub(crate) appearance: Option<RegionalStamp>,
    pub(crate) texture_entry: Option<RegionalStamp>,
    pub(crate) render_materials: Option<RegionalStamp>,
    pub(crate) gltf_material_overrides: Option<RegionalStamp>,
    pub(crate) avatar_appearance: Option<RegionalStamp>,
    pub(crate) sit_target: Option<RegionalStamp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegionalStamp {
    pub(crate) region: Region,
    pub(crate) stamp: FieldStamp,
}

impl From<PropertyProvenance> for RegionalStamp {
    fn from(provenance: PropertyProvenance) -> Self {
        Self {
            region: provenance.region,
            stamp: FieldStamp {
                authority: provenance.authority,
                stamp: Stamp { order: provenance.order, block_ordinal: provenance.block_ordinal },
            },
        }
    }
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

#[derive(Clone, Copy, Debug)]
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
    pub(crate) fn has_current_local_pose(&self) -> bool {
        self.transform.position().is_some()
            && self.transform.rotation().is_some()
            && self.position_frame_generation == Some(self.frame_generation)
            && self.rotation_frame_generation == Some(self.frame_generation)
    }

    pub(crate) fn accepts_observation(&self, ordering: &ObjectOrdering, observation: &ObjectObservation, stamp: Stamp) -> bool {
        let field_stamp = FieldStamp { authority: observation.source.authority(), stamp };
        self.binding.address.region != observation.source.region
            || stamp > self.binding.fence
            || observation
                .pose
                .is_some_and(|pose| (pose.position.is_some() || pose.rotation.is_some()) && newer_field(field_stamp, ordering.fences.pose))
            || observation.linear_motion.is_some_and(|motion| {
                (motion.velocity.is_some() || motion.acceleration.is_some()) && newer_field(field_stamp, ordering.fences.linear_motion)
            })
            || observation.angular_velocity.is_some_and(|_| newer_field(field_stamp, ordering.fences.angular_velocity))
            || observation.scale.is_some_and(|_| newer_field(field_stamp, ordering.fences.scale))
            || (observation.attachment_point.is_present() && newer_field(field_stamp, ordering.fences.attachment_point))
            || (observation.shape.is_present() && newer_region_field(observation.source.region, field_stamp, ordering.fences.shape))
            || (observation.appearance.is_present()
                && newer_region_field(observation.source.region, field_stamp, ordering.fences.appearance))
            || (observation.kind != Kind::Avatar
                && observation.surface_appearance.is_some()
                && newer_region_field(observation.source.region, field_stamp, ordering.fences.texture_entry))
            || (observation.kind != Kind::Avatar
                && observation.render_materials.is_some()
                && newer_region_field(observation.source.region, field_stamp, ordering.fences.render_materials))
            || newer_field(field_stamp, ordering.fences.parent)
    }

    pub(crate) fn reset_region_relative(&mut self) {
        self.attachment_point = None;
        self.position_frame_generation = None;
        self.rotation_frame_generation = None;
        self.provisional_pose = None;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ObjectContent {
    pub(crate) shape: Option<Shape>,
    pub(crate) appearance: Option<Appearance>,
    pub(crate) surface_appearance: Option<Arc<SurfaceAppearance>>,
    pub(crate) render_materials: Option<Arc<RenderMaterials>>,
    pub(crate) gltf_material_overrides: Option<Arc<GltfMaterialOverrides>>,
    pub(crate) object_media: Option<Arc<ObjectMedia>>,
    pub(crate) avatar_appearance: Option<Arc<AvatarAppearance>>,
    pub(crate) sit_target: Option<SitTarget>,
    pub(crate) properties: Properties,
    pub(crate) properties_provenance: PropertiesProvenance,
}

impl ObjectContent {
    pub(crate) const EMPTY: Self = Self {
        shape: None,
        appearance: None,
        surface_appearance: None,
        render_materials: None,
        gltf_material_overrides: None,
        object_media: None,
        avatar_appearance: None,
        sit_target: None,
        properties: Properties { owner_id: None, creator_id: None },
        properties_provenance: PropertiesProvenance { owner: None, creator: None },
    };

    pub(crate) fn is_empty(&self) -> bool {
        self == &Self::EMPTY
    }
}

impl Default for ObjectContent {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ObjectOrdering {
    pub(crate) fences: Fences,
    pub(crate) parent_rejection_barrier: Option<ParentRejectionBarrier>,
}

impl ObjectOrdering {
    pub(crate) fn reset_region_relative(&mut self) {
        self.fences.pose = None;
        self.fences.frame = None;
        self.fences.linear_motion = None;
        self.fences.angular_velocity = None;
        self.fences.scale = None;
        self.fences.parent = None;
        self.fences.attachment_point = None;
        self.fences.sit_target = None;
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
