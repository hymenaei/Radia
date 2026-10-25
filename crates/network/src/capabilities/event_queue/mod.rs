mod decode;

#[cfg(test)]
use std::slice::from_ref;
use std::{
    mem,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use llsd::{Map, Value, xml};
use reqwest::{StatusCode, Url};
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
    time,
};

use self::decode::{capability_event_batch_retained_size, decode_admitted_event_batch};
#[cfg(test)]
use self::decode::{decode_event_batch, llsd_heap_size, queued_event_retained_size};
#[cfg(test)]
use super::budget::MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES;
use super::{
    budget::{
        MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES, MAX_EVENT_QUEUE_RESPONSE_BYTES, MAX_SEED_CAPABILITY_RESPONSE_BYTES,
        spawn_blocking_with_permit,
    },
    request::{decode_seed_capabilities, is_transient_seed_lookup, request_capabilities_response, seed_lookup_retry_after},
};
use crate::{
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource},
    event::EventRetention,
    event::{CapabilityEvent, Event, EventEnvelope, EventQueueBatchOrder, EventSendOutcome, SimulatorId},
    http::{self, Response},
};

const EVENT_QUEUE_RETRY_DELAY: Duration = Duration::from_secs(1);
const EVENT_QUEUE_RETRY_INCREMENT: Duration = Duration::from_secs(3);
const MIN_EMPTY_POLL_DURATION: Duration = Duration::from_secs(10);
const EVENT_QUEUE_ADMISSION_RETRY: Duration = Duration::from_secs(1);
const MAX_EVENT_QUEUE_RETRY_AFTER: Duration = Duration::from_secs(30);
const MAX_EVENT_QUEUE_ERRORS: u32 = 15;
const SEED_LOOKUP_RETRY_LIMIT: u32 = 5;
const MAX_SEED_RETRY_AFTER: Duration = Duration::from_secs(30);
pub(crate) const EVENT_QUEUE_NAME: &str = "EventQueueGet";
const LINDEN_HTTP_INTERNAL_ERROR: u16 = 499;
const MAX_QUEUED_CAPABILITY_EVENT_BYTES: usize = 256 * 1024;

static NEXT_EVENT_QUEUE_INCARNATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EventQueueExit {
    Stopped,
    StoppedWithCoverageLoss,
    Unavailable,
    RetryLimitReached,
    TaskFailed,
}

pub(crate) struct EventQueueTask {
    task: JoinHandle<()>,
    completion: Completion,
    role: EventQueueRoleHandle,
}

impl EventQueueTask {
    pub(super) fn new(task: JoinHandle<()>, completion: oneshot::Receiver<EventQueueExit>, role: EventQueueRoleHandle) -> Self {
        Self { task, completion: Completion::Pending(completion), role }
    }

    pub(crate) fn role_handle(&self) -> EventQueueRoleHandle {
        self.role.clone()
    }

    pub(crate) async fn completion(&mut self) -> EventQueueExit {
        match &mut self.completion {
            Completion::Finished(exit) => *exit,
            Completion::Pending(completion) => {
                let exit = completion.await.unwrap_or(EventQueueExit::TaskFailed);
                self.completion = Completion::Finished(exit);
                exit
            }
        }
    }

    pub(crate) async fn wait(mut self) -> EventQueueExit {
        let exit = self.completion().await;
        if let Err(error) = (&mut self.task).await {
            tracing::error!(%error, "EventQueueGet task failed");
        }
        exit
    }
}

impl Drop for EventQueueTask {
    fn drop(&mut self) {
        self.task.abort();
    }
}

enum Completion {
    Pending(oneshot::Receiver<EventQueueExit>),
    Finished(EventQueueExit),
}

#[derive(Clone)]
pub(crate) struct EventQueueRoleHandle {
    primary: watch::Sender<bool>,
}

impl EventQueueRoleHandle {
    pub(crate) fn channel(is_primary: bool) -> (Self, watch::Receiver<bool>) {
        let (primary, receiver) = watch::channel(is_primary);
        (Self { primary }, receiver)
    }

