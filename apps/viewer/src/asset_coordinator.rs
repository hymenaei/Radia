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
                if reply.send(result).is_err() {
                    tracing::trace!("asset request receiver closed");
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use assets::Priority;
    use llsd::{Value, binary, xml};
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };
    use test_support::texture::{RGBA_J2C, RGBA_RAW};
    use url::Url;
    use utils::AssetId;
    use uuid::Uuid;

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
                    let (status, body) = respond(index, &request, &base);
                    let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
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
            let job = store.next_job(Instant::now()).unwrap();
            let result = transport.fetch(&job).await.unwrap();
            assert_eq!(result, expected);
            drop(job);
            drop(demand);
        }
        assert_eq!(peer.finish().len(), 4, "capability discovery is shared across resource kinds");
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
        let first = transport.fetch(&job).await;
        assert!(matches!(first, Err(Failure::Transient { .. })));
        let first = first.and_then(|bytes| job.prepare(bytes));
        store.complete(job, first, now);
        let job = store.next_job(now + Duration::from_secs(1)).unwrap();
        assert_eq!(transport.fetch(&job).await, Err(Failure::MissingCapability));
        store.complete(job, Err(Failure::MissingCapability), now + Duration::from_secs(1));
        assert!(store.next_job(now + Duration::from_secs(2)).is_none());
        assert_eq!(peer.finish().len(), 2);
    }

    #[tokio::test]
    async fn material_preparation_publishes_content() {
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
        );
        let region = Region::new(1);
        service.register(region, client);
        let mut demand =
            service.handle().request(region, assets::Request::new(resource(Kind::PbrMaterial)), Priority::Visible).await.unwrap();
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
}
