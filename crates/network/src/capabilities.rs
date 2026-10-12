//! Bounded requests to simulator capabilities.
//!
//! [`Client`] resolves capability names and sends LLSD or raw HTTP requests. Use [`CapabilityUrl`] to keep authenticated URLs out of debug output.

use std::{collections::HashMap, fmt, net::SocketAddr, str::from_utf8, sync::Arc, time::Duration};

use llsd::{Limits, Value, xml};
use reqwest::{StatusCode, Url};
use serde::{Serialize, de::DeserializeOwned};
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    task::{JoinHandle, JoinSet},
};

use crate::{
    capture::CaptureRecorder,
    error::Error,
    event::{EventEnvelope, SimulatorId},
    http::{self, Response, validate_http_url},
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
        let http = http::Client::new(Some(REQUEST_TIMEOUT), false, None)
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
    pub async fn request_capabilities(&self, names: &[&str]) -> Result<HashMap<String, CapabilityUrl>, Error> {
        validate_capability_names(names)?;
        let permit = self.acquire_request_permit().await?;
        let names = names.iter().map(|name| (*name).to_owned()).collect();
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::RequestCapabilities { names, reply, _permit: permit })
            .await
            .map_err(|_| Error::CapabilitySessionClosed)?;
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
    pub async fn post<Request, Response>(&self, url: &CapabilityUrl, payload: &Request) -> Result<Response, Error>
    where
        Request: Serialize + ?Sized,
        Response: DeserializeOwned,
    {
        let permit = self.acquire_request_permit().await?;
        let body = xml::to_vec_with_max_bytes(payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
            .map_err(|error| Error::Capability(format!("Could not encode an LLSD request: {error}")))?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::Post { url: url.clone(), body, reply, _permit: permit })
            .await
            .map_err(|_| Error::CapabilitySessionClosed)?;
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
    pub async fn get<Response: DeserializeOwned>(&self, url: &CapabilityUrl) -> Result<Response, Error> {
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
    pub async fn get_bytes(&self, url: &CapabilityUrl) -> Result<Vec<u8>, Error> {
        Ok(self.get_bytes_admitted(url).await?.body)
    }

    async fn get_bytes_admitted(&self, url: &CapabilityUrl) -> Result<AdmittedResponse, Error> {
        let permit = self.acquire_request_permit().await?;
        let (reply, response) = oneshot::channel();
        self.commands
            .send(Command::GetBytes { url: url.clone(), reply, _permit: permit })
            .await
            .map_err(|_| Error::CapabilitySessionClosed)?;
        response.await.map_err(|_| self.request_ended_error())?.map_err(capability_error)
    }

    async fn acquire_request_permit(&self) -> Result<OwnedSemaphorePermit, Error> {
        let budget = Arc::clone(&self.budget.ordinary_requests);
        tokio::select! {
            biased;
            _ = wait_for_session_close(self.session_closed.subscribe()) => Err(Error::CapabilitySessionClosed),
            permit = budget.acquire_owned() => permit.map_err(|_| self.request_ended_error()),
        }
    }

    fn request_ended_error(&self) -> Error {
        if *self.session_closed.borrow() { Error::CapabilitySessionClosed } else { Error::CapabilityRequestCanceled }
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
        let runtime = Handle::try_current().map_err(|_| "The simulator event queue requires an active Tokio runtime.".to_string())?;
        let http = http::Client::new(Some(EVENT_QUEUE_REQUEST_TIMEOUT), false, None)
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

fn capability_error(message: String) -> Error {
    if message == CLOSED_ERROR { Error::CapabilitySessionClosed } else { Error::Capability(message) }
}

fn validate_capability_names(names: &[&str]) -> Result<(), Error> {
    if names.len() > MAX_CAPABILITY_NAMES {
        return Err(Error::Capability(format!("Capability discovery supports at most {MAX_CAPABILITY_NAMES} names.")));
    }

    let mut total_bytes = 0_usize;
    for name in names {
        if name.len() > MAX_CAPABILITY_NAME_BYTES {
            return Err(Error::Capability(format!("A capability discovery name exceeds the {MAX_CAPABILITY_NAME_BYTES}-byte limit.")));
        }
        total_bytes = total_bytes.checked_add(name.len()).filter(|size| *size <= MAX_CAPABILITY_NAMES_BYTES).ok_or_else(|| {
            Error::Capability(format!("Capability discovery names exceed the {MAX_CAPABILITY_NAMES_BYTES}-byte total limit."))
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
    client: http::Client,
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

async fn request_capabilities(client: &http::Client, seed: &Url, names: &[String]) -> Result<HashMap<String, Url>, String> {
    let response =
        request_capabilities_response(client, seed, names, MAX_SEED_CAPABILITY_RESPONSE_BYTES).await.map_err(LookupError::message)?;
    decode_capabilities(&response.body, names)
}

async fn request_capabilities_response(
    client: &http::Client,
    seed: &Url,
    names: &[String],
    max_response_bytes: usize,
) -> Result<Response, LookupError> {
    let payload = Value::Array(names.iter().cloned().map(Value::String).collect());
    let body = xml::to_vec_with_max_bytes(&payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
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
    if response.status.is_server_error() || response.status == StatusCode::TOO_MANY_REQUESTS {
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
    let body = from_utf8(body).map_err(|error| format!("Could not request capabilities: response was not UTF-8: {error}"))?;
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
    client: &http::Client,
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

async fn get_bytes(client: &http::Client, url: &Url, capture_endpoint: Option<&str>) -> Result<Vec<u8>, String> {
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
    xml::from_slice_with_limits(body.as_bytes(), capability_llsd_limits()).map_err(|error| {
        let reason = llsd_error(error);
        format!("{context}: {reason}")
    })
}

fn decode_capability_response<T: DeserializeOwned>(body: &[u8]) -> Result<T, Error> {
    xml::from_slice_with_limits(body, capability_llsd_limits())
        .map_err(|error| Error::Capability(format!("Could not decode an LLSD response: {}", llsd_error(error))))
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
    use tokio::task::yield_now;
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
                yield_now().await;
            }
        })
        .await
        .expect("workers should drop canceled commands and return their permits");
    }
}

#[cfg(test)]
mod tests {
    use std::io::ErrorKind;
    use std::{
        io::{Read, Write},
        net::{SocketAddr, SocketAddrV4, TcpListener, TcpStream},
        str,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use crate::http::test_support::{LocalServer, response};
    use crate::{
        AgentSession,
        Error::{Capability, CapabilitySessionClosed},
        Event, Grid, NetworkStatusHandle,
        capabilities::{CapabilityBudget, CapabilityUrl, Client},
        capabilities::{
            EventQueueExit, MAX_CAPABILITY_REQUEST_BODY_BYTES, MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS,
            MAX_CONCURRENT_EVENT_QUEUES, MAX_EVENT_QUEUE_ERRORS, MAX_EVENT_QUEUE_RESPONSE_BYTES, MAX_LLSD_XML_ELEMENTS,
            MAX_QUEUED_CAPABILITY_EVENT_BYTES, MAX_SEED_CAPABILITY_RESPONSE_BYTES, RetryOutcome, decode_event_batch,
            event_queue_retry_delay, expected_empty_poll, post_event_queue, retry_event_queue, seed_capability_retry_delay,
            seed_retry_delay,
        },
        circuit::Delivery,
        client::{COMMAND_CAPACITY, Command},
        event::{CLIENT_EVENT_CAPACITY, EventEnvelope, EventReceiver, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId},
        http,
        http::Error,
        simulator::run_session,
    };
    use base::UUID;
    use llsd::{Map, Value, xml};
    use protocol::{
        lludp::{Message, Packet},
        messages::{
            AgentMovementComplete, CompletePingCheck, PingId,
            agent_movement_complete::{AgentData, Data, SimData},
            packet_ack::{PacketAck, Packets},
        },
    };
    use reqwest::StatusCode;
    use serde::{Deserialize, Serialize};
    use test_support::{lludp::vectors, parse_hex_fixture};
    use tokio::{
        net::UdpSocket,
        sync::{mpsc, watch},
        task::JoinSet,
        task::{spawn, yield_now},
        time::{sleep, timeout},
    };
    use url::Url;

    const TEST_TIMEOUT: Duration = Duration::from_secs(5);

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
    async fn typed_get_and_post() {
        #[derive(Deserialize, Debug, Eq, PartialEq)]
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

        let posted: Reply = client.post(&url, &Request { count: 7 }).await.expect("typed POST response");
        let fetched: Reply = client.get(&url).await.expect("typed GET response");
        let requests = server.finish();

        assert_eq!(posted, Reply { count: 7 });
        assert_eq!(fetched, Reply { count: 7 });
        let post = str::from_utf8(&requests[0]).expect("POST is UTF-8");
        assert!(post.starts_with("POST /capability HTTP/1.1"));
        let (_, post_body) = post.split_once("\r\n\r\n").expect("POST has a body");
        let payload: Value = xml::from_str(post_body).expect("POST body is valid LLSD XML");
        assert_eq!(payload.as_map().and_then(|map| map.get("count")), Some(&Value::Integer(7)));
        assert!(str::from_utf8(&requests[1]).expect("GET is UTF-8").starts_with("GET /capability HTTP/1.1"));
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
    async fn seed_limit_is_enforced() {
        let server = LocalServer::start(vec![content_length_only_response(MAX_SEED_CAPABILITY_RESPONSE_BYTES + 1)]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

        let error = client.request_capabilities(&["TestCapability"]).await.expect_err("oversized seed response should fail");
        assert!(matches!(&error, Capability(_)));

        assert!(error.to_string().contains(&format!("{}-byte limit", MAX_SEED_CAPABILITY_RESPONSE_BYTES)));
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn response_limit_is_enforced() {
        let server = LocalServer::start(vec![content_length_only_response(MAX_CAPABILITY_RESPONSE_BYTES + 1)]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

        let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
        let error = client.post::<_, Value>(&url, &Value::Undefined).await.expect_err("oversized LLSD response should fail");
        assert!(matches!(&error, Capability(_)));

        assert!(error.to_string().contains(&format!("{}-byte limit", MAX_CAPABILITY_RESPONSE_BYTES)));
        assert_eq!(server.finish().len(), 1);
    }

    #[tokio::test]
    async fn request_body_is_bounded() {
        let server = LocalServer::start(vec![]);
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
        let payload = Value::String("N".repeat(MAX_CAPABILITY_REQUEST_BODY_BYTES + 1));

        let error = client.post::<_, Value>(&url, &payload).await.expect_err("an oversized request body should fail");

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
            tasks.push(
                client
                    .start_event_queue(SimulatorId::new(index as u64 + 1), test_source(), events.clone(), shutdown_requested.clone())
                    .expect("start event queue"),
            );
        }

        let mut polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
        for _ in 0..task_count {
            server.next_seed();
        }
        let unexpected_poll = server.polls.recv_timeout(Duration::from_millis(150)).ok();
        let admitted = unexpected_poll.is_none();
        let mut forwarded_batch = false;

        if admitted {
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
            let first_event = timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("first decoded event should be forwarded")
                .expect("event channel remains open");
            assert!(matches!(first_event, Event::CapabilityEvent { message, .. } if message == "First"));
            polls.push(server.next_poll());
            let second_event = timeout(TEST_TIMEOUT, received_events.recv())
                .await
                .expect("second decoded event should be forwarded")
                .expect("event channel remains open");
            forwarded_batch =
                no_poll_while_forwarding && matches!(second_event, Event::CapabilityEvent { message, .. } if message == "Second");
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
            tasks.push(
                clients[0]
                    .start_event_queue(SimulatorId::new(index as u64 + 1), test_source(), events.clone(), shutdown_requested.clone())
                    .expect("start admitted event queue"),
            );
        }
        let polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES {
            server.next_seed();
        }

        let waiting_client = clients[1].clone();
        let waiting_task = waiting_client
            .start_event_queue(SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1), test_source(), events, shutdown_requested.clone())
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

        client.close_session();
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
    async fn event_delivery_acks_batch() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/opaque/event/queue-token");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, EVENT_RESPONSE)),
            ]
        });
        let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("event queue should deliver the batch")
            .expect("event channel should remain open");
        let Event::CapabilityEvent { source, message, body, .. } = event else {
            panic!("expected a generic capability event");
        };
        assert_eq!(source, test_source());
        assert_eq!(message, "ViewerSpecificMessage");
        assert_eq!(body, Value::Map(Map::from([("value".to_owned(), Value::Array(vec![Value::Integer(12), Value::Boolean(true)]),)])));

        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_body(&requests[0]).contains("EventQueueGet"));
        assert!(request_line(&requests[1]).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
        let first_poll = decode_request_body(&requests[1]);
        assert_eq!(map_field(&first_poll, "ack"), Some(&Value::Undefined));
        assert_eq!(map_field(&first_poll, "done"), Some(&Value::Boolean(false)));

        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("event queue should stop after shutdown");
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

        let mut saw_enable = false;
        let mut saw_establish = false;
        let mut saw_capabilities = false;
        while !(saw_enable && saw_establish && saw_capabilities) {
            let event =
                timeout(TEST_TIMEOUT, received_events.recv()).await.expect("forward capability events").expect("network event channel");
            match event {
                Event::CapabilityEvent { message, .. } if message == "EnableSimulator" => saw_enable = true,
                Event::CapabilityEvent { message, body, .. } if message == "EstablishAgentCommunication" => {
                    saw_establish = true;
                    assert!(matches!(
                        body.as_map().and_then(|fields| fields.get("seed-capability")),
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
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let seed_request = server.next_request();
        let poll_request = server.next_request();
        assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&poll_request).starts_with("POST /event-queue HTTP/1.1"));
        let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
        while tokio::time::Instant::now() < deadline {
            assert!(
                matches!(server.requests.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)),
                "poller requested another batch while blocked"
            );
            sleep(Duration::from_millis(10)).await;
        }

        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel a blocked event send");
        assert!(matches!(received_events.try_recv(), Ok(Event::CapabilityEvent { message, .. }) if message == "First"));
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
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let seed_request = server.next_request();
        let first_poll = server.next_request();
        sleep(Duration::from_millis(100)).await;
        assert!(matches!(server.requests.try_recv(), Err(std::sync::mpsc::TryRecvError::Empty)), "a fast 502 should use retry backoff");

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
        let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let mut received_events = EventReceiver::new(received_events);
        let (shutdown, shutdown_requested) = watch::channel(false);
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let requests = server.finish();
        assert_eq!(requests.len(), 2);
        assert!(received_events.try_recv().is_err(), "a malformed batch must not be dispatched");

        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("retry delay should be cancelable");
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
        let error = decode_event_batch(xml.as_bytes()).err().expect("oversized event body should be rejected");
        assert!(error.contains("event exceeded"));

        let one_event = "<map><key>message</key><string>x</string></map>";
        let events = one_event.repeat(1025);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let error = decode_event_batch(xml.as_bytes()).err().expect("oversized event batch should be rejected");
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
        let error = decode_event_batch(&exact_response).err().expect("oversized response should be rejected");
        assert!(error.contains("byte limit"));
    }

    #[test]
    fn event_data_limits() {
        let nested = format!("{}<undef/>{}", "<array>".repeat(65), "</array>".repeat(65));
        let xml = format!("<llsd>{nested}</llsd>");
        let error = decode_event_batch(xml.as_bytes()).err().expect("excessive LLSD nesting should be rejected before parsing");
        assert!(error.contains("nesting exceeded"));

        let body = "x".repeat(200_000);
        let one_event = format!("<map><key>message</key><string>Large</string><key>body</key><string>{body}</string></map>");
        let events = one_event.repeat(42);
        let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
        let error = decode_event_batch(xml.as_bytes()).err().expect("aggregate event data should be bounded");
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
        let linden_internal_error = StatusCode::from_u16(499).expect("499 is a valid HTTP status");
        for status in [linden_internal_error, StatusCode::BAD_GATEWAY, StatusCode::SERVICE_UNAVAILABLE, StatusCode::GATEWAY_TIMEOUT] {
            assert!(!expected_empty_poll(status, Duration::from_secs(9)));
            assert!(expected_empty_poll(status, Duration::from_secs(10)));
        }
        for status in [StatusCode::INTERNAL_SERVER_ERROR, StatusCode::NOT_FOUND, StatusCode::TOO_MANY_REQUESTS] {
            assert!(!expected_empty_poll(status, Duration::from_secs(30)));
        }
        let fast_timeout = Error::Timeout { elapsed: Duration::from_secs(9), message: String::new() };
        let long_timeout = Error::Timeout { elapsed: Duration::from_secs(10), message: String::new() };
        assert!(!fast_timeout.is_timeout_after(Duration::from_secs(10)));
        assert!(long_timeout.is_timeout_after(Duration::from_secs(10)));
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
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let seed = server.next_request();
        assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
        let poll = server.next_request();
        assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
        shutdown.send_replace(true);
        timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel the held request");
        assert!(server.finish().is_empty());
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
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

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
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("404 should stop event queue polling")
            .expect("event channel should remain open");
        let Event::CapabilityError { capability, reason, .. } = event else {
            panic!("expected a capability error");
        };
        assert_eq!(capability, "EventQueueGet");
        assert!(reason.contains("HTTP 404"));
        assert_eq!(server.finish().len(), 2, "404 must not trigger another poll");
        assert_eq!(task.wait().await, EventQueueExit::PollCanceled);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn primary_event_queue_404_keeps_session_connected() {
        let server = EventQueueTestServer::start(|base| {
            let mut event_url = base.clone();
            event_url.set_path("/event-queue");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
            ]
        });
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let agent_id = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
        let session = AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address: simulator.local_addr().expect("simulator address"),
            agent_id,
            session_id: UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID"),
            circuit_code: 0x1122_3344,
            seed_capability: server.url("/seed"),
        };
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let mut events = EventReceiver::new(event_receiver);
        let (disconnect, disconnect_requested) = watch::channel(false);
        let mut worker = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

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

        let mut session_ready = false;
        let mut poll_canceled = false;
        timeout(TEST_TIMEOUT, async {
            while !session_ready || !poll_canceled {
                match events.recv().await.expect("session event stream remains open") {
                    Event::SessionReady { .. } => session_ready = true,
                    Event::CapabilityError { reason, .. } if reason.contains("HTTP 404") && reason.contains("polling stopped") => {
                        poll_canceled = true;
                    }
                    Event::SimulatorFailed { reason, .. } | Event::Failed(reason) => {
                        panic!("primary session failed after EventQueue 404: {reason}")
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("session startup and EventQueue cancellation");

        assert!(timeout(Duration::from_millis(100), &mut worker).await.is_err(), "HTTP 404 must not stop the primary session");
        let mut datagram = [0; 65_535];
        while let Ok((size, _)) = simulator.try_recv_from(&mut datagram) {
            assert!(
                !matches!(Packet::decode(&datagram[..size]).expect("decode simulator packet").message(), Message::LogoutRequest(_)),
                "HTTP 404 must not send logout"
            );
        }

        disconnect.send_replace(true);
        let (logout, _) = receive_udp(&simulator).await;
        assert!(matches!(Packet::decode(&logout).expect("decode explicit logout").message(), Message::LogoutRequest(_)));
        timeout(TEST_TIMEOUT, worker)
            .await
            .expect("explicit disconnect stops the session")
            .expect("session task")
            .expect("clean disconnect");
        assert!(server.finish().is_empty(), "404 must end polling without another request");
    }

    fn test_fixture(source: &str) -> Vec<u8> {
        parse_hex_fixture(source).expect("valid LLUDP fixture")
    }

    fn set_sequence(bytes: &mut [u8], sequence: u32) {
        bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
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
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("seed lookup should retry after a server error")
            .expect("event channel should remain open");
        assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 404")));
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
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("seed lookup should retry after a rate limit")
            .expect("event channel should remain open");
        assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 404")));
        let requests = server.finish();
        assert_eq!(requests.len(), 3);
        assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
        assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
        task.wait().await;
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
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("seed lookup should retry after a transport failure")
            .expect("event channel should remain open");
        assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 404")));
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
        let task =
            client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

        let event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("permanent seed failure should be reported")
            .expect("event channel should remain open");
        assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 401")));
        assert_eq!(server.finish().len(), 1, "permanent errors must not be retried");
        task.wait().await;
    }

    const EVENT_RESPONSE: &str = "<llsd><map><key>id</key><integer>4</integer><key>events</key><array><map><key>message</key><string>ViewerSpecificMessage</string><key>body</key><map><key>value</key><array><integer>12</integer><boolean>true</boolean></array></map></map></array></map></llsd>";
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
        seeds: std::sync::mpsc::Receiver<()>,
        polls: std::sync::mpsc::Receiver<TcpStream>,
        stop: std::sync::mpsc::Sender<()>,
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
            let (seed_sender, seeds) = std::sync::mpsc::channel();
            let (poll_sender, polls) = std::sync::mpsc::channel();
            let (stop, stop_receiver) = std::sync::mpsc::channel();
            let worker = thread::spawn(move || {
                while let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) {
                    stream.set_nonblocking(false).expect("set budget stream blocking");
                    stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set budget request timeout");
                    let request = read_request(&mut stream);
                    match request_line(&request).split_whitespace().nth(1) {
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

    pub(super) enum EventServerResponse {
        Response(Vec<u8>),
        Hold,
        DropConnection,
    }

    pub(super) struct EventQueueTestServer {
        base: Url,
        requests: std::sync::mpsc::Receiver<Vec<u8>>,
        stop: std::sync::mpsc::Sender<()>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl EventQueueTestServer {
        pub(super) fn start(responses: impl FnOnce(&Url) -> Vec<EventServerResponse>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue test server");
            listener.set_nonblocking(true).expect("set test listener nonblocking");
            let address = listener.local_addr().expect("read test server address");
            let base = Url::parse(&format!("http://{address}/")).expect("valid local test URL");
            let responses = responses(&base);
            let (request_sender, requests) = std::sync::mpsc::channel();
            let (stop, stop_receiver) = std::sync::mpsc::channel();
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

        pub(super) fn url(&self, path: &str) -> Url {
            let mut url = self.base.clone();
            url.set_path(path);
            url
        }

        fn next_request(&self) -> Vec<u8> {
            self.requests.recv_timeout(TEST_TIMEOUT).expect("server should receive HTTP request")
        }

        pub(super) fn stop(&self) {
            let _ = self.stop.send(());
        }

        pub(super) fn finish(mut self) -> Vec<Vec<u8>> {
            if let Some(worker) = self.worker.take() {
                worker.join().expect("event queue server should stop");
            }
            self.requests.try_iter().collect()
        }
    }

    fn accept_until_stopped(listener: &TcpListener, stop: &std::sync::mpsc::Receiver<()>) -> Option<(TcpStream, SocketAddr)> {
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
                        Value::Map(Map::from([
                            ("AgentID".to_owned(), Value::UUID(agent_id)),
                            ("SessionID".to_owned(), Value::UUID(session_id)),
                        ])),
                    ),
                    (
                        "RegionData".to_owned(),
                        Value::Map(Map::from([
                            ("RegionHandle".to_owned(), Value::Binary(region_handle.to_be_bytes().to_vec().into())),
                            ("SimIP".to_owned(), Value::Binary(endpoint.ip().octets().to_vec().into())),
                            ("SimPort".to_owned(), Value::Integer(i32::from(endpoint.port()))),
                            ("SeedCapability".to_owned(), Value::String(seed.to_string())),
                        ])),
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

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let size = stream.read(&mut chunk).expect("read test HTTP request");
            if size == 0 {
                return request;
            }
            request.extend_from_slice(&chunk[..size]);
            if let Some(body_start) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let content_length = request[..body_start]
                    .split(|byte| *byte == b'\n')
                    .filter_map(|line| str::from_utf8(line).ok())
                    .find_map(|line| {
                        let (name, value) = line.trim().split_once(':')?;
                        name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                    })
                    .unwrap_or(0);
                if request.len() >= body_start + 4 + content_length {
                    return request;
                }
            }
        }
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
