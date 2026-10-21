use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    mem::{size_of, take},
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use base::UUID;
use world::{
    Entity, EnvironmentRead, LocalAddress, MAX_ENVIRONMENT_READ_BYTES, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_PAGE_ITEMS,
    MAX_REGION_PAGE_ITEMS, MAX_TERRAIN_PAGE_PATCHES, MAX_WORLD_CHANGE_PAGE_ITEMS, ObjectDetailBatch, ObjectDetailResult, ObjectListPage,
    ObjectPageCursor, Region, RegionPage, RegionPageCursor, RegionSummary, TerrainPage, TerrainPageCursor, TerrainPatch, TerrainPatchId,
    WorldChange, WorldChangeCursor, WorldChangePage, WorldReadBarrier, WorldReadError, WorldRevision,
};

use crate::{
    AssetChange, AssetTicket, Batch, Budget, Camera, EnvironmentPush, EnvironmentPushError, MeshCompletion, MeshJob, Object, Patch,
    RecoveryReason, RegionData, SceneView, Status, SurfaceKind,
    asset_demand::AssetDemand,
    budget::Lease,
    dependencies::{Dependencies, DependencyTraversal},
    environment::ViewerEnvironments,
    model::{Change, EnvironmentData, MeshChange, Mirror, SceneLifetime},
    planning::{Completion, MeshPlanner, Work},
    surfaces::{SurfaceJobs, SurfaceStep},
};

pub const MAX_OBJECTS: usize = 65_536;
pub const MAX_REGIONS: usize = 33;
pub const MAX_RETAINED_BYTES: usize = 96 * 1024 * 1024;
const MAX_PATCHES: usize = MAX_REGIONS * 256;
pub const MAX_BATCH_CHANGES: usize = 2 * MAX_OBJECTS + 2 * MAX_PATCHES + 5 * MAX_REGIONS + 1;
const MAX_PENDING: usize = MAX_OBJECTS + MAX_OBJECT_PAGE_ITEMS;
const READ_BYTES: usize = 1024 * 1024 + 16 * 1024;
const WORK: usize = 128;
const READ_QUANTUM: usize = 4;
const POLL_DELAY: Duration = Duration::from_millis(16);
const PATCH_BYTES: usize = size_of::<TerrainPatch>() + 256 * size_of::<f32>() + 32;

pub enum ReadQuery {
    Barrier,
    Objects(Option<ObjectPageCursor>),
    Regions(Option<RegionPageCursor>),
    Details(Vec<Entity>),
    Changes(WorldChangeCursor),
    Terrain(Region, Option<TerrainPageCursor>),
    Environment(Region),
}

pub struct ReadJob {
    session: SceneLifetime,
    epoch: u64,
    query: ReadQuery,
    lease: Lease,
    completed: Arc<AtomicU8>,
}

impl ReadJob {
    pub fn query(&self) -> &ReadQuery {
        &self.query
    }
}

impl Drop for ReadJob {
    fn drop(&mut self) {
        self.completed.store(1, Ordering::Release);
    }
}

