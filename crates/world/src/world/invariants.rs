//! Cross-checks authoritative storage and every derived World index.

use super::{ObjectContent, ObjectOrdering, ObjectRecord, World};
use crate::{Entity, Kind, LocalAddress, ObjectMedia, Parent, Region};
use base::UUID;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

impl World {
    /// Checks the World storage and all derived indexes when the `invariants` feature is enabled.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        if self.ecs.query::<(&ObjectRecord, &ObjectContent, &ObjectOrdering)>().iter().count() != self.entities.len()
            || self.ecs.len() as usize != self.entities.len()
            || self.object_ids.len() != self.entities.len()
        {
            return Err("entity storage and UUID indexes have different lengths");
        }

        if self.object_ids.iter().any(|id| !self.entities.contains_key(id)) || self.entities.keys().any(|id| !self.object_ids.contains(id))
        {
            return Err("UUID indexes do not match live ECS entities");
        }

        if self.addresses.len() != self.entities.len() {
            return Err("address index and live entity counts differ");
        }
        let mut ecs_entities = HashSet::with_capacity(self.entities.len());
        let mut expected_by_region = HashMap::<Region, BTreeSet<UUID>>::new();
        let mut expected_children = HashMap::<Entity, BTreeMap<UUID, Entity>>::new();
        let mut expected_pending_children = HashMap::<LocalAddress, HashSet<Entity>>::new();
        let mut expected_avatars = BTreeSet::new();
        let mut object_media_bytes = 0_usize;
        for (id, entity) in &self.entities {
            if !ecs_entities.insert(*entity) {
                return Err("UUID index contains the same ECS entity more than once");
            }
            let object = self.object(*entity).ok_or("UUID index points to an incomplete or dead ECS entity")?;
            let address = object.address();
            if object.kind() == Kind::Avatar {
                expected_avatars.insert(*id);
            }
            if object.record.id != *id || self.addresses.get(&address) != Some(entity) || !self.regions.contains_key(&address.region()) {
                return Err("live entity identity or address index is inconsistent");
            }
            expected_by_region.entry(address.region()).or_default().insert(*id);
            object_media_bytes =
                object_media_bytes.saturating_add(object.content().object_media.as_deref().map_or(0, ObjectMedia::retained_bytes));
            match object.parent() {
                Parent::Resolved(parent) => {
                    let parent_object = self.object(parent).ok_or("resolved parent is not live")?;
                    if parent_object.address().region() != address.region() {
                        return Err("resolved parent and child belong to different regions");
                    }
                    expected_children.entry(parent).or_default().insert(*id, *entity);
                }
                Parent::Pending { address, .. } => {
                    expected_pending_children.entry(address).or_default().insert(*entity);
                }
                Parent::Unknown | Parent::Root => {}
            };
        }
        if self.region_entities != expected_by_region
            || self.children != expected_children
            || self.pending_children != expected_pending_children
        {
            return Err("region or parent relationship indexes are inconsistent");
        }
        if self.avatars != expected_avatars {
            return Err("avatar index does not match live avatar records");
        }
        let mut checked = HashSet::with_capacity(self.entities.len());
        let mut path = HashSet::new();
        for entity in self.entities.values().copied() {
            path.clear();
            let mut current = entity;
            while !checked.contains(&current) {
                if !path.insert(current) {
                    return Err("resolved parent graph contains a cycle");
                }
                let object = self.object(current).ok_or("resolved parent is not live")?;
                let Parent::Resolved(parent) = object.parent() else { break };
                current = parent;
            }
            checked.extend(path.iter().copied());
        }
        if self.regions.keys().any(|region| self.retired.contains(region))
            || self.floors.keys().any(|region| !self.regions.contains_key(region))
        {
            return Err("region lifecycle indexes are inconsistent");
        }
        if self.object_media_bytes != object_media_bytes {
            return Err("ObjectMedia byte accounting is inconsistent");
        }
        if !self.tombstones.invariants_hold(&self.regions, &self.addresses) {
            return Err("tombstone indexes are inconsistent");
        }
        if let Some(error) = self.spatial.invariant_error(self) {
            return Err(error);
        }
        if !self.journal.invariants_hold() {
            return Err("change journal accounting is inconsistent");
        }
        if !self.recent.invariants_hold() || self.recent.iter_after(None).any(|(id, _)| self.entities.contains_key(&id)) {
            return Err("recent-object indexes overlap live entities or are inconsistent");
        }
        Ok(())
    }
}
