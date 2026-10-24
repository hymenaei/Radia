use world::{
    MAX_ENVIRONMENT_READ_BYTES, MAX_REGION_PAGE_ITEMS, MAX_TERRAIN_PAGE_PATCHES, MAX_WORLD_CHANGE_PAGE_ITEMS, ObjectDetailResult,
    WorldChange,
};

use super::{
    Arc, Change, Entity, EnvironmentData, Instant, Lease, MAX_OBJECT_PAGE_ITEMS, MAX_OBJECTS, MAX_PATCHES, MAX_PENDING, MAX_REGIONS,
    MAX_RETAINED_BYTES, Object, PATCH_BYTES, Patch, READ_BYTES, ReadFailure, ReadJob, ReadQuery, ReadReply, ReconcileStep, Reconciler,
    RecoveryReason, Region, RegionData, RegionSummary, SurfaceKind, Weak, WorldReadError, WorldRevision, size_of,
};

impl Reconciler {
    pub(super) fn read(&mut self, query: ReadQuery, now: Instant) -> ReconcileStep {
        let bytes = if matches!(query, ReadQuery::Environment(_)) { MAX_ENVIRONMENT_READ_BYTES + 32 } else { READ_BYTES };
        let Some(lease) = self.budget.reserve(bytes) else {
            self.begin_recovery(RecoveryReason::Capacity, now);
            return ReconcileStep::Continue;
        };
        let life = Arc::new(());
        if let ReadQuery::Terrain(region, _) = &query {
            self.preparation.surfaces.hydrating(*region, true);
        }
        self.world.reading = Some(Arc::downgrade(&life));
        self.world.reads_since_changes =
            if matches!(query, ReadQuery::Changes(_) | ReadQuery::Barrier) { 0 } else { self.world.reads_since_changes.saturating_add(1) };
        ReconcileStep::Read(ReadJob { session: self.session.clone(), epoch: self.world.epoch, query, lease, life })
    }

    pub fn complete_read(&mut self, mut job: ReadJob, reply: Result<ReadReply, ReadFailure>, now: Instant) {
        if !self.session.is_active()
            || !job.session.is_active()
            || job.epoch != self.world.epoch
            || !self.world.reading.as_ref().is_some_and(|reading| Weak::ptr_eq(reading, &Arc::downgrade(&job.life)))
        {
            return;
        }
        self.world.reading = None;
        let reply = match reply {
            Ok(reply) => reply,
            Err(ReadFailure::Closed | ReadFailure::World(WorldReadError::StaleSession)) => {
                self.close();
                return;
            }
            Err(ReadFailure::World(WorldReadError::ResyncRequired)) => {
                self.begin_recovery(RecoveryReason::HistoryLost, now);
                return;
            }
            Err(ReadFailure::World(WorldReadError::TooLarge)) => {
                self.begin_recovery(RecoveryReason::Capacity, now);
                return;
            }
            Err(_) => {
                self.begin_recovery(RecoveryReason::ReadFailed, now);
                return;
            }
        };
        let result = self.apply_read_reply(&job.query, reply, &mut job.lease);
        if let Err(reason) = result {
            self.begin_recovery(reason, now);
        }
    }

