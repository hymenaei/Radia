//! Authoritative World ownership, coherent mutations, and bounded read models.

use std::collections::{BTreeMap, HashMap, HashSet};

use base::UUID;

use crate::identity::WorldId;
use crate::update::{FieldStamp, Stamp};
use crate::{ApplyError, Entity, LocalAddress, ObjectBindingTarget, Region, RegionInfo, WorldLimits, WorldRevision};

mod address;
mod components;
#[cfg(feature = "invariants")]
mod invariants;
mod journal;
mod mutation;
mod pose;
mod query;
mod read;
mod recent;
mod relationships;
mod spatial;
mod storage;
mod view;

use address::Tombstones;
use components::{
    Binding, Fences, ObjectContent, ObjectOrdering, ObjectRecord, ObjectState, ParentRejectionBarrier, RegionalStamp, newer_field,
    newer_region_field,
};
use journal::Journal;
pub use journal::{WorldChange, WorldChangeClasses, WorldEntityRemovalReason};
pub use query::{
    MAX_WORLD_QUERY_RESULTS, MAX_WORLD_QUERY_WORK, QueryCompleteness, QueryIncompleteReason, QueryScope, QueryUnresolved, ValueFilter,
    WorldQuery, WorldQueryBudget, WorldQueryBuildError, WorldQueryCursor, WorldQueryItem, WorldQueryPage,
};
pub use read::{
    MAX_OBJECT_DETAIL_BATCH_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_DETAIL_BYTES, MAX_OBJECT_MEDIA_READ_BYTES,
    MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_BYTES, MAX_REGION_PAGE_ITEMS, MAX_WORLD_CHANGE_PAGE_ITEMS, ObjectDetail, ObjectDetailBatch,
    ObjectDetailResult, ObjectListItem, ObjectListItemRead, ObjectListPage, ObjectMediaRead, ObjectPage, ObjectPageCursor, ObjectSummary,
    ObjectSummaryRead, RegionPage, RegionPageCursor, RegionSummary, RegionSummaryRead, WorldChangeCursor, WorldChangePage,
    WorldChangeRecord, WorldReadBarrier, WorldReadError,
};
pub use recent::{MAX_RECENT_OBJECT_BYTES, MAX_RECENT_OBJECTS, RECENT_OBJECT_TTL, RecentObject};
use recent::{RecentBatch, RecentStore};
use spatial::SpatialIndex;
pub use spatial::{MAX_QUERY_STATE_MAINTENANCE_WORK, QueryStateMaintenance};
use storage::{ObjectRecordRef, ObjectStore};
pub use view::{Object, Objects};

#[derive(Default)]
struct RegionState {
    info: RegionInfo,
    name_fence: Option<FieldStamp>,
}

const MAX_OBJECT_MEDIA_BYTES: usize = 64 * 1024 * 1024;

/// One authoritative world for a login session.
pub struct World {
    id: WorldId,
    store: ObjectStore,
    regions: BTreeMap<Region, RegionState>,
    retired: HashSet<Region>,
    self_agent: Option<UUID>,
    children: HashMap<Entity, BTreeMap<UUID, Entity>>,
    pending_children: HashMap<LocalAddress, HashSet<Entity>>,
    tombstones: Tombstones,
    floors: HashMap<Region, Stamp>,
    object_media_bytes: usize,
    limits: WorldLimits,
    journal: Journal,
    spatial: SpatialIndex,
    recent: RecentStore,
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
            limits,
            journal: Journal::new(),
            spatial: SpatialIndex::default(),
            recent: RecentStore::default(),
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

    /// The latest committed observable change. Updates that only advance ordering evidence do not increment the revision.
    pub const fn revision(&self) -> WorldRevision {
        self.journal.revision
    }

    pub fn entity(&self, id: UUID) -> Option<Entity> {
        self.store.entity(id)
    }

    pub fn entity_at(&self, address: LocalAddress) -> Option<Entity> {
        self.store.entity_at(address)
    }

    /// Captures the current object binding for an asynchronous operation.
    pub fn object_binding_target(&self, entity: Entity) -> Option<ObjectBindingTarget> {
        let object = self.store.record(entity)?;
        Some(ObjectBindingTarget {
            entity,
            id: object.id,
            address: object.state.binding.address,
            address_generation: object.state.binding.address_generation,
        })
    }

    /// Returns whether a target still names the same live object binding.
    pub fn object_binding_target_is_current(&self, target: ObjectBindingTarget) -> bool {
        if target.entity.1 != self.id || self.entity(target.id) != Some(target.entity) {
            return false;
        }
        let Some(object) = self.store.record(target.entity) else { return false };
        object.state.binding.address == target.address
            && object.state.binding.address_generation == target.address_generation
            && self.entity_at(target.address) == Some(target.entity)
    }

    pub fn object(&self, entity: Entity) -> Option<Object<'_>> {
        let record = self.store.object(entity)?;
        Some(Object { entity, record, world: self })
    }

    /// Borrows live object state for scans in unspecified storage order.
    pub fn objects(&self) -> Objects<'_> {
        Objects { records: self.store.objects(), world: self }
    }

    /// Returns the number of live objects in the world.
    pub fn object_count(&self) -> usize {
        self.store.len()
    }

    pub fn objects_in_region(&self, region: Region) -> impl Iterator<Item = Object<'_>> {
        self.store.region_ids(region).into_iter().flat_map(|ids| ids.iter()).map(|id| {
            let entity = self.entity(*id).expect("region index contains only live objects");
            self.object(entity).expect("region index contains only complete objects")
        })
    }

    pub fn children(&self, entity: Entity) -> impl Iterator<Item = Object<'_>> {
        self.children.get(&entity).into_iter().flat_map(|children| children.values()).filter_map(|child| self.object(*child))
    }

    /// Iterates live avatars in unspecified order without scanning other objects.
    pub fn avatars(&self) -> impl Iterator<Item = Object<'_>> {
        self.store.avatar_ids().iter().map(|id| {
            let entity = self.entity(*id).expect("avatar index contains only live entities");
            self.object(entity).expect("avatar index contains only complete objects")
        })
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
