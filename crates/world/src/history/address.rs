#[cfg(feature = "invariants")]
use std::collections::BTreeMap;
use std::{
    collections::{BTreeSet, HashMap},
    num::NonZeroU32,
};

use uuid::Uuid;

#[cfg(feature = "invariants")]
use crate::RegionState;
use crate::World;
#[cfg(feature = "invariants")]
use crate::object::storage::ObjectStore;
use crate::update::Stamp;
use crate::{Applied, ApplyOutcome, DeferredReason, LocalAddress, Region, WorldLimits};

#[derive(Debug, Default)]
pub(crate) struct Tombstones {
    by_address: HashMap<LocalAddress, Stamp>,
    oldest_by_region: HashMap<Region, BTreeSet<(Stamp, NonZeroU32)>>,
}

impl Tombstones {
    pub(crate) fn get(&self, address: &LocalAddress) -> Option<&Stamp> {
        self.by_address.get(address)
    }

    pub(crate) fn contains_key(&self, address: &LocalAddress) -> bool {
        self.by_address.contains_key(address)
    }

    pub(crate) fn len(&self) -> usize {
        self.by_address.len()
    }

    pub(crate) fn insert(&mut self, address: LocalAddress, stamp: Stamp) -> Option<Stamp> {
        let previous = self.remove(&address);
        self.by_address.insert(address, stamp);
        let inserted = self.oldest_by_region.entry(address.region()).or_default().insert((stamp, address.local_id()));
        assert!(inserted, "tombstone address index and eviction index disagree");
        previous
    }

    pub(crate) fn remove(&mut self, address: &LocalAddress) -> Option<Stamp> {
        let stamp = self.by_address.remove(address)?;
        let region = address.region();
        let entries = self.oldest_by_region.get_mut(&region).expect("every tombstone has a region index");
        assert!(entries.remove(&(stamp, address.local_id())), "tombstone eviction index is missing an address");
        if entries.is_empty() {
            self.oldest_by_region.remove(&region);
        }
        Some(stamp)
    }

    pub(crate) fn remove_region(&mut self, region: Region) {
        if let Some(entries) = self.oldest_by_region.remove(&region) {
            for (_, local_id) in entries {
                self.by_address.remove(&LocalAddress::new(region, local_id));
            }
        }
    }

    fn region_len(&self, region: Region) -> usize {
        self.oldest_by_region.get(&region).map_or(0, BTreeSet::len)
    }

    fn oldest_in_region(&self, region: Region) -> Option<(Stamp, NonZeroU32)> {
        self.oldest_by_region.get(&region)?.first().copied()
    }

    #[cfg(feature = "invariants")]
    pub(crate) fn invariants_hold(&self, active_regions: &BTreeMap<Region, RegionState>, store: &ObjectStore) -> bool {
        self.by_address.len() == self.oldest_by_region.values().map(BTreeSet::len).sum::<usize>()
            && self.by_address.iter().all(|(address, stamp)| {
                active_regions.contains_key(&address.region())
                    && store.entity_at(*address).is_none()
                    && self.oldest_by_region.get(&address.region()).is_some_and(|entries| entries.contains(&(*stamp, address.local_id())))
            })
            && self.oldest_by_region.iter().all(|(region, entries)| {
                !entries.is_empty()
                    && entries.iter().all(|(stamp, local_id)| self.by_address.get(&LocalAddress::new(*region, *local_id)) == Some(stamp))
            })
    }
}

impl World {
    pub(crate) fn tombstone_would_change(&self, address: LocalAddress, stamp: Stamp) -> bool {
        if self.entity_at(address).is_some() || self.tombstones.get(&address).is_some_and(|existing| *existing >= stamp) {
            return false;
        }
        if self.max_tombstones() == 0 || self.max_tombstones_per_region() == 0 {
            return self.floors.get(&address.region).is_none_or(|floor| stamp > *floor);
        }
        true
    }

