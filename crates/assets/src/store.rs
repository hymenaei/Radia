use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use base::AssetId;
use tokio::sync::watch;

use crate::{
    Blob, Budget, Content, Prepared, TextureEvents, TextureHandle, TextureSize, WeakTexture,
    budget::{Reservation, TEXTURE_WORK_BYTES, WORK_BYTES},
    content::{Finished, StoredContent},
};

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Priority {
    Background,
    Visible,
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
            other => other.clone().map(|_| unreachable!("content variants handled")),
        }
    }
}

impl Status {
    pub fn content(&self) -> Option<&Content> {
        match self {
            Self::Ready(content) | Self::Refining(content) | Self::RefinementFailed { content, .. } => Some(content),
            _ => None,
        }
    }

    fn retaining(self, ready: Option<&Content>) -> Self {
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
    total: Arc<AtomicUsize>,
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
        self.counter.total.fetch_sub(1, Ordering::AcqRel);
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
            return Status::StaleSession;
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
    fn publish_status(&self, status: Status) {
        self.status.send_replace(status.map(|content| content.downgrade()));
    }
}

enum LoadPhase {
    Idle,
    Queued,
    Running { generation: u64, route: Route, life: Weak<()>, cancelled: watch::Sender<bool>, decoding: bool },
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
        for view in self.views.values().filter(|view| view.count() > 0) {
            view.publish_status(status.clone().retaining(self.ready().as_ref()));
        }
    }
}

pub struct LoadJob {
    resource: Resource,
    route: Route,
    generation: u64,
    reservation: Reservation,
    _life: Arc<()>,
    cancelled: watch::Receiver<bool>,
    size: TextureSize,
    source: Option<Blob>,
    decode_admitted: bool,
}

impl LoadJob {
    pub fn prepare(&self, bytes: Vec<u8>) -> Result<Prepared, Failure> {
        if !self.decode_admitted {
            return Err(Failure::Malformed);
        }
        match &self.source {
            Some(source) => crate::content::prepare_cached(self.resource, self.size, source, || self.is_cancelled()),
            None => crate::content::prepare_cancellable(self.resource, self.size, bytes, || self.is_cancelled()),
        }
    }

    pub fn has_source(&self) -> bool {
        self.source.is_some()
    }

    pub fn retain_response(&mut self, capacity: usize) -> Result<(), Failure> {
        if self.decode_admitted || capacity > self.resource.kind.max_response_bytes() {
            return Err(Failure::TooLarge);
        }
        if !self.reservation.resize(capacity) {
            return Err(Failure::TooLarge);
        }
        Ok(())
    }

    pub fn resource(&self) -> Resource {
        self.resource
    }
    pub fn route(&self) -> Route {
        self.route
    }
    pub fn is_cancelled(&self) -> bool {
        *self.cancelled.borrow()
    }
    pub fn cancelled(&self) -> impl Future<Output = ()> + Send + use<> {
        let mut cancelled = self.cancelled.clone();
        async move {
            if *cancelled.borrow() {
                return;
            }
            let _ = cancelled.changed().await;
        }
    }
}

