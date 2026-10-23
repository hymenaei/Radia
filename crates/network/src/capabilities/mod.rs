mod budget;
mod client;
mod event_queue;
mod request;

pub use crate::http::Error as HttpError;
pub(crate) use budget::{CapabilityBudget, spawn_blocking_with_permit};
#[cfg(test)]
pub(crate) use budget::{
    MAX_CAPABILITY_REQUEST_BODY_BYTES, MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS, MAX_CONCURRENT_EVENT_QUEUES,
    MAX_EVENT_QUEUE_RESPONSE_BYTES, MAX_LLSD_XML_ELEMENTS, MAX_SEED_CAPABILITY_RESPONSE_BYTES,
};
#[cfg(test)]
pub(crate) use client::tests::{EventQueueTestServer, EventServerResponse};
pub use client::{CapabilityUrl, Client};
pub(crate) use event_queue::{EVENT_QUEUE_NAME, EventQueueExit, EventQueueRoleHandle, EventQueueTask};
#[cfg(test)]
pub(crate) use event_queue::{
    MAX_EVENT_QUEUE_ERRORS, MAX_QUEUED_CAPABILITY_EVENT_BYTES, RetryOutcome, decode_event_batch, expected_empty_poll, post_event_queue,
    retry_event_queue, seed_capability_retry_delay, seed_retry_delay,
};
#[cfg(test)]
pub(crate) use request::allocation_measurement::{ProcessMeasurement, begin_process_measurement, measure as measure_allocations};
