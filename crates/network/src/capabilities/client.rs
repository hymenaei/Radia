use std::{collections::HashMap, fmt, future::Future, net::SocketAddr, sync::Arc, time::Duration};

use llsd::xml;
use reqwest::Url;
use serde::Serialize;
use tokio::{
    runtime::Handle,
    sync::{Mutex, OwnedSemaphorePermit, mpsc, oneshot, watch},
    task::{AbortHandle, JoinError, JoinHandle},
    time,
};
use tracing::{Instrument, Span};

use super::{
    budget::{
        COMMAND_CAPACITY, CapabilityBudget, EVENT_QUEUE_REQUEST_TIMEOUT, LLSD_SCAN_WORKING_BYTES, MAX_CAPABILITY_REQUEST_BODY_BYTES,
        MAX_CAPABILITY_RESPONSE_BYTES, REQUEST_TIMEOUT, spawn_blocking_with_permit,
    },
    event_queue::{self, EventQueueTask},
    request::{
        AdmittedCapabilities, AdmittedResponse, Command, RequestDeadline, capability_error, capability_llsd_limits_with_response_limit,
        decode_capability_value_with_response_limit, run, validate_capability_names, wait_for_session_close,
    },
};
use crate::{
    coverage::CoverageLossLedger,
    error::Error,
    event::{EventEnvelope, SimulatorId},
    http::{self, validate_http_url},
    status::NetworkStatusHandle,
};

#[derive(Clone)]
pub struct CapabilityUrl {
    pub(super) url: Url,
}

impl CapabilityUrl {
    pub(super) fn from_discovery(url: Url) -> Self {
        Self { url }
    }

    pub fn parse(value: &str) -> Result<Self, Error> {
        let url = Url::parse(value).map_err(|_| Error::Capability("Capability URL is invalid.".into()))?;
        validate_http_url(&url).map_err(Error::Capability)?;
        Ok(Self { url })
    }

    pub fn with_query_pair(&self, name: &str, value: &str) -> Self {
        let mut url = self.url.clone();
        url.query_pairs_mut().append_pair(name, value);
        Self { url }
    }

    pub fn with_trailing_slash(&self) -> Self {
        let mut url = self.url.clone();
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        Self { url }
    }
}

impl fmt::Debug for CapabilityUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("CapabilityUrl").field(&"[REDACTED]").finish()
    }
}