    pub(crate) fn insert_tombstone(&mut self, address: LocalAddress, stamp: Stamp) -> bool {
        if self.entity_at(address).is_some() || self.tombstones.get(&address).is_some_and(|existing| *existing >= stamp) {
            return false;
        }
        if self.max_tombstones() == 0 || self.max_tombstones_per_region() == 0 {
            let replaced = self.tombstones.remove(&address).is_some();
            return self.advance_floor(address.region, stamp) || replaced;
        }
        let replaced = self.tombstones.insert(address, stamp).is_some();
        let mut changed = true;
        loop {
            if self.tombstones.region_len(address.region()) <= self.max_tombstones_per_region()
                && self.tombstones.len() <= self.max_tombstones()
            {
                break;
            }
            let Some((evicted_stamp, local_id)) = self.tombstones.oldest_in_region(address.region()) else { break };
            let key = LocalAddress::new(address.region(), local_id);
            self.tombstones.remove(&key);
            let floor_changed = self.advance_floor(key.region, evicted_stamp);
            if key == address {
                changed = replaced || floor_changed;
            }
        }
        changed
    }

    fn max_tombstones_per_region(&self) -> usize {
        let byte_limit = self.limits.max_tombstone_bytes_per_region / WorldLimits::tombstone_bytes_for_count(1);
        self.limits.max_tombstones_per_region.min(byte_limit)
    }

    fn max_tombstones(&self) -> usize {
        let byte_limit = self.limits.max_tombstone_bytes / WorldLimits::tombstone_bytes_for_count(1);
        self.limits.max_tombstones.min(byte_limit)
    }

    fn advance_floor(&mut self, region: Region, stamp: Stamp) -> bool {
        match self.floors.get_mut(&region) {
            Some(floor) if stamp > *floor => {
                *floor = stamp;
                true
            }
            Some(_) => false,
            None => {
                self.floors.insert(region, stamp);
                true
            }
        }
    }

    pub(crate) fn preflight_address(&self, address: LocalAddress, id: Uuid, stamp: Stamp) -> Option<Applied> {
        if let Some(entity) = self.entity_at(address) {
            let record = self.store.record(entity).expect("address index contains a live entity");
            return (record.id != id && stamp <= record.state.binding.fence()).then(|| Applied::new(ApplyOutcome::IgnoredStale));
        }
        if let Some(tombstone) = self.tombstones.get(&address) {
            return (stamp <= *tombstone).then(|| Applied::new(ApplyOutcome::IgnoredStale));
        }
        self.floors.get(&address.region).is_some_and(|floor| stamp <= *floor).then(|| self.defer(DeferredReason::AmbiguousOrder, address))
    }

    pub(crate) fn unknown_address(&self, address: LocalAddress, stamp: Stamp) -> Applied {
        match self.tombstones.get(&address) {
            Some(tombstone) if stamp <= *tombstone => Applied::unchanged(),
            None if self.floors.get(&address.region).is_some_and(|floor| stamp <= *floor) => {
                self.defer(DeferredReason::AmbiguousOrder, address)
            }
            Some(_) | None => self.defer(DeferredReason::UnknownObject, address),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tombstone_index_tracks_mutations() {
        let first_region = Region::new(1);
        let second_region = Region::new(2);
        let first = LocalAddress::new(first_region, NonZeroU32::new(1).unwrap());
        let second = LocalAddress::new(first_region, NonZeroU32::new(2).unwrap());
        let other = LocalAddress::new(second_region, NonZeroU32::new(1).unwrap());
        let old = Stamp { order: 1, block_ordinal: 0 };
        let new = Stamp { order: 3, block_ordinal: 0 };
        let replacement = Stamp { order: 4, block_ordinal: 0 };
        let mut tombstones = Tombstones::default();

        assert_eq!(tombstones.insert(first, old), None);
        assert_eq!(tombstones.insert(second, new), None);
        assert_eq!(tombstones.insert(other, old), None);
        assert_eq!(tombstones.region_len(first_region), 2);
        assert_eq!(tombstones.oldest_in_region(first_region), Some((old, first.local_id())));

        assert_eq!(tombstones.insert(first, replacement), Some(old));
        assert_eq!(tombstones.oldest_in_region(first_region), Some((new, second.local_id())));
        assert_eq!(tombstones.remove(&second), Some(new));
        assert_eq!(tombstones.region_len(first_region), 1);

        tombstones.remove_region(first_region);
        assert_eq!(tombstones.len(), 1);
        assert!(tombstones.contains_key(&other));
        assert_eq!(tombstones.oldest_in_region(first_region), None);
    }
}