    pub(crate) fn set_primary(&self, primary: bool) {
        if *self.primary.borrow() != primary {
            self.primary.send_replace(primary);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RetryOutcome {
    Retry,
    Stopped,
    LimitReached,
}

pub(super) struct EventQueueContext {
    pub(super) simulator_id: SimulatorId,
    pub(super) source: SocketAddr,
    pub(super) seed: Url,
    pub(super) http: http::Client,
    pub(super) events: mpsc::Sender<EventEnvelope>,
    pub(super) ordinary_requests: Arc<Semaphore>,
    pub(super) primary_event_queues: Arc<Semaphore>,
    pub(super) neighbor_event_queues: Arc<Semaphore>,
    pub(super) queued_event_bytes: Arc<Semaphore>,
    pub(super) coverage_losses: CoverageLossLedger,
}

#[derive(Clone)]
pub(super) struct Stop {
    shutdown: watch::Receiver<bool>,
    session_closed: watch::Receiver<bool>,
}

impl Stop {
    pub(super) fn new(shutdown: watch::Receiver<bool>, session_closed: watch::Receiver<bool>) -> Self {
        Self { shutdown, session_closed }
    }

    fn is_stopped(&self) -> bool {
        *self.shutdown.borrow() || *self.session_closed.borrow()
    }

    async fn stopped(&self) {
        let mut shutdown = self.shutdown.clone();
        let mut session_closed = self.session_closed.clone();
        tokio::select! {
            biased;
            _ = shutdown.wait_for(|stopped| *stopped) => {}
            _ = session_closed.wait_for(|closed| *closed) => {}
        }
    }
}

struct EventQueueScope {
    ledger: CoverageLossLedger,
    simulator_id: SimulatorId,
    source: CoverageSource,
}

impl Drop for EventQueueScope {
    fn drop(&mut self) {
        self.ledger.retire(self.simulator_id, self.source);
    }
}

struct PendingEventQueueLoss {
    ledger: CoverageLossLedger,
    simulator_id: SimulatorId,
    source: CoverageSource,
    notifier: EventQueueLossNotifier,
    state: PendingLossState,
}

impl PendingEventQueueLoss {
    fn new(ledger: CoverageLossLedger, simulator_id: SimulatorId, source: CoverageSource, notifier: EventQueueLossNotifier) -> Self {
        Self { ledger, simulator_id, source, notifier, state: PendingLossState::Pending }
    }

    fn record(&mut self) {
        if matches!(self.state, PendingLossState::Pending) {
            self.ledger.record(self.simulator_id, self.source, CoverageLossReason::EventQueueAdmission);
            self.state = PendingLossState::Recorded { reason: None };
        }
    }

    async fn notify(&mut self, reason: String) -> EventSendOutcome {
        self.record();
        if let PendingLossState::Recorded { reason: stored } = &mut self.state {
            *stored = Some(reason.clone());
        }
        self.notifier.notify(reason).await
    }

    fn commit(&mut self) {
        if matches!(self.state, PendingLossState::Pending) {
            self.state = PendingLossState::Committed;
        }
    }
}

impl Drop for PendingEventQueueLoss {
    fn drop(&mut self) {
        self.record();
        if let PendingLossState::Recorded { reason } = mem::replace(&mut self.state, PendingLossState::Committed)
            && !self.notifier.was_notified()
        {
            let notifier = self.notifier.clone();
            let reason =
                reason.unwrap_or_else(|| "EventQueueGet polling was canceled before its response could be admitted safely.".into());
            if let Ok(runtime) = Handle::try_current() {
                runtime.spawn(async move {
                    let _ = notifier.notify(reason).await;
                });
            }
        }
    }
}

enum PendingLossState {
    Pending,
    Committed,
    Recorded { reason: Option<String> },
}

#[derive(Clone)]
struct EventQueueLossNotifier {
    events: mpsc::Sender<EventEnvelope>,
    simulator_id: SimulatorId,
    source: SocketAddr,
    stop: Stop,
    notified: Arc<AtomicBool>,
}

impl EventQueueLossNotifier {
    fn new(events: mpsc::Sender<EventEnvelope>, simulator_id: SimulatorId, source: SocketAddr, stop: Stop) -> Self {
        Self { events, simulator_id, source, stop, notified: Arc::new(AtomicBool::new(false)) }
    }

    async fn notify(&self, reason: String) -> EventSendOutcome {
        if self.notified.load(Ordering::Acquire) {
            return EventSendOutcome::Sent;
        }
        if self.stop.is_stopped() {
            return EventSendOutcome::Shutdown;
        }

        let permit = tokio::select! {
            biased;
            _ = self.stop.stopped() => return EventSendOutcome::Shutdown,
            result = self.events.reserve() => match result {
                Ok(permit) => permit,
                Err(_) => return EventSendOutcome::ReceiverClosed,
            },
        };
        if self.notified.swap(true, Ordering::AcqRel) {
            drop(permit);
            return EventSendOutcome::Sent;
        }

        permit.send(EventEnvelope::new(Event::EventQueueCoverageLost { simulator_id: self.simulator_id, source: self.source, reason }));
        EventSendOutcome::Sent
    }

    fn was_notified(&self) -> bool {
        self.notified.load(Ordering::Acquire)
    }
}

struct PollResources {
    event_queue: Option<OwnedSemaphorePermit>,
    batch: Option<OwnedSemaphorePermit>,
}

async fn acquire_budget_permit(budget: Arc<Semaphore>, stop: &Stop) -> Option<OwnedSemaphorePermit> {
    if stop.is_stopped() {
        return None;
    }

    tokio::select! {
        biased;
        _ = stop.stopped() => None,
        permit = budget.acquire_owned() => permit.ok(),
    }
}

async fn acquire_event_queue_permit(
    primary_budget: Arc<Semaphore>,
    neighbor_budget: Arc<Semaphore>,
    primary: &mut watch::Receiver<bool>,
    stop: &Stop,
) -> Option<OwnedSemaphorePermit> {
    loop {
        if stop.is_stopped() {
            return None;
        }
        let budget = if *primary.borrow() { Arc::clone(&primary_budget) } else { Arc::clone(&neighbor_budget) };
        tokio::select! {
            biased;
            _ = stop.stopped() => return None,
            changed = primary.changed() => {
                if changed.is_err() {
                    return None;
                }
            }
            permit = budget.acquire_owned() => return permit.ok(),
        }
    }
}

fn acquire_event_batch_reservation(budget: Arc<Semaphore>, stop: &Stop) -> Option<OwnedSemaphorePermit> {
    if stop.is_stopped() {
        return None;
    }
    budget.try_acquire_many_owned(MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES as u32).ok()
}

async fn send_event_queue_batch(
    events: &mpsc::Sender<EventEnvelope>,
    simulator_id: SimulatorId,
    source: SocketAddr,
    coverage_source: CoverageSource,
    batch_order: u64,
    batch: Vec<CapabilityEvent>,
    stop: &Stop,
    mut batch_reservation: OwnedSemaphorePermit,
    coverage_losses: &CoverageLossLedger,
    pending_loss: &mut PendingEventQueueLoss,
) -> EventSendOutcome {
    if batch.iter().any(|event| event.retained_bytes > MAX_QUEUED_CAPABILITY_EVENT_BYTES) {
        pending_loss.record();
        return EventSendOutcome::ReceiverClosed;
    }
    let retained_bytes = capability_event_batch_retained_size(&batch, batch.capacity());
    if retained_bytes > MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES {
        pending_loss.record();
        return EventSendOutcome::ReceiverClosed;
    }
    let Some(permit) = batch_reservation.split(retained_bytes) else {
        pending_loss.record();
        return EventSendOutcome::ReceiverClosed;
    };
    drop(batch_reservation);

    let retention = EventRetention::from_permits(vec![permit]);

    let CoverageSource::EventQueue { incarnation } = coverage_source else { unreachable!("EventQueue batches use an EventQueue source") };
    let event = Event::CapabilityEventBatch {
        simulator_id,
        source,
        order: EventQueueBatchOrder { incarnation, ordinal: batch_order },
        events: batch,
    };
    let permit = match reserve_event_queue_slot(events, stop).await {
        Ok(permit) => permit,
        Err(outcome) => {
            pending_loss.record();
            return outcome;
        }
    };
    let watermark = coverage_losses.watermark(simulator_id, coverage_source);
    permit.send(EventEnvelope::with_retention_and_coverage(event, retention, watermark));
    pending_loss.commit();
    EventSendOutcome::Sent
}

async fn reserve_event_queue_slot<'a>(
    events: &'a mpsc::Sender<EventEnvelope>,
    stop: &Stop,
) -> Result<mpsc::Permit<'a, EventEnvelope>, EventSendOutcome> {
    if stop.is_stopped() {
        return Err(EventSendOutcome::Shutdown);
    }
    tokio::select! {
        biased;
        _ = stop.stopped() => Err(EventSendOutcome::Shutdown),
        result = events.reserve() => result.map_err(|_| EventSendOutcome::ReceiverClosed),
    }
}

fn allocate_event_queue_incarnation() -> Option<u64> {
    NEXT_EVENT_QUEUE_INCARNATION.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1)).ok()
}

