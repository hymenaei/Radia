#[cfg(test)]
use std::slice::from_ref;
#[cfg(test)]
use std::sync;
use std::{
    fmt, mem,
    net::SocketAddr,
    str::from_utf8,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use llsd::{Binary, Map, Uri, Value, xml};
use reqwest::{StatusCode, Url};
use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, Semaphore, mpsc, oneshot, watch},
    task::{JoinError, JoinHandle},
    time,
};

#[cfg(test)]
use super::budget::{MAX_CONCURRENT_EVENT_QUEUES, MAX_SESSION_QUEUED_CAPABILITY_EVENT_BYTES};
use super::{
    budget::{
        LLSD_SCAN_WORKING_BYTES, MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES, MAX_EVENT_QUEUE_RESPONSE_BYTES, MAX_SEED_CAPABILITY_RESPONSE_BYTES,
        spawn_blocking_with_permit,
    },
    request::{
        LookupError, admit_and_decode_seed_capabilities, capability_llsd_limits, decode_llsd_xml, request_capabilities_response,
        url_retained_memory_estimate, wait_for_session_close,
    },
};
use crate::{
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource},
    event::{CapabilityEvent, Event, EventEnvelope, EventQueueBatchOrder, EventSendOutcome, SimulatorId},
    http::{self, Error, Response, ResponseMemoryPermit},
    simulator::EventRetention,
    status::NetworkStatusHandle,
};

const EVENT_QUEUE_RETRY_DELAY: Duration = Duration::from_secs(1);
const EVENT_QUEUE_RETRY_INCREMENT: Duration = Duration::from_secs(3);
const MIN_EMPTY_POLL_DURATION: Duration = Duration::from_secs(10);
const EVENT_QUEUE_ADMISSION_RETRY: Duration = Duration::from_secs(1);
const MAX_EVENT_QUEUE_RETRY_AFTER: Duration = Duration::from_secs(30);
pub(crate) const MAX_EVENT_QUEUE_ERRORS: u32 = 15;
const SEED_LOOKUP_RETRY_LIMIT: u32 = 5;
const MAX_SEED_RETRY_AFTER: Duration = Duration::from_secs(30);
pub(crate) const EVENT_QUEUE_NAME: &str = "EventQueueGet";
const LINDEN_HTTP_INTERNAL_ERROR: u16 = 499;
pub(crate) const MAX_QUEUED_CAPABILITY_EVENT_BYTES: usize = 256 * 1024;
const MAX_EVENT_QUEUE_BATCH_EVENTS: usize = 1024;

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
        let _ = (&mut self.task).await;
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
pub(crate) enum RetryOutcome {
    Retry,
    Stopped,
    LimitReached,
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
    shutdown: watch::Receiver<bool>,
    session_closed: watch::Receiver<bool>,
    notified: Arc<AtomicBool>,
}

impl EventQueueLossNotifier {
    fn new(
        events: mpsc::Sender<EventEnvelope>,
        simulator_id: SimulatorId,
        source: SocketAddr,
        shutdown: watch::Receiver<bool>,
        session_closed: watch::Receiver<bool>,
    ) -> Self {
        Self { events, simulator_id, source, shutdown, session_closed, notified: Arc::new(AtomicBool::new(false)) }
    }

