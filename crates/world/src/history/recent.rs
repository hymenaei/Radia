use std::{
    collections::{BTreeMap, BTreeSet, Bound::Excluded, Bound::Unbounded},
    mem::size_of,
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::WorldEntityRemovalReason;
use crate::{AttachmentPoint, Kind, LocalAddress, Properties, PropertiesProvenance, WorldPose};

pub const RECENT_OBJECT_TTL: Duration = Duration::from_secs(5 * 60);
pub const MAX_RECENT_OBJECTS: usize = 4096;
pub const MAX_RECENT_OBJECT_BYTES: usize = 1024 * 1024;
const RECENT_OBJECT_CHARGE: usize = size_of::<Uuid>() + size_of::<RecentEntry>() + size_of::<(Instant, Uuid)>() + 32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecentObject {
    pub id: Uuid,
    pub kind: Kind,
    pub address: LocalAddress,
    pub attachment_point: Option<AttachmentPoint>,
    pub properties: Properties,
    pub properties_provenance: PropertiesProvenance,
    pub last_pose: Option<WorldPose>,
    pub removal_reason: WorldEntityRemovalReason,
}

#[derive(Clone, Copy)]
pub(crate) struct RecentEntry {
    pub(crate) object: RecentObject,
    expires_at: Instant,
}

#[derive(Default)]
pub(crate) struct RecentBatch {
    entries: BTreeMap<(Instant, Uuid), RecentObject>,
}

impl RecentBatch {
    pub(crate) fn push(&mut self, object: RecentObject, now: Instant) {
        let limit = MAX_RECENT_OBJECTS.min(MAX_RECENT_OBJECT_BYTES / RECENT_OBJECT_CHARGE);
        if limit == 0 {
            return;
        }
        let expires_at = now.checked_add(RECENT_OBJECT_TTL).unwrap_or(now);
        self.entries.insert((expires_at, object.id), object);
        if self.entries.len() > limit {
            self.entries.pop_first();
        }
    }
}

#[derive(Default)]
pub(crate) struct RecentStore {
    by_id: BTreeMap<Uuid, RecentEntry>,
    by_expiry: BTreeSet<(Instant, Uuid)>,
    charged_bytes: usize,
}

impl RecentStore {
    #[cfg(test)]
    pub(crate) fn insert(&mut self, object: RecentObject, now: Instant) {
        self.insert_with_limits(object, now, RECENT_OBJECT_TTL, MAX_RECENT_OBJECTS, MAX_RECENT_OBJECT_BYTES);
    }

    pub(crate) fn apply_batch(&mut self, batch: RecentBatch) {
        for ((expires_at, _), object) in batch.entries {
            self.insert_entry(RecentEntry { object, expires_at }, MAX_RECENT_OBJECTS, MAX_RECENT_OBJECT_BYTES);
        }
    }

    #[cfg(test)]
    fn insert_with_limits(&mut self, object: RecentObject, now: Instant, ttl: Duration, max_objects: usize, max_bytes: usize) {
        self.insert_entry(RecentEntry { object, expires_at: now.checked_add(ttl).unwrap_or(now) }, max_objects, max_bytes);
    }

    fn insert_entry(&mut self, entry: RecentEntry, max_objects: usize, max_bytes: usize) {
        let RecentEntry { object, expires_at } = entry;
        self.remove(object.id);
        self.by_id.insert(object.id, entry);
        let inserted = self.by_expiry.insert((expires_at, object.id));
        assert!(inserted, "recent record expiry index contains a duplicate key");
        self.charged_bytes = self.charged_bytes.saturating_add(RECENT_OBJECT_CHARGE);
        while self.by_id.len() > max_objects || self.charged_bytes > max_bytes {
            let Some((_, id)) = self.by_expiry.first().copied() else { break };
            self.remove(id);
        }
    }

    pub(crate) fn remove(&mut self, id: Uuid) -> Option<RecentEntry> {
        let entry = self.by_id.remove(&id)?;
        let removed = self.by_expiry.remove(&(entry.expires_at, id));
        assert!(removed, "recent record is missing from its expiry index");
        self.charged_bytes = self.charged_bytes.saturating_sub(RECENT_OBJECT_CHARGE);
        Some(entry)
    }

    pub(crate) fn purge_expired(&mut self, now: Instant, budget: usize) -> usize {
        let mut purged = 0;
        while purged < budget {
            let Some((expires_at, id)) = self.by_expiry.first().copied() else { break };
            if expires_at > now {
                break;
            }
            self.remove(id);
            purged += 1;
        }
        purged
    }

    pub(crate) fn iter_after(&self, after: Option<Uuid>) -> impl Iterator<Item = (Uuid, RecentEntry)> + '_ {
        let bounds = after.map_or((Unbounded, Unbounded), |id| (Excluded(id), Unbounded));
        self.by_id.range(bounds).map(|(id, entry)| (*id, *entry))
    }

    pub(crate) fn is_expired(&self, entry: RecentEntry, now: Instant) -> bool {
        entry.expires_at <= now
    }

    pub(crate) fn next_expiry_after(&self, now: Instant) -> Option<Instant> {
        self.by_expiry.range((Excluded((now, Uuid::from_u128(u128::MAX))), Unbounded)).next().map(|(expires_at, _)| *expires_at)
    }

    #[cfg(feature = "invariants")]
    pub(crate) fn invariants_hold(&self) -> bool {
        if self.by_id.len() != self.by_expiry.len() || self.charged_bytes != self.by_id.len().saturating_mul(RECENT_OBJECT_CHARGE) {
            return false;
        }
        self.by_id.iter().all(|(id, entry)| entry.object.id == *id && self.by_expiry.contains(&(entry.expires_at, *id)))
            && self.by_expiry.iter().all(|(expires_at, id)| self.by_id.get(id).is_some_and(|entry| entry.expires_at == *expires_at))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.by_id.len()
    }

    #[cfg(test)]
    pub(crate) fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::{Region, WorldEntityRemovalReason::Killed};

    fn object(id: u128) -> RecentObject {
        RecentObject {
            id: Uuid::from_u128(id),
            kind: Kind::Object,
            address: LocalAddress::new(Region::new(1), std::num::NonZeroU32::new(id as u32).unwrap()),
            attachment_point: None,
            properties: Properties::default(),
            properties_provenance: PropertiesProvenance::default(),
            last_pose: None,
            removal_reason: Killed,
        }
    }

    #[test]
    fn batch_matches_sequential_retention() {
        let start = Instant::now();
        let capacity = MAX_RECENT_OBJECTS.min(MAX_RECENT_OBJECT_BYTES / RECENT_OBJECT_CHARGE);
        let count = capacity * 2 + 17;
        for tied_timestamps in [false, true] {
            let mut expected = RecentStore::default();
            let mut actual = RecentStore::default();
            for id in count + 1..=count + capacity {
                let now = start + Duration::from_secs((id % 5) as u64);
                expected.insert(object(id as u128), now);
                actual.insert(object(id as u128), now);
            }
            let mut batch = RecentBatch::default();
            for id in (1..=count).rev() {
                let now = start + Duration::from_secs(if tied_timestamps { 3 } else { (id % 7) as u64 });
                let mut removed = object(id as u128);
                removed.properties.owner_id = Some(Uuid::from_u128(id as u128 + 1));
                expected.insert(removed, now);
                batch.push(removed, now);
                assert!(batch.entries.len() <= capacity);
            }
            actual.apply_batch(batch);
            let snapshots =
                |store: &RecentStore| store.iter_after(None).map(|(id, entry)| (id, entry.expires_at, entry.object)).collect::<Vec<_>>();
            assert_eq!(snapshots(&actual), snapshots(&expected));
            assert_eq!(actual.charged_bytes(), expected.charged_bytes());
        }
    }

    #[test]
    fn replacement_refreshes_expiry() {
        let start = Instant::now();
        let mut store = RecentStore::default();
        store.insert_with_limits(object(1), start, Duration::from_secs(1), 4, MAX_RECENT_OBJECT_BYTES);
        store.insert_with_limits(object(1), start + Duration::from_secs(2), Duration::from_secs(1), 4, MAX_RECENT_OBJECT_BYTES);

        assert_eq!(store.len(), 1);
        assert_eq!(store.purge_expired(start + Duration::from_millis(1500), 4), 0);
        assert_eq!(store.purge_expired(start + Duration::from_secs(4), 4), 1);
        assert_eq!(store.charged_bytes(), 0);
    }

    #[test]
    fn byte_cap_evicts_oldest_records() {
        let mut store = RecentStore::default();
        let start = Instant::now();
        store.insert_with_limits(object(1), start, RECENT_OBJECT_TTL, 8, RECENT_OBJECT_CHARGE * 2);
        store.insert_with_limits(object(2), start + Duration::from_secs(1), RECENT_OBJECT_TTL, 8, RECENT_OBJECT_CHARGE * 2);
        store.insert_with_limits(object(3), start + Duration::from_secs(2), RECENT_OBJECT_TTL, 8, RECENT_OBJECT_CHARGE * 2);

        assert_eq!(store.len(), 2);
        assert_eq!(store.charged_bytes(), RECENT_OBJECT_CHARGE * 2);
        assert_eq!(store.iter_after(None).map(|(id, _)| id).collect::<Vec<_>>(), [Uuid::from_u128(2), Uuid::from_u128(3)]);
    }
}