async fn discover_capability(context: &EventQueueContext, stop: &Stop) -> Result<Url, EventQueueExit> {
    let names = [EVENT_QUEUE_NAME.to_owned()];
    let mut retries = 0;
    loop {
        let Some(request_permit) = acquire_budget_permit(Arc::clone(&context.ordinary_requests), stop).await else {
            return Err(EventQueueExit::Stopped);
        };
        let requested = tokio::select! {
            biased;
            _ = stop.stopped() => return Err(EventQueueExit::Stopped),
            result = request_capabilities_response(
                &context.http,
                &context.seed,
                &names,
                MAX_SEED_CAPABILITY_RESPONSE_BYTES,
            ) => result,
        };
        let requested = match requested {
            Ok(response) => {
                let decode_names = names.clone();
                tokio::select! {
                    biased;
                    _ = stop.stopped() => return Err(EventQueueExit::Stopped),
                    decoded = spawn_blocking_with_permit(request_permit, move || decode_seed_capabilities(response, &decode_names)) => {
                        match decoded {
                            Ok((decoded, _permit)) => decoded,
                            Err(error) => Err(crate::error::Error::Capability(format!("Capability decoder task failed: {error}"))),
                        }
                    }
                }
            }
            Err(error) => {
                drop(request_permit);
                Err(error)
            }
        };
        match requested {
            Ok(mut granted) => {
                if let Some(capability) = granted.remove(EVENT_QUEUE_NAME) {
                    return Ok(capability);
                }
                let outcome = send_event_queue_error(
                    &context.events,
                    Event::CapabilityError {
                        simulator_id: context.simulator_id,
                        source: context.source,
                        capability: EVENT_QUEUE_NAME.to_owned(),
                        reason: format!("Second Life did not provide the {EVENT_QUEUE_NAME} capability."),
                    },
                    stop,
                )
                .await;
                return Err(exit_after_send(outcome, EventQueueExit::Unavailable));
            }
            Err(error) if !is_transient_seed_lookup(&error) => {
                let message = error.to_string();
                let outcome = send_event_queue_error(
                    &context.events,
                    Event::CapabilityError {
                        simulator_id: context.simulator_id,
                        source: context.source,
                        capability: EVENT_QUEUE_NAME.to_owned(),
                        reason: format!("Could not start the simulator event queue: {message}"),
                    },
                    stop,
                )
                .await;
                return Err(exit_after_send(outcome, EventQueueExit::Unavailable));
            }
            Err(error) => {
                let message = error.to_string();
                let retry_after = seed_lookup_retry_after(&error);
                match retry_seed_capability_lookup(context, &mut retries, message, retry_after, stop).await {
                    RetryOutcome::Retry => {}
                    RetryOutcome::Stopped => return Err(EventQueueExit::Stopped),
                    RetryOutcome::LimitReached => return Err(EventQueueExit::Unavailable),
                }
            }
        }
    }
}

