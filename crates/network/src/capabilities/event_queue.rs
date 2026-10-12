//! Simulator EventQueueGet polling, batch decoding, and retry policy.

use std::{
    mem::{size_of, take},
    net::SocketAddr,
    str::from_utf8,
    sync::Arc,
    time::Duration,
};

use llsd::{Binary, Map, URI, Value, xml};
use reqwest::{StatusCode, Url};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch},
    time,
};

use super::{
    LookupError, MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES, MAX_SEED_CAPABILITY_RESPONSE_BYTES, decode_capabilities, decode_llsd_xml,
    request_capabilities_response, wait_for_session_close,
};
#[cfg(test)]
use super::{MAX_CONCURRENT_EVENT_QUEUES, MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES};
use crate::{
    event::{Event, EventEnvelope, EventSendOutcome, SimulatorId},
    http::{self, Error, Response},
    status::NetworkStatusHandle,
};

const EVENT_QUEUE_RETRY_DELAY: Duration = Duration::from_secs(1);
const EVENT_QUEUE_RETRY_INCREMENT: Duration = Duration::from_secs(3);
const MIN_EMPTY_POLL_DURATION: Duration = Duration::from_secs(10);
pub(crate) const MAX_EVENT_QUEUE_ERRORS: u32 = 15;
const SEED_LOOKUP_RETRY_LIMIT: u32 = 5;
const MAX_SEED_RETRY_AFTER: Duration = Duration::from_secs(30);
pub(crate) const EVENT_QUEUE_NAME: &str = "EventQueueGet";
const LINDEN_HTTP_INTERNAL_ERROR: u16 = 499;
pub(crate) const MAX_EVENT_QUEUE_RESPONSE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_QUEUED_CAPABILITY_EVENT_BYTES: usize = 256 * 1024;
const MAX_EVENT_QUEUE_BATCH_EVENTS: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventQueueExit {
    Stopped,
    PollCanceled,
    Unavailable,
    RetryLimitReached,
    TaskFailed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RetryOutcome {
    Retry,
    Stopped,
    LimitReached,
}

async fn wait_for_watch_true(mut watch: watch::Receiver<bool>) {
    while !*watch.borrow() {
        if watch.changed().await.is_err() {
            break;
        }
    }
}

async fn acquire_budget_permit(
    budget: Arc<Semaphore>,
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> Option<OwnedSemaphorePermit> {
    if *shutdown.borrow() || *session_closed.borrow() {
        return None;
    }

    tokio::select! {
        biased;
        _ = wait_for_watch_true(shutdown.clone()) => None,
        _ = wait_for_session_close(session_closed.clone()) => None,
        permit = budget.acquire_owned() => permit.ok(),
    }
}

async fn acquire_event_batch_reservation(
    budget: Arc<Semaphore>,
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> Option<OwnedSemaphorePermit> {
    if *shutdown.borrow() || *session_closed.borrow() {
        return None;
    }

    tokio::select! {
        biased;
        _ = wait_for_watch_true(shutdown.clone()) => None,
        _ = wait_for_session_close(session_closed.clone()) => None,
        permit = budget.acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32) => permit.ok(),
    }
}

async fn send_event_queue_batch(
    events: &mpsc::Sender<EventEnvelope>,
    simulator_id: SimulatorId,
    source: SocketAddr,
    batch: Vec<QueuedCapabilityEvent>,
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
    mut batch_reservation: OwnedSemaphorePermit,
    _event_queue_permit: OwnedSemaphorePermit,
) -> EventSendOutcome {
    let mut envelopes = Vec::with_capacity(batch.len());
    for event in batch {
        if event.retained_bytes > MAX_QUEUED_CAPABILITY_EVENT_BYTES {
            return EventSendOutcome::Shutdown;
        }
        let Some(permit) = batch_reservation.split(event.retained_bytes) else {
            return EventSendOutcome::Shutdown;
        };
        envelopes.push(EventEnvelope::with_capability_bytes(
            Event::CapabilityEvent { simulator_id, source, message: event.message, body: event.body },
            permit,
        ));
    }
    drop(batch_reservation);

    for envelope in envelopes {
        if *shutdown.borrow() || *session_closed.borrow() {
            return EventSendOutcome::Shutdown;
        }

        let outcome = tokio::select! {
            biased;
            _ = wait_for_watch_true(shutdown.clone()) => EventSendOutcome::Shutdown,
            _ = wait_for_session_close(session_closed.clone()) => EventSendOutcome::Shutdown,
            result = events.send(envelope) => {
                match result {
                    Ok(()) => EventSendOutcome::Sent,
                    Err(_) => EventSendOutcome::ReceiverClosed,
                }
            }
        };
        if !matches!(outcome, EventSendOutcome::Sent) {
            return outcome;
        }
    }

    EventSendOutcome::Sent
}

#[tracing::instrument(name = "capabilities.event_queue", level = "debug", skip_all)]
pub(super) async fn run_event_queue(
    simulator_id: SimulatorId,
    source: SocketAddr,
    seed: Url,
    http: http::Client,
    events: mpsc::Sender<EventEnvelope>,
    mut shutdown: watch::Receiver<bool>,
    status: NetworkStatusHandle,
    session_closed: watch::Receiver<bool>,
    ordinary_requests: Arc<Semaphore>,
    event_queues: Arc<Semaphore>,
    queued_event_bytes: Arc<Semaphore>,
) -> EventQueueExit {
    if *shutdown.borrow() || *session_closed.borrow() {
        return EventQueueExit::Stopped;
    }

    let names = [EVENT_QUEUE_NAME.to_owned()];
    let mut lookup_retries = 0;
    let event_capability = loop {
        let Some(request_permit) = acquire_budget_permit(Arc::clone(&ordinary_requests), &shutdown, &session_closed).await else {
            return EventQueueExit::Stopped;
        };
        let requested = tokio::select! {
            biased;
            _ = wait_for_watch_true(shutdown.clone()) => return EventQueueExit::Stopped,
            _ = wait_for_session_close(session_closed.clone()) => return EventQueueExit::Stopped,
            result = request_capabilities_response(&http, &seed, &names, MAX_SEED_CAPABILITY_RESPONSE_BYTES) => result,
        };
        let requested = match requested {
            Ok(response) => decode_capabilities(&response.body, &names).map_err(LookupError::Permanent),
            Err(error) => Err(error),
        };
        drop(request_permit);
        match requested {
            Ok(mut granted) => {
                status.note_capabilities(granted.keys().map(String::as_str));
                match granted.remove(EVENT_QUEUE_NAME) {
                    Some(url) => break url,
                    None => {
                        status.record_error("Second Life did not provide the EventQueueGet capability.");
                        let outcome = send_event_queue_error(
                            &events,
                            Event::CapabilityError {
                                simulator_id,
                                source,
                                capability: EVENT_QUEUE_NAME.to_owned(),
                                reason: format!("Second Life did not provide the {EVENT_QUEUE_NAME} capability."),
                            },
                            &shutdown,
                            &session_closed,
                        )
                        .await;
                        return if matches!(outcome, EventSendOutcome::Sent) {
                            EventQueueExit::Unavailable
                        } else {
                            EventQueueExit::Stopped
                        };
                    }
                }
            }
            Err(LookupError::Permanent(error)) => {
                status.record_error(&error);
                let outcome = send_event_queue_error(
                    &events,
                    Event::CapabilityError {
                        simulator_id,
                        source,
                        capability: EVENT_QUEUE_NAME.to_owned(),
                        reason: format!("Could not start the simulator event queue: {error}"),
                    },
                    &shutdown,
                    &session_closed,
                )
                .await;
                return if matches!(outcome, EventSendOutcome::Sent) { EventQueueExit::Unavailable } else { EventQueueExit::Stopped };
            }
            Err(LookupError::Transient { message, retry_after }) => {
                status.record_error(&message);
                match retry_seed_capability_lookup(
                    simulator_id,
                    &mut lookup_retries,
                    source,
                    &events,
                    message,
                    retry_after,
                    &mut shutdown,
                    &session_closed,
                )
                .await
                {
                    RetryOutcome::Retry => {}
                    RetryOutcome::Stopped => return EventQueueExit::Stopped,
                    RetryOutcome::LimitReached => return EventQueueExit::Unavailable,
                }
            }
        }
    };

    let mut acknowledge = Value::Undefined;
    let mut error_count = 0;

    loop {
        if *shutdown.borrow() || *session_closed.borrow() {
            return EventQueueExit::Stopped;
        }

        let Some(event_queue_permit) = acquire_budget_permit(Arc::clone(&event_queues), &shutdown, &session_closed).await else {
            return EventQueueExit::Stopped;
        };
        let response = tokio::select! {
            biased;
            _ = wait_for_watch_true(shutdown.clone()) => return EventQueueExit::Stopped,
            _ = wait_for_session_close(session_closed.clone()) => return EventQueueExit::Stopped,
            result = post_event_queue(&http, &event_capability, &acknowledge, MAX_EVENT_QUEUE_RESPONSE_BYTES) => result,
        };

        match response {
            Ok(response) if response.status == StatusCode::NOT_FOUND => {
                drop(response);
                drop(event_queue_permit);
                status.record_error("EventQueueGet returned HTTP 404.");
                let outcome = send_event_queue_error(
                    &events,
                    Event::CapabilityError {
                        simulator_id,
                        source,
                        capability: EVENT_QUEUE_NAME.to_owned(),
                        reason: "EventQueueGet returned HTTP 404; polling stopped.".into(),
                    },
                    &shutdown,
                    &session_closed,
                )
                .await;
                return if matches!(outcome, EventSendOutcome::Sent) { EventQueueExit::PollCanceled } else { EventQueueExit::Stopped };
            }
            Ok(response) if expected_empty_poll(response.status, response.elapsed) => {
                drop(event_queue_permit);
                error_count = 0;
            }
            Ok(response) if response.status.is_success() => {
                let Some(batch_reservation) =
                    acquire_event_batch_reservation(Arc::clone(&queued_event_bytes), &shutdown, &session_closed).await
                else {
                    return EventQueueExit::Stopped;
                };
                let decoded = decode_event_batch(&response.body);
                drop(response);
                match decoded {
                    Ok((id, batch)) => {
                        acknowledge = id;
                        error_count = 0;
                        if !matches!(
                            send_event_queue_batch(
                                &events,
                                simulator_id,
                                source,
                                batch,
                                &shutdown,
                                &session_closed,
                                batch_reservation,
                                event_queue_permit,
                            )
                            .await,
                            EventSendOutcome::Sent
                        ) {
                            return EventQueueExit::Stopped;
                        }
                    }
                    Err(error) => {
                        drop(batch_reservation);
                        drop(event_queue_permit);
                        status.record_error(&error);
                        match retry_event_queue(simulator_id, &mut error_count, source, &events, error, &mut shutdown, &session_closed)
                            .await
                        {
                            RetryOutcome::Retry => {}
                            RetryOutcome::Stopped => return EventQueueExit::Stopped,
                            RetryOutcome::LimitReached => return EventQueueExit::RetryLimitReached,
                        }
                    }
                }
            }
            Ok(response) => {
                let status_code = response.status;
                drop(response);
                drop(event_queue_permit);
                let error = format!("EventQueueGet returned HTTP {status_code}.");
                status.record_error(&error);
                match retry_event_queue(simulator_id, &mut error_count, source, &events, error, &mut shutdown, &session_closed).await {
                    RetryOutcome::Retry => {}
                    RetryOutcome::Stopped => return EventQueueExit::Stopped,
                    RetryOutcome::LimitReached => return EventQueueExit::RetryLimitReached,
                }
            }
            Err(error) if error.is_timeout_after(MIN_EMPTY_POLL_DURATION) => {
                drop(event_queue_permit);
                error_count = 0;
            }
            Err(error) => {
                drop(event_queue_permit);
                let error = format!("EventQueueGet request failed: {error}");
                status.record_error(&error);
                match retry_event_queue(simulator_id, &mut error_count, source, &events, error, &mut shutdown, &session_closed).await {
                    RetryOutcome::Retry => {}
                    RetryOutcome::Stopped => return EventQueueExit::Stopped,
                    RetryOutcome::LimitReached => return EventQueueExit::RetryLimitReached,
                }
            }
        }
    }
}

pub(crate) async fn post_event_queue(
    client: &http::Client,
    url: &Url,
    acknowledge: &Value,
    max_response_bytes: usize,
) -> Result<Response, Error> {
    let payload = Value::Map(Map::from([("ack".to_owned(), acknowledge.clone()), ("done".to_owned(), Value::Boolean(false))]));
    let body = xml::to_string(&payload).map_err(|error| Error::Message(format!("Could not encode an EventQueueGet request: {error}")))?;
    client
        .post_operation_bounded_classified(
            "event_queue",
            Some(EVENT_QUEUE_NAME),
            url,
            "application/llsd+xml",
            Some("application/llsd+xml"),
            body.into_bytes(),
            max_response_bytes,
        )
        .await
}

pub(crate) fn decode_event_batch(body: &[u8]) -> Result<(Value, Vec<QueuedCapabilityEvent>), String> {
    if body.len() > MAX_EVENT_QUEUE_RESPONSE_BYTES {
        return Err(format!("EventQueueGet response exceeded the {MAX_EVENT_QUEUE_RESPONSE_BYTES}-byte limit."));
    }
    let body = from_utf8(body).map_err(|error| format!("EventQueueGet response was not UTF-8: {error}"))?;
    let Value::Map(mut fields) = decode_llsd_xml(body, "Could not decode an EventQueueGet response")? else {
        return Err("EventQueueGet returned a non-map response.".to_string());
    };
    let id = fields.remove("id").ok_or_else(|| "EventQueueGet response omitted its id.".to_string())?;
    if !matches!(id, Value::Integer(_)) {
        return Err("EventQueueGet response returned an invalid id.".to_string());
    }
    let Some(Value::Array(event_values)) = fields.remove("events") else {
        return Err("EventQueueGet response omitted its events array.".to_string());
    };
    if event_values.len() > MAX_EVENT_QUEUE_BATCH_EVENTS {
        return Err(format!("EventQueueGet batch exceeded the {MAX_EVENT_QUEUE_BATCH_EVENTS}-event limit."));
    }
    let mut events = Vec::with_capacity(event_values.len());
    let mut retained_bytes = 0usize;

    for value in event_values {
        let Value::Map(mut fields) = value else {
            return Err("EventQueueGet event was not a map.".to_string());
        };
        let Some(Value::String(message)) = fields.remove("message") else {
            return Err("EventQueueGet event omitted its message name.".to_string());
        };
        let mut body = fields.remove("body").unwrap_or(Value::Undefined);
        let event_size = queued_event_retained_size(message.capacity(), &mut body);
        if event_size > MAX_QUEUED_CAPABILITY_EVENT_BYTES {
            return Err(format!("EventQueueGet event exceeded the {MAX_QUEUED_CAPABILITY_EVENT_BYTES}-byte limit."));
        }
        retained_bytes = retained_bytes.saturating_add(event_size);
        if retained_bytes > MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES {
            return Err(format!("EventQueueGet batch exceeded the {MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES}-byte retained-data limit."));
        }
        events.push(QueuedCapabilityEvent { message, body, retained_bytes: event_size });
    }

    Ok((id, events))
}

pub(crate) struct QueuedCapabilityEvent {
    pub(crate) message: String,
    pub(crate) body: Value,
    retained_bytes: usize,
}

fn queued_event_retained_size(message_capacity: usize, body: &mut Value) -> usize {
    size_of::<QueuedCapabilityEvent>()
        .saturating_add(size_of::<EventEnvelope>())
        .saturating_add(message_capacity)
        .saturating_add(llsd_heap_size(body))
}

fn llsd_heap_size(value: &mut Value) -> usize {
    match value {
        Value::String(value) => value.capacity(),
        Value::URI(uri) => {
            let text = take(uri).into_string();
            let capacity = text.capacity();
            *uri = URI::from(text);
            capacity
        }
        Value::Binary(binary) => {
            let bytes = take(binary).into_vec();
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
        Value::Undefined | Value::Boolean(_) | Value::Real(_) | Value::Integer(_) | Value::UUID(_) | Value::Date(_) => 0,
    }
}

pub(crate) async fn retry_event_queue(
    simulator_id: SimulatorId,
    error_count: &mut u32,
    source: SocketAddr,
    events: &mpsc::Sender<EventEnvelope>,
    error: String,
    shutdown: &mut watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> RetryOutcome {
    if *shutdown.borrow() || *session_closed.borrow() {
        return RetryOutcome::Stopped;
    }

    if *error_count >= MAX_EVENT_QUEUE_ERRORS {
        let outcome = send_event_queue_error(
            events,
            Event::CapabilityError {
                simulator_id,
                source,
                capability: EVENT_QUEUE_NAME.to_owned(),
                reason: format!("Event queue stopped after repeated errors: {error}"),
            },
            shutdown,
            session_closed,
        )
        .await;
        return if matches!(outcome, EventSendOutcome::Sent) { RetryOutcome::LimitReached } else { RetryOutcome::Stopped };
    }

    let delay = event_queue_retry_delay(*error_count);
    *error_count += 1;
    wait_for_retry(delay, shutdown, session_closed).await
}

async fn retry_seed_capability_lookup(
    simulator_id: SimulatorId,
    retries: &mut u32,
    source: SocketAddr,
    events: &mpsc::Sender<EventEnvelope>,
    error: String,
    retry_after: Option<Duration>,
    shutdown: &mut watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> RetryOutcome {
    if *shutdown.borrow() || *session_closed.borrow() {
        return RetryOutcome::Stopped;
    }

    if *retries >= SEED_LOOKUP_RETRY_LIMIT {
        let outcome = send_event_queue_error(
            events,
            Event::CapabilityError {
                simulator_id,
                source,
                capability: EVENT_QUEUE_NAME.to_owned(),
                reason: format!("Could not start the simulator event queue after repeated transient errors: {error}"),
            },
            shutdown,
            session_closed,
        )
        .await;
        return if matches!(outcome, EventSendOutcome::Sent) { RetryOutcome::LimitReached } else { RetryOutcome::Stopped };
    }

    *retries += 1;
    let delay = seed_retry_delay(*retries, retry_after);
    wait_for_retry(delay, shutdown, session_closed).await
}

async fn wait_for_retry(delay: Duration, shutdown: &watch::Receiver<bool>, session_closed: &watch::Receiver<bool>) -> RetryOutcome {
    tokio::select! {
        biased;
        _ = wait_for_watch_true(shutdown.clone()) => RetryOutcome::Stopped,
        _ = wait_for_session_close(session_closed.clone()) => RetryOutcome::Stopped,
        _ = time::sleep(delay) => RetryOutcome::Retry,
    }
}

async fn send_event_queue_error(
    events: &mpsc::Sender<EventEnvelope>,
    event: Event,
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> EventSendOutcome {
    if *shutdown.borrow() || *session_closed.borrow() {
        return EventSendOutcome::Shutdown;
    }

    tokio::select! {
        biased;
        _ = wait_for_watch_true(shutdown.clone()) => EventSendOutcome::Shutdown,
        _ = wait_for_session_close(session_closed.clone()) => EventSendOutcome::Shutdown,
        result = events.send(EventEnvelope::new(event)) => match result {
            Ok(()) => EventSendOutcome::Sent,
            Err(_) => EventSendOutcome::ReceiverClosed,
        },
    }
}

pub(crate) fn event_queue_retry_delay(error_count: u32) -> Duration {
    EVENT_QUEUE_RETRY_DELAY + EVENT_QUEUE_RETRY_INCREMENT * error_count
}

pub(crate) fn expected_empty_poll(status: StatusCode, elapsed: Duration) -> bool {
    elapsed >= MIN_EMPTY_POLL_DURATION
        && (status.as_u16() == LINDEN_HTTP_INTERNAL_ERROR
            || matches!(status, StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT))
}

pub(crate) fn seed_capability_retry_delay(retry_count: u32) -> Duration {
    Duration::from_secs(1 << retry_count.saturating_sub(1).min(3))
}

pub(crate) fn seed_retry_delay(retry_count: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.map(|delay| delay.min(MAX_SEED_RETRY_AFTER)).unwrap_or_else(|| seed_capability_retry_delay(retry_count))
}

#[cfg(test)]
mod retry_tests {
    use super::*;
    use std::{future::Future, net::SocketAddr};
    use tokio::task::yield_now;
    use tokio::time::timeout;

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);

    async fn close_while_pending<F>(future: F, session_closed: &watch::Sender<bool>) -> RetryOutcome
    where
        F: Future<Output = RetryOutcome>,
    {
        tokio::pin!(future);
        tokio::select! {
            biased;
            outcome = &mut future => return outcome,
            _ = yield_now() => {}
        }
        session_closed.send_replace(true);
        timeout(TEST_TIMEOUT, &mut future).await.expect("session close cancels retry")
    }

    #[tokio::test]
    async fn poll_backoff_stops_when_session_closes() {
        let (events, _received) = mpsc::channel(1);
        let (_shutdown, mut shutdown) = watch::channel(false);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let mut error_count = 0;

        assert_eq!(
            close_while_pending(
                retry_event_queue(
                    SimulatorId::new(1),
                    &mut error_count,
                    SocketAddr::from(([127, 0, 0, 1], 0)),
                    &events,
                    "poll failed".into(),
                    &mut shutdown,
                    &session_closed_receiver,
                ),
                &session_closed,
            )
            .await,
            RetryOutcome::Stopped
        );
    }

    #[tokio::test]
    async fn seed_backoff_stops_when_session_closes() {
        let (events, _received) = mpsc::channel(1);
        let (_shutdown, mut shutdown) = watch::channel(false);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let mut retries = 0;

        assert_eq!(
            close_while_pending(
                retry_seed_capability_lookup(
                    SimulatorId::new(1),
                    &mut retries,
                    SocketAddr::from(([127, 0, 0, 1], 0)),
                    &events,
                    "seed lookup failed".into(),
                    Some(Duration::from_secs(30)),
                    &mut shutdown,
                    &session_closed_receiver,
                ),
                &session_closed,
            )
            .await,
            RetryOutcome::Stopped
        );
    }

    #[tokio::test]
    async fn poll_limit_notification_stops_when_session_closes() {
        let (events, mut received) = mpsc::channel(1);
        events.try_send(EventEnvelope::new(Event::Failed("occupy event channel".into()))).expect("fill event channel");
        let (_shutdown, mut shutdown) = watch::channel(false);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let mut error_count = MAX_EVENT_QUEUE_ERRORS;

        assert_eq!(
            close_while_pending(
                retry_event_queue(
                    SimulatorId::new(1),
                    &mut error_count,
                    SocketAddr::from(([127, 0, 0, 1], 0)),
                    &events,
                    "poll failed".into(),
                    &mut shutdown,
                    &session_closed_receiver,
                ),
                &session_closed,
            )
            .await,
            RetryOutcome::Stopped
        );
        assert!(matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_))));
    }

    #[tokio::test]
    async fn seed_limit_notification_stops_when_session_closes() {
        let (events, mut received) = mpsc::channel(1);
        events.try_send(EventEnvelope::new(Event::Failed("occupy event channel".into()))).expect("fill event channel");
        let (_shutdown, mut shutdown) = watch::channel(false);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let mut retries = SEED_LOOKUP_RETRY_LIMIT;

        assert_eq!(
            close_while_pending(
                retry_seed_capability_lookup(
                    SimulatorId::new(1),
                    &mut retries,
                    SocketAddr::from(([127, 0, 0, 1], 0)),
                    &events,
                    "seed lookup failed".into(),
                    Some(Duration::from_secs(30)),
                    &mut shutdown,
                    &session_closed_receiver,
                ),
                &session_closed,
            )
            .await,
            RetryOutcome::Stopped
        );
        assert!(matches!(received.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(_))));
    }
}

#[cfg(test)]
mod retained_size_tests {
    use super::*;

    #[test]
    fn counts_owned_buffer_capacity() {
        let mut uri_text = String::with_capacity(4096);
        uri_text.push_str("uri");
        let uri_capacity = uri_text.capacity();
        let mut uri = Value::URI(URI::from(uri_text));
        assert_eq!(llsd_heap_size(&mut uri), uri_capacity);
        assert!(matches!(&uri, Value::URI(value) if value.as_str() == "uri"));

        let mut bytes = Vec::with_capacity(4096);
        bytes.extend_from_slice(b"bin");
        let binary_capacity = bytes.capacity();
        let mut binary = Value::Binary(Binary::from(bytes));
        assert_eq!(llsd_heap_size(&mut binary), binary_capacity);
        assert!(matches!(&binary, Value::Binary(value) if value.as_slice() == b"bin"));
    }
}

#[cfg(test)]
mod event_queue_batch_tests {
    use super::*;
    use tokio::{spawn, task::yield_now};

    #[tokio::test]
    async fn closed_session_or_shutdown_skips_batch_reservation() {
        for close_session in [false, true] {
            let budget = Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES));
            let (shutdown, shutdown_receiver) = watch::channel(false);
            let (session_closed, session_closed_receiver) = watch::channel(false);
            if close_session {
                session_closed.send_replace(true);
            } else {
                shutdown.send_replace(true);
            }

            assert!(acquire_event_batch_reservation(Arc::clone(&budget), &shutdown_receiver, &session_closed_receiver).await.is_none());
            assert_eq!(budget.available_permits(), MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES);
        }
    }

    #[tokio::test]
    async fn cancelled_batch_is_not_forwarded() {
        for close_session in [false, true] {
            let (events, mut received) = mpsc::channel(1);
            let (shutdown, shutdown_receiver) = watch::channel(false);
            let (session_closed, session_closed_receiver) = watch::channel(false);
            if close_session {
                session_closed.send_replace(true);
            } else {
                shutdown.send_replace(true);
            }
            let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
            let batch_reservation =
                queued_event_bytes.clone().acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32).await.expect("reserve a batch");
            let event_queue_permit = Arc::new(Semaphore::new(1)).acquire_owned().await.expect("reserve the queue");
            let batch = vec![QueuedCapabilityEvent { message: "cancelled".into(), body: Value::Undefined, retained_bytes: 1 }];

            assert!(matches!(
                send_event_queue_batch(
                    &events,
                    SimulatorId::new(1),
                    SocketAddr::from(([127, 0, 0, 1], 0)),
                    batch,
                    &shutdown_receiver,
                    &session_closed_receiver,
                    batch_reservation,
                    event_queue_permit,
                )
                .await,
                EventSendOutcome::Shutdown
            ));
            assert!(received.try_recv().is_err());
            assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
        }
    }

    #[tokio::test]
    async fn accepts_event_at_size_limit() {
        let (events, mut received) = mpsc::channel(1);
        let (_shutdown, shutdown_receiver) = watch::channel(false);
        let (_session_closed, session_closed_receiver) = watch::channel(false);
        let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
        let batch_reservation =
            queued_event_bytes.clone().acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32).await.expect("reserve a batch");
        let event_queue_permit = Arc::new(Semaphore::new(1)).acquire_owned().await.expect("reserve the queue");
        let batch = vec![QueuedCapabilityEvent {
            message: "limit".into(),
            body: Value::Undefined,
            retained_bytes: MAX_QUEUED_CAPABILITY_EVENT_BYTES,
        }];

        assert!(matches!(
            send_event_queue_batch(
                &events,
                SimulatorId::new(1),
                SocketAddr::from(([127, 0, 0, 1], 0)),
                batch,
                &shutdown_receiver,
                &session_closed_receiver,
                batch_reservation,
                event_queue_permit,
            )
            .await,
            EventSendOutcome::Sent
        ));
        assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES - MAX_QUEUED_CAPABILITY_EVENT_BYTES);
        assert!(matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::CapabilityEvent { .. })));
        assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
    }

    #[tokio::test]
    async fn close_cancels_forwarding() {
        let (events, mut received) = mpsc::channel(1);
        events.try_send(EventEnvelope::new(Event::Failed("occupied".into()))).expect("fill event channel");

        let (_shutdown, shutdown_receiver) = watch::channel(false);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let mut body = Value::Undefined;
        let retained_bytes = queued_event_retained_size("test".len(), &mut body);
        let batch = vec![QueuedCapabilityEvent { message: "test".into(), body, retained_bytes }];
        let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
        let batch_reservation =
            queued_event_bytes.clone().acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32).await.expect("reserve a batch");
        let event_queue_budget = Arc::new(Semaphore::new(1));
        let event_queue_permit = event_queue_budget.clone().acquire_owned().await.expect("acquire event queue permit");
        let task = spawn(async move {
            send_event_queue_batch(
                &events,
                SimulatorId::new(1),
                SocketAddr::from(([127, 0, 0, 1], 0)),
                batch,
                &shutdown_receiver,
                &session_closed_receiver,
                batch_reservation,
                event_queue_permit,
            )
            .await
        });

        yield_now().await;
        assert!(!task.is_finished());
        assert_eq!(event_queue_budget.available_permits(), 0);

        session_closed.send(true).expect("signal session close");

        assert!(matches!(task.await.expect("forwarding task"), EventSendOutcome::Shutdown));
        assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
        assert_eq!(event_queue_budget.available_permits(), 1);
        assert!(matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(_))));
    }

    #[tokio::test]
    async fn session_byte_budget_blocks_another_batch() {
        let queued_event_bytes = Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES));
        let mut reservations = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES);
        for _ in 0..MAX_CONCURRENT_EVENT_QUEUES {
            reservations.push(
                queued_event_bytes
                    .clone()
                    .acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32)
                    .await
                    .expect("reserve one queue batch"),
            );
        }
        assert_eq!(queued_event_bytes.available_permits(), 0);

        let (_shutdown, shutdown_receiver) = watch::channel(false);
        let (_session_closed, session_closed_receiver) = watch::channel(false);
        let shutdown = shutdown_receiver.clone();
        let session_closed = session_closed_receiver.clone();
        let byte_budget = Arc::clone(&queued_event_bytes);
        let pending = spawn(async move { acquire_event_batch_reservation(byte_budget, &shutdown, &session_closed).await });
        yield_now().await;
        assert!(!pending.is_finished(), "the session-wide budget must include every simulator queue");

        drop(reservations.pop());
        let permit = time::timeout(Duration::from_secs(1), pending)
            .await
            .expect("released batch capacity should unblock another queue")
            .expect("reservation task")
            .expect("session remains open");
        assert_eq!(queued_event_bytes.available_permits(), 0);
        drop(permit);
        drop(reservations);
        assert_eq!(queued_event_bytes.available_permits(), MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES);
    }

    #[tokio::test]
    async fn shutdown_cancels_batch_reservation() {
        let queued_event_bytes = Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES));
        let full = queued_event_bytes
            .clone()
            .acquire_many_owned(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES as u32)
            .await
            .expect("occupy the session budget");
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (_session_closed, session_closed_receiver) = watch::channel(false);
        let shutdown_receiver = shutdown_receiver.clone();
        let session_closed = session_closed_receiver.clone();
        let byte_budget = Arc::clone(&queued_event_bytes);
        let pending = spawn(async move { acquire_event_batch_reservation(byte_budget, &shutdown_receiver, &session_closed).await });
        yield_now().await;
        assert!(!pending.is_finished());

        shutdown.send_replace(true);
        assert!(
            time::timeout(Duration::from_secs(1), pending)
                .await
                .expect("shutdown should cancel reservation")
                .expect("reservation task")
                .is_none()
        );
        drop(full);
        assert_eq!(queued_event_bytes.available_permits(), MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES);
    }

    #[tokio::test]
    async fn session_close_cancels_batch_reservation() {
        let queued_event_bytes = Arc::new(Semaphore::new(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES));
        let full = queued_event_bytes
            .clone()
            .acquire_many_owned(MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES as u32)
            .await
            .expect("occupy the session budget");
        let (_shutdown, shutdown_receiver) = watch::channel(false);
        let (session_closed, session_closed_receiver) = watch::channel(false);
        let shutdown_receiver = shutdown_receiver.clone();
        let session_closed_receiver = session_closed_receiver.clone();
        let byte_budget = Arc::clone(&queued_event_bytes);
        let pending =
            spawn(async move { acquire_event_batch_reservation(byte_budget, &shutdown_receiver, &session_closed_receiver).await });
        yield_now().await;
        assert!(!pending.is_finished());

        session_closed.send_replace(true);
        assert!(
            time::timeout(Duration::from_secs(1), pending)
                .await
                .expect("session close should cancel reservation")
                .expect("reservation task")
                .is_none()
        );
        drop(full);
        assert_eq!(queued_event_bytes.available_permits(), MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES);
    }

    #[tokio::test]
    async fn receiver_close_releases_batch_bytes() {
        let (events, received) = mpsc::channel(1);
        events.try_send(EventEnvelope::new(Event::Failed("occupy the channel".into()))).expect("fill event channel");
        let (_shutdown, shutdown_receiver) = watch::channel(false);
        let (_session_closed, session_closed_receiver) = watch::channel(false);
        let mut body = Value::Undefined;
        let retained_bytes = queued_event_retained_size("test".len(), &mut body);
        let batch = vec![QueuedCapabilityEvent { message: "test".into(), body, retained_bytes }];
        let queued_event_bytes = Arc::new(Semaphore::new(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES));
        let batch_reservation =
            queued_event_bytes.clone().acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32).await.expect("reserve a batch");
        let event_queue_permit = Arc::new(Semaphore::new(1)).acquire_owned().await.expect("reserve the queue");
        let task = spawn(async move {
            send_event_queue_batch(
                &events,
                SimulatorId::new(1),
                SocketAddr::from(([127, 0, 0, 1], 0)),
                batch,
                &shutdown_receiver,
                &session_closed_receiver,
                batch_reservation,
                event_queue_permit,
            )
            .await
        });
        yield_now().await;
        drop(received);

        assert!(matches!(task.await.expect("forwarding task"), EventSendOutcome::ReceiverClosed));
        assert_eq!(queued_event_bytes.available_permits(), MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES);
    }
}