pub enum ReadReply {
    Barrier(WorldReadBarrier),
    Objects(ObjectListPage),
    Regions(RegionPage),
    Details(ObjectDetailBatch),
    Changes(WorldChangePage),
    Terrain(Option<TerrainPage>),
    Environment(Option<EnvironmentRead>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadFailure {
    Busy,
    Closed,
    World(WorldReadError),
}

pub enum ReconcileStep {
    Asset(AssetChange),
    Read(ReadJob),
    Build(MeshJob),
    Publish(Batch),
    Continue,
    Idle,
    Wait(Instant),
    Closed,
}

#[derive(Default)]
struct DirtySet {
    objects: HashSet<Entity>,
    removed: HashSet<Entity>,
    regions: BTreeSet<Region>,
    environments: BTreeSet<Region>,
    retired: BTreeSet<Region>,
    patches: BTreeSet<(Region, TerrainPatchId)>,
    water: BTreeSet<Region>,
    surfaces: BTreeSet<(Region, SurfaceKind)>,
}

impl DirtySet {
    fn is_empty(&self) -> bool {
        self.objects.is_empty()
            && self.removed.is_empty()
            && self.regions.is_empty()
            && self.environments.is_empty()
            && self.retired.is_empty()
            && self.patches.is_empty()
            && self.water.is_empty()
            && self.surfaces.is_empty()
    }
}

struct PendingPublication {
    receipt: Arc<AtomicU8>,
    revision: Option<WorldRevision>,
    status: Status,
}

pub struct SceneReconciler {
    session: SceneLifetime,
    budget: Budget,
    world: WorldSync,
    state: SceneState,
    preparation: Preparation,
    publication: PublicationState,
    recovery: RecoveryState,
    viewer: ViewerEnvironments,
}

struct WorldSync {
    epoch: u64,
    barrier: Option<WorldReadBarrier>,
    objects: Option<ObjectPageCursor>,
    enumerate_objects: bool,
    regions: Option<RegionPageCursor>,
    refresh_regions: bool,
    terrain: BTreeSet<Region>,
    environments: BTreeSet<Region>,
    terrain_cursor: Option<(Region, Option<TerrainPageCursor>)>,
    cursor: Option<WorldChangeCursor>,
    revision: Option<WorldRevision>,
    needs_changes: bool,
    reads_since_changes: usize,
    reading: Option<Arc<AtomicU8>>,
    pending: VecDeque<Entity>,
    pending_set: HashSet<Entity>,
}

#[derive(Default)]
struct SceneState {
    mirror: Mirror,
    identities: BTreeMap<UUID, Entity>,
    bindings: HashMap<LocalAddress, Entity>,
    bytes: usize,
    dependencies: Dependencies,
}

struct Preparation {
    meshes: MeshPlanner,
    assets: AssetDemand,
    surfaces: SurfaceJobs,
    expansion: DependencyTraversal,
    scan: VecDeque<Entity>,
}

struct PublicationState {
    viewer_version: u64,
    sequence: u64,
    dirty: DirtySet,
    publish: bool,
    replace: bool,
    pending: Option<PendingPublication>,
    published_revision: Option<WorldRevision>,
    published_status: Status,
}

struct RecoveryState {
    startup: bool,
    status: Status,
    status_pending: bool,
    failures: u8,
    retry: Option<Instant>,
}

impl WorldSync {
    fn new(epoch: u64, revision: Option<WorldRevision>) -> Self {
        Self {
            epoch,
            barrier: None,
            objects: None,
            enumerate_objects: true,
            regions: None,
            refresh_regions: true,
            terrain: BTreeSet::new(),
            environments: BTreeSet::new(),
            terrain_cursor: None,
            cursor: None,
            revision,
            needs_changes: true,
            reads_since_changes: 0,
            reading: None,
            pending: VecDeque::new(),
            pending_set: HashSet::new(),
        }
    }

    fn restart(&mut self) {
        let epoch = self.epoch.checked_add(1).expect("Scene epoch space exhausted");
        *self = Self::new(epoch, self.revision);
    }
}

impl Preparation {
    fn reset(&mut self, budget: Budget) {
        self.meshes.reset(budget.clone());
        self.assets.reset();
        self.surfaces = SurfaceJobs::new(budget);
        self.expansion.clear();
        self.scan.clear();
    }
}

impl SceneReconciler {
    pub fn new(budget: Budget) -> (Self, SceneView) {
        let session = SceneLifetime::new();
        let view = SceneView::new(session.clone());
        let scene = Self {
            session,
            world: WorldSync::new(0, None),
            state: SceneState::default(),
            preparation: Preparation {
                meshes: MeshPlanner::new(budget.clone()),
                assets: AssetDemand::new(budget.clone()),
                surfaces: SurfaceJobs::new(budget.clone()),
                expansion: DependencyTraversal::default(),
                scan: VecDeque::new(),
            },
            viewer: ViewerEnvironments::new(budget.clone()),
            publication: PublicationState {
                viewer_version: 0,
                sequence: 0,
                dirty: DirtySet::default(),
                publish: false,
                replace: true,
                pending: None,
                published_revision: None,
                published_status: Status::Loading,
            },
            recovery: RecoveryState { startup: true, status: Status::Loading, status_pending: false, failures: 0, retry: None },
            budget,
        };
        (scene, view)
    }

    pub fn status(&self) -> Status {
        self.recovery.status
    }

    pub fn notify_world_change(&mut self) {
        self.world.needs_changes = true;
    }

    pub fn camera(&mut self, camera: Camera) {
        self.preparation.meshes.camera(camera);
        self.preparation.assets.camera(camera);
    }

    pub fn push_environment(&mut self, region: Region, push: EnvironmentPush) -> Result<(), EnvironmentPushError> {
        if !self.session.is_active() {
            return Err(EnvironmentPushError::Invalid);
        }
        self.viewer.push(region, push)
    }

    pub fn complete_asset(&mut self, ticket: AssetTicket, status: assets::Status) {
        if self.viewer.complete(ticket, &status) {
            return;
        }
        let changed = self.preparation.assets.affected_entities(ticket);
        self.preparation.assets.complete(ticket, status);
        for entity in changed {
            if let Some(object) = self.state.mirror.objects.get(&entity) {
                self.preparation.assets.invalidate(object);
            }
        }
    }

    pub fn complete_mesh(&mut self, completion: MeshCompletion) {
        match completion.0 {
            Completion::Prim(completion) => self.preparation.meshes.complete(completion),
            Completion::Surface(completion) => {
                if let Some(surface) = self.preparation.surfaces.complete(completion) {
                    self.publication.dirty.surfaces.insert((surface.region, surface.kind));
                    self.state.mirror.apply(Change::Surface(surface));
                    self.publication.publish = true;
                }
            }
        }
    }

    pub fn close(&mut self) {
        self.session.close();
        self.viewer = ViewerEnvironments::new(self.budget.clone());
        self.reset();
        self.recovery.status = Status::Closed;
    }

    pub fn poll(&mut self, now: Instant) -> ReconcileStep {
        if !self.session.is_active() {
            return ReconcileStep::Closed;
        }
        self.viewer.advance();
        if let Some(event) = self.viewer.event() {
            return ReconcileStep::Asset(event);
        }
        if let Some(event) = self.preparation.assets.event() {
            return ReconcileStep::Asset(event);
        }
        if self.world.reading.as_ref().is_some_and(|reading| reading.load(Ordering::Acquire) != 0) {
            self.world.reading = None;
            self.begin_recovery(RecoveryReason::ReadFailed, now);
        }
        let mut blocked = false;
        if let Some(publication) = &self.publication.pending {
            match publication.receipt.load(Ordering::Acquire) {
                0 => blocked = true,
                1 => {
                    self.publication.published_revision = publication.revision;
                    self.publication.published_status = publication.status;
                    self.publication.pending = None;
                }
                _ => {
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
        } else if self.preparation.meshes.retry_admission() || self.preparation.assets.retry_admission() || self.viewer.waiting() {
            ReconcileStep::Wait(now + Duration::from_millis(200))
        } else if self.preparation.meshes.waiting() || surfaces_waiting {
            ReconcileStep::Wait(now + POLL_DELAY)
        } else {
            ReconcileStep::Idle
        }
    }

    fn read(&mut self, query: ReadQuery, now: Instant) -> ReconcileStep {
        let bytes = if matches!(query, ReadQuery::Environment(_)) { MAX_ENVIRONMENT_READ_BYTES + 32 } else { READ_BYTES };
        let Some(lease) = self.budget.reserve(bytes) else {
            self.begin_recovery(RecoveryReason::Capacity, now);
            return ReconcileStep::Continue;
        };
        let completed = Arc::new(AtomicU8::new(0));
        if let ReadQuery::Terrain(region, _) = &query {
            self.preparation.surfaces.hydrating(*region, true);
        }
        self.world.reading = Some(Arc::clone(&completed));
        self.world.reads_since_changes =
            if matches!(query, ReadQuery::Changes(_) | ReadQuery::Barrier) { 0 } else { self.world.reads_since_changes.saturating_add(1) };
        ReconcileStep::Read(ReadJob { session: self.session.clone(), epoch: self.world.epoch, query, lease, completed })
    }

    pub fn complete(&mut self, mut job: ReadJob, reply: Result<ReadReply, ReadFailure>, now: Instant) {
        if !self.session.is_active()
            || !job.session.is_active()
            || job.epoch != self.world.epoch
            || !self.world.reading.as_ref().is_some_and(|reading| Arc::ptr_eq(reading, &job.completed))
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

    fn invalidate(&mut self, entity: Entity) {
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

    fn collect_changes(&mut self) -> Vec<Change> {
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
            changes
                .extend(self.state.mirror.environments.iter().map(|(region, source)| Change::Environment(*region, Some(source.clone()))));
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

    fn batch(&mut self, replace: bool, changes: Vec<Change>) -> Batch {
        self.batch_at(replace, changes, self.world.revision, self.recovery.status)
    }

    fn batch_at(&mut self, replace: bool, changes: Vec<Change>, revision: Option<WorldRevision>, status: Status) -> Batch {
        self.publication.sequence = self.publication.sequence.checked_add(1).expect("Scene sequence space exhausted");
        let receipt = Arc::new(AtomicU8::new(0));
        self.publication.pending = Some(PendingPublication { receipt: Arc::clone(&receipt), revision, status });
        Batch { session: self.session.clone(), sequence: self.publication.sequence, revision, status, replace, changes, receipt }
    }

    fn reset(&mut self) {
        self.preparation.reset(self.budget.clone());
        self.world.restart();
        self.state = SceneState::default();
        self.publication.dirty = DirtySet::default();
        self.publication.publish = false;
        self.publication.replace = true;
        self.recovery.startup = false;
    }

    fn begin_recovery(&mut self, reason: RecoveryReason, now: Instant) {
        self.reset();
        self.recovery.failures = self.recovery.failures.saturating_add(1).min(4);
        let delay = if self.recovery.failures <= 3 {
            self.recovery.status = Status::Recovering { reason, attempt: self.recovery.failures };
            Duration::from_secs(1 << (self.recovery.failures - 1))
        } else {
            self.recovery.status = Status::Degraded { reason };
            Duration::from_secs(30)
        };
        self.recovery.retry = Some(now + delay);
        self.recovery.status_pending = true;
    }
}

impl Drop for SceneReconciler {
    fn drop(&mut self) {
        self.session.close();
    }
}

fn object_bytes(object: &Object) -> usize {
    object.detail().owned_bytes() + 32
}
fn region_bytes(region: &RegionData) -> usize {
    size_of::<RegionSummary>() + region.summary().info.name.as_ref().map_or(0, String::capacity) + 32
}