fn exit_after_send(outcome: EventSendOutcome, sent: EventQueueExit) -> EventQueueExit {
    if matches!(outcome, EventSendOutcome::Sent) { sent } else { EventQueueExit::Stopped }
}

async fn notify_loss_then(pending_loss: &mut PendingEventQueueLoss, reason: String, sent: EventQueueExit) -> EventQueueExit {
    exit_after_send(pending_loss.notify(reason).await, sent)
}

#[tracing::instrument(name = "capabilities.event_queue", level = "debug", skip_all, fields(simulator = context.simulator_id.get()), ret)]
pub(super) async fn run_event_queue(context: EventQueueContext, stop: Stop, mut primary: watch::Receiver<bool>) -> EventQueueExit {
    if stop.is_stopped() {
        return EventQueueExit::Stopped;
    }

    let Some(incarnation) = allocate_event_queue_incarnation() else {
        return EventQueueExit::TaskFailed;
    };
    let coverage_source = CoverageSource::EventQueue { incarnation };
    let _scope = EventQueueScope { ledger: context.coverage_losses.clone(), simulator_id: context.simulator_id, source: coverage_source };
    let loss_notifier = EventQueueLossNotifier::new(context.events.clone(), context.simulator_id, context.source, stop.clone());
    let event_capability = match discover_capability(&context, &stop).await {
        Ok(capability) => capability,
        Err(exit) => return exit,
    };
    let EventQueueContext {
        simulator_id,
        source,
        http,
        events,
        primary_event_queues,
        neighbor_event_queues,
        queued_event_bytes,
        coverage_losses,
        ..
    } = context;

    let mut acknowledge = Value::Undefined;
    let mut error_count = 0;
    let mut batch_order = 0_u64;

    loop {
        if stop.is_stopped() {
            return EventQueueExit::Stopped;
        }

        let Some(event_queue_slot) =
            acquire_event_queue_permit(Arc::clone(&primary_event_queues), Arc::clone(&neighbor_event_queues), &mut primary, &stop).await
        else {
            return EventQueueExit::Stopped;
        };
        let mut poll = PollResources { event_queue: Some(event_queue_slot), batch: None };
        let Some(batch_reservation) = acquire_event_batch_reservation(Arc::clone(&queued_event_bytes), &stop) else {
            drop(poll);
            if matches!(wait_for_retry(EVENT_QUEUE_ADMISSION_RETRY, &stop).await, RetryOutcome::Retry) {
                continue;
            }
            return EventQueueExit::Stopped;
        };
        poll.batch = Some(batch_reservation);
        let mut pending_loss = PendingEventQueueLoss::new(coverage_losses.clone(), simulator_id, coverage_source, loss_notifier.clone());
        let response = tokio::select! {
            biased;
            _ = stop.stopped() => {
                pending_loss.commit();
                return EventQueueExit::Stopped;
            },
            changed = primary.changed() => {
                pending_loss.record();
                if changed.is_err() {
                    return EventQueueExit::Stopped;
                }
                continue;
            },
            result = post_event_queue(&http, &event_capability, &acknowledge, MAX_EVENT_QUEUE_RESPONSE_BYTES) => result,
        };

        match response {
            Ok(response) if response.status == StatusCode::NOT_FOUND => {
                pending_loss.record();
                let status_code = response.status;
                drop(poll);
                let reason = format!("EventQueueGet returned HTTP {status_code}; polling stopped.");
                if !matches!(pending_loss.notify(reason.clone()).await, EventSendOutcome::Sent) {
                    return EventQueueExit::Stopped;
                }
                let outcome = send_event_queue_error(
                    &events,
                    Event::CapabilityError { simulator_id, source, capability: EVENT_QUEUE_NAME.to_owned(), reason },
                    &stop,
                )
                .await;
                return exit_after_send(outcome, EventQueueExit::StoppedWithCoverageLoss);
            }
            Ok(response) if expected_empty_poll(response.status, response.elapsed) => {
                pending_loss.commit();
                error_count = 0;
            }
            Ok(response) if response.status.is_success() => {
                let Some(event_queue_permit) = poll.event_queue.take() else {
                    return EventQueueExit::TaskFailed;
                };
                let decoded = decode_admitted_event_batch(response.body, event_queue_permit).await;
                match decoded {
                    Ok((id, batch, event_queue_permit)) => {
                        poll.event_queue = Some(event_queue_permit);
                        error_count = 0;
                        if Value::Integer(id) == acknowledge {
                            pending_loss.commit();
                            drop(batch);
                            continue;
                        }
                        let Some(next_batch_order) = batch_order.checked_add(1) else {
                            pending_loss.record();
                            drop(batch);
                            return EventQueueExit::Unavailable;
                        };
                        let Some(batch_reservation) = poll.batch.take() else {
                            return EventQueueExit::TaskFailed;
                        };
                        let outcome = send_event_queue_batch(
                            &events,
                            simulator_id,
                            source,
                            coverage_source,
                            batch_order,
                            batch,
                            &stop,
                            batch_reservation,
                            &coverage_losses,
                            &mut pending_loss,
                        )
                        .await;
                        match outcome {
                            EventSendOutcome::Sent => {
                                acknowledge = Value::Integer(id);
                                batch_order = next_batch_order;
                            }
                            EventSendOutcome::Shutdown => return EventQueueExit::Stopped,
                            EventSendOutcome::ReceiverClosed => {
                                let reason =
                                    String::from("EventQueueGet batch could not be delivered in full; some events may be missing.");
                                return notify_loss_then(&mut pending_loss, reason, EventQueueExit::StoppedWithCoverageLoss).await;
                            }
                        }
                    }
                    Err(error) => {
                        let error = error.to_string();
                        pending_loss.record();
                        drop(poll);
                        return notify_loss_then(&mut pending_loss, error, EventQueueExit::StoppedWithCoverageLoss).await;
                    }
                }
            }
            Ok(response) => {
                let status_code = response.status;
                let retry_after = response.retry_after();
                drop(poll);
                let error = format!("EventQueueGet returned HTTP {status_code}.");
                if !retryable_event_queue_error(status_code) {
                    pending_loss.record();
                    return notify_loss_then(&mut pending_loss, error, EventQueueExit::StoppedWithCoverageLoss).await;
                }
                match retry_event_queue_with_hint(
                    simulator_id,
                    &mut error_count,
                    source,
                    &events,
                    error.clone(),
                    retry_after,
                    &stop,
                    &mut pending_loss,
                )
                .await
                {
                    RetryOutcome::Retry => {}
                    RetryOutcome::Stopped => return EventQueueExit::Stopped,
                    RetryOutcome::LimitReached => {
                        return notify_loss_then(&mut pending_loss, error, EventQueueExit::RetryLimitReached).await;
                    }
                }
            }
            Err(error)
                if error.is_timeout_after(MIN_EMPTY_POLL_DURATION)
                    || error.is_transport_without_response_after(MIN_EMPTY_POLL_DURATION) =>
            {
                pending_loss.commit();
                error_count = 0;
            }
            Err(error) if error.is_resource_exhausted() => {
                pending_loss.record();
                drop(poll);
                let reason = format!("EventQueueGet response could not be admitted: {error}");
                return notify_loss_then(&mut pending_loss, reason, EventQueueExit::StoppedWithCoverageLoss).await;
            }
            Err(error) if error.is_transient() => {
                let retry_after = error.retry_after;
                drop(poll);
                let error = format!("EventQueueGet request failed: {error}");
                match retry_event_queue_with_hint(
                    simulator_id,
                    &mut error_count,
                    source,
                    &events,
                    error.clone(),
                    retry_after,
                    &stop,
                    &mut pending_loss,
                )
                .await
                {
                    RetryOutcome::Retry => {}
                    RetryOutcome::Stopped => return EventQueueExit::Stopped,
                    RetryOutcome::LimitReached => {
                        return notify_loss_then(&mut pending_loss, error, EventQueueExit::RetryLimitReached).await;
                    }
                }
            }
            Err(error) => {
                pending_loss.record();
                drop(poll);
                let error = format!("EventQueueGet request failed: {error}");
                return notify_loss_then(&mut pending_loss, error, EventQueueExit::StoppedWithCoverageLoss).await;
            }
        }
    }
}

