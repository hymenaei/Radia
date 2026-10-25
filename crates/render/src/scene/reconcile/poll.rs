use super::{
    Change, DeliveryState, Duration, Instant, MAX_OBJECT_DETAIL_BATCH_ITEMS, MeshChange, MeshJob, POLL_DELAY, READ_QUANTUM, ReadQuery,
    ReconcileStep, Reconciler, RecoveryReason, Status, SurfaceStep, WORK, Work,
};
use crate::scene::region_environment::EnvironmentStart;

impl Reconciler {
    pub fn poll(&mut self, now: Instant) -> ReconcileStep {
        if let Some(event) = self.preparation.environments.event() {
            return ReconcileStep::Asset(event);
        }
        if let Some(event) = self.preparation.environment_textures.event() {
            return ReconcileStep::Asset(event);
        }
        if !self.session.is_active() {
            return ReconcileStep::Closed;
        }
        let viewer_version = self.viewer.version();
        self.viewer.advance();
        if viewer_version != self.viewer.version() {
            self.sync_environment_textures();
        }
        if let Some(event) = self.viewer.event() {
            return ReconcileStep::Asset(event);
        }
        if let Some(event) = self.preparation.assets.event() {
            return ReconcileStep::Asset(event);
        }
        self.preparation.environments.retry_ready(now);
        if let Some(start) = self.preparation.environments.start(self.world.epoch, self.session.clone()) {
            return match start {
                EnvironmentStart::Asset(event) => ReconcileStep::Asset(event),
                EnvironmentStart::Inline(job) => ReconcileStep::Build(MeshJob(Work::Environment(job))),
            };
        }
        if self.world.reading.as_ref().is_some_and(|reading| reading.upgrade().is_none()) {
            self.world.reading = None;
            self.begin_recovery(RecoveryReason::ReadFailed, now);
        }
        let mut blocked = false;
        if let Some(publication) = &self.publication.pending {
            match publication.receipt.state() {
                DeliveryState::Pending => blocked = true,
                DeliveryState::Applied => {
                    self.publication.published_revision = publication.revision;
                    self.publication.published_status = publication.status;
                    self.publication.pending = None;
                }
                DeliveryState::Abandoned => {
                    self.publication.pending = None;
                    self.publication.sequence -= 1;
                    self.begin_recovery(RecoveryReason::InvalidReply, now);
                }
            }
        }
        if !blocked && self.publication.viewer_version != self.viewer.version() {
            self.publication.viewer_version = self.viewer.version();
            return ReconcileStep::Publish(self.batch_at(
                false,
                vec![Change::ViewerEnvironment(self.viewer.current())],
                self.publication.published_revision,
                self.publication.published_status,
            ));
        }
        if self.recovery.status_pending && !blocked {
            self.recovery.status_pending = false;
            return ReconcileStep::Publish(self.batch_at(false, Vec::new(), self.publication.published_revision, self.recovery.status));
        }
        if let Some(retry) = self.recovery.retry {
            if now < retry {
                return ReconcileStep::Wait(retry);
            }
            self.recovery.retry = None;
        }
        if self.world.reading.is_none()
            && self.world.needs_changes
            && self.world.reads_since_changes >= READ_QUANTUM
            && let Some(cursor) = self.world.cursor
        {
            return self.read(ReadQuery::Changes(cursor), now);
        }
        let surfaces_waiting = match self.preparation.surfaces.start(&self.state.mirror) {
            SurfaceStep::Build(job) => return ReconcileStep::Build(MeshJob(Work::Surface(Box::new(job)))),
            SurfaceStep::Remove(region, kind) => {
                self.state.mirror.apply(Change::RemoveSurface(region, kind));
                self.publication.dirty.surfaces.insert((region, kind));
                self.publication.publish = true;
                return ReconcileStep::Continue;
            }
            SurfaceStep::Waiting => true,
            SurfaceStep::Idle => false,
            SurfaceStep::Continue => return ReconcileStep::Continue,
        };
        if !blocked && self.recovery.startup && !self.publication.dirty.is_empty() {
            let changes = self.collect_changes();
            let replace = self.publication.replace;
            self.publication.replace = false;
            return ReconcileStep::Publish(self.batch_at(replace, changes, None, Status::Loading));
        }
        if self.world.reading.is_none() && self.world.barrier.is_some() && !self.world.refresh_regions {
            if self.world.terrain_cursor.is_none()
                && let Some(region) = self.world.terrain.pop_first()
            {
                self.world.terrain_cursor = Some((region, None));
            }
            if let Some((region, cursor)) = self.world.terrain_cursor {
                return self.read(ReadQuery::Terrain(region, cursor), now);
            }
            if let Some(region) = self.world.environments.pop_first() {
                return self.read(ReadQuery::Environment(region), now);
            }
        }
        if !blocked {
            let changes = self.preparation.meshes.step(&self.state.mirror, &self.state.identities);
            if !changes.is_empty() {
                for change in &changes {
                    self.state.mirror.apply_mesh(change);
                    let entity = match change {
                        MeshChange::Set(entity, ..) | MeshChange::Clear(entity) => entity,
                    };
                    if let Some(object) = self.state.mirror.objects.get(entity) {
                        self.preparation.assets.invalidate(object);
                    }
                }
                let changes = changes.into_iter().map(Change::Mesh).collect();
                return ReconcileStep::Publish(self.batch_at(
                    false,
                    changes,
                    self.publication.published_revision,
                    self.publication.published_status,
                ));
            }
            if let Some(job) = self.preparation.meshes.start() {
                return ReconcileStep::Build(job);
            }
            let changes = self.preparation.assets.step(&self.state.mirror, &self.state.identities);
            if !changes.is_empty() {
                let mut published = Vec::with_capacity(changes.len());
                for change in changes {
                    self.state.mirror.apply(Change::Material(change.clone()));
                    published.push(Change::Material(change));
                }
                return ReconcileStep::Publish(self.batch_at(
                    false,
                    published,
                    self.publication.published_revision,
                    self.publication.published_status,
                ));
            }
            if let Some(event) = self.preparation.assets.event() {
                return ReconcileStep::Asset(event);
            }
        }
        if self.world.reading.is_some() {
            return if !blocked && (self.preparation.meshes.has_work() || self.preparation.assets.has_work()) {
                ReconcileStep::Continue
            } else {
                ReconcileStep::Wait(now + POLL_DELAY)
            };
        }
        if self.world.barrier.is_none() {
            return self.read(ReadQuery::Barrier, now);
        }
        if self.world.refresh_regions {
            return self.read(ReadQuery::Regions(self.world.regions), now);
        }
        for _ in 0..WORK {
            if let Some(entity) = self.preparation.scan.pop_front() {
                self.invalidate(entity);
            } else if !self.preparation.expansion.is_empty() {
                if let Some(entity) = self.preparation.expansion.step(&self.state.dependencies) {
                    self.invalidate(entity);
                }
            } else {
                break;
            }
        }
        if !self.world.pending.is_empty() {
            let entities = self.world.pending.iter().take(MAX_OBJECT_DETAIL_BATCH_ITEMS).copied().collect();
            return self.read(ReadQuery::Details(entities), now);
        }
        if !self.preparation.scan.is_empty() || !self.preparation.expansion.is_empty() {
            return ReconcileStep::Continue;
        }
        self.preparation.expansion.clear();
        if self.world.enumerate_objects {
            return self.read(ReadQuery::Objects(self.world.objects), now);
        }
        if self.publication.publish && !blocked {
            self.publication.publish = false;
            self.recovery.status = Status::Ready;
            self.recovery.startup = false;
            self.recovery.failures = 0;
            let changes = self.collect_changes();
            let replace = self.publication.replace;
            self.publication.replace = false;
            return ReconcileStep::Publish(self.batch(replace, changes));
        }
        if self.world.needs_changes {
            return self.read(ReadQuery::Changes(self.world.cursor.expect("barrier before journal")), now);
        }
        if blocked {
            ReconcileStep::Wait(now + POLL_DELAY)
        } else if self.preparation.meshes.has_work() || self.preparation.assets.has_work() {
            ReconcileStep::Continue
        } else if let Some(retry_at) = self.preparation.environments.retry_at(now) {
            ReconcileStep::Wait(retry_at)
        } else if self.preparation.meshes.retry_admission()
            || self.preparation.assets.retry_admission()
            || self.preparation.environments.waiting()
            || self.viewer.waiting()
        {
            ReconcileStep::Wait(now + Duration::from_millis(200))
        } else if self.preparation.meshes.waiting() || surfaces_waiting {
            ReconcileStep::Wait(now + POLL_DELAY)
        } else {
            ReconcileStep::Idle
        }
    }
}
