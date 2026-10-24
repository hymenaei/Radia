mod poll;
mod publication;
mod read;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use uuid::Uuid;
use world::{
    Entity, EnvironmentRead, LocalAddress, MAX_OBJECT_DETAIL_BATCH_ITEMS, MAX_OBJECT_PAGE_ITEMS, ObjectDetailBatch, ObjectListPage,
    ObjectPageCursor, Region, RegionPage, RegionPageCursor, RegionSummary, TerrainPage, TerrainPageCursor, TerrainPatch, TerrainPatchId,
    WorldChangeCursor, WorldChangePage, WorldReadBarrier, WorldReadError, WorldRevision,
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
    completed: Arc<AtomicBool>,
}

impl ReadJob {
    pub fn query(&self) -> &ReadQuery {
        &self.query
    }
}

impl Drop for ReadJob {
    fn drop(&mut self) {
        self.completed.store(true, Ordering::Release);
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

pub struct Reconciler {
    session: SceneLifetime,
    budget: Budget,
    world: WorldSync,
    state: SceneState,
    preparation: Preparation,
    publication: PublicationState,
    recovery: RecoveryState,
    viewer: ViewerEnvironments,
}

impl Reconciler {
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

    pub fn set_camera(&mut self, camera: Camera) {
        self.preparation.meshes.set_camera(camera);
        self.preparation.assets.set_camera(camera);
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

impl Drop for Reconciler {
    fn drop(&mut self) {
        self.session.close();
    }
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
    reading: Option<Arc<AtomicBool>>,
    pending: VecDeque<Entity>,
    pending_set: HashSet<Entity>,
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

#[derive(Default)]
struct SceneState {
    mirror: Mirror,
    identities: BTreeMap<Uuid, Entity>,
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

impl Preparation {
    fn reset(&mut self, budget: Budget) {
        self.meshes.reset(budget.clone());
        self.assets.reset();
        self.surfaces = SurfaceJobs::new(budget);
        self.expansion.clear();
        self.scan.clear();
    }
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