async fn post_event_queue(
    client: &http::Client,
    url: &Url,
    acknowledge: &Value,
    max_response_bytes: usize,
) -> Result<Response, http::Error> {
    let payload = Value::Map(Map::from([("ack".to_owned(), acknowledge.clone()), ("done".to_owned(), Value::Boolean(false))]));
    let body = xml::to_string(&payload)
        .map_err(|error| http::Error::resource_exhausted(format!("Could not encode an EventQueueGet request: {error}")))?
        .into_bytes();
    client.post("event_queue", url, "application/llsd+xml", Some("application/llsd+xml"), body, max_response_bytes).await
}

async fn retry_event_queue_with_hint(
    simulator_id: SimulatorId,
    error_count: &mut u32,
    source: SocketAddr,
    events: &mpsc::Sender<EventEnvelope>,
    error: String,
    retry_after: Option<Duration>,
    stop: &Stop,
    pending_loss: &mut PendingEventQueueLoss,
) -> RetryOutcome {
    if stop.is_stopped() {
        pending_loss.commit();
        return RetryOutcome::Stopped;
    }

    if *error_count >= MAX_EVENT_QUEUE_ERRORS {
        pending_loss.record();
        let outcome = send_event_queue_error(
            events,
            Event::CapabilityError {
                simulator_id,
                source,
                capability: EVENT_QUEUE_NAME.to_owned(),
                reason: format!("Event queue stopped after repeated errors: {error}"),
            },
            stop,
        )
        .await;
        return if matches!(outcome, EventSendOutcome::Sent) { RetryOutcome::LimitReached } else { RetryOutcome::Stopped };
    }

    pending_loss.commit();
    let delay = event_queue_retry_delay_with_hint(*error_count, retry_after);
    tracing::debug!(attempt = *error_count, delay_ms = delay.as_millis(), "retrying event queue poll");
    *error_count += 1;
    wait_for_retry(delay, stop).await
}

