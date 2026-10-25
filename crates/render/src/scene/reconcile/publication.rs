use std::mem::take;

use super::{Batch, Change, MAX_BATCH_CHANGES, PendingPublication, Receipt, Reconciler, Status, WorldRevision};

impl Reconciler {
    pub(super) fn collect_changes(&mut self) -> Vec<Change> {
        let dirty = take(&mut self.publication.dirty);
        let count = if self.publication.replace {
            1 + self.state.mirror.regions.len()
                + self.state.mirror.environments.len()
                + self.state.mirror.objects.len()
                + self.state.mirror.patches.len()
                + self.state.mirror.water.len()
                + self.state.mirror.surfaces.len()
        } else {
            dirty.removed.len()
                + dirty.retired.len()
                + dirty.regions.len()
                + dirty.environments.len()
                + dirty.objects.len()
                + dirty.patches.len()
                + dirty.water.len()
                + dirty.surfaces.len()
        };
        debug_assert!(count <= MAX_BATCH_CHANGES);
        let mut changes = Vec::with_capacity(count);
        if self.publication.replace {
            changes.push(Change::ViewerEnvironment(self.viewer.current()));
            changes.extend(self.state.mirror.regions.values().cloned().map(Change::Region));
            changes.extend(self.state.mirror.environments.iter().map(|(region, value)| Change::Environment(*region, Some(value.clone()))));
            changes.extend(self.state.mirror.objects.values().cloned().map(Change::Object));
            changes.extend(self.state.mirror.patches.iter().map(|((region, _), patch)| Change::Patch(*region, patch.clone())));
            changes.extend(self.state.mirror.water.iter().map(|(region, height)| Change::Water(*region, *height)));
            changes.extend(self.state.mirror.surfaces.values().cloned().map(Change::Surface));
        } else {
            changes.extend(dirty.removed.into_iter().map(Change::Remove));
            changes.extend(dirty.retired.into_iter().map(Change::Retire));
            changes.extend(
                dirty
                    .environments
                    .into_iter()
                    .map(|region| Change::Environment(region, self.state.mirror.environments.get(&region).cloned())),
            );
            changes
                .extend(dirty.regions.into_iter().filter_map(|region| self.state.mirror.regions.get(&region).cloned().map(Change::Region)));
            changes
                .extend(dirty.objects.into_iter().filter_map(|entity| self.state.mirror.objects.get(&entity).cloned().map(Change::Object)));
            changes.extend(
                dirty
                    .patches
                    .into_iter()
                    .filter_map(|key| self.state.mirror.patches.get(&key).cloned().map(|patch| Change::Patch(key.0, patch))),
            );
            changes.extend(dirty.water.into_iter().map(|region| Change::Water(region, self.state.mirror.water[&region])));
            changes.extend(dirty.surfaces.into_iter().map(|(region, kind)| {
                self.state.mirror.surfaces.get(&(region, kind)).cloned().map_or(Change::RemoveSurface(region, kind), Change::Surface)
            }));
        }
        changes
    }

    pub(super) fn batch(&mut self, replace: bool, changes: Vec<Change>) -> Batch {
        self.batch_at(replace, changes, self.world.revision, self.recovery.status)
    }

    pub(super) fn batch_at(&mut self, replace: bool, changes: Vec<Change>, revision: Option<WorldRevision>, status: Status) -> Batch {
        self.publication.sequence = self.publication.sequence.checked_add(1).expect("Scene sequence space exhausted");
        let (receipt, delivery) = Receipt::new();
        self.publication.pending = Some(PendingPublication { receipt, revision, status });
        Batch { session: self.session.clone(), sequence: self.publication.sequence, revision, status, replace, changes, delivery }
    }
}