#[derive(Clone)]
pub struct Client {
    seed: Arc<Url>,
    runtime: Handle,
    pub(super) commands: mpsc::Sender<Command>,
    http: http::Client,
    session_closed: watch::Sender<bool>,
    request_worker: Arc<RequestWorker>,
    pub(super) budget: CapabilityBudget,
    status: NetworkStatusHandle,
    request_timeout: Duration,
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Client").field("seed", &"<redacted>").finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(seed: Url) -> Result<Self, Error> {
        Self::new_with_status(seed, NetworkStatusHandle::default()).map_err(Error::Capability)
    }

    pub(crate) fn new_with_status(seed: Url, status: NetworkStatusHandle) -> Result<Self, String> {
        Self::new_with_status_and_budget(seed, status, CapabilityBudget::new())
    }

    pub(crate) fn new_with_status_and_budget(seed: Url, status: NetworkStatusHandle, budget: CapabilityBudget) -> Result<Self, String> {
        validate_http_url(&seed)?;
        let runtime = Handle::try_current().map_err(|_| "A Tokio runtime is required for capability requests.".to_string())?;
        let http = budget
            .http_client()?
            .with_status(status.clone())
            .with_memory_budget(budget.response_memory.clone(), http::ResponseMemoryClass::Ordinary);
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let task = runtime.spawn(run(http.clone(), seed.clone(), receiver, session_closed_receiver, status.clone()).in_current_span());
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

    #[cfg(test)]
    pub(crate) fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

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
        time::timeout_at(
            deadline.at,
            self.commands.send(Command::RequestCapabilities { names, reply, deadline, span: Span::current(), _permit: permit }),
        )
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

    pub async fn post<Request>(&self, url: &CapabilityUrl, payload: &Request) -> Result<llsd::Value, Error>
    where
        Request: Serialize + ?Sized,
    {
        self.post_with_response_limit(url, payload, MAX_CAPABILITY_RESPONSE_BYTES).await
    }

    pub async fn post_with_response_limit<Request>(
        &self,
        url: &CapabilityUrl,
        payload: &Request,
        max_response_bytes: usize,
    ) -> Result<llsd::Value, Error>
    where
        Request: Serialize + ?Sized,
    {
        if max_response_bytes == 0 || max_response_bytes > MAX_CAPABILITY_RESPONSE_BYTES {
            return Err(Error::Capability(format!(
                "Capability response limit must be between 1 and {MAX_CAPABILITY_RESPONSE_BYTES} bytes."
            )));
        }
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
            let response = client.post_admitted(url, body, max_response_bytes, deadline, permit).await?;
            decode_admitted_response(response, client.http.clone(), deadline, || {}).await
        })
        .await
    }

    async fn post_admitted(
        &self,
        url: CapabilityUrl,
        body: Vec<u8>,
        max_response_bytes: usize,
        deadline: RequestDeadline,
        permit: OwnedSemaphorePermit,
    ) -> Result<AdmittedResponse, Error> {
        let (reply, response) = oneshot::channel();
        time::timeout_at(
            deadline.at,
            self.commands.send(Command::Post { url, body, max_response_bytes, reply, deadline, span: Span::current(), _permit: permit }),
        )
        .await
        .map_err(|_| deadline.error())?
        .map_err(|_| Error::CapabilitySessionClosed)?;
        time::timeout_at(deadline.at, response)
            .await
            .map_err(|_| deadline.error())?
            .map_err(|_| self.request_ended_error())?
            .map_err(capability_error)
    }

    pub async fn post_bytes_with_response_limit<Request>(
        &self,
        url: &CapabilityUrl,
        payload: &Request,
        max_response_bytes: usize,
    ) -> Result<Vec<u8>, Error>
    where
        Request: Serialize + ?Sized,
    {
        if max_response_bytes == 0 || max_response_bytes > MAX_CAPABILITY_RESPONSE_BYTES {
            return Err(Error::Capability(format!(
                "Capability response limit must be between 1 and {MAX_CAPABILITY_RESPONSE_BYTES} bytes."
            )));
        }
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit(deadline).await?;
        let body = xml::to_vec_with_max_bytes(payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
            .map_err(|error| Error::Capability(format!("Could not encode an LLSD request: {error}")))?;
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        let url = url.clone();
        let response = self.on_runtime(async move { client.post_admitted(url, body, max_response_bytes, deadline, permit).await }).await?;
        Ok(response.body)
    }

    pub async fn get(&self, url: &CapabilityUrl) -> Result<llsd::Value, Error> {
        self.get_with_decode_hook(url, || {}).await
    }

    pub(super) async fn get_with_decode_hook(
        &self,
        url: &CapabilityUrl,
        before_decode: impl FnOnce() + Send + 'static,
    ) -> Result<llsd::Value, Error> {
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit(deadline).await?;
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let response = client.get_bytes_admitted_with_permit(url, MAX_CAPABILITY_RESPONSE_BYTES, deadline, permit).await?;
            decode_admitted_response(response, client.http.clone(), deadline, before_decode).await
        })
        .await
    }

    pub async fn get_bytes(&self, url: &CapabilityUrl) -> Result<Vec<u8>, Error> {
        self.get_bytes_with_response_limit(url, MAX_CAPABILITY_RESPONSE_BYTES).await
    }

    pub async fn get_bytes_with_response_limit(&self, url: &CapabilityUrl, max_response_bytes: usize) -> Result<Vec<u8>, Error> {
        if max_response_bytes == 0 || max_response_bytes > MAX_CAPABILITY_RESPONSE_BYTES {
            return Err(Error::Capability(format!(
                "Capability response limit must be between 1 and {MAX_CAPABILITY_RESPONSE_BYTES} bytes."
            )));
        }
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit(deadline).await?;
        let client = self.clone();
        let url = url.clone();
        let response =
            self.on_runtime(async move { client.get_bytes_admitted_with_permit(url, max_response_bytes, deadline, permit).await }).await?;
        Ok(response.body)
    }

    async fn get_bytes_admitted_with_permit(
        &self,
        url: CapabilityUrl,
        max_response_bytes: usize,
        deadline: RequestDeadline,
        permit: OwnedSemaphorePermit,
    ) -> Result<AdmittedResponse, Error> {
        let (reply, response) = oneshot::channel();
        time::timeout_at(
            deadline.at,
            self.commands.send(Command::GetBytes { url, max_response_bytes, reply, deadline, span: Span::current(), _permit: permit }),
        )
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
        let task = self.runtime.spawn(future.in_current_span());
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

    pub(crate) fn close_session(&self) {
        self.session_closed.send_replace(true);
    }

    pub(crate) async fn close_session_and_wait(&self) {
        self.close_session();
        self.wait_for_worker().await;
    }

    pub(crate) async fn wait_for_worker(&self) {
        if self.request_worker.wait().await.is_err() {
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

    pub(super) fn start_event_queue_with_role(
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
        let task = runtime.spawn(
            async move {
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
            }
            .in_current_span(),
        );
        Ok(EventQueueTask::new(task, completion, role))
    }
}

struct RequestWorker {
    task: Mutex<Option<JoinHandle<()>>>,
}

impl RequestWorker {
    async fn wait(&self) -> Result<(), JoinError> {
        let mut task = self.task.lock().await;
        let result = match task.as_mut() {
            Some(task) => task.await,
            None => return Ok(()),
        };
        *task = None;
        result
    }
}

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
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

fn admit_capability_decode_memory_with_response_limit(
    body: &[u8],
    http: &http::Client,
    max_response_bytes: usize,
) -> Result<http::ResponseMemoryPermit, Error> {
    let mut memory =
        http.try_reserve_memory(http::ResponseMemoryClass::Ordinary, LLSD_SCAN_WORKING_BYTES).map_err(Error::CapabilityHttp)?;
    let bound = xml::value_allocation_bound(body, capability_llsd_limits_with_response_limit(max_response_bytes))
        .map_err(capability_preflight_error)?;
    memory.try_grow(bound).map_err(Error::CapabilityHttp)?;
    Ok(memory)
}

async fn decode_admitted_response(
    response: AdmittedResponse,
    http: http::Client,
    deadline: RequestDeadline,
    before_decode: impl FnOnce() + Send + 'static,
) -> Result<llsd::Value, Error> {
    let AdmittedResponse { body, max_response_bytes, _memory: raw_memory, _request: request } = response;
    let (decoded, request) = time::timeout_at(
        deadline.at,
        spawn_blocking_with_permit(request, move || {
            let decoded = if deadline.expired() {
                Err(deadline.error())
            } else {
                admit_capability_decode_memory_with_response_limit(&body, &http, max_response_bytes).and_then(|memory| {
                    if deadline.expired() {
                        return Err(deadline.error());
                    }
                    before_decode();
                    if deadline.expired() {
                        return Err(deadline.error());
                    }
                    decode_capability_value_with_response_limit(&body, max_response_bytes).map(|value| (value, memory))
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
#[path = "client_tests.rs"]
mod tests;
