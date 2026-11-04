use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet, BinaryHeap},
    fmt,
    ops::Range,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::watch;
use utils::AssetId;

use crate::{
    Blob, Budget, Content, TextureEvents, TextureHandle, TextureSize, WeakTexture,
    budget::{Reservation, TEXTURE_WORK_BYTES, WORK_BYTES},
    cache::{AssetCache, AssetLookup, CacheUnit, CacheWrite, cache_unit, read_work_bytes_for_request},
    content::{Finished, Prepared, StoredContent},
    mesh::{MeshLod, MeshLodRequest},
};

pub const MAX_ASSET_ROUTES: usize = 33;
pub const MAX_TEXTURE_DECODES: usize = 2;
const MAX_NON_TEXTURE_DECODES: usize = 2;
pub const MAX_NON_MESH_FETCHES: usize = 8;
const MAX_MESH_FETCHES: usize = 32;
const DEMAND_DEADLINE: Duration = Duration::from_secs(60);
const NEGATIVE_TTL: Duration = Duration::from_secs(30);
const CACHE_PRESSURE_RETRY: Duration = Duration::from_secs(1);
const MAX_ATTEMPTS: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ScheduleKey {
    priority: Reverse<Priority>,
    importance: Reverse<u16>,
    ready: bool,
    used: Instant,
    key: LoadKey,
    route: Route,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    Texture,
    PbrMaterial,
    LegacyMaterial,
    Environment,
    Mesh,
}

impl Kind {
    const fn work_bytes(self) -> Option<usize> {
        match self {
            Self::Texture => Some(TEXTURE_WORK_BYTES),
            Self::Mesh => None,
            _ => Some(WORK_BYTES),
        }
    }

    pub const fn max_response_bytes(self) -> usize {
        match self {
            Self::Texture => 32 * 1024 * 1024,
            Self::Mesh => 32 * 1024 * 1024,
            Self::PbrMaterial | Self::LegacyMaterial | Self::Environment => 2 * 1024 * 1024,
        }
    }

