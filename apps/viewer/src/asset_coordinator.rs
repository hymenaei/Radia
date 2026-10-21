use std::{
    collections::HashMap,
    future::pending,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use assets::{AdmissionError, Budget, Demand, Failure, Kind, LoadJob, MemoryStats, Prepared, Priority, Route, Store, legacy_request};
use futures::{FutureExt, StreamExt, channel::oneshot, pin_mut, stream::FuturesUnordered};
use network::{
    Error,
    capabilities::{CapabilityUrl, Client, HttpError},
};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{OnceCell, mpsc};
use tokio::{
    runtime::Handle,
    task::{JoinError, JoinHandle},
};
use world::Region;

#[cfg(test)]
#[path = "asset_coordinator_tests.rs"]
mod tests;

const COMMANDS: usize = 64;

#[derive(Clone)]
struct CapabilityTransport {
    client: Client,
    grants: Arc<OnceCell<HashMap<String, CapabilityUrl>>>,
}

impl CapabilityTransport {
    fn new(client: Client) -> Self {
        Self { client, grants: Arc::new(OnceCell::new()) }
    }

    async fn fetch(&self, job: &LoadJob) -> Result<Vec<u8>, Failure> {
        let grants = self
            .grants
            .get_or_try_init(|| async {
                self.client.request_capabilities(&["ViewerAsset", "RenderMaterials"]).await.map_err(classify_network_error)
            })
            .await?;
        let resource = job.resource();
        let limit = resource.kind.max_response_bytes();
        match resource.kind {
            Kind::Texture | Kind::PbrMaterial | Kind::Environment => {
                let grant = grants.get("ViewerAsset").ok_or(Failure::MissingCapability)?;
                let field = match resource.kind {
                    Kind::Texture => "texture_id",
                    Kind::PbrMaterial => "material_id",
                    Kind::Environment => "settings_id",
                    Kind::LegacyMaterial => unreachable!("separate capability"),
                };
                let url = grant.with_trailing_slash().with_query_pair(field, &resource.id.uuid().to_string());
                self.client.get_bytes_with_response_limit(&url, limit).await.map_err(classify_network_error)
            }
            Kind::LegacyMaterial => {
                let grant = grants.get("RenderMaterials").ok_or(Failure::MissingCapability)?;
                let request = legacy_request(resource.id)?;
                self.client.post_bytes_with_response_limit(grant, &request, limit).await.map_err(classify_network_error)
            }
        }
    }
}

fn classify_network_error(error: Error) -> Failure {
    match error {
        Error::CapabilitySessionClosed | Error::SessionClosed => Failure::RouteClosed,
        Error::CapabilityRequestCanceled => Failure::Cancelled,
        Error::CapabilityHttp(error) => match error {
            HttpError::HttpStatus { status, retry_after, .. } => match status.as_u16() {
                401 | 403 => Failure::Denied,
                404 | 410 => Failure::NotFound,
                408 | 429 | 500..=599 => Failure::Transient { retry_after: retry_after.unwrap_or_default() },
                _ => Failure::Malformed,
            },
            HttpError::Transport { retry_after, .. } | HttpError::Timeout { retry_after, .. } => {
                Failure::Transient { retry_after: retry_after.unwrap_or_default() }
            }
            HttpError::ResourceExhausted { .. } => Failure::TooLarge,
            HttpError::Cancelled { .. } => Failure::Cancelled,
            _ => Failure::Malformed,
        },
        _ => Failure::Malformed,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    Busy,
    Closed,
    NoRoute,
    Admission(AdmissionError),
}

struct DemandRequest {
    region: Region,
    resource: assets::Request,
    priority: Priority,
    reply: oneshot::Sender<Result<Demand, RequestError>>,
}
enum RouteCommand {
    Register(Region, Client),
    Retire(Region),
}

#[derive(Clone)]
pub struct AssetHandle {
    requests: mpsc::Sender<DemandRequest>,
    active: Arc<AtomicBool>,
    runtime: Handle,
    budget: Budget,
    textures: assets::TextureEvents,
}

impl AssetHandle {
    pub fn texture_events(&self) -> tokio::sync::watch::Receiver<u64> {
        self.textures.subscribe()
    }

    pub async fn request(&self, region: Region, resource: assets::Request, priority: Priority) -> Result<Demand, RequestError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(RequestError::Closed);
        }
        let (reply, response) = oneshot::channel();
        self.requests.try_send(DemandRequest { region, resource, priority, reply }).map_err(|error| match error {
            TrySendError::Closed(_) => RequestError::Closed,
            TrySendError::Full(_) if self.active.load(Ordering::Acquire) => RequestError::Busy,
            TrySendError::Full(_) => RequestError::Closed,
        })?;
        let response = response.fuse();
        let deadline = {
            let _entered = self.runtime.enter();
            tokio::time::sleep(Duration::from_secs(1))
        }
        .fuse();
        pin_mut!(response, deadline);
        let result = futures::select_biased! {
            result = response => result.map_err(|_| RequestError::Closed)?,
            _ = deadline => return Err(if self.active.load(Ordering::Acquire) { RequestError::Busy } else { RequestError::Closed }),
        };
        if !self.active.load(Ordering::Acquire) {
            return Err(RequestError::Closed);
        }
        result
    }

    pub fn memory_stats(&self) -> MemoryStats {
        self.budget.stats()
    }
}

