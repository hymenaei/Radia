use std::{
    collections::{BTreeSet, HashMap},
    ops::Deref,
};

use hecs::{QueryBorrow, QueryOne, Ref, ViewBorrow};
use uuid::Uuid;

use crate::identity::WorldId;
use crate::{Entity, Kind, LocalAddress, Parent, Region};
use crate::{ObjectContent, ObjectOrdering, ObjectRecord, ObjectState};

#[derive(Clone)]
pub(crate) enum ObjectRecordRef<'a> {
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
    pub(crate) fn content(&self) -> &ObjectContent {
        match self {
            Self::Owned { content, .. } => content.as_deref().unwrap_or(&ObjectContent::EMPTY),
            Self::Borrowed { content, .. } => content.unwrap_or(&ObjectContent::EMPTY),
        }
    }
}

pub(crate) struct ObjectStore {
    world: WorldId,
    ecs: hecs::World,
    entities: HashMap<Uuid, Entity>,
    ids: BTreeSet<Uuid>,
    addresses: HashMap<LocalAddress, Entity>,
    regions: HashMap<Region, BTreeSet<Uuid>>,
    avatars: BTreeSet<Uuid>,
}

impl ObjectStore {
    pub(crate) fn new(world: WorldId) -> Self {
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

    pub(crate) fn entity(&self, id: Uuid) -> Option<Entity> {
        self.entities.get(&id).copied()
    }

    pub(crate) fn entity_at(&self, address: LocalAddress) -> Option<Entity> {
        self.addresses.get(&address).copied()
    }

    pub(crate) fn len(&self) -> usize {
        self.entities.len()
    }

    pub(crate) fn ids(&self) -> &BTreeSet<Uuid> {
        &self.ids
    }

    pub(crate) fn avatar_ids(&self) -> &BTreeSet<Uuid> {
        &self.avatars
    }

    pub(crate) fn region_ids(&self, region: Region) -> Option<&BTreeSet<Uuid>> {
        self.regions.get(&region)
    }

    #[cfg(feature = "invariants")]
    pub(crate) fn entities(&self) -> impl Iterator<Item = (Uuid, Entity)> + '_ {
        self.entities.iter().map(|(id, entity)| (*id, *entity))
    }

    pub(crate) fn insert(&mut self, record: ObjectRecord, content: ObjectContent, ordering: ObjectOrdering) -> Entity {
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

    pub(crate) fn remove(&mut self, entity: Entity) {
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

    pub(crate) fn commit(&mut self, entity: Entity, state: ObjectState, content: Option<ObjectContent>, ordering: ObjectOrdering) {
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
            record.state = ObjectState { parent: record.state.parent, ..state };
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

    fn remove_region_id(&mut self, region: Region, id: Uuid) {
        let ids = self.regions.get_mut(&region).expect("live object has a region index");
        assert!(ids.remove(&id), "region index is missing a live object");
        if ids.is_empty() {
            self.regions.remove(&region);
        }
    }

    pub(crate) fn record(&self, entity: Entity) -> Option<Ref<'_, ObjectRecord>> {
        if entity.1 != self.world {
            return None;
        }
        self.ecs.get::<&ObjectRecord>(entity.0).ok()
    }

    pub(crate) fn object(&self, entity: Entity) -> Option<ObjectRecordRef<'_>> {
        if entity.1 != self.world {
            return None;
        }
        let entry = self.ecs.entity(entity.0).ok()?;
        let record = entry.get::<&ObjectRecord>().expect("every object has a record");
        let content = entry.get::<&ObjectContent>();
        Some(ObjectRecordRef::Owned { record, content })
    }

    pub(crate) fn objects(&self) -> QueryBorrow<'_, (hecs::Entity, &ObjectRecord, Option<&ObjectContent>)> {
        self.ecs.query::<(hecs::Entity, &ObjectRecord, Option<&ObjectContent>)>()
    }

    pub(crate) fn object_view(&self) -> ViewBorrow<'_, (&ObjectRecord, Option<&ObjectContent>)> {
        self.ecs.view::<(&ObjectRecord, Option<&ObjectContent>)>()
    }

    pub(crate) fn records(&self) -> ViewBorrow<'_, &ObjectRecord> {
        self.ecs.view::<&ObjectRecord>()
    }

    pub(crate) fn ordered_records(&self) -> ViewBorrow<'_, (&ObjectRecord, &ObjectOrdering)> {
        self.ecs.view::<(&ObjectRecord, &ObjectOrdering)>()
    }

    pub(crate) fn read(&self, entity: hecs::Entity) -> QueryOne<'_, (&ObjectRecord, Option<&ObjectContent>, &ObjectOrdering)> {
        self.ecs.query_one::<(&ObjectRecord, Option<&ObjectContent>, &ObjectOrdering)>(entity)
    }

    pub(crate) fn read_ordered(&self, entity: hecs::Entity) -> QueryOne<'_, (&ObjectRecord, &ObjectOrdering)> {
        self.ecs.query_one::<(&ObjectRecord, &ObjectOrdering)>(entity)
    }

    pub(crate) fn ordering(&self, entity: hecs::Entity) -> Ref<'_, ObjectOrdering> {
        self.ecs.get::<&ObjectOrdering>(entity).expect("every object has ordering")
    }

    pub(crate) fn set_parent(&mut self, entity: Entity, parent: Parent) {
        assert_eq!(entity.1, self.world, "parent mutation belongs to another World");
        self.ecs.query_one_mut::<&mut ObjectRecord>(entity.0).expect("parent mutation target is live").state.parent = parent;
    }

    #[cfg(test)]
    pub(crate) fn record_mut(&mut self, entity: hecs::Entity) -> &mut ObjectRecord {
        self.ecs.query_one_mut::<&mut ObjectRecord>(entity).expect("test mutation target is live")
    }

    pub(crate) fn ordering_mut(&mut self, entity: hecs::Entity) -> &mut ObjectOrdering {
        self.ecs.query_one_mut::<&mut ObjectOrdering>(entity).expect("mutation target has ordering")
    }

    #[cfg(feature = "invariants")]
    pub(crate) fn check_invariants(&self) -> Result<(), &'static str> {
        let count = self.entities.len();
        if self.ecs.len() as usize != count
            || self.ecs.query::<(&ObjectRecord, &ObjectOrdering)>().iter().count() != count
            || self.ids.len() != count
            || self.addresses.len() != count
        {
            return Err("object storage and identity index counts differ");
        }
        let mut regions = HashMap::<Region, BTreeSet<Uuid>>::new();
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
