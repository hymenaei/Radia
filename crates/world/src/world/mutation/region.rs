//! Registration, metadata, and atomic retirement of simulator region lifetimes.

use super::super::journal::WorldChange::{RegionAdopted, RegionRetired, RegionUpdated};
use super::super::{RecentBatch, World};
use super::lifecycle::RemovalCause;
use crate::{ApplyError, Region, RegionApplied, RegionInfo, RegionOutcome};

impl World {
    /// Registers a simulator connection lifetime before accepting its object updates.
    ///
    /// Repeated adoption of an active region is unchanged. A retired lifetime cannot be reused.
    ///
    /// # Errors
    /// Returns an [`ApplyError`] for a retired lifetime or exhausted registration or revision
    /// capacity. The World remains unchanged on error.
    pub fn adopt_region(&mut self, region: Region) -> Result<RegionApplied, ApplyError> {
        if self.retired.contains(&region) {
            return Err(ApplyError::RegionRetired);
        }
        if self.regions.contains_key(&region) {
            return Ok(RegionApplied { outcome: RegionOutcome::Unchanged, recovery_cancellations: Vec::new() });
        }
        if self.regions.len().saturating_add(self.retired.len()) >= self.limits.max_region_registrations {
            return Err(ApplyError::RegistrationBudgetExhausted);
        }
        self.ensure_revision_available()?;
        self.regions.insert(region, RegionInfo::default());
        let revision = self.publish_changes(vec![RegionAdopted(region)]).expect("adopting a region journals a change");
        Ok(RegionApplied { outcome: RegionOutcome::Adopted { revision }, recovery_cancellations: Vec::new() })
    }

    /// Adds non-conflicting metadata to an active simulator region lifetime.
    ///
    /// Missing fields leave stored metadata unchanged. Changing the resolved origin invalidates
    /// derived spatial state for the region.
    ///
    /// # Errors
    /// Returns an [`ApplyError`] for invalid or conflicting metadata, an inactive region, or
    /// exhausted revision capacity. The World remains unchanged on error.
    pub fn patch_region(&mut self, region: Region, info: RegionInfo) -> Result<RegionApplied, ApplyError> {
        self.require_active_region(region)?;
        self.validate_region_info(&info)?;
        let current = self.regions.get(&region).expect("active region remains registered");
        let previous_origin = current.origin.or_else(|| current.handle.map(|handle| handle.origin()));
        let merged = self.merge_region_info(current, &info)?;
        if merged == *current {
            return Ok(RegionApplied { outcome: RegionOutcome::Unchanged, recovery_cancellations: Vec::new() });
        }
        let next_origin = merged.origin.or_else(|| merged.handle.map(|handle| handle.origin()));
        self.ensure_revision_available()?;
        self.regions.insert(region, merged);
        if previous_origin != next_origin {
            self.mark_spatial_region(region);
        }
        let revision = self.publish_changes(vec![RegionUpdated(region)]).expect("patching a region journals a change");
        Ok(RegionApplied { outcome: RegionOutcome::Patched { revision }, recovery_cancellations: Vec::new() })
    }

