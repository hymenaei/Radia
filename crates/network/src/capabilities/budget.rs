use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::{JoinError, spawn_blocking},
};

use super::SetupError;
use crate::http;

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const EVENT_QUEUE_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub(crate) const MAX_SEED_CAPABILITY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_CAPABILITY_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_EVENT_QUEUE_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES: usize = MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES * MAX_CONCURRENT_EVENT_QUEUES;
pub(super) const MAX_LLSD_NESTING_DEPTH: usize = 64;
pub(crate) const MAX_LLSD_XML_ELEMENTS: usize = 262_144;
pub(crate) const MAX_CONCURRENT_CAPABILITY_REQUESTS: usize = 4;
pub(crate) const MAX_CONCURRENT_EVENT_QUEUES: usize = 9;
pub(super) const MAX_CONCURRENT_PRIMARY_EVENT_QUEUES: usize = 1;
pub(super) const MAX_CONCURRENT_NEIGHBOR_EVENT_QUEUES: usize = MAX_CONCURRENT_EVENT_QUEUES - MAX_CONCURRENT_PRIMARY_EVENT_QUEUES;
pub(crate) const MAX_CAPABILITY_REQUEST_BODY_BYTES: usize = 1024 * 1024;
pub(super) const MAX_CAPABILITY_NAMES: usize = 256;
pub(super) const MAX_CAPABILITY_NAME_BYTES: usize = 128;
pub(super) const MAX_CAPABILITY_NAMES_BYTES: usize = 16 * 1024;

#[derive(Clone)]
pub(crate) struct CapabilityBudget {
    pub(super) ordinary_requests: Arc<Semaphore>,
    pub(super) primary_event_queues: Arc<Semaphore>,
    pub(super) neighbor_event_queues: Arc<Semaphore>,
    pub(super) queued_event_bytes: Arc<Semaphore>,
    http_client: Arc<OnceLock<Result<http::Client, Arc<reqwest::Error>>>>,
}

impl CapabilityBudget {
    pub(crate) fn new() -> Self {
        Self {
            ordinary_requests: Arc::new(Semaphore::new(MAX_CONCURRENT_CAPABILITY_REQUESTS)),
            primary_event_queues: Arc::new(Semaphore::new(MAX_CONCURRENT_PRIMARY_EVENT_QUEUES)),
            neighbor_event_queues: Arc::new(Semaphore::new(MAX_CONCURRENT_NEIGHBOR_EVENT_QUEUES)),
            queued_event_bytes: Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES)),
            http_client: Arc::new(OnceLock::new()),
        }
    }

    pub(super) fn http_client(&self) -> Result<http::Client, SetupError> {
        self.http_client
            .get_or_init(|| http::Client::new(Some(REQUEST_TIMEOUT), false, None).map_err(Arc::new))
            .clone()
            .map_err(SetupError::HttpClient)
    }
}

pub(crate) async fn spawn_blocking_with_permit<T>(
    permit: OwnedSemaphorePermit,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<(T, OwnedSemaphorePermit), JoinError>
where
    T: Send + 'static,
{
    let span = tracing::Span::current();
    spawn_blocking(move || span.in_scope(|| (work(), permit))).await
}

#[cfg(test)]
#[path = "tests/budget.rs"]
mod tests;
