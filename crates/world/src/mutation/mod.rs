mod fields;
mod lifecycle;
mod observe;
mod patch;
mod region;

use super::World;
use crate::{
    Applied, ApplyError, ApplyOutcome, DeferredReason, Entity, Kind, LocalAddress, Parent, PoseState, RecentBatch, Recovery, RegionApplied,
    RegionOutcome, WorldChange, WorldPose, WorldRevision,
};

#[derive(Default)]
pub(crate) struct Effects {
    pub(crate) changes: Vec<WorldChange>,
    pub(crate) recovery_targets: Vec<LocalAddress>,
    pub(crate) recovery_cancellations: Vec<LocalAddress>,
    pub(crate) recent: RecentBatch,
}

impl Effects {
    fn from_changes(changes: Vec<WorldChange>) -> Self {
        Self { changes, ..Self::default() }
    }
}

impl World {
    pub(super) fn ensure_revision_available(&self) -> Result<(), ApplyError> {
        self.journal.revision.0.checked_add(1).map(|_| ()).ok_or(ApplyError::RevisionExhausted)
    }

    pub(super) fn publish_changes(&mut self, changes: Vec<WorldChange>) -> Option<WorldRevision> {
        self.update_spatial_state(&changes);
        self.journal.append(changes, self.limits.max_journal_entries, self.limits.max_journal_changes)
    }

    pub(super) fn publish_region_patch(&mut self, change: WorldChange) -> RegionApplied {
        let revision = self.publish_changes(vec![change]).expect("a region patch journals its change");
        RegionApplied::new(RegionOutcome::Patched { revision })
    }

    pub(super) fn recovery(&self, targets: Vec<LocalAddress>) -> Recovery {
        Recovery::from_targets(targets, self.limits.max_recovery_targets)
    }

    pub(super) fn defer(&self, reason: DeferredReason, address: LocalAddress) -> Applied {
        Applied::deferred(reason, self.recovery(vec![address]))
    }

    fn detach_snapshot(&self, entity: Entity, kind: Kind, current: Parent, next: Parent) -> Option<WorldPose> {
        let leaves_object =
            matches!(current, Parent::Resolved(parent) if self.store.record(parent).is_some_and(|record| record.kind != Kind::Avatar));
        if kind != Kind::Avatar || next != Parent::Root || !leaves_object {
            return None;
        }
        match self.resolve_pose(entity) {
            Ok(PoseState::Resolved(pose)) => Some(pose),
            Ok(PoseState::Unresolved(_) | PoseState::Provisional { .. }) | Err(_) => None,
        }
    }

