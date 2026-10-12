use super::journal::Change::{RegionAdopted, RegionRetired, RegionUpdated};
use super::{Applied, ApplyError, ApplyOutcome, EntityRemovalReason, Region, RegionInfo, World};

impl World {
    pub fn adopt_region(&mut self, region: Region) -> Result<Applied, ApplyError> {
        if self.retired.contains(&region) {
            return Err(ApplyError::RegionRetired);
        }
        if self.regions.contains_key(&region) {
            return Ok(Applied::unchanged());
        }
        if self.registration_count >= self.limits.max_region_registrations {
            return Err(ApplyError::RegistrationBudgetExhausted);
        }
        self.ensure_revision_available()?;
        self.registration_count += 1;
        self.regions.insert(region, RegionInfo::default());
        Ok(self.finish(ApplyOutcome::RegionAdopted, vec![RegionAdopted(region)], None))
    }

    pub fn patch_region(&mut self, region: Region, info: RegionInfo) -> Result<Applied, ApplyError> {
        self.require_active_region(region)?;
        self.validate_region_info(&info)?;
        let current = self.regions.get(&region).expect("active region remains registered");
        let merged = self.merge_region_info(current, &info)?;
        if merged == *current {
            return Ok(Applied::unchanged());
        }
        self.ensure_revision_available()?;
        self.regions.insert(region, merged);
        Ok(self.finish(ApplyOutcome::RegionPatched, vec![RegionUpdated(region)], None))
    }

    pub fn retire_region(&mut self, region: Region) -> Result<Applied, ApplyError> {
        if self.retired.contains(&region) {
            return Ok(Applied::unchanged());
        }
        self.require_active_region(region)?;
        self.ensure_revision_available()?;
        let entities = self.region_entities.get(&region).cloned().unwrap_or_default();
        let removed = entities.len();
        let mut changes = Vec::with_capacity(removed.saturating_add(1));
        let mut recovery_cancellations = Vec::new();
        for entity in entities {
            self.remove_entity(
                entity,
                None,
                false,
                EntityRemovalReason::RegionRetired,
                None,
                &mut changes,
                &mut Vec::new(),
                &mut recovery_cancellations,
            )?;
        }
        self.regions.remove(&region);
        self.retired.insert(region);
        self.tombstones.remove_region(region);
        self.floors.remove(&region);
        changes.push(RegionRetired { region, entities_removed: removed });
        Ok(self
            .finish(ApplyOutcome::RegionRetired { entities_removed: removed }, changes, None)
            .with_recovery_cancellations(recovery_cancellations))
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
