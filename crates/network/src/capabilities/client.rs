use std::{collections::HashMap, fmt, future::Future, net::SocketAddr, ops::Range, sync::Arc, time::Duration};

use llsd::xml;
use reqwest::Url;
use serde::Serialize;
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, mpsc, oneshot, watch},
    task::AbortHandle,
    time,
};
use tracing::Instrument;

use super::{
    SetupError,
    budget::{
        CapabilityBudget, EVENT_QUEUE_REQUEST_TIMEOUT, MAX_CAPABILITY_REQUEST_BODY_BYTES, MAX_CAPABILITY_RESPONSE_BYTES, REQUEST_TIMEOUT,
        spawn_blocking_with_permit,
    },
    event_queue::{self, EventQueueContext, EventQueueTask, Stop},
    request::{
        RequestDeadline, decode_capability_value_with_response_limit, decode_seed_capabilities, delete_capability, get_capability_bytes,
        get_capability_range_bytes, patch_capability, post_capability, request_capabilities_response, validate_capability_names,
    },
};
use crate::{
    coverage::CoverageLossLedger,
    error::Error,
    event::{EventEnvelope, SimulatorId},
    http::{self, RedirectPolicy, validate_http_url},
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
        validate_http_url(&url).map_err(|error| Error::Capability(error.to_string()))?;
        Ok(Self { url })
    }

    pub fn with_query_pair(&self, name: &str, value: &str) -> Self {
        let mut url = self.url.clone();
        url.query_pairs_mut().append_pair(name, value);
        Self { url }
    }

    pub fn with_path_segments(&self, segments: &[&str]) -> Result<Self, Error> {
        let mut url = self.url.clone();
        let Ok(mut path) = url.path_segments_mut() else {
            return Err(Error::Capability("Capability URL cannot contain path segments.".into()));
        };
        path.pop_if_empty();
        path.extend(segments.iter().copied());
        drop(path);
        Ok(Self { url })
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContentRange {
    pub start: u64,
    pub end: u64,
    pub total: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RangeResponse {
    Partial { bytes: Vec<u8>, content_range: Option<ContentRange> },
    Full(Vec<u8>),
}

#[derive(Clone)]
pub struct Client {
    seed: Arc<Url>,
    runtime: Handle,
    http: http::Client,
    session_closed: watch::Sender<bool>,
    requests: RequestTracker,
    pub(super) budget: CapabilityBudget,
    request_timeout: Duration,
}

impl fmt::Debug for Client {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("Client").field("seed", &"<redacted>").finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(seed: Url) -> Result<Self, Error> {
        Self::new_with_budget(seed, CapabilityBudget::new()).map_err(|error| Error::Capability(error.to_string()))
    }

    pub(crate) fn new_with_budget(seed: Url, budget: CapabilityBudget) -> Result<Self, SetupError> {
        validate_http_url(&seed).map_err(|error| SetupError::InvalidSeed(Arc::new(error)))?;
        let runtime = Handle::try_current().map_err(|_| SetupError::RequestRuntimeUnavailable)?;
        let http = budget.http_client()?;
        let (session_closed, _) = watch::channel(false);
        Ok(Self {
            seed: Arc::new(seed),
            runtime,
            http,
            session_closed,
            requests: RequestTracker::new(),
            budget,
            request_timeout: REQUEST_TIMEOUT,
        })
    }

    #[tracing::instrument(
        name = "capabilities.discover", level = "debug", skip_all,
        fields(capability_count = names.len())
    )]
    pub async fn request_capabilities(&self, names: &[&str]) -> Result<HashMap<String, CapabilityUrl>, Error> {
        validate_capability_names(names)?;
        let deadline = RequestDeadline::new(self.request_timeout);
        let names = names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        self.on_runtime(async move {
            let seed = client.seed.as_ref().clone();
            let request_names = names.clone();
            let (response, permit) = client
                .run_with_permit(deadline, move |http, permit| async move {
                    let response =
                        request_capabilities_response(&http, &seed, &request_names, super::budget::MAX_SEED_CAPABILITY_RESPONSE_BYTES)
                            .await?;
                    Ok((response, permit))
                })
                .await?;
            let decode_names = names.clone();
            let decode_guard = client.requests.track();
            let decode = spawn_blocking_with_permit(permit, move || {
                let _decode_guard = decode_guard;
                decode_seed_capabilities(response, &decode_names)
            });
            let (result, permit) = client
                .race_request(deadline, async {
                    time::timeout_at(deadline.at, decode)
                        .await
                        .map_err(|_| deadline.error())?
                        .map_err(|error| Error::Capability(format!("Capability response decoder failed: {error}")))
                })
                .await?;
            drop(permit);
            let capabilities = result?;
            Ok(capabilities.into_iter().map(|(name, url)| (name, CapabilityUrl::from_discovery(url))).collect())
        })
        .await
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
        validate_response_limit(max_response_bytes)?;
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit_on_runtime(deadline).await?;
        let body = encode_request(payload)?;
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let (response, permit) = client
                .run_with_admitted_permit(deadline, permit, |http, permit| async move {
                    let response = post_capability(&http, &url.url, body, max_response_bytes).await.map_err(Error::CapabilityHttp)?;
                    Ok((response, permit))
                })
                .await?;
            client.decode_admitted_response(response, max_response_bytes, deadline, permit).await
        })
        .await
    }

    pub async fn post<Request>(&self, url: &CapabilityUrl, payload: &Request) -> Result<llsd::Value, Error>
    where
        Request: Serialize + ?Sized,
    {
        self.post_with_response_limit(url, payload, MAX_CAPABILITY_RESPONSE_BYTES).await
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
        validate_response_limit(max_response_bytes)?;
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit_on_runtime(deadline).await?;
        let body = encode_request(payload)?;
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let (response, permit) = client
                .run_with_admitted_permit(deadline, permit, |http, permit| async move {
                    let response = post_capability(&http, &url.url, body, max_response_bytes).await.map_err(Error::CapabilityHttp)?;
                    Ok((response, permit))
                })
                .await?;
            drop(permit);
            Ok(response.body)
        })
        .await
    }

    pub async fn patch_bytes_with_response_limit<Request>(
        &self,
        url: &CapabilityUrl,
        payload: &Request,
        max_response_bytes: usize,
    ) -> Result<Vec<u8>, Error>
    where
        Request: Serialize + ?Sized,
    {
        validate_response_limit(max_response_bytes)?;
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit_on_runtime(deadline).await?;
        let body = encode_request(payload)?;
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let (response, permit) = client
                .run_with_admitted_permit(deadline, permit, |http, permit| async move {
                    let response = patch_capability(&http, &url.url, body, max_response_bytes).await.map_err(Error::CapabilityHttp)?;
                    Ok((response, permit))
                })
                .await?;
            drop(permit);
            Ok(response.body)
        })
        .await
    }

    pub async fn delete_bytes_with_response_limit(&self, url: &CapabilityUrl, max_response_bytes: usize) -> Result<Vec<u8>, Error> {
        validate_response_limit(max_response_bytes)?;
        let deadline = RequestDeadline::new(self.request_timeout);
        let permit = self.acquire_request_permit_on_runtime(deadline).await?;
        if deadline.expired() {
            return Err(deadline.error());
        }
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let (response, permit) = client
                .run_with_admitted_permit(deadline, permit, |http, permit| async move {
                    let response = delete_capability(&http, &url.url, max_response_bytes).await.map_err(Error::CapabilityHttp)?;
                    Ok((response, permit))
                })
                .await?;
            drop(permit);
            Ok(response.body)
        })
        .await
    }

    pub async fn get_bytes_with_response_limit(&self, url: &CapabilityUrl, max_response_bytes: usize) -> Result<Vec<u8>, Error> {
        self.get_bytes_inner(url, None, max_response_bytes, RedirectPolicy::None).await
    }

    pub async fn get_bytes_with_response_limit_and_redirect_policy(
        &self,
        url: &CapabilityUrl,
        max_response_bytes: usize,
        redirect_policy: RedirectPolicy,
    ) -> Result<Vec<u8>, Error> {
        self.get_bytes_inner(url, None, max_response_bytes, redirect_policy).await
    }

    pub async fn get_range_with_response_limit_and_redirect_policy(
        &self,
        url: &CapabilityUrl,
        range: Range<u64>,
        max_response_bytes: usize,
        redirect_policy: RedirectPolicy,
    ) -> Result<RangeResponse, Error> {
        validate_response_limit(max_response_bytes)?;
        if range.start >= range.end {
            return Err(Error::Capability("Capability byte range must be non-empty.".into()));
        }
        let deadline = RequestDeadline::new(self.request_timeout);
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let (response, permit) = client
                .run_with_permit(deadline, |http, permit| async move {
                    let response = get_capability_range_bytes(&http, &url.url, range, max_response_bytes, redirect_policy)
                        .await
                        .map_err(Error::CapabilityHttp)?;
                    Ok((response, permit))
                })
                .await?;
            drop(permit);
            Ok(response)
        })
        .await
    }

    pub async fn get_bytes_with_accept(&self, url: &CapabilityUrl, accept: &str, max_response_bytes: usize) -> Result<Vec<u8>, Error> {
        self.get_bytes_inner(url, Some(accept.to_owned()), max_response_bytes, RedirectPolicy::None).await
    }

    async fn get_bytes_inner(
        &self,
        url: &CapabilityUrl,
        accept: Option<String>,
        max_response_bytes: usize,
        redirect_policy: RedirectPolicy,
    ) -> Result<Vec<u8>, Error> {
        validate_response_limit(max_response_bytes)?;
        let deadline = RequestDeadline::new(self.request_timeout);
        let client = self.clone();
        let url = url.clone();
        self.on_runtime(async move {
            let (response, permit) = client
                .run_with_permit(deadline, |http, permit| async move {
                    let response = get_capability_bytes(&http, &url.url, accept.as_deref(), max_response_bytes, redirect_policy)
                        .await
                        .map_err(Error::CapabilityHttp)?;
                    Ok((response, permit))
                })
                .await?;
            drop(permit);
            Ok(response.body)
        })
        .await
    }

    async fn run_with_permit<T, F, Fut>(&self, deadline: RequestDeadline, operation: F) -> Result<T, Error>
    where
        F: FnOnce(http::Client, OwnedSemaphorePermit) -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        let permit = self.acquire_request_permit(deadline).await?;
        self.run_with_admitted_permit(deadline, permit, operation).await
    }

    async fn run_with_admitted_permit<T, F, Fut>(
        &self,
        deadline: RequestDeadline,
        permit: OwnedSemaphorePermit,
        operation: F,
    ) -> Result<T, Error>
    where
        F: FnOnce(http::Client, OwnedSemaphorePermit) -> Fut,
        Fut: Future<Output = Result<T, Error>>,
    {
        self.race_request(deadline, operation(self.http.clone(), permit)).await
    }

    async fn acquire_request_permit(&self, deadline: RequestDeadline) -> Result<OwnedSemaphorePermit, Error> {
        let budget = Arc::clone(&self.budget.ordinary_requests);
        let mut session_closed = self.session_closed.subscribe();
        if *session_closed.borrow() {
            return Err(Error::CapabilitySessionClosed);
        }
        tokio::select! {
            biased;
            _ = session_closed.wait_for(|closed| *closed) => Err(Error::CapabilitySessionClosed),
            _ = time::sleep_until(deadline.at) => Err(deadline.error()),
            permit = budget.acquire_owned() => permit.map_err(|_| Error::CapabilityRequestCanceled),
        }
    }

    async fn acquire_request_permit_on_runtime(&self, deadline: RequestDeadline) -> Result<OwnedSemaphorePermit, Error> {
        let client = self.clone();
        self.on_runtime(async move { client.acquire_request_permit(deadline).await }).await
    }

    async fn race_request<T>(&self, deadline: RequestDeadline, operation: impl Future<Output = Result<T, Error>>) -> Result<T, Error> {
        let mut session_closed = self.session_closed.subscribe();
        tokio::select! {
            biased;
            _ = session_closed.wait_for(|closed| *closed) => Err(Error::CapabilitySessionClosed),
            _ = time::sleep_until(deadline.at) => Err(deadline.error()),
            result = operation => result,
        }
    }

    async fn decode_admitted_response(
        &self,
        response: http::Response,
        max_response_bytes: usize,
        deadline: RequestDeadline,
        permit: OwnedSemaphorePermit,
    ) -> Result<llsd::Value, Error> {
        let decode_guard = self.requests.track();
        let decode = spawn_blocking_with_permit(permit, move || {
            let _decode_guard = decode_guard;
            decode_capability_value_with_response_limit(&response.body, max_response_bytes)
        });
        let (result, permit) = self
            .race_request(deadline, async {
                time::timeout_at(deadline.at, decode)
                    .await
                    .map_err(|_| deadline.error())?
                    .map_err(|error| Error::Capability(format!("LLSD response decoder task failed: {error}")))
            })
            .await?;
        drop(permit);
        result
    }

    fn on_runtime<T, F>(&self, future: F) -> impl Future<Output = Result<T, Error>> + '_
    where
        T: Send + 'static,
        F: Future<Output = Result<T, Error>> + Send + 'static,
    {
        let requests = self.requests.clone();
        let runtime = self.runtime.clone();
        let session_closed = self.session_closed.subscribe();
        async move {
            let request = requests.track();
            if *session_closed.borrow() {
                drop(request);
                return Err(Error::CapabilitySessionClosed);
            }
            let task = runtime.spawn(
                async move {
                    let _request = request;
                    future.await
                }
                .in_current_span(),
            );
            let abort = AbortOnDrop(task.abort_handle());
            let result = task.await;
            drop(abort);
            match result {
                Ok(result) => result,
                Err(error) => {
                    if error.is_panic() {
                        return Err(Error::Capability("The capability request task failed.".into()));
                    }
                    if *session_closed.borrow() { Err(Error::CapabilitySessionClosed) } else { Err(Error::CapabilityRequestCanceled) }
                }
            }
        }
    }

    pub(crate) fn close_session(&self) {
        self.session_closed.send_replace(true);
    }

    pub(crate) async fn close_session_and_wait(&self) {
        self.close_session();
        self.wait_for_requests().await;
    }

    pub(crate) async fn wait_for_requests(&self) {
        let mut session_closed = self.session_closed.subscribe();
        if let Ok(closed) = session_closed.wait_for(|closed| *closed).await {
            drop(closed);
        }
        self.requests.wait_until_idle().await;
    }

    pub(crate) fn start_event_queue(
        &self,
        simulator_id: SimulatorId,
        source: SocketAddr,
        events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        coverage_losses: CoverageLossLedger,
    ) -> Result<EventQueueTask, SetupError> {
        self.start_event_queue_with_role(simulator_id, source, events, shutdown, coverage_losses, false)
    }

    pub(crate) fn start_primary_event_queue(
        &self,
        simulator_id: SimulatorId,
        source: SocketAddr,
        events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        coverage_losses: CoverageLossLedger,
    ) -> Result<EventQueueTask, SetupError> {
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
    ) -> Result<EventQueueTask, SetupError> {
        let runtime = Handle::try_current().map_err(|_| SetupError::EventQueueRuntimeUnavailable)?;
        let http = self.http.clone().with_timeout(Some(EVENT_QUEUE_REQUEST_TIMEOUT));
        let seed = self.seed.as_ref().clone();
        let ordinary_requests = Arc::clone(&self.budget.ordinary_requests);
        let primary_event_queues = Arc::clone(&self.budget.primary_event_queues);
        let neighbor_event_queues = Arc::clone(&self.budget.neighbor_event_queues);
        let queued_event_bytes = Arc::clone(&self.budget.queued_event_bytes);
        let (role, primary) = event_queue::EventQueueRoleHandle::channel(is_primary);
        let session_closed = self.session_closed.subscribe();
        let (completed, completion) = oneshot::channel();
        let context = EventQueueContext {
            simulator_id,
            source,
            seed,
            http,
            events,
            ordinary_requests,
            primary_event_queues,
            neighbor_event_queues,
            queued_event_bytes,
            coverage_losses,
        };
        let stop = Stop::new(shutdown, session_closed);
        let task = runtime.spawn(
            async move {
                let exit = event_queue::run_event_queue(context, stop, primary).await;
                if let Err(exit) = completed.send(exit) {
                    tracing::debug!(?exit, "EventQueueGet completion receiver was dropped");
                }
            }
            .in_current_span(),
        );
        Ok(EventQueueTask::new(task, completion, role))
    }
}

