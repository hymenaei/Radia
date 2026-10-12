use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
};

use base::UUID;
use hecs::Ref;
use math::{Quat, Vec3};

use crate::{model::*, update::*};

mod journal;
mod pose;
mod reducer;
mod region;
mod relationships;

use journal::{Change, EntityRemovalReason, Journal};
use relationships::Tombstones;
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Binding {
    address: LocalAddress,
    fence: Stamp,
    // Reused local ids need a newer child declaration before linking to this binding.
    parent_resolution_fence: Option<Stamp>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ObservableState {
    address: LocalAddress,
    transform: Transform,
    position_frame_generation: Option<u64>,
    rotation_frame_generation: Option<u64>,
    provisional_pose: Option<WorldPose>,
    parent: Parent,
    linear_motion: LinearMotion,
    angular_velocity: Option<Vec3>,
    shape: Option<Shape>,
    appearance: Option<Appearance>,
    attachment_point: Option<AttachmentPoint>,
    sit_target: Option<SitTarget>,
    properties: Properties,
}

impl Binding {
    fn admits_parent_link(self, expected_uuid: Option<UUID>, declared_at: Option<FieldStamp>, parent_uuid: UUID) -> bool {
        match (expected_uuid, self.parent_resolution_fence) {
            (Some(expected), _) if expected == parent_uuid => true,
            (_, Some(fence)) => declared_at.is_some_and(|stamp| stamp.authority == UpdateAuthority::Tracked && stamp.stamp > fence),
            (_, None) => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Fences {
    pose: Option<FieldStamp>,
    linear_motion: Option<FieldStamp>,
    angular_velocity: Option<FieldStamp>,
    scale: Option<FieldStamp>,
    parent: Option<FieldStamp>,
    attachment_point: Option<FieldStamp>,
    sit_target: Option<RegionalStamp>,
    properties: Option<RegionalStamp>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RegionalStamp {
    region: Region,
    stamp: FieldStamp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParentRejectionBarrier {
    region: Region,
    stamp: Stamp,
}

#[derive(Clone, Copy, Debug)]
struct ObjectData {
    binding: Binding,
    parent: Parent,
    parent_rejection_barrier: Option<ParentRejectionBarrier>,
    linear_motion: LinearMotion,
    angular_velocity: Option<Vec3>,
    shape: Option<Shape>,
    appearance: Option<Appearance>,
    attachment_point: Option<AttachmentPoint>,
    sit_target: Option<SitTarget>,
    frame_generation: u64,
    position_frame_generation: Option<u64>,
    rotation_frame_generation: Option<u64>,
    provisional_pose: Option<WorldPose>,
    properties: Properties,
    fences: Fences,
}

impl ObjectData {
    fn has_current_local_pose(&self, transform: Transform) -> bool {
        transform.position().is_some()
            && transform.rotation().is_some()
            && self.position_frame_generation == Some(self.frame_generation)
            && self.rotation_frame_generation == Some(self.frame_generation)
    }

    fn accepts_observation(&self, observation: &ObjectObservation, stamp: Stamp) -> bool {
        let field_stamp = FieldStamp { authority: observation.source.authority(), stamp };
        self.binding.address.region != observation.source.region
            || stamp > self.binding.fence
            || observation
                .pose
                .is_some_and(|pose| (pose.position.is_some() || pose.rotation.is_some()) && newer_field(field_stamp, self.fences.pose))
            || observation.linear_motion.is_some_and(|motion| {
                (motion.velocity.is_some() || motion.acceleration.is_some()) && newer_field(field_stamp, self.fences.linear_motion)
            })
            || observation.angular_velocity.is_some_and(|_| newer_field(field_stamp, self.fences.angular_velocity))
            || observation.scale.is_some_and(|_| newer_field(field_stamp, self.fences.scale))
            || observation.attachment_point.is_some_and(|_| newer_field(field_stamp, self.fences.attachment_point))
            || newer_field(field_stamp, self.fences.parent)
    }

    fn reset_region_relative(&mut self) {
        self.fences.pose = None;
        self.fences.linear_motion = None;
        self.fences.angular_velocity = None;
        self.fences.scale = None;
        self.fences.parent = None;
        self.fences.attachment_point = None;
        self.fences.sit_target = None;
        self.attachment_point = None;
        self.sit_target = None;
        self.position_frame_generation = None;
        self.rotation_frame_generation = None;
        self.provisional_pose = None;
        self.parent_rejection_barrier = None;
    }
}

fn newer_field(stamp: FieldStamp, fence: Option<FieldStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.authority) {
        (UpdateAuthority::Tracked, UpdateAuthority::Untracked) => true,
        (UpdateAuthority::Untracked, UpdateAuthority::Tracked) => false,
        _ => stamp.stamp > fence.stamp,
    })
}

fn newer_region_field(region: Region, stamp: FieldStamp, fence: Option<RegionalStamp>) -> bool {
    fence.is_none_or(|fence| match (stamp.authority, fence.stamp.authority) {
        (UpdateAuthority::Tracked, UpdateAuthority::Untracked) => true,
        (UpdateAuthority::Untracked, UpdateAuthority::Tracked) => false,
        _ if fence.region != region => true,
        _ => stamp.stamp > fence.stamp.stamp,
    })
}

fn remove_index<K: Copy + Eq + Hash>(map: &mut HashMap<K, HashSet<Entity>>, key: K, entity: Entity) {
    if let Some(entities) = map.get_mut(&key) {
        entities.remove(&entity);
        if entities.is_empty() {
            map.remove(&key);
        }
    }
}

/// A read-only view over one object's live World components.
///
/// The view holds shared component borrows for its lifetime. Drop it before mutating the same World.
#[derive(Clone)]
pub struct Object<'a> {
    entity: Entity,
    identity: Ref<'a, Identity>,
    kind: Ref<'a, Kind>,
    transform: Ref<'a, Transform>,
    data: Ref<'a, ObjectData>,
    world: &'a World,
}

impl Object<'_> {
    pub const fn entity(&self) -> Entity {
        self.entity
    }

    pub fn identity(&self) -> &Identity {
        &self.identity
    }

    pub fn kind(&self) -> Kind {
        *self.kind
    }

    pub fn transform(&self) -> Transform {
        *self.transform
    }

    pub fn position(&self) -> Option<Vec3> {
        self.transform.position()
    }

    pub fn rotation(&self) -> Option<Quat> {
        self.transform.rotation()
    }

    pub fn scale(&self) -> Option<Vec3> {
        self.transform.scale()
    }

    pub fn local_transform(&self) -> Option<LocalTransform> {
        self.transform.local_transform()
    }

    pub fn address(&self) -> LocalAddress {
        self.data.binding.address
    }

    pub fn parent(&self) -> Parent {
        self.data.parent
    }

    pub fn linear_motion(&self) -> LinearMotion {
        self.data.linear_motion
    }

    pub fn angular_velocity(&self) -> Option<Vec3> {
        self.data.angular_velocity
    }

    pub fn shape(&self) -> Option<Shape> {
        self.data.shape
    }

    pub fn appearance(&self) -> Option<Appearance> {
        self.data.appearance
    }

    pub fn properties(&self) -> Properties {
        self.data.properties
    }

    pub fn attachment_point(&self) -> Option<AttachmentPoint> {
        self.data.attachment_point
    }

    pub fn sit_target(&self) -> Option<SitTarget> {
        self.data.sit_target
    }

    pub fn coordinate_frame(&self) -> Result<CoordinateFrame, PoseError> {
        self.world.coordinate_frame(self.entity)
    }

    pub fn world_pose(&self) -> Result<PoseState, PoseError> {
        self.world.resolve_pose(self.entity)
    }

    pub fn world_transform(&self) -> Result<WorldTransformState, PoseError> {
        self.world.resolve_transform(self.entity)
    }
}

/// One authoritative world for a login session.
pub struct World {
    id: WorldId,
    ecs: hecs::World,
    entities: HashMap<UUID, Entity>,
    addresses: HashMap<LocalAddress, Entity>,
    region_entities: HashMap<Region, HashSet<Entity>>,
    regions: HashMap<Region, RegionInfo>,
    retired: HashSet<Region>,
    registration_count: usize,
    self_agent: Option<UUID>,
    children: HashMap<Entity, HashSet<Entity>>,
    pending_children: HashMap<LocalAddress, HashSet<Entity>>,
    tombstones: Tombstones,
    floors: HashMap<Region, Stamp>,
    limits: WorldLimits,
    journal: Journal,
}

impl World {
    pub fn new(limits: WorldLimits) -> Self {
        Self {
            id: WorldId::fresh(),
            ecs: hecs::World::new(),
            entities: HashMap::new(),
            addresses: HashMap::new(),
            region_entities: HashMap::new(),
            regions: HashMap::new(),
            retired: HashSet::new(),
            registration_count: 0,
            self_agent: None,
            children: HashMap::new(),
            pending_children: HashMap::new(),
            tombstones: Tombstones::default(),
            floors: HashMap::new(),
            limits,
            journal: Journal::new(),
        }
    }

    /// Sets the authenticated agent UUID supplied by login/session setup.
    pub fn set_self_agent(&mut self, id: UUID) -> Result<(), ApplyError> {
        if id.is_nil() {
            return Err(ApplyError::InvalidValue { field: "agent_id" });
        }
        if self.self_agent.is_some_and(|current| current != id) {
            return Err(ApplyError::InvalidValue { field: "agent_id" });
        }
        self.self_agent = Some(id);
        Ok(())
    }

    pub const fn revision(&self) -> WorldRevision {
        self.journal.revision
    }

    pub fn entity(&self, id: UUID) -> Option<Entity> {
        self.entities.get(&id).copied()
    }

    pub fn entity_at(&self, address: LocalAddress) -> Option<Entity> {
        self.addresses.get(&address).copied()
    }

    pub fn object(&self, entity: Entity) -> Option<Object<'_>> {
        if entity.1 != self.id {
            return None;
        }
        let identity = self.ecs.get::<&Identity>(entity.0).ok()?;
        let kind = self.ecs.get::<&Kind>(entity.0).ok()?;
        let transform = self.ecs.get::<&Transform>(entity.0).ok()?;
        let data = self.ecs.get::<&ObjectData>(entity.0).ok()?;
        Some(Object { entity, identity, kind, transform, data, world: self })
    }

    pub fn objects(&self) -> impl Iterator<Item = Object<'_>> {
        self.entities.values().filter_map(|entity| self.object(*entity))
    }

    /// Returns the number of live objects in the world.
    pub fn object_count(&self) -> usize {
        self.entities.len()
    }

    pub fn objects_in_region(&self, region: Region) -> impl Iterator<Item = Object<'_>> {
        self.region_entities.get(&region).into_iter().flat_map(|entities| entities.iter()).filter_map(|entity| self.object(*entity))
    }

    pub fn children(&self, entity: Entity) -> impl Iterator<Item = Object<'_>> {
        self.children.get(&entity).into_iter().flat_map(|children| children.iter()).filter_map(|child| self.object(*child))
    }

    pub fn avatars(&self) -> impl Iterator<Item = Object<'_>> {
        self.objects().filter(|object| object.kind() == Kind::Avatar)
    }

    pub fn regions(&self) -> impl Iterator<Item = (Region, &RegionInfo)> {
        self.regions.iter().map(|(region, info)| (*region, info))
    }

    pub fn region(&self, region: Region) -> Option<&RegionInfo> {
        self.regions.get(&region)
    }

    fn require_active_region(&self, region: Region) -> Result<(), ApplyError> {
        if self.regions.contains_key(&region) { Ok(()) } else { Err(ApplyError::StaleRegion) }
    }

    fn observable_state(&self, entity: Entity) -> ObservableState {
        let object = self.object(entity).expect("entity index contains a live object");
        ObservableState {
            address: object.address(),
            transform: object.transform(),
            position_frame_generation: object.data.position_frame_generation,
            rotation_frame_generation: object.data.rotation_frame_generation,
            provisional_pose: object.data.provisional_pose,
            parent: object.parent(),
            linear_motion: object.linear_motion(),
            angular_velocity: object.angular_velocity(),
            shape: object.shape(),
            appearance: object.appearance(),
            attachment_point: object.attachment_point(),
            sit_target: object.sit_target(),
            properties: object.properties(),
        }
    }

    fn transform(&self, entity: Entity) -> hecs::Ref<'_, Transform> {
        self.ecs.get::<&Transform>(entity.0).expect("entity index points to a live transform")
    }

    fn transform_mut(&mut self, entity: Entity) -> hecs::RefMut<'_, Transform> {
        self.ecs.get::<&mut Transform>(entity.0).expect("entity index points to a live transform")
    }

    fn data_mut(&mut self, entity: Entity) -> hecs::RefMut<'_, ObjectData> {
        self.ecs.get::<&mut ObjectData>(entity.0).expect("entity index points to live object data")
    }
}

impl Applied {
    fn unchanged() -> Self {
        Self {
            outcome: ApplyOutcome::Unchanged,
            revision: None,
            recovery: None,
            recovery_cancellations: Vec::new(),
            rejections: Rejections::default(),
        }
    }

    fn deferred(reason: DeferredReason, recovery: Option<Recovery>) -> Self {
        Self {
            outcome: ApplyOutcome::Deferred(reason),
            revision: None,
            recovery,
            recovery_cancellations: Vec::new(),
            rejections: Rejections::default(),
        }
    }
}
