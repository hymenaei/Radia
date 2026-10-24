mod history;
mod identity;
#[cfg(test)]
mod invariants;
mod mutation;
mod object;
mod read;
mod region;
mod update;

use std::collections::{BTreeMap, HashMap, HashSet};

use uuid::Uuid;

use crate::history::Journal;
use crate::history::address::Tombstones;
use crate::identity::WorldId;
use crate::object::components::{
    Binding, Fences, ObjectOrdering, ObjectRecord, ObjectState, ParentRejectionBarrier, RegionalStamp, newer_field, newer_region_field,
};
use crate::object::storage::ObjectStore;
use crate::object::view::IndexedObjects;
use crate::region::RegionState;
use crate::update::Stamp;

pub use history::{WorldChange, WorldChangeClasses, WorldEntityRemovalReason};
pub use identity::{Entity, LocalAddress, ObjectBindingTarget, Region, RegionHandle, RegionInfo};
pub use object::appearance::{
    Appearance, FaceAppearance, MAX_PBR_MATERIAL_BINDINGS, MAX_SURFACE_FACES, PbrAlphaMode, PbrMaterialAssignments,
    PbrMaterialAssignmentsError, PbrMaterialOverride, PbrMaterialOverrides, PbrMaterialOverridesError, PbrTextureOverride,
    PbrTextureTransformOverride, SurfaceAppearance, SurfaceAppearanceError, TextureMapping,
};
pub use object::avatar::{
    AvatarAppearance, AvatarAppearanceError, AvatarAppearanceUpdate, AvatarBakeSlot, AvatarTextureSlot, AvatarTextureSlots,
    AvatarVisualParamIndex, MAX_AVATAR_TEXTURE_SLOTS, MAX_AVATAR_VISUAL_PARAMS,
};
pub use object::media::{
    MAX_MEDIA_DIMENSION, MAX_MEDIA_URL_BYTES, MAX_MEDIA_WHITELIST_BYTES, MAX_MEDIA_WHITELIST_ENTRIES, MediaControls, MediaEntry,
    ObjectMedia, ObjectMediaError, ObjectMediaUpdate,
};
pub use object::shape::{PrimShape, SculptFlags, SculptType, Shape};
pub use object::transform::{
    AttachmentPoint, CoordinateFrame, LinearMotion, LocalTransform, Pose, PoseError, PoseState, SitTarget, Transform, UnresolvedReason,
    WorldPose, WorldTransform, WorldTransformState,
};
pub use object::view::{Object, Objects};
pub use object::{Kind, Parent, ParentUpdate};
pub use read::{
    MAX_OBJECT_DETAIL_BATCH_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_DETAIL_BYTES, MAX_OBJECT_MEDIA_READ_BYTES,
    MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_BYTES, MAX_REGION_PAGE_ITEMS, MAX_WORLD_CHANGE_PAGE_ITEMS, ObjectDetail, ObjectDetailBatch,
    ObjectDetailResult, ObjectListItem, ObjectListPage, ObjectMediaRead, ObjectPageCursor, ObjectSummary, RegionPage, RegionPageCursor,
    RegionSummary, WorldChangeCursor, WorldChangePage, WorldChangeRecord, WorldReadBarrier, WorldReadError,
};
pub use region::environment::{
    EnvironmentAssignment, EnvironmentRead, EnvironmentScope, EnvironmentSource, EnvironmentTarget, MAX_ENVIRONMENT_BYTES,
    MAX_ENVIRONMENT_READ_BYTES, MAX_ENVIRONMENT_SOURCE_BYTES, ParcelEnvironment, ParcelLocalId,
};
pub use region::terrain::{
    MAX_TERRAIN_PAGE_PATCHES, MAX_TERRAIN_PATCHES, TERRAIN_PATCH_SAMPLES, TERRAIN_PATCH_SIZE, TERRAIN_REGION_SIZE, TerrainPage,
    TerrainPageCursor, TerrainPatch, TerrainPatchError, TerrainPatchId,
};
pub use update::{
    Applied, ApplyError, ApplyOutcome, DeferredReason, FieldGroup, FieldRejection, FieldRejectionReason, FieldUpdate, ObjectObservation,
    ObjectPatch, OrderEvidence, Recovery, RegionApplied, RegionOutcome, Rejections, UpdateAuthority, UpdateSource, WorldLimits,
    WorldRevision,
};

