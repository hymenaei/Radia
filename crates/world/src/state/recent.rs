use std::{
    collections::{BTreeMap, BTreeSet, Bound::Excluded, Bound::Unbounded},
    mem::size_of,
    time::{Duration, Instant},
};

use base::UUID;

use super::{AttachmentPoint, EntityRemovalReason, Kind, LocalAddress, Properties, PropertiesProvenance, WorldPose};

pub const RECENT_OBJECT_TTL: Duration = Duration::from_secs(5 * 60);
pub const MAX_RECENT_OBJECTS: usize = 4096;
pub const MAX_RECENT_OBJECT_BYTES: usize = 1024 * 1024;

const RECENT_OBJECT_CHARGE: usize = size_of::<UUID>() + size_of::<RecentEntry>() + size_of::<(Instant, UUID)>() + 32;

/// Bounded metadata retained after a live object leaves the World.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecentObject {
    pub id: UUID,
    pub kind: Kind,
    pub address: LocalAddress,
    pub attachment_point: Option<AttachmentPoint>,
    pub properties: Properties,
    pub properties_provenance: PropertiesProvenance,
    pub last_pose: Option<WorldPose>,
    pub removal_reason: EntityRemovalReason,
}

#[derive(Clone, Copy)]
pub(super) struct RecentEntry {
    pub(super) object: RecentObject,
    expires_at: Instant,
}

#[derive(Default)]
pub(super) struct RecentStore {
    by_id: BTreeMap<UUID, RecentEntry>,
    by_expiry: BTreeSet<(Instant, UUID)>,
    charged_bytes: usize,
}

impl RecentStore {
    pub(super) fn insert(&mut self, object: RecentObject, now: Instant) {
        self.insert_with_limits(object, now, RECENT_OBJECT_TTL, MAX_RECENT_OBJECTS, MAX_RECENT_OBJECT_BYTES);
    }

    fn insert_with_limits(&mut self, object: RecentObject, now: Instant, ttl: Duration, max_objects: usize, max_bytes: usize) {
        self.remove(object.id);
        let expires_at = now.checked_add(ttl).unwrap_or(now);
        self.by_id.insert(object.id, RecentEntry { object, expires_at });
        let inserted = self.by_expiry.insert((expires_at, object.id));
        assert!(inserted, "recent record expiry index contains a duplicate key");
        self.charged_bytes = self.charged_bytes.saturating_add(RECENT_OBJECT_CHARGE);
        while self.by_id.len() > max_objects || self.charged_bytes > max_bytes {
            let Some((_, id)) = self.by_expiry.first().copied() else { break };
            self.remove(id);
        }
    }

    pub(super) fn remove(&mut self, id: UUID) -> Option<RecentEntry> {
        let entry = self.by_id.remove(&id)?;
        let removed = self.by_expiry.remove(&(entry.expires_at, id));
        assert!(removed, "recent record is missing from its expiry index");
        self.charged_bytes = self.charged_bytes.saturating_sub(RECENT_OBJECT_CHARGE);
        Some(entry)
    }

    pub(super) fn purge_expired(&mut self, now: Instant, budget: usize) -> usize {
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

    pub(super) fn iter_after(&self, after: Option<UUID>) -> impl Iterator<Item = (UUID, RecentEntry)> + '_ {
        let bounds = after.map_or((Unbounded, Unbounded), |id| (Excluded(id), Unbounded));
        self.by_id.range(bounds).map(|(id, entry)| (*id, *entry))
    }

    pub(super) fn is_expired(&self, entry: RecentEntry, now: Instant) -> bool {
        entry.expires_at <= now
    }

    pub(super) fn next_expiry_after(&self, now: Instant) -> Option<Instant> {
        self.by_expiry.range((Excluded((now, UUID::from_u128(u128::MAX))), Unbounded)).next().map(|(expires_at, _)| *expires_at)
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.by_id.len()
    }

    #[cfg(test)]
    pub(super) fn charged_bytes(&self) -> usize {
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
            id: UUID::from_u128(id),
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
        assert_eq!(store.iter_after(None).map(|(id, _)| id).collect::<Vec<_>>(), [UUID::from_u128(2), UUID::from_u128(3)]);
    }
}