    pub(crate) const fn max_cache_bytes(self) -> usize {
        match self {
            Self::LegacyMaterial => 4 * 1024 * 1024,
            _ => self.max_response_bytes(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    Background,
    Visible,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Resource {
    pub id: AssetId,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    Texture { id: AssetId, size: TextureSize },
    PbrMaterial(AssetId),
    LegacyMaterial(AssetId),
    Environment(AssetId),
    MeshHeader(AssetId),
    MeshLod(MeshLodRequest),
}

impl Request {
    pub fn new(resource: Resource) -> Self {
        match resource.kind {
            Kind::Texture => Self::Texture { id: resource.id, size: TextureSize::default() },
            Kind::PbrMaterial => Self::PbrMaterial(resource.id),
            Kind::LegacyMaterial => Self::LegacyMaterial(resource.id),
            Kind::Environment => Self::Environment(resource.id),
            Kind::Mesh => Self::MeshHeader(resource.id),
        }
    }
    pub fn resource(self) -> Resource {
        match self {
            Self::Texture { id, .. } => Resource { id, kind: Kind::Texture },
            Self::PbrMaterial(id) => Resource { id, kind: Kind::PbrMaterial },
            Self::LegacyMaterial(id) => Resource { id, kind: Kind::LegacyMaterial },
            Self::Environment(id) => Resource { id, kind: Kind::Environment },
            Self::MeshHeader(id) => Resource { id, kind: Kind::Mesh },
            Self::MeshLod(block) => Resource { id: block.asset_id(), kind: Kind::Mesh },
        }
    }

    fn unit(self) -> CacheUnit {
        cache_unit(self)
    }

    fn size(self) -> Option<TextureSize> {
        match self {
            Self::Texture { size, .. } => Some(size),
            _ => None,
        }
    }

    fn is_compatible_with(self, other: Self) -> bool {
        match (self, other) {
            (Self::Texture { id: left, .. }, Self::Texture { id: right, .. }) => left == right,
            _ => self == other,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
struct LoadKey {
    resource: Resource,
    unit: CacheUnit,
}

#[derive(Debug)]
pub enum FetchResponse {
    Full(Vec<u8>),
    Partial { bytes: Vec<u8>, range: Range<u64>, total_length: Option<u64> },
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Route {
    store: u64,
    index: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Failure {
    MissingCapability,
    NotFound,
    Denied,
    HttpStatus { status: u16 },
    Network,
    MemoryPressure,
    TooLarge,
    Malformed,
    Unsupported,
    MissingLod(MeshLod),
    MalformedBlock,
    UnsupportedVersion(u32),
    RouteClosed,
    SessionClosed,
    Cancelled,
    Deadline,
    Transient { retry_after: Duration },
    FullAssetRequired,
}

#[derive(Clone, Debug)]
pub enum Status<C = Content> {
    Queued,
    Fetching,
    Decoding,
    Ready(C),
    Refining(C),
    RefinementFailed { content: C, failure: Failure },
    Failed { failure: Failure, retry_at: Option<Instant> },
    StaleSession,
}

impl<C> Status<C> {
    pub fn content(&self) -> Option<&C> {
        match self {
            Self::Ready(content) | Self::Refining(content) | Self::RefinementFailed { content, .. } => Some(content),
            _ => None,
        }
    }

    fn map<T>(self, content: impl FnOnce(C) -> T) -> Status<T> {
        match self {
            Self::Queued => Status::Queued,
            Self::Fetching => Status::Fetching,
            Self::Decoding => Status::Decoding,
            Self::Ready(value) => Status::Ready(content(value)),
            Self::Refining(value) => Status::Refining(content(value)),
            Self::RefinementFailed { content: value, failure } => Status::RefinementFailed { content: content(value), failure },
            Self::Failed { failure, retry_at } => Status::Failed { failure, retry_at },
            Self::StaleSession => Status::StaleSession,
        }
    }
}

impl Status<StoredContent> {
    fn upgrade(&self) -> Status {
        match self {
            Self::Ready(content) => content.upgrade().map_or(Status::Queued, Status::Ready),
            Self::Refining(content) => content.upgrade().map_or(Status::Queued, Status::Refining),
            Self::RefinementFailed { content, failure } => {
                content.upgrade().map_or(Status::Failed { failure: *failure, retry_at: None }, |content| Status::RefinementFailed {
                    content,
                    failure: *failure,
                })
            }
            Self::Queued => Status::Queued,
            Self::Fetching => Status::Fetching,
            Self::Decoding => Status::Decoding,
            Self::Failed { failure, retry_at } => Status::Failed { failure: *failure, retry_at: *retry_at },
            Self::StaleSession => Status::StaleSession,
        }
    }
}

impl<C: Clone> Status<C> {
    pub fn retaining(self, ready: Option<&C>) -> Self {
        match (ready, &self) {
            (Some(content), Self::Fetching | Self::Decoding | Self::Queued) => Self::Refining(content.clone()),
            (Some(content), Self::Failed { failure, .. }) => Self::RefinementFailed { content: content.clone(), failure: *failure },
            _ => self,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    Closed,
    Route,
    InvalidId,
    CacheProtection,
    ConflictingRequest,
}

struct Counter {
    count: AtomicUsize,
    wake: watch::Sender<()>,
}

struct Lease {
    counter: Arc<Counter>,
    maintenance: Arc<AtomicBool>,
    size: AtomicU16,
    priority: AtomicBool,
    importance: AtomicU16,
    _cache_protection: Option<storage::Protection>,
    _texture: Option<TextureHandle>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.counter.count.fetch_sub(1, Ordering::AcqRel);
        self.counter.wake.send_replace(());
        self.maintenance.store(true, Ordering::Release);
    }
}

impl Lease {
    fn size(&self) -> Option<TextureSize> {
        let edge = self.size.load(Ordering::Acquire);
        (edge != 0).then(|| TextureSize::new(u32::from(edge)))
    }

    fn priority(&self) -> Priority {
        if self.priority.load(Ordering::Acquire) { Priority::Visible } else { Priority::Background }
    }
}

#[derive(Clone)]
pub struct Demand {
    _lease: Arc<Lease>,
    status: watch::Receiver<Status<StoredContent>>,
}

impl Demand {
    pub fn status(&self) -> Status {
        self.status.borrow().upgrade()
    }

    pub async fn changed(&mut self) -> Status {
        if self.status.changed().await.is_err() {
            return self.status();
        }
        self.status.borrow_and_update().upgrade()
    }

    pub fn update_importance(&self, importance: u16) {
        self._lease.importance.store(importance, Ordering::Release);
        self._lease.maintenance.store(true, Ordering::Release);
        self._lease.counter.wake.send_replace(());
    }

    pub fn update_priority(&self, priority: Priority) {
        self._lease.priority.store(priority == Priority::Visible, Ordering::Release);
        self._lease.maintenance.store(true, Ordering::Release);
        self._lease.counter.wake.send_replace(());
    }

    pub fn update_size(&self, size: TextureSize) {
        let size = size.edge() as u16;
        if self._lease.size.try_update(Ordering::AcqRel, Ordering::Acquire, |current| (current != 0).then_some(size)).is_ok() {
            self._lease.maintenance.store(true, Ordering::Release);
            self._lease.counter.wake.send_replace(());
        }
    }
}

struct FailureRecord {
    failure: Failure,
    retry_at: Option<Instant>,
    expires: Instant,
}

struct RouteState {
    counter: Arc<Counter>,
    status: watch::Sender<Status<StoredContent>>,
    attempts: u8,
    failed: Option<FailureRecord>,
    deadline: Instant,
    leases: Vec<Weak<Lease>>,
}

impl RouteState {
    fn count(&self) -> usize {
        self.counter.count.load(Ordering::Acquire)
    }

    fn restart(&mut self, now: Instant) {
        self.failed = None;
        self.attempts = 0;
        self.deadline = now + DEMAND_DEADLINE;
    }

    fn queued_status(&self) -> Status {
        self.failed.as_ref().map_or(Status::Queued, |failed| Status::Failed { failure: failed.failure, retry_at: failed.retry_at })
    }

    fn fail(&mut self, failure: Failure, retry_at: Option<Instant>, now: Instant, ready: Option<&Content>) {
        self.failed = Some(FailureRecord { failure, retry_at, expires: now + NEGATIVE_TTL });
        self.publish_status(Status::Failed { failure, retry_at }.retaining(ready));
    }

    fn publish_status(&self, status: Status) {
        self.status.send_replace(status.map(|content| content.downgrade()));
    }
}

struct WorkLife {
    maintenance: Arc<AtomicBool>,
}

impl Drop for WorkLife {
    fn drop(&mut self) {
        self.maintenance.store(true, Ordering::Release);
    }
}

enum LoadPhase {
    Idle,
    Queued,
    Running { generation: u64, route: Route, life: Weak<WorkLife>, cancelled: watch::Sender<bool>, stage: Stage },
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Stage {
    ReadingCache,
    Fetching,
    Cached,
    Decoding,
}

struct AssetEntry {
    request: Request,
    views: BTreeMap<Route, RouteState>,
    phase: LoadPhase,
    used: Instant,
    ready: Option<StoredContent>,
    texture: Option<WeakTexture>,
    target: Option<TextureSize>,
    priority: Priority,
    importance: u16,
    cache_checked: bool,
    cache_retry_at: Option<Instant>,
    force_full_mesh: bool,
}

impl AssetEntry {
    fn ready(&self) -> Option<Content> {
        self.ready.as_ref().and_then(StoredContent::upgrade)
    }
    fn texture(&self) -> Option<TextureHandle> {
        self.texture.as_ref().and_then(WeakTexture::upgrade)
    }
    fn effective_priority(&self) -> Priority {
        self.views
            .values()
            .flat_map(|view| &view.leases)
            .filter_map(Weak::upgrade)
            .map(|lease| lease.priority())
            .max()
            .unwrap_or(Priority::Background)
    }
    fn requested_size(&self) -> Option<TextureSize> {
        self.views.values().flat_map(|view| &view.leases).filter_map(Weak::upgrade).filter_map(|lease| lease.size()).max()
    }

    fn has_active_demand(&self) -> bool {
        self.views.values().any(|view| view.count() > 0)
    }
    fn broadcast_status(&self, status: Status) {
        let ready = self.ready();
        for view in self.views.values().filter(|view| view.count() > 0) {
            view.publish_status(status.clone().retaining(ready.as_ref()));
        }
    }
}

struct Job {
    resource: Resource,
    key: LoadKey,
    request: Request,
    route: Route,
    generation: u64,
    _life: Arc<WorkLife>,
    reservation: Reservation,
    cancelled: watch::Receiver<bool>,
    size: TextureSize,
    force_full_mesh: bool,
}

impl fmt::Debug for Job {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Job").field("resource", &self.resource).field("route", &self.route).finish_non_exhaustive()
    }
}

impl Job {
    fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }
    fn cancelled(&self) -> impl Future<Output = ()> + Send + use<> {
        let mut cancelled = self.cancelled.clone();
        async move {
            if *cancelled.borrow() {
                return;
            }
            cancelled.changed().await.unwrap_or(());
        }
    }

    fn fail(self, failure: Failure) -> LoadCompletion {
        let failure = if self.is_cancelled() { Failure::Cancelled } else { failure };
        LoadCompletion { job: self, result: Err(failure), origin: DecodeOrigin::Network, decode_duration: None }
    }
}

#[derive(Debug)]
pub struct FetchFailure {
    job: Box<Job>,
    failure: Failure,
}

impl FetchFailure {
    fn new(job: Job, failure: Failure) -> Self {
        let failure = if job.is_cancelled() { Failure::Cancelled } else { failure };
        Self { job: Box::new(job), failure }
    }

    pub fn failure(&self) -> Failure {
        self.failure
    }
}

impl From<FetchFailure> for LoadCompletion {
    fn from(fetch: FetchFailure) -> Self {
        Self { job: *fetch.job, result: Err(fetch.failure), origin: DecodeOrigin::Network, decode_duration: None }
    }
}

#[derive(Debug)]
pub struct FetchJob {
    job: Job,
}

impl FetchJob {
    pub fn resource(&self) -> Resource {
        self.job.resource
    }
    pub fn request(&self) -> Request {
        self.job.request
    }
    pub fn route(&self) -> Route {
        self.job.route
    }
    pub fn response_limit(&self) -> usize {
        self.job.reservation.bytes()
    }
    pub fn force_full_mesh(&self) -> bool {
        self.job.force_full_mesh
    }
    pub fn is_cancelled(&self) -> bool {
        self.job.is_cancelled()
    }
    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + use<> {
        self.job.cancelled()
    }
    pub fn complete(self, response: Result<Vec<u8>, Failure>) -> Result<DecodeInput, FetchFailure> {
        self.complete_response(response.map(FetchResponse::Full))
    }

    pub fn complete_response(self, response: Result<FetchResponse, Failure>) -> Result<DecodeInput, FetchFailure> {
        let Self { mut job } = self;
        let response = match response {
            Ok(response) if !job.is_cancelled() => response,
            Ok(_) => return Err(FetchFailure::new(job, Failure::Cancelled)),
            Err(failure) => return Err(FetchFailure::new(job, failure)),
        };
        let (length, capacity) = match &response {
            FetchResponse::Full(bytes) | FetchResponse::Partial { bytes, .. } => (bytes.len(), bytes.capacity()),
        };
        if length > job.reservation.bytes() || capacity > job.resource.kind.max_response_bytes() {
            return Err(FetchFailure::new(job, Failure::TooLarge));
        }
        if !job.reservation.resize(capacity) {
            return Err(FetchFailure::new(job, Failure::Transient { retry_after: Duration::from_secs(1) }));
        }
        Ok(DecodeInput { job, source: DecodeSource::Response(response) })
    }
}

#[derive(Debug)]
pub struct CacheReadJob {
    job: Job,
    cache: Arc<AssetCache>,
}

pub struct CacheReadCompletion {
    job: Job,
    result: Result<AssetLookup, Failure>,
}

impl CacheReadJob {
    pub fn run(self) -> CacheReadCompletion {
        self.run_lookup(|cache, request| cache.lookup(request))
    }

    fn run_lookup(self, lookup: impl FnOnce(&AssetCache, Request) -> AssetLookup) -> CacheReadCompletion {
        let result = if self.job.is_cancelled() {
            Err(Failure::Cancelled)
        } else {
            let lookup = lookup(&self.cache, self.job.request);
            if self.job.is_cancelled() { Err(Failure::Cancelled) } else { Ok(lookup) }
        };
        CacheReadCompletion { job: self.job, result }
    }
}

#[derive(Debug)]
pub enum LoadJob {
    Fetch(FetchJob),
    Decode(DecodeInput),
    ReadCache(CacheReadJob),
}

impl LoadJob {
    pub fn fail(self, failure: Failure) -> LoadCompletion {
        match self {
            Self::Fetch(job) => job.job.fail(failure),
            Self::Decode(input) => input.fail(failure),
            Self::ReadCache(job) => {
                LoadCompletion { job: job.job, result: Err(failure), origin: DecodeOrigin::Disk, decode_duration: None }
            }
        }
    }
}

enum DecodeSource {
    Response(FetchResponse),
    Disk(Vec<u8>),
    Cached(Blob),
}

pub struct DecodeInput {
    source: DecodeSource,
    job: Job,
}

impl fmt::Debug for DecodeInput {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let source = match &self.source {
            DecodeSource::Response(_) => "response",
            DecodeSource::Disk(_) => "disk",
            DecodeSource::Cached(_) => "cached",
        };
        f.debug_struct("DecodeInput")
            .field("resource", &self.job.resource)
            .field("route", &self.job.route)
            .field("source", &source)
            .finish()
    }
}

impl DecodeInput {
    fn origin(&self) -> DecodeOrigin {
        match &self.source {
            DecodeSource::Response(_) => DecodeOrigin::Network,
            DecodeSource::Disk(_) => DecodeOrigin::Disk,
            DecodeSource::Cached(_) => DecodeOrigin::Memory,
        }
    }

    pub fn resource(&self) -> Resource {
        self.job.resource
    }
    pub fn is_cancelled(&self) -> bool {
        self.job.is_cancelled()
    }
    pub fn fail(self, failure: Failure) -> LoadCompletion {
        let failure = if self.job.is_cancelled() { Failure::Cancelled } else { failure };
        let origin = self.origin();
        LoadCompletion { job: self.job, result: Err(failure), origin, decode_duration: None }
    }
}

#[derive(Debug)]
pub struct DecodeJob(DecodeInput);

impl DecodeJob {
    pub fn fail(self, failure: Failure) -> LoadCompletion {
        self.0.fail(failure)
    }

    pub fn run(self) -> LoadCompletion {
        let DecodeInput { mut job, source, .. } = self.0;
        let cancelled = &job.cancelled;
        let started = Instant::now();
        let (result, origin) = match source {
            DecodeSource::Response(response) => (
                crate::content::prepare_cancellable(job.request, job.size, response, &mut job.reservation, || *cancelled.borrow()),
                DecodeOrigin::Network,
            ),
            DecodeSource::Disk(bytes) => (
                crate::content::prepare_disk(job.request, job.size, bytes, &mut job.reservation, || *cancelled.borrow()),
                DecodeOrigin::Disk,
            ),
            DecodeSource::Cached(source) => {
                (crate::content::prepare_cached(job.size, &source, || *cancelled.borrow()), DecodeOrigin::Memory)
            }
        };
        LoadCompletion { job, result, origin, decode_duration: Some(started.elapsed()) }
    }

    pub fn resource(&self) -> Resource {
        self.0.resource()
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

pub struct LoadCompletion {
    result: Result<Prepared, Failure>,
    job: Job,
    origin: DecodeOrigin,
    decode_duration: Option<Duration>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DecodeOrigin {
    Network,
    Disk,
    Memory,
}

impl fmt::Debug for LoadCompletion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadCompletion")
            .field("resource", &self.job.resource)
            .field("route", &self.job.route)
            .field("failure", &self.result.as_ref().err())
            .finish()
    }
}

impl LoadCompletion {
    pub fn failure(&self) -> Option<Failure> {
        self.result.as_ref().err().copied()
    }
}

pub struct Store {
    id: u64,
    sequence: u64,
    routes: BTreeSet<Route>,
    entries: BTreeMap<LoadKey, AssetEntry>,
    candidates: Vec<ScheduleKey>,
    mesh_candidate_heap: BinaryHeap<ScheduleKey>,
    other_candidate_heap: BinaryHeap<ScheduleKey>,
    candidates_dirty: bool,
    budget: Budget,
    closed: bool,
    maintenance_dirty: Arc<AtomicBool>,
    maintained_at: Option<Instant>,
    running_mesh_fetches: usize,
    running_other_fetches: usize,
    running_cache_reads: usize,
    running_non_mesh_cache_reads: usize,
    pending_non_mesh_cache_decodes: usize,
    running_texture_decodes: usize,
    running_other_decodes: usize,
    textures: TextureEvents,
    cache: Option<Arc<AssetCache>>,
}

impl Drop for Store {
    fn drop(&mut self) {
        self.close();
    }
}

impl Store {
    pub fn texture_events(&self) -> TextureEvents {
        self.textures.clone()
    }

    pub fn new(budget: Budget) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("asset session generation exhausted");
        Self {
            id,
            sequence: 0,
            routes: BTreeSet::new(),
            entries: BTreeMap::new(),
            candidates: Vec::with_capacity(MAX_NON_MESH_FETCHES + MAX_MESH_FETCHES),
            mesh_candidate_heap: BinaryHeap::with_capacity(MAX_MESH_FETCHES),
            other_candidate_heap: BinaryHeap::with_capacity(MAX_NON_MESH_FETCHES),
            candidates_dirty: true,
            budget,
            closed: false,
            maintenance_dirty: Arc::new(AtomicBool::new(true)),
            maintained_at: None,
            running_mesh_fetches: 0,
            running_other_fetches: 0,
            running_cache_reads: 0,
            running_non_mesh_cache_reads: 0,
            pending_non_mesh_cache_decodes: 0,
            running_texture_decodes: 0,
            running_other_decodes: 0,
            textures: TextureEvents::default(),
            cache: None,
        }
    }

    pub fn with_cache(budget: Budget, cache: Arc<AssetCache>) -> Self {
        let mut store = Self::new(budget);
        store.cache = Some(cache);
        store
    }

    pub fn wake(&self) -> watch::Receiver<()> {
        self.budget.wake().subscribe()
    }

    pub fn priority(&self, input: &DecodeInput) -> Priority {
        self.entries.get(&input.job.key).map_or(Priority::Background, AssetEntry::effective_priority)
    }

    pub fn register_route(&mut self) -> Result<Route, AdmissionError> {
        if self.closed {
            return Err(AdmissionError::Closed);
        }
        if self.routes.len() >= MAX_ASSET_ROUTES {
            return Err(AdmissionError::Route);
        }
        self.sequence = self.sequence.checked_add(1).ok_or(AdmissionError::Closed)?;
        let route = Route { store: self.id, index: self.sequence };
        self.routes.insert(route);
        Ok(route)
    }

    pub fn retire_route(&mut self, route: Route) {
        self.maintenance_dirty.store(true, Ordering::Release);
        self.routes.remove(&route);
        for entry in self.entries.values_mut() {
            if let Some(view) = entry.views.remove(&route) {
                view.publish_status(Status::Failed { failure: Failure::RouteClosed, retry_at: None });
            }
            if let LoadPhase::Running { route: running, cancelled, .. } = &entry.phase
                && *running == route
            {
                cancelled.send_replace(true);
            }
        }
    }

    pub fn request(&mut self, route: Route, request: Request, priority: Priority, now: Instant) -> Result<Demand, AdmissionError> {
        self.request_with_importance(route, request, priority, 0, now)
    }

    pub fn request_with_importance(
        &mut self,
        route: Route,
        request: Request,
        priority: Priority,
        importance: u16,
        now: Instant,
    ) -> Result<Demand, AdmissionError> {
        let resource = request.resource();
        let key = LoadKey { resource, unit: request.unit() };
        if self.closed {
            return Err(AdmissionError::Closed);
        }
        if !self.routes.contains(&route) {
            return Err(AdmissionError::Route);
        }
        if resource.id.uuid().is_nil() {
            return Err(AdmissionError::InvalidId);
        }
        let cache_protection =
            self.cache.as_ref().map(|cache| cache.protect(request).map_err(|_| AdmissionError::CacheProtection)).transpose()?;
        let entry = self.entries.entry(key).or_insert_with(|| AssetEntry {
            request,
            views: BTreeMap::new(),
            phase: LoadPhase::Queued,
            used: now,
            ready: None,
            texture: None,
            target: None,
            priority,
            importance,
            cache_checked: false,
            cache_retry_at: None,
            force_full_mesh: false,
        });
        if !entry.request.is_compatible_with(request) {
            return Err(AdmissionError::ConflictingRequest);
        }
        entry.used = now;
        let texture = if resource.kind == Kind::Texture {
            let texture = entry.texture().unwrap_or_else(|| {
                entry.ready = None;
                match &entry.phase {
                    LoadPhase::Running { cancelled, .. } => {
                        cancelled.send_replace(true);
                    }
                    LoadPhase::Idle | LoadPhase::Queued => entry.phase = LoadPhase::Queued,
                }
                TextureHandle::new(resource.id, self.budget.clone(), self.textures.clone())
            });
            entry.texture = Some(texture.downgrade());
            Some(texture)
        } else {
            None
        };
        let wake = self.budget.wake();
        let ready = entry.ready();
        let view = entry.views.entry(route).or_insert_with(|| {
            let (status, _) = watch::channel(Status::Queued);
            RouteState {
                counter: Arc::new(Counter { count: AtomicUsize::new(0), wake }),
                status,
                attempts: 0,
                failed: None,
                deadline: now + DEMAND_DEADLINE,
                leases: Vec::new(),
            }
        });
        if view.count() == 0 && view.failed.as_ref().is_none_or(|failed| failed.expires <= now) {
            view.restart(now);
        }
        let status = match &entry.phase {
            LoadPhase::Idle => ready.as_ref().map_or(Status::Queued, |content| Status::Ready(content.clone())),
            LoadPhase::Running { stage: Stage::Decoding, .. } => Status::Decoding,
            LoadPhase::Running { stage: Stage::ReadingCache | Stage::Fetching | Stage::Cached, .. } => Status::Fetching,
            LoadPhase::Queued => view.queued_status(),
        };
        view.publish_status(status.retaining(ready.as_ref()));
        view.counter.count.fetch_add(1, Ordering::AcqRel);
        let lease = Arc::new(Lease {
            counter: Arc::clone(&view.counter),
            maintenance: Arc::clone(&self.maintenance_dirty),
            size: AtomicU16::new(request.size().map_or(0, |size| size.edge() as u16)),
            priority: AtomicBool::new(priority == Priority::Visible),
            importance: AtomicU16::new(importance),
            _cache_protection: cache_protection,
            _texture: texture,
        });
        view.leases.retain(|lease| lease.strong_count() > 0);
        view.leases.push(Arc::downgrade(&lease));
        self.maintenance_dirty.store(true, Ordering::Release);
        Ok(Demand { _lease: lease, status: view.status.subscribe() })
    }

    pub fn maintain(&mut self, now: Instant) {
        let dirty = self.maintenance_dirty.swap(false, Ordering::AcqRel);
        if self.maintained_at == Some(now) && !dirty {
            return;
        }
        for entry in self.entries.values_mut() {
            let ready = entry.ready();
            let mut desired = None;
            let mut priority = Priority::Background;
            let mut importance = 0;
            for view in entry.views.values_mut() {
                view.leases.retain(|weak| {
                    let Some(lease) = weak.upgrade() else { return false };
                    desired = desired.max(lease.size());
                    priority = priority.max(lease.priority());
                    importance = importance.max(lease.importance.load(Ordering::Acquire));
                    true
                });
            }
            if priority != entry.priority {
                for view in entry.views.values_mut() {
                    if now >= view.deadline && view.failed.as_ref().is_none_or(|failed| failed.failure == Failure::Deadline) {
                        view.restart(now);
                    }
                }
            }
            entry.priority = priority;
            entry.importance = importance;
            if entry.target != desired {
                entry.target = desired;
                if let Some(size) = desired
                    && let Request::Texture { id, .. } = entry.request
                {
                    entry.request = Request::Texture { id, size };
                }
                for view in entry.views.values_mut() {
                    view.restart(now);
                }
                if !matches!(entry.phase, LoadPhase::Running { .. }) {
                    entry.phase = LoadPhase::Queued;
                }
            }
            if matches!(entry.phase, LoadPhase::Queued)
                && let Some(content) = ready.as_ref()
                && content_satisfies(content, desired)
            {
                entry.phase = LoadPhase::Idle;
                entry.broadcast_status(Status::Ready((*content).clone()));
            }
            if let LoadPhase::Running { life, cancelled, route, stage, .. } = &entry.phase {
                if !entry.has_active_demand() || entry.views.get(route).is_none_or(|view| now >= view.deadline) {
                    cancelled.send_replace(true);
                }
                if life.upgrade().is_none() {
                    if let Some(view) = entry.views.get_mut(route) {
                        view.fail(Failure::Cancelled, None, now, ready.as_ref());
                    }
                    let stage = *stage;
                    entry.phase = LoadPhase::Queued;
                    match stage {
                        Stage::ReadingCache => {
                            finish_cache_read(&mut self.running_cache_reads);
                            if entry.request.resource().kind != Kind::Mesh {
                                finish_cache_read(&mut self.running_non_mesh_cache_reads);
                            }
                        }
                        Stage::Cached if entry.request.resource().kind != Kind::Mesh => {
                            finish_cache_read(&mut self.pending_non_mesh_cache_decodes);
                        }
                        Stage::Cached => {}
                        Stage::Fetching => {
                            finish_fetch(entry.request.resource().kind, &mut self.running_mesh_fetches, &mut self.running_other_fetches);
                        }
                        Stage::Decoding => {
                            finish_decode(entry.request.resource().kind, &mut self.running_texture_decodes, &mut self.running_other_decodes)
                        }
                    }
                }
            }
            if !matches!(entry.phase, LoadPhase::Idle)
                && (entry.priority == Priority::Visible || matches!(entry.phase, LoadPhase::Running { .. }))
            {
                for view in entry.views.values_mut().filter(|view| {
                    view.count() > 0 && now >= view.deadline && view.failed.as_ref().is_none_or(|failed| failed.retry_at.is_some())
                }) {
                    view.fail(Failure::Deadline, None, now, ready.as_ref());
                }
            }
        }
        self.entries
            .retain(|_, entry| entry.has_active_demand() || entry.texture().is_some() || matches!(entry.phase, LoadPhase::Running { .. }));
        self.candidates.clear();
        self.mesh_candidate_heap.clear();
        self.other_candidate_heap.clear();
        self.candidates_dirty = true;
        self.maintained_at = Some(now);
    }

    pub fn next_job(&mut self, now: Instant) -> Option<LoadJob> {
        if self.closed {
            return None;
        }
        self.maintain(now);
        self.build_candidates(now);
        let mut selected = None;
        for index in (0..self.candidates.len()).rev() {
            let candidate = self.candidates[index];
            let key = candidate.key;
            let resource = key.resource;
            let Some(request) = self.entries.get(&key).map(|entry| entry.request) else { continue };
            let cache_unchecked = self
                .entries
                .get(&key)
                .is_some_and(|entry| !entry.cache_checked && entry.texture().and_then(|texture| texture.source()).is_none());
            if cache_unchecked
                && self.cache.as_ref().is_some_and(|cache| !cache.may_contain(request))
                && let Some(entry) = self.entries.get_mut(&key)
            {
                entry.cache_checked = true;
            }
            let cache_read = self.cache.is_some()
                && self
                    .entries
                    .get(&key)
                    .is_some_and(|entry| !entry.cache_checked && entry.texture().and_then(|texture| texture.source()).is_none());
            if cache_read {
                if self.running_cache_reads >= storage::MAX_IN_FLIGHT_READS
                    || resource.kind != Kind::Mesh
                        && self.running_non_mesh_cache_reads + self.pending_non_mesh_cache_decodes
                            >= MAX_TEXTURE_DECODES + MAX_NON_TEXTURE_DECODES
                {
                    continue;
                }
            } else if (resource.kind == Kind::Mesh && self.running_mesh_fetches >= MAX_MESH_FETCHES)
                || (resource.kind != Kind::Mesh && self.running_other_fetches >= MAX_NON_MESH_FETCHES)
            {
                continue;
            }
            let prefix_lod = (!cache_read).then(|| self.mesh_lod_prefix(request)).flatten();
            let force_full_mesh = self.entries.get(&key).is_some_and(|entry| entry.force_full_mesh);
            let fetch_bytes = if force_full_mesh && resource.kind == Kind::Mesh {
                Some(resource.kind.max_response_bytes())
            } else {
                fetch_work_bytes(request)
            };
            let Some(bytes) = (if cache_read { read_work_bytes_for_request(request) } else { fetch_bytes })
                .map(|bytes| bytes.max(prefix_lod.as_ref().map_or(0, |(_, bytes)| bytes.capacity())))
            else {
                continue;
            };
            let headroom =
                if self.running_texture_decodes + self.running_other_decodes > 0 { 0 } else { TEXTURE_WORK_BYTES.saturating_sub(bytes) };
            let Some(reservation) = self.budget.reserve_with_headroom(bytes, headroom) else { continue };
            let generation = self.sequence.checked_add(1)?;
            self.sequence = generation;
            selected = Some((index, candidate, prefix_lod, reservation, generation));
            break;
        }
        let (index, candidate, prefix_lod, reservation, generation) = selected?;
        self.candidates.remove(index);
        let key = candidate.key;
        let route = candidate.route;
        let resource = key.resource;
        let life = Arc::new(WorkLife { maintenance: Arc::clone(&self.maintenance_dirty) });
        let (cancelled, receiver) = watch::channel(false);
        let entry = self.entries.get_mut(&key).expect("selected demanded entry");
        if entry.cache_retry_at.is_some_and(|retry_at| retry_at <= now) {
            entry.cache_retry_at = None;
        }
        let source = entry.texture().and_then(|texture| texture.source());
        let job = Job {
            resource,
            key,
            request: entry.request,
            route,
            generation,
            reservation,
            _life: life,
            cancelled: receiver,
            size: entry.target.unwrap_or_default(),
            force_full_mesh: entry.force_full_mesh,
        };
        if let Some(source) = source {
            if entry.priority == Priority::Background {
                entry.views.get_mut(&route).expect("selected route").deadline = now + DEMAND_DEADLINE;
            }
            entry.views.get_mut(&route).expect("selected route").attempts += 1;
            entry.phase = LoadPhase::Running { generation, route, life: Arc::downgrade(&job._life), cancelled, stage: Stage::Fetching };
            start_fetch(resource.kind, &mut self.running_mesh_fetches, &mut self.running_other_fetches);
            entry.broadcast_status(Status::Fetching);
            return Some(LoadJob::Decode(DecodeInput { source: DecodeSource::Cached(source), job }));
        }
        if let Some((block, bytes)) = prefix_lod {
            if entry.priority == Priority::Background {
                entry.views.get_mut(&route).expect("selected route").deadline = now + DEMAND_DEADLINE;
            }
            entry.views.get_mut(&route).expect("selected route").attempts += 1;
            entry.phase = LoadPhase::Running { generation, route, life: Arc::downgrade(&job._life), cancelled, stage: Stage::Fetching };
            start_fetch(resource.kind, &mut self.running_mesh_fetches, &mut self.running_other_fetches);
            entry.broadcast_status(Status::Fetching);
            let response = FetchResponse::Partial { bytes, range: block.range(), total_length: Some(block.asset_length()) };
            return Some(LoadJob::Decode(DecodeInput { source: DecodeSource::Response(response), job }));
        }
        if let Some(cache) = &self.cache
            && !entry.cache_checked
        {
            if entry.priority == Priority::Background {
                entry.views.get_mut(&route).expect("selected route").deadline = now + DEMAND_DEADLINE;
            }
            entry.phase = LoadPhase::Running { generation, route, life: Arc::downgrade(&job._life), cancelled, stage: Stage::ReadingCache };
            self.running_cache_reads += 1;
            if resource.kind != Kind::Mesh {
                self.running_non_mesh_cache_reads += 1;
            }
            entry.broadcast_status(Status::Fetching);
            return Some(LoadJob::ReadCache(CacheReadJob { job, cache: Arc::clone(cache) }));
        }
        if entry.priority == Priority::Background {
            entry.views.get_mut(&route).expect("selected route").deadline = now + DEMAND_DEADLINE;
        }
        entry.views.get_mut(&route).expect("selected route").attempts += 1;
        entry.phase = LoadPhase::Running { generation, route, life: Arc::downgrade(&job._life), cancelled, stage: Stage::Fetching };
        start_fetch(resource.kind, &mut self.running_mesh_fetches, &mut self.running_other_fetches);
        entry.broadcast_status(Status::Fetching);
        Some(LoadJob::Fetch(FetchJob { job }))
    }

    fn mesh_lod_prefix(&self, request: Request) -> Option<(MeshLodRequest, Vec<u8>)> {
        let Request::MeshLod(block) = request else { return None };
        let resource = Resource { id: block.asset_id(), kind: Kind::Mesh };
        let key = LoadKey { resource, unit: CacheUnit::Header };
        let Content::MeshHeader(header) = self.entries.get(&key)?.ready()? else { return None };
        let source = header.lod_bytes(block)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(source.len()).ok()?;
        bytes.extend_from_slice(source);
        Some((block, bytes))
    }

    #[allow(clippy::result_large_err)]
    pub fn start_decode(&mut self, mut input: DecodeInput) -> Result<DecodeJob, DecodeInput> {
        if self.closed || input.is_cancelled() || input.job.route.store != self.id {
            return Err(input);
        }
        let decode_slots_full = if input.job.resource.kind == Kind::Texture {
            self.running_texture_decodes >= MAX_TEXTURE_DECODES
        } else {
            self.running_other_decodes >= MAX_NON_TEXTURE_DECODES
        };
        if decode_slots_full {
            return Err(input);
        }
        if !self.entries.get(&input.job.key).is_some_and(
            |entry| matches!(entry.phase, LoadPhase::Running { generation, stage: Stage::Fetching | Stage::Cached, .. } if generation == input.job.generation),
        ) {
            return Err(input);
        }
        if let Some(bytes) = input.job.resource.kind.work_bytes()
            && !input.job.reservation.resize(bytes)
        {
            return Err(input);
        }
        let entry = self.entries.get_mut(&input.job.key).expect("validated running job");
        let LoadPhase::Running { stage, .. } = &mut entry.phase else { unreachable!("validated running phase") };
        let cached = *stage == Stage::Cached;
        *stage = Stage::Decoding;
        if cached && input.job.resource.kind != Kind::Mesh {
            finish_cache_read(&mut self.pending_non_mesh_cache_decodes);
        } else {
            if !cached {
                finish_fetch(input.job.resource.kind, &mut self.running_mesh_fetches, &mut self.running_other_fetches);
            }
        }
        start_decode(input.job.resource.kind, &mut self.running_texture_decodes, &mut self.running_other_decodes);
        self.maintenance_dirty.store(true, Ordering::Release);
        entry.broadcast_status(Status::Decoding);
        Ok(DecodeJob(input))
    }

    pub fn complete_cache_read(&mut self, completion: CacheReadCompletion, now: Instant) -> Option<LoadJob> {
        let CacheReadCompletion { mut job, result } = completion;
        if self.closed || job.route.store != self.id {
            return None;
        }
        let entry = self.entries.get_mut(&job.key)?;
        if !matches!(entry.phase, LoadPhase::Running { generation, stage: Stage::ReadingCache, .. } if generation == job.generation) {
            return None;
        }
        self.maintenance_dirty.store(true, Ordering::Release);
        finish_cache_read(&mut self.running_cache_reads);
        if job.resource.kind != Kind::Mesh {
            finish_cache_read(&mut self.running_non_mesh_cache_reads);
        }
        if !self.routes.contains(&job.route)
            || job.is_cancelled()
            || !entry.has_active_demand()
            || entry.views.get(&job.route).is_none_or(|view| now >= view.deadline)
        {
            entry.phase = LoadPhase::Queued;
            return None;
        }
        if !matches!(&result, Err(Failure::Cancelled))
            && !matches!(&result, Ok(AssetLookup::Unavailable(storage::UnavailableReason::ReadLimit)))
        {
            entry.cache_checked = true;
        }
        match result {
            Ok(AssetLookup::Hit(asset)) => {
                let decode_bytes = job.resource.kind.work_bytes().unwrap_or(asset.bytes.capacity());
                job.reservation.shrink_to_at_most(decode_bytes.max(asset.bytes.capacity()));
                if job.resource.kind != Kind::Mesh {
                    self.pending_non_mesh_cache_decodes += 1;
                }
                if let LoadPhase::Running { stage, .. } = &mut entry.phase {
                    *stage = Stage::Cached;
                }
                Some(LoadJob::Decode(DecodeInput { source: DecodeSource::Disk(asset.bytes), job }))
            }
            Ok(AssetLookup::Unavailable(storage::UnavailableReason::ReadLimit)) => {
                entry.phase = LoadPhase::Queued;
                entry.broadcast_status(Status::Queued);
                None
            }
            _ => {
                let fetch = Self::cache_read_fallback(entry, job, now)?;
                start_fetch(fetch.resource().kind, &mut self.running_mesh_fetches, &mut self.running_other_fetches);
                Some(LoadJob::Fetch(fetch))
            }
        }
    }

    fn cache_read_fallback(entry: &mut AssetEntry, mut job: Job, now: Instant) -> Option<FetchJob> {
        let bytes = if entry.force_full_mesh && job.resource.kind == Kind::Mesh {
            job.resource.kind.max_response_bytes()
        } else {
            fetch_work_bytes(job.request)?
        };
        job.reservation.shrink_to_at_most(bytes);
        if entry.priority == Priority::Background
            && let Some(view) = entry.views.get_mut(&job.route)
        {
            view.deadline = now + DEMAND_DEADLINE;
        }
        if let Some(view) = entry.views.get_mut(&job.route) {
            view.attempts = view.attempts.saturating_add(1);
        }
        if let LoadPhase::Running { stage, .. } = &mut entry.phase {
            *stage = Stage::Fetching;
        }
        Some(FetchJob { job })
    }

    pub fn complete(&mut self, completion: LoadCompletion, now: Instant) -> Vec<CacheWrite> {
        let LoadCompletion { job, result, origin, decode_duration } = completion;
        let mut writes = Vec::new();
        if self.closed || job.route.store != self.id {
            return writes;
        }
        let Some(entry) = self.entries.get_mut(&job.key) else { return writes };
        let stage = match &entry.phase {
            LoadPhase::Running { generation, stage, .. } if *generation == job.generation => *stage,
            _ => return writes,
        };
        if stage == Stage::Decoding
            && result.is_ok()
            && let (Some(cache), Some(duration)) = (&self.cache, decode_duration)
        {
            cache.observe_decode(job.resource.kind, duration);
        }
        match stage {
            Stage::ReadingCache => {
                finish_cache_read(&mut self.running_cache_reads);
                if job.resource.kind != Kind::Mesh {
                    finish_cache_read(&mut self.running_non_mesh_cache_reads);
                }
            }
            Stage::Fetching => {
                finish_fetch(job.resource.kind, &mut self.running_mesh_fetches, &mut self.running_other_fetches);
            }
            Stage::Cached if job.resource.kind != Kind::Mesh => {
                finish_cache_read(&mut self.pending_non_mesh_cache_decodes);
            }
            Stage::Cached => {}
            Stage::Decoding => finish_decode(job.resource.kind, &mut self.running_texture_decodes, &mut self.running_other_decodes),
        }
        entry.phase = LoadPhase::Queued;
        self.maintenance_dirty.store(true, Ordering::Release);
        if !self.routes.contains(&job.route)
            || job.is_cancelled()
            || !entry.has_active_demand()
            || entry.views.get(&job.route).is_none_or(|view| now >= view.deadline)
        {
            return writes;
        }
        if origin == DecodeOrigin::Disk && result.as_ref().is_err_and(|failure| *failure == Failure::MemoryPressure) {
            entry.cache_checked = false;
            entry.cache_retry_at = Some(now + CACHE_PRESSURE_RETRY);
            entry.broadcast_status(Status::Queued);
            return writes;
        }
        if result.as_ref().is_err_and(|failure| *failure == Failure::FullAssetRequired)
            && job.resource.kind == Kind::Mesh
            && !entry.force_full_mesh
        {
            entry.force_full_mesh = true;
            if let Some(view) = entry.views.get_mut(&job.route) {
                view.attempts = view.attempts.saturating_sub(1);
                view.failed = None;
            }
            entry.broadcast_status(Status::Queued);
            return writes;
        }
        if origin == DecodeOrigin::Disk && result.as_ref().is_err_and(|failure| *failure != Failure::Cancelled) && !job.is_cancelled() {
            if let Some(cache) = &self.cache {
                cache.discard_corrupt(job.request);
            }
            return writes;
        }
        let persist = self.cache.is_some() && origin == DecodeOrigin::Network;
        match result.and_then(|prepared| prepared.finish(job.reservation, persist)) {
            Ok((content, source, persisted)) => {
                entry.used = now;
                if let (Some(cache), Some(payload)) = (&self.cache, persisted)
                    && let Some(write) = cache.queue_write(job.request, payload, &self.budget)
                {
                    writes.push(write);
                }
                if job.resource.kind == Kind::Texture
                    && entry.requested_size().is_some_and(|size| size != job.size)
                    && let Some(ready) = entry.ready()
                {
                    entry.broadcast_status(Status::Ready(ready));
                    return writes;
                }
                let content = match content {
                    Finished::Texture(image) => {
                        let Some(texture) = entry.texture() else { return writes };
                        texture.install(image, source);
                        Content::Texture(texture)
                    }
                    Finished::Other(content) => content,
                };
                let satisfied = content_satisfies(&content, entry.requested_size());
                if satisfied {
                    entry.phase = LoadPhase::Idle;
                }
                entry.broadcast_status(Status::Ready(content.clone()));
                entry.ready = Some(content.downgrade());
            }
            Err(failure) => {
                if job.resource.kind == Kind::Texture && entry.requested_size().is_some_and(|size| size != job.size) {
                    return writes;
                }
                let ready = entry.ready();
                let view = entry.views.get_mut(&job.route).expect("active job route");
                let retry_at = match failure {
                    Failure::Transient { retry_after } if view.attempts < MAX_ATTEMPTS => {
                        let delay = retry_after.max(Duration::from_secs(u64::from(view.attempts.max(1))));
                        if delay > Duration::from_secs(30) {
                            None
                        } else {
                            let retry = now + delay;
                            (retry < view.deadline).then_some(retry)
                        }
                    }
                    _ => None,
                };
                view.fail(failure, retry_at, now, ready.as_ref());
                for (route, view) in &entry.views {
                    if *route != job.route && view.count() > 0 {
                        view.publish_status(view.queued_status().retaining(ready.as_ref()));
                    }
                }
            }
        }
        writes
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        self.entries
            .values()
            .filter(|entry| entry.has_active_demand() && !matches!(entry.phase, LoadPhase::Idle))
            .flat_map(|entry| {
                let demand_deadlines =
                    (entry.priority == Priority::Visible || matches!(entry.phase, LoadPhase::Running { .. })).then(|| {
                        entry
                            .views
                            .values()
                            .filter(|view| view.count() > 0 && view.failed.as_ref().is_none_or(|failed| failed.retry_at.is_some()))
                            .flat_map(|view| std::iter::once(view.deadline).chain(view.failed.as_ref().and_then(|failed| failed.retry_at)))
                    });
                entry.cache_retry_at.into_iter().chain(demand_deadlines.into_iter().flatten())
            })
            .filter(|deadline| *deadline > now)
            .min()
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.running_mesh_fetches = 0;
        self.running_other_fetches = 0;
        self.running_cache_reads = 0;
        self.running_non_mesh_cache_reads = 0;
        self.pending_non_mesh_cache_decodes = 0;
        self.running_texture_decodes = 0;
        self.running_other_decodes = 0;
        self.maintenance_dirty.store(true, Ordering::Release);
        for entry in self.entries.values() {
            if let LoadPhase::Running { cancelled, .. } = &entry.phase {
                cancelled.send_replace(true);
            }
            entry.broadcast_status(Status::StaleSession);
        }
        self.entries.clear();
        self.candidates.clear();
        self.mesh_candidate_heap.clear();
        self.other_candidate_heap.clear();
        self.candidates_dirty = true;
        self.routes.clear();
    }

    fn build_candidates(&mut self, now: Instant) {
        if !self.candidates_dirty {
            return;
        }
        self.candidates.clear();
        self.mesh_candidate_heap.clear();
        self.other_candidate_heap.clear();
        let mesh_limit = MAX_MESH_FETCHES;
        let other_limit = MAX_NON_MESH_FETCHES;
        for (key, entry) in &self.entries {
            if matches!(entry.phase, LoadPhase::Queued)
                && let Some((route, _)) = entry.views.iter().find(|(_, view)| {
                    view.count() > 0
                        && view.attempts < MAX_ATTEMPTS
                        && (entry.priority == Priority::Background || now < view.deadline)
                        && entry.cache_retry_at.is_none_or(|retry_at| retry_at <= now)
                        && view.failed.as_ref().is_none_or(|failed| failed.retry_at.is_some_and(|retry| retry <= now))
                })
            {
                let candidate = ScheduleKey {
                    priority: Reverse(entry.priority),
                    importance: Reverse(entry.importance),
                    ready: entry.ready().is_some(),
                    used: entry.used,
                    key: *key,
                    route: *route,
                };
                if key.resource.kind == Kind::Mesh {
                    retain_candidate(&mut self.mesh_candidate_heap, mesh_limit, candidate);
                } else {
                    retain_candidate(&mut self.other_candidate_heap, other_limit, candidate);
                }
            }
        }
        while let Some(candidate) = self.mesh_candidate_heap.pop() {
            self.candidates.push(candidate);
        }
        while let Some(candidate) = self.other_candidate_heap.pop() {
            self.candidates.push(candidate);
        }
        self.candidates.sort_unstable_by(|left, right| right.cmp(left));
        self.candidates_dirty = false;
    }
}

fn content_satisfies(content: &Content, desired: Option<TextureSize>) -> bool {
    match content {
        Content::Texture(image) => {
            desired.is_none_or(|size| image.info().is_some_and(|info| info.discard == size.discard_level(info.source_size)))
        }
        Content::Material(_) | Content::Environment(_) | Content::MeshHeader(_) | Content::Mesh(_) => true,
    }
}

fn start_fetch(kind: Kind, mesh: &mut usize, other: &mut usize) {
    if kind == Kind::Mesh {
        *mesh += 1;
    } else {
        *other += 1;
    }
}

fn finish_cache_read(reads: &mut usize) {
    *reads -= 1;
}

fn finish_fetch(kind: Kind, mesh: &mut usize, other: &mut usize) {
    if kind == Kind::Mesh {
        *mesh -= 1;
    } else {
        *other -= 1;
    }
}

fn start_decode(kind: Kind, texture: &mut usize, other: &mut usize) {
    if kind == Kind::Texture {
        *texture += 1;
    } else {
        *other += 1;
    }
}

fn finish_decode(kind: Kind, texture: &mut usize, other: &mut usize) {
    if kind == Kind::Texture {
        *texture -= 1;
    } else {
        *other -= 1;
    }
}

fn fetch_work_bytes(request: Request) -> Option<usize> {
    match request {
        Request::MeshHeader(_) => Some(crate::mesh::MAX_MESH_HEADER_BYTES),
        Request::MeshLod(block) => usize::try_from(block.range().end.checked_sub(block.range().start)?).ok(),
        _ => Some(request.resource().kind.max_response_bytes()),
    }
}

fn retain_candidate(candidates: &mut BinaryHeap<ScheduleKey>, limit: usize, candidate: ScheduleKey) {
    if limit == 0 {
        return;
    }
    if candidates.len() < limit {
        candidates.push(candidate);
        return;
    }
    if candidates.peek().is_some_and(|largest| candidate < *largest) {
        candidates.pop();
        candidates.push(candidate);
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
