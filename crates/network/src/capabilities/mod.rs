mod budget;
mod client;
mod event_queue;
mod request;

pub(crate) use budget::{CapabilityBudget, spawn_blocking_with_permit};
#[cfg(test)]
pub(crate) use client::DecodeGate;
pub(crate) use event_queue::{EVENT_QUEUE_NAME, EventQueueExit, EventQueueRoleHandle, EventQueueTask};
#[cfg(test)]
pub(crate) use request::allocation_measurement::{ProcessMeasurement, begin_process_measurement, measure as measure_allocations};

pub use crate::http::Error as HttpError;
pub use client::{CapabilityUrl, Client};
