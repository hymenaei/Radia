//! Bounded requests to simulator capabilities.
//!
//! [`Client`] resolves capability names and sends LLSD or raw HTTP requests. Use [`CapabilityUrl`] to keep authenticated URLs out of debug output.

use std::{collections::HashMap, fmt, net::SocketAddr, sync::Arc, time::Duration};

use llsd::{Limits, Value};
use reqwest::Url;
use serde::{Serialize, de::DeserializeOwned};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};

use crate::{
    capture::CaptureRecorder,
    error::Error as NetworkError,
    event::{EventEnvelope, SimulatorId},
    http::{Client as HttpClient, Response, validate_http_url},
    status::NetworkStatusHandle,
};

mod event_queue;
pub(crate) use event_queue::{EVENT_QUEUE_NAME, EventQueueExit};
#[cfg(test)]
pub(crate) use event_queue::{
    MAX_EVENT_QUEUE_ERRORS, MAX_EVENT_QUEUE_RESPONSE_BYTES, MAX_QUEUED_CAPABILITY_EVENT_BYTES, RetryOutcome, decode_event_batch,
    event_queue_retry_delay, expected_empty_poll, post_event_queue, retry_event_queue, seed_capability_retry_delay, seed_retry_delay,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const EVENT_QUEUE_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const COMMAND_CAPACITY: usize = 64;
const CLOSED_ERROR: &str = "The capability connection has closed.";
pub(crate) const MAX_SEED_CAPABILITY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_CAPABILITY_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
const MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES: usize = 8 * 1024 * 1024;
const MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES: usize = MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES * MAX_CONCURRENT_EVENT_QUEUES;
const MAX_LLSD_NESTING_DEPTH: usize = 64;
pub(crate) const MAX_LLSD_XML_ELEMENTS: usize = 262_144;
pub(crate) const MAX_CONCURRENT_CAPABILITY_REQUESTS: usize = 4;
pub(crate) const MAX_CONCURRENT_EVENT_QUEUES: usize = 9;
/// Maximum encoded LLSD XML body sent in a capability request.
pub(crate) const MAX_CAPABILITY_REQUEST_BODY_BYTES: usize = 1024 * 1024;
const MAX_CAPABILITY_NAMES: usize = 256;
const MAX_CAPABILITY_NAME_BYTES: usize = 128;
const MAX_CAPABILITY_NAMES_BYTES: usize = 16 * 1024;

#[derive(Clone)]
/// An opaque handle for an authenticated simulator capability URL.
pub struct CapabilityUrl {
    url: Url,
    capture_endpoint: Option<String>,
}

impl CapabilityUrl {
    /// Parses and validates an HTTP or HTTPS capability URL.
    ///
    /// The URL must have a host and must not contain user information or a fragment.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL is malformed or violates these constraints.
    pub fn parse(value: &str) -> Result<Self, NetworkError> {
        let url = Url::parse(value).map_err(|_| NetworkError::Capability("Capability URL is invalid.".into()))?;
        validate_http_url(&url).map_err(NetworkError::Capability)?;
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
pub(crate) struct CapabilityBudget {
    ordinary_requests: Arc<Semaphore>,
    event_queues: Arc<Semaphore>,
    queued_event_bytes: Arc<Semaphore>,
}

impl CapabilityBudget {
    pub(crate) fn new() -> Self {
        Self {
            ordinary_requests: Arc::new(Semaphore::new(MAX_CONCURRENT_CAPABILITY_REQUESTS)),
            event_queues: Arc::new(Semaphore::new(MAX_CONCURRENT_EVENT_QUEUES)),
            queued_event_bytes: Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES)),
        }
    }
}

#[derive(Clone)]
/// Makes bounded requests to a simulator's discovered capabilities.
pub struct Client {
    seed: Url,
    commands: mpsc::Sender<Command>,
    session_closed: watch::Sender<bool>,
    budget: CapabilityBudget,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
}

struct AdmittedResponse {
    body: Vec<u8>,
    _permit: OwnedSemaphorePermit,
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
    ///
    /// # Errors
    ///
    /// Returns an error if the seed URL is invalid or no Tokio runtime is active.
    pub fn new(seed: Url) -> Result<Self, NetworkError> {
        Self::new_with_capture(seed, None).map_err(NetworkError::Capability)
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
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| "A Tokio runtime is required for capability requests.".to_string())?;
        let http = HttpClient::new(Some(REQUEST_TIMEOUT), false, None)
            .map_err(|error| format!("Could not prepare the capability client: {error}"))?
            .with_capture(capture.clone())
            .with_status(status.clone());
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        runtime.spawn(run(http, seed.clone(), receiver, session_closed_receiver, capture.clone(), status.clone()));
        Ok(Self { seed, commands, session_closed, budget, capture, status })
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
    pub async fn request_capabilities(&self, names: &[&str]) -> Result<HashMap<String, CapabilityUrl>, NetworkError> {
        validate_capability_names(names)?;
        let permit = self.acquire_request_permit().await?;
        let names = names.iter().map(|name| (*name).to_owned()).collect();
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::RequestCapabilities { names, reply, _permit: permit })
            .await
            .map_err(|_| NetworkError::CapabilitySessionClosed)?;
        response.await.map_err(|_| self.request_ended_error())?.map_err(capability_error).map(|capabilities| {
            capabilities
                .into_iter()
                .map(|(name, url)| {
                    let capture_endpoint = logical_endpoint_label(&name);
                    (name, CapabilityUrl { url, capture_endpoint })
                })
                .collect()
        })
    }

    /// Sends an LLSD XML request and decodes the response directly into a Serde type.
    ///
    /// The encoded request body is limited to 1 MiB and the response body to 32 MiB. Encoding and response decoding run on the calling task.
    /// Run large codecs on a background executor so they do not delay UI work.
    ///
    /// # Errors
    ///
    /// Returns an error if encoding exceeds its limit, the capability request fails, or response decoding fails.
    pub async fn post<Request, Response>(&self, url: &CapabilityUrl, payload: &Request) -> Result<Response, NetworkError>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        let permit = self.acquire_request_permit().await?;
        let body = llsd::xml::to_vec_with_max_bytes(payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
            .map_err(|error| NetworkError::Capability(format!("Could not encode an LLSD request: {error}")))?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Post { url: url.clone(), body, reply, _permit: permit })
            .await
            .map_err(|_| NetworkError::CapabilitySessionClosed)?;
        let body = response.await.map_err(|_| self.request_ended_error())?.map_err(capability_error)?;
        decode_capability_response(&body.body)
    }

    /// Fetches an LLSD XML response and decodes it directly into a Serde type.
    ///
    /// The response body is limited to 32 MiB. Response decoding runs on the calling task.
    /// Run large codecs on a background executor so they do not delay UI work.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response cannot be decoded.
    pub async fn get<Response: DeserializeOwned>(&self, url: &CapabilityUrl) -> Result<Response, NetworkError> {
        let body = self.get_bytes_admitted(url).await?;
        decode_capability_response(&body.body)
    }

    /// Fetches the raw body for binary or otherwise non-LLSD capability resources.
    ///
    /// The response body is limited to 32 MiB.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails or the response body exceeds its limit.
    pub async fn get_bytes(&self, url: &CapabilityUrl) -> Result<Vec<u8>, NetworkError> {
        Ok(self.get_bytes_admitted(url).await?.body)
    }

    async fn get_bytes_admitted(&self, url: &CapabilityUrl) -> Result<AdmittedResponse, NetworkError> {
        let permit = self.acquire_request_permit().await?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::GetBytes { url: url.clone(), reply, _permit: permit })
            .await
            .map_err(|_| NetworkError::CapabilitySessionClosed)?;
        response.await.map_err(|_| self.request_ended_error())?.map_err(capability_error)
    }

    async fn acquire_request_permit(&self) -> Result<OwnedSemaphorePermit, NetworkError> {
        let budget = Arc::clone(&self.budget.ordinary_requests);
        tokio::select! {
            biased;
            _ = wait_for_session_close(self.session_closed.subscribe()) => Err(NetworkError::CapabilitySessionClosed),
            permit = budget.acquire_owned() => permit.map_err(|_| self.request_ended_error()),
        }
    }

    fn request_ended_error(&self) -> NetworkError {
        if *self.session_closed.borrow() { NetworkError::CapabilitySessionClosed } else { NetworkError::CapabilityRequestCanceled }
    }

    pub(crate) fn close_session(&self) {
        self.session_closed.send_replace(true);
    }

    pub(crate) fn start_event_queue(
        &self,
        simulator_id: SimulatorId,
        source: SocketAddr,
        events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
    ) -> Result<EventQueueTask, String> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| "The simulator event queue requires an active Tokio runtime.".to_string())?;
        let http = HttpClient::new(Some(EVENT_QUEUE_REQUEST_TIMEOUT), false, None)
            .map_err(|error| format!("Could not prepare the simulator event queue: {error}"))?
            .with_capture(self.capture.clone())
            .with_status(self.status.clone());
        let seed = self.seed.clone();
        let status = self.status.clone();
        let ordinary_requests = Arc::clone(&self.budget.ordinary_requests);
        let event_queues = Arc::clone(&self.budget.event_queues);
        let queued_event_bytes = Arc::clone(&self.budget.queued_event_bytes);
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
                status,
                session_closed,
                ordinary_requests,
                event_queues,
                queued_event_bytes,
            )
            .await;
            let _ = completed.send(exit);
        });
        Ok(EventQueueTask { task: Some(task), completion })
    }
}

