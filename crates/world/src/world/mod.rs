//! Authoritative World ownership, coherent mutations, and bounded read models.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use base::UUID;
use hecs::Ref;

use crate::identity::WorldId;
use crate::update::Stamp;
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
    MAX_OBJECT_DETAIL_BATCH_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_DETAIL_BYTES, MAX_OBJECT_PAGE_ITEMS, MAX_REGION_PAGE_BYTES,
    MAX_REGION_PAGE_ITEMS, MAX_WORLD_CHANGE_PAGE_ITEMS, ObjectDetail, ObjectDetailBatch, ObjectDetailResult, ObjectListItem,
    ObjectListItemRead, ObjectListPage, ObjectPage, ObjectPageCursor, ObjectSummary, ObjectSummaryRead, RegionPage, RegionPageCursor,
    RegionSummary, RegionSummaryRead, WorldChangeCursor, WorldChangePage, WorldChangeRecord, WorldReadBarrier, WorldReadError,
};
pub use recent::{MAX_RECENT_OBJECT_BYTES, MAX_RECENT_OBJECTS, RECENT_OBJECT_TTL, RecentObject};
use recent::{RecentBatch, RecentStore};
use spatial::SpatialIndex;
pub use spatial::{MAX_QUERY_STATE_MAINTENANCE_WORK, QueryStateMaintenance};
use view::ObjectRecordRef;
pub use view::{Object, Objects};

const MAX_OBJECT_MEDIA_BYTES: usize = 64 * 1024 * 1024;
pub(super) fn remove_region_index(map: &mut HashMap<Region, BTreeSet<UUID>>, region: Region, id: UUID) {
    if let Some(ids) = map.get_mut(&region) {
        ids.remove(&id);
        if ids.is_empty() {
            map.remove(&region);
        }
    }
}

/// One authoritative world for a login session.
pub struct World {
    id: WorldId,
    ecs: hecs::World,
    entities: HashMap<UUID, Entity>,
    avatars: BTreeSet<UUID>,
    object_ids: BTreeSet<UUID>,
    addresses: HashMap<LocalAddress, Entity>,
    region_entities: HashMap<Region, BTreeSet<UUID>>,
    regions: BTreeMap<Region, RegionInfo>,
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
        Self {
            id: WorldId::fresh(),
            ecs: hecs::World::new(),
            entities: HashMap::new(),
            avatars: BTreeSet::new(),
            object_ids: BTreeSet::new(),
            addresses: HashMap::new(),
            region_entities: HashMap::new(),
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

    pub const fn revision(&self) -> WorldRevision {
        self.journal.revision
    }

    pub fn entity(&self, id: UUID) -> Option<Entity> {
        self.entities.get(&id).copied()
    }

    pub fn entity_at(&self, address: LocalAddress) -> Option<Entity> {
        self.addresses.get(&address).copied()
    }

    /// Captures the current object binding for an asynchronous operation.
    pub fn object_binding_target(&self, entity: Entity) -> Option<ObjectBindingTarget> {
        let object = self.record(entity)?;
        Some(ObjectBindingTarget {
            entity,
            id: object.id,
            address: object.state.binding.address,
            address_generation: object.state.binding.address_generation,
        })
    }

    /// Returns whether a target still names the same live object binding.
    pub fn object_binding_target_is_current(&self, target: ObjectBindingTarget) -> bool {
        if target.entity.1 != self.id || self.entities.get(&target.id) != Some(&target.entity) {
            return false;
        }
        let Some(object) = self.record(target.entity) else { return false };
        object.state.binding.address == target.address
            && object.state.binding.address_generation == target.address_generation
            && self.addresses.get(&target.address) == Some(&target.entity)
    }

    fn record(&self, entity: Entity) -> Option<Ref<'_, ObjectRecord>> {
        if entity.1 != self.id {
            return None;
        }
        self.ecs.get::<&ObjectRecord>(entity.0).ok()
    }

    pub fn object(&self, entity: Entity) -> Option<Object<'_>> {
        if entity.1 != self.id {
            return None;
        }
        let entry = self.ecs.entity(entity.0).ok()?;
        let record = entry.get::<&ObjectRecord>()?;
        let content = entry.get::<&ObjectContent>()?;
        Some(Object { entity, record: ObjectRecordRef::Owned { record, content }, world: self })
    }

    /// Borrows live object state for scans in unspecified storage order.
    pub fn objects(&self) -> Objects<'_> {
        Objects { records: self.ecs.query::<(hecs::Entity, &ObjectRecord, &ObjectContent)>(), world: self }
    }

    /// Returns the number of live objects in the world.
    pub fn object_count(&self) -> usize {
        self.entities.len()
    }

    pub fn objects_in_region(&self, region: Region) -> impl Iterator<Item = Object<'_>> {
        self.region_entities
            .get(&region)
            .into_iter()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| self.entities.get(id).copied())
            .filter_map(|entity| self.object(entity))
    }

    pub fn children(&self, entity: Entity) -> impl Iterator<Item = Object<'_>> {
        self.children.get(&entity).into_iter().flat_map(|children| children.values()).filter_map(|child| self.object(*child))
    }

    /// Iterates live avatars in unspecified order without scanning other objects.
    pub fn avatars(&self) -> impl Iterator<Item = Object<'_>> {
        self.avatars.iter().map(|id| self.object(self.entities[id]).expect("avatar index contains only live entities"))
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

    fn state_mut(&mut self, entity: Entity) -> &mut ObjectState {
        &mut self.ecs.query_one_mut::<&mut ObjectRecord>(entity.0).expect("entity index points to live state").state
    }

    fn ordering_mut(&mut self, entity: Entity) -> &mut ObjectOrdering {
        self.ecs.query_one_mut::<&mut ObjectOrdering>(entity.0).expect("entity index points to live ordering")
    }
}
