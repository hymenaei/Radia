//! Complete object lifetimes, identity indexes, and read-only ECS access.
//!
//! Component mutation requires an exclusive store borrow. Shared access exposes only immutable
//! queries, so readers cannot create conflicts with other readers through hecs's runtime borrows.
//! Every entity has an identity/spatial record and ordering metadata. Content is present only
//! while it retains data; its absence does not discard field fences. Regions and other World
//! resources are not entities in this store.

use std::{
    collections::{BTreeSet, HashMap},
    ops::Deref,
};

use base::UUID;
use hecs::{QueryBorrow, QueryOne, Ref, ViewBorrow};

use crate::identity::WorldId;
use crate::{Entity, Kind, LocalAddress, Parent, Region};

use super::{ObjectContent, ObjectOrdering, ObjectRecord, ObjectState};

#[derive(Clone)]
pub(super) enum ObjectRecordRef<'a> {
    Owned { record: Ref<'a, ObjectRecord>, content: Option<Ref<'a, ObjectContent>> },
    Borrowed { record: &'a ObjectRecord, content: Option<&'a ObjectContent> },
}

impl Deref for ObjectRecordRef<'_> {
    type Target = ObjectRecord;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Owned { record, .. } => record,
            Self::Borrowed { record, .. } => record,
        }
    }
}

impl ObjectRecordRef<'_> {
    pub(super) fn content(&self) -> &ObjectContent {
        match self {
            Self::Owned { content, .. } => content.as_deref().unwrap_or(&ObjectContent::EMPTY),
            Self::Borrowed { content, .. } => content.unwrap_or(&ObjectContent::EMPTY),
        }
    }
}

pub(super) struct ObjectStore {
    world: WorldId,
    ecs: hecs::World,
    entities: HashMap<UUID, Entity>,
    ids: BTreeSet<UUID>,
    addresses: HashMap<LocalAddress, Entity>,
    regions: HashMap<Region, BTreeSet<UUID>>,
    avatars: BTreeSet<UUID>,
}

impl ObjectStore {
    pub(super) fn new(world: WorldId) -> Self {
        Self {
            world,
            ecs: hecs::World::new(),
            entities: HashMap::new(),
            ids: BTreeSet::new(),
            addresses: HashMap::new(),
            regions: HashMap::new(),
            avatars: BTreeSet::new(),
        }
    }

    pub(super) fn entity(&self, id: UUID) -> Option<Entity> {
        self.entities.get(&id).copied()
    }

    pub(super) fn entity_at(&self, address: LocalAddress) -> Option<Entity> {
        self.addresses.get(&address).copied()
    }

    pub(super) fn len(&self) -> usize {
        self.entities.len()
    }

    pub(super) fn ids(&self) -> &BTreeSet<UUID> {
        &self.ids
    }

    pub(super) fn avatar_ids(&self) -> &BTreeSet<UUID> {
        &self.avatars
    }

    pub(super) fn region_ids(&self, region: Region) -> Option<&BTreeSet<UUID>> {
        self.regions.get(&region)
    }