fn capability_error(message: String) -> NetworkError {
    if message == CLOSED_ERROR { NetworkError::CapabilitySessionClosed } else { NetworkError::Capability(message) }
}

fn validate_capability_names(names: &[&str]) -> Result<(), NetworkError> {
    if names.len() > MAX_CAPABILITY_NAMES {
        return Err(NetworkError::Capability(format!("Capability discovery supports at most {MAX_CAPABILITY_NAMES} names.")));
    }

    let mut total_bytes = 0_usize;
    for name in names {
        if name.len() > MAX_CAPABILITY_NAME_BYTES {
            return Err(NetworkError::Capability(format!(
                "A capability discovery name exceeds the {MAX_CAPABILITY_NAME_BYTES}-byte limit."
            )));
        }
        total_bytes = total_bytes.checked_add(name.len()).filter(|size| *size <= MAX_CAPABILITY_NAMES_BYTES).ok_or_else(|| {
            NetworkError::Capability(format!("Capability discovery names exceed the {MAX_CAPABILITY_NAMES_BYTES}-byte total limit."))
        })?;
    }
    Ok(())
}

pub(crate) struct EventQueueTask {
    task: Option<JoinHandle<()>>,
    completion: oneshot::Receiver<EventQueueExit>,
}

impl EventQueueTask {
    pub(crate) async fn completion(&mut self) -> EventQueueExit {
        (&mut self.completion).await.unwrap_or(EventQueueExit::TaskFailed)
    }

