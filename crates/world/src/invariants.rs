use std::collections::{BTreeMap, HashMap, HashSet};

use uuid::Uuid;

use crate::{Entity, LocalAddress, ObjectMedia, Parent};
use crate::{MAX_ENVIRONMENT_BYTES, MAX_PARCEL_BYTES, MAX_TERRAIN_PATCHES, World};

#[cfg(test)]
impl World {
    pub(crate) fn check_invariants(&self) -> Result<(), &'static str> {
        let environment_bytes: usize = self.regions.values().map(|state| state.environment.source_bytes()).sum();
        if environment_bytes != self.environment_bytes || environment_bytes > MAX_ENVIRONMENT_BYTES {
            return Err("environment source accounting is inconsistent");
        }
        let parcel_bytes: usize = self.regions.values().map(|state| state.parcels.retained_bytes()).sum();
        if parcel_bytes != self.parcel_bytes || parcel_bytes > MAX_PARCEL_BYTES {
            return Err("parcel state accounting is inconsistent");
        }
        if self.regions.values().any(|state| !state.parcels.invariants_hold()) {
            return Err("parcel state invariants are inconsistent");
        }
        let terrain_patches: usize = self.regions.values().map(|state| state.terrain.patch_count()).sum();
        if terrain_patches != self.terrain_patches || terrain_patches > MAX_TERRAIN_PATCHES {
            return Err("terrain patch accounting is inconsistent");
        }
        crate::object::storage::object_store_invariants_hold(&self.store)?;
        if self.store.len() > self.limits.max_entities
            || self.regions.len().saturating_add(self.retired.len()) > self.limits.max_region_registrations
            || self
                .regions
                .keys()
                .any(|region| self.store.region_ids(*region).is_some_and(|ids| ids.len() > self.limits.max_entities_per_region))
        {
            return Err("World storage exceeds its configured limits");
        }
        let mut expected_children = HashMap::<Entity, BTreeMap<Uuid, Entity>>::new();
        let mut expected_pending_children = HashMap::<LocalAddress, HashSet<Entity>>::new();
        let mut object_media_bytes = 0_usize;
        for (id, entity) in crate::object::storage::object_store_entities(&self.store) {
            let object = self.object(entity).ok_or("UUID index points to an incomplete or dead ECS entity")?;
            let address = object.address();
            if !self.regions.contains_key(&address.region()) {
                return Err("live object belongs to an inactive region");
            }
            object_media_bytes = object_media_bytes.saturating_add(object.object_media().map_or(0, ObjectMedia::retained_bytes));
            match object.parent() {
                Parent::Resolved(parent) => {
                    let parent_object = self.object(parent).ok_or("resolved parent is not live")?;
                    if parent_object.address().region() != address.region() {
                        return Err("resolved parent and child belong to different regions");
                    }
                    expected_children.entry(parent).or_default().insert(id, entity);
                }
                Parent::Pending { address, .. } => {
                    if address.region() != object.address().region() {
                        return Err("pending parent and child belong to different regions");
                    }
                    expected_pending_children.entry(address).or_default().insert(entity);
                }
                Parent::Unknown | Parent::Root => {}
            };
        }
        if self.children != expected_children || self.pending_children != expected_pending_children {
            return Err("parent relationship indexes are inconsistent");
        }
        let mut checked = HashSet::with_capacity(self.store.len());
        let mut path = HashSet::new();
        for entity in crate::object::storage::object_store_entities(&self.store).map(|(_, entity)| entity) {
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
        if !crate::history::address::tombstones_invariants_hold(&self.tombstones, &self.regions, &self.store) {
            return Err("tombstone indexes are inconsistent");
        }
        if !crate::history::journal_invariants_hold(&self.journal) {
            return Err("change journal accounting is inconsistent");
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "invariants_tests.rs"]
mod tests;