pub(crate) struct AssetCoordinator {
    handle: AssetHandle,
    controls: mpsc::Sender<RouteCommand>,
    worker: JoinHandle<()>,
}

impl AssetCoordinator {
    pub(crate) fn new(runtime: Handle, cancelled: impl Future<Output = ()> + Send + 'static) -> Self {
        static MEMORY: OnceLock<Budget> = OnceLock::new();
        let budget = MEMORY.get_or_init(Budget::default).clone();
        Self::with_budget(runtime, cancelled, budget)
    }

    fn with_budget(runtime: Handle, cancelled: impl Future<Output = ()> + Send + 'static, budget: Budget) -> Self {
        let (requests, receiver) = mpsc::channel(COMMANDS);
        let (controls, control_receiver) = mpsc::channel(COMMANDS);
        let active = Arc::new(AtomicBool::new(true));
        let store = Store::new(budget.clone());
        let handle =
            AssetHandle { requests, active: Arc::clone(&active), runtime: runtime.clone(), budget, textures: store.texture_events() };
        let worker = runtime.clone().spawn(run(store, receiver, control_receiver, active, runtime, cancelled));
        Self { handle, controls, worker }
    }

    pub(crate) async fn stop(self) {
        let Self { handle, controls, worker } = self;
        handle.active.store(false, Ordering::Release);
        drop((handle, controls));
        if let Err(error) = worker.await {
            tracing::error!(%error, "asset coordinator stopped unexpectedly");
        }
    }

    pub(crate) fn handle(&self) -> AssetHandle {
        self.handle.clone()
    }

    pub(crate) fn register(&self, region: Region, client: Client) {
        self.control(RouteCommand::Register(region, client));
    }
    pub(crate) fn retire(&self, region: Region) {
        self.control(RouteCommand::Retire(region));
    }

    fn control(&self, control: RouteCommand) {
        if self.handle.active.load(Ordering::Acquire) && self.controls.try_send(control).is_err() {
            self.handle.active.store(false, Ordering::Release);
            tracing::warn!("asset route control overloaded; asset service stopped");
        }
    }
}

enum LoadCompletion {
    Fetched(LoadJob, Result<Vec<u8>, Failure>),
    Prepared(LoadJob, Result<Prepared, Failure>),
}