    fn apply_read_reply(&mut self, query: &ReadQuery, reply: ReadReply, lease: &mut Lease) -> Result<(), RecoveryReason> {
        match (query, reply) {
            (ReadQuery::Barrier, ReadReply::Barrier(barrier)) => {
                if barrier.object_count > MAX_OBJECTS || barrier.region_count > MAX_REGIONS {
                    return Err(RecoveryReason::Capacity);
                }
                self.world.cursor = Some(barrier.changes);
                self.world.barrier = Some(barrier);
            }
            (ReadQuery::Objects(_), ReadReply::Objects(page)) if page.objects.len() <= MAX_OBJECT_PAGE_ITEMS => {
                for item in page.objects {
                    self.invalidate(item.entity);
                }
                self.world.objects = page.next;
                self.world.enumerate_objects = page.next.is_some();
            }
            (ReadQuery::Regions(_), ReadReply::Regions(page)) if page.regions.len() <= MAX_REGION_PAGE_ITEMS => {
                for summary in page.regions {
                    let region = summary.region;
                    let bytes = size_of::<RegionSummary>() + summary.info.name.as_ref().map_or(0, String::capacity) + 32;
                    let old = self.state.mirror.regions.get(&region);
                    let origin_changed = old.is_some_and(|old| old.summary().info.resolved_origin() != summary.info.resolved_origin());
                    let spatial_changed = old.is_none() || origin_changed;
                    if old.is_none() && self.state.mirror.regions.len() >= MAX_REGIONS {
                        return Err(RecoveryReason::Capacity);
                    }
                    let old_bytes = old.map_or(0, region_bytes);
                    if origin_changed {
                        self.conservative_scan();
                    }
                    self.retain(bytes, old_bytes)?;
                    let charge = lease.split(bytes).ok_or(RecoveryReason::Capacity)?;
                    self.state.mirror.regions.insert(region, RegionData::new(summary, charge));
                    self.world.environments.insert(region);
                    if spatial_changed {
                        self.preparation.surfaces.regions(&self.state.mirror);
                    }
                    self.publication.dirty.regions.insert(region);
                    if self.publication.replace || self.recovery.startup {
                        self.world.terrain.insert(region);
                    }
                }
                self.world.regions = page.next;
                self.world.refresh_regions = page.next.is_some();
            }
            (ReadQuery::Details(entities), ReadReply::Details(batch)) if batch.details.len() == entities.len() => {
                for (entity, detail) in entities.iter().copied().zip(batch.details) {
                    let returned = match &detail {
                        ObjectDetailResult::Found(detail) => detail.summary.entity,
                        ObjectDetailResult::Gone(entity) | ObjectDetailResult::TooLarge(entity) => *entity,
                    };
                    if returned != entity {
                        return Err(RecoveryReason::InvalidReply);
                    }
                    if self.world.pending.pop_front() != Some(entity) {
                        return Err(RecoveryReason::InvalidReply);
                    }
                    self.world.pending_set.remove(&entity);
                    match detail {
                        ObjectDetailResult::Found(detail) => {
                            if detail.hydration_revision != batch.hydration_revision {
                                return Err(RecoveryReason::InvalidReply);
                            }
                            let conflicts = [
                                self.state.identities.get(&detail.summary.id).copied(),
                                self.state.bindings.get(&detail.summary.address).copied(),
                            ];
                            for conflict in conflicts.into_iter().flatten() {
                                if conflict != entity {
                                    self.expand(conflict);
                                    self.remove(conflict);
                                }
                            }
                            if !self.state.mirror.objects.contains_key(&entity) && self.state.mirror.objects.len() >= MAX_OBJECTS {
                                return Err(RecoveryReason::Capacity);
                            }
                            let bytes = detail.owned_bytes() + 32;
                            let old_bytes = self.state.mirror.objects.get(&entity).map_or(0, object_bytes);
                            self.retain(bytes, old_bytes)?;
                            let charge = lease.split(bytes).ok_or(RecoveryReason::Capacity)?;
                            if let Some(old) = self.state.mirror.objects.get(&entity) {
                                self.state.bindings.remove(&old.detail().summary.address);
                            }
                            self.state.identities.insert(detail.summary.id, entity);
                            self.state.bindings.insert(detail.summary.address, entity);
                            self.state.dependencies.replace(entity, detail.summary.parent);
                            self.state.mirror.apply(Change::Object(Object::new(detail, charge)));
                            self.preparation.meshes.invalidate();
                            self.preparation.assets.invalidate(&self.state.mirror.objects[&entity]);
                            self.publication.dirty.objects.insert(entity);
                        }
                        ObjectDetailResult::Gone(_) => self.remove(entity),
                        ObjectDetailResult::TooLarge(_) => return Err(RecoveryReason::Capacity),
                    }
                }
            }
            (ReadQuery::Changes(_), ReadReply::Changes(page)) if page.records.len() <= MAX_WORLD_CHANGE_PAGE_ITEMS => {
                let partial = page.records.last().is_some_and(|record| record.revision > page.completed_revision);
                for record in page.records {
                    self.apply_world_change(record.change, record.revision)?;
                }
                self.world.cursor = Some(page.cursor);
                self.world.revision = Some(page.completed_revision);
                self.world.needs_changes = page.has_more;
                self.publication.publish = !partial;
            }
            (ReadQuery::Environment(region), ReadReply::Environment(source)) => {
                let old_bytes = self.state.mirror.environments.get(region).map_or(0, |source| source.source().retained_bytes() + 32);
                if let Some(source) = source {
                    if source.region != *region || !self.state.mirror.regions.contains_key(region) {
                        return Err(RecoveryReason::InvalidReply);
                    }
                    let bytes = source.retained_bytes() + 32;
                    if bytes > MAX_ENVIRONMENT_READ_BYTES + 32 {
                        return Err(RecoveryReason::Capacity);
                    }
                    let charge = lease.split(bytes).ok_or(RecoveryReason::Capacity)?;
                    self.retain(bytes, old_bytes)?;
                    self.state.mirror.environments.insert(*region, EnvironmentData::new(source, charge));
                } else {
                    self.state.bytes -= old_bytes;
                    self.state.mirror.environments.remove(region);
                }
                self.publication.dirty.environments.insert(*region);
                self.publication.publish = true;
            }
            (ReadQuery::Terrain(region, _), ReadReply::Terrain(page)) => match page {
                Some(page)
                    if page.region == *region
                        && page.patches.len() <= MAX_TERRAIN_PAGE_PATCHES
                        && page.dimensions == [256, 256]
                        && page.known_patches <= 256
                        && page.water_height.is_none_or(f32::is_finite) =>
                {
                    if !self.state.mirror.regions.contains_key(region) {
                        return Err(RecoveryReason::InvalidReply);
                    }
                    for patch in page.patches {
                        let key = (*region, patch.id());
                        if self.state.mirror.patches.get(&key).is_some_and(|old| old.source() == &patch) {
                            continue;
                        }
                        if !self.state.mirror.patches.contains_key(&key) && self.state.mirror.patches.len() >= MAX_PATCHES {
                            return Err(RecoveryReason::Capacity);
                        }
                        let old = usize::from(self.state.mirror.patches.contains_key(&key)) * PATCH_BYTES;
                        self.retain(PATCH_BYTES, old)?;
                        let charge = lease.split(PATCH_BYTES).ok_or(RecoveryReason::Capacity)?;
                        self.state.mirror.patches.insert(key, Patch::new(patch, charge));
                        self.preparation.surfaces.patch(&self.state.mirror, *region, key.1);
                        self.publication.dirty.patches.insert(key);
                    }
                    if self.state.mirror.water.get(region).copied() != Some(page.water_height) {
                        self.state.mirror.water.insert(*region, page.water_height);
                        if page.water_height.is_some() || self.state.mirror.surfaces.contains_key(&(*region, SurfaceKind::Water)) {
                            self.preparation.surfaces.water(*region);
                        }
                        self.publication.dirty.water.insert(*region);
                    }
                    self.world.terrain_cursor = page.next.map(|cursor| (*region, Some(cursor)));
                    self.preparation.surfaces.hydrating(*region, page.next.is_some());
                }
                None => {
                    self.retire(*region);
                    self.world.terrain_cursor = None;
                }
                _ => return Err(RecoveryReason::InvalidReply),
            },
            _ => return Err(RecoveryReason::InvalidReply),
        }
        if self.world.pending_set.len() > MAX_PENDING
            || self.publication.dirty.removed.len() > MAX_OBJECTS
            || self.publication.dirty.retired.len() > MAX_REGIONS
            || self.publication.dirty.regions.len() > MAX_REGIONS
            || self.publication.dirty.water.len() > MAX_REGIONS
        {
            return Err(RecoveryReason::Capacity);
        }
        Ok(())
    }