    pub(crate) async fn wait(mut self) -> EventQueueExit {
        let exit = self.completion().await;
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        exit
    }
}

impl Drop for EventQueueTask {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

enum Command {
    RequestCapabilities { names: Vec<String>, reply: oneshot::Sender<Result<HashMap<String, Url>, String>>, _permit: OwnedSemaphorePermit },
    Post { url: CapabilityUrl, body: Vec<u8>, reply: oneshot::Sender<Result<AdmittedResponse, String>>, _permit: OwnedSemaphorePermit },
    GetBytes { url: CapabilityUrl, reply: oneshot::Sender<Result<AdmittedResponse, String>>, _permit: OwnedSemaphorePermit },
}

#[tracing::instrument(name = "capabilities.worker", level = "debug", skip_all)]
async fn run(
    client: HttpClient,
    seed: Url,
    mut commands: mpsc::Receiver<Command>,
    session_closed: watch::Receiver<bool>,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
) {
    let mut requests = JoinSet::new();
    loop {
        let command = tokio::select! {
            biased;
            _ = wait_for_session_close(session_closed.clone()) => break,
            command = commands.recv() => command,
        };
        let Some(command) = command else {
            break;
        };

        if requests.len() >= MAX_CONCURRENT_CAPABILITY_REQUESTS {
            tokio::select! {
                biased;
                _ = wait_for_session_close(session_closed.clone()) => break,
                _ = requests.join_next() => {}
            }
        }

        let client = client.clone();
        let seed = seed.clone();
        let session_closed = session_closed.clone();
        let capture = capture.clone();
        let status = status.clone();
        requests.spawn(async move {
            match command {
                Command::RequestCapabilities { names, mut reply, _permit } => {
                    let result = tokio::select! {
                        biased;
                        _ = wait_for_session_close(session_closed) => Err(CLOSED_ERROR.to_string()),
                        _ = reply.closed() => return,
                        result = request_capabilities(&client, &seed, &names) => result,
                    };
                    match &result {
                        Ok(granted) => status.note_capabilities(granted.keys().map(String::as_str)),
                        Err(error) if error != CLOSED_ERROR => status.record_error(error),
                        Err(_) => {}
                    }
                    if result.is_ok()
                        && let Some(capture) = &capture
                    {
                        capture.lifecycle("capabilities_discovered");
                    }
                    let _ = reply.send(result);
                }
                Command::Post { url, body, mut reply, _permit } => {
                    let result = tokio::select! {
                        biased;
                        _ = wait_for_session_close(session_closed) => Err(CLOSED_ERROR.to_string()),
                        _ = reply.closed() => return,
                        result = post(&client, &url.url, url.capture_endpoint.as_deref(), body, "Capability request", MAX_CAPABILITY_RESPONSE_BYTES) => result,
                    };
                    if let Err(error) = &result
                        && error != CLOSED_ERROR
                    {
                        status.record_error(error);
                    }
                    let result = result.map(|body| AdmittedResponse { body, _permit });
                    let _ = reply.send(result);
                }
                Command::GetBytes { url, mut reply, _permit } => {
                    let result = tokio::select! {
                        biased;
                        _ = wait_for_session_close(session_closed) => Err(CLOSED_ERROR.to_string()),
                        _ = reply.closed() => return,
                        result = get_bytes(&client, &url.url, url.capture_endpoint.as_deref()) => result,
                    };
                    if let Err(error) = &result
                        && error != CLOSED_ERROR
                    {
                        status.record_error(error);
                    }
                    let result = result.map(|body| AdmittedResponse { body, _permit });
                    let _ = reply.send(result);
                }
            }
        });
    }

    drop(commands);
    while requests.join_next().await.is_some() {}
}

async fn wait_for_session_close(mut session_closed: watch::Receiver<bool>) {
    while !*session_closed.borrow() {
        if session_closed.changed().await.is_err() {
            break;
        }
    }
}

async fn request_capabilities(client: &HttpClient, seed: &Url, names: &[String]) -> Result<HashMap<String, Url>, String> {
    let response =
        request_capabilities_response(client, seed, names, MAX_SEED_CAPABILITY_RESPONSE_BYTES).await.map_err(LookupError::message)?;
    decode_capabilities(&response.body, names)
}

async fn request_capabilities_response(
    client: &HttpClient,
    seed: &Url,
    names: &[String],
    max_response_bytes: usize,
) -> Result<Response, LookupError> {
    let payload = Value::Array(names.iter().cloned().map(Value::String).collect());
    let body = llsd::xml::to_vec_with_max_bytes(&payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
        .map_err(|error| LookupError::Permanent(format!("Could not encode an LLSD request: {error}")))?;
    let response = client
        .post_operation_bounded(
            "seed_capability",
            None,
            seed,
            "application/llsd+xml",
            Some("application/llsd+xml"),
            body,
            max_response_bytes,
        )
        .await
        .map_err(|error| LookupError::Transient { message: format!("Could not request capabilities: {error}"), retry_after: None })?;
    if response.status.is_server_error() || response.status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err(LookupError::Transient {
            message: format!("Could not request capabilities: HTTP {}", response.status),
            retry_after: response.retry_after(),
        });
    }
    if !response.status.is_success() {
        return Err(LookupError::Permanent(format!("Could not request capabilities: HTTP {}", response.status)));
    }
    Ok(response)
}

fn decode_capabilities(body: &[u8], names: &[String]) -> Result<HashMap<String, Url>, String> {
    let body = std::str::from_utf8(body).map_err(|error| format!("Could not request capabilities: response was not UTF-8: {error}"))?;
    let response = decode_llsd_xml(body, "Could not request capabilities")?;
    let fields = response.as_map().ok_or_else(|| "Second Life returned an invalid capability response.".to_string())?;
    let mut granted = HashMap::with_capacity(names.len());
    for name in names {
        let Some(value) = fields.get(name) else {
            continue;
        };
        let value = string_value(value).ok_or_else(|| format!("Second Life returned an invalid {name} capability."))?;
        let url = Url::parse(value).map_err(|_| format!("Second Life returned an invalid {name} capability."))?;
        validate_http_url(&url).map_err(|_| format!("Second Life returned an invalid {name} capability."))?;
        granted.insert(name.clone(), url);
    }
    Ok(granted)
}

enum LookupError {
    Transient { message: String, retry_after: Option<Duration> },
    Permanent(String),
}

impl LookupError {
    fn message(self) -> String {
        match self {
            Self::Transient { message, .. } | Self::Permanent(message) => message,
        }
    }
}

async fn post(
    client: &HttpClient,
    url: &Url,
    capture_endpoint: Option<&str>,
    body: Vec<u8>,
    context: &str,
    max_response_bytes: usize,
) -> Result<Vec<u8>, String> {
    let response = client
        .post_operation_bounded(
            "capability_post",
            capture_endpoint,
            url,
            "application/llsd+xml",
            Some("application/llsd+xml"),
            body,
            max_response_bytes,
        )
        .await
        .map_err(|error| format!("{context}: {error}"))?;
    if !response.status.is_success() {
        return Err(format!("{context}: HTTP {}", response.status));
    }
    Ok(response.body)
}

async fn get_bytes(client: &HttpClient, url: &Url, capture_endpoint: Option<&str>) -> Result<Vec<u8>, String> {
    let response = client
        .get_bounded("capability_get", capture_endpoint, url, MAX_CAPABILITY_RESPONSE_BYTES)
        .await
        .map_err(|error| format!("Could not fetch capability resource: {error}"))?;
    if !response.status.is_success() {
        return Err(format!("Could not fetch capability resource: HTTP {}", response.status));
    }
    Ok(response.body)
}

pub(crate) fn logical_endpoint_label(name: &str) -> Option<String> {
    const MAX_LABEL_BYTES: usize = 128;

    if name.is_empty()
        || name.len() > MAX_LABEL_BYTES
        || !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return None;
    }
    Some(name.to_owned())
}