    fn finish(&mut self, outcome: ApplyOutcome, effects: Effects) -> Applied {
        self.recent.apply_batch(effects.recent);
        let recovery = self.recovery(effects.recovery_targets);
        Applied {
            revision: self.publish_changes(effects.changes),
            recovery,
            recovery_cancellations: effects.recovery_cancellations,
            ..Applied::new(outcome)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use uuid::Uuid;

    use crate::World;
    use crate::update::Stamp;
    use crate::{
        ApplyOutcome,
        ApplyOutcome::{Deferred, TombstoneRecorded},
        DeferredReason,
        DeferredReason::AmbiguousOrder,
        Kind, LocalAddress, ObjectObservation, ObjectPatch,
        OrderEvidence::Ordered,
        ParentUpdate::Root,
        Region, UpdateSource, WorldLimits,
    };

    pub(super) fn test_world(regions: &[Region]) -> World {
        let tombstones = 8;
        let mut world = World::new(WorldLimits {
            max_region_registrations: regions.len(),
            max_entities: 8,
            max_entities_per_region: 8,
            max_tombstones_per_region: tombstones,
            max_tombstones: tombstones,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(tombstones),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(tombstones),
            max_journal_entries: 8,
            max_journal_changes: 32,
            max_region_name_bytes: 32,
            max_recovery_targets: 4,
        });
        for region in regions {
            assert!(matches!(world.adopt_region(*region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));
        }
        world
    }

    #[test]
    fn floor_defers_unknown_updates() {
        let region = Region::new(24);
        let mut world = test_world(&[region]);
        let address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        world.floors.insert(region, Stamp { order: 200, block_ordinal: 0 });
        let stale = UpdateSource::new(region, 150, Ordered(150), 0);

        let patch = world.patch_object(ObjectPatch::new(stale, address.local_id())).unwrap();
        assert_eq!(patch.outcome, Deferred(AmbiguousOrder));
        assert_eq!(patch.recovery.requests, [address]);
        let removal = world.remove_object(stale, address.local_id()).unwrap();
        assert_eq!(removal.outcome, Deferred(AmbiguousOrder));
        assert_eq!(removal.recovery.requests, [address]);
        assert_eq!(world.tombstones.get(&address), None);

        let newer = UpdateSource::new(region, 250, Ordered(250), 0);
        assert_eq!(world.remove_object(newer, address.local_id()).unwrap().outcome, TombstoneRecorded(address));
    }

    pub(super) fn observation(region: Region, id: u128, local_id: u32, sequence: u32) -> ObjectObservation {
        ObjectObservation::new(
            UpdateSource::new(region, sequence, Ordered(u64::from(sequence)), 0),
            Uuid::from_u128(id),
            NonZeroU32::new(local_id).unwrap(),
            Kind::Object,
            Root,
        )
    }

    #[test]
    fn tombstone_takes_precedence_over_floor() {
        let region = Region::new(20);
        let mut world = World::new(WorldLimits {
            max_region_registrations: 1,
            max_entities: 1,
            max_entities_per_region: 1,
            max_tombstones_per_region: 1,
            max_tombstones: 1,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(1),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(1),
            max_journal_entries: 4,
            max_journal_changes: 8,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        });
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));

        let address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        world.tombstones.insert(address, Stamp { order: 100, block_ordinal: 0 });
        world.floors.insert(region, Stamp { order: 200, block_ordinal: 0 });

        let result = world
            .observe_object(ObjectObservation::new(
                UpdateSource::new(region, 90, Ordered(90), 0),
                Uuid::from_u128(20),
                NonZeroU32::new(1).unwrap(),
                Kind::Object,
                Root,
            ))
            .unwrap();

        assert_eq!(result.outcome, ApplyOutcome::IgnoredStale);
        assert!(result.recovery.requests.is_empty());
        assert_eq!(world.entity(Uuid::from_u128(20)), None);
    }

    #[test]
    fn newer_than_tombstone_bypasses_floor() {
        let region = Region::new(22);
        let mut world = World::new(WorldLimits {
            max_region_registrations: 1,
            max_entities: 1,
            max_entities_per_region: 1,
            max_tombstones_per_region: 2,
            max_tombstones: 2,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(2),
            max_journal_entries: 4,
            max_journal_changes: 8,
            max_region_name_bytes: 8,
            max_recovery_targets: 1,
        });
        assert!(matches!(world.adopt_region(region).unwrap().outcome, crate::RegionOutcome::Adopted { .. }));

        let killed_address = LocalAddress::new(region, NonZeroU32::new(1).unwrap());
        let patched_address = LocalAddress::new(region, NonZeroU32::new(2).unwrap());
        let old = Stamp { order: 100, block_ordinal: 0 };
        world.tombstones.insert(killed_address, old);
        world.tombstones.insert(patched_address, old);
        world.floors.insert(region, Stamp { order: 200, block_ordinal: 0 });

        let killed = world.remove_object(UpdateSource::new(region, 150, Ordered(150), 0), killed_address.local_id()).unwrap();
        assert_eq!(killed.outcome, ApplyOutcome::TombstoneRecorded(killed_address));
        assert_eq!(world.tombstones.get(&killed_address), Some(&Stamp { order: 150, block_ordinal: 0 }));

        let patched =
            world.patch_object(ObjectPatch::new(UpdateSource::new(region, 150, Ordered(150), 0), patched_address.local_id())).unwrap();
        assert_eq!(patched.outcome, ApplyOutcome::Deferred(DeferredReason::UnknownObject));
        assert_eq!(patched.recovery.requests, [patched_address]);
    }
}