const MAX_OBJECT_MEDIA_BYTES: usize = 64 * 1024 * 1024;

pub struct World {
    id: WorldId,
    store: ObjectStore,
    regions: BTreeMap<Region, RegionState>,
    retired: HashSet<Region>,
    self_agent: Option<Uuid>,
    children: HashMap<Entity, BTreeMap<Uuid, Entity>>,
    pending_children: HashMap<LocalAddress, HashSet<Entity>>,
    tombstones: Tombstones,
    floors: HashMap<Region, Stamp>,
    object_media_bytes: usize,
    terrain_patches: usize,
    environment_bytes: usize,
    limits: WorldLimits,
    journal: Journal,
}

impl World {
    pub fn new(limits: WorldLimits) -> Self {
        let id = WorldId::fresh();
        Self {
            id,
            store: ObjectStore::new(id),
            regions: BTreeMap::new(),
            retired: HashSet::new(),
            self_agent: None,
            children: HashMap::new(),
            pending_children: HashMap::new(),
            tombstones: Tombstones::default(),
            floors: HashMap::new(),
            object_media_bytes: 0,
            terrain_patches: 0,
            environment_bytes: 0,
            limits,
            journal: Journal::new(),
        }
    }

    pub fn set_self_agent(&mut self, id: Uuid) -> Result<(), ApplyError> {
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

    pub fn entity(&self, id: Uuid) -> Option<Entity> {
        self.store.entity(id)
    }

    pub fn entity_at(&self, address: LocalAddress) -> Option<Entity> {
        self.store.entity_at(address)
    }

    pub fn object_binding_target(&self, entity: Entity) -> Option<ObjectBindingTarget> {
        let object = self.store.object_record(entity)?;
        Some(ObjectBindingTarget {
            entity,
            id: object.id,
            address: object.state.binding.address,
            address_generation: object.state.binding.address_generation,
        })
    }

    pub fn object_binding_target_is_current(&self, target: ObjectBindingTarget) -> bool {
        if target.entity.1 != self.id || self.entity(target.id) != Some(target.entity) {
            return false;
        }
        let Some(object) = self.store.object_record(target.entity) else { return false };
        object.state.binding.address == target.address
            && object.state.binding.address_generation == target.address_generation
            && self.entity_at(target.address) == Some(target.entity)
    }

    pub fn object(&self, entity: Entity) -> Option<Object<'_>> {
        let record = self.store.object_record(entity)?;
        Some(Object::new(entity, record, self))
    }

    pub fn objects(&self) -> Objects<'_> {
        Objects { records: self.store.objects(), world: self }
    }

    pub fn object_count(&self) -> usize {
        self.store.len()
    }

    pub fn objects_in_region(&self, region: Region) -> impl Iterator<Item = Object<'_>> {
        self.store.region_ids(region).into_iter().flat_map(|ids| IndexedObjects::new(ids.iter(), self))
    }

    pub fn children(&self, entity: Entity) -> impl Iterator<Item = Object<'_>> {
        self.children.get(&entity).into_iter().flat_map(|children| children.values()).filter_map(|child| self.object(*child))
    }

    pub fn avatars(&self) -> impl Iterator<Item = Object<'_>> {
        IndexedObjects::new(self.store.avatar_ids().iter(), self)
    }

    pub fn regions(&self) -> impl Iterator<Item = (Region, &RegionInfo)> {
        self.regions.iter().map(|(region, info)| (*region, &info.info))
    }

    pub fn region(&self, region: Region) -> Option<&RegionInfo> {
        self.regions.get(&region).map(|state| &state.info)
    }

    fn require_active_region(&self, region: Region) -> Result<(), ApplyError> {
        if self.regions.contains_key(&region) { Ok(()) } else { Err(ApplyError::StaleRegion) }
    }

    #[cfg(test)]
    fn state_mut(&mut self, entity: Entity) -> &mut ObjectState {
        &mut self.store.record_mut(entity.0).state
    }
}