async fn retry_seed_capability_lookup(
    context: &EventQueueContext,
    retries: &mut u32,
    error: String,
    retry_after: Option<Duration>,
    stop: &Stop,
) -> RetryOutcome {
    if stop.is_stopped() {
        return RetryOutcome::Stopped;
    }

    if *retries >= SEED_LOOKUP_RETRY_LIMIT {
        let outcome = send_event_queue_error(
            &context.events,
            Event::CapabilityError {
                simulator_id: context.simulator_id,
                source: context.source,
                capability: EVENT_QUEUE_NAME.to_owned(),
                reason: format!("Could not start the simulator event queue after repeated transient errors: {error}"),
            },
            stop,
        )
        .await;
        return if matches!(outcome, EventSendOutcome::Sent) { RetryOutcome::LimitReached } else { RetryOutcome::Stopped };
    }

    *retries += 1;
    let delay = seed_retry_delay(*retries, retry_after);
    tracing::debug!(attempt = *retries, delay_ms = delay.as_millis(), "retrying event queue discovery");
    wait_for_retry(delay, stop).await
}

async fn wait_for_retry(delay: Duration, stop: &Stop) -> RetryOutcome {
    tokio::select! {
        biased;
        _ = stop.stopped() => RetryOutcome::Stopped,
        _ = time::sleep(delay) => RetryOutcome::Retry,
    }
}