struct RequestTracker {
    active: watch::Sender<usize>,
}

impl RequestTracker {
    fn new() -> Self {
        let (active, _) = watch::channel(0);
        Self { active }
    }

    fn track(&self) -> RequestGuard {
        self.active.send_modify(|active| *active = active.saturating_add(1));
        RequestGuard(self.active.clone())
    }

    async fn wait_until_idle(&self) {
        let mut active = self.active.subscribe();
        if let Ok(active) = active.wait_for(|active| *active == 0).await {
            drop(active);
        }
    }
}

impl Clone for RequestTracker {
    fn clone(&self) -> Self {
        Self { active: self.active.clone() }
    }
}

struct RequestGuard(watch::Sender<usize>);

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.0.send_modify(|active| *active = active.saturating_sub(1));
    }
}

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn validate_response_limit(max_response_bytes: usize) -> Result<(), Error> {
    if max_response_bytes == 0 || max_response_bytes > MAX_CAPABILITY_RESPONSE_BYTES {
        return Err(Error::Capability(format!("Capability response limit must be between 1 and {MAX_CAPABILITY_RESPONSE_BYTES} bytes.")));
    }
    Ok(())
}

fn encode_request<Request: Serialize + ?Sized>(payload: &Request) -> Result<Vec<u8>, Error> {
    xml::to_vec_with_max_bytes(payload, MAX_CAPABILITY_REQUEST_BODY_BYTES)
        .map_err(|error| Error::Capability(format!("Could not encode an LLSD request: {error}")))
}

#[cfg(test)]
pub(crate) mod test_values {
    use std::time::Duration;

    use super::Client;

    pub(crate) fn client_with_request_timeout(mut client: Client, timeout: Duration) -> Client {
        client.request_timeout = timeout;
        client
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
