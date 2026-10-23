use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use hecs::{EntityBuilder, EntityRef, Iter, QueryOne, Ref, ViewBorrow};
use uuid::Uuid;

use crate::identity::WorldId;
use crate::mutation::facet::{FacetChanges, FacetSet, FacetUpdates};
use crate::object::components::{Fenced, ObjectProperties};
use crate::{Entity, Kind, LocalAddress, ObjectMedia, Parent, Region};
use crate::{ObjectOrdering, ObjectRecord, ObjectState};

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

    pub(crate) fn insert(&mut self, record: ObjectRecord, facets: FacetSet, ordering: ObjectOrdering) -> Entity {
        let id = record.id;
        let address = record.state.binding.address;
        let kind = record.kind;
        assert!(!self.entities.contains_key(&id), "new object UUID is already live");
        assert!(!self.addresses.contains_key(&address), "new object address is already occupied");
        let mut builder = EntityBuilder::new();
        builder.add(record);
        builder.add(ordering);
        facets.add_components(&mut builder);
        let entity = self.ecs.spawn(builder.build());
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

    pub(crate) fn commit(
        &mut self,
        entity: Entity,
        state: ObjectState,
        facets: Option<FacetChanges>,
        properties: Option<ObjectProperties>,
        ordering: ObjectOrdering,
    ) {
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
        if let Some(facets) = facets {
            facets.apply(&mut self.ecs, entity.0);
        }
        if let Some(properties) = properties {
            if properties.is_empty() {
                if self.ecs.satisfies::<&ObjectProperties>(entity.0) {
                    self.ecs.remove_one::<ObjectProperties>(entity.0).expect("properties component is present");
                }
            } else {
                self.ecs.insert_one(entity.0, properties).expect("properties mutation target is live");
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

    pub(crate) fn object(&self, entity: Entity) -> Option<EntityRef<'_>> {
        if entity.1 != self.world {
            return None;
        }
        self.ecs.entity(entity.0).ok()
    }

    pub(crate) fn facets(&self, entity: hecs::Entity, updates: &FacetUpdates, reset: bool) -> FacetSet {
        FacetSet::read(self.ecs.entity(entity).expect("facet read target is live"), updates, reset)
    }

    pub(crate) fn properties(&self, entity: hecs::Entity) -> ObjectProperties {
        self.ecs
            .entity(entity)
            .expect("properties read target is live")
            .get::<&ObjectProperties>()
            .map(|properties| *properties)
            .unwrap_or_default()
    }

    pub(crate) fn sit_target(&self, entity: hecs::Entity) -> Fenced<crate::SitTarget> {
        self.ecs
            .entity(entity)
            .expect("sit-target read target is live")
            .get::<&Fenced<crate::SitTarget>>()
            .map(|component| (*component).clone())
            .unwrap_or_default()
    }

    pub(crate) fn removal_data(&self, entity: hecs::Entity) -> (Uuid, Kind, ObjectState, ObjectProperties, usize) {
        let entry = self.ecs.entity(entity).expect("removal target is live");
        let mut query = entry.query::<(&ObjectRecord, Option<&ObjectProperties>, Option<&Fenced<Arc<ObjectMedia>>>)>();
        let (record, properties, object_media) = query.get().expect("every object has a record");
        (
            record.id,
            record.kind,
            record.state,
            properties.copied().unwrap_or_default(),
            object_media.and_then(|component| component.value.as_deref()).map_or(0, ObjectMedia::retained_bytes),
        )
    }

    pub(crate) fn objects(&self) -> Iter<'_> {
        self.ecs.iter()
    }

    pub(crate) fn records(&self) -> ViewBorrow<'_, &ObjectRecord> {
        self.ecs.view::<&ObjectRecord>()
    }

    pub(crate) fn ordered_records(&self) -> ViewBorrow<'_, (&ObjectRecord, &ObjectOrdering)> {
        self.ecs.view::<(&ObjectRecord, &ObjectOrdering)>()
    }

    pub(crate) fn read(&self, entity: hecs::Entity) -> QueryOne<'_, (&ObjectRecord, &ObjectOrdering)> {
        self.ecs.query_one::<(&ObjectRecord, &ObjectOrdering)>(entity)
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
        if self
            .ecs
            .query::<(hecs::Entity, &ObjectRecord)>()
            .iter()
            .any(|(entity, _)| self.ecs.entity(entity).is_ok_and(|entry| !FacetSet::components_valid(entry)))
        {
            return Err("empty facet components are stored");
        }
        if self.regions != regions || self.avatars != avatars {
            return Err("region or avatar indexes disagree with stored state");
        }
        Ok(())
    }
}
