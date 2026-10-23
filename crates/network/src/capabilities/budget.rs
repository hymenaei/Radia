use std::{
    mem::size_of,
    sync::{Arc, OnceLock},
    time::Duration,
};

use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::{JoinError, spawn_blocking},
};

use crate::http;

pub(super) const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const EVENT_QUEUE_REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
pub(super) const COMMAND_CAPACITY: usize = 64;
pub(super) const CLOSED_ERROR: &str = "The capability connection has closed.";
pub(crate) const MAX_SEED_CAPABILITY_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_CAPABILITY_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_EVENT_QUEUE_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES: usize = 8 * 1024 * 1024;
pub(super) const MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES: usize = MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES * MAX_CONCURRENT_EVENT_QUEUES;
pub(super) const MAX_LLSD_NESTING_DEPTH: usize = 64;
pub(super) const LLSD_SCAN_WORKING_BYTES: usize = 8 * MAX_LLSD_NESTING_DEPTH * size_of::<llsd::Value>();
pub(crate) const MAX_LLSD_XML_ELEMENTS: usize = 262_144;
pub(crate) const MAX_CONCURRENT_CAPABILITY_REQUESTS: usize = 4;
pub(crate) const MAX_CONCURRENT_EVENT_QUEUES: usize = 9;
pub(super) const MAX_CONCURRENT_PRIMARY_EVENT_QUEUES: usize = 1;
pub(super) const MAX_CONCURRENT_NEIGHBOR_EVENT_QUEUES: usize = MAX_CONCURRENT_EVENT_QUEUES - MAX_CONCURRENT_PRIMARY_EVENT_QUEUES;
const ORDINARY_RESPONSE_MEMORY_BYTES: usize = MAX_CAPABILITY_RESPONSE_BYTES * MAX_CONCURRENT_CAPABILITY_REQUESTS;
const EVENT_QUEUE_RESPONSE_MEMORY_BYTES: usize =
    (2 * MAX_EVENT_QUEUE_RESPONSE_BYTES + MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES) * MAX_CONCURRENT_EVENT_QUEUES;
const SHARED_RESPONSE_MEMORY_BYTES: usize = MAX_EVENT_QUEUE_RESPONSE_BYTES;
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
    pub(super) response_memory: http::ResponseMemoryBudget,
    http_client: Arc<OnceLock<Result<http::Client, String>>>,
}

impl CapabilityBudget {
    pub(crate) fn new() -> Self {
        Self {
            ordinary_requests: Arc::new(Semaphore::new(MAX_CONCURRENT_CAPABILITY_REQUESTS)),
            primary_event_queues: Arc::new(Semaphore::new(MAX_CONCURRENT_PRIMARY_EVENT_QUEUES)),
            neighbor_event_queues: Arc::new(Semaphore::new(MAX_CONCURRENT_NEIGHBOR_EVENT_QUEUES)),
            queued_event_bytes: Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES)),
            response_memory: http::ResponseMemoryBudget::new(
                ORDINARY_RESPONSE_MEMORY_BYTES,
                EVENT_QUEUE_RESPONSE_MEMORY_BYTES,
                crate::capture::MAX_CAPTURE_QUEUED_PAYLOAD_BYTES,
                SHARED_RESPONSE_MEMORY_BYTES,
            ),
            http_client: Arc::new(OnceLock::new()),
        }
    }

    pub(super) fn http_client(&self) -> Result<http::Client, String> {
        self.http_client
            .get_or_init(|| {
                http::Client::new(Some(REQUEST_TIMEOUT), false, None)
                    .map_err(|error| format!("Could not prepare the capability client: {error}"))
            })
            .clone()
    }
}

pub(crate) async fn spawn_blocking_with_permit<T>(
    permit: OwnedSemaphorePermit,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<(T, OwnedSemaphorePermit), JoinError>
where
    T: Send + 'static,
{
    spawn_blocking(move || (work(), permit)).await
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use tokio::{
        spawn,
        sync::{Semaphore, oneshot},
    };

    use super::spawn_blocking_with_permit;

    #[tokio::test(flavor = "multi_thread")]
    async fn cancellation_keeps_permit_until_blocking_work_finishes() {
        let slots = Arc::new(Semaphore::new(1));
        let permit = Arc::clone(&slots).acquire_owned().await.expect("request slot");
        let (entered, entered_rx) = oneshot::channel();
        let (release, release_rx) = mpsc::channel();
        let task = spawn(spawn_blocking_with_permit(permit, move || {
            entered.send(()).expect("blocking work receiver is alive");
            release_rx.recv().expect("release blocking work");
        }));

        entered_rx.await.expect("blocking work started");
        task.abort();
        assert!(task.await.expect_err("waiter must be canceled").is_cancelled());
        assert_eq!(slots.available_permits(), 0, "cancellation cannot release work admission early");

        release.send(()).expect("unblock running work");
        let permit = slots.acquire().await.expect("blocking work completion releases its permit");
        drop(permit);
    }
}
