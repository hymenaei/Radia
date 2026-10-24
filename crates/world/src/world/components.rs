use crate::update::{FieldStamp, Stamp};
use crate::{
    Appearance, AttachmentPoint, AvatarAppearance, GltfMaterialOverrides, Kind, LinearMotion, LocalAddress, ObjectMedia, ObjectObservation,
    Parent, Properties, PropertiesProvenance, PropertyProvenance, Region, RenderMaterials, Shape, SitTarget, SurfaceAppearance, Transform,
    UpdateAuthority, UpdateSource, WorldPose,
};
use math::Vec3;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct Binding {
    pub(super) address: LocalAddress,
    pub(super) address_generation: u64,
    fence: Stamp,
    pub(super) parent_resolution_fence: Option<Stamp>,
}

impl Binding {
    pub(super) fn new(address: LocalAddress, address_generation: u64, fence: Stamp, parent_resolution_fence: Option<Stamp>) -> Self {
        Self { address, address_generation, fence, parent_resolution_fence }
    }

    pub(super) fn fence(self) -> Stamp {
        self.fence
    }

    pub(super) fn advance_fence(&mut self, source: UpdateSource, stamp: Stamp) {
        if source.authority() == UpdateAuthority::Tracked {
            self.fence = self.fence.max(stamp);
        }
    }

    pub(super) fn admits_parent_link(self, expected_uuid: Option<Uuid>, declared_at: Option<FieldStamp>, parent_uuid: Uuid) -> bool {
        match (expected_uuid, self.parent_resolution_fence) {
            (Some(expected), _) if expected == parent_uuid => true,
            (_, Some(fence)) => declared_at.is_some_and(|stamp| stamp.authority == UpdateAuthority::Tracked && stamp.stamp > fence),
            (_, None) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Fences {
    pub(super) pose: Option<FieldStamp>,
    pub(super) frame: Option<FieldStamp>,
    pub(super) linear_motion: Option<FieldStamp>,
    pub(super) angular_velocity: Option<FieldStamp>,
    pub(super) scale: Option<FieldStamp>,
    pub(super) parent: Option<FieldStamp>,
    pub(super) attachment_point: Option<FieldStamp>,
    pub(super) shape: Option<RegionalStamp>,
    pub(super) appearance: Option<RegionalStamp>,
    pub(super) texture_entry: Option<RegionalStamp>,
    pub(super) render_materials: Option<RegionalStamp>,
    pub(super) gltf_material_overrides: Option<RegionalStamp>,
    pub(super) avatar_appearance: Option<RegionalStamp>,
    pub(super) sit_target: Option<RegionalStamp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RegionalStamp {
    pub(super) region: Region,
    pub(super) stamp: FieldStamp,
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
pub(super) struct ParentRejectionBarrier {
    pub(super) region: Region,
    pub(super) stamp: Stamp,
}

impl ParentRejectionBarrier {
    pub(super) fn admits(self, source: UpdateSource, stamp: Stamp) -> bool {
        self.region != source.region || (source.authority() == UpdateAuthority::Tracked && stamp > self.stamp)
    }
}

#[derive(Debug)]
pub(super) struct ObjectRecord {
    pub(super) id: Uuid,
    pub(super) kind: Kind,
    pub(super) state: ObjectState,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ObjectState {
    pub(super) transform: Transform,
    pub(super) binding: Binding,
    pub(super) parent: Parent,
    pub(super) linear_motion: LinearMotion,
    pub(super) angular_velocity: Option<Vec3>,
    pub(super) attachment_point: Option<AttachmentPoint>,
    pub(super) frame_generation: u64,
    pub(super) position_frame_generation: Option<u64>,
    pub(super) rotation_frame_generation: Option<u64>,
    pub(super) provisional_pose: Option<WorldPose>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ObjectContent {
    pub(super) shape: Option<Shape>,
    pub(super) appearance: Option<Appearance>,
    pub(super) surface_appearance: Option<Arc<SurfaceAppearance>>,
    pub(super) render_materials: Option<Arc<RenderMaterials>>,
    pub(super) gltf_material_overrides: Option<Arc<GltfMaterialOverrides>>,
    pub(super) object_media: Option<Arc<ObjectMedia>>,
    pub(super) avatar_appearance: Option<Arc<AvatarAppearance>>,
    pub(super) sit_target: Option<SitTarget>,
    pub(super) properties: Properties,
    pub(super) properties_provenance: PropertiesProvenance,
}

impl ObjectContent {
    pub(super) const EMPTY: Self = Self {
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

    pub(super) fn is_empty(&self) -> bool {
        self == &Self::EMPTY
    }
}

impl Default for ObjectContent {
    fn default() -> Self {
        Self::EMPTY
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct ObjectOrdering {
    pub(super) fences: Fences,
    pub(super) parent_rejection_barrier: Option<ParentRejectionBarrier>,
}

impl ObjectOrdering {
    pub(super) fn reset_region_relative(&mut self) {
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

impl ObjectState {
    pub(super) fn has_current_local_pose(&self) -> bool {
        self.transform.position().is_some()
            && self.transform.rotation().is_some()
            && self.position_frame_generation == Some(self.frame_generation)
            && self.rotation_frame_generation == Some(self.frame_generation)
    }

    pub(super) fn accepts_observation(&self, ordering: &ObjectOrdering, observation: &ObjectObservation, stamp: Stamp) -> bool {
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

    pub(super) fn reset_region_relative(&mut self) {
        self.attachment_point = None;
        self.position_frame_generation = None;
        self.rotation_frame_generation = None;
        self.provisional_pose = None;
    }
}

pub(super) fn newer_field(stamp: FieldStamp, fence: Option<FieldStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.authority) {
        (UpdateAuthority::Tracked, UpdateAuthority::Untracked) => true,
        (UpdateAuthority::Untracked, UpdateAuthority::Tracked) => false,
        _ => stamp.stamp > fence.stamp,
    })
}

pub(super) fn newer_region_field(region: Region, stamp: FieldStamp, fence: Option<RegionalStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.stamp.authority) {
        (UpdateAuthority::Tracked, UpdateAuthority::Untracked) => true,
        (UpdateAuthority::Untracked, UpdateAuthority::Tracked) => false,
        _ if fence.region != region => true,
        _ => stamp.stamp > fence.stamp.stamp,
    })
}