fn decode_llsd_xml(body: &str, context: &str) -> Result<Value, String> {
    llsd::xml::from_slice_with_limits(body.as_bytes(), capability_llsd_limits()).map_err(|error| {
        let reason = llsd_error(error);
        format!("{context}: {reason}")
    })
}

fn decode_capability_response<T: DeserializeOwned>(body: &[u8]) -> Result<T, NetworkError> {
    llsd::xml::from_slice_with_limits(body, capability_llsd_limits())
        .map_err(|error| NetworkError::Capability(format!("Could not decode an LLSD response: {}", llsd_error(error))))
}

fn capability_llsd_limits() -> Limits {
    Limits {
        max_input_bytes: MAX_CAPABILITY_RESPONSE_BYTES,
        max_depth: MAX_LLSD_NESTING_DEPTH,
        max_values: MAX_LLSD_XML_ELEMENTS,
        max_container_items: MAX_LLSD_XML_ELEMENTS,
        max_string_bytes: MAX_CAPABILITY_RESPONSE_BYTES,
        max_map_key_bytes: MAX_CAPABILITY_RESPONSE_BYTES,
        max_binary_bytes: MAX_CAPABILITY_RESPONSE_BYTES,
    }
}

fn llsd_error(error: llsd::Error) -> String {
    match error {
        llsd::Error::LimitExceeded("nesting depth") => format!("LLSD nesting exceeded the {MAX_LLSD_NESTING_DEPTH}-level limit."),
        llsd::Error::LimitExceeded("value count" | "container items") => {
            format!("LLSD exceeded the {MAX_LLSD_XML_ELEMENTS}-element limit.")
        }
        error => error.to_string(),
    }
}
fn string_value(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        Value::URI(value) => Some(value.as_str()),
        _ => None,
    }
}