async fn send_event_queue_error(events: &mpsc::Sender<EventEnvelope>, event: Event, stop: &Stop) -> EventSendOutcome {
    if stop.is_stopped() {
        return EventSendOutcome::Shutdown;
    }

    tokio::select! {
        biased;
        _ = stop.stopped() => EventSendOutcome::Shutdown,
        result = events.send(EventEnvelope::new(event)) => match result {
            Ok(()) => EventSendOutcome::Sent,
            Err(_) => EventSendOutcome::ReceiverClosed,
        },
    }
}

fn event_queue_retry_delay(error_count: u32) -> Duration {
    EVENT_QUEUE_RETRY_DELAY + EVENT_QUEUE_RETRY_INCREMENT * error_count
}

fn event_queue_retry_delay_with_hint(error_count: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.map(|delay| delay.min(MAX_EVENT_QUEUE_RETRY_AFTER)).unwrap_or_else(|| event_queue_retry_delay(error_count))
}

fn expected_empty_poll(status: StatusCode, elapsed: Duration) -> bool {
    elapsed >= MIN_EMPTY_POLL_DURATION
        && (status.as_u16() == LINDEN_HTTP_INTERNAL_ERROR
            || matches!(status, StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT))
}

fn retryable_event_queue_error(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status.as_u16() == LINDEN_HTTP_INTERNAL_ERROR
        || matches!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR | StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
        )
}

fn seed_capability_retry_delay(retry_count: u32) -> Duration {
    Duration::from_secs(1 << retry_count.saturating_sub(1).min(3))
}

fn seed_retry_delay(retry_count: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.map(|delay| delay.min(MAX_SEED_RETRY_AFTER)).unwrap_or_else(|| seed_capability_retry_delay(retry_count))
}

#[cfg(test)]
mod tests;
