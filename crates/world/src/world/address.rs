//! Local-address admission, tombstones, and bounded ordering evidence.

#[cfg(feature = "invariants")]
use super::RegionState;
use super::World;
#[cfg(feature = "invariants")]
use super::storage::ObjectStore;
use crate::update::Stamp;
use crate::{Applied, ApplyOutcome, DeferredReason, LocalAddress, Recovery, Region, WorldLimits};
use base::UUID;
#[cfg(feature = "invariants")]
use std::collections::BTreeMap;
use std::{
    collections::{BTreeSet, HashMap},
    num::NonZeroU32,
};

#[derive(Debug, Default)]
pub(super) struct Tombstones {
    by_address: HashMap<LocalAddress, Stamp>,
    oldest_by_region: HashMap<Region, BTreeSet<(Stamp, NonZeroU32)>>,
}

impl Tombstones {
    pub(super) fn get(&self, address: &LocalAddress) -> Option<&Stamp> {
        self.by_address.get(address)
    }

    pub(super) fn contains_key(&self, address: &LocalAddress) -> bool {
        self.by_address.contains_key(address)
    }

    pub(super) fn len(&self) -> usize {
        self.by_address.len()
    }

    pub(super) fn insert(&mut self, address: LocalAddress, stamp: Stamp) -> Option<Stamp> {
        let previous = self.remove(&address);
        self.by_address.insert(address, stamp);
        let inserted = self.oldest_by_region.entry(address.region()).or_default().insert((stamp, address.local_id()));
        assert!(inserted, "tombstone address index and eviction index disagree");
        previous
    }

    pub(super) fn remove(&mut self, address: &LocalAddress) -> Option<Stamp> {
        let stamp = self.by_address.remove(address)?;
        let region = address.region();
        let entries = self.oldest_by_region.get_mut(&region).expect("every tombstone has a region index");
        assert!(entries.remove(&(stamp, address.local_id())), "tombstone eviction index is missing an address");
        if entries.is_empty() {
            self.oldest_by_region.remove(&region);
        }
        Some(stamp)
    }

    pub(super) fn remove_region(&mut self, region: Region) {
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
    pub(super) fn invariants_hold(&self, active_regions: &BTreeMap<Region, RegionState>, store: &ObjectStore) -> bool {
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
    pub(super) fn tombstone_would_change(&self, address: LocalAddress, stamp: Stamp) -> bool {
        if self.entity_at(address).is_some() || self.tombstones.get(&address).is_some_and(|existing| *existing >= stamp) {
            return false;
        }
        if self.max_tombstones() == 0 || self.max_tombstones_per_region() == 0 {
            return self.floors.get(&address.region).is_none_or(|floor| stamp > *floor);
        }
        true
    }

    pub(super) fn insert_tombstone(&mut self, address: LocalAddress, stamp: Stamp) -> bool {
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

    pub(super) fn preflight_address(&self, address: LocalAddress, id: UUID, stamp: Stamp) -> Option<Applied> {
        if let Some(entity) = self.entity_at(address) {
            let record = self.store.record(entity).expect("address index contains a live entity");
            if record.id != id && stamp <= record.state.binding.fence() {
                let mut applied = Applied::unchanged();
                applied.outcome = ApplyOutcome::IgnoredStale;
                return Some(applied);
            }
            return None;
        }
        if let Some(tombstone) = self.tombstones.get(&address) {
            if stamp <= *tombstone {
                let mut applied = Applied::unchanged();
                applied.outcome = ApplyOutcome::IgnoredStale;
                return Some(applied);
            }
            return None;
        }
        if self.floors.get(&address.region).is_some_and(|floor| stamp <= *floor) {
            return Some(Applied::deferred(
                DeferredReason::AmbiguousOrder,
                Recovery::from_targets(vec![address], self.limits.max_recovery_targets),
            ));
        }
        None
    }

    pub(super) fn unknown_address(&self, address: LocalAddress, stamp: Stamp) -> Applied {
        if let Some(tombstone) = self.tombstones.get(&address) {
            if stamp <= *tombstone {
                return Applied::unchanged();
            }
            return Applied::deferred(
                DeferredReason::UnknownObject,
                Recovery::from_targets(vec![address], self.limits.max_recovery_targets),
            );
        }
        if self.floors.get(&address.region).is_some_and(|floor| stamp <= *floor) {
            return Applied::deferred(
                DeferredReason::AmbiguousOrder,
                Recovery::from_targets(vec![address], self.limits.max_recovery_targets),
            );
        }
        Applied::deferred(DeferredReason::UnknownObject, Recovery::from_targets(vec![address], self.limits.max_recovery_targets))
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