#[cfg(test)]
mod request_admission_tests {
    use super::*;
    use std::future::poll_fn;
    use std::task::Poll;
    use tokio::time;

    #[tokio::test]
    async fn canceled_post_keeps_its_request_slot() {
        let budget = CapabilityBudget::new();
        let clients = (0..4)
            .map(|_| {
                Client::new_with_status_and_budget(
                    Url::parse("http://127.0.0.1:9/seed").expect("seed URL"),
                    None,
                    NetworkStatusHandle::default(),
                    budget.clone(),
                )
                .expect("active Tokio runtime")
            })
            .collect::<Vec<_>>();
        let url = CapabilityUrl::parse("http://127.0.0.1:9/post").expect("capability URL");
        let payload = Value::String("request".into());

        for client in &clients {
            let mut request = Box::pin(client.post::<_, Value>(&url, &payload));
            assert!(poll_fn(|context| Poll::Ready(request.as_mut().poll(context))).await.is_pending());
            drop(request);
        }

        assert_eq!(budget.ordinary_requests.available_permits(), 0);
        assert!(clients.iter().all(|client| client.commands.capacity() == COMMAND_CAPACITY - 1));

        let mut extra = Box::pin(clients[0].post::<_, Value>(&url, &payload));
        assert!(poll_fn(|context| Poll::Ready(extra.as_mut().poll(context))).await.is_pending());
        drop(extra);

        time::timeout(Duration::from_secs(1), async {
            while budget.ordinary_requests.available_permits() != MAX_CONCURRENT_CAPABILITY_REQUESTS {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("workers should drop canceled commands and return their permits");
    }
}