pub struct Store {
    id: u64,
    sequence: u64,
    routes: BTreeSet<Route>,
    entries: BTreeMap<Resource, AssetEntry>,
    demands: Arc<AtomicUsize>,
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
            demands: Arc::new(AtomicUsize::new(0)),
            budget,
            closed: false,
            textures: TextureEvents::default(),
        }
    }

    pub fn wake(&self) -> watch::Receiver<()> {
        self.budget.wake().subscribe()
    }

    pub fn priority(&self, job: &LoadJob) -> Priority {
        self.entries.get(&job.resource).map_or(Priority::Background, AssetEntry::effective_priority)
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
                counter: Arc::new(Counter { count: AtomicUsize::new(0), total: Arc::clone(&self.demands), wake }),
                status,
                attempts: 0,
                failed: None,
                deadline: now + DEMAND_DEADLINE,
                leases: Vec::new(),
            }
        });
        if view.count() == 0 && view.failed.as_ref().is_none_or(|failed| failed.expires <= now) {
            view.failed = None;
            view.attempts = 0;
            view.deadline = now + DEMAND_DEADLINE;
        }
        let status = match &entry.phase {
            LoadPhase::Idle => ready.as_ref().map_or(Status::Queued, |content| Status::Ready(content.clone())),
            LoadPhase::Running { decoding, .. } => {
                if *decoding {
                    Status::Decoding
                } else {
                    Status::Fetching
                }
            }
            LoadPhase::Queued => {
                view.failed.as_ref().map_or(Status::Queued, |failed| Status::Failed { failure: failed.failure, retry_at: failed.retry_at })
            }
        };
        view.publish_status(status.retaining(ready.as_ref()));
        view.counter.count.fetch_add(1, Ordering::AcqRel);
        self.demands.fetch_add(1, Ordering::AcqRel);
        let lease = Arc::new(Lease { counter: Arc::clone(&view.counter), size: request.size(), priority, _texture: texture });
        view.leases.retain(|lease| lease.strong_count() > 0);
        view.leases.push(Arc::downgrade(&lease));
        Ok(Demand { _lease: lease, status: view.status.subscribe() })
    }

    pub fn maintain(&mut self, now: Instant) {
        for entry in self.entries.values_mut() {
            let ready = entry.ready();
            for view in entry.views.values_mut() {
                view.leases.retain(|lease| lease.strong_count() > 0);
            }
            let desired = entry.requested_size();
            let priority = entry.effective_priority();
            if priority != entry.priority {
                for view in entry.views.values_mut() {
                    if now >= view.deadline && view.failed.as_ref().is_none_or(|failed| failed.failure == Failure::Deadline) {
                        view.deadline = now + DEMAND_DEADLINE;
                        view.failed = None;
                        view.attempts = 0;
                    }
                }
            }
            entry.priority = priority;
            if entry.target != desired {
                entry.target = desired;
                if entry.ready().is_some() {
                    for view in entry.views.values_mut() {
                        view.failed = None;
                        view.attempts = 0;
                        view.deadline = now + DEMAND_DEADLINE;
                    }
                }
                if !matches!(entry.phase, LoadPhase::Running { .. }) {
                    entry.phase = LoadPhase::Queued;
                }
            }
            if matches!(entry.phase, LoadPhase::Queued)
                && let Some(content) = entry.ready()
            {
                let satisfied = match &content {
                    Content::Texture(image) => {
                        desired.is_none_or(|size| image.info().is_some_and(|info| info.discard == size.discard_level(info.source_size)))
                    }
                    Content::Material(_) | Content::Environment(_) => true,
                };
                if satisfied {
                    entry.phase = LoadPhase::Idle;
                    entry.broadcast_status(Status::Ready(content.clone()));
                }
            }
            if let LoadPhase::Running { life, cancelled, route, .. } = &entry.phase {
                if !entry.has_active_demand() || entry.views.get(route).is_none_or(|view| now >= view.deadline) {
                    cancelled.send_replace(true);
                }
                if life.upgrade().is_none() {
                    if let Some(view) = entry.views.get_mut(route) {
                        view.failed = Some(FailureRecord { failure: Failure::Cancelled, retry_at: None, expires: now + NEGATIVE_TTL });
                        view.publish_status(Status::Failed { failure: Failure::Cancelled, retry_at: None }.retaining(ready.as_ref()));
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
                    view.failed = Some(FailureRecord { failure: Failure::Deadline, retry_at: None, expires: now + NEGATIVE_TTL });
                    view.publish_status(Status::Failed { failure: Failure::Deadline, retry_at: None }.retaining(ready.as_ref()));
                }
            }
        }
        self.entries
            .retain(|_, entry| entry.has_active_demand() || entry.texture().is_some() || matches!(entry.phase, LoadPhase::Running { .. }));
    }

    pub fn next_job(&mut self, now: Instant) -> Option<LoadJob> {
        self.maintain(now);
        if self.closed
            || self.entries.values().filter(|entry| matches!(entry.phase, LoadPhase::Running { decoding: false, .. })).count()
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
        let headroom = if self.entries.values().any(|entry| matches!(entry.phase, LoadPhase::Running { decoding: true, .. })) {
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
        entry.phase = LoadPhase::Running { generation, route, life: Arc::downgrade(&life), cancelled, decoding: false };
        entry.broadcast_status(Status::Fetching);
        Some(LoadJob {
            resource,
            route,
            generation,
            reservation,
            _life: life,
            cancelled: receiver,
            size: entry.target.unwrap_or_default(),
            source: entry.texture().and_then(|texture| texture.source()),
            decode_admitted: false,
        })
    }

    pub fn try_start_decode(&mut self, job: &mut LoadJob) -> bool {
        if self.closed || job.is_cancelled() || job.route.store != self.id {
            return false;
        }
        if self.entries.values().filter(|entry| matches!(entry.phase, LoadPhase::Running { decoding: true, .. })).count()
            >= MAX_ASSET_DECODES
        {
            return false;
        }
        if !self.entries.get(&job.resource).is_some_and(
            |entry| matches!(entry.phase, LoadPhase::Running { generation, decoding: false, .. } if generation == job.generation),
        ) {
            return false;
        }
        if !job.reservation.resize(job.resource.kind.work_bytes()) {
            return false;
        }
        let entry = self.entries.get_mut(&job.resource).expect("validated running job");
        let LoadPhase::Running { decoding, .. } = &mut entry.phase else { unreachable!("validated running phase") };
        *decoding = true;
        job.decode_admitted = true;
        entry.broadcast_status(Status::Decoding);
        true
    }

    pub fn complete(&mut self, job: LoadJob, result: Result<Prepared, Failure>, now: Instant) {
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
        match result.and_then(|prepared| prepared.finish(job.resource, job.size, job.reservation)) {
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
                let satisfied = match &content {
                    Content::Texture(image) => entry
                        .requested_size()
                        .is_none_or(|size| image.info().is_some_and(|info| info.discard == size.discard_level(info.source_size))),
                    Content::Material(_) | Content::Environment(_) => true,
                };
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
                view.failed = Some(FailureRecord { failure, retry_at, expires: now + NEGATIVE_TTL });
                view.publish_status(match ready.as_ref() {
                    Some(content) => Status::RefinementFailed { content: content.clone(), failure },
                    None => Status::Failed { failure, retry_at },
                });
                for (route, view) in &entry.views {
                    if *route != job.route && view.count() > 0 {
                        let status = view
                            .failed
                            .as_ref()
                            .map_or(Status::Queued, |failed| Status::Failed { failure: failed.failure, retry_at: failed.retry_at });
                        view.publish_status(status.retaining(entry.ready().as_ref()));
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