    async fn notify(&self, reason: String) -> EventSendOutcome {
        if self.notified.load(Ordering::Acquire) {
            return EventSendOutcome::Sent;
        }
        if *self.shutdown.borrow() || *self.session_closed.borrow() {
            return EventSendOutcome::Shutdown;
        }

        let permit = tokio::select! {
            biased;
            _ = wait_for_watch_true(self.shutdown.clone()) => return EventSendOutcome::Shutdown,
            _ = wait_for_session_close(self.session_closed.clone()) => return EventSendOutcome::Shutdown,
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

#[derive(Debug)]
enum BatchDecodeError {
    Preflight(xml::BoundError),
    Resource(Error),
    Decode(String),
    Task(JoinError),
}

impl fmt::Display for BatchDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preflight(error) => write!(f, "EventQueueGet response allocation preflight failed: {error}"),
            Self::Resource(error) => write!(f, "EventQueueGet decode memory is unavailable: {error}"),
            Self::Decode(error) => f.write_str(error),
            Self::Task(error) => write!(f, "EventQueueGet decoder task failed: {error}"),
        }
    }
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

async fn acquire_event_queue_permit(
    primary_budget: Arc<Semaphore>,
    neighbor_budget: Arc<Semaphore>,
    primary: &mut watch::Receiver<bool>,
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> Option<OwnedSemaphorePermit> {
    loop {
        if *shutdown.borrow() || *session_closed.borrow() {
            return None;
        }
        let budget = if *primary.borrow() { Arc::clone(&primary_budget) } else { Arc::clone(&neighbor_budget) };
        tokio::select! {
            biased;
            _ = wait_for_watch_true(shutdown.clone()) => return None,
            _ = wait_for_session_close(session_closed.clone()) => return None,
            changed = primary.changed() => {
                if changed.is_err() {
                    return None;
                }
            }
            permit = budget.acquire_owned() => return permit.ok(),
        }
    }
}

fn acquire_event_batch_reservation(
    budget: Arc<Semaphore>,
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> Option<OwnedSemaphorePermit> {
    if *shutdown.borrow() || *session_closed.borrow() {
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
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
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
    let permit = match reserve_event_queue_slot(events, shutdown, session_closed).await {
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
    shutdown: &watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> Result<mpsc::Permit<'a, EventEnvelope>, EventSendOutcome> {
    if *shutdown.borrow() || *session_closed.borrow() {
        return Err(EventSendOutcome::Shutdown);
    }
    tokio::select! {
        biased;
        _ = wait_for_watch_true(shutdown.clone()) => Err(EventSendOutcome::Shutdown),
        _ = wait_for_session_close(session_closed.clone()) => Err(EventSendOutcome::Shutdown),
        result = events.reserve() => result.map_err(|_| EventSendOutcome::ReceiverClosed),
    }
}

fn allocate_event_queue_incarnation() -> Option<u64> {
    NEXT_EVENT_QUEUE_INCARNATION.try_update(Ordering::Relaxed, Ordering::Relaxed, |value| value.checked_add(1)).ok()
}

#[tracing::instrument(name = "capabilities.event_queue", level = "debug", skip_all, fields(simulator = simulator_id.get()), ret)]
pub(super) async fn run_event_queue(
    simulator_id: SimulatorId,
    source: SocketAddr,
    seed: Url,
    http: http::Client,
    events: mpsc::Sender<EventEnvelope>,
    mut shutdown: watch::Receiver<bool>,
    mut primary: watch::Receiver<bool>,
    status: NetworkStatusHandle,
    session_closed: watch::Receiver<bool>,
    ordinary_requests: Arc<Semaphore>,
    primary_event_queues: Arc<Semaphore>,
    neighbor_event_queues: Arc<Semaphore>,
    queued_event_bytes: Arc<Semaphore>,
    coverage_losses: CoverageLossLedger,
) -> EventQueueExit {
    if *shutdown.borrow() || *session_closed.borrow() {
        return EventQueueExit::Stopped;
    }

    let Some(incarnation) = allocate_event_queue_incarnation() else {
        status.record_error("EventQueueGet poll incarnation space is exhausted.");
        return EventQueueExit::TaskFailed;
    };
    let coverage_source = CoverageSource::EventQueue { incarnation };
    let _scope = EventQueueScope { ledger: coverage_losses.clone(), simulator_id, source: coverage_source };
    let loss_notifier = EventQueueLossNotifier::new(events.clone(), simulator_id, source, shutdown.clone(), session_closed.clone());

    let names = [EVENT_QUEUE_NAME.to_owned()];
    let mut lookup_retries = 0;
    let (event_capability, _event_capability_memory) = loop {
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
            Ok(response) => {
                let names = names.clone();
                tokio::select! {
                    biased;
                    _ = wait_for_watch_true(shutdown.clone()) => return EventQueueExit::Stopped,
                    _ = wait_for_session_close(session_closed.clone()) => return EventQueueExit::Stopped,
                    decoded = spawn_blocking_with_permit(request_permit, move || admit_and_decode_seed_capabilities(response, &names)) => {
                        match decoded {
                            Ok((decoded, _permit)) => decoded,
                            Err(error) => Err(LookupError::Permanent(format!("Capability decoder task failed: {error}"))),
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
            Ok((mut granted, mut memory)) => {
                status.note_capabilities(granted.keys().map(String::as_str));
                match granted.remove(EVENT_QUEUE_NAME) {
                    Some(capability) => {
                        drop(granted);
                        if let Some(memory) = &mut memory {
                            memory.shrink_to(url_retained_memory_estimate(&capability.url, capability.input_bytes));
                        }
                        break (capability.url, memory);
                    }
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
            Err(error) if !error.is_transient() => {
                let error = error.to_string();
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
            Err(error) => {
                let message = error.to_string();
                let retry_after = error.retry_after();
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
    let mut batch_order = 0_u64;

    loop {
        if *shutdown.borrow() || *session_closed.borrow() {
            return EventQueueExit::Stopped;
        }

        let Some(event_queue_slot) = acquire_event_queue_permit(
            Arc::clone(&primary_event_queues),
            Arc::clone(&neighbor_event_queues),
            &mut primary,
            &shutdown,
            &session_closed,
        )
        .await
        else {
            return EventQueueExit::Stopped;
        };
        let mut event_queue_permit = Some(event_queue_slot);
        let Some(batch_reservation) = acquire_event_batch_reservation(Arc::clone(&queued_event_bytes), &shutdown, &session_closed) else {
            drop(event_queue_permit.take());
            if matches!(wait_for_retry(EVENT_QUEUE_ADMISSION_RETRY, &shutdown, &session_closed).await, RetryOutcome::Retry) {
                continue;
            }
            return EventQueueExit::Stopped;
        };
        let decoded_memory = match http.try_reserve_memory(http::ResponseMemoryClass::EventQueue, LLSD_SCAN_WORKING_BYTES) {
            Ok(permit) => permit,
            Err(error) => {
                status.record_error(format!("EventQueueGet response memory is unavailable: {error}"));
                drop(event_queue_permit.take());
                drop(batch_reservation);
                if matches!(wait_for_retry(EVENT_QUEUE_ADMISSION_RETRY, &shutdown, &session_closed).await, RetryOutcome::Retry) {
                    continue;
                }
                return EventQueueExit::Stopped;
            }
        };
        let mut pending_loss = PendingEventQueueLoss::new(coverage_losses.clone(), simulator_id, coverage_source, loss_notifier.clone());
        let response = tokio::select! {
            biased;
            _ = wait_for_watch_true(shutdown.clone()) => {
                pending_loss.commit();
                return EventQueueExit::Stopped;
            },
            _ = wait_for_session_close(session_closed.clone()) => {
                pending_loss.commit();
                return EventQueueExit::Stopped;
            },
            changed = primary.changed() => {
                pending_loss.record();
                drop(event_queue_permit.take());
                drop(batch_reservation);
                if changed.is_err() {
                    return EventQueueExit::Stopped;
                }
                continue;
            },
            result = post_event_queue_with_extra(
                &http,
                &event_capability,
                &acknowledge,
                MAX_EVENT_QUEUE_RESPONSE_BYTES,
                decoded_memory,
            ) => result,
        };

        match response {
            Ok(response) if response.status == StatusCode::NOT_FOUND => {
                pending_loss.record();
                let status_code = response.status;
                drop(response);
                drop(event_queue_permit.take());
                drop(batch_reservation);
                let reason = format!("EventQueueGet returned HTTP {status_code}; polling stopped.");
                status.record_error(&reason);
                let _ = pending_loss.notify(reason.clone()).await;
                let outcome = send_event_queue_error(
                    &events,
                    Event::CapabilityError { simulator_id, source, capability: EVENT_QUEUE_NAME.to_owned(), reason },
                    &shutdown,
                    &session_closed,
                )
                .await;
                return if matches!(outcome, EventSendOutcome::Sent) {
                    EventQueueExit::StoppedWithCoverageLoss
                } else {
                    EventQueueExit::Stopped
                };
            }
            Ok(response) if expected_empty_poll(response.status, response.elapsed) => {
                pending_loss.commit();
                drop(response);
                drop(event_queue_permit.take());
                drop(batch_reservation);
                error_count = 0;
            }
            Ok(response) if response.status.is_success() => {
                let decoded = decode_admitted_event_batch(
                    response.body,
                    response.memory,
                    response.extra_memory,
                    event_queue_permit.take().expect("successful response owns an EventQueue slot"),
                )
                .await;
                match decoded {
                    Ok((id, batch, event_queue_permit)) => {
                        error_count = 0;
                        if Value::Integer(id) == acknowledge {
                            pending_loss.commit();
                            drop(batch_reservation);
                            drop(batch);
                            drop(event_queue_permit);
                            continue;
                        }
                        let Some(next_batch_order) = batch_order.checked_add(1) else {
                            pending_loss.record();
                            status.record_error("EventQueue batch-order counter exhausted; polling stopped.");
                            drop(batch_reservation);
                            drop(batch);
                            drop(event_queue_permit);
                            return EventQueueExit::Unavailable;
                        };
                        let outcome = send_event_queue_batch(
                            &events,
                            simulator_id,
                            source,
                            coverage_source,
                            batch_order,
                            batch,
                            &shutdown,
                            &session_closed,
                            batch_reservation,
                            &coverage_losses,
                            &mut pending_loss,
                        )
                        .await;
                        drop(event_queue_permit);
                        match outcome {
                            EventSendOutcome::Sent => {
                                acknowledge = Value::Integer(id);
                                batch_order = next_batch_order;
                            }
                            EventSendOutcome::Shutdown => return EventQueueExit::Stopped,
                            EventSendOutcome::ReceiverClosed => {
                                let reason =
                                    String::from("EventQueueGet batch could not be delivered in full; some events may be missing.");
                                status.record_error(&reason);
                                let outcome = pending_loss.notify(reason).await;
                                return if matches!(outcome, EventSendOutcome::Sent) {
                                    EventQueueExit::StoppedWithCoverageLoss
                                } else {
                                    EventQueueExit::Stopped
                                };
                            }
                        }
                    }
                    Err(error) => {
                        let error = error.to_string();
                        pending_loss.record();
                        drop(batch_reservation);
                        drop(event_queue_permit.take());
                        status.record_error(&error);
                        let outcome = pending_loss.notify(error).await;
                        return if matches!(outcome, EventSendOutcome::Sent) {
                            EventQueueExit::StoppedWithCoverageLoss
                        } else {
                            EventQueueExit::Stopped
                        };
                    }
                }
            }
            Ok(response) => {
                let status_code = response.status;
                let retry_after = response.retry_after();
                drop(response);
                drop(event_queue_permit.take());
                drop(batch_reservation);
                let error = format!("EventQueueGet returned HTTP {status_code}.");
                status.record_error(&error);
                if !retryable_event_queue_error(status_code) {
                    pending_loss.record();
                    let outcome = pending_loss.notify(error).await;
                    return if matches!(outcome, EventSendOutcome::Sent) {
                        EventQueueExit::StoppedWithCoverageLoss
                    } else {
                        EventQueueExit::Stopped
                    };
                }
                match retry_event_queue_with_hint(
                    simulator_id,
                    &mut error_count,
                    source,
                    &events,
                    error.clone(),
                    retry_after,
                    &mut shutdown,
                    &session_closed,
                    &mut pending_loss,
                )
                .await
                {
                    RetryOutcome::Retry => {}
                    RetryOutcome::Stopped => return EventQueueExit::Stopped,
                    RetryOutcome::LimitReached => {
                        let outcome = pending_loss.notify(error).await;
                        return if matches!(outcome, EventSendOutcome::Sent) {
                            EventQueueExit::RetryLimitReached
                        } else {
                            EventQueueExit::Stopped
                        };
                    }
                }
            }
            Err(error) if error.is_timeout_after(MIN_EMPTY_POLL_DURATION) => {
                pending_loss.commit();
                drop(event_queue_permit.take());
                drop(batch_reservation);
                error_count = 0;
            }
            Err(error) if error.is_resource_exhausted() => {
                pending_loss.record();
                drop(event_queue_permit.take());
                drop(batch_reservation);
                let reason = format!("EventQueueGet response could not be admitted: {error}");
                status.record_error(&reason);
                let outcome = pending_loss.notify(reason).await;
                return if matches!(outcome, EventSendOutcome::Sent) {
                    EventQueueExit::StoppedWithCoverageLoss
                } else {
                    EventQueueExit::Stopped
                };
            }
            Err(error) => {
                pending_loss.record();
                drop(event_queue_permit.take());
                drop(batch_reservation);
                let error = format!("EventQueueGet request failed: {error}");
                status.record_error(&error);
                let outcome = pending_loss.notify(error).await;
                return if matches!(outcome, EventSendOutcome::Sent) {
                    EventQueueExit::StoppedWithCoverageLoss
                } else {
                    EventQueueExit::Stopped
                };
            }
        }
    }
}

#[cfg(test)]
pub(crate) async fn post_event_queue(
    client: &http::Client,
    url: &Url,
    acknowledge: &Value,
    max_response_bytes: usize,
) -> Result<Response, Error> {
    post_event_queue_inner(client, url, acknowledge, max_response_bytes, None).await
}

async fn post_event_queue_with_extra(
    client: &http::Client,
    url: &Url,
    acknowledge: &Value,
    max_response_bytes: usize,
    extra_memory: ResponseMemoryPermit,
) -> Result<Response, Error> {
    post_event_queue_inner(client, url, acknowledge, max_response_bytes, Some(extra_memory)).await
}

async fn post_event_queue_inner(
    client: &http::Client,
    url: &Url,
    acknowledge: &Value,
    max_response_bytes: usize,
    extra_memory: Option<ResponseMemoryPermit>,
) -> Result<Response, Error> {
    let payload = Value::Map(Map::from([("ack".to_owned(), acknowledge.clone()), ("done".to_owned(), Value::Boolean(false))]));
    let body = xml::to_string(&payload).map_err(|error| Error::Message(format!("Could not encode an EventQueueGet request: {error}")))?;
    let body = body.into_bytes();
    match extra_memory {
        Some(extra_memory) => {
            client
                .post_operation_bounded_classified_with_extra(
                    "event_queue",
                    url,
                    "application/llsd+xml",
                    Some("application/llsd+xml"),
                    body,
                    max_response_bytes,
                    extra_memory,
                )
                .await
        }
        None => {
            client
                .post_operation_bounded_classified(
                    "event_queue",
                    url,
                    "application/llsd+xml",
                    Some("application/llsd+xml"),
                    body,
                    max_response_bytes,
                )
                .await
        }
    }
}

async fn decode_admitted_event_batch(
    body: Vec<u8>,
    raw_memory: Option<ResponseMemoryPermit>,
    decoded_memory: Option<ResponseMemoryPermit>,
    event_queue_permit: OwnedSemaphorePermit,
) -> Result<(i32, Vec<CapabilityEvent>, OwnedSemaphorePermit), BatchDecodeError> {
    decode_admitted_event_batch_with(body, raw_memory, decoded_memory, event_queue_permit, decode_event_batch).await
}

async fn decode_admitted_event_batch_with<F>(
    body: Vec<u8>,
    raw_memory: Option<ResponseMemoryPermit>,
    mut decoded_memory: Option<ResponseMemoryPermit>,
    event_queue_permit: OwnedSemaphorePermit,
    decode: F,
) -> Result<(i32, Vec<CapabilityEvent>, OwnedSemaphorePermit), BatchDecodeError>
where
    F: FnOnce(&[u8]) -> Result<(i32, Vec<CapabilityEvent>), String> + Send + 'static,
{
    let (decoded, event_queue_permit) = spawn_blocking_with_permit(event_queue_permit, move || {
        let result = (|| {
            let required = event_batch_allocation_bound(&body).map_err(BatchDecodeError::Preflight)?;
            if let Some(memory) = &mut decoded_memory {
                memory.try_grow_to(required).map_err(BatchDecodeError::Resource)?;
            }
            decode(&body).map_err(BatchDecodeError::Decode)
        })();
        drop(body);
        drop(raw_memory);
        drop(decoded_memory);
        result
    })
    .await
    .map_err(BatchDecodeError::Task)?;
    decoded.map(|(id, events)| (id, events, event_queue_permit))
}

pub(super) fn event_batch_allocation_bound(body: &[u8]) -> Result<usize, xml::BoundError> {
    let bound = xml::value_allocation_bound(body, capability_llsd_limits())?;
    let conversion = MAX_EVENT_QUEUE_BATCH_EVENTS
        .checked_mul(size_of::<CapabilityEvent>())
        .and_then(|bytes| bytes.checked_add(capability_event_batch_retained_size(&[], 0)))
        .ok_or(xml::BoundError::Overflow)?;
    bound.checked_add(conversion).ok_or(xml::BoundError::Overflow)
}

pub(crate) fn decode_event_batch(body: &[u8]) -> Result<(i32, Vec<CapabilityEvent>), String> {
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
        if event_size > MAX_QUEUED_CAPABILITY_EVENT_BYTES {
            return Err(format!("EventQueueGet event exceeded the {MAX_QUEUED_CAPABILITY_EVENT_BYTES}-byte limit."));
        }
        events.push(CapabilityEvent { message, body, retained_bytes: event_size });
        if capability_event_batch_retained_size(&events, events.capacity()) > MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES {
            return Err(format!("EventQueueGet batch exceeded the {MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES}-byte retained-data limit."));
        }
    }

    Ok((id, events))
}

fn queued_event_retained_size(message_capacity: usize, body: &mut Value) -> usize {
    size_of::<CapabilityEvent>().saturating_add(message_capacity).saturating_add(llsd_heap_size(body))
}

fn capability_event_batch_retained_size(events: &[CapabilityEvent], event_capacity: usize) -> usize {
    let event_data_bytes =
        events.iter().fold(0usize, |total, event| total.saturating_add(event.retained_bytes.saturating_sub(size_of::<CapabilityEvent>())));
    size_of::<EventEnvelope>()
        .saturating_add(size_of::<OwnedSemaphorePermit>())
        .saturating_add(2 * size_of::<usize>())
        .saturating_add(event_capacity.saturating_mul(size_of::<CapabilityEvent>()))
        .saturating_add(event_data_bytes)
}

fn llsd_heap_size(value: &mut Value) -> usize {
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

#[cfg(test)]
pub(crate) async fn retry_event_queue(
    simulator_id: SimulatorId,
    error_count: &mut u32,
    source: SocketAddr,
    events: &mpsc::Sender<EventEnvelope>,
    error: String,
    shutdown: &mut watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
) -> RetryOutcome {
    let coverage_losses = CoverageLossLedger::new();
    let coverage_source = CoverageSource::EventQueue { incarnation: 1 };
    let notifier = EventQueueLossNotifier::new(events.clone(), simulator_id, source, shutdown.clone(), session_closed.clone());
    let mut pending_loss = PendingEventQueueLoss::new(coverage_losses, simulator_id, coverage_source, notifier);
    retry_event_queue_with_hint(simulator_id, error_count, source, events, error, None, shutdown, session_closed, &mut pending_loss).await
}

async fn retry_event_queue_with_hint(
    simulator_id: SimulatorId,
    error_count: &mut u32,
    source: SocketAddr,
    events: &mpsc::Sender<EventEnvelope>,
    error: String,
    retry_after: Option<Duration>,
    shutdown: &mut watch::Receiver<bool>,
    session_closed: &watch::Receiver<bool>,
    pending_loss: &mut PendingEventQueueLoss,
) -> RetryOutcome {
    if *shutdown.borrow() || *session_closed.borrow() {
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
            shutdown,
            session_closed,
        )
        .await;
        return if matches!(outcome, EventSendOutcome::Sent) { RetryOutcome::LimitReached } else { RetryOutcome::Stopped };
    }

    pending_loss.commit();
    let delay = event_queue_retry_delay_with_hint(*error_count, retry_after);
    tracing::debug!(attempt = *error_count, delay_ms = delay.as_millis(), "retrying event queue poll");
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
    tracing::debug!(attempt = *retries, delay_ms = delay.as_millis(), "retrying event queue discovery");
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

fn event_queue_retry_delay_with_hint(error_count: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.map(|delay| delay.min(MAX_EVENT_QUEUE_RETRY_AFTER)).unwrap_or_else(|| event_queue_retry_delay(error_count))
}

pub(crate) fn expected_empty_poll(status: StatusCode, elapsed: Duration) -> bool {
    elapsed >= MIN_EMPTY_POLL_DURATION
        && matches!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR | StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
        )
}

pub(super) fn retryable_event_queue_error(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS
        || status.as_u16() == LINDEN_HTTP_INTERNAL_ERROR
        || matches!(
            status,
            StatusCode::INTERNAL_SERVER_ERROR | StatusCode::BAD_GATEWAY | StatusCode::SERVICE_UNAVAILABLE | StatusCode::GATEWAY_TIMEOUT
        )
}

pub(crate) fn seed_capability_retry_delay(retry_count: u32) -> Duration {
    Duration::from_secs(1 << retry_count.saturating_sub(1).min(3))
}

pub(crate) fn seed_retry_delay(retry_count: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.map(|delay| delay.min(MAX_SEED_RETRY_AFTER)).unwrap_or_else(|| seed_capability_retry_delay(retry_count))
}

#[cfg(test)]
#[path = "event_queue_tests.rs"]
mod tests;