async fn run(
    mut store: Store,
    mut requests: mpsc::Receiver<DemandRequest>,
    mut controls: mpsc::Receiver<RouteCommand>,
    active: Arc<AtomicBool>,
    runtime: Handle,
    cancelled: impl Future<Output = ()>,
) {
    let cancelled = cancelled.fuse();
    pin_mut!(cancelled);
    let mut wake = store.wake();
    let mut regions = HashMap::<Region, Route>::new();
    let mut transports = HashMap::<Route, CapabilityTransport>::new();
    let mut jobs = FuturesUnordered::<JoinHandle<LoadCompletion>>::new();
    let mut waiting = Vec::<(LoadJob, Vec<u8>)>::new();
    loop {
        if !active.load(Ordering::Acquire) {
            break;
        }
        let now = Instant::now();
        store.maintain(now);
        while let Some(job) = store.next_job(now) {
            if job.has_source() {
                waiting.push((job, Vec::new()));
                continue;
            }
            let transport = transports.get(&job.route()).cloned();
            jobs.push(runtime.spawn(async move {
                let Some(transport) = transport else { return LoadCompletion::Fetched(job, Err(Failure::RouteClosed)) };
                let result = {
                    let fetched = transport.fetch(&job).fuse();
                    let cancelled = job.cancelled().fuse();
                    pin_mut!(fetched, cancelled);
                    futures::select_biased! {
                        _ = cancelled => Err(Failure::Cancelled),
                        result = fetched => result,
                    }
                };
                if job.is_cancelled() {
                    LoadCompletion::Fetched(job, Err(Failure::Cancelled))
                } else {
                    LoadCompletion::Fetched(job, result)
                }
            }));
        }
        waiting.sort_by_key(|(job, _)| std::cmp::Reverse(store.priority(job)));
        let mut index = 0;
        while index < waiting.len() {
            if waiting[index].0.is_cancelled() {
                let (job, _) = waiting.remove(index);
                store.complete(job, Err(Failure::Cancelled), now);
            } else if store.try_start_decode(&mut waiting[index].0) {
                let (job, bytes) = waiting.remove(index);
                jobs.push(runtime.spawn_blocking(move || {
                    let result = job.prepare(bytes);
                    LoadCompletion::Prepared(job, result)
                }));
            } else {
                index += 1;
            }
        }
        enum Input {
            Cancel,
            Control(Option<RouteCommand>),
            Request(Option<DemandRequest>),
            Job(Result<LoadCompletion, JoinError>),
            Wake,
        }
        let input = {
            let control = controls.recv().fuse();
            let request = requests.recv().fuse();
            let completion = async { if jobs.is_empty() { pending().await } else { jobs.next().await.expect("active asset jobs") } }.fuse();
            let notified = wake.changed().fuse();
            let delay = store.next_deadline(now).map(|at| at.saturating_duration_since(now));
            let timer = async {
                match delay {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => pending().await,
                }
            }
            .fuse();
            pin_mut!(control, request, completion, notified, timer);
            futures::select_biased! {
                _ = cancelled => Input::Cancel,
                control = control => Input::Control(control),
                completion = completion => Input::Job(completion),
                request = request => Input::Request(request),
                _ = notified => Input::Wake,
                _ = timer => Input::Wake,
            }
        };
        match input {
            Input::Cancel | Input::Control(None) | Input::Request(None) => break,
            Input::Job(Err(error)) => {
                tracing::error!(%error, "asset worker stopped unexpectedly");
                break;
            }
            Input::Wake => {}
            Input::Control(Some(RouteCommand::Register(region, client))) => {
                if regions.contains_key(&region) {
                    continue;
                }
                if let Ok(route) = store.register_route() {
                    regions.insert(region, route);
                    transports.insert(route, CapabilityTransport::new(client));
                }
            }
            Input::Control(Some(RouteCommand::Retire(region))) => {
                if let Some(route) = regions.remove(&region) {
                    store.retire_route(route);
                    transports.remove(&route);
                }
            }
            Input::Request(Some(DemandRequest { region, resource, priority, reply })) => {
                if reply.is_canceled() {
                    continue;
                }
                let result = regions
                    .get(&region)
                    .ok_or(RequestError::NoRoute)
                    .and_then(|route| store.request(*route, resource, priority, Instant::now()).map_err(RequestError::Admission));
                let _ = reply.send(result);
            }
            Input::Job(Ok(LoadCompletion::Fetched(mut job, Ok(bytes)))) if !job.is_cancelled() => {
                match job.retain_response(bytes.capacity()) {
                    Ok(()) => waiting.push((job, bytes)),
                    Err(failure) => store.complete(job, Err(failure), Instant::now()),
                }
            }
            Input::Job(Ok(LoadCompletion::Fetched(job, result))) => {
                let result = result.and(Err(Failure::Cancelled));
                store.complete(job, result, Instant::now());
            }
            Input::Job(Ok(LoadCompletion::Prepared(job, result))) => store.complete(job, result, Instant::now()),
        }
    }
    active.store(false, Ordering::Release);
    store.close();
    drop(waiting);
    requests.close();
    controls.close();
    while let Some(result) = jobs.next().await {
        if let Err(error) = result {
            tracing::error!(%error, "retiring asset worker stopped unexpectedly");
        }
    }
}