    fn apply_world_change(&mut self, change: WorldChange, revision: WorldRevision) -> Result<(), RecoveryReason> {
        match change {
            WorldChange::EntityAdded { entity, address, .. } => {
                self.preparation.expansion.enqueue_address(address);
                self.invalidate(entity);
            }
            WorldChange::EntityUpdated { entity, .. } => self.invalidate(entity),
            WorldChange::SubtreeInvalidated { entity, .. } => self.expand(entity),
            WorldChange::EntityRemoved { entity, .. } => {
                self.expand(entity);
                self.remove(entity);
            }
            WorldChange::EntityRebound { entity, to, .. } | WorldChange::EntityTransferred { entity, to, .. } => {
                self.preparation.expansion.enqueue_address(to);
                self.expand(entity);
            }
            WorldChange::RegionAdopted(region) => {
                self.world.refresh_regions = true;
                self.world.terrain.insert(region);
            }
            WorldChange::RegionUpdated(_) => {
                self.world.refresh_regions = true;
            }
            WorldChange::EnvironmentUpdated(region) => {
                self.world.environments.insert(region);
            }
            WorldChange::TerrainPatchUpdated { region, .. } | WorldChange::WaterHeightUpdated(region) => {
                self.world.terrain.insert(region);
            }
            WorldChange::RegionRetired { region, .. } => {
                let removed: Vec<_> = self
                    .state
                    .mirror
                    .objects
                    .iter()
                    .filter_map(|(entity, object)| {
                        let detail = object.detail();
                        (detail.summary.address.region() == region && detail.hydration_revision <= revision).then_some(*entity)
                    })
                    .collect();
                for entity in removed {
                    self.remove(entity);
                }
                self.retire(region);
                self.conservative_scan();
            }
        }
        if self.world.pending_set.len() > MAX_PENDING
            || self.world.terrain.len() > MAX_REGIONS
            || self.world.environments.len() > MAX_REGIONS
        {
            return Err(RecoveryReason::Capacity);
        }
        Ok(())
    }

