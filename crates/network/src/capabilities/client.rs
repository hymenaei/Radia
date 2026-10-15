use std::{collections::HashMap, fmt, future::Future, net::SocketAddr, sync::Arc, time::Duration};

use llsd::xml;
use reqwest::Url;
use serde::Serialize;
use tokio::{
    runtime::Handle,
    sync::{Mutex, OwnedSemaphorePermit, mpsc, oneshot, watch},
    task::{AbortHandle, JoinHandle},
    time,
};

use crate::{
    capture::CaptureRecorder,
    coverage::CoverageLossLedger,
    error::Error,
    event::{EventEnvelope, SimulatorId},
    http::{self, validate_http_url},
    status::NetworkStatusHandle,
};

use super::{
    budget::{
        COMMAND_CAPACITY, CapabilityBudget, EVENT_QUEUE_REQUEST_TIMEOUT, LLSD_SCAN_WORKING_BYTES, MAX_CAPABILITY_REQUEST_BODY_BYTES,
        REQUEST_TIMEOUT, spawn_blocking_with_permit,
    },
    event_queue::{self, EventQueueTask},
    request::{
        AdmittedCapabilities, AdmittedResponse, Command, RequestDeadline, capability_error, capability_llsd_limits,
        decode_capability_value, logical_endpoint_label, run, validate_capability_names, wait_for_session_close,
    },
};

#[derive(Clone)]
/// An opaque handle for an authenticated simulator capability URL.
pub struct CapabilityUrl {
    pub(super) url: Url,
    pub(super) capture_endpoint: Option<String>,
}

impl CapabilityUrl {
    pub(super) fn from_discovery(name: &str, url: Url) -> Self {
        Self { url, capture_endpoint: logical_endpoint_label(name) }
    }

    /// Parses and validates an HTTP or HTTPS capability URL.
    ///
    /// The URL must have a host and must not contain user information or a fragment.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL is malformed or violates these constraints.
    pub fn parse(value: &str) -> Result<Self, Error> {
        let url = Url::parse(value).map_err(|_| Error::Capability("Capability URL is invalid.".into()))?;
        validate_http_url(&url).map_err(Error::Capability)?;
        Ok(Self { url, capture_endpoint: None })
    }

    /// Returns a new handle with one additional URL-encoded query pair.
    pub fn with_query_pair(&self, name: &str, value: &str) -> Self {
        let mut url = self.url.clone();
        url.query_pairs_mut().append_pair(name, value);
        Self { url, capture_endpoint: self.capture_endpoint.clone() }
    }
}

impl fmt::Debug for CapabilityUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("CapabilityUrl").field(&"[REDACTED]").finish()
    }
}

#[derive(Clone)]
/// Makes bounded requests to a simulator's discovered capabilities.
///
/// [`Client::get`] and [`Client::post`] return built-in LLSD [`Value`](llsd::Value) values. Network reserves the raw response,
/// scanner workspace, and the LLSD value decoder's allocation estimate while decoding. The guarantee ends when the value is returned;
/// caller-side typed conversion, caller-owned values, HTTP/TLS buffers, allocator metadata, and fragmentation are outside that budget.
/// Use [`Client::get_bytes`] for binary resources; it returns caller-owned bytes after the Network response admission is released.
pub struct Client {
    seed: Arc<Url>,
    runtime: Handle,
    pub(super) commands: mpsc::Sender<Command>,
    http: http::Client,
    session_closed: watch::Sender<bool>,
    request_worker: Arc<RequestWorker>,
    budget: CapabilityBudget,
    status: NetworkStatusHandle,
    request_timeout: Duration,
}

struct RequestWorker {
    task: Mutex<Option<JoinHandle<()>>>,
}

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Client").field("seed", &"<redacted>").finish_non_exhaustive()
    }
}

impl Client {
    /// Creates a client for the login response's seed capability URL.
    ///
    /// The current thread must be inside a Tokio runtime. The client starts a bounded request worker on that runtime.
    /// Request operations run on the owning runtime and can be awaited from other executors.
    ///
    /// # Errors
    ///
    /// Returns an error if the seed URL is invalid or no Tokio runtime is active.
    pub fn new(seed: Url) -> Result<Self, Error> {
        Self::new_with_capture(seed, None).map_err(Error::Capability)
    }

    pub(crate) fn new_with_capture(seed: Url, capture: Option<CaptureRecorder>) -> Result<Self, String> {
        Self::new_with_status(seed, capture, NetworkStatusHandle::default())
    }

    pub(crate) fn new_with_status(seed: Url, capture: Option<CaptureRecorder>, status: NetworkStatusHandle) -> Result<Self, String> {
        Self::new_with_status_and_budget(seed, capture, status, CapabilityBudget::new())
    }

    pub(crate) fn new_with_status_and_budget(
        seed: Url,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        budget: CapabilityBudget,
    ) -> Result<Self, String> {
        validate_http_url(&seed)?;
        let runtime = Handle::try_current().map_err(|_| "A Tokio runtime is required for capability requests.".to_string())?;
        let http = budget
            .http_client()?
            .with_capture(capture.clone())
            .with_status(status.clone())
            .with_memory_budget(budget.response_memory.clone(), http::ResponseMemoryClass::Ordinary);
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let task = runtime.spawn(run(http.clone(), seed.clone(), receiver, session_closed_receiver, capture, status.clone()));
        let request_worker = Arc::new(RequestWorker { task: Mutex::new(Some(task)) });
        Ok(Self {
            seed: Arc::new(seed),
            runtime,
            commands,
            http,
            session_closed,
            request_worker,
            budget,
            status,
            request_timeout: REQUEST_TIMEOUT,
        })
    }

    /// Resolves capability names into opaque URL handles.
    ///
    /// The request accepts up to 256 names and 16 KiB of name data. A name may use up to 128 UTF-8 bytes.
    /// The returned map contains only names granted by the server.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid request sizes, a closed session, or a failed capability request.
    #[tracing::instrument(
        name = "capabilities.discover", level = "debug", skip_all,
        fields(capability_count = names.len())
    )]
    pub async fn request_capabilities(&self, names: &[&str]) -> Result<HashMap<String, CapabilityUrl>, Error> {
        validate_capability_names(names)?;
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit(deadline).await?;
        let names = names.iter().map(|name| (*name).to_owned()).collect();
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        self.on_runtime(async move { client.request_capabilities_admitted(names, deadline, permit).await }).await
    }

    async fn request_capabilities_admitted(
        &self,
        names: Vec<String>,
        deadline: RequestDeadline,
        permit: OwnedSemaphorePermit,
    ) -> Result<HashMap<String, CapabilityUrl>, Error> {
        let (reply, response) = oneshot::channel();
        time::timeout_at(deadline.at, self.commands.send(Command::RequestCapabilities { names, reply, deadline, _permit: permit }))
            .await
            .map_err(|_| deadline.error())?
            .map_err(|_| Error::CapabilitySessionClosed)?;
        let response = time::timeout_at(deadline.at, response)
            .await
            .map_err(|_| deadline.error())?
            .map_err(|_| self.request_ended_error())?
            .map_err(capability_error)?;
        let AdmittedCapabilities { capabilities, _request, _memory } = response;
        drop(_memory);
        drop(_request);
        Ok(capabilities)
    }

    /// Sends an LLSD XML request and decodes the response into an LLSD [`Value`](llsd::Value).
    ///
    /// The encoded request body is limited to 1 MiB and the response body to 32 MiB. Network admits the raw response, bounded LLSD scan
    /// workspace, and the LLSD `Value` decoder's allocation estimate before decoding. This estimate excludes allocator metadata and
    /// fragmentation. The request slot and decode-memory admission remain held until decoding completes, including if the awaiting task
    /// is canceled. The admission estimate includes the decoded `Value`'s construction peak. After this method returns, the caller
    /// owns the value; its retained memory and any typed conversion are outside Network's budget.
    ///
    /// # Errors
    ///
    /// Returns an error if encoding exceeds its limit, the capability request fails, response memory cannot be admitted, or the LLSD
    /// response is malformed or exceeds a decoder limit.
    pub async fn post<Request>(&self, url: &CapabilityUrl, payload: &Request) -> Result<llsd::Value, Error>
    where
        Request: Serialize + ?Sized,
    {
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit(deadline).await?;
        let body = xml::to_vec_with_max_bytes(payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
            .map_err(|error| Error::Capability(format!("Could not encode an LLSD request: {error}")))?;
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let response = client.post_admitted(url, body, deadline, permit).await?;
            decode_admitted_response(response, client.http.clone(), deadline).await
        })
        .await
    }

    async fn post_admitted(
        &self,
        url: CapabilityUrl,
        body: Vec<u8>,
        deadline: RequestDeadline,
        permit: OwnedSemaphorePermit,
    ) -> Result<AdmittedResponse, Error> {
        let (reply, response) = oneshot::channel();
        time::timeout_at(deadline.at, self.commands.send(Command::Post { url, body, reply, deadline, _permit: permit }))
            .await
            .map_err(|_| deadline.error())?
            .map_err(|_| Error::CapabilitySessionClosed)?;
        time::timeout_at(deadline.at, response)
            .await
            .map_err(|_| deadline.error())?
            .map_err(|_| self.request_ended_error())?
            .map_err(capability_error)
    }

    /// Fetches an LLSD XML response and decodes it into an LLSD [`Value`](llsd::Value).
    ///
    /// The response body is limited to 32 MiB. Network admits the raw response, bounded LLSD scan workspace, and the LLSD `Value` decoder's
    /// allocation estimate before decoding. This estimate excludes allocator metadata and fragmentation. The request slot and decode-memory
    /// admission remain held until decoding completes, including if the awaiting task is canceled. The admission estimate includes the
    /// decoded `Value`'s construction peak. After this method returns, the caller owns the value; its retained memory and any typed
    /// conversion are outside Network's budget.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails, response memory cannot be admitted, or the LLSD response is malformed or exceeds a decoder
    /// limit.
    pub async fn get(&self, url: &CapabilityUrl) -> Result<llsd::Value, Error> {
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit(deadline).await?;
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let response = client.get_bytes_admitted_with_permit(url, deadline, permit).await?;
            decode_admitted_response(response, client.http.clone(), deadline).await
        })
        .await
    }

    /// Fetches the raw body for binary or otherwise non-LLSD capability resources.
    ///
    /// The response body is limited to 32 MiB.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response body exceeds its limit.
    pub async fn get_bytes(&self, url: &CapabilityUrl) -> Result<Vec<u8>, Error> {
        Ok(self.get_bytes_admitted(url).await?.body)
    }

    async fn get_bytes_admitted(&self, url: &CapabilityUrl) -> Result<AdmittedResponse, Error> {
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit(deadline).await?;
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move { client.get_bytes_admitted_with_permit(url, deadline, permit).await }).await
    }

    async fn get_bytes_admitted_with_permit(
        &self,
        url: CapabilityUrl,
        deadline: RequestDeadline,
        permit: OwnedSemaphorePermit,
    ) -> Result<AdmittedResponse, Error> {
        let (reply, response) = oneshot::channel();
        time::timeout_at(deadline.at, self.commands.send(Command::GetBytes { url, reply, deadline, _permit: permit }))
            .await
            .map_err(|_| deadline.error())?
            .map_err(|_| Error::CapabilitySessionClosed)?;
        time::timeout_at(deadline.at, response)
            .await
            .map_err(|_| deadline.error())?
            .map_err(|_| self.request_ended_error())?
            .map_err(capability_error)
    }

    async fn acquire_request_permit(&self, deadline: RequestDeadline) -> Result<OwnedSemaphorePermit, Error> {
        let budget = Arc::clone(&self.budget.ordinary_requests);
        let session_closed = self.session_closed.subscribe();
        self.on_runtime(async move {
            tokio::select! {
                biased;
                _ = wait_for_session_close(session_closed) => Err(Error::CapabilitySessionClosed),
                permit = budget.acquire_owned() => permit.map_err(|_| Error::CapabilityRequestCanceled),
                _ = time::sleep_until(deadline.at) => Err(deadline.error()),
            }
        })
        .await
    }

    fn on_runtime<T, F>(&self, future: F) -> impl Future<Output = Result<T, Error>> + '_
    where
        T: Send + 'static,
        F: Future<Output = Result<T, Error>> + Send + 'static,
    {
        let task = self.runtime.spawn(future);
        let abort = AbortOnDrop(task.abort_handle());
        async move {
            let result = task.await;
            drop(abort);
            match result {
                Ok(result) => result,
                Err(error) => {
                    if error.is_panic() {
                        self.status.record_error("The capability request task failed.");
                        return Err(Error::Capability("The capability request task failed.".into()));
                    }
                    Err(self.request_ended_error())
                }
            }
        }
    }

    fn request_ended_error(&self) -> Error {
        if *self.session_closed.borrow() { Error::CapabilitySessionClosed } else { Error::CapabilityRequestCanceled }
    }

    #[cfg(test)]
    pub(super) fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    pub(crate) fn close_session(&self) {
        self.session_closed.send_replace(true);
    }

    pub(crate) async fn close_session_and_wait(&self) {
        self.close_session();
        self.wait_for_worker().await;
    }

    pub(crate) async fn wait_for_worker(&self) {
        let mut task = self.request_worker.task.lock().await;
        if let Some(task) = task.take()
            && task.await.is_err()
        {
            self.status.record_error("The capability request worker failed during shutdown.");
        }
    }

    pub(crate) fn start_event_queue(
        &self,
        simulator_id: SimulatorId,
        source: SocketAddr,
        events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        coverage_losses: CoverageLossLedger,
    ) -> Result<EventQueueTask, String> {
        self.start_event_queue_with_role(simulator_id, source, events, shutdown, coverage_losses, false)
    }

    pub(crate) fn start_primary_event_queue(
        &self,
        simulator_id: SimulatorId,
        source: SocketAddr,
        events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        coverage_losses: CoverageLossLedger,
    ) -> Result<EventQueueTask, String> {
        self.start_event_queue_with_role(simulator_id, source, events, shutdown, coverage_losses, true)
    }

    fn start_event_queue_with_role(
        &self,
        simulator_id: SimulatorId,
        source: SocketAddr,
        events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        coverage_losses: CoverageLossLedger,
        is_primary: bool,
    ) -> Result<EventQueueTask, String> {
        let runtime = Handle::try_current().map_err(|_| "The simulator event queue requires an active Tokio runtime.".to_string())?;
        let http = self
            .http
            .clone()
            .with_timeout(Some(EVENT_QUEUE_REQUEST_TIMEOUT))
            .with_memory_budget(self.budget.response_memory.clone(), http::ResponseMemoryClass::EventQueue);
        let seed = self.seed.as_ref().clone();
        let status = self.status.clone();
        let ordinary_requests = Arc::clone(&self.budget.ordinary_requests);
        let primary_event_queues = Arc::clone(&self.budget.primary_event_queues);
        let neighbor_event_queues = Arc::clone(&self.budget.neighbor_event_queues);
        let queued_event_bytes = Arc::clone(&self.budget.queued_event_bytes);
        let (role, primary) = event_queue::EventQueueRoleHandle::channel(is_primary);
        let session_closed = self.session_closed.subscribe();
        let (completed, completion) = oneshot::channel();
        let task = runtime.spawn(async move {
            let exit = event_queue::run_event_queue(
                simulator_id,
                source,
                seed,
                http,
                events,
                shutdown,
                primary,
                status,
                session_closed,
                ordinary_requests,
                primary_event_queues,
                neighbor_event_queues,
                queued_event_bytes,
                coverage_losses,
            )
            .await;
            let _ = completed.send(exit);
        });
        Ok(EventQueueTask::new(task, completion, role))
    }
}

fn capability_preflight_error(error: xml::BoundError) -> Error {
    match error {
        xml::BoundError::Malformed => Error::Capability(format!("Could not decode an LLSD response: {error}")),
        xml::BoundError::LimitExceeded(_) | xml::BoundError::Overflow => {
            Error::CapabilityHttp(http::Error::resource_exhausted(format!("Could not admit LLSD response decoding: {error}")))
        }
        _ => Error::Capability(format!("Could not decode an LLSD response: {error}")),
    }
}

fn admit_capability_decode_memory(body: &[u8], http: &http::Client) -> Result<http::ResponseMemoryPermit, Error> {
    let mut memory =
        http.try_reserve_memory(http::ResponseMemoryClass::Ordinary, LLSD_SCAN_WORKING_BYTES).map_err(Error::CapabilityHttp)?;
    let bound = xml::value_allocation_bound(body, capability_llsd_limits()).map_err(capability_preflight_error)?;
    memory.try_grow(bound).map_err(Error::CapabilityHttp)?;
    Ok(memory)
}

