use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::watch;
use utils::AssetId;

use crate::{
    Blob, Budget, Content, TextureEvents, TextureHandle, TextureSize, WeakTexture,
    budget::{Reservation, TEXTURE_WORK_BYTES, WORK_BYTES},
    content::{Finished, Prepared, StoredContent},
};

pub const MAX_ASSET_ROUTES: usize = 33;
pub const MAX_ASSET_DECODES: usize = 2;
pub const MAX_ASSET_FETCHES: usize = 8;
const DEMAND_DEADLINE: Duration = Duration::from_secs(60);
const NEGATIVE_TTL: Duration = Duration::from_secs(30);
const MAX_ATTEMPTS: u8 = 3;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Kind {
    Texture,
    PbrMaterial,
    LegacyMaterial,
    Environment,
}

impl Kind {
    const fn work_bytes(self) -> usize {
        match self {
            Self::Texture => TEXTURE_WORK_BYTES,
            _ => WORK_BYTES,
        }
    }

    pub const fn max_response_bytes(self) -> usize {
        match self {
            Self::Texture => 32 * 1024 * 1024,
            Self::PbrMaterial | Self::LegacyMaterial | Self::Environment => 2 * 1024 * 1024,
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
}

impl Request {
    pub fn new(resource: Resource) -> Self {
        match resource.kind {
            Kind::Texture => Self::Texture { id: resource.id, size: TextureSize::default() },
            Kind::PbrMaterial => Self::PbrMaterial(resource.id),
            Kind::LegacyMaterial => Self::LegacyMaterial(resource.id),
            Kind::Environment => Self::Environment(resource.id),
        }
    }
    pub fn resource(self) -> Resource {
        match self {
            Self::Texture { id, .. } => Resource { id, kind: Kind::Texture },
            Self::PbrMaterial(id) => Resource { id, kind: Kind::PbrMaterial },
            Self::LegacyMaterial(id) => Resource { id, kind: Kind::LegacyMaterial },
            Self::Environment(id) => Resource { id, kind: Kind::Environment },
        }
    }

    fn size(self) -> Option<TextureSize> {
        match self {
            Self::Texture { size, .. } => Some(size),
            _ => None,
        }
    }
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
    TooLarge,
    Malformed,
    Unsupported,
    RouteClosed,
    SessionClosed,
    Cancelled,
    Deadline,
    Transient { retry_after: Duration },
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
}

struct Counter {
    count: AtomicUsize,
    wake: watch::Sender<()>,
}

struct Lease {
    counter: Arc<Counter>,
    size: Option<TextureSize>,
    priority: Priority,
    _texture: Option<TextureHandle>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        self.counter.count.fetch_sub(1, Ordering::AcqRel);
        self.counter.wake.send_replace(());
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

enum LoadPhase {
    Idle,
    Queued,
    Running { generation: u64, route: Route, life: Weak<()>, cancelled: watch::Sender<bool>, stage: Stage },
}

enum Stage {
    Fetching,
    Decoding,
}

struct AssetEntry {
    views: BTreeMap<Route, RouteState>,
    phase: LoadPhase,
    used: Instant,
    ready: Option<StoredContent>,
    texture: Option<WeakTexture>,
    target: Option<TextureSize>,
    priority: Priority,
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
            .map(|lease| lease.priority)
            .max()
            .unwrap_or(Priority::Background)
    }
    fn requested_size(&self) -> Option<TextureSize> {
        self.views.values().flat_map(|view| &view.leases).filter_map(Weak::upgrade).filter_map(|lease| lease.size).max()
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
    route: Route,
    generation: u64,
    reservation: Reservation,
    _life: Arc<()>,
    cancelled: watch::Receiver<bool>,
    size: TextureSize,
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
        LoadCompletion { job: self, result: Err(failure) }
    }
}

#[derive(Debug)]
pub struct FetchFailure {
    job: Job,
    failure: Failure,
}

impl FetchFailure {
    fn new(job: Job, failure: Failure) -> Self {
        let failure = if job.is_cancelled() { Failure::Cancelled } else { failure };
        Self { job, failure }
    }

    pub fn failure(&self) -> Failure {
        self.failure
    }
}

impl From<FetchFailure> for LoadCompletion {
    fn from(fetch: FetchFailure) -> Self {
        Self { job: fetch.job, result: Err(fetch.failure) }
    }
}

#[derive(Debug)]
pub struct FetchJob(Job);

impl FetchJob {
    pub fn resource(&self) -> Resource {
        self.0.resource
    }
    pub fn route(&self) -> Route {
        self.0.route
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + use<> {
        self.0.cancelled()
    }

    pub fn complete(self, response: Result<Vec<u8>, Failure>) -> Result<DecodeInput, FetchFailure> {
        let bytes = match response {
            Ok(bytes) if !self.0.is_cancelled() => bytes,
            Ok(_) => return Err(FetchFailure::new(self.0, Failure::Cancelled)),
            Err(failure) => return Err(FetchFailure::new(self.0, failure)),
        };
        if bytes.capacity() > self.0.resource.kind.max_response_bytes() {
            return Err(FetchFailure::new(self.0, Failure::TooLarge));
        }
        let mut job = self.0;
        if !job.reservation.resize(bytes.capacity()) {
            return Err(FetchFailure::new(job, Failure::TooLarge));
        }
        Ok(DecodeInput { job, source: DecodeSource::Response(bytes) })
    }
}

#[derive(Debug)]
pub enum LoadJob {
    Fetch(FetchJob),
    Decode(DecodeInput),
}

impl LoadJob {
    pub fn fail(self, failure: Failure) -> LoadCompletion {
        match self {
            Self::Fetch(job) => job.0.fail(failure),
            Self::Decode(input) => input.fail(failure),
        }
    }
}

enum DecodeSource {
    Response(Vec<u8>),
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
    pub fn resource(&self) -> Resource {
        self.job.resource
    }
    pub fn is_cancelled(&self) -> bool {
        self.job.is_cancelled()
    }
    pub fn fail(self, failure: Failure) -> LoadCompletion {
        self.job.fail(failure)
    }
}

#[derive(Debug)]
pub struct DecodeJob(DecodeInput);

impl DecodeJob {
    pub fn run(self) -> LoadCompletion {
        let DecodeInput { job, source } = self.0;
        let result = match source {
            DecodeSource::Response(bytes) => crate::content::prepare_cancellable(job.resource, job.size, bytes, || job.is_cancelled()),
            DecodeSource::Cached(source) => crate::content::prepare_cached(job.size, &source, || job.is_cancelled()),
        };
        LoadCompletion { job, result }
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
    entries: BTreeMap<Resource, AssetEntry>,
    budget: Budget,
    closed: bool,
    textures: TextureEvents,
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
            budget,
            closed: false,
            textures: TextureEvents::default(),
        }
    }

    pub fn wake(&self) -> watch::Receiver<()> {
        self.budget.wake().subscribe()
    }

    pub fn priority(&self, input: &DecodeInput) -> Priority {
        self.entries.get(&input.job.resource).map_or(Priority::Background, AssetEntry::effective_priority)
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
        let resource = request.resource();
        if self.closed {
            return Err(AdmissionError::Closed);
        }
        if !self.routes.contains(&route) {
            return Err(AdmissionError::Route);
        }
        if resource.id.uuid().is_nil() {
            return Err(AdmissionError::InvalidId);
        }
        let entry = self.entries.entry(resource).or_insert_with(|| AssetEntry {
            views: BTreeMap::new(),
            phase: LoadPhase::Queued,
            used: now,
            ready: None,
            texture: None,
            target: None,
            priority,
        });
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
            LoadPhase::Running { stage: Stage::Fetching, .. } => Status::Fetching,
            LoadPhase::Queued => view.queued_status(),
        };
        view.publish_status(status.retaining(ready.as_ref()));
        view.counter.count.fetch_add(1, Ordering::AcqRel);
        let lease = Arc::new(Lease { counter: Arc::clone(&view.counter), size: request.size(), priority, _texture: texture });
        view.leases.retain(|lease| lease.strong_count() > 0);
        view.leases.push(Arc::downgrade(&lease));
        Ok(Demand { _lease: lease, status: view.status.subscribe() })
    }

    pub fn maintain(&mut self, now: Instant) {
        for entry in self.entries.values_mut() {
            let ready = entry.ready();
            let mut desired = None;
            let mut priority = Priority::Background;
            for view in entry.views.values_mut() {
                view.leases.retain(|weak| {
                    let Some(lease) = weak.upgrade() else { return false };
                    desired = desired.max(lease.size);
                    priority = priority.max(lease.priority);
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
            if entry.target != desired {
                entry.target = desired;
                if ready.is_some() {
                    for view in entry.views.values_mut() {
                        view.restart(now);
                    }
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
            if let LoadPhase::Running { life, cancelled, route, .. } = &entry.phase {
                if !entry.has_active_demand() || entry.views.get(route).is_none_or(|view| now >= view.deadline) {
                    cancelled.send_replace(true);
                }
                if life.upgrade().is_none() {
                    if let Some(view) = entry.views.get_mut(route) {
                        view.fail(Failure::Cancelled, None, now, ready.as_ref());
                    }
                    entry.phase = LoadPhase::Queued;
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
    }

    pub fn next_job(&mut self, now: Instant) -> Option<LoadJob> {
        self.maintain(now);
        if self.closed
            || self.entries.values().filter(|entry| matches!(entry.phase, LoadPhase::Running { stage: Stage::Fetching, .. })).count()
                >= MAX_ASSET_FETCHES
        {
            return None;
        }
        let (resource, route) = self
            .entries
            .iter()
            .filter(|(_, entry)| matches!(entry.phase, LoadPhase::Queued))
            .filter_map(|(resource, entry)| {
                entry
                    .views
                    .iter()
                    .find(|(_, view)| {
                        view.count() > 0
                            && view.attempts < MAX_ATTEMPTS
                            && (entry.priority == Priority::Background || now < view.deadline)
                            && view.failed.as_ref().is_none_or(|failed| failed.retry_at.is_some_and(|retry| retry <= now))
                    })
                    .map(|(route, _)| (*resource, *route, (Reverse(entry.priority), entry.ready().is_some(), entry.used)))
            })
            .min_by_key(|(_, _, used)| *used)
            .map(|(resource, route, _)| (resource, route))?;
        let bytes = resource.kind.max_response_bytes();
        let headroom = if self.entries.values().any(|entry| matches!(entry.phase, LoadPhase::Running { stage: Stage::Decoding, .. })) {
            0
        } else {
            TEXTURE_WORK_BYTES.saturating_sub(bytes)
        };
        let reservation = self.budget.reserve_with_headroom(bytes, headroom)?;
        self.sequence = self.sequence.checked_add(1)?;
        let generation = self.sequence;
        let life = Arc::new(());
        let (cancelled, receiver) = watch::channel(false);
        let entry = self.entries.get_mut(&resource).expect("selected demanded entry");
        if entry.priority == Priority::Background {
            entry.views.get_mut(&route).expect("selected route").deadline = now + DEMAND_DEADLINE;
        }
        entry.views.get_mut(&route).expect("selected route").attempts += 1;
        entry.phase = LoadPhase::Running { generation, route, life: Arc::downgrade(&life), cancelled, stage: Stage::Fetching };
        entry.broadcast_status(Status::Fetching);
        let source = entry.texture().and_then(|texture| texture.source());
        let job =
            Job { resource, route, generation, reservation, _life: life, cancelled: receiver, size: entry.target.unwrap_or_default() };
        Some(match source {
            Some(source) => LoadJob::Decode(DecodeInput { source: DecodeSource::Cached(source), job }),
            None => LoadJob::Fetch(FetchJob(job)),
        })
    }

    pub fn start_decode(&mut self, mut input: DecodeInput) -> Result<DecodeJob, DecodeInput> {
        if self.closed || input.is_cancelled() || input.job.route.store != self.id {
            return Err(input);
        }
        if self.entries.values().filter(|entry| matches!(entry.phase, LoadPhase::Running { stage: Stage::Decoding, .. })).count()
            >= MAX_ASSET_DECODES
        {
            return Err(input);
        }
        if !self.entries.get(&input.job.resource).is_some_and(
            |entry| matches!(entry.phase, LoadPhase::Running { generation, stage: Stage::Fetching, .. } if generation == input.job.generation),
        ) {
            return Err(input);
        }
        if !input.job.reservation.resize(input.job.resource.kind.work_bytes()) {
            return Err(input);
        }
        let entry = self.entries.get_mut(&input.job.resource).expect("validated running job");
        let LoadPhase::Running { stage, .. } = &mut entry.phase else { unreachable!("validated running phase") };
        *stage = Stage::Decoding;
        entry.broadcast_status(Status::Decoding);
        Ok(DecodeJob(input))
    }

    pub fn complete(&mut self, completion: LoadCompletion, now: Instant) {
        let LoadCompletion { job, result } = completion;
        if self.closed || job.route.store != self.id {
            return;
        }
        let Some(entry) = self.entries.get_mut(&job.resource) else { return };
        if !matches!(entry.phase, LoadPhase::Running { generation, .. } if generation == job.generation) {
            return;
        }
        entry.phase = LoadPhase::Queued;
        if !self.routes.contains(&job.route)
            || job.is_cancelled()
            || !entry.has_active_demand()
            || entry.views.get(&job.route).is_none_or(|view| now >= view.deadline)
        {
            return;
        }
        match result.and_then(|prepared| prepared.finish(job.reservation)) {
            Ok((content, source)) => {
                entry.used = now;
                if job.resource.kind == Kind::Texture
                    && entry.requested_size().is_some_and(|size| size != job.size)
                    && let Some(ready) = entry.ready()
                {
                    entry.broadcast_status(Status::Ready(ready));
                    return;
                }
                let content = match content {
                    Finished::Texture(image) => {
                        let Some(texture) = entry.texture() else { return };
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
                    return;
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
    }

    pub fn next_deadline(&self, now: Instant) -> Option<Instant> {
        self.entries
            .values()
            .filter(|entry| {
                entry.has_active_demand()
                    && !matches!(entry.phase, LoadPhase::Idle)
                    && (entry.priority == Priority::Visible || matches!(entry.phase, LoadPhase::Running { .. }))
            })
            .flat_map(|entry| {
                entry
                    .views
                    .values()
                    .filter(|view| view.count() > 0 && view.failed.as_ref().is_none_or(|failed| failed.retry_at.is_some()))
                    .flat_map(|view| std::iter::once(view.deadline).chain(view.failed.as_ref().and_then(|failed| failed.retry_at)))
            })
            .filter(|deadline| *deadline > now)
            .min()
    }

    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        for entry in self.entries.values() {
            if let LoadPhase::Running { cancelled, .. } = &entry.phase {
                cancelled.send_replace(true);
            }
            entry.broadcast_status(Status::StaleSession);
        }
        self.entries.clear();
        self.routes.clear();
    }
}

fn content_satisfies(content: &Content, desired: Option<TextureSize>) -> bool {
    match content {
        Content::Texture(image) => {
            desired.is_none_or(|size| image.info().is_some_and(|info| info.discard == size.discard_level(info.source_size)))
        }
        Content::Material(_) | Content::Environment(_) => true,
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
