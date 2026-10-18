//! Cross-checks authoritative storage and every derived World index.

use super::World;
use crate::{Entity, LocalAddress, ObjectMedia, Parent};
use base::UUID;
use std::collections::{BTreeMap, HashMap, HashSet};

impl World {
    /// Checks the World storage and all derived indexes when the `invariants` feature is enabled.
    pub fn check_invariants(&self) -> Result<(), &'static str> {
        self.store.check_invariants()?;
        if self.store.len() > self.limits.max_entities
            || self.regions.len().saturating_add(self.retired.len()) > self.limits.max_region_registrations
            || self
                .regions
                .keys()
                .any(|region| self.store.region_ids(*region).is_some_and(|ids| ids.len() > self.limits.max_entities_per_region))
        {
            return Err("World storage exceeds its configured limits");
        }
        let mut expected_children = HashMap::<Entity, BTreeMap<UUID, Entity>>::new();
        let mut expected_pending_children = HashMap::<LocalAddress, HashSet<Entity>>::new();
        let mut object_media_bytes = 0_usize;
        for (id, entity) in self.store.entities() {
            let object = self.object(entity).ok_or("UUID index points to an incomplete or dead ECS entity")?;
            let address = object.address();
            if !self.regions.contains_key(&address.region()) {
                return Err("live object belongs to an inactive region");
            }
            object_media_bytes =
                object_media_bytes.saturating_add(object.content().object_media.as_deref().map_or(0, ObjectMedia::retained_bytes));
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
        for entity in self.store.entities().map(|(_, entity)| entity) {
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
        if !self.tombstones.invariants_hold(&self.regions, &self.store) {
            return Err("tombstone indexes are inconsistent");
        }
        if let Some(error) = self.spatial.invariant_error(self) {
            return Err(error);
        }
        if !self.journal.invariants_hold() {
            return Err("change journal accounting is inconsistent");
        }
        if !self.recent.invariants_hold() || self.recent.iter_after(None).any(|(id, _)| self.entity(id).is_some()) {
            return Err("recent-object indexes overlap live entities or are inconsistent");
        }
        Ok(())
    }
}