    #[cfg(feature = "invariants")]
    pub(super) fn entities(&self) -> impl Iterator<Item = (UUID, Entity)> + '_ {
        self.entities.iter().map(|(id, entity)| (*id, *entity))
    }

    pub(super) fn insert(&mut self, record: ObjectRecord, content: ObjectContent, ordering: ObjectOrdering) -> Entity {
        let id = record.id;
        let address = record.state.binding.address;
        let kind = record.kind;
        assert!(!self.entities.contains_key(&id), "new object UUID is already live");
        assert!(!self.addresses.contains_key(&address), "new object address is already occupied");
        let entity = if content.is_empty() { self.ecs.spawn((record, ordering)) } else { self.ecs.spawn((record, content, ordering)) };
        let entity = Entity(entity, self.world);
        self.entities.insert(id, entity);
        self.ids.insert(id);
        self.addresses.insert(address, entity);
        self.regions.entry(address.region()).or_default().insert(id);
        if kind == Kind::Avatar {
            self.avatars.insert(id);
        }
        entity
    }

    pub(super) fn remove(&mut self, entity: Entity) {
        let (id, address) = {
            let record = self.record(entity).expect("removal target is live");
            (record.id, record.state.binding.address)
        };
        self.ecs.despawn(entity.0).expect("removal target is live");
        self.entities.remove(&id);
        self.ids.remove(&id);
        self.avatars.remove(&id);
        self.addresses.remove(&address);
        self.remove_region_id(address.region(), id);
    }

    /// `None` preserves content; an empty value removes the optional component.
    pub(super) fn commit(&mut self, entity: Entity, state: ObjectState, content: Option<ObjectContent>, ordering: ObjectOrdering) {
        let (id, previous) = {
            let record = self.record(entity).expect("mutation target is live");
            (record.id, record.state.binding.address)
        };
        let address = state.binding.address;
        if previous != address {
            assert!(self.entity_at(address).is_none(), "new object address is already occupied");
            self.addresses.remove(&previous);
            self.addresses.insert(address, entity);
            if previous.region() != address.region() {
                self.remove_region_id(previous.region(), id);
                self.regions.entry(address.region()).or_default().insert(id);
            }
        }
        {
            let (record, stored_ordering) = self
                .ecs
                .query_one_mut::<(&mut ObjectRecord, &mut ObjectOrdering)>(entity.0)
                .expect("mutation target has its required components");
            record.state = state;
            *stored_ordering = ordering;
        }
        if let Some(content) = content {
            if content.is_empty() {
                if self.ecs.satisfies::<&ObjectContent>(entity.0) {
                    self.ecs.remove_one::<ObjectContent>(entity.0).expect("content component is present");
                }
            } else {
                self.ecs.insert_one(entity.0, content).expect("content mutation target is live");
            }
        }
    }

    fn remove_region_id(&mut self, region: Region, id: UUID) {
        let ids = self.regions.get_mut(&region).expect("live object has a region index");
        assert!(ids.remove(&id), "region index is missing a live object");
        if ids.is_empty() {
            self.regions.remove(&region);
        }
    }

    pub(super) fn record(&self, entity: Entity) -> Option<Ref<'_, ObjectRecord>> {
        if entity.1 != self.world {
            return None;
        }
        self.ecs.get::<&ObjectRecord>(entity.0).ok()
    }

    pub(super) fn object(&self, entity: Entity) -> Option<ObjectRecordRef<'_>> {
        if entity.1 != self.world {
            return None;
        }
        let entry = self.ecs.entity(entity.0).ok()?;
        let record = entry.get::<&ObjectRecord>().expect("every object has a record");
        let content = entry.get::<&ObjectContent>();
        Some(ObjectRecordRef::Owned { record, content })
    }

    pub(super) fn objects(&self) -> QueryBorrow<'_, (hecs::Entity, &ObjectRecord, Option<&ObjectContent>)> {
        self.ecs.query::<(hecs::Entity, &ObjectRecord, Option<&ObjectContent>)>()
    }

    pub(super) fn object_view(&self) -> ViewBorrow<'_, (&ObjectRecord, Option<&ObjectContent>)> {
        self.ecs.view::<(&ObjectRecord, Option<&ObjectContent>)>()
    }

    pub(super) fn records(&self) -> ViewBorrow<'_, &ObjectRecord> {
        self.ecs.view::<&ObjectRecord>()
    }

    pub(super) fn ordered_records(&self) -> ViewBorrow<'_, (&ObjectRecord, &ObjectOrdering)> {
        self.ecs.view::<(&ObjectRecord, &ObjectOrdering)>()
    }

    pub(super) fn read(&self, entity: hecs::Entity) -> QueryOne<'_, (&ObjectRecord, Option<&ObjectContent>, &ObjectOrdering)> {
        self.ecs.query_one::<(&ObjectRecord, Option<&ObjectContent>, &ObjectOrdering)>(entity)
    }

    pub(super) fn read_ordered(&self, entity: hecs::Entity) -> QueryOne<'_, (&ObjectRecord, &ObjectOrdering)> {
        self.ecs.query_one::<(&ObjectRecord, &ObjectOrdering)>(entity)
    }

    pub(super) fn ordering(&self, entity: hecs::Entity) -> Ref<'_, ObjectOrdering> {
        self.ecs.get::<&ObjectOrdering>(entity).expect("every object has ordering")
    }

    pub(super) fn set_parent(&mut self, entity: Entity, parent: Parent) {
        assert_eq!(entity.1, self.world, "parent mutation belongs to another World");
        self.ecs.query_one_mut::<&mut ObjectRecord>(entity.0).expect("parent mutation target is live").state.parent = parent;
    }

    #[cfg(test)]
    pub(super) fn record_mut(&mut self, entity: hecs::Entity) -> &mut ObjectRecord {
        self.ecs.query_one_mut::<&mut ObjectRecord>(entity).expect("test mutation target is live")
    }

    pub(super) fn ordering_mut(&mut self, entity: hecs::Entity) -> &mut ObjectOrdering {
        self.ecs.query_one_mut::<&mut ObjectOrdering>(entity).expect("mutation target has ordering")
    }

    #[cfg(feature = "invariants")]
    pub(super) fn check_invariants(&self) -> Result<(), &'static str> {
        let count = self.entities.len();
        if self.ecs.len() as usize != count
            || self.ecs.query::<(&ObjectRecord, &ObjectOrdering)>().iter().count() != count
            || self.ids.len() != count
            || self.addresses.len() != count
        {
            return Err("object storage and identity index counts differ");
        }
        let mut regions = HashMap::<Region, BTreeSet<UUID>>::new();
        let mut avatars = BTreeSet::new();
        for (id, entity) in self.entities() {
            let record = self.record(entity).ok_or("UUID index points to a dead or foreign entity")?;
            if id.is_nil() || record.id != id || !self.ids.contains(&id) || self.entity_at(record.state.binding.address) != Some(entity) {
                return Err("object identity indexes disagree with stored state");
            }
            regions.entry(record.state.binding.address.region()).or_default().insert(id);
            if record.kind == Kind::Avatar {
                avatars.insert(id);
            }
        }
        if self.regions != regions || self.avatars != avatars {
            return Err("region or avatar indexes disagree with stored state");
        }
        Ok(())
    }
}
