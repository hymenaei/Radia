use super::Effects;
use super::lifecycle::RemovalCause;
use crate::history::WorldChange::{RegionAdopted, RegionRetired, RegionUpdated};
use crate::update::FieldStamp;
use crate::{ApplyError, Region, RegionApplied, RegionInfo, RegionOutcome, UpdateSource};
use crate::{RegionState, World, newer_field};

impl World {
    pub fn adopt_region(&mut self, region: Region) -> Result<RegionApplied, ApplyError> {
        if self.retired.contains(&region) {
            return Err(ApplyError::RegionRetired);
        }
        if self.regions.contains_key(&region) {
            return Ok(RegionApplied::unchanged());
        }
        if self.regions.len().saturating_add(self.retired.len()) >= self.limits.max_region_registrations {
            return Err(ApplyError::RegistrationBudgetExhausted);
        }
        self.ensure_revision_available()?;
        self.regions.insert(region, RegionState::default());
        let revision = self.publish_changes(vec![RegionAdopted(region)]).expect("adopting a region journals a change");
        Ok(RegionApplied::new(RegionOutcome::Adopted { revision }))
    }
    pub fn patch_region(&mut self, region: Region, info: RegionInfo) -> Result<RegionApplied, ApplyError> {
        self.require_active_region(region)?;
        self.validate_region_info(&info)?;
        let current = self.regions.get(&region).expect("active region remains registered");
        let merged = self.merge_region_info(&current.info, &info)?;
        self.commit_region_metadata(region, merged, current.name_fence)
    }
    pub fn observe_region_metadata(&mut self, source: UpdateSource, mut info: RegionInfo) -> Result<RegionApplied, ApplyError> {
        let region = source.region();
        self.require_active_region(region)?;
        self.validate_region_info(&info)?;
        let current = self.regions.get(&region).expect("active region remains registered");
        let name = info.name.take();
        let mut merged = self.merge_region_info(&current.info, &info)?;
        let mut name_fence = current.name_fence;
        if let (Some(name), Some(stamp)) = (name, source.stamp()) {
            let stamp = FieldStamp { authority: source.authority(), stamp };
            if newer_field(stamp, name_fence) {
                merged.name = Some(name);
                name_fence = Some(stamp);
            }
        }
        self.commit_region_metadata(region, merged, name_fence)
    }
    fn commit_region_metadata(
        &mut self,
        region: Region,
        info: RegionInfo,
        name_fence: Option<FieldStamp>,
    ) -> Result<RegionApplied, ApplyError> {
        let current = self.regions.get(&region).expect("metadata commit requires an active region");
        if info == current.info {
            self.regions.get_mut(&region).expect("active region remains registered").name_fence = name_fence;
            return Ok(RegionApplied::unchanged());
        }
        self.ensure_revision_available()?;
        let current = self.regions.get_mut(&region).expect("active region remains registered");
        current.info = info;
        current.name_fence = name_fence;
        Ok(self.publish_region_patch(RegionUpdated(region)))
    }
    pub fn retire_region(&mut self, region: Region) -> Result<RegionApplied, ApplyError> {
        if self.retired.contains(&region) {
            return Ok(RegionApplied::unchanged());
        }
        self.require_active_region(region)?;
        self.ensure_revision_available()?;
        let removed = self.store.region_ids(region).map_or(0, |ids| ids.len());
        let mut effects = Effects { changes: Vec::with_capacity(removed.saturating_add(1)), ..Effects::default() };
        while let Some(id) = self.store.region_ids(region).and_then(|ids| ids.first()).copied() {
            let entity = self.entity(id).expect("object index contains only live entities");
            self.remove_entity(entity, RemovalCause::RegionRetired, None, &mut effects);
        }
        let removed_region = self.regions.remove(&region).expect("active region remains registered");
        self.terrain_patches -= removed_region.terrain.patch_count();
        self.environment_bytes -= removed_region.environment.source_bytes();
        self.parcel_bytes -= removed_region.parcels.retained_bytes();
        self.retired.insert(region);
        self.tombstones.remove_region(region);
        self.floors.remove(&region);
        effects.changes.push(RegionRetired { region, entities_removed: removed });
        let revision = self.publish_changes(effects.changes).expect("retiring a region journals a change");
        Ok(RegionApplied {
            outcome: RegionOutcome::Retired { revision, entities_removed: removed },
            recovery_cancellations: effects.recovery_cancellations,
        })
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
    fn failed_name_commit_does_not_advance_ordering() {
        let region = Region::new(1);
        let mut world = World::new(limits());
        let _ = world.adopt_region(region).unwrap();
        let source = |order| UpdateSource::new(region, order as u32, crate::OrderEvidence::Ordered(order), 0);
        let name = |name: &str| RegionInfo { name: Some(name.into()), ..RegionInfo::default() };
        let _ = world.observe_region_metadata(source(10), name("First")).unwrap();
        let revision = world.revision();
        world.journal.revision = WorldRevision(u64::MAX);
        assert_eq!(world.observe_region_metadata(source(30), name("Rejected")), Err(RevisionExhausted));
        assert_eq!(world.region(region).unwrap().name.as_deref(), Some("First"));
        world.journal.revision = revision;
        let _ = world.observe_region_metadata(source(20), name("Next")).unwrap();
        assert_eq!(world.region(region).unwrap().name.as_deref(), Some("Next"));
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