async fn decode_admitted_response(response: AdmittedResponse, http: http::Client, deadline: RequestDeadline) -> Result<llsd::Value, Error> {
    let AdmittedResponse { body, _memory: raw_memory, _request: request } = response;
    let (decoded, request) = time::timeout_at(
        deadline.at,
        spawn_blocking_with_permit(request, move || {
            let decoded = if deadline.expired() {
                Err(deadline.error())
            } else {
                admit_capability_decode_memory(&body, &http).and_then(|memory| {
                    if deadline.expired() {
                        return Err(deadline.error());
                    }
                    #[cfg(test)]
                    tests::wait_for_test_decode_gate();
                    if deadline.expired() {
                        return Err(deadline.error());
                    }
                    decode_capability_value(&body).map(|value| (value, memory))
                })
            };
            drop(body);
            drop(raw_memory);
            decoded
        }),
    )
    .await
    .map_err(|_| deadline.error())?
    .map_err(|error| Error::Capability(format!("LLSD response decoder task failed: {error}")))?;
    drop(request);
    let (value, memory) = decoded?;
    drop(memory);
    Ok(value)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::{RequestDeadline, admit_capability_decode_memory, decode_admitted_response};
    use std::{
        collections::HashSet,
        fs,
        future::Future,
        io::{self, ErrorKind, Read, Write},
        net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream},
        str,
        sync::{
            self, Arc, OnceLock, PoisonError,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Context, Poll, Wake, Waker},
        thread,
        time::{Duration, Instant},
    };

    use super::super::budget::LLSD_SCAN_WORKING_BYTES;
    use crate::http::testing::{LocalServer, read_request, response};
    use crate::{
        AgentSession,
        Error::{Capability, CapabilityHttp, CapabilitySessionClosed},
        Event, Grid, NetworkStatusHandle,
        capabilities::request::{AdmittedResponse, allocation_measurement::begin_process_measurement},
        capabilities::{CapabilityBudget, CapabilityUrl, Client},
        capabilities::{
            EVENT_QUEUE_NAME, EventQueueExit, MAX_CAPABILITY_REQUEST_BODY_BYTES, MAX_CAPABILITY_RESPONSE_BYTES,
            MAX_CONCURRENT_CAPABILITY_REQUESTS, MAX_CONCURRENT_EVENT_QUEUES, MAX_EVENT_QUEUE_ERRORS, MAX_EVENT_QUEUE_RESPONSE_BYTES,
            MAX_LLSD_XML_ELEMENTS, MAX_QUEUED_CAPABILITY_EVENT_BYTES, MAX_SEED_CAPABILITY_RESPONSE_BYTES, RetryOutcome, decode_event_batch,
            event_queue::{event_batch_allocation_bound, event_queue_retry_delay, retryable_event_queue_error},
            expected_empty_poll, measure_allocations, post_event_queue, retry_event_queue, seed_capability_retry_delay, seed_retry_delay,
        },
        capture::{
            CaptureConfig, CaptureEvent, CaptureEventKind, CaptureManifest, CapturePublication, CaptureRecorder, CaptureSession,
            CaptureState,
        },
        coverage::CoverageLossLedger,
        event::{CLIENT_EVENT_CAPACITY, EventEnvelope, EventReceiver, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId},
        http,
        session::{COMMAND_CAPACITY, Command},
        simulator::run_session,
        transport::lludp::Delivery,
    };
    use ::testing::{lludp::vectors, parse_hex_fixture};
    use base::UUID;
    use llsd::{Map, Value, xml};
    use protocol::{
        ChatType,
        lludp::{Message, Packet},
        messages::{
            AgentMovementComplete, ChatFromViewer, CompletePingCheck, PingId,
            agent_movement_complete::{AgentData, Data, SimData},
            chat_from_viewer,
            packet_ack::{PacketAck, Packets},
        },
    };
    use reqwest::StatusCode;
    use serde::Serialize;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{self, UdpSocket},
        runtime,
        sync::{Mutex, MutexGuard, Semaphore, mpsc, oneshot, watch},
        task::{JoinSet, spawn, yield_now},
        time::{self, sleep, timeout},
    };
    use url::Url;
    use zip::ZipArchive;

    fn available_event_queue_permits(budget: &CapabilityBudget) -> usize {
        budget.primary_event_queues.available_permits() + budget.neighbor_event_queues.available_permits()
    }

    struct DecodeGate {
        entered: sync::mpsc::Sender<()>,
        released: sync::Mutex<bool>,
        release: sync::Condvar,
    }

    impl DecodeGate {
        fn new() -> (Arc<Self>, sync::mpsc::Receiver<()>) {
            let (entered, entered_rx) = sync::mpsc::channel();
            (Arc::new(Self { entered, released: sync::Mutex::new(false), release: sync::Condvar::new() }), entered_rx)
        }

        fn release(&self) {
            *self.released.lock().unwrap_or_else(PoisonError::into_inner) = true;
            self.release.notify_all();
        }
    }

    static DECODE_GATE: OnceLock<sync::Mutex<Option<Arc<DecodeGate>>>> = OnceLock::new();
    static DECODE_GATE_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_decode_gate_test() -> MutexGuard<'static, ()> {
        let lock = DECODE_GATE_TEST_LOCK.get_or_init(|| Mutex::new(()));
        loop {
            if let Ok(guard) = lock.try_lock() {
                return guard;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn wait_in_decode() {
        let gate = DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock").clone();
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            let mut released = gate.released.lock().unwrap_or_else(PoisonError::into_inner);
            while !*released {
                let (next, result) = gate.release.wait_timeout(released, TEST_TIMEOUT).unwrap_or_else(PoisonError::into_inner);
                released = next;
                if !*released && result.timed_out() {
                    drop(released);
                    panic!("timed out waiting for test to release decoder");
                }
            }
        }
    }

    pub(super) fn wait_for_test_decode_gate() {
        wait_in_decode();
    }

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

    async fn read_request_headers(stream: &mut net::TcpStream) -> io::Result<()> {
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        loop {
            let size = stream.read(&mut chunk).await?;
            if size == 0 {
                return Err(ErrorKind::UnexpectedEof.into());
            }
            request.extend_from_slice(&chunk[..size]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                return Ok(());
            }
        }
    }

    async fn write_empty_response(stream: &mut net::TcpStream) -> io::Result<()> {
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await
    }

    #[test]
    fn requests_run_outside_tokio() {
        let server = LocalServer::start(vec![response(200, "OK", "application/octet-stream", None, "body")]);
        let runtime = runtime::Builder::new_multi_thread().enable_all().build().expect("Tokio runtime");
        let client = runtime.block_on(async { Client::new(server.url("/seed")) }).expect("active Tokio runtime");
        let url = CapabilityUrl::parse(server.url("/resource").as_str()).expect("valid capability URL");

        let body = block_on(client.get_bytes(&url)).expect("capability request from the external executor");

        assert_eq!(body, b"body");
        drop(client);
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn capability_clients_share_the_session_http_connection_pool() {
        let listener = net::TcpListener::bind("127.0.0.1:0").await.expect("bind local HTTP listener");
        let address = listener.local_addr().expect("read listener address");
        let server = spawn(async move {
            let (mut first, first_peer) = listener.accept().await.expect("accept first capability request");
            read_request_headers(&mut first).await.expect("read first request");
            write_empty_response(&mut first).await.expect("write first response");

            tokio::select! {
                result = read_request_headers(&mut first) => {
                    result.expect("read second request on reused connection");
                    write_empty_response(&mut first).await.expect("write second response");
                    (first_peer, first_peer)
                }
                accepted = listener.accept() => {
                    let (mut second, second_peer) = accepted.expect("accept second capability request");
                    read_request_headers(&mut second).await.expect("read second request on another connection");
                    write_empty_response(&mut second).await.expect("write second response");
                    (first_peer, second_peer)
                }
            }
        });

        let endpoint = format!("http://{address}");
        let seed = Url::parse(&format!("{endpoint}/seed")).expect("valid seed URL");
        let resource = CapabilityUrl::parse(&format!("{endpoint}/resource")).expect("valid capability URL");
        let budget = CapabilityBudget::new();
        let first = Client::new_with_status_and_budget(seed.clone(), None, NetworkStatusHandle::default(), budget.clone())
            .expect("create first capability client");
        let second = Client::new_with_status_and_budget(seed, None, NetworkStatusHandle::default(), budget)
            .expect("create second capability client");

        assert!(first.get_bytes(&resource).await.expect("first capability request").is_empty());
        assert!(second.get_bytes(&resource).await.expect("second capability request").is_empty());
        let (first_peer, second_peer) =
            timeout(TEST_TIMEOUT, server).await.expect("server should receive both requests").expect("server task");

        assert_eq!(first_peer, second_peer, "clients in one capability session should reuse an HTTP connection");
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        struct Unparker(thread::Thread);

        impl Wake for Unparker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }

            fn wake_by_ref(self: &Arc<Self>) {
                self.0.unpark();
            }
        }

        let waker = Waker::from(Arc::new(Unparker(thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(output) => return output,
                Poll::Pending => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    assert!(!remaining.is_zero(), "external-executor request timed out");
                    thread::park_timeout(remaining);
                }
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn response_memory_stays_admitted_through_value_decode() {
        let _decode_gate_test = lock_decode_gate_test();
        let body = b"<llsd><integer>42</integer></llsd>".to_vec();
        let body_len = body.len();
        let allocation_bound =
            xml::value_allocation_bound(&body, super::super::request::capability_llsd_limits()).expect("bound valid LLSD response");
        let memory_limit = body_len + LLSD_SCAN_WORKING_BYTES + allocation_bound;
        let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0, 0);
        let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_len).expect("reserve raw response memory");
        let (gate, entered_rx) = DecodeGate::new();
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = Some(gate.clone());
        let requests = Arc::new(Semaphore::new(1));
        let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
        let response = AdmittedResponse { body, _memory: Some(memory), _request: request };
        let http = http::Client::new(Some(TEST_TIMEOUT), false, None)
            .expect("build HTTP client")
            .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);
        let task = spawn(decode_admitted_response(response, http, RequestDeadline::new(TEST_TIMEOUT)));

        let entered_decode = entered_rx.recv_timeout(TEST_TIMEOUT).is_ok();
        let raw_memory_held = budget.try_reserve(http::ResponseMemoryClass::Ordinary, 1).is_err();
        let request_held = requests.try_acquire().is_err();
        gate.release();
        let result = timeout(TEST_TIMEOUT, task).await.expect("decoder should finish").expect("decoder task");
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = None;

        assert!(entered_decode, "LLSD decode should reach its admitted blocking point");
        assert!(raw_memory_held, "raw response and decode admission should remain held while decoding");
        assert!(request_held, "request concurrency should remain admitted while decoding");
        assert_eq!(result.expect("decode admitted response"), Value::Integer(42));
        let permit = budget
            .try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit)
            .expect("successful decode should release body and decoder admissions");
        drop(permit);
        drop(requests.try_acquire().expect("successful decode should release its request slot"));

        let malformed = b"<llsd><array></map></llsd>".to_vec();
        let malformed_len = malformed.len();
        let bound = xml::value_allocation_bound(&malformed, super::super::request::capability_llsd_limits());
        assert!(matches!(bound, Err(xml::BoundError::Malformed)), "malformed input must fail preflight");
        let memory_limit = malformed_len + LLSD_SCAN_WORKING_BYTES;
        let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0, 0);
        let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, malformed_len).expect("reserve malformed response memory");
        let requests = Arc::new(Semaphore::new(1));
        let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
        let response = AdmittedResponse { body: malformed, _memory: Some(memory), _request: request };
        let http = http::Client::new(Some(TEST_TIMEOUT), false, None)
            .expect("build HTTP client")
            .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);
        assert!(matches!(decode_admitted_response(response, http, RequestDeadline::new(TEST_TIMEOUT)).await, Err(Capability(_))));
        let permit = budget
            .try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit)
            .expect("failed decode should release body and decoder admissions");
        drop(permit);
        drop(requests.try_acquire().expect("failed decode should release its request slot"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_keeps_capability_decode_admission_until_worker_finishes() {
        let _decode_gate_test = lock_decode_gate_test();
        let body = b"<llsd><integer>42</integer></llsd>".to_vec();
        let allocation_bound =
            xml::value_allocation_bound(&body, super::super::request::capability_llsd_limits()).expect("bound valid LLSD response");
        let memory_limit = body.capacity() + LLSD_SCAN_WORKING_BYTES + allocation_bound;
        let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0, 0);
        let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body.capacity()).expect("reserve raw response memory");
        let (gate, entered_rx) = DecodeGate::new();
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = Some(gate.clone());
        let requests = Arc::new(Semaphore::new(1));
        let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
        let response = AdmittedResponse { body, _memory: Some(memory), _request: request };
        let http = http::Client::new(Some(TEST_TIMEOUT), false, None)
            .expect("build HTTP client")
            .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);
        let task = spawn(decode_admitted_response(response, http, RequestDeadline::new(TEST_TIMEOUT)));

        entered_rx.recv_timeout(TEST_TIMEOUT).expect("LLSD decoder reached its admitted blocking point");
        task.abort();
        assert!(task.await.expect_err("async decoder waiter should be canceled").is_cancelled());
        assert!(budget.try_reserve(http::ResponseMemoryClass::Ordinary, 1).is_err());
        assert_eq!(requests.available_permits(), 0, "the request slot remains held by the blocking decoder");

        gate.release();
        timeout(TEST_TIMEOUT, async {
            loop {
                let memory_released = match budget.try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit) {
                    Ok(permit) => {
                        drop(permit);
                        true
                    }
                    Err(_) => false,
                };
                let request_released = match Arc::clone(&requests).try_acquire_owned() {
                    Ok(permit) => {
                        drop(permit);
                        true
                    }
                    Err(_) => false,
                };
                if memory_released && request_released {
                    break;
                }
                yield_now().await;
            }
        })
        .await
        .expect("decoder completion should release body, decode memory, and request slot");
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = None;
    }

    #[test]
    fn capability_decode_preflight_rejects_depth_and_element_limits() {
        let max_depth = super::super::budget::MAX_LLSD_NESTING_DEPTH;
        let mut nested = String::from("<llsd>");
        for _ in 0..=max_depth {
            nested.push_str("<array>");
        }
        nested.push_str("<undef/>");
        for _ in 0..=max_depth {
            nested.push_str("</array>");
        }
        nested.push_str("</llsd>");

        let mut many_values = String::from("<llsd><array>");
        for _ in 0..MAX_LLSD_XML_ELEMENTS {
            many_values.push_str("<undef/>");
        }
        many_values.push_str("<undef/></array></llsd>");

        for body in [nested.into_bytes(), many_values.into_bytes()] {
            let memory_limit = body.capacity() + LLSD_SCAN_WORKING_BYTES;
            let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0, 0);
            let raw = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body.capacity()).expect("admit raw response body");
            let client = http::Client::new(Some(TEST_TIMEOUT), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);

            let error = match admit_capability_decode_memory(&body, &client) {
                Ok(_) => panic!("preflight must reject a configured LLSD limit"),
                Err(error) => error,
            };
            assert!(matches!(error, CapabilityHttp(http::Error::ResourceExhausted { .. })));
            drop(raw);
            drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit).expect("failed preflight releases workspace"));
        }
    }

    #[test]
    fn capability_decode_preflight_rejects_memory_budget_exhaustion() {
        let body = format!("<llsd><string>{}</string></llsd>", "x".repeat(128 * 1024)).into_bytes();
        let bound = xml::value_allocation_bound(&body, super::super::request::capability_llsd_limits()).expect("bound valid LLSD response");
        let body_capacity = body.capacity();

        for limit in [body_capacity + LLSD_SCAN_WORKING_BYTES - 1, body_capacity + LLSD_SCAN_WORKING_BYTES + bound - 1] {
            let budget = http::ResponseMemoryBudget::new(limit, 0, 0, 0);
            let raw = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_capacity).expect("admit raw response body");
            let client = http::Client::new(Some(TEST_TIMEOUT), false, None)
                .expect("build HTTP client")
                .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);

            let error = match admit_capability_decode_memory(&body, &client) {
                Ok(_) => panic!("preflight must reject insufficient memory"),
                Err(error) => error,
            };
            assert!(matches!(error, CapabilityHttp(http::Error::ResourceExhausted { .. })));
            drop(raw);
            drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).expect("failed admission returns all memory"));
        }
    }

    #[test]
    fn dropped_reply_releases_admission() {
        let body = b"body".to_vec();
        let body_len = body.len();
        let budget = http::ResponseMemoryBudget::new(body_len, 0, 0, 0);
        let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_len).expect("reserve response memory");
        let requests = Arc::new(Semaphore::new(1));
        let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
        let response = AdmittedResponse { body, _memory: Some(memory), _request: request };
        let (reply, waiting) = oneshot::channel();

        assert!(reply.send(response).is_ok(), "caller should be able to retain the admitted reply");
        assert_eq!(requests.available_permits(), 0);
        assert!(budget.try_reserve(http::ResponseMemoryClass::Ordinary, 1).is_err());

        drop(waiting);

        assert_eq!(requests.available_permits(), 1);
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_len).expect("dropping reply releases response memory"));
    }

    #[tokio::test]
    async fn debug_redacts_seed() {
        let seed = Url::parse("https://cap.example/secret-token").expect("valid seed capability URL");
        let client = Client::new(seed).expect("active Tokio runtime");

        let debug = format!("{client:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret-token"));
        let event = Event::SimulatorCapabilities { simulator_id: SimulatorId::new(1), source: test_source(), capabilities: client };
        let debug = format!("{event:?}");
        assert!(!debug.contains("secret-token"));
    }

    #[tokio::test]
    async fn errors_redact_urls() {
        let seed = Url::parse("https://user:password@caps.example/secret-token").expect("valid URL with rejected userinfo");
        let error = Client::new(seed).expect_err("embedded credentials should be rejected");
        let debug = format!("{error:?}");

        assert!(matches!(error, Capability(_)));
        assert!(!debug.contains("user"));
        assert!(!debug.contains("password"));
        assert!(!debug.contains("secret-token"));
    }

    #[tokio::test]
    async fn discovered_urls_are_opaque() {
        let body = "<llsd><map><key>TestCapability</key><string>https://caps.example/opaque-token</string></map></llsd>";
        let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let capabilities = client.request_capabilities(&["TestCapability"]).await.expect("capability discovery");
        let capability = capabilities.get("TestCapability").expect("discovered capability");
        let debug = format!("{capability:?}");
        let with_query = capability.with_query_pair("asset_id", "private-value");

        assert!(debug.contains("REDACTED"));
        assert!(!debug.contains("opaque-token"));
        assert!(!format!("{with_query:?}").contains("private-value"));
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn get_and_post_return_llsd_values_for_consumer_conversion() {
        #[derive(Debug, Eq, PartialEq)]
        struct Reply {
            count: i32,
        }

        #[derive(Serialize)]
        struct Request {
            count: i32,
        }

        let body = "<?xml version=\"1.0\"?><llsd><map><key>count</key><integer>7</integer></map></llsd>";
        let server = LocalServer::start(vec![
            response(200, "OK", "application/llsd+xml", None, body),
            response(200, "OK", "application/llsd+xml", None, body),
        ]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");

        let posted = client.post(&url, &Request { count: 7 }).await.expect("LLSD POST response");
        let fetched = client.get(&url).await.expect("LLSD GET response");
        let requests = server.finish();

        let into_reply = |value: Value| {
            let count = value.as_map().and_then(|fields| fields.get("count")).and_then(Value::as_integer).expect("integer count field");
            Reply { count }
        };
        assert_eq!(into_reply(posted), Reply { count: 7 });
        assert_eq!(into_reply(fetched), Reply { count: 7 });
        let post = str::from_utf8(&requests[0]).expect("POST is UTF-8");
        assert!(post.starts_with("POST /capability HTTP/1.1"));
        let (_, post_body) = post.split_once("\r\n\r\n").expect("POST has a body");
        let payload: Value = xml::from_str(post_body).expect("POST body is valid LLSD XML");
        assert_eq!(payload.as_map().and_then(|map| map.get("count")), Some(&Value::Integer(7)));
        assert!(str::from_utf8(&requests[1]).expect("GET is UTF-8").starts_with("GET /capability HTTP/1.1"));
    }

    #[tokio::test]
    async fn large_valid_capability_value_is_decoded_with_memory_admission() {
        const PAYLOAD_BYTES: usize = 1024 * 1024;

        let payload = "x".repeat(PAYLOAD_BYTES);
        let body = format!("<llsd><string>{payload}</string></llsd>");
        let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, &body)]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let url = CapabilityUrl::parse(server.url("/large").as_str()).expect("valid capability URL");

        let value = client.get(&url).await.expect("admit and decode a 1 MiB LLSD value");

        assert_eq!(value, Value::String(payload));
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn ordinary_post_is_not_replayed_after_a_server_error() {
        let server = LocalServer::start(vec![response(503, "Service Unavailable", "text/plain", Some("0"), "try again")]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let url = CapabilityUrl::parse(server.url("/side-effect").as_str()).expect("valid capability URL");

        let error = client.post(&url, &Value::String("one-shot".into())).await.expect_err("ordinary POST reports the server failure");

        assert!(matches!(
            &error,
            CapabilityHttp(http::Error::HttpStatus {
                status: StatusCode::SERVICE_UNAVAILABLE,
                retry_after: Some(retry_after),
                elapsed,
                ..
            }) if *retry_after == Duration::ZERO && !elapsed.is_zero()
        ));
        assert!(!format!("{error:?}").contains("try again"));
        assert_eq!(server.finish().len(), 1, "ordinary capability POSTs are never retried automatically");
    }

    #[tokio::test]
    async fn partial_grants() {
        let body = concat!("<llsd><map><key>GrantedCapability</key>", "<uri>http://caps.example/granted</uri></map></llsd>");
        let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
        let seed = server.url("/seed");

        let client = Client::new(seed.clone()).expect("active Tokio runtime");
        let capabilities = client.request_capabilities(&["GrantedCapability", "MissingCapability"]).await.expect("partial grant response");

        assert_eq!(capabilities.len(), 1);
        assert!(capabilities.contains_key("GrantedCapability"));
        let request = server.finish().remove(0);
        let request = str::from_utf8(&request).expect("request is UTF-8");
        let (headers, body) = request.split_once("\r\n\r\n").expect("request has a body");

        assert!(headers.starts_with("POST /seed HTTP/1.1"));
        assert!(headers.to_ascii_lowercase().contains("application/llsd+xml"));
        assert!(body.contains("GrantedCapability"));
        assert!(body.contains("MissingCapability"));
    }

    #[tokio::test]
    async fn empty_grants() {
        let server =
            LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, "<?xml version=\"1.0\"?><llsd><map></map></llsd>")]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

        let capabilities = client.request_capabilities(&["MissingCapability"]).await.expect("empty grants are a valid response");

        assert!(capabilities.is_empty());
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn oversized_declared_seed_body_is_rejected_before_reading() {
        let server = LocalServer::start(vec![content_length_only_response(MAX_SEED_CAPABILITY_RESPONSE_BYTES + 1)]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

        let error = client.request_capabilities(&["TestCapability"]).await.expect_err("oversized seed response should fail");
        assert!(matches!(&error, CapabilityHttp(http::Error::ResourceExhausted { status: Some(StatusCode::OK), .. })));

        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn oversized_declared_capability_body_is_rejected_before_reading() {
        let server = LocalServer::start(vec![content_length_only_response(MAX_CAPABILITY_RESPONSE_BYTES + 1)]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

        let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
        let error = client.post(&url, &Value::Undefined).await.expect_err("oversized LLSD response should fail");
        assert!(matches!(&error, CapabilityHttp(http::Error::ResourceExhausted { status: Some(StatusCode::OK), .. })));

        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn request_body_is_bounded() {
        let server = LocalServer::start(vec![]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
        let payload = Value::String("N".repeat(MAX_CAPABILITY_REQUEST_BODY_BYTES + 1));

        let error = client.post(&url, &payload).await.expect_err("an oversized request body should fail");

        assert!(matches!(error, Capability(_)));
        assert!(server.finish().is_empty(), "an oversized request must not be sent");
    }

    #[tokio::test]
    async fn chunked_event_limit() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Transfer-Encoding: chunked\r\n",
            "Connection: close\r\n\r\n",
            "3\r\nabc\r\n",
            "3\r\ndef\r\n",
            "0\r\n\r\n"
        );
        let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
        let client = http::Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

        let error = match post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 5).await {
            Err(error) => error,
            Ok(_) => panic!("chunked response exceeds limit"),
        };

        assert!(error.to_string().contains("5-byte limit"));
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn bad_cap_urls_are_rejected() {
        let body = concat!("<llsd><map><key>TestCapability</key><string>", "ftp://caps.example.net/capability", "</string></map></llsd>");
        let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

        let error = client.request_capabilities(&["TestCapability"]).await.expect_err("FTP capability URLs are not supported");
        assert!(matches!(&error, Capability(_)));
        assert!(error.to_string().contains("invalid TestCapability capability"));
        server.finish();
    }

    #[tokio::test]
    async fn capability_names_are_bounded() {
        let server = LocalServer::start(vec![
            response(200, "OK", "application/llsd+xml", None, "<llsd><map/></llsd>"),
            response(200, "OK", "application/llsd+xml", None, "<llsd><map/></llsd>"),
        ]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

        let maximum_count = vec!["N"; 256];
        assert!(
            client.request_capabilities(&maximum_count).await.expect("256 names at the count limit are accepted").is_empty(),
            "an empty seed response grants no capabilities"
        );

        let too_many = vec!["N"; 257];
        let error = client.request_capabilities(&too_many).await.expect_err("too many names should be rejected");
        assert!(error.to_string().contains("at most 256 names"));

        let long_name = "N".repeat(129);
        let error = client.request_capabilities(&[&long_name]).await.expect_err("a long name should be rejected");
        assert!(error.to_string().contains("128-byte limit"));

        let total_names = vec!["N".repeat(128); 129];
        let total_name_refs = total_names.iter().map(String::as_str).collect::<Vec<_>>();
        let error = client.request_capabilities(&total_name_refs).await.expect_err("the total name bytes should be rejected");
        assert!(error.to_string().contains("16384-byte total limit"));

        let maximum_name = "N".repeat(128);
        assert!(
            client.request_capabilities(&[&maximum_name]).await.expect("a name at the byte limit is accepted").is_empty(),
            "an empty seed response grants no capabilities"
        );
        assert_eq!(server.finish().len(), 2, "invalid names must not send a request");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn requests_limit_concurrency() {
        let request_count = MAX_CONCURRENT_CAPABILITY_REQUESTS * 2;
        let server = ConcurrentCapabilityTestServer::start(request_count, Duration::from_millis(75));
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let mut requests = JoinSet::new();

        for _ in 0..request_count {
            let client = client.clone();
            requests.spawn(async move { client.request_capabilities(&["TestCapability"]).await });
        }

        timeout(TEST_TIMEOUT, async {
            while let Some(request) = requests.join_next().await {
                request.expect("capability request task should finish").expect("local seed request should succeed");
            }
        })
        .await
        .expect("bounded capability requests should finish");

        let maximum_active = server.finish();
        assert!(maximum_active > 1, "capability requests should remain concurrent");
        assert!(maximum_active <= MAX_CONCURRENT_CAPABILITY_REQUESTS, "too many HTTP requests ran at once");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shared_budget_bounds_requests() {
        let request_count = MAX_CONCURRENT_CAPABILITY_REQUESTS * 2;
        let server = ConcurrentCapabilityTestServer::start(request_count, Duration::from_millis(150));
        let budget = CapabilityBudget::new();
        let clients = (0..2)
            .map(|_| {
                Client::new_with_status_and_budget(server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                    .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let mut requests = JoinSet::new();

        for index in 0..request_count {
            let client = clients[index % clients.len()].clone();
            requests.spawn(async move { client.request_capabilities(&["TestCapability"]).await });
        }

        timeout(TEST_TIMEOUT, async {
            while let Some(request) = requests.join_next().await {
                request.expect("capability request task should finish").expect("local seed request should succeed");
            }
        })
        .await
        .expect("shared budget requests should finish");

        let maximum_active = server.finish();
        assert!(maximum_active > 1, "the shared budget should allow concurrent requests");
        assert!(maximum_active <= MAX_CONCURRENT_CAPABILITY_REQUESTS, "shared clients exceeded their request budget");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shared_budget_bounds_event_polls() {
        let task_count = MAX_CONCURRENT_EVENT_QUEUES + 1;
        let server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let clients = (0..2)
            .map(|_| {
                Client::new_with_status_and_budget(server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                    .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let (events, received_events) = mpsc::channel(1);
        let mut received_events = EventReceiver::new(received_events);
        events.try_send(EventEnvelope::new(Event::SendFailed("hold event forwarding".into()))).expect("fill the application event channel");
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut tasks = Vec::with_capacity(task_count);

        for index in 0..task_count {
            let client = clients[index % clients.len()].clone();
            let task = client
                .start_event_queue_with_role(
                    SimulatorId::new(index as u64 + 1),
                    test_source(),
                    events.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                    index == 0,
                )
                .expect("start event queue");
            tasks.push(task);
        }

        let mut polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
        for _ in 0..task_count {
            server.next_seed();
        }
        let unexpected_poll = server.polls.recv_timeout(Duration::from_millis(150)).ok();
        let admitted = unexpected_poll.is_none();
        let mut forwarded_batch = false;

        if admitted {
            let wait_started = Instant::now();
            write_response(
                polls.pop().expect("one event poll to release"),
                response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH),
            );
            assert!(matches!(
                timeout(TEST_TIMEOUT, received_events.recv()).await.expect("held event channel item").expect("event channel remains open"),
                Event::SendFailed(message) if message == "hold event forwarding"
            ));
            timeout(TEST_TIMEOUT, async {
                while received_events.is_empty() {
                    yield_now().await;
                }
            })
            .await
            .expect("first event should fill the application channel");
            let no_poll_while_forwarding = server.polls.recv_timeout(Duration::from_millis(150)).is_err();
            let batch_event = timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("complete decoded batch should be forwarded")
                .expect("event channel remains open");
            let next_poll = server.next_poll();
            let observed_wait = wait_started.elapsed();
            eprintln!(
                "observed EventQueue wait from confirmed contention to next server poll (includes slot release and HTTP handling): {observed_wait:?}"
            );
            polls.push(next_poll);
            forwarded_batch = no_poll_while_forwarding
                && matches!(batch_event, Event::CapabilityEventBatch { events, .. }
                    if events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]);
        } else if let Some(poll) = unexpected_poll {
            polls.push(poll);
        }

        shutdown.send_replace(true);
        drop(polls);

        timeout(TEST_TIMEOUT, async {
            for task in tasks {
                assert_eq!(task.wait().await, EventQueueExit::Stopped);
            }
        })
        .await
        .expect("all event queue tasks should stop");
        server.stop();
        server.finish();

        assert!(admitted, "a shared EventQueueGet permit must hold until the batch is decoded and forwarded");
        assert!(forwarded_batch, "the permit must remain held until every decoded event is forwarded");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shared_budget_covers_event_queue_and_capture() {
        let _decode_gate_test = DECODE_GATE_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        let event_server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                hold_response(),
            ]
        });
        let ordinary_body = "<llsd><integer>42</integer></llsd>";
        let ordinary_server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, ordinary_body)]);
        let mut budget = CapabilityBudget::new();
        let ordinary_memory_limit = 64 * 1024;
        let event_queue_memory_limit = 2 * 1024 * 1024;
        let capture_memory_limit = 64 * 1024;
        budget.response_memory = http::ResponseMemoryBudget::new(ordinary_memory_limit, event_queue_memory_limit, capture_memory_limit, 0);
        let ordinary_request_capacity = budget.ordinary_requests.available_permits();
        let event_queue_capacity = available_event_queue_permits(&budget);
        let queued_event_capacity = budget.queued_event_bytes.available_permits();
        let recorder = CaptureRecorder::new();
        let capture_directory = tempfile::tempdir().expect("create capture directory");
        let capture = CaptureSession::start(CaptureConfig::new(capture_directory.path().join("captures")), recorder.clone())
            .expect("start network capture");
        let client =
            Client::new_with_status_and_budget(event_server.url("/seed"), Some(recorder), NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let (event_sender, mut received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start EventQueue");

        let seed_request = event_server.next_request();
        let first_poll = event_server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&first_poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));

        let batch = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("EventQueue response should be decoded")
            .expect("event channel remains open");
        assert!(matches!(&batch.event, Event::CapabilityEventBatch { events, .. } if events.len() == 2));
        assert!(budget.queued_event_bytes.available_permits() < queued_event_capacity, "decoded batch must retain its queue reservation");
        let second_poll = event_server.next_request();
        assert_eq!(map_field(&decode_request_body(&second_poll), "ack"), Some(&Value::Integer(5)));
        assert_eq!(available_event_queue_permits(&budget), event_queue_capacity - 1, "long poll holds its concurrency slot");

        let ordinary_url = CapabilityUrl::parse(ordinary_server.url("/ordinary").as_str()).expect("valid ordinary capability URL");
        let (gate, entered_receiver) = DecodeGate::new();
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = Some(gate.clone());
        let ordinary_client = client.clone();
        let ordinary_task = spawn(async move { ordinary_client.get(&ordinary_url).await });

        assert!(entered_receiver.recv_timeout(TEST_TIMEOUT).is_ok(), "ordinary response should decode while EventQueue is polling");
        assert!(budget.response_memory.try_reserve(http::ResponseMemoryClass::Ordinary, ordinary_memory_limit).is_err());
        assert!(available_event_queue_permits(&budget) < event_queue_capacity);
        gate.release();
        assert_eq!(
            timeout(TEST_TIMEOUT, ordinary_task)
                .await
                .expect("ordinary decode should finish")
                .expect("ordinary request task")
                .expect("ordinary response should decode"),
            Value::Integer(42)
        );
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = None;
        assert!(
            budget.response_memory.try_reserve(http::ResponseMemoryClass::Ordinary, ordinary_memory_limit).is_ok(),
            "ordinary raw-response admission returns after decode"
        );
        assert_eq!(ordinary_server.finish().len(), 1);

        shutdown.send_replace(true);
        assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("EventQueue should stop"), EventQueueExit::Stopped);
        let event_queue_memory = budget
            .response_memory
            .try_reserve(http::ResponseMemoryClass::EventQueue, event_queue_memory_limit)
            .expect("decode-memory admission returns when ownership transfers to the queued-event budget");
        drop(event_queue_memory);
        assert!(
            budget.queued_event_bytes.available_permits() < queued_event_capacity,
            "the queued batch keeps its retained-event reservation until the envelope is dropped"
        );
        drop(batch);
        assert_eq!(event_server.finish().len(), 0, "the seed and both polls were already observed");
        assert_eq!(budget.ordinary_requests.available_permits(), ordinary_request_capacity);
        assert_eq!(available_event_queue_permits(&budget), event_queue_capacity);
        assert_eq!(budget.queued_event_bytes.available_permits(), queued_event_capacity);
        let event_queue_memory = budget
            .response_memory
            .try_reserve(http::ResponseMemoryClass::EventQueue, event_queue_memory_limit)
            .expect("EventQueue response admission returns after batch drop and shutdown");
        drop(event_queue_memory);

        let summary = capture.finish_recording(&CapturePublication::default());
        assert_eq!(summary.state, CaptureState::Complete, "capture should finish: {:?}", summary.error);
        let capture_path = summary.capture_path.unwrap_or_else(|| panic!("capture archive path: {:?}", summary.error));
        let mut archive = ZipArchive::new(fs::File::open(capture_path).expect("open capture archive")).expect("decode capture archive");
        let mut manifest_bytes = Vec::new();
        archive.by_name("manifest.json").expect("capture manifest").read_to_end(&mut manifest_bytes).expect("read capture manifest");
        let manifest: CaptureManifest = serde_json::from_slice(&manifest_bytes).expect("decode capture manifest");
        let mut event_bytes = Vec::new();
        archive.by_name("events.jsonl").expect("capture events").read_to_end(&mut event_bytes).expect("read capture events");
        let mut payload_bytes = Vec::new();
        archive.by_name("payloads.bin").expect("capture payloads").read_to_end(&mut payload_bytes).expect("read capture payloads");
        assert_eq!(manifest.payload_bytes, payload_bytes.len() as u64);
        let events = event_bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice::<CaptureEvent>(line).expect("decode capture event"))
            .collect::<Vec<_>>();
        let request_ids = events
            .iter()
            .filter_map(|event| match &event.kind {
                CaptureEventKind::HttpRequest { request_id, .. } => Some(*request_id),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut ordinary_response_captured = false;
        let mut event_queue_response_captured = false;
        for event in &events {
            let CaptureEventKind::HttpResponse { request_id, status: 200, .. } = &event.kind else {
                continue;
            };
            assert!(request_ids.contains(request_id), "captured HTTP response must match a captured request");
            let Some(reference) = event.payloads.first() else {
                continue;
            };
            let start = usize::try_from(reference.offset).expect("capture payload offset fits usize");
            let length = usize::try_from(reference.length).expect("capture payload length fits usize");
            let end = start.checked_add(length).expect("capture payload range does not overflow");
            let body = payload_bytes.get(start..end).expect("captured response payload is in range");
            ordinary_response_captured |= body == ordinary_body.as_bytes();
            event_queue_response_captured |= body == TWO_EVENT_BATCH.as_bytes();
        }
        assert!(ordinary_response_captured, "capture must contain the exact ordinary HTTP response body");
        assert!(event_queue_response_captured, "capture must contain the exact EventQueue HTTP response body");
        drop(client);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "manual process-wide allocator sample; run with cargo test -p network --lib -- --ignored event_queue_peak_with_ordinary_live --nocapture"]
    async fn event_queue_peak_with_ordinary_live() {
        event_queue_peak_sample(false).await;
        event_queue_peak_sample(true).await;
    }

    async fn event_queue_peak_sample(capture_enabled: bool) {
        const EVENT_QUEUE_COUNT: usize = MAX_CONCURRENT_EVENT_QUEUES;
        const ORDINARY_REQUEST_COUNT: usize = MAX_CONCURRENT_CAPABILITY_REQUESTS;
        const EVENT_COUNT: usize = 4;
        const EVENT_PAYLOAD_BYTES: usize = MAX_QUEUED_CAPABILITY_EVENT_BYTES - 16 * 1024;
        const ORDINARY_PAYLOAD_BYTES: usize = 64 * 1024;

        let _decode_gate_test = DECODE_GATE_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().await;
        let event_payload = "e".repeat(EVENT_PAYLOAD_BYTES);
        let mut event_body = String::from("<llsd><map><key>id</key><integer>6</integer><key>events</key><array>");
        for _ in 0..EVENT_COUNT {
            event_body.push_str("<map><key>message</key><string>ObjectUpdate</string><key>body</key><map><key>payload</key><string>");
            event_body.push_str(&event_payload);
            event_body.push_str("</string></map></map>");
        }
        event_body.push_str("</array></map></llsd>");
        let ordinary_payload = "o".repeat(ORDINARY_PAYLOAD_BYTES);
        let ordinary_body = format!("<llsd><string>{ordinary_payload}</string></llsd>");
        let event_value_bound = xml::value_allocation_bound(event_body.as_bytes(), super::super::request::capability_llsd_limits())
            .expect("bound EventQueue XML decode");
        let event_batch_bound = event_batch_allocation_bound(event_body.as_bytes()).expect("bound EventQueue batch decode");
        let event_response = response(200, "OK", "application/llsd+xml", None, &event_body);
        let ordinary_response = response(200, "OK", "application/llsd+xml", None, &ordinary_body);
        let event_server = EventQueueBudgetTestServer::start();
        let ordinary_server = LocalServer::start(vec![ordinary_response; ORDINARY_REQUEST_COUNT]);
        let budget = CapabilityBudget::new();
        let queued_event_capacity = budget.queued_event_bytes.available_permits();
        let (capture, recorder, _capture_directory) = if capture_enabled {
            let recorder = CaptureRecorder::new();
            let directory = tempfile::tempdir().expect("create capture directory");
            let capture = CaptureSession::start(CaptureConfig::new(directory.path().join("captures")), recorder.clone())
                .expect("start network capture");
            (Some(capture), Some(recorder), Some(directory))
        } else {
            (None, None, None)
        };
        let client =
            Client::new_with_status_and_budget(event_server.url("/seed"), recorder, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let (event_sender, mut received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut event_tasks = Vec::with_capacity(EVENT_QUEUE_COUNT);
        for index in 0..EVENT_QUEUE_COUNT {
            let task = client
                .start_event_queue_with_role(
                    SimulatorId::new(index as u64 + 1),
                    test_source(),
                    event_sender.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                    index == 0,
                )
                .expect("start EventQueue");
            event_tasks.push(task);
        }

        let mut event_polls = Vec::with_capacity(EVENT_QUEUE_COUNT);
        for _ in 0..EVENT_QUEUE_COUNT {
            event_server.next_seed();
            event_polls.push(event_server.next_poll());
        }
        assert_eq!(available_event_queue_permits(&budget), 0, "all configured EventQueue slots should be occupied");

        let ordinary_url = CapabilityUrl::parse(ordinary_server.url("/ordinary").as_str()).expect("valid capability URL");
        let (gate, entered_receiver) = DecodeGate::new();
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = Some(gate.clone());
        let mut ordinary_tasks = Vec::with_capacity(ORDINARY_REQUEST_COUNT);
        for _ in 0..ORDINARY_REQUEST_COUNT {
            let ordinary_client = client.clone();
            let ordinary_url = ordinary_url.clone();
            ordinary_tasks.push(spawn(async move { ordinary_client.get(&ordinary_url).await }));
        }
        for _ in 0..ORDINARY_REQUEST_COUNT {
            entered_receiver.recv_timeout(TEST_TIMEOUT).expect("ordinary response should pause with its body admitted");
        }
        assert_eq!(budget.ordinary_requests.available_permits(), 0, "all configured ordinary request slots should be occupied");

        let measurement = begin_process_measurement();
        thread::scope(|scope| {
            for mut poll in event_polls {
                let event_response = &event_response;
                scope.spawn(move || poll.write_all(event_response).expect("write EventQueue response"));
            }
        });

        let mut batches = Vec::with_capacity(EVENT_QUEUE_COUNT);
        for _ in 0..EVENT_QUEUE_COUNT {
            let batch = timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("EventQueue response should decode")
                .expect("event channel remains open");
            assert!(matches!(&batch.event, Event::CapabilityEventBatch { events, .. } if events.len() == EVENT_COUNT));
            batches.push(batch);
        }
        let queued_event_bytes_remaining = budget.queued_event_bytes.available_permits();
        assert!(queued_event_bytes_remaining < queued_event_capacity, "decoded batches should retain their queue reservations");

        shutdown.send_replace(true);
        event_server.stop();
        for task in event_tasks {
            assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("EventQueue should stop"), EventQueueExit::Stopped);
        }
        event_server.finish();

        if let Some(capture) = capture {
            let summary = capture.finish_recording(&CapturePublication::default());
            assert_eq!(summary.state, CaptureState::Complete, "capture should finish: {:?}", summary.error);
        }
        let (baseline_bytes, live_delta_bytes, peak_bytes, elapsed_ns) = measurement.finish();
        let peak_delta_bytes = peak_bytes.saturating_sub(baseline_bytes);
        let total_batch_bound = event_batch_bound.saturating_mul(EVENT_QUEUE_COUNT);
        eprintln!(
            "Configured-concurrency EventQueue process allocation sample: capture={capture_enabled} event_queues={EVENT_QUEUE_COUNT} ordinary_requests={ORDINARY_REQUEST_COUNT} event_response={} B ordinary_response={} B events_per_batch={} value_bound_per_queue={} B batch_bound_per_queue={} B queued_event_bytes_remaining={} ordinary_limit={} B event_queue_limit={} B process_baseline={} B process_live_delta={} B process_peak={} B process_peak_delta={} B peak_delta_to_all_batch_bounds={:.2} elapsed={} ns. Four ordinary decoders remain paused with their bodies admitted while nine EventQueue responses are sent concurrently; all decoded EventQueue batches remain retained through the sample. Process-wide allocator sample includes test harness, Reqwest/Hyper, Tokio, and capture writer; excludes allocator overhead and RSS.",
            event_body.len(),
            ordinary_body.len(),
            EVENT_COUNT,
            event_value_bound,
            event_batch_bound,
            queued_event_bytes_remaining,
            MAX_CAPABILITY_RESPONSE_BYTES * ORDINARY_REQUEST_COUNT,
            (2 * MAX_EVENT_QUEUE_RESPONSE_BYTES) * EVENT_QUEUE_COUNT + MAX_QUEUED_CAPABILITY_EVENT_BYTES,
            baseline_bytes,
            live_delta_bytes,
            peak_bytes,
            peak_delta_bytes,
            peak_delta_bytes as f64 / total_batch_bound.max(1) as f64,
            elapsed_ns,
        );

        gate.release();
        for task in ordinary_tasks {
            assert_eq!(
                timeout(TEST_TIMEOUT, task)
                    .await
                    .expect("ordinary decode should finish")
                    .expect("ordinary request task")
                    .expect("ordinary response should decode"),
                Value::String(ordinary_payload.clone())
            );
        }
        *DECODE_GATE.get_or_init(Default::default).lock().expect("decode gate lock") = None;
        assert_eq!(ordinary_server.finish().len(), ORDINARY_REQUEST_COUNT);
        drop(client);
        drop(batches);
    }

    #[test]
    fn event_capture_decode_peak() {
        const EVENT_COUNT: usize = 4;
        const EVENT_PAYLOAD_BYTES: usize = 32 * 1024;
        const ORDINARY_PAYLOAD_BYTES: usize = 64 * 1024;

        let event_payload = "e".repeat(EVENT_PAYLOAD_BYTES);
        let mut event_template = String::from("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>");
        for _ in 0..EVENT_COUNT {
            event_template.push_str("<map><key>message</key><string>ObjectUpdate</string><key>body</key><map><key>payload</key><string>");
            event_template.push_str(&event_payload);
            event_template.push_str("</string></map></map>");
        }
        event_template.push_str("</array></map></llsd>");
        let event_template = event_template.into_bytes();
        let ordinary_payload = "o".repeat(ORDINARY_PAYLOAD_BYTES);
        let ordinary_template = format!("<llsd><map><key>payload</key><string>{ordinary_payload}</string></map></llsd>").into_bytes();

        let recorder = CaptureRecorder::new();
        let capture_directory = tempfile::tempdir().expect("create capture directory");
        let capture = CaptureSession::start(CaptureConfig::new(capture_directory.path().join("captures")), recorder.clone())
            .expect("start network capture");
        let request_id = recorder.http_request("EventQueueGet", "POST", Vec::new(), &[]);
        let capture_memory = http::ResponseMemoryBudget::new(0, 0, event_template.len(), 0);

        let ((event_body, ordinary_body, event_id, events, ordinary_value, allocation_bound), _, peak_bytes, elapsed_ns) =
            measure_allocations(|| {
                let event_body = event_template.clone();
                let ordinary_body = ordinary_template.clone();
                let allocation_bound = xml::value_allocation_bound(&event_body, super::super::request::capability_llsd_limits())
                    .expect("bound EventQueue XML decode");
                let (event_id, events) = decode_event_batch(&event_body).expect("decode the EventQueue batch");
                let ordinary_value: Value =
                    super::super::request::decode_capability_value(&ordinary_body).expect("decode the ordinary response");

                assert_eq!(event_id, 5);
                assert_eq!(events.len(), EVENT_COUNT);
                assert!(matches!(&ordinary_value, Value::Map(_)));

                let permit = capture_memory
                    .try_reserve(http::ResponseMemoryClass::Capture, event_body.len())
                    .expect("admit the captured EventQueue response");
                recorder.http_response_with_memory(
                    request_id,
                    Some(200),
                    Vec::new(),
                    Duration::ZERO,
                    "complete",
                    Some(&event_body),
                    Some(permit),
                );

                (event_body, ordinary_body, event_id, events, ordinary_value, allocation_bound)
            });

        assert_eq!(event_body.len(), event_template.len());
        assert_eq!(ordinary_body.len(), ordinary_template.len());
        assert!(peak_bytes >= (event_body.len() + ordinary_body.len()) as i128);
        eprintln!(
            "EventQueue/capture/decode allocation sample: event={} B ordinary={} B LLSD-bound={} B caller-thread-peak={} B elapsed={} ns",
            event_body.len(),
            ordinary_body.len(),
            allocation_bound,
            peak_bytes,
            elapsed_ns
        );

        drop((event_id, events, ordinary_value, event_body, ordinary_body));
        let summary = capture.finish_recording(&CapturePublication::default());
        assert_eq!(summary.state, CaptureState::Complete, "capture should finish: {:?}", summary.error);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_poll_wait_cancels_on_close() {
        let server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let clients = (0..2)
            .map(|_| {
                Client::new_with_status_and_budget(server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                    .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut tasks = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES);

        for index in 0..MAX_CONCURRENT_EVENT_QUEUES {
            let task = clients[0]
                .start_event_queue_with_role(
                    SimulatorId::new(index as u64 + 1),
                    test_source(),
                    events.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                    index == 0,
                )
                .expect("start admitted event queue");
            tasks.push(task);
        }
        let polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES {
            server.next_seed();
        }

        let waiting_client = clients[1].clone();
        let waiting_task = waiting_client
            .start_primary_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
                test_source(),
                events,
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start waiting event queue");
        server.next_seed();
        assert!(server.polls.recv_timeout(Duration::from_millis(150)).is_err(), "the waiting queue must not start a poll");

        waiting_client.close_session();
        let waiting_exit = timeout(Duration::from_millis(500), waiting_task.wait()).await;
        shutdown.send_replace(true);
        drop(polls);

        timeout(TEST_TIMEOUT, async {
            for task in tasks {
                assert_eq!(task.wait().await, EventQueueExit::Stopped);
            }
        })
        .await
        .expect("admitted event queues should stop");
        server.stop();
        server.finish();

        assert_eq!(waiting_exit.expect("session close should cancel permit acquisition"), EventQueueExit::Stopped);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn waiting_event_queue_gets_slot_after_active_poll_stops() {
        let active_server = EventQueueBudgetTestServer::start();
        let waiting_server = EventQueueBudgetTestServer::start();
        let primary_server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let active_client =
            Client::new_with_status_and_budget(active_server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let waiting_client =
            Client::new_with_status_and_budget(waiting_server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let primary_client = Client::new_with_status_and_budget(primary_server.url("/seed"), None, NetworkStatusHandle::default(), budget)
            .expect("active Tokio runtime");
        let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let mut active_tasks = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES - 1);

        for index in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            active_tasks.push(
                active_client
                    .start_event_queue(
                        SimulatorId::new(index as u64 + 1),
                        test_source(),
                        events.clone(),
                        shutdown_requested.clone(),
                        CoverageLossLedger::new(),
                    )
                    .expect("start admitted EventQueue"),
            );
        }
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            active_server.next_seed();
        }
        let mut active_polls = (0..MAX_CONCURRENT_EVENT_QUEUES - 1).map(|_| active_server.next_poll()).collect::<Vec<_>>();

        let waiting_task = waiting_client
            .start_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64),
                test_source(),
                events.clone(),
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start waiting EventQueue");
        waiting_server.next_seed();

        let primary_task = primary_client
            .start_primary_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
                test_source(),
                events,
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start primary EventQueue");
        primary_server.next_seed();
        let primary_poll =
            primary_server.polls.recv_timeout(Duration::from_secs(1)).expect("the primary EventQueue uses its reserved poll slot");
        assert!(
            waiting_server.polls.recv_timeout(Duration::from_millis(150)).is_err(),
            "neighbor polls cannot take the reserved primary slot"
        );

        active_polls
            .remove(0)
            .write_all(&response(404, "Not Found", "text/plain", None, "stopped"))
            .expect("stop one active EventQueue poll");
        let released_at = Instant::now();
        let waiting_poll = waiting_server
            .polls
            .recv_timeout(Duration::from_secs(1))
            .expect("a queued simulator should use the released EventQueue slot promptly");
        assert!(released_at.elapsed() < Duration::from_secs(1), "a queued simulator should use the released EventQueue slot promptly");

        shutdown.send_replace(true);
        drop((active_polls, waiting_poll, primary_poll));
        for task in active_tasks {
            assert_ne!(task.wait().await, EventQueueExit::TaskFailed, "active queue should stop without panicking");
        }
        assert_eq!(waiting_task.wait().await, EventQueueExit::Stopped);
        assert_eq!(primary_task.wait().await, EventQueueExit::Stopped);
        active_server.stop();
        waiting_server.stop();
        primary_server.stop();
        active_server.finish();
        waiting_server.finish();
        primary_server.finish();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn primary_event_queue_slot_follows_role_changes() {
        let neighbor_server = EventQueueBudgetTestServer::start();
        let old_primary_server = EventQueueBudgetTestServer::start();
        let new_primary_server = EventQueueBudgetTestServer::start();
        let budget = CapabilityBudget::new();
        let neighbor_client =
            Client::new_with_status_and_budget(neighbor_server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let old_primary_client =
            Client::new_with_status_and_budget(old_primary_server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime");
        let new_primary_client =
            Client::new_with_status_and_budget(new_primary_server.url("/seed"), None, NetworkStatusHandle::default(), budget)
                .expect("active Tokio runtime");
        let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let old_coverage_losses = CoverageLossLedger::new();
        let mut old_losses_changed = old_coverage_losses.subscribe();
        let mut neighbors = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES - 1);

        for index in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            neighbors.push(
                neighbor_client
                    .start_event_queue(
                        SimulatorId::new(index as u64 + 1),
                        test_source(),
                        events.clone(),
                        shutdown_requested.clone(),
                        CoverageLossLedger::new(),
                    )
                    .expect("start neighbor EventQueue"),
            );
        }
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
            neighbor_server.next_seed();
        }
        let neighbor_polls = (0..MAX_CONCURRENT_EVENT_QUEUES - 1).map(|_| neighbor_server.next_poll()).collect::<Vec<_>>();

        let old_primary = old_primary_client
            .start_primary_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64),
                test_source(),
                events.clone(),
                shutdown_requested.clone(),
                old_coverage_losses.clone(),
            )
            .expect("start old primary EventQueue");
        old_primary_server.next_seed();
        let mut old_poll = old_primary_server.next_poll();

        let partial_response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
            TWO_EVENT_BATCH.len() + 32,
            TWO_EVENT_BATCH
        );
        old_poll.write_all(partial_response.as_bytes()).expect("send a partial event batch from the old simulator");
        old_primary.role_handle().set_primary(false);
        timeout(Duration::from_secs(1), old_losses_changed.changed())
            .await
            .expect("canceling a live EventQueue poll must record uncertain coverage")
            .expect("coverage ledger remains available");
        assert!(old_coverage_losses.snapshot().losses.iter().any(|loss| {
            matches!(loss, crate::coverage::CoverageLoss::Source(record)
                if record.watermark.simulator_id == SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64)
                    && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission)
        }));
        let new_primary = new_primary_client
            .start_event_queue(
                SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
                test_source(),
                events,
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
            )
            .expect("start promoted EventQueue");
        new_primary.role_handle().set_primary(true);
        new_primary_server.next_seed();
        let new_poll = new_primary_server
            .polls
            .recv_timeout(Duration::from_secs(1))
            .expect("the promoted EventQueue acquires the released primary slot");

        new_primary.role_handle().set_primary(false);
        old_primary.role_handle().set_primary(true);
        let returned_poll =
            old_primary_server.polls.recv_timeout(Duration::from_secs(1)).expect("a re-promoted EventQueue reacquires the primary slot");

        shutdown.send_replace(true);
        drop((neighbor_polls, old_poll, new_poll, returned_poll));
        for task in neighbors {
            assert_ne!(task.wait().await, EventQueueExit::TaskFailed, "neighbor queue should stop without panicking");
        }
        assert_eq!(old_primary.wait().await, EventQueueExit::Stopped);
        assert_eq!(new_primary.wait().await, EventQueueExit::Stopped);
        neighbor_server.stop();
        old_primary_server.stop();
        new_primary_server.stop();
        neighbor_server.finish();
        old_primary_server.finish();
        new_primary_server.finish();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_frees_slot() {
        let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS + 1, Duration::from_secs(1));
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let mut requests = (0..MAX_CONCURRENT_CAPABILITY_REQUESTS)
            .map(|_| {
                let client = client.clone();
                spawn(async move { client.request_capabilities(&["TestCapability"]).await })
            })
            .collect::<Vec<_>>();

        for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
            server.next_request().await;
        }

        let client = client.clone();
        requests.push(spawn(async move { client.request_capabilities(&["TestCapability"]).await }));
        let cancelled = requests.remove(0);
        cancelled.abort();
        let _ = cancelled.await;

        timeout(Duration::from_millis(750), server.next_request()).await.expect("the canceled request should release its concurrency slot");

        for request in requests {
            request.abort();
            let _ = request.await;
        }
        server.finish();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_stops_client() {
        let server = EventQueueTestServer::start(|_| vec![hold_response()]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let retained_client = client.clone();
        let request = spawn(async move { retained_client.request_capabilities(&["TestCapability"]).await });
        let _ = server.next_request();

        client.close_session_and_wait().await;
        let error = timeout(TEST_TIMEOUT, request)
            .await
            .expect("closing the session should cancel the HTTP request")
            .expect("capability request task should finish")
            .expect_err("closed session cannot return capabilities");
        assert!(matches!(error, CapabilitySessionClosed));

        let error = client.request_capabilities(&["TestCapability"]).await.expect_err("retained client must reject new requests");
        assert!(matches!(error, CapabilitySessionClosed));
        assert!(server.finish().is_empty(), "no additional request should reach the server after session close");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn close_cancels_queue() {
        let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS, Duration::from_secs(1));
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let mut requests = (0..=MAX_CONCURRENT_CAPABILITY_REQUESTS)
            .map(|_| {
                let client = client.clone();
                spawn(async move { client.request_capabilities(&["TestCapability"]).await })
            })
            .collect::<Vec<_>>();

        for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
            server.next_request().await;
        }
        yield_now().await;
        client.close_session();

        for request in requests.drain(..) {
            let error = timeout(TEST_TIMEOUT, request)
                .await
                .expect("closing the session should end queued and active requests")
                .expect("capability request task should finish")
                .expect_err("closed session cannot return capabilities");
            assert!(matches!(error, CapabilitySessionClosed));
        }
        assert_eq!(server.finish(), MAX_CONCURRENT_CAPABILITY_REQUESTS, "only requests already in flight should reach the server");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_ack_advances_after_batch_admission() {
        const TWO_EVENT_RESPONSE: &str = "<llsd><map><key>id</key><integer>4</integer><key>events</key><array><map><key>message</key><string>FirstEvent</string></map><map><key>message</key><string>SecondEvent</string></map></array></map></llsd>";
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/opaque/event/queue-token");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_RESPONSE)),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        event_sender.try_send(EventEnvelope::new(Event::Failed("hold first event slot".into()))).expect("fill event channel");
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let seed_request = server.next_request();
        let first_poll = server.next_request();
        sleep(Duration::from_millis(100)).await;
        assert!(matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)), "ack must wait for batch admission");

        assert!(matches!(received_events.recv().await, Some(Event::Failed(_))));
        let batch = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("event queue should deliver one complete batch")
            .expect("event channel should remain open");
        let Event::CapabilityEventBatch { source, events, .. } = batch else {
            panic!("EventQueue response must be admitted as one complete batch");
        };
        assert_eq!(source, test_source());
        let mut events = events.into_iter();
        assert_eq!(events.next().expect("first event").message, "FirstEvent");
        drop(events);

        let second_poll = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_body(&seed_request).contains("EventQueueGet"));
        assert!(request_line(&first_poll).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
        let first_poll = decode_request_body(&first_poll);
        assert_eq!(map_field(&first_poll, "ack"), Some(&Value::Undefined));
        assert!(request_line(&second_poll).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
        let second_poll = decode_request_body(&second_poll);
        assert_eq!(map_field(&second_poll, "ack"), Some(&Value::Integer(4)));
        assert_eq!(map_field(&second_poll, "done"), Some(&Value::Boolean(false)));

        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("event queue should stop after shutdown");
        assert!(server.finish().is_empty(), "shutdown must stop further event polling");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_starts_neighbor() {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let primary_endpoint = primary.local_addr().expect("primary simulator address");
        let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let (ip, port) = match neighbor_endpoint {
            SocketAddr::V4(endpoint) => (endpoint.ip().octets(), endpoint.port()),
            SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
        };
        let agent_id = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
        let neighbor_seed_server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/neighbor-event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "stopped")),
            ]
        });
        let neighbor_seed = neighbor_seed_server.url("/neighbor-seed");
        let server = EventQueueTestServer::start(move |base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            let seed = seed_response(&event_url);
            let event = simulator_events(ip, port, agent_id, &neighbor_seed);
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed)),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &event)),
                hold_response(),
            ]
        });

        let session = AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address: primary_endpoint,
            agent_id,
            session_id: UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID"),
            circuit_code: 0x1122_3344,
            seed_capability: server.url("/seed"),
        };
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (disconnect, shutdown) = watch::channel(false);
        let worker = spawn(run_session(session, command_receiver, event_sender, shutdown));

        let (primary_packet, primary_peer) = receive_udp(&primary).await;
        assert!(matches!(Packet::decode(&primary_packet).expect("decode initial circuit").message(), Message::UseCircuitCode(_)));
        primary.send_to(&acknowledgement(40, 1), primary_peer).await.expect("ack primary circuit");
        let (movement_packet, movement_peer) = receive_udp(&primary).await;
        let movement_packet = Packet::decode(&movement_packet).expect("decode initial movement");
        assert!(matches!(movement_packet.message(), Message::CompleteAgentMovement(_)));
        primary.send_to(&acknowledgement(41, movement_packet.sequence()), movement_peer).await.expect("ack initial movement");
        let (neighbor_packet, neighbor_peer) = receive_udp(&neighbor).await;
        let neighbor_packet = Packet::decode(&neighbor_packet).expect("decode neighbor circuit");
        assert_eq!(neighbor_packet.sequence(), 1);
        assert!(neighbor_packet.is_reliable());
        assert!(matches!(neighbor_packet.message(), Message::UseCircuitCode(_)));
        neighbor.send_to(&acknowledgement(40, 1), neighbor_peer).await.expect("ack neighbor circuit");

        let mut saw_event_batch = false;
        let mut saw_capabilities = false;
        while !(saw_event_batch && saw_capabilities) {
            let event =
                timeout(TEST_TIMEOUT, received_events.recv()).await.expect("forward capability events").expect("network event channel");
            match event {
                Event::CapabilityEventBatch { events, .. } => {
                    assert!(!saw_event_batch, "one EventQueue response is published as one application batch");
                    assert_eq!(
                        events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>(),
                        ["EnableSimulator", "EstablishAgentCommunication"],
                        "batch items keep simulator wire order"
                    );
                    saw_event_batch = true;
                    assert!(matches!(
                        events[1].body.as_map().and_then(|fields| fields.get("seed-capability")),
                        Some(Value::String(seed)) if seed == "[redacted]"
                    ));
                }
                Event::SimulatorCapabilities { source, .. } if source == neighbor_endpoint => saw_capabilities = true,
                _ => {}
            }
        }
        assert!(request_line(&neighbor_seed_server.next_request()).starts_with("POST /neighbor-seed HTTP/1.1"));

        disconnect.send_replace(true);
        let (logout, _) = receive_udp(&primary).await;
        assert!(matches!(Packet::decode(&logout).expect("decode logout").message(), Message::LogoutRequest(_)));
        timeout(TEST_TIMEOUT, worker).await.expect("network worker shutdown").expect("network task").expect("clean shutdown");
        server.stop();
        let requests = server.finish();
        assert!(requests.len() >= 2, "fake received seed lookup and event poll");
        neighbor_seed_server.stop();
        let _ = neighbor_seed_server.finish();
        assert_ne!(primary_peer, neighbor_peer);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn crossed_region_promotes_after_matching_movement() {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination simulator");
        let primary_endpoint = primary.local_addr().expect("primary simulator address");
        let neighbor_endpoint = neighbor.local_addr().expect("destination simulator address");
        let SocketAddr::V4(neighbor_endpoint_v4) = neighbor_endpoint else { panic!("fake destination must use IPv4") };
        let agent_id = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
        let session_id = UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID");
        let region_handle = 0x1000_0000_0000_1000;
        let neighbor_seed_server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/neighbor-event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                hold_response(),
            ]
        });
        let neighbor_seed = neighbor_seed_server.url("/neighbor-seed");
        let primary_seed_server = EventQueueTestServer::start(move |base| {
            let mut event_url = base.clone();
            event_url.set_path("/primary-event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(
                    200,
                    "OK",
                    "application/llsd+xml",
                    None,
                    &crossed_region_events(agent_id, session_id, region_handle, neighbor_endpoint_v4, &neighbor_seed),
                )),
                hold_response(),
            ]
        });
        let session = AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address: primary_endpoint,
            agent_id,
            session_id,
            circuit_code: 0x1122_3344,
            seed_capability: primary_seed_server.url("/primary-seed"),
        };
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_disconnect, shutdown) = watch::channel(false);
        let worker = spawn(run_session(session, command_receiver, event_sender, shutdown));

        let (primary_packet, primary_peer) = receive_udp(&primary).await;
        assert!(matches!(Packet::decode(&primary_packet).expect("decode primary circuit").message(), Message::UseCircuitCode(_)));
        primary.send_to(&acknowledgement(40, 1), primary_peer).await.expect("ack primary circuit");
        let (movement, movement_peer) = receive_udp(&primary).await;
        let movement = Packet::decode(&movement).expect("decode primary movement request");
        assert!(matches!(movement.message(), Message::CompleteAgentMovement(_)));
        primary.send_to(&acknowledgement(41, movement.sequence()), movement_peer).await.expect("ack primary movement request");

        let mut primary_handshake =
            parse_hex_fixture(vectors::get("region_handshake.hex").expect("handshake fixture")).expect("parse handshake fixture");
        primary_handshake[0] |= 0x40;
        set_sequence(&mut primary_handshake, 22);
        primary.send_to(&primary_handshake, primary_peer).await.expect("send primary region handshake");
        let reply = receive_message(&primary, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        primary.send_to(&acknowledgement(42, reply.sequence()), primary_peer).await.expect("ack primary handshake reply");
        let mut primary_movement_complete =
            parse_hex_fixture(vectors::get("agent_movement_complete.hex").expect("movement completion fixture"))
                .expect("parse movement completion fixture");
        primary_movement_complete[0] |= 0x40;
        set_sequence(&mut primary_movement_complete, 23);
        primary.send_to(&primary_movement_complete, primary_peer).await.expect("complete primary movement");

        let primary_id = loop {
            match timeout(TEST_TIMEOUT, received_events.recv()).await.expect("receive primary events").expect("event channel remains open")
            {
                Event::SimulatorConnected { simulator_id, endpoint, region_handle: None } if endpoint == primary_endpoint => {
                    break simulator_id;
                }
                _ => {}
            }
        };
        let (neighbor_packet, neighbor_peer) = receive_udp(&neighbor).await;
        let neighbor_packet = Packet::decode(&neighbor_packet).expect("decode destination circuit");
        assert!(matches!(neighbor_packet.message(), Message::UseCircuitCode(_)));
        neighbor.send_to(&acknowledgement(40, neighbor_packet.sequence()), neighbor_peer).await.expect("ack destination circuit");
        let neighbor_id = loop {
            match timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("receive destination events")
                .expect("event channel remains open")
            {
                Event::SimulatorConnected { simulator_id, endpoint, region_handle: Some(handle) }
                    if endpoint == neighbor_endpoint && handle == region_handle =>
                {
                    break simulator_id;
                }
                _ => {}
            }
        };

        let mut neighbor_handshake =
            parse_hex_fixture(vectors::get("region_handshake.hex").expect("handshake fixture")).expect("parse handshake fixture");
        neighbor_handshake[0] |= 0x40;
        set_sequence(&mut neighbor_handshake, 41);
        neighbor.send_to(&neighbor_handshake, neighbor_peer).await.expect("send destination region handshake");
        let reply = receive_message(&neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        let movement = receive_message(&neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
        neighbor.send_to(&acknowledgement(42, reply.sequence()), neighbor_peer).await.expect("ack destination handshake reply");
        neighbor.send_to(&acknowledgement(43, movement.sequence()), neighbor_peer).await.expect("ack destination movement request");

        let wrong_movement = Packet::new(
            44,
            Message::AgentMovementComplete(AgentMovementComplete {
                agent_data: AgentData { agent_id, session_id },
                data: Data { region_handle: region_handle + 1, ..Default::default() },
                sim_data: SimData::default(),
            }),
        )
        .with_reliable()
        .encode()
        .expect("encode mismatched movement completion");
        neighbor.send_to(&wrong_movement, neighbor_peer).await.expect("send mismatched movement completion");
        let mut barrier = parse_hex_fixture(vectors::get("chat_from_simulator.hex").expect("chat fixture")).expect("parse chat fixture");
        barrier[0] |= 0x40;
        set_sequence(&mut barrier, 45);
        neighbor.send_to(&barrier, neighbor_peer).await.expect("send post-movement barrier");
        loop {
            match timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("receive before destination promotion")
                .expect("event channel remains open")
            {
                Event::SimulatorMessage { simulator_id, message: Message::ChatFromSimulator(_), .. } if simulator_id == neighbor_id => {
                    break;
                }
                Event::SimulatorPromoted { .. } => panic!("mismatched movement completion must not promote the destination"),
                _ => {}
            }
        }

        let matching_movement = Packet::new(
            46,
            Message::AgentMovementComplete(AgentMovementComplete {
                agent_data: AgentData { agent_id, session_id },
                data: Data { region_handle, ..Default::default() },
                sim_data: SimData::default(),
            }),
        )
        .with_reliable()
        .encode()
        .expect("encode matching movement completion");
        neighbor.send_to(&matching_movement, neighbor_peer).await.expect("send matching movement completion");
        let (previous, promoted, promoted_endpoint) = loop {
            if let Event::SimulatorPromoted { previous_simulator_id, simulator_id, session, .. } =
                timeout(TEST_TIMEOUT, received_events.recv())
                    .await
                    .expect("receive destination promotion")
                    .expect("event channel remains open")
            {
                break (previous_simulator_id, simulator_id, session.simulator_address);
            }
        };
        assert_eq!(previous, primary_id);
        assert_eq!(promoted, neighbor_id);
        assert_eq!(promoted_endpoint, neighbor_endpoint);

        commands
            .send(Command::SendMessage {
                message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 9 } }),
                delivery: Delivery::Unreliable,
            })
            .await
            .expect("send through the promoted simulator");
        let routed = receive_message(&neighbor, |message| matches!(message, Message::CompletePingCheck(_))).await;
        assert!(matches!(routed.message(), Message::CompletePingCheck(_)));

        drop(commands);
        let logout = receive_message(&neighbor, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(matches!(logout.message(), Message::LogoutRequest(_)));
        timeout(TEST_TIMEOUT, worker).await.expect("network worker shutdown").expect("network task").expect("clean shutdown");
        primary_seed_server.stop();
        let primary_requests = primary_seed_server.finish();
        assert!(primary_requests.len() >= 2, "primary event queue should receive its seed and poll requests");
        neighbor_seed_server.stop();
        let _ = neighbor_seed_server.finish();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn backpressure_stops_poll() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        event_sender.try_send(EventEnvelope::new(Event::Failed("hold the only downstream slot".into()))).expect("fill event channel");
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let seed_request = server.next_request();
        let poll_request = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&poll_request).starts_with("POST /event-queue HTTP/1.1"));
        let deadline = time::Instant::now() + Duration::from_millis(100);
        while time::Instant::now() < deadline {
            assert!(
                matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)),
                "poller requested another batch while blocked"
            );
            sleep(Duration::from_millis(10)).await;
        }

        assert!(matches!(received_events.try_recv(), Ok(Event::Failed(_))));
        assert!(
            matches!(timeout(TEST_TIMEOUT, received_events.recv()).await.expect("batch should enter Network intake"), Some(Event::CapabilityEventBatch { events, .. })
            if events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"])
        );
        let acknowledged_poll = server.next_request();
        assert!(request_line(&acknowledged_poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&acknowledged_poll), "ack"), Some(&Value::Integer(5)));
        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel the next held long poll");
        server.stop();
        assert!(server.finish().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fast_502_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(502, "Upstream error", "text/plain", None, "Upstream error: ")),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, mut received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let seed_request = server.next_request();
        let first_poll = server.next_request();
        sleep(Duration::from_millis(100)).await;
        assert!(matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)), "a fast 502 should use retry backoff");

        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("retry backoff should stop on shutdown");
        server.stop();
        assert!(server.finish().is_empty());
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&first_poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
        assert!(received_events.try_recv().is_err(), "empty batches should not emit events");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn bad_batch_not_dispatched() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/events");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, MALFORMED_BATCH)),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        event_sender
            .try_send(EventEnvelope::new(Event::Failed("hold the only event slot".into())))
            .expect("fill the event channel before the malformed response");
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed_request = server.next_request();
        let poll_request = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&poll_request).starts_with("POST /events HTTP/1.1"));
        timeout(TEST_TIMEOUT, loss_changes.changed())
            .await
            .expect("durable coverage loss should wake subscribers")
            .expect("coverage ledger remains open");
        assert!(matches!(
            received_events.try_recv(),
            Ok(Event::Failed(reason)) if reason == "hold the only event slot"
        ));
        let notification = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("loss notification should follow after a slot opens")
            .expect("event channel should remain open");
        assert!(matches!(
            notification,
            Event::EventQueueCoverageLost { simulator_id, source, reason }
                if simulator_id == SimulatorId::new(1) && source == test_source() && reason.contains("response")
        ));
        assert!(received_events.try_recv().is_err(), "one queue incarnation emits one loss notification");
        assert_eq!(
            task.wait().await,
            EventQueueExit::StoppedWithCoverageLoss,
            "an irrecoverable malformed batch stops this queue incarnation"
        );
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(loss)]
                if loss.watermark.simulator_id == SimulatorId::new(1)
                    && matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. })
                    && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                    && loss.retired
        ));

        assert!(server.finish().is_empty(), "a malformed batch must stop polling");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn redelivered_batch_is_suppressed() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/events");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let seed_request = server.next_request();
        let first_poll = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
        let first_batch = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("first response batch should be admitted")
            .expect("event channel should remain open");
        assert!(matches!(
            first_batch,
            Event::CapabilityEventBatch { events, .. }
                if events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]
        ));

        let redelivery_poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&redelivery_poll), "ack"), Some(&Value::Integer(5)));
        let next_poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&next_poll), "ack"), Some(&Value::Integer(5)));
        assert!(received_events.try_recv().is_err(), "the same response id in this queue incarnation must not republish its events");

        shutdown.send_replace(true);
        assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should stop the held poll"), EventQueueExit::Stopped);
        server.stop();
        assert!(server.finish().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn lost_batch_is_redelivered_on_new_queue_incarnation() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/events");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let simulator_id = SimulatorId::new(1);
        let source = test_source();
        let coverage_losses = CoverageLossLedger::new();
        let queued_event_capacity = client.budget.queued_event_bytes.available_permits();
        let batch_reservation_floor = queued_event_capacity
            .checked_sub(super::super::budget::MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES)
            .expect("session queue budget includes one EventQueue batch reservation");

        let (first_sender, first_receiver) = mpsc::channel(1);
        first_sender
            .try_send(EventEnvelope::new(Event::Failed("hold the first queue slot".into())))
            .expect("fill the first EventQueue channel");
        let mut first_receiver = EventReceiver::new(first_receiver);
        let (first_shutdown, first_shutdown_requested) = watch::channel(false);
        let first_task = client
            .start_event_queue(simulator_id, source, first_sender.clone(), first_shutdown_requested, coverage_losses.clone())
            .expect("start first EventQueue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        let first_poll = server.next_request();
        assert!(request_line(&first_poll).starts_with("POST /events HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
        timeout(TEST_TIMEOUT, async {
            while client.budget.queued_event_bytes.available_permits() <= batch_reservation_floor {
                yield_now().await;
            }
        })
        .await
        .expect("the returned batch should reach queue admission while the first channel is full");
        assert_eq!(first_sender.capacity(), 0, "the first queue slot remains occupied");

        first_shutdown.send_replace(true);
        assert_eq!(
            timeout(TEST_TIMEOUT, first_task.wait()).await.expect("the first EventQueue should stop after shutdown"),
            EventQueueExit::Stopped
        );
        assert!(
            matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)),
            "the unadmitted batch must not advance the ACK or start another poll"
        );
        assert!(matches!(
            first_receiver.try_recv(),
            Ok(Event::Failed(message)) if message == "hold the first queue slot"
        ));
        assert!(first_receiver.try_recv().is_err(), "the first queue must not publish a batch prefix or loss wake-up after shutdown");

        let first_loss = match coverage_losses.snapshot().losses.as_slice() {
            [crate::coverage::CoverageLoss::Source(loss)] => *loss,
            losses => panic!("expected one durable EventQueue coverage loss, got {losses:?}"),
        };
        assert_eq!(first_loss.watermark.simulator_id, simulator_id);
        assert!(matches!(first_loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. }));
        assert_eq!(first_loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
        assert!(first_loss.retired, "stopping the first queue retires its loss record");

        let (second_sender, second_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut second_receiver = EventReceiver::new(second_receiver);
        let (second_shutdown, second_shutdown_requested) = watch::channel(false);
        let second_task = client
            .start_event_queue(simulator_id, source, second_sender, second_shutdown_requested, coverage_losses.clone())
            .expect("start a fresh EventQueue incarnation");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        let redelivery_poll = server.next_request();
        assert!(request_line(&redelivery_poll).starts_with("POST /events HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&redelivery_poll), "ack"), Some(&Value::Undefined));
        let redelivered_batch = timeout(TEST_TIMEOUT, second_receiver.recv())
            .await
            .expect("the fresh queue should admit the redelivered batch")
            .expect("the second EventQueue channel should remain open");
        assert!(matches!(
            redelivered_batch,
            Event::CapabilityEventBatch { simulator_id: id, events, .. }
                if id == simulator_id
                    && events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]
        ));
        assert!(second_receiver.try_recv().is_err(), "the complete redelivery is published exactly once");

        let held_poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&held_poll), "ack"), Some(&Value::Integer(5)));
        second_shutdown.send_replace(true);
        assert_eq!(
            timeout(TEST_TIMEOUT, second_task.wait()).await.expect("the second EventQueue should stop cleanly"),
            EventQueueExit::Stopped
        );
        server.stop();
        assert!(server.finish().is_empty(), "all expected seed and poll requests were observed");
        assert!(
            matches!(
                coverage_losses.snapshot().losses.as_slice(),
                [crate::coverage::CoverageLoss::Source(current)]
                    if current.watermark == first_loss.watermark && current.retired
            ),
            "the first incarnation's loss remains pending for reconciliation"
        );
    }

    #[test]
    fn bad_event_fields() {
        for body in [
            "<llsd><map><key>events</key><array></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><string>not an array</string></map></llsd>",
            "<llsd><map><key>id</key><string>five</string><key>events</key><array></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><string>not an event map</string></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>body</key><string>missing message name</string></map></array></map></llsd>",
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><integer>5</integer></map></array></map></llsd>",
        ] {
            assert!(decode_event_batch(body.as_bytes()).is_err(), "malformed batch should be rejected");
        }
    }

    #[test]
    fn event_batch_limits() {
        let large_body = "x".repeat(MAX_QUEUED_CAPABILITY_EVENT_BYTES + 1);
        let xml = format!(
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>Large</string><key>body</key><string>{large_body}</string></map></array></map></llsd>"
        );
        let error = decode_event_batch(xml.as_bytes()).expect_err("oversized event body should be rejected");
        assert!(error.contains("event exceeded"));

        let one_event = "<map><key>message</key><string>x</string></map>";
        let events = one_event.repeat(1025);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let error = decode_event_batch(xml.as_bytes()).expect_err("oversized event batch should be rejected");
        assert!(error.contains("1024-event limit"));

        let events = one_event.repeat(1024);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let (_, decoded) = decode_event_batch(xml.as_bytes()).expect("the maximum event count is accepted");
        assert_eq!(decoded.len(), 1024);

        let mut exact_response = b"<llsd><map><key>id</key><integer>5</integer><key>events</key><array/></map></llsd>".to_vec();
        exact_response.resize(MAX_EVENT_QUEUE_RESPONSE_BYTES, b' ');
        let (_, decoded) = decode_event_batch(&exact_response).expect("a response at the byte limit is accepted");
        assert!(decoded.is_empty());

        exact_response.push(b' ');
        let error = decode_event_batch(&exact_response).expect_err("oversized response should be rejected");
        assert!(error.contains("byte limit"));
    }

    #[test]
    fn event_data_limits() {
        let nested = format!("{}<undef/>{}", "<array>".repeat(65), "</array>".repeat(65));
        let xml = format!("<llsd>{nested}</llsd>");
        let error = decode_event_batch(xml.as_bytes()).expect_err("excessive LLSD nesting should be rejected before parsing");
        assert!(error.contains("nesting exceeded"));

        let body = "x".repeat(200_000);
        let one_event = format!("<map><key>message</key><string>Large</string><key>body</key><string>{body}</string></map>");
        let events = one_event.repeat(42);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let error = decode_event_batch(xml.as_bytes()).expect_err("aggregate event data should be bounded");
        assert!(error.contains("retained-data limit"));
    }

    #[test]
    fn map_budget() {
        let fields = (0..2_000).map(|index| format!("<key>field{index:04}</key><integer>1</integer>")).collect::<String>();
        let xml = format!(
            "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>CompactMap</string><key>body</key><map>{fields}</map></map></array></map></llsd>"
        );
        let Err(error) = decode_event_batch(xml.as_bytes()) else {
            panic!("decoded map storage should exceed the event memory limit");
        };
        assert!(error.contains("event exceeded"), "{error}");
    }

    #[test]
    fn xml_element_limit() {
        let values = "<undef/>".repeat(MAX_LLSD_XML_ELEMENTS);
        let xml = format!("<llsd><array>{values}</array></llsd>");
        let Err(error) = decode_event_batch(xml.as_bytes()) else {
            panic!("excessive LLSD values should be rejected during preflight");
        };
        assert!(error.contains("element limit"));
    }

    #[test]
    fn viewer_retry_backoff() {
        assert_eq!(event_queue_retry_delay(0), Duration::from_secs(1));
        assert_eq!(event_queue_retry_delay(1), Duration::from_secs(4));
        assert_eq!(event_queue_retry_delay(2), Duration::from_secs(7));
        assert_eq!(event_queue_retry_delay(14), Duration::from_secs(43));
    }

    #[test]
    fn long_poll_error_classification() {
        for status in
            [StatusCode::INTERNAL_SERVER_ERROR, StatusCode::BAD_GATEWAY, StatusCode::SERVICE_UNAVAILABLE, StatusCode::GATEWAY_TIMEOUT]
        {
            assert!(!expected_empty_poll(status, Duration::from_secs(10) - Duration::from_nanos(1)));
            assert!(expected_empty_poll(status, Duration::from_secs(10)));
        }
        let linden_error = StatusCode::from_u16(499).expect("499 is a valid HTTP status");
        for status in [linden_error, StatusCode::NOT_FOUND, StatusCode::TOO_MANY_REQUESTS] {
            assert!(!expected_empty_poll(status, Duration::from_secs(10)));
            assert!(!expected_empty_poll(status, Duration::from_secs(30)));
        }
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            linden_error,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            assert!(retryable_event_queue_error(status), "HTTP {status} should retry as an ordinary error");
        }
        for status in [StatusCode::BAD_REQUEST, StatusCode::UNAUTHORIZED, StatusCode::NOT_FOUND] {
            assert!(!retryable_event_queue_error(status), "HTTP {status} should stop this poll incarnation");
        }
        let fast_timeout =
            http::Error::Timeout { elapsed: Duration::from_secs(9), status: None, retry_after: None, message: String::new() };
        let long_timeout =
            http::Error::Timeout { elapsed: Duration::from_secs(10), status: None, retry_after: None, message: String::new() };
        assert!(!fast_timeout.is_timeout_after(Duration::from_secs(10)));
        assert!(long_timeout.is_timeout_after(Duration::from_secs(10)));

        let partial_body_timeout = http::Error::Timeout {
            elapsed: Duration::from_secs(10),
            status: Some(StatusCode::OK),
            retry_after: None,
            message: String::new(),
        };
        assert!(!partial_body_timeout.is_timeout_after(Duration::from_secs(10)));
    }

    #[tokio::test]
    async fn poll_timeout_is_typed() {
        let server = EventQueueTestServer::start(|_| vec![hold_response()]);
        let client = http::Client::new(Some(Duration::from_millis(25)), false, None).expect("build short-timeout HTTP client");

        let error = post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 1024)
            .await
            .expect_err("held response should time out");
        assert!(error.is_timeout());
        assert_eq!(server.finish().len(), 1);
    }

    #[test]
    fn seed_backoff_caps() {
        assert_eq!(seed_capability_retry_delay(1), Duration::from_secs(1));
        assert_eq!(seed_capability_retry_delay(2), Duration::from_secs(2));
        assert_eq!(seed_capability_retry_delay(3), Duration::from_secs(4));
        assert_eq!(seed_capability_retry_delay(4), Duration::from_secs(8));
        assert_eq!(seed_capability_retry_delay(5), Duration::from_secs(8));
        assert_eq!(seed_retry_delay(2, None), Duration::from_secs(2));
        assert_eq!(seed_retry_delay(1, Some(Duration::ZERO)), Duration::ZERO);
        assert_eq!(seed_retry_delay(3, Some(Duration::from_secs(15))), Duration::from_secs(15));
        assert_eq!(seed_retry_delay(1, Some(Duration::from_secs(90))), Duration::from_secs(30));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn shutdown_cancels_long_poll() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/held-long-poll");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed = server.next_request();
        assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
        let poll = server.next_request();
        assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel the held request");
        assert!(coverage_losses.snapshot().losses.is_empty(), "an intentional shutdown is not a coverage loss");
        assert!(server.finish().is_empty());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn aborting_poll_records_coverage_loss() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/held-long-poll");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                hold_response(),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed = server.next_request();
        assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
        let poll = server.next_request();
        assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
        drop(task);

        let loss = timeout(TEST_TIMEOUT, async {
            loop {
                if let [crate::coverage::CoverageLoss::Source(loss)] = coverage_losses.snapshot().losses.as_slice()
                    && loss.retired
                {
                    break *loss;
                }
                loss_changes.changed().await.expect("coverage ledger remains open");
            }
        })
        .await
        .expect("aborting an in-flight poll should record and retire its source loss");
        assert_eq!(loss.watermark.simulator_id, simulator_id);
        assert!(matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. }));
        assert_eq!(loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
        let notification = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("aborting a poll should still wake the event receiver")
            .expect("event channel should remain open");
        assert!(matches!(
            notification,
            Event::EventQueueCoverageLost { simulator_id: id, source, reason }
                if id == simulator_id && source == test_source() && reason.contains("canceled")
        ));
        server.stop();
        assert!(server.finish().is_empty(), "the two observed requests were already consumed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn loss_notification_waits_for_event_channel_capacity() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/malformed-batch");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, "<llsd><map>")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        let mut received_events = EventReceiver::new(received_events);
        event_sender
            .send(EventEnvelope::new(Event::Failed("fixture occupying the event channel".into())))
            .await
            .expect("fill the single event slot");
        assert_eq!(event_sender.capacity(), 0);

        let (_shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender.clone(), shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&server.next_request()).starts_with("POST /malformed-batch HTTP/1.1"));
        let loss = timeout(TEST_TIMEOUT, async {
            loop {
                let snapshot = coverage_losses.snapshot();
                if let [crate::coverage::CoverageLoss::Source(loss)] = snapshot.losses.as_slice()
                    && loss.watermark.simulator_id == simulator_id
                    && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                {
                    break *loss;
                }
                loss_changes.changed().await.expect("coverage ledger remains open");
            }
        })
        .await
        .expect("the durable loss fact should be recorded while its notification is blocked");
        assert_eq!(event_sender.capacity(), 0, "the wake-up must wait behind the queued event");

        assert!(matches!(
            received_events.recv().await,
            Some(Event::Failed(message)) if message == "fixture occupying the event channel"
        ));
        let notification = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("the loss wake-up should be sent after a slot opens")
            .expect("event channel should remain open");
        assert!(matches!(
            notification,
            Event::EventQueueCoverageLost { simulator_id: id, source, reason }
                if id == simulator_id && source == test_source() && reason.contains("EventQueueGet")
        ));
        assert!(received_events.try_recv().is_err(), "one queue incarnation emits only one loss wake-up");
        assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(current)]
                if current.watermark == loss.watermark && current.retired
        ));
        assert!(server.finish().is_empty(), "the observed requests were already consumed");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn decode_pressure_stops_queue() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(
                    200,
                    "OK",
                    "application/llsd+xml",
                    None,
                    "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>test</string></map></array></map></llsd>",
                )),
            ]
        });
        let mut client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        client.budget.response_memory = http::ResponseMemoryBudget::new(0, 0, 0, 64 * 1024);
        let (sender, receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut receiver = EventReceiver::new(receiver);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let losses = CoverageLossLedger::new();
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), sender, shutdown_requested, losses.clone()).expect("start queue");
        server.next_request();
        let poll = server.next_request();
        assert_eq!(map_field(&decode_request_body(&poll), "ack"), Some(&Value::Undefined));
        let notification = timeout(TEST_TIMEOUT, receiver.recv()).await.expect("decode rejection notification").expect("open receiver");
        assert!(matches!(notification, Event::EventQueueCoverageLost { reason, .. } if reason.contains("decode memory")));
        assert_eq!(
            timeout(TEST_TIMEOUT, task.wait()).await.expect("queue stops after admission failure"),
            EventQueueExit::StoppedWithCoverageLoss
        );
        assert!(matches!(losses.snapshot().losses.as_slice(), [crate::coverage::CoverageLoss::Source(loss)] if loss.retired));
        assert!(receiver.try_recv().is_err(), "no partial batch is published");
        assert!(client.budget.response_memory.try_reserve(http::ResponseMemoryClass::EventQueue, 64 * 1024).is_ok());
        assert!(server.finish().is_empty(), "rejected batch cannot produce an ACK poll");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unavailable_queue_errors() {
        let server = EventQueueTestServer::start(|_| {
            vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, "<llsd><map></map></llsd>"))]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("missing capability should be reported")
            .expect("event channel should remain open");
        let Event::CapabilityError { capability, reason, .. } = event else {
            panic!("expected a capability-scoped error");
        };
        assert_eq!(capability, "EventQueueGet");
        assert!(reason.contains("EventQueueGet"));
        assert_eq!(server.finish().len(), 1);
        task.wait().await;

        shutdown.send_replace(true);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn not_found_stops_polling() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let reason = receive_poll_error(&mut received_events).await;
        assert!(reason.contains("HTTP 404"));
        assert_eq!(server.finish().len(), 2, "404 must not trigger another poll");
        assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(loss)]
                if matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. })
                    && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                    && loss.retired
        ));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn primary_event_queue_404_keeps_active_session_connected() {
        let (response_sender, response_receiver) = sync::mpsc::channel();
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Controlled(response_receiver),
            ]
        });
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let agent_id = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
        let session_id = UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID");
        let session = AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address: simulator.local_addr().expect("simulator address"),
            agent_id,
            session_id,
            circuit_code: 0x1122_3344,
            seed_capability: server.url("/seed"),
        };
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let mut events = EventReceiver::new(event_receiver);
        let (disconnect, disconnect_requested) = watch::channel(false);
        let worker = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let seed_request = server.next_request();
        let poll_request = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&poll_request).starts_with("POST /event-queue HTTP/1.1"));

        let (_, peer) = receive_udp(&simulator).await;
        simulator.send_to(&acknowledgement(20, 1), peer).await.expect("ack circuit setup");
        let (movement, _) = receive_udp(&simulator).await;
        assert!(matches!(Packet::decode(&movement).expect("decode movement").message(), Message::CompleteAgentMovement(_)));
        simulator.send_to(&acknowledgement(21, 2), peer).await.expect("ack initial movement");

        let mut handshake = test_fixture(vectors::REGION_HANDSHAKE);
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        let (reply, _) = receive_udp(&simulator).await;
        let reply = Packet::decode(&reply).expect("decode handshake reply");
        assert!(matches!(reply.message(), Message::RegionHandshakeReply(_)));
        simulator.send_to(&acknowledgement(23, reply.sequence()), peer).await.expect("ack handshake reply");

        let mut movement_complete = test_fixture(vectors::AGENT_MOVEMENT_COMPLETE);
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 24);
        simulator.send_to(&movement_complete, peer).await.expect("complete agent movement");

        timeout(TEST_TIMEOUT, async {
            loop {
                match events.recv().await.expect("session event stream remains open") {
                    Event::SessionReady { .. } => break,
                    Event::Failed(reason) => panic!("primary session failed before the EventQueue response: {reason}"),
                    _ => continue,
                }
            }
        })
        .await
        .expect("primary session should become ready before the held EventQueue response");

        response_sender
            .send(response(404, "Not Found", "text/plain", None, "not found"))
            .expect("EventQueue request is still waiting for its response");
        timeout(TEST_TIMEOUT, async {
            loop {
                match events.recv().await.expect("active session event stream remains open") {
                    Event::EventQueueCoverageLost { .. } => break,
                    Event::Failed(reason) => panic!("EventQueue 404 must not end the active session: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("EventQueue coverage loss should be reported");
        timeout(TEST_TIMEOUT, async {
            loop {
                match events.recv().await.expect("active session event stream remains open") {
                    Event::CapabilityError { capability, reason, .. } if capability == EVENT_QUEUE_NAME && reason.contains("HTTP 404") => {
                        break;
                    }
                    Event::Failed(reason) => panic!("EventQueue 404 must not end the active session: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("EventQueue capability failure should be reported");

        let chat = Message::ChatFromViewer(ChatFromViewer {
            agent_data: chat_from_viewer::AgentData { agent_id, session_id },
            chat_data: chat_from_viewer::ChatData { message: "still connected".into(), chat_type: ChatType::Normal, channel: 0 },
        });
        let send_result = commands.try_send(Command::SendMessage { message: chat, delivery: Delivery::Reliable });
        let message =
            receive_message(&simulator, |message| matches!(message, Message::ChatFromViewer(_) | Message::LogoutRequest(_))).await;
        if matches!(message.message(), Message::LogoutRequest(_)) {
            simulator.send_to(&acknowledgement(25, message.sequence()), peer).await.expect("ack unexpected active simulator logout");
            let mut logout_reply = test_fixture(vectors::LOGOUT_REPLY);
            logout_reply[0] |= 0x40;
            set_sequence(&mut logout_reply, 26);
            simulator.send_to(&logout_reply, peer).await.expect("reply to unexpected active simulator logout");
            let result = timeout(TEST_TIMEOUT, worker).await.expect("failed session should stop").expect("session task");
            assert!(result.is_err(), "the pre-fix runtime treats EventQueue loss as terminal");
            panic!("EventQueue 404 ended the active viewer session");
        }
        send_result.expect("active session must continue accepting simulator commands");
        assert!(matches!(
            message.message(),
            Message::ChatFromViewer(chat) if chat.chat_data.message == "still connected"
        ));
        simulator.send_to(&acknowledgement(25, message.sequence()), peer).await.expect("ack post-failure chat");

        disconnect.send_replace(true);
        let logout = receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
        simulator.send_to(&acknowledgement(26, logout.sequence()), peer).await.expect("ack explicit active simulator logout");
        let mut logout_reply = test_fixture(vectors::LOGOUT_REPLY);
        logout_reply[0] |= 0x40;
        set_sequence(&mut logout_reply, 27);
        simulator.send_to(&logout_reply, peer).await.expect("reply to explicit active simulator logout");
        let result = timeout(TEST_TIMEOUT, worker).await.expect("explicit logout should stop the session").expect("session task");
        assert!(result.is_ok(), "explicit logout is a normal session end");
        assert!(server.finish().is_empty(), "404 must end polling without another request");
    }

    fn test_fixture(source: &str) -> Vec<u8> {
        parse_hex_fixture(source).expect("valid LLUDP fixture")
    }

    fn set_sequence(bytes: &mut [u8], sequence: u32) {
        bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
    }

    async fn receive_poll_error(events: &mut EventReceiver) -> String {
        let loss = timeout(TEST_TIMEOUT, events.recv())
            .await
            .expect("coverage loss should be reported before the poll error")
            .expect("event channel should remain open");
        assert!(matches!(loss, Event::EventQueueCoverageLost { .. }));

        let event = timeout(TEST_TIMEOUT, events.recv())
            .await
            .expect("poll error should be reported after coverage loss")
            .expect("event channel should remain open");
        let Event::CapabilityError { capability, reason, .. } = event else {
            panic!("expected an EventQueueGet capability error");
        };
        assert_eq!(capability, EVENT_QUEUE_NAME);
        reason
    }

    #[tokio::test]
    async fn retry_limit_is_terminal() {
        let mut error_count = MAX_EVENT_QUEUE_ERRORS;
        let (events, received_events) = mpsc::channel(1);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, mut shutdown_requested) = watch::channel(false);
        let (_session_closed, session_closed) = watch::channel(false);

        let outcome = retry_event_queue(
            SimulatorId::new(1),
            &mut error_count,
            test_source(),
            &events,
            "EventQueueGet returned HTTP 500.".into(),
            &mut shutdown_requested,
            &session_closed,
        )
        .await;
        let Some(Event::CapabilityError { capability, reason, .. }) = received_events.recv().await else {
            panic!("retry exhaustion must report a capability error");
        };

        assert_eq!(outcome, RetryOutcome::LimitReached);
        assert_eq!(capability, "EventQueueGet");
        assert!(reason.contains("repeated errors"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_retry_limit_records_coverage_before_reporting_error() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            let mut responses =
                vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url)))];
            for _ in 0..=MAX_EVENT_QUEUE_ERRORS {
                responses.push(EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", Some("0"), "try again")));
            }
            responses
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(1);
        event_sender.try_send(EventEnvelope::new(Event::Failed("occupy event channel".into()))).expect("fill event channel");
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let mut loss_changes = coverage_losses.subscribe();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        for _ in 0..=MAX_EVENT_QUEUE_ERRORS {
            let poll = server.next_request();
            assert!(request_line(&poll).starts_with("POST /event-queue HTTP/1.1"));
            assert_eq!(map_field(&decode_request_body(&poll), "ack"), Some(&Value::Undefined));
        }

        let loss = timeout(TEST_TIMEOUT, async {
            loop {
                let snapshot = coverage_losses.snapshot();
                if let [crate::coverage::CoverageLoss::Source(loss)] = snapshot.losses.as_slice()
                    && loss.watermark.generation == 1
                {
                    break *loss;
                }
                loss_changes.changed().await.expect("coverage ledger remains open");
            }
        })
        .await
        .expect("retry exhaustion records coverage while the event channel is full");
        assert_eq!(loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
        assert_eq!(loss.watermark.simulator_id, simulator_id);

        assert!(matches!(received_events.recv().await, Some(Event::Failed(_))));
        let Some(Event::CapabilityError { capability, reason, .. }) =
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("retry error is reported")
        else {
            panic!("retry exhaustion must report a capability error");
        };
        assert_eq!(capability, EVENT_QUEUE_NAME);
        assert!(reason.contains("repeated errors"));
        assert!(matches!(
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("coverage loss is reported"),
            Some(Event::EventQueueCoverageLost { simulator_id: id, source, .. })
                if id == simulator_id && source == test_source()
        ));

        assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("retry limit stops polling"), EventQueueExit::RetryLimitReached);
        assert!(matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(record)]
                if record.watermark.generation == 1
                    && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                    && record.retired
        ));
        assert!(server.finish().is_empty(), "all expected requests were observed and no extra retry should be sent");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seed_lookup_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", None, "try again")),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
        task.wait().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rate_limit_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(429, "Too Many Requests", "text/plain", Some("0"), "try again")),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
        task.wait().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_retry_after_keeps_ack() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(499, "Client Closed Request", "text/plain", Some("0"), "busy")),
                EventServerResponse::Response(response(429, "Too Many Requests", "text/plain", Some("0"), "busy")),
                EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", Some("0"), "busy")),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "stopped")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        let seed = server.next_request();
        let first_poll = server.next_request();
        assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));

        let retry_499_at = Instant::now();
        let after_499_poll = server.next_request();
        assert!(retry_499_at.elapsed() < Duration::from_millis(750), "499 Retry-After: 0 should bypass the default backoff");
        assert_eq!(map_field(&decode_request_body(&after_499_poll), "ack"), Some(&Value::Undefined), "499 retry keeps the previous ACK");
        let retry_429_at = Instant::now();
        let after_429_poll = server.next_request();
        assert!(retry_429_at.elapsed() < Duration::from_millis(750), "Retry-After: 0 should bypass the default one-second backoff");
        assert_eq!(map_field(&decode_request_body(&after_429_poll), "ack"), Some(&Value::Undefined), "429 retry keeps the previous ACK");
        let retry_503_at = Instant::now();
        let after_503_poll = server.next_request();
        assert!(retry_503_at.elapsed() < Duration::from_millis(750), "503 Retry-After: 0 should bypass the default backoff");
        assert_eq!(map_field(&decode_request_body(&after_503_poll), "ack"), Some(&Value::Undefined), "503 retry keeps the previous ACK");

        let batch =
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("admitted EventQueue batch").expect("event channel stays open");
        assert!(matches!(batch, Event::CapabilityEventBatch { events, .. } if events.len() == 2));
        let next_poll = server.next_request();
        assert_eq!(
            map_field(&decode_request_body(&next_poll), "ack"),
            Some(&Value::Integer(5)),
            "ACK advances only after full batch admission"
        );

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        assert!(!coverage_losses.snapshot().losses.is_empty(), "a possibly lost poll response must remain durable");
        assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
        assert!(server.finish().is_empty(), "all expected requests were observed and no extra retry should be sent");
    }

    async fn event_queue_stops_after_ambiguous_transport_failure(failed_response: EventServerResponse) {
        let server = EventQueueTestServer::start(move |base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                failed_response,
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let simulator_id = SimulatorId::new(1);
        let coverage_losses = CoverageLossLedger::new();
        let task = client
            .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
            .expect("start event queue");

        assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
        let failure_poll = server.next_request();
        assert!(request_line(&failure_poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&failure_poll), "ack"), Some(&Value::Undefined));
        let loss = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("the ambiguous poll should report durable loss")
            .expect("event channel should remain open");
        assert!(matches!(loss, Event::EventQueueCoverageLost { reason, .. } if reason.contains("request failed")));
        assert_eq!(
            timeout(TEST_TIMEOUT, task.wait()).await.expect("ambiguous outcome stops this poll incarnation"),
            EventQueueExit::StoppedWithCoverageLoss
        );
        assert!(
            matches!(
                coverage_losses.snapshot().losses.as_slice(),
                [crate::coverage::CoverageLoss::Source(record)]
                    if record.watermark.simulator_id == simulator_id
                        && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                        && record.retired
            ),
            "the loss must remain in the durable ledger after the poll incarnation stops"
        );
        assert!(server.finish().is_empty());
        drop(shutdown);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_stops_after_connection_failure() {
        event_queue_stops_after_ambiguous_transport_failure(EventServerResponse::DropConnection).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn event_queue_stops_after_partial_response() {
        let body = TWO_EVENT_BATCH;
        let partial = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len() + 32,
            body
        )
        .into_bytes();
        event_queue_stops_after_ambiguous_transport_failure(EventServerResponse::Response(partial)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn seed_transport_retries() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::DropConnection,
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
        task.wait().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn permanent_seed_error_stops() {
        let server = EventQueueTestServer::start(|_| {
            vec![EventServerResponse::Response(response(401, "Unauthorized", "text/plain", None, "denied"))]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (_shutdown, shutdown_requested) = watch::channel(false);
        let task = client
            .start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
            .expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("permanent seed failure should be reported")
            .expect("event channel should remain open");
        assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 401")));
        assert_eq!(server.finish().len(), 1, "permanent errors must not be retried");
        task.wait().await;
    }

    const TWO_EVENT_BATCH: &str = "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>First</string></map><map><key>message</key><string>Second</string></map></array></map></llsd>";
    const MALFORMED_BATCH: &str = "<llsd><map>";

    fn content_length_only_response(content_length: usize) -> Vec<u8> {
        format!("HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n")
            .into_bytes()
    }

    struct ConcurrentCapabilityTestServer {
        base: Url,
        requests: mpsc::UnboundedReceiver<()>,
        worker: Option<thread::JoinHandle<usize>>,
    }

    impl ConcurrentCapabilityTestServer {
        fn start(request_count: usize, response_delay: Duration) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind capability concurrency server");
            listener.set_nonblocking(true).expect("set concurrency listener nonblocking");
            let address = listener.local_addr().expect("read concurrency server address");
            let base = Url::parse(&format!("http://{address}/")).expect("valid local capability URL");
            let mut capability_url = base.clone();
            capability_url.set_path("/capability");
            let body = format!("<llsd><map><key>TestCapability</key><string>{capability_url}</string></map></llsd>");
            let response = response(200, "OK", "application/llsd+xml", None, &body);
            let (request_sender, requests) = mpsc::unbounded_channel();

            let worker = thread::spawn(move || {
                let active = Arc::new(AtomicUsize::new(0));
                let maximum_active = Arc::new(AtomicUsize::new(0));
                let mut handlers = Vec::with_capacity(request_count);

                for _ in 0..request_count {
                    let (mut stream, _) = accept_with_timeout(&listener);
                    stream.set_nonblocking(false).expect("set capability test stream blocking");
                    stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set concurrency request timeout");
                    let response = response.clone();
                    let active = Arc::clone(&active);
                    let maximum_active = Arc::clone(&maximum_active);
                    let request_sender = request_sender.clone();
                    handlers.push(thread::spawn(move || {
                        let _request = read_request(&mut stream);
                        let _ = request_sender.send(());
                        let active_count = active.fetch_add(1, Ordering::Relaxed) + 1;
                        maximum_active.fetch_max(active_count, Ordering::Relaxed);
                        thread::sleep(response_delay);
                        let _ = stream.write_all(&response);
                        active.fetch_sub(1, Ordering::Relaxed);
                    }));
                }
                drop(request_sender);

                for handler in handlers {
                    handler.join().expect("capability request handler should finish");
                }
                maximum_active.load(Ordering::Relaxed)
            });

            Self { base, requests, worker: Some(worker) }
        }

        fn url(&self, path: &str) -> Url {
            let mut url = self.base.clone();
            url.set_path(path);
            url
        }

        async fn next_request(&mut self) {
            timeout(TEST_TIMEOUT, self.requests.recv())
                .await
                .expect("HTTP request should reach capability server")
                .expect("capability server request stream should remain open");
        }

        fn finish(mut self) -> usize {
            self.worker.take().expect("concurrency server worker").join().expect("concurrency server should stop")
        }
    }

    struct EventQueueBudgetTestServer {
        base: Url,
        seeds: sync::mpsc::Receiver<()>,
        polls: sync::mpsc::Receiver<TcpStream>,
        stop: sync::mpsc::Sender<()>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl EventQueueBudgetTestServer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue budget server");
            listener.set_nonblocking(true).expect("set budget listener nonblocking");
            let address = listener.local_addr().expect("read budget server address");
            let base = Url::parse(&format!("http://{address}/")).expect("valid event queue test URL");
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            let seed_response = response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url));
            let (seed_sender, seeds) = sync::mpsc::channel();
            let (poll_sender, polls) = sync::mpsc::channel();
            let (stop, stop_receiver) = sync::mpsc::channel();
            let worker = thread::spawn(move || {
                while let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) {
                    stream.set_nonblocking(false).expect("set budget stream blocking");
                    stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set budget request timeout");
                    let request = read_request(&mut stream);
                    let path = request_line(&request).split_whitespace().nth(1);
                    match path {
                        Some("/seed") => {
                            stream.write_all(&seed_response).expect("write seed capability response");
                            seed_sender.send(()).expect("test should observe every seed lookup");
                        }
                        Some("/event-queue") => poll_sender.send(stream).expect("test should receive each event poll"),
                        path => panic!("unexpected capability request path: {path:?}"),
                    }
                }
            });
            Self { base, seeds, polls, stop, worker: Some(worker) }
        }

        fn url(&self, path: &str) -> Url {
            let mut url = self.base.clone();
            url.set_path(path);
            url
        }

        fn next_poll(&self) -> TcpStream {
            self.polls.recv_timeout(TEST_TIMEOUT).expect("event queue request should reach budget server")
        }

        fn next_seed(&self) {
            self.seeds.recv_timeout(TEST_TIMEOUT).expect("seed lookup should reach budget server");
        }

        fn stop(&self) {
            let _ = self.stop.send(());
        }

        fn finish(mut self) {
            self.worker.take().expect("budget server worker").join().expect("event queue budget server should stop");
        }
    }

    pub(crate) enum EventServerResponse {
        Response(Vec<u8>),
        Hold,
        Controlled(sync::mpsc::Receiver<Vec<u8>>),
        DropConnection,
    }

    pub(crate) struct EventQueueTestServer {
        base: Url,
        requests: sync::mpsc::Receiver<Vec<u8>>,
        stop: sync::mpsc::Sender<()>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl EventQueueTestServer {
        pub(crate) fn start(responses: impl FnOnce(&Url) -> Vec<EventServerResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue test server");
            listener.set_nonblocking(true).expect("set test listener nonblocking");
            let address = listener.local_addr().expect("read test server address");
            let base = Url::parse(&format!("http://{address}/")).expect("valid local test URL");
            let responses = responses(&base);
            let (request_sender, requests) = sync::mpsc::channel();
            let (stop, stop_receiver) = sync::mpsc::channel();
            let worker = thread::spawn(move || {
                for response in responses {
                    let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) else {
                        return;
                    };
                    stream.set_nonblocking(false).expect("set test stream blocking");
                    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set test request timeout");
                    let request = read_request(&mut stream);
                    request_sender.send(request).expect("test should read request");
                    match response {
                        EventServerResponse::Response(response) => {
                            stream.write_all(&response).expect("write test HTTP response");
                        }
                        EventServerResponse::Controlled(response) => {
                            let response = response.recv_timeout(TEST_TIMEOUT).expect("controlled HTTP response");
                            stream.write_all(&response).expect("write controlled HTTP response");
                        }
                        EventServerResponse::Hold => {
                            let mut byte = [0];
                            let _ = stream.read(&mut byte);
                        }
                        EventServerResponse::DropConnection => drop(stream),
                    }
                }
            });
            Self { base, requests, stop, worker: Some(worker) }
        }

        pub(crate) fn url(&self, path: &str) -> Url {
            let mut url = self.base.clone();
            url.set_path(path);
            url
        }

        fn next_request(&self) -> Vec<u8> {
            self.requests.recv_timeout(TEST_TIMEOUT).expect("server should receive HTTP request")
        }

        pub(crate) fn stop(&self) {
            let _ = self.stop.send(());
        }

        pub(crate) fn finish(mut self) -> Vec<Vec<u8>> {
            if let Some(worker) = self.worker.take() {
                worker.join().expect("event queue server should stop");
            }
            self.requests.try_iter().collect()
        }
    }

    fn accept_until_stopped(listener: &TcpListener, stop: &sync::mpsc::Receiver<()>) -> Option<(TcpStream, SocketAddr)> {
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            if stop.try_recv().is_ok() {
                return None;
            }
            match listener.accept() {
                Ok(connection) => return Some(connection),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "event queue test server did not receive HTTP request");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept event queue test connection: {error}"),
            }
        }
    }

    pub(super) fn seed_response(event_url: &Url) -> String {
        format!("<llsd><map><key>EventQueueGet</key><string>{event_url}</string></map></llsd>")
    }

    fn test_source() -> SocketAddr {
        "127.0.0.1:13000".parse().expect("fixture simulator endpoint")
    }

    fn simulator_events(ip: [u8; 4], port: u16, agent_id: UUID, seed: &Url) -> String {
        let simulator = Value::Map(Map::from([
            ("Handle".to_owned(), Value::Binary(0x1000_0000_0000_1000_u64.to_be_bytes().to_vec().into())),
            ("IP".to_owned(), Value::Binary(ip.to_vec().into())),
            ("Port".to_owned(), Value::Integer(i32::from(port))),
        ]));
        let enable = Value::Map(Map::from([
            ("message".to_owned(), Value::String("EnableSimulator".to_owned())),
            ("body".to_owned(), Value::Map(Map::from([("SimulatorInfo".to_owned(), Value::Array(vec![simulator]))]))),
        ]));
        let establish = Value::Map(Map::from([
            ("message".to_owned(), Value::String("EstablishAgentCommunication".to_owned())),
            (
                "body".to_owned(),
                Value::Map(Map::from([
                    ("agent-id".to_owned(), Value::UUID(agent_id)),
                    (
                        "sim-ip-and-port".to_owned(),
                        Value::String(format!("{}:{}", ip.iter().map(u8::to_string).collect::<Vec<_>>().join("."), port)),
                    ),
                    ("seed-capability".to_owned(), Value::String(seed.to_string())),
                ])),
            ),
        ]));
        let batch =
            Value::Map(Map::from([("id".to_owned(), Value::Integer(1)), ("events".to_owned(), Value::Array(vec![enable, establish]))]));
        xml::to_string(&batch).expect("encode local LLSD event queue response")
    }

    fn crossed_region_events(agent_id: UUID, session_id: UUID, region_handle: u64, endpoint: SocketAddrV4, seed: &Url) -> String {
        let crossed_region = Value::Map(Map::from([
            ("message".to_owned(), Value::String("CrossedRegion".to_owned())),
            (
                "body".to_owned(),
                Value::Map(Map::from([
                    (
                        "AgentData".to_owned(),
                        Value::Array(vec![Value::Map(Map::from([
                            ("AgentID".to_owned(), Value::UUID(agent_id)),
                            ("SessionID".to_owned(), Value::UUID(session_id)),
                        ]))]),
                    ),
                    (
                        "RegionData".to_owned(),
                        Value::Array(vec![Value::Map(Map::from([
                            ("RegionHandle".to_owned(), Value::Binary(region_handle.to_be_bytes().to_vec().into())),
                            ("SimIP".to_owned(), Value::Binary(endpoint.ip().octets().to_vec().into())),
                            ("SimPort".to_owned(), Value::Integer(i32::from(endpoint.port()))),
                            ("SeedCapability".to_owned(), Value::String(seed.to_string())),
                        ]))]),
                    ),
                ])),
            ),
        ]));
        let batch =
            Value::Map(Map::from([("id".to_owned(), Value::Integer(1)), ("events".to_owned(), Value::Array(vec![crossed_region]))]));
        xml::to_string(&batch).expect("encode local CrossedRegion event")
    }

    async fn receive_udp(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        let mut buffer = [0; 65_535];
        let (size, endpoint) = timeout(TEST_TIMEOUT, socket.recv_from(&mut buffer))
            .await
            .expect("receive local simulator packet")
            .expect("read local simulator packet");
        (buffer[..size].to_vec(), endpoint)
    }

    async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
        timeout(TEST_TIMEOUT, async {
            loop {
                let (bytes, _) = receive_udp(socket).await;
                let packet = Packet::decode(&bytes).expect("decode local simulator packet");
                if expected(packet.message()) {
                    return packet;
                }
            }
        })
        .await
        .expect("receive expected local simulator message")
    }

    fn acknowledgement(sequence: u32, received_sequence: u32) -> Vec<u8> {
        Packet::new(sequence, Message::PacketAck(PacketAck { packets: vec![Packets { id: received_sequence }] }))
            .encode()
            .expect("encode simulator circuit acknowledgement")
    }

    fn hold_response() -> EventServerResponse {
        EventServerResponse::Hold
    }

    fn accept_with_timeout(listener: &TcpListener) -> (TcpStream, SocketAddr) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            match listener.accept() {
                Ok(connection) => return connection,
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "event queue server did not receive a request");
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("accept test connection: {error}"),
            }
        }
    }

    fn write_response(mut stream: TcpStream, response: Vec<u8>) {
        stream.write_all(&response).expect("write controlled event queue response");
    }

    fn request_line(request: &[u8]) -> &str {
        str::from_utf8(request).expect("request should be UTF-8").split("\r\n").next().expect("request line")
    }

    fn request_body(request: &[u8]) -> &str {
        str::from_utf8(request).expect("request should be UTF-8").split_once("\r\n\r\n").expect("request should include body").1
    }

    fn decode_request_body(request: &[u8]) -> Value {
        xml::from_str(request_body(request)).expect("request body should contain LLSD XML")
    }

    fn map_field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
        value.as_map()?.get(key)
    }
}
