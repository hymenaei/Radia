use std::{fmt, mem, mem::size_of, str::from_utf8};

use llsd::{Binary, Uri, Value};
use tokio::{sync::OwnedSemaphorePermit, task::JoinError};

use super::super::{
    budget::{MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES, MAX_EVENT_QUEUE_RESPONSE_BYTES, spawn_blocking_with_permit},
    request::decode_llsd_xml,
};
use crate::event::{CapabilityEvent, EventEnvelope};

const MAX_EVENT_QUEUE_BATCH_EVENTS: usize = 1024;

#[derive(Debug)]
pub(super) enum BatchDecodeError {
    Decode(String),
    Task(JoinError),
}

impl fmt::Display for BatchDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => formatter.write_str(error),
            Self::Task(error) => write!(formatter, "EventQueueGet decoder task failed: {error}"),
        }
    }
}

pub(super) async fn decode_admitted_event_batch(
    body: Vec<u8>,
    event_queue_permit: OwnedSemaphorePermit,
) -> Result<(i32, Vec<CapabilityEvent>, OwnedSemaphorePermit), BatchDecodeError> {
    let (decoded, event_queue_permit) =
        spawn_blocking_with_permit(event_queue_permit, move || decode_event_batch(&body).map_err(BatchDecodeError::Decode))
            .await
            .map_err(BatchDecodeError::Task)?;
    decoded.map(|(id, events)| (id, events, event_queue_permit))
}

pub(super) fn decode_event_batch(body: &[u8]) -> Result<(i32, Vec<CapabilityEvent>), String> {
    if body.len() > MAX_EVENT_QUEUE_RESPONSE_BYTES {
        return Err(format!("EventQueueGet response exceeded the {MAX_EVENT_QUEUE_RESPONSE_BYTES}-byte limit."));
    }
    let body = from_utf8(body).map_err(|error| format!("EventQueueGet response was not UTF-8: {error}"))?;
    let Value::Map(mut fields) = decode_llsd_xml(body, "Could not decode an EventQueueGet response")? else {
        return Err("EventQueueGet returned a non-map response.".to_string());
    };
    let Some(Value::Integer(id)) = fields.remove("id") else {
        return Err("EventQueueGet response omitted or returned an invalid id.".to_string());
    };
    let Some(Value::Array(event_values)) = fields.remove("events") else {
        return Err("EventQueueGet response omitted its events array.".to_string());
    };
    if event_values.len() > MAX_EVENT_QUEUE_BATCH_EVENTS {
        return Err(format!("EventQueueGet batch exceeded the {MAX_EVENT_QUEUE_BATCH_EVENTS}-event limit."));
    }
    let mut events = Vec::with_capacity(event_values.len());

    for value in event_values {
        let Value::Map(mut fields) = value else {
            return Err("EventQueueGet event was not a map.".to_string());
        };
        let Some(Value::String(message)) = fields.remove("message") else {
            return Err("EventQueueGet event omitted its message name.".to_string());
        };
        let mut body = fields.remove("body").unwrap_or(Value::Undefined);
        let event_size = queued_event_retained_size(message.capacity(), &mut body);
        if event_size > super::MAX_QUEUED_CAPABILITY_EVENT_BYTES {
            return Err(format!("EventQueueGet event exceeded the {}-byte limit.", super::MAX_QUEUED_CAPABILITY_EVENT_BYTES));
        }
        events.push(CapabilityEvent { message, body, retained_bytes: event_size });
        if capability_event_batch_retained_size(&events, events.capacity()) > MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES {
            return Err(format!("EventQueueGet batch exceeded the {MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES}-byte retained-data limit."));
        }
    }

    Ok((id, events))
}

pub(super) fn queued_event_retained_size(message_capacity: usize, body: &mut Value) -> usize {
    size_of::<CapabilityEvent>().saturating_add(message_capacity).saturating_add(llsd_heap_size(body))
}

pub(super) fn capability_event_batch_retained_size(events: &[CapabilityEvent], event_capacity: usize) -> usize {
    let event_data_bytes =
        events.iter().fold(0usize, |total, event| total.saturating_add(event.retained_bytes.saturating_sub(size_of::<CapabilityEvent>())));
    size_of::<EventEnvelope>()
        .saturating_add(size_of::<OwnedSemaphorePermit>())
        .saturating_add(2 * size_of::<usize>())
        .saturating_add(event_capacity.saturating_mul(size_of::<CapabilityEvent>()))
        .saturating_add(event_data_bytes)
}

pub(super) fn llsd_heap_size(value: &mut Value) -> usize {
    match value {
        Value::String(value) => value.capacity(),
        Value::Uri(uri) => {
            let text = mem::take(uri).into_string();
            let capacity = text.capacity();
            *uri = Uri::from(text);
            capacity
        }
        Value::Binary(binary) => {
            let bytes = mem::take(binary).into_vec();
            let capacity = bytes.capacity();
            *binary = Binary::from(bytes);
            capacity
        }
        Value::Map(values) => {
            let entry_size = size_of::<(String, Value)>();
            let node_size =
                if values.is_empty() { 0 } else { entry_size.saturating_mul(11).saturating_add(size_of::<usize>().saturating_mul(12)) };
            values.len().saturating_mul(entry_size.saturating_mul(3)).saturating_add(node_size).saturating_add(
                values
                    .iter_mut()
                    .fold(0usize, |size, (key, value)| size.saturating_add(key.capacity()).saturating_add(llsd_heap_size(value))),
            )
        }
        Value::Array(values) => values
            .capacity()
            .saturating_mul(size_of::<Value>())
            .saturating_add(values.iter_mut().fold(0usize, |size, value| size.saturating_add(llsd_heap_size(value)))),
        Value::Undefined | Value::Boolean(_) | Value::Real(_) | Value::Integer(_) | Value::Uuid(_) | Value::Date(_) => 0,
    }
}