    /// Atomically retires a region lifetime and removes its live objects.
    ///
    /// Identity, relationships, ordering evidence, and recovery cancellations are updated before
    /// publishing one revision. Repeated retirement is unchanged. Work is proportional to the
    /// region's live objects and affected relationships; this operation does not yield midway.
    ///
    /// # Errors
    /// Returns an [`ApplyError`] for an unknown region or exhausted revision capacity. The World
    /// remains unchanged on error.
    pub fn retire_region(&mut self, region: Region) -> Result<RegionApplied, ApplyError> {
        if self.retired.contains(&region) {
            return Ok(RegionApplied { outcome: RegionOutcome::Unchanged, recovery_cancellations: Vec::new() });
        }
        self.require_active_region(region)?;
        self.ensure_revision_available()?;
        let removed = self.region_entities.get(&region).map_or(0, |ids| ids.len());
        let mut changes = Vec::with_capacity(removed.saturating_add(1));
        let mut recovery_cancellations = Vec::new();
        let mut recent = RecentBatch::default();
        while let Some(id) = self.region_entities.get(&region).and_then(|ids| ids.first()).copied() {
            let entity = self.entities[&id];
            self.remove_entity(
                entity,
                RemovalCause::RegionRetired,
                None,
                &mut changes,
                &mut Vec::new(),
                &mut recovery_cancellations,
                &mut recent,
            );
        }
        self.recent.apply_batch(recent);
        self.regions.remove(&region);
        self.retired.insert(region);
        self.tombstones.remove_region(region);
        self.floors.remove(&region);
        changes.push(RegionRetired { region, entities_removed: removed });
        let revision = self.publish_changes(changes).expect("retiring a region journals a change");
        Ok(RegionApplied { outcome: RegionOutcome::Retired { revision, entities_removed: removed }, recovery_cancellations })
    }

    fn validate_region_info(&self, info: &RegionInfo) -> Result<(), ApplyError> {
        if info.region_id.is_some_and(|id| id.is_nil()) {
            return Err(ApplyError::InvalidValue { field: "region_id" });
        }
        if info.name.as_ref().is_some_and(|name| name.len() > self.limits.max_region_name_bytes) {
            return Err(ApplyError::InvalidValue { field: "region_name" });
        }
        if info.origin.is_some_and(|origin| !origin.is_finite()) {
            return Err(ApplyError::InvalidValue { field: "region_origin" });
        }
        if info.handle.zip(info.origin).is_some_and(|(handle, origin)| handle.origin() != origin) {
            return Err(ApplyError::InvalidValue { field: "region_origin" });
        }
        Ok(())
    }

    fn merge_region_info(&self, current: &RegionInfo, incoming: &RegionInfo) -> Result<RegionInfo, ApplyError> {
        fn merge<T: Copy + PartialEq>(current: Option<T>, incoming: Option<T>, field: &'static str) -> Result<Option<T>, ApplyError> {
            match (current, incoming) {
                (Some(a), Some(b)) if a != b => Err(ApplyError::InvalidValue { field }),
                (Some(a), _) => Ok(Some(a)),
                (None, incoming) => Ok(incoming),
            }
        }
        let region_id = merge(current.region_id, incoming.region_id, "region_id")?;
        let handle = merge(current.handle, incoming.handle, "region_handle")?;
        let origin = merge(current.origin, incoming.origin, "region_origin")?;
        let name = match (&current.name, &incoming.name) {
            (Some(a), Some(b)) if a != b => return Err(ApplyError::InvalidValue { field: "region_name" }),
            (Some(a), _) => Some(a.clone()),
            (None, Some(b)) => Some(b.clone()),
            (None, None) => None,
        };
        let merged = RegionInfo { region_id, handle, name, origin };
        self.validate_region_info(&merged)?;
        Ok(merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApplyError::RevisionExhausted, WorldLimits, WorldRevision};

    fn limits() -> WorldLimits {
        WorldLimits {
            max_region_registrations: 2,
            max_entities: 2,
            max_entities_per_region: 2,
            max_tombstones_per_region: 2,
            max_tombstones: 2,
            max_tombstone_bytes_per_region: WorldLimits::tombstone_bytes_for_count(2),
            max_tombstone_bytes: WorldLimits::tombstone_bytes_for_count(2),
            max_journal_entries: 2,
            max_journal_changes: 4,
            max_region_name_bytes: 16,
            max_recovery_targets: 2,
        }
    }

    #[test]
    fn revision_exhaustion_keeps_region_active() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).expect("adopt region");
        world.journal.revision = WorldRevision(u64::MAX);

        assert!(matches!(world.retire_region(region), Err(RevisionExhausted)));
        assert!(world.region(region).is_some());
        assert!(!world.retired.contains(&region));
    }
}