    pub(super) fn invalidate(&mut self, entity: Entity) {
        if self.world.pending_set.insert(entity) {
            self.world.pending.push_back(entity);
        }
    }

    fn expand(&mut self, entity: Entity) {
        if self.state.dependencies.is_complete() {
            self.preparation.expansion.enqueue_entity(entity);
        } else {
            self.conservative_scan();
            self.invalidate(entity);
        }
    }

    fn conservative_scan(&mut self) {
        if self.preparation.scan.is_empty() {
            self.preparation.scan.extend(self.state.mirror.objects.keys().copied());
        }
    }

    fn remove(&mut self, entity: Entity) {
        self.preparation.meshes.remove(entity);
        self.preparation.assets.remove(entity);
        if let Some(old) = self.state.mirror.objects.remove(&entity) {
            self.state.identities.remove(&old.detail().summary.id);
            self.state.bindings.remove(&old.detail().summary.address);
            self.state.bytes -= object_bytes(&old);
            self.state.dependencies.remove(entity);
            self.publication.dirty.objects.remove(&entity);
            self.publication.dirty.removed.insert(entity);
        }
    }

    fn retire(&mut self, region: Region) {
        if let Some(old) = self.state.mirror.regions.remove(&region) {
            self.state.bytes -= region_bytes(&old);
        }
        if let Some(old) = self.state.mirror.environments.remove(&region) {
            self.state.bytes -= old.source().retained_bytes() + 32;
        }
        self.world.environments.remove(&region);
        self.publication.dirty.environments.remove(&region);
        self.state.mirror.patches.retain(|(r, _), _| {
            if *r == region {
                self.state.bytes -= PATCH_BYTES;
                false
            } else {
                true
            }
        });
        self.state.mirror.water.remove(&region);
        self.state.mirror.surfaces.retain(|(r, _), _| *r != region);
        self.preparation.surfaces.retire(region);
        self.preparation.surfaces.regions(&self.state.mirror);
        self.world.terrain.remove(&region);
        self.publication.dirty.regions.remove(&region);
        self.publication.dirty.water.remove(&region);
        self.publication.dirty.patches.retain(|(r, _)| *r != region);
        self.publication.dirty.surfaces.retain(|(r, _)| *r != region);
        self.publication.dirty.retired.insert(region);
    }

    fn retain(&mut self, new: usize, old: usize) -> Result<(), RecoveryReason> {
        let total =
            (self.state.bytes - old).checked_add(new).filter(|bytes| *bytes <= MAX_RETAINED_BYTES).ok_or(RecoveryReason::Capacity)?;
        self.state.bytes = total;
        Ok(())
    }
}

fn object_bytes(object: &Object) -> usize {
    object.detail().owned_bytes() + 32
}

fn region_bytes(region: &RegionData) -> usize {
    size_of::<RegionSummary>() + region.summary().info.name.as_ref().map_or(0, String::capacity) + 32
}
