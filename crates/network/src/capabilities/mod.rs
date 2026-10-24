mod budget;
mod client;
mod error;
mod event_queue;
mod request;

pub use crate::http::{Error as HttpError, ErrorKind as HttpErrorKind};
pub(crate) use budget::{CapabilityBudget, spawn_blocking_with_permit};
#[cfg(test)]
pub(crate) use client::test_values::client_with_request_timeout;
pub use client::{CapabilityUrl, Client};
pub(crate) use error::SetupError;
pub(crate) use event_queue::{EVENT_QUEUE_NAME, EventQueueExit, EventQueueRoleHandle, EventQueueTask};

#[cfg(test)]
pub(crate) fn available_event_queue_permits(budget: &CapabilityBudget) -> usize {
    budget.primary_event_queues.available_permits() + budget.neighbor_event_queues.available_permits()
}
