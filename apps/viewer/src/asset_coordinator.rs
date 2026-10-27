use std::{
    collections::{HashMap, VecDeque},
    future::pending,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use assets::{
    AdmissionError, Budget, CacheReadCompletion, DecodeInput, Demand, Failure, FetchJob, FetchResponse, Kind, LoadCompletion, LoadJob,
    MemoryStats, Priority, Request, Route, Store,
    cache::{AssetCache, AssetCacheStats, CacheWrite},
    legacy_request,
};
use futures::{FutureExt, StreamExt, channel::oneshot, pin_mut, stream::FuturesUnordered};
use network::{
    Error,
    capabilities::{CapabilityUrl, Client, ContentRange, HttpErrorKind, RangeResponse, RedirectPolicy},
};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{OnceCell, mpsc, watch};
use tokio::{
    runtime::Handle,
    task::{JoinError, JoinHandle},
};
use tracing::Instrument;
use utils::AssetId;
use world::Region;

const COMMANDS: usize = 64;
static UNEXPECTED_HTTP_STATUS_WARNED: AtomicU8 = AtomicU8::new(0);

#[derive(Clone)]
struct CapabilityTransport {
    client: Client,
    grants: Arc<OnceCell<HashMap<String, CapabilityUrl>>>,
}

impl CapabilityTransport {
    fn new(client: Client) -> Self {
        Self { client, grants: Arc::new(OnceCell::new()) }
    }

    async fn fetch(&self, job: &FetchJob) -> Result<FetchResponse, Failure> {
        let resource = job.resource();
        let request = job.request();
        let grants = self
            .grants
            .get_or_try_init(|| async {
                self.client
                    .request_capabilities(&["ViewerAsset", "RenderMaterials"])
                    .await
                    .map_err(|error| classify_network_error(error, resource.kind))
            })
            .await?;
        let limit = resource.kind.max_response_bytes();
        match resource.kind {
            Kind::Texture | Kind::PbrMaterial | Kind::Environment | Kind::Mesh => {
                let grant = grants.get("ViewerAsset").ok_or(Failure::MissingCapability)?;
                let field = match resource.kind {
                    Kind::Texture => "texture_id",
                    Kind::PbrMaterial => "material_id",
                    Kind::Environment => "settings_id",
                    Kind::Mesh => "mesh_id",
                    Kind::LegacyMaterial => unreachable!("separate capability"),
                };
                let url = grant.with_trailing_slash().with_query_pair(field, &resource.id.uuid().to_string());
                if resource.kind == Kind::Mesh {
                    return self.fetch_mesh(&url, request, limit).await;
                }
                self.client
                    .get_bytes_with_response_limit_and_redirect_policy(&url, limit, RedirectPolicy::ViewerAsset)
                    .await
                    .map_err(|error| classify_network_error(error, resource.kind))
                    .map(FetchResponse::Full)
            }
            Kind::LegacyMaterial => {
                let grant = grants.get("RenderMaterials").ok_or(Failure::MissingCapability)?;
                let request = legacy_request(resource.id)?;
                self.client
                    .post_bytes_with_response_limit(grant, &request, limit)
                    .await
                    .map_err(|error| classify_network_error(error, resource.kind))
                    .map(FetchResponse::Full)
            }
        }
    }

    async fn fetch_mesh(&self, url: &CapabilityUrl, request: Request, limit: usize) -> Result<FetchResponse, Failure> {
        match request {
            Request::MeshHeader(_) => self.fetch_mesh_header(url, limit).await,
            Request::MeshLod(block) => self.fetch_mesh_lod(url, block, limit).await,
            _ => Err(Failure::MalformedBlock),
        }
    }

    async fn fetch_mesh_header(&self, url: &CapabilityUrl, limit: usize) -> Result<FetchResponse, Failure> {
        let max_header = u64::try_from(assets::mesh::MAX_MESH_HEADER_BYTES).map_err(|_| Failure::TooLarge)?;
        let mut range_end = u64::try_from(assets::mesh::MESH_HEADER_BYTES).map_err(|_| Failure::TooLarge)?;
        loop {
            let response = self
                .client
                .get_range_with_response_limit_and_redirect_policy(url, 0..range_end, limit, RedirectPolicy::ViewerAsset)
                .await
                .map_err(|error| classify_network_error(error, Kind::Mesh))?;
            let (bytes, content_range) = match response {
                RangeResponse::Full(bytes) => return Ok(FetchResponse::Full(bytes)),
                RangeResponse::Partial { bytes, content_range: Some(content_range) } => (bytes, content_range),
                RangeResponse::Partial { .. } => return self.fetch_mesh_full(url, limit).await,
            };
            let Some(total) = content_range.total else {
                return self.fetch_mesh_full(url, limit).await;
            };
            let expected_end = total.min(range_end);
            if content_range.start != 0
                || content_range.end.checked_add(1) != Some(expected_end)
                || u64::try_from(bytes.len()).ok() != Some(expected_end)
            {
                return Err(Failure::MalformedBlock);
            }
            if assets::mesh::header_needs_more(&bytes) {
                if expected_end == total {
                    return Err(Failure::MalformedBlock);
                }
                if range_end >= max_header {
                    return Err(Failure::TooLarge);
                }
                range_end = range_end.checked_mul(2).ok_or(Failure::TooLarge)?.min(max_header).min(total);
                if range_end <= expected_end {
                    return Err(Failure::MalformedBlock);
                }
                continue;
            }
            return Ok(FetchResponse::Partial { bytes, range: 0..expected_end, total_length: Some(total) });
        }
    }

    async fn fetch_mesh_full(&self, url: &CapabilityUrl, limit: usize) -> Result<FetchResponse, Failure> {
        self.client
            .get_bytes_with_response_limit_and_redirect_policy(url, limit, RedirectPolicy::ViewerAsset)
            .await
            .map(FetchResponse::Full)
            .map_err(|error| classify_network_error(error, Kind::Mesh))
    }

    async fn fetch_mesh_lod(
        &self,
        url: &CapabilityUrl,
        block: assets::mesh::MeshLodRequest,
        limit: usize,
    ) -> Result<FetchResponse, Failure> {
        let requested = block.range();
        let response = self
            .client
            .get_range_with_response_limit_and_redirect_policy(url, requested.clone(), limit, RedirectPolicy::ViewerAsset)
            .await
            .map_err(|error| classify_network_error(error, Kind::Mesh))?;
        match response {
            RangeResponse::Full(bytes) => Ok(FetchResponse::Full(bytes)),
            RangeResponse::Partial { bytes, content_range: Some(content_range) } => {
                if !content_range_matches(content_range, &requested, block.asset_length(), bytes.len()) {
                    return Err(Failure::MalformedBlock);
                }
                Ok(FetchResponse::Partial { bytes, range: requested, total_length: Some(block.asset_length()) })
            }
            RangeResponse::Partial { bytes, content_range: None } => {
                if u64::try_from(bytes.len()).ok() != Some(requested.end - requested.start) {
                    return Err(Failure::MalformedBlock);
                }
                Ok(FetchResponse::Partial { bytes, range: requested, total_length: Some(block.asset_length()) })
            }
        }
    }
}

fn content_range_matches(content_range: ContentRange, requested: &std::ops::Range<u64>, total: u64, body_len: usize) -> bool {
    content_range.start == requested.start
        && content_range.end.checked_add(1) == Some(requested.end)
        && content_range.total.is_none_or(|length| length == total)
        && u64::try_from(body_len).ok() == Some(requested.end - requested.start)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestError {
    Busy,
    Closed,
    Admission(AdmissionError),
}

struct DemandRequest {
    region: Region,
    resource: assets::Request,
    priority: Priority,
    reply: oneshot::Sender<Result<Option<Demand>, RequestError>>,
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
    routes: watch::Receiver<()>,
    cache: Option<Arc<AssetCache>>,
}

impl AssetHandle {
    pub fn texture_events(&self) -> watch::Receiver<u64> {
        self.textures.subscribe()
    }

    pub async fn request(&self, region: Region, resource: assets::Request, priority: Priority) -> Result<Demand, RequestError> {
        let mut routes = self.routes.clone();
        routes.borrow_and_update();
        loop {
            if let Some(demand) = self.request_once(region, resource, priority).await? {
                return Ok(demand);
            }
            routes.changed().await.map_err(|_| RequestError::Closed)?;
        }
    }

    pub async fn request_mesh_header(&self, region: Region, id: AssetId, priority: Priority) -> Result<Demand, RequestError> {
        self.request(region, Request::MeshHeader(id), priority).await
    }

    pub async fn request_mesh_lod(
        &self,
        region: Region,
        block: assets::mesh::MeshLodRequest,
        priority: Priority,
    ) -> Result<Demand, RequestError> {
        self.request(region, Request::MeshLod(block), priority).await
    }

    async fn request_once(&self, region: Region, resource: assets::Request, priority: Priority) -> Result<Option<Demand>, RequestError> {
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

    pub fn cache_stats(&self) -> Option<AssetCacheStats> {
        self.cache.as_ref().map(|cache| cache.stats())
    }
}

pub(crate) struct AssetCoordinator {
    handle: AssetHandle,
    controls: mpsc::Sender<RouteCommand>,
    worker: JoinHandle<()>,
}

impl AssetCoordinator {
    pub(crate) fn new(runtime: Handle, cancelled: impl Future<Output = ()> + Send + 'static, cache: Option<Arc<AssetCache>>) -> Self {
        static MEMORY: OnceLock<Budget> = OnceLock::new();
        let budget = MEMORY.get_or_init(Budget::default).clone();
        Self::build(runtime, cancelled, budget, cache, Box::new(spawn_cache_writes), Box::new(|| {}))
    }

    #[cfg(test)]
    fn with_budget(
        runtime: Handle,
        cancelled: impl Future<Output = ()> + Send + 'static,
        budget: Budget,
        cache: Option<Arc<AssetCache>>,
    ) -> Self {
        Self::build(runtime, cancelled, budget, cache, Box::new(spawn_cache_writes), Box::new(|| {}))
    }

    fn build(
        runtime: Handle,
        cancelled: impl Future<Output = ()> + Send + 'static,
        budget: Budget,
        cache: Option<Arc<AssetCache>>,
        write_spawner: Box<CacheWriteSpawner>,
        shutdown_drain_hook: Box<dyn FnOnce() + Send>,
    ) -> Self {
        let (requests, receiver) = mpsc::channel(COMMANDS);
        let (controls, control_receiver) = mpsc::channel(COMMANDS);
        let (routes, route_events) = watch::channel(());
        let active = Arc::new(AtomicBool::new(true));
        let store = cache.as_ref().map_or_else(|| Store::new(budget.clone()), |cache| Store::with_cache(budget.clone(), Arc::clone(cache)));
        let handle = AssetHandle {
            requests,
            active: Arc::clone(&active),
            runtime: runtime.clone(),
            budget,
            textures: store.texture_events(),
            routes: route_events,
            cache,
        };
        let worker = runtime.clone().spawn(
            run(store, receiver, control_receiver, routes, active, runtime, cancelled, write_spawner, shutdown_drain_hook)
                .in_current_span(),
        );
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

enum WorkerResult {
    Input(DecodeInput),
    Complete(LoadCompletion),
    CacheRead(CacheReadCompletion),
    CacheWritten,
}

type CacheWriteSpawner = dyn for<'a, 'b> FnMut(Vec<CacheWrite>, &'a Handle, &'b mut FuturesUnordered<JoinHandle<WorkerResult>>) + Send;

fn classify_network_error(error: Error, kind: Kind) -> Failure {
    match error {
        Error::CapabilitySessionClosed | Error::SessionClosed => Failure::RouteClosed,
        Error::CapabilityRequestCanceled => Failure::Cancelled,
        Error::CapabilityHttp(error) => match error.kind {
            HttpErrorKind::HttpStatus => match error.status.map(|status| status.as_u16()) {
                Some(401 | 403) => Failure::Denied,
                Some(404 | 410) => Failure::NotFound,
                Some(416) if kind == Kind::Mesh => Failure::MalformedBlock,
                Some(408 | 429 | 500..=599) => Failure::Transient { retry_after: error.retry_after.unwrap_or_default() },
                Some(status) => {
                    warn_unexpected_http_status(kind, status);
                    Failure::HttpStatus { status }
                }
                None => Failure::Network,
            },
            HttpErrorKind::Transport | HttpErrorKind::Timeout => Failure::Transient { retry_after: error.retry_after.unwrap_or_default() },
            HttpErrorKind::ResourceExhausted => Failure::TooLarge,
            HttpErrorKind::InvalidResponse => Failure::MalformedBlock,
            HttpErrorKind::Cancelled => Failure::Cancelled,
            HttpErrorKind::InvalidUrl => Failure::Network,
            _ => Failure::Network,
        },
        _ => Failure::Network,
    }
}

fn warn_unexpected_http_status(kind: Kind, status: u16) {
    let bit = match kind {
        Kind::Texture => 1,
        Kind::PbrMaterial => 2,
        Kind::LegacyMaterial => 4,
        Kind::Environment => 8,
        Kind::Mesh => 16,
    };
    if UNEXPECTED_HTTP_STATUS_WARNED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        tracing::warn!(kind = ?kind, http_status = status, "asset request returned an unexpected HTTP status");
    }
}

#[tracing::instrument(name = "assets.service", level = "debug", skip_all)]
async fn run(
    mut store: Store,
    mut requests: mpsc::Receiver<DemandRequest>,
    mut controls: mpsc::Receiver<RouteCommand>,
    routes: watch::Sender<()>,
    active: Arc<AtomicBool>,
    runtime: Handle,
    cancelled: impl Future<Output = ()>,
    mut write_spawner: Box<CacheWriteSpawner>,
    shutdown_drain_hook: Box<dyn FnOnce() + Send>,
) {
    let cancelled = cancelled.fuse();
    pin_mut!(cancelled);
    let mut wake = store.wake();
    let mut regions = HashMap::<Region, Route>::new();
    let mut transports = HashMap::<Route, CapabilityTransport>::new();
    let mut jobs = FuturesUnordered::<JoinHandle<WorkerResult>>::new();
    let mut waiting = VecDeque::<DecodeInput>::new();
    let mut fetches = VecDeque::<FetchJob>::new();
    loop {
        if !active.load(Ordering::Acquire) {
            break;
        }
        let now = Instant::now();
        store.maintain(now);
        while let Some(job) = fetches.pop_front() {
            spawn_fetch(job, &transports, &runtime, &mut jobs);
        }
        while let Some(job) = store.next_job(now) {
            match job {
                LoadJob::Decode(input) => {
                    waiting.push_back(input);
                    continue;
                }
                LoadJob::Fetch(job) => fetches.push_back(job),
                LoadJob::ReadCache(job) => {
                    jobs.push(runtime.spawn_blocking(move || WorkerResult::CacheRead(job.run())));
                }
            }
        }
        while let Some(job) = fetches.pop_front() {
            spawn_fetch(job, &transports, &runtime, &mut jobs);
        }
        waiting.make_contiguous().sort_by_key(|input| std::cmp::Reverse(store.priority(input)));
        for _ in 0..waiting.len() {
            let input = waiting.pop_front().expect("queued decode input");
            if input.is_cancelled() {
                store.complete(input.fail(Failure::Cancelled), now);
                continue;
            }
            let job = match store.start_decode(input) {
                Ok(job) => job,
                Err(input) => {
                    waiting.push_back(input);
                    continue;
                }
            };
            let span = tracing::debug_span!("assets.decode", asset = ?job.resource().id, kind = ?job.resource().kind);
            jobs.push(runtime.spawn_blocking(move || {
                let _span = span.enter();
                let completion = job.run();
                if let Some(failure) = completion.failure() {
                    tracing::debug!(?failure, "asset preparation rejected");
                }
                WorkerResult::Complete(completion)
            }));
        }
        enum Input {
            Cancel,
            Control(Option<RouteCommand>),
            Request(Option<DemandRequest>),
            Job(Result<WorkerResult, JoinError>),
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
                match store.register_route() {
                    Ok(route) => {
                        regions.insert(region, route);
                        transports.insert(route, CapabilityTransport::new(client));
                        routes.send_replace(());
                    }
                    Err(error) => tracing::warn!(region = region.get(), ?error, "could not register asset route"),
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
                    .map(|route| store.request(*route, resource, priority, Instant::now()).map_err(RequestError::Admission))
                    .transpose();
                if reply.send(result).is_err() {
                    tracing::trace!(region = region.get(), "asset request receiver closed");
                }
            }
            Input::Job(Ok(WorkerResult::Input(input))) => waiting.push_back(input),
            Input::Job(Ok(WorkerResult::Complete(completion))) => {
                write_spawner(store.complete(completion, Instant::now()), &runtime, &mut jobs);
            }
            Input::Job(Ok(WorkerResult::CacheRead(completion))) => match store.complete_cache_read(completion, Instant::now()) {
                Some(LoadJob::Decode(input)) => waiting.push_back(input),
                Some(LoadJob::Fetch(job)) => fetches.push_back(job),
                Some(LoadJob::ReadCache(_)) => unreachable!("cache reads cannot schedule another cache read"),
                None => {}
            },
            Input::Job(Ok(WorkerResult::CacheWritten)) => {}
        }
    }
    active.store(false, Ordering::Release);
    store.close();
    drop(routes);
    drop(waiting);
    requests.close();
    controls.close();
    shutdown_drain_hook();
    while let Some(result) = jobs.next().await {
        if let Err(error) = result {
            tracing::error!(%error, "retiring asset worker stopped unexpectedly");
        }
    }
}

fn spawn_fetch(
    job: FetchJob,
    transports: &HashMap<Route, CapabilityTransport>,
    runtime: &Handle,
    jobs: &mut FuturesUnordered<JoinHandle<WorkerResult>>,
) {
    let transport = transports.get(&job.route()).cloned();
    let span = tracing::debug_span!("assets.fetch", asset = ?job.resource().id, kind = ?job.resource().kind, route = ?job.route());
    jobs.push(
        runtime.spawn(
            async move {
                let result = match transport {
                    Some(transport) => {
                        let fetched = transport.fetch(&job).fuse();
                        let cancelled = job.cancelled().fuse();
                        pin_mut!(fetched, cancelled);

                        futures::select_biased! {
                            _ = cancelled => Err(Failure::Cancelled),
                            result = fetched => result,
                        }
                    }
                    None => Err(Failure::RouteClosed),
                };
                if let Err(failure) = &result {
                    tracing::debug!(?failure, "asset fetch did not complete");
                }
                match job.complete_response(result) {
                    Ok(input) => WorkerResult::Input(input),
                    Err(failure) => WorkerResult::Complete(failure.into()),
                }
            }
            .instrument(span),
        ),
    );
}

fn spawn_cache_writes(writes: Vec<CacheWrite>, runtime: &Handle, jobs: &mut FuturesUnordered<JoinHandle<WorkerResult>>) {
    for write in writes {
        jobs.push(runtime.spawn_blocking(move || {
            write.run();
            WorkerResult::CacheWritten
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::{
        assert_matches,
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::Arc,
        thread,
    };

    use assets::Priority;
    use llsd::{Value, binary, xml};
    use tempfile::tempdir;
    use test_support::texture::{RGBA_J2C, RGBA_RAW};
    use url::Url;
    use utils::AssetId;
    use uuid::Uuid;

    use super::*;

    struct Peer {
        url: Url,
        thread: thread::JoinHandle<Vec<Vec<u8>>>,
    }

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut bytes = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0, "request ended before framing completed");
            bytes.extend_from_slice(&chunk[..count]);
            if let Some(header) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let length = String::from_utf8_lossy(&bytes[..header])
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= header + 4 + length {
                    return bytes;
                }
            }
        }
    }

    impl Peer {
        fn new(count: usize, mut respond: impl FnMut(usize, &[u8], &Url) -> (u16, Vec<u8>) + Send + 'static) -> Self {
            Self::new_with_headers(count, move |index, request, base| {
                let (status, body) = respond(index, request, base);
                (status, Vec::new(), body)
            })
        }

        fn new_with_headers(
            count: usize,
            mut respond: impl FnMut(usize, &[u8], &Url) -> (u16, Vec<(String, String)>, Vec<u8>) + Send + 'static,
        ) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            let base = url.clone();
            let thread = thread::spawn(move || {
                let mut requests = Vec::new();
                for index in 0..count {
                    let deadline = Instant::now() + Duration::from_secs(5);
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                                thread::sleep(Duration::from_millis(1))
                            }
                            Err(error) => panic!("expected capability request: {error}"),
                        }
                    };
                    let request = read_request(&mut stream);
                    let (status, headers, body) = respond(index, &request, &base);
                    let mut header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\n", body.len());
                    for (name, value) in headers {
                        header.push_str(&format!("{name}: {value}\r\n"));
                    }
                    header.push_str("Connection: close\r\n\r\n");
                    let _ = stream.write_all(header.as_bytes()).and_then(|_| stream.write_all(&body));
                    requests.push(request);
                }
                requests
            });
            Self { url, thread }
        }
        fn finish(self) -> Vec<Vec<u8>> {
            self.thread.join().unwrap()
        }
    }

    fn grants(base: &Url) -> Vec<u8> {
        xml::to_vec(&Value::Map(
            [
                ("ViewerAsset".into(), Value::String(base.join("content").unwrap().to_string())),
                ("RenderMaterials".into(), Value::String(base.join("materials").unwrap().to_string())),
            ]
            .into(),
        ))
        .unwrap()
    }

    fn resource(kind: Kind) -> assets::Resource {
        assets::Resource { id: AssetId::new(Uuid::from_u128(1)), kind }
    }

    fn mesh_fixture_integer(value: i32) -> Vec<u8> {
        let mut bytes = vec![b'i'];
        bytes.extend_from_slice(&value.to_be_bytes());
        bytes
    }

    fn mesh_fixture_string(value: &[u8]) -> Vec<u8> {
        let mut bytes = vec![b's'];
        bytes.extend_from_slice(&u32::try_from(value.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(value);
        bytes
    }

    fn mesh_fixture_array(values: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = vec![b'['];
        bytes.extend_from_slice(&u32::try_from(values.len()).unwrap().to_be_bytes());
        for value in values {
            bytes.extend_from_slice(value);
        }
        bytes.push(b']');
        bytes
    }

    fn mesh_fixture_map(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut bytes = vec![b'{'];
        bytes.extend_from_slice(&u32::try_from(entries.len()).unwrap().to_be_bytes());
        for (key, value) in entries {
            bytes.push(b'k');
            bytes.extend_from_slice(&u32::try_from(key.len()).unwrap().to_be_bytes());
            bytes.extend_from_slice(key.as_bytes());
            bytes.extend_from_slice(value);
        }
        bytes.push(b'}');
        bytes
    }

    fn mesh_fixture_block_entry(offset: i32, size: i32) -> Vec<u8> {
        mesh_fixture_map(&[("offset", mesh_fixture_integer(offset)), ("size", mesh_fixture_integer(size))])
    }

    fn mesh_fixture_lod_request(id: AssetId) -> assets::mesh::MeshLodRequest {
        let mut bytes = b"<?llsd/binary?>\n".to_vec();
        bytes.extend(mesh_fixture_map(&[
            ("version", mesh_fixture_integer(1)),
            ("lowest_lod", mesh_fixture_block_entry(-1, 0)),
            ("low_lod", mesh_fixture_block_entry(0, 1)),
            ("medium_lod", mesh_fixture_block_entry(-1, 0)),
            ("high_lod", mesh_fixture_block_entry(-1, 0)),
            ("physics_mesh", mesh_fixture_block_entry(-1, 0)),
            ("skin", mesh_fixture_block_entry(-1, 0)),
        ]));
        let total = u64::try_from(bytes.len() + 1).unwrap();
        assets::mesh::decode_header(&bytes, total).unwrap().lod(id, assets::mesh::MeshLod::Low).unwrap()
    }

    #[tokio::test]
    async fn capability_adapter_builds_texture_pbr_and_legacy_requests() {
        let peer = Peer::new(4, |index, request, base| {
            let header = String::from_utf8_lossy(request);
            match index {
                0 => {
                    assert!(header.starts_with("POST /seed HTTP/1.1"));
                    (200, grants(base))
                }
                1 => {
                    assert!(header.starts_with("GET /content/?texture_id=00000000-0000-0000-0000-000000000001 HTTP/1.1"));
                    (200, b"texture".to_vec())
                }
                2 => {
                    assert!(header.starts_with("GET /content/?material_id=00000000-0000-0000-0000-000000000001 HTTP/1.1"));
                    (200, b"pbr".to_vec())
                }
                3 => {
                    assert!(header.starts_with("POST /materials HTTP/1.1"));
                    let body = request.windows(4).position(|bytes| bytes == b"\r\n\r\n").unwrap() + 4;
                    let value: Value = xml::from_slice(&request[body..]).unwrap();
                    assert!(matches!(value.as_map().unwrap().get("Zipped"), Some(Value::Binary(_))));
                    (200, b"legacy".to_vec())
                }
                _ => unreachable!(),
            }
        });
        let transport = CapabilityTransport::new(Client::new(peer.url.join("seed").unwrap()).unwrap());
        let mut store = Store::new(Budget::default());
        let route = store.register_route().unwrap();
        for (kind, expected) in
            [(Kind::Texture, b"texture".as_slice()), (Kind::PbrMaterial, b"pbr".as_slice()), (Kind::LegacyMaterial, b"legacy".as_slice())]
        {
            let demand = store.request(route, assets::Request::new(resource(kind)), Priority::Visible, Instant::now()).unwrap();
            let asset_job = store.next_job(Instant::now()).unwrap();
            let LoadJob::Fetch(fetch) = &asset_job else { panic!("new request must start with a fetch") };
            let result = transport.fetch(fetch).await.unwrap();
            assert_matches!(result, FetchResponse::Full(bytes) if bytes.as_slice() == expected);
            drop(asset_job);
            drop(demand);
        }
        assert_eq!(peer.finish().len(), 4, "capability discovery is shared across resource kinds");
    }

    #[tokio::test]
    async fn mesh_header_uses_a_bounded_mesh_id_range_and_accepts_a_full_fallback() {
        let asset = b"full mesh representation".to_vec();
        let response_asset = asset.clone();
        let peer = Peer::new_with_headers(2, move |index, request, base| match index {
            0 => (200, Vec::new(), grants(base)),
            1 => {
                let request = String::from_utf8_lossy(request).to_ascii_lowercase();
                assert!(request.starts_with("get /content/?mesh_id=00000000-0000-0000-0000-000000000001 http/1.1"));
                assert!(request.contains("range: bytes=0-4095"));
                assert!(request.contains("accept-encoding: identity"));
                (200, Vec::new(), response_asset.clone())
            }
            _ => unreachable!(),
        });
        let transport = CapabilityTransport::new(Client::new(peer.url.join("seed").unwrap()).unwrap());
        let mut store = Store::new(Budget::default());
        let route = store.register_route().unwrap();
        let id = resource(Kind::Mesh).id;
        let demand = store.request(route, Request::MeshHeader(id), Priority::Visible, Instant::now()).unwrap();
        let LoadJob::Fetch(fetch) = store.next_job(Instant::now()).unwrap() else { panic!("mesh header starts with a fetch") };

        let result = transport.fetch(&fetch).await.unwrap();
        assert_matches!(result, FetchResponse::Full(bytes) if bytes == asset);
        drop(fetch);
        drop(demand);
        assert_eq!(peer.finish().len(), 2);
    }

    #[tokio::test]
    async fn public_mesh_demands_fetch_and_decode_header_then_lod() {
        let compressed_empty_lod = [0x78, 0x01, 0x01, 0x06, 0x00, 0xf9, 0xff, 0x5b, 0x00, 0x00, 0x00, 0x00, 0x5d, 0x02, 0x85, 0x00, 0xb9];
        let row = mesh_fixture_array(&vec![mesh_fixture_string(&[b'x'; 64]); 24]);
        let padding = mesh_fixture_array(&vec![row; 4]);
        let mut header = b"<?llsd/binary?>\n".to_vec();
        header.extend(mesh_fixture_map(&[
            ("version", mesh_fixture_integer(1)),
            ("lowest_lod", mesh_fixture_block_entry(-1, 0)),
            ("low_lod", mesh_fixture_block_entry(0, i32::try_from(compressed_empty_lod.len()).unwrap())),
            ("medium_lod", mesh_fixture_block_entry(-1, 0)),
            ("high_lod", mesh_fixture_block_entry(-1, 0)),
            ("physics_mesh", mesh_fixture_block_entry(-1, 0)),
            ("skin", mesh_fixture_block_entry(-1, 0)),
            ("padding", padding),
        ]));
        let header_size = header.len();
        assert!(header_size > assets::mesh::MESH_HEADER_BYTES);
        let mut asset = header;
        asset.extend_from_slice(&compressed_empty_lod);
        let asset_length = asset.len();
        let initial_header_response = asset[..assets::mesh::MESH_HEADER_BYTES].to_vec();
        let header_response = asset.clone();
        let lod_response = compressed_empty_lod.to_vec();
        let peer = Peer::new_with_headers(4, move |index, request, base| match index {
            0 => (200, Vec::new(), grants(base)),
            1 => {
                let request = String::from_utf8_lossy(request).to_ascii_lowercase();
                assert!(request.starts_with("get /content/?mesh_id=00000000-0000-0000-0000-000000000001 http/1.1"));
                assert!(request.contains("range: bytes=0-4095"));
                (206, vec![("Content-Range".into(), format!("bytes 0-4095/{asset_length}"))], initial_header_response.clone())
            }
            2 => {
                let request = String::from_utf8_lossy(request).to_ascii_lowercase();
                assert!(request.contains(&format!("range: bytes=0-{}", asset_length - 1)));
                (206, vec![("Content-Range".into(), format!("bytes 0-{}/{}", asset_length - 1, asset_length))], header_response.clone())
            }
            3 => {
                let request = String::from_utf8_lossy(request).to_ascii_lowercase();
                assert!(request.contains(&format!("range: bytes={header_size}-{}", asset_length - 1)));
                (
                    206,
                    vec![("Content-Range".into(), format!("bytes {header_size}-{}/{}", asset_length - 1, asset_length))],
                    lod_response.clone(),
                )
            }
            _ => unreachable!(),
        });
        let (cancel, cancelled) = oneshot::channel();
        let service = AssetCoordinator::with_budget(
            Handle::current(),
            async move {
                let _ = cancelled.await;
            },
            Budget::default(),
            None,
        );
        let region = Region::new(1);
        let id = resource(Kind::Mesh).id;
        service.register(region, Client::new(peer.url.join("seed").unwrap()).unwrap());
        let handle = service.handle();
        let mut header_demand = handle.request_mesh_header(region, id, Priority::Visible).await.unwrap();
        let mesh_header = loop {
            match header_demand.status() {
                assets::Status::Ready(assets::Content::MeshHeader(header)) => break header,
                assets::Status::Failed { failure, .. } => panic!("unexpected mesh header failure: {failure:?}"),
                _ => {
                    header_demand.changed().await;
                }
            }
        };
        let block = mesh_header.lod(id, assets::mesh::MeshLod::Low).unwrap();
        let mut lod_demand = handle.request_mesh_lod(region, block, Priority::Visible).await.unwrap();
        let lod = loop {
            match lod_demand.status() {
                assets::Status::Ready(assets::Content::Mesh(lod)) => break lod,
                assets::Status::Failed { failure, .. } => panic!("unexpected mesh LOD failure: {failure:?}"),
                _ => {
                    lod_demand.changed().await;
                }
            }
        };
        assert_eq!(lod.lod(), assets::mesh::MeshLod::Low);
        assert_matches!(lod.geometry(), assets::mesh::LodGeometry::Empty { face_count: 0 });
        cancel.send(()).unwrap();
        service.stop().await;
        assert_eq!(peer.finish().len(), 4);
        drop(lod);
        drop(lod_demand);
        drop(mesh_header);
        drop(header_demand);
        drop(handle);
    }

    #[tokio::test]
    async fn mesh_lod_range_is_cancelled_when_its_final_demand_drops() {
        let id = resource(Kind::Mesh).id;
        let block = mesh_fixture_lod_request(id);
        let range = block.range();
        let content_range = format!("bytes {}-{}/{}", range.start, range.end - 1, block.asset_length());
        let (request_seen, request_received) = std::sync::mpsc::channel();
        let (release_response, response_released) = std::sync::mpsc::channel();
        let peer = Peer::new_with_headers(2, move |index, _, base| match index {
            0 => (200, Vec::new(), grants(base)),
            1 => {
                request_seen.send(()).unwrap();
                response_released.recv_timeout(Duration::from_secs(5)).expect("test releases the stalled range response");
                (206, vec![("Content-Range".into(), content_range.clone())], b"x".to_vec())
            }
            _ => unreachable!(),
        });
        let transport = CapabilityTransport::new(Client::new(peer.url.join("seed").unwrap()).unwrap());
        let now = Instant::now();
        let mut store = Store::new(Budget::default());
        let route = store.register_route().unwrap();
        let demand = store.request(route, Request::MeshLod(block), Priority::Visible, now).unwrap();
        let LoadJob::Fetch(fetch) = store.next_job(now).unwrap() else { panic!("mesh LOD starts with a fetch") };
        let fetch_task = tokio::spawn(async move {
            let result = tokio::select! {
                _ = fetch.cancelled() => Err(Failure::Cancelled),
                result = transport.fetch(&fetch) => result,
            };
            (fetch, result)
        });

        tokio::task::spawn_blocking(move || request_received.recv_timeout(Duration::from_secs(5)).expect("mesh LOD request reached peer"))
            .await
            .unwrap();
        drop(demand);
        store.maintain(now);
        let (fetch, result) = fetch_task.await.unwrap();
        assert_matches!(result, Err(Failure::Cancelled));
        store.complete(fetch.complete_response(result).unwrap_err().into(), now);
        release_response.send(()).unwrap();

        let requests = peer.finish();
        assert_eq!(requests.len(), 2);
        let request = String::from_utf8_lossy(&requests[1]).to_ascii_lowercase();
        assert!(request.starts_with("get /content/?mesh_id=00000000-0000-0000-0000-000000000001 http/1.1"));
        assert!(request.contains(&format!("range: bytes={}-{}", range.start, range.end - 1)));
        assert!(request.contains("accept-encoding: identity"));
    }

    #[tokio::test]
    async fn viewer_asset_redirect_decodes_library_environment() {
        let settings = include_bytes!("../../../crates/assets/tests/fixtures/settings/Midday.dat").to_vec();
        let peer = Peer::new_with_headers(3, move |index, request, base| {
            let headers = String::from_utf8_lossy(request);
            match index {
                0 => (200, Vec::new(), grants(base)),
                1 => {
                    assert!(headers.starts_with("GET /content/?settings_id=00000000-0000-0000-0000-000000000001 HTTP/1.1"));
                    (302, vec![("Location".into(), base.join("settings").unwrap().to_string())], Vec::new())
                }
                2 => {
                    assert!(headers.starts_with("GET /settings HTTP/1.1"));
                    assert!(!headers.to_ascii_lowercase().contains("referer:"));
                    (200, Vec::new(), settings.clone())
                }
                _ => unreachable!(),
            }
        });
        let (cancel, cancelled) = oneshot::channel();
        let service = AssetCoordinator::with_budget(
            Handle::current(),
            async move {
                let _ = cancelled.await;
            },
            Budget::default(),
            None,
        );
        let region = Region::new(1);
        service.register(region, Client::new(peer.url.join("seed").unwrap()).unwrap());
        let handle = service.handle();
        let mut demand = handle.request(region, assets::Request::new(resource(Kind::Environment)), Priority::Visible).await.unwrap();
        loop {
            match demand.status() {
                assets::Status::Ready(assets::Content::Environment(_)) => break,
                assets::Status::Failed { failure, .. } => panic!("unexpected environment failure: {failure:?}"),
                _ => {
                    demand.changed().await;
                }
            }
        }
        cancel.send(()).unwrap();
        service.stop().await;
        assert_eq!(peer.finish().len(), 3);
        drop(demand);
        drop(handle);
    }

    #[tokio::test]
    async fn a_second_session_decodes_a_seen_texture_without_http() {
        let directory = tempdir().unwrap();
        let cache = Arc::new(AssetCache::open(directory.path(), 1024 * 1024 * 1024).unwrap());
        let first_peer = Peer::new(2, |index, _, base| if index == 0 { (200, grants(base)) } else { (200, RGBA_J2C.to_vec()) });
        let (cancel_first, cancelled_first) = oneshot::channel();
        let first_service = AssetCoordinator::with_budget(
            Handle::current(),
            async move {
                let _ = cancelled_first.await;
            },
            Budget::default(),
            Some(Arc::clone(&cache)),
        );
        let region = Region::new(1);
        first_service.register(region, Client::new(first_peer.url.join("seed").unwrap()).unwrap());
        let first_handle = first_service.handle();
        let mut first_demand =
            first_handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
        let first_texture = loop {
            match first_demand.status() {
                assets::Status::Ready(assets::Content::Texture(texture)) => break texture,
                assets::Status::Failed { failure, .. } => panic!("unexpected texture failure: {failure:?}"),
                _ => {
                    first_demand.changed().await;
                }
            }
        };
        assert_eq!(first_texture.image().unwrap().pixels(), RGBA_RAW);
        cancel_first.send(()).unwrap();
        first_service.stop().await;
        assert_eq!(first_peer.finish().len(), 2);
        assert_eq!(cache.stats().bytes_written, u64::try_from(RGBA_J2C.len()).unwrap());
        drop(first_texture);
        drop(first_demand);
        drop(first_handle);

        let second_peer = Peer::new(0, |_, _, _| panic!("warm cache must not make HTTP requests"));
        let (cancel_second, cancelled_second) = oneshot::channel();
        let second_service = AssetCoordinator::with_budget(
            Handle::current(),
            async move {
                let _ = cancelled_second.await;
            },
            Budget::default(),
            Some(Arc::clone(&cache)),
        );
        second_service.register(region, Client::new(second_peer.url.join("seed").unwrap()).unwrap());
        let second_handle = second_service.handle();
        let mut second_demand =
            second_handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
        let second_texture = loop {
            match second_demand.status() {
                assets::Status::Ready(assets::Content::Texture(texture)) => break texture,
                assets::Status::Failed { failure, .. } => panic!("unexpected warm texture failure: {failure:?}"),
                _ => {
                    second_demand.changed().await;
                }
            }
        };
        assert_eq!(second_texture.image().unwrap().pixels(), RGBA_RAW);
        cancel_second.send(()).unwrap();
        second_service.stop().await;
        assert!(second_peer.finish().is_empty());
        assert_eq!(second_handle.cache_stats().unwrap().hits, 1);
        drop(second_texture);
        drop(second_demand);
        drop(second_handle);
        Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handle released")).shutdown().unwrap();
    }

    #[tokio::test]
    async fn a_semantically_corrupt_texture_is_refetched_and_replaced() {
        let directory = tempdir().unwrap();
        let cache = Arc::new(AssetCache::open(directory.path(), 1024 * 1024 * 1024).unwrap());
        cache.put_complete(resource(Kind::Texture), b"bad".to_vec()).unwrap();
        let peer = Peer::new(2, |index, _, base| if index == 0 { (200, grants(base)) } else { (200, RGBA_J2C.to_vec()) });
        let (cancel, cancelled) = oneshot::channel();
        let service = AssetCoordinator::with_budget(
            Handle::current(),
            async move {
                let _ = cancelled.await;
            },
            Budget::default(),
            Some(Arc::clone(&cache)),
        );
        let region = Region::new(1);
        service.register(region, Client::new(peer.url.join("seed").unwrap()).unwrap());
        let handle = service.handle();
        let mut demand = handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
        let texture = loop {
            match demand.status() {
                assets::Status::Ready(assets::Content::Texture(texture)) => break texture,
                assets::Status::Failed { failure, .. } => panic!("unexpected refetch failure: {failure:?}"),
                _ => {
                    demand.changed().await;
                }
            }
        };
        assert_eq!(texture.image().unwrap().pixels(), RGBA_RAW);
        cancel.send(()).unwrap();
        service.stop().await;
        assert_eq!(peer.finish().len(), 2);
        assert_eq!(handle.cache_stats().unwrap().corrupt_entries, 1);
        let assets::cache::AssetLookup::Hit(asset) = cache.lookup(resource(Kind::Texture)) else {
            panic!("validated network content replaced the corrupt entry")
        };
        assert_eq!(asset.bytes, RGBA_J2C);
        drop(texture);
        drop(demand);
        drop(handle);
        Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handle released")).shutdown().unwrap();
    }

    #[tokio::test]
    async fn server_failures_and_partial_full_responses_are_not_cached() {
        for (response_status, body) in [(503, Vec::new()), (206, RGBA_J2C.to_vec()), (418, Vec::new())] {
            let directory = tempdir().unwrap();
            let cache = Arc::new(AssetCache::open(directory.path(), 1024 * 1024 * 1024).unwrap());
            let peer = Peer::new(2, move |index, _, base| if index == 0 { (200, grants(base)) } else { (response_status, body.clone()) });
            let (cancel, cancelled) = oneshot::channel();
            let service = AssetCoordinator::with_budget(
                Handle::current(),
                async move {
                    let _ = cancelled.await;
                },
                Budget::default(),
                Some(Arc::clone(&cache)),
            );
            let region = Region::new(1);
            service.register(region, Client::new(peer.url.join("seed").unwrap()).unwrap());
            let handle = service.handle();
            let mut demand = handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
            let failure = loop {
                match demand.status() {
                    assets::Status::Failed { failure, .. } => break failure,
                    _ => {
                        demand.changed().await;
                    }
                }
            };
            if response_status == 503 {
                assert_matches!(failure, Failure::Transient { .. });
            } else {
                assert_eq!(failure, Failure::HttpStatus { status: response_status });
            }
            cancel.send(()).unwrap();
            service.stop().await;
            assert_eq!(peer.finish().len(), 2);
            assert_eq!(cache.stats().bytes_written, 0);
            assert_eq!(cache.stats().dropped_writes, 0);
            assert_eq!(cache.lookup(resource(Kind::Texture)), assets::cache::AssetLookup::Miss);
            drop(demand);
            drop(handle);
            Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handle released")).shutdown().unwrap();
        }
    }

    #[tokio::test]
    async fn missing_grants_and_discovery_retry() {
        let peer = Peer::new(2, |index, _, _| if index == 0 { (503, Vec::new()) } else { (200, b"<llsd><map/></llsd>".to_vec()) });
        let transport = CapabilityTransport::new(Client::new(peer.url.join("seed").unwrap()).unwrap());
        let mut store = Store::new(Budget::default());
        let route = store.register_route().unwrap();
        let now = Instant::now();
        let _demand = store.request(route, assets::Request::new(resource(Kind::Texture)), Priority::Visible, now).unwrap();
        let job = store.next_job(now).unwrap();
        let LoadJob::Fetch(fetch) = &job else { panic!("new request must start with a fetch") };
        let failure = transport.fetch(fetch).await.expect_err("discovery fails transiently");
        assert!(matches!(failure, Failure::Transient { .. }));
        store.complete(job.fail(failure), now);
        let job = store.next_job(now + Duration::from_secs(1)).unwrap();
        let LoadJob::Fetch(fetch) = &job else { panic!("retry must start with a fetch") };
        assert_eq!(transport.fetch(fetch).await.expect_err("discovery has no capability"), Failure::MissingCapability);
        store.complete(job.fail(Failure::MissingCapability), now + Duration::from_secs(1));
        assert!(store.next_job(now + Duration::from_secs(2)).is_none());
        assert_eq!(peer.finish().len(), 2);
    }

    #[tokio::test]
    async fn late_route_material_loads() {
        use assets::{Content, MaterialDescription, PbrAlphaMode, Status};

        let peer = Peer::new(2, |index, _, base| {
            if index == 0 {
                return (200, grants(base));
            }
            let fields = Value::Map(
                [
                    ("version".into(), Value::String("1.0".into())),
                    ("type".into(), Value::String("GLTF 2.0".into())),
                    (
                        "data".into(),
                        Value::String(r#"{"asset":{"version":"2.0"},"materials":[{"alphaMode":"MASK","alphaCutoff":0.75}]}"#.into()),
                    ),
                ]
                .into(),
            );
            (200, binary::to_vec(&fields).unwrap())
        });
        let client = Client::new(peer.url.join("seed").unwrap()).unwrap();
        let (cancel, cancelled) = oneshot::channel();
        let budget = Budget::default();
        let service = AssetCoordinator::with_budget(
            Handle::current(),
            async {
                let _ = cancelled.await;
            },
            budget.clone(),
            None,
        );
        let region = Region::new(1);
        let handle = service.handle();
        let request = assets::Request::new(resource(Kind::PbrMaterial));
        let mut requesting = Box::pin(handle.request(region, request, Priority::Visible));
        assert!(futures::poll!(&mut requesting).is_pending());
        assert!(handle.request_once(region, request, Priority::Visible).await.unwrap().is_none());
        assert!(futures::poll!(&mut requesting).is_pending(), "a request must wait for its region's route");
        service.register(region, client);
        let mut demand = requesting.await.unwrap();
        let material = loop {
            match demand.status() {
                Status::Ready(Content::Material(material)) => break material,
                Status::Failed { failure, .. } => panic!("unexpected material failure: {failure:?}"),
                Status::StaleSession | Status::Ready(Content::Texture(_)) => panic!("unexpected content or stale session"),
                _ => {}
            }
            demand.changed().await;
        };
        let MaterialDescription::Pbr(description) = &*material else { panic!("expected PBR material") };
        assert_eq!(description.alpha_mode, PbrAlphaMode::Mask);
        assert_eq!(description.alpha_cutoff, 0.75);
        cancel.send(()).unwrap();
        service.stop().await;
        assert_eq!(budget.stats().charged_bytes, size_of::<MaterialDescription>());
        drop(material);
        assert_eq!(budget.stats().charged_bytes, 0);
        assert_eq!(peer.finish().len(), 2);
    }

    #[tokio::test]
    async fn pending_route_closes_with_session() {
        let service = AssetCoordinator::with_budget(Handle::current(), pending(), Budget::default(), None);
        let handle = service.handle();
        let region = Region::new(1);
        let request = assets::Request::new(resource(Kind::Texture));
        let mut requesting = Box::pin(handle.request(region, request, Priority::Visible));
        assert!(futures::poll!(&mut requesting).is_pending());
        assert!(handle.request_once(region, request, Priority::Visible).await.unwrap().is_none());
        assert!(futures::poll!(&mut requesting).is_pending());
        service.stop().await;
        assert!(matches!(requesting.await, Err(RequestError::Closed)));
        assert_eq!(handle.memory_stats().charged_bytes, 0);
    }

    #[tokio::test]
    async fn shared_images_outlive_session() {
        let (started, received) = oneshot::channel();
        let mut started = Some(started);
        let (release, hold) = std::sync::mpsc::channel();
        let peer = Peer::new(2, move |index, _, base| {
            if index == 0 {
                return (200, grants(base));
            }
            started.take().unwrap().send(()).unwrap();
            hold.recv_timeout(Duration::from_secs(5)).unwrap();
            (200, RGBA_J2C.to_vec())
        });
        let client = Client::new(peer.url.join("seed").unwrap()).unwrap();
        let (cancel, cancelled) = oneshot::channel();
        let budget = Budget::default();
        let service = AssetCoordinator::with_budget(
            Handle::current(),
            async {
                let _ = cancelled.await;
            },
            budget.clone(),
            None,
        );
        let region = Region::new(1);
        service.register(region, client);
        let handle = service.handle();
        let first = handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
        let mut second = handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
        received.await.unwrap();
        drop(first);
        assert_eq!(budget.stats().charged_bytes, Kind::Texture.max_response_bytes(), "the remaining face still owns the active fetch");
        release.send(()).unwrap();
        let blob = loop {
            match second.status() {
                assets::Status::Ready(assets::Content::Texture(blob)) => break blob,
                assets::Status::Failed { failure, .. } => panic!("unexpected {failure:?}"),
                _ => {}
            }
            second.changed().await;
        };
        assert_eq!(blob.image().unwrap().pixels(), RGBA_RAW);
        assert_eq!(budget.stats().high_water_bytes, 192 * 1024 * 1024);
        cancel.send(()).unwrap();
        service.stop().await;
        assert!(matches!(second.status(), assets::Status::StaleSession));
        assert_eq!(budget.stats().resident_bytes, blob.image().unwrap().pixels().len() + RGBA_J2C.len());
        assert!(matches!(
            handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await,
            Err(RequestError::Closed)
        ));
        drop(second);
        drop(blob);
        assert_eq!(budget.stats().charged_bytes, 0);
        assert_eq!(peer.finish().len(), 2);
    }

    #[tokio::test]
    async fn shutdown_cancels_active_fetch() {
        let (started, received) = oneshot::channel();
        let mut started = Some(started);
        let (release, hold) = std::sync::mpsc::channel();
        let peer = Peer::new(2, move |index, _, base| {
            if index == 0 {
                return (200, grants(base));
            }
            started.take().unwrap().send(()).unwrap();
            hold.recv_timeout(Duration::from_secs(5)).unwrap();
            (200, b"late data".to_vec())
        });
        let client = Client::new(peer.url.join("seed").unwrap()).unwrap();
        let (cancel, cancelled) = oneshot::channel();
        let budget = Budget::default();
        let service = AssetCoordinator::with_budget(
            Handle::current(),
            async {
                let _ = cancelled.await;
            },
            budget.clone(),
            None,
        );
        let region = Region::new(1);
        service.register(region, client);
        let demand = service.handle().request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
        received.await.unwrap();
        assert_eq!(budget.stats().charged_bytes, Kind::Texture.max_response_bytes());
        drop(demand);
        cancel.send(()).unwrap();
        service.stop().await;
        assert_eq!(budget.stats().charged_bytes, 0);
        release.send(()).unwrap();
        assert_eq!(peer.finish().len(), 2);
    }

    #[tokio::test]
    async fn shutdown_drains_blocked_cache_write() {
        let directory = tempdir().unwrap();
        let cache = Arc::new(AssetCache::open(directory.path(), 1024 * 1024 * 1024).unwrap());
        let peer = Peer::new(2, |index, _, base| if index == 0 { (200, grants(base)) } else { (200, RGBA_J2C.to_vec()) });
        let (write_started, write_started_receive) = oneshot::channel();
        let mut write_started = Some(write_started);
        let (write_finished, write_finished_receive) = oneshot::channel();
        let mut write_finished = Some(write_finished);
        let (release_write, hold_write) = std::sync::mpsc::sync_channel(0);
        let mut hold_write = Some(hold_write);
        let (shutdown_drain_started, shutdown_drain_started_receive) = oneshot::channel();
        let service = AssetCoordinator::build(
            Handle::current(),
            pending(),
            Budget::default(),
            Some(Arc::clone(&cache)),
            Box::new(move |writes, runtime, jobs| {
                for write in writes {
                    let write_started = write_started.take().expect("one cache write starts");
                    let write_finished = write_finished.take().expect("one cache write finishes");
                    let hold_write = hold_write.take().expect("one cache write gate");
                    jobs.push(runtime.spawn_blocking(move || {
                        write_started.send(()).expect("signal cache write started");
                        hold_write.recv_timeout(Duration::from_secs(5)).expect("release cache write");
                        write.run();
                        write_finished.send(()).expect("signal cache write finished");
                        WorkerResult::CacheWritten
                    }));
                }
            }),
            Box::new(move || shutdown_drain_started.send(()).expect("signal cache write drain started")),
        );
        let region = Region::new(1);
        service.register(region, Client::new(peer.url.join("seed").unwrap()).unwrap());
        let handle = service.handle();
        let mut demand = handle.request(region, assets::Request::new(resource(Kind::Texture)), Priority::Visible).await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match demand.status() {
                    assets::Status::Ready(_) => break,
                    assets::Status::Failed { failure, .. } => panic!("unexpected texture failure: {failure:?}"),
                    _ => {
                        demand.changed().await;
                    }
                }
            }
        })
        .await
        .expect("texture decode completes");
        tokio::time::timeout(Duration::from_secs(5), write_started_receive).await.expect("cache write starts").unwrap();

        let stop = service.stop();
        tokio::pin!(stop);
        assert_matches!(futures::poll!(stop.as_mut()), std::task::Poll::Pending);
        tokio::time::timeout(Duration::from_secs(5), shutdown_drain_started_receive)
            .await
            .expect("coordinator entered cache write drain")
            .unwrap();
        let stop_state = futures::poll!(stop.as_mut());
        if let std::task::Poll::Ready(()) = stop_state {
            release_write.send(()).expect("release cache write after early shutdown");
            write_finished_receive.await.expect("cache write finishes after early shutdown");
            panic!("coordinator completed while cache write remained blocked");
        }
        assert_matches!(stop_state, std::task::Poll::Pending);
        release_write.send(()).expect("resume cache write");
        tokio::time::timeout(Duration::from_secs(5), stop.as_mut()).await.expect("coordinator drains cache write");
        write_finished_receive.await.unwrap();
        assert_eq!(handle.cache_stats().unwrap().dropped_writes, 0);
        assert!(handle.cache_stats().unwrap().bytes_written > 0);
        assert_eq!(peer.finish().len(), 2);
        drop(demand);
        drop(handle);
        Arc::try_unwrap(cache).unwrap_or_else(|_| panic!("cache handles released")).shutdown().unwrap();

        let reopened = AssetCache::open(directory.path(), 1024 * 1024 * 1024).unwrap();
        assert_matches!(reopened.lookup(resource(Kind::Texture)), assets::cache::AssetLookup::Hit(_));
        reopened.shutdown().unwrap();
    }
}
