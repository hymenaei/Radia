//! Owns one simulator's UDP socket, LLUDP circuit, and connection lifecycle.

use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use protocol::{
    LLUdpError,
    lludp::{AllocationStrategy, Message, MessageId, Packet},
    messages::{
        AgentThrottle, CompleteAgentMovement, CompletePingCheck, LogoutRequest, PingId, RegionHandshakeReply, ThrottleRates,
        UseCircuitCode,
        agent_throttle::{AgentData, Throttle},
        complete_agent_movement, logout_request, region_handshake_reply,
        use_circuit_code::CircuitCode,
    },
    schema::ids::PACKET_ACK,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::yield_now,
    time::{self, Instant},
};

use crate::{
    AgentSession,
    capture::CaptureRecorder,
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource},
    error::Error,
    event::{Event, EventEnvelope, SimulatorId},
    status::NetworkStatusHandle,
    transport::{
        UdpTransport,
        lludp::{Circuit, Delivery, KEEPALIVE_TIMEOUT, PING_INTERVAL, PreparedDatagram, ReceiveAdmission},
        pacing::{ConnectionPacer, OutboundBudget, OutboundBudgetHandle, PacerHandle, TrafficClass},
    },
};

#[cfg(test)]
use crate::transport::pacing::ProbeConnection;

#[cfg(test)]
use crate::transport::SendGate;

use super::{
    ACK_INTERVAL, ConnectionRole, EVENT_BACKLOG_DISCONNECT_REASON, HandoffTarget, LOGOUT_TIMEOUT, MAX_PRECONNECT_EVENTS,
    MAX_QUEUED_SIMULATOR_EVENTS, NEIGHBOR_PRECONNECT_OVERFLOW_REASON, NeighborCommand, NeighborEvent, NeighborRequest, PrimaryState,
    SimulatorMemoryBudget, connection_message_has_application_event, finish_logout, queue_neighbor_event, record_coverage_loss,
    record_event_envelope_loss,
};
pub(super) struct NeighborConnection {
    pub(super) simulator_id: SimulatorId,
    pub(super) session: AgentSession,
    pub(super) region_handle: Option<u64>,
    pub(super) role: ConnectionRole,
    pub(super) socket: Arc<UdpTransport>,
    pub(super) circuit: Circuit,
    pub(super) use_circuit_sequence: u32,
    pub(super) handoff: Option<HandoffTarget>,
    handoff_role_generation: Option<u64>,
    pub(super) movement_requested: bool,
    pub(super) initial_movement_sent: bool,
    pub(super) primary_active: bool,
    pub(super) session_ready: bool,
    agent_throttle_sent: bool,
    pub(super) capture: Option<CaptureRecorder>,
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) pacer: ConnectionPacer,
    _owned_budget: Option<OutboundBudget>,
    primary_state_receiver: watch::Receiver<PrimaryState>,
}

const MAX_PENDING_SENDS: usize = 8;
const CONTROL_SEND_RESERVE: usize = 2;
const MAX_NONCONTROL_SENDS: usize = MAX_PENDING_SENDS - CONTROL_SEND_RESERVE;
const MAX_PENDING_SEND_NOTIFICATIONS: usize = MAX_PENDING_SENDS * 2 + 2;
const MAX_SEND_ERROR_BYTES: usize = 256;
const OUTBOUND_SEND_CHARGE_BYTES: usize = 8 * 1024;
const MAX_PENDING_SEND_BYTES: usize = MAX_PENDING_SENDS * OUTBOUND_SEND_CHARGE_BYTES;
const MAX_SEND_AGE: Duration = Duration::from_secs(5);
const SEND_QUEUE_EXPIRED: &str = "A simulator datagram exceeded its queue age limit.";
const RELIABLE_RETRY_EXPIRED: &str = "A reliable simulator retransmission exceeded its send age limit.";
/// Matches the pinned viewer's default 3,000 throttle preference with its 1.5 headroom factor.
/// AgentThrottle carries the resulting per-class values in bits per second.
const DEFAULT_AGENT_THROTTLE_RATES: ThrottleRates = ThrottleRates {
    resend_bits_per_second: 460_800.0,
    land_bits_per_second: 317_440.0,
    wind_bits_per_second: 63_488.0,
    cloud_bits_per_second: 63_488.0,
    task_bits_per_second: 1_564_672.0,
    texture_bits_per_second: 1_564_672.0,
    asset_bits_per_second: 573_440.0,
};

struct QueuedMessage {
    message: Message,
    delivery: Delivery,
    class: TrafficClass,
    queued_at: Instant,
    completion: Option<oneshot::Sender<Result<(), Error>>>,
    cancel_target: Option<super::HandoffCancellation>,
    report_failure: bool,
    movement: MovementSend,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementSend {
    None,
    Initial,
    Handoff,
}

struct OutboundQueue {
    messages: VecDeque<QueuedMessage>,
    charged_bytes: usize,
    class_cursor: usize,
    #[cfg(test)]
    probe: Option<ProbeConnection>,
}

impl OutboundQueue {
    fn new() -> Self {
        Self {
            messages: VecDeque::with_capacity(MAX_PENDING_SENDS),
            charged_bytes: 0,
            class_cursor: 0,
            #[cfg(test)]
            probe: None,
        }
    }

    #[cfg(test)]
    fn with_probe(probe: Option<ProbeConnection>) -> Self {
        Self { probe, ..Self::new() }
    }

    #[cfg(test)]
    fn record_probe(&self) {
        if let Some(probe) = &self.probe {
            let now = Instant::now();
            let oldest_age = self.messages.iter().map(|queued| now.saturating_duration_since(queued.queued_at)).max().unwrap_or_default();
            let class_ages = [TrafficClass::Control, TrafficClass::Deadline, TrafficClass::Fair]
                .into_iter()
                .filter_map(|class| {
                    self.messages
                        .iter()
                        .filter(|queued| queued.class == class)
                        .map(|queued| now.saturating_duration_since(queued.queued_at))
                        .max()
                        .map(|age| (class, age))
                })
                .collect::<Vec<_>>();
            probe.record_local_queue(self.messages.len(), self.charged_bytes, oldest_age, &class_ages, now);
        }
    }

    fn push(&mut self, queued: QueuedMessage, active_class: Option<TrafficClass>) -> Result<(), Box<QueuedMessage>> {
        if self.is_full(queued.class, active_class) {
            return Err(Box::new(queued));
        }
        self.charged_bytes += OUTBOUND_SEND_CHARGE_BYTES;
        self.messages.push_back(queued);
        #[cfg(test)]
        self.record_probe();
        Ok(())
    }

    fn push_front(&mut self, queued: QueuedMessage) -> Result<(), Box<QueuedMessage>> {
        if self.is_full(queued.class, None) {
            return Err(Box::new(queued));
        }
        self.charged_bytes += OUTBOUND_SEND_CHARGE_BYTES;
        self.messages.push_front(queued);
        #[cfg(test)]
        self.record_probe();
        Ok(())
    }

    fn is_full(&self, class: TrafficClass, active_class: Option<TrafficClass>) -> bool {
        let active_count = usize::from(active_class.is_some());
        let active_bytes = active_count * OUTBOUND_SEND_CHARGE_BYTES;
        let noncontrol_count = self.messages.iter().filter(|queued| queued.class != TrafficClass::Control).count()
            + usize::from(active_class.is_some_and(|active| active != TrafficClass::Control));
        self.messages.len().saturating_add(active_count) >= MAX_PENDING_SENDS
            || (class != TrafficClass::Control && noncontrol_count >= MAX_NONCONTROL_SENDS)
            || self.charged_bytes.saturating_add(active_bytes).saturating_add(OUTBOUND_SEND_CHARGE_BYTES) > MAX_PENDING_SEND_BYTES
    }

    fn has_higher_priority_matching(&self, class: TrafficClass, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> bool {
        self.messages.iter().any(|queued| traffic_priority(queued.class) < traffic_priority(class) && eligible(queued))
    }

    fn pop_priority_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
        for class in [TrafficClass::Control, TrafficClass::Deadline] {
            if let Some(index) = self.messages.iter().position(|queued| queued.class == class && eligible(queued)) {
                #[cfg(test)]
                self.record_probe();
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                let removed = self.messages.remove(index);
                #[cfg(test)]
                self.record_probe();
                return removed;
            }
        }
        None
    }

    fn pop_control_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
        let index = self.messages.iter().position(|queued| queued.class == TrafficClass::Control && eligible(queued))?;
        #[cfg(test)]
        self.record_probe();
        self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
        let removed = self.messages.remove(index);
        #[cfg(test)]
        self.record_probe();
        removed
    }

    fn pop_next_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
        const ORDER: [TrafficClass; 5] =
            [TrafficClass::Control, TrafficClass::Control, TrafficClass::Deadline, TrafficClass::Fair, TrafficClass::Fair];
        for _ in 0..ORDER.len() {
            let class = ORDER[self.class_cursor];
            self.class_cursor = (self.class_cursor + 1) % ORDER.len();
            if let Some(index) = self.messages.iter().position(|queued| queued.class == class && eligible(queued)) {
                #[cfg(test)]
                self.record_probe();
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                let removed = self.messages.remove(index);
                #[cfg(test)]
                self.record_probe();
                return removed;
            }
        }
        None
    }

    #[cfg(test)]
    fn pop_next(&mut self) -> Option<QueuedMessage> {
        self.pop_next_matching(|_| true)
    }

    fn expire(&mut self, now: Instant) -> Vec<QueuedMessage> {
        #[cfg(test)]
        self.record_probe();
        let mut expired = Vec::new();
        let mut retained = VecDeque::with_capacity(self.messages.len());
        while let Some(queued) = self.messages.pop_front() {
            if now.saturating_duration_since(queued.queued_at) >= MAX_SEND_AGE {
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                #[cfg(test)]
                if let Some(probe) = &self.probe {
                    probe.record_queue_age(queued.class, now.saturating_duration_since(queued.queued_at), now);
                    probe.record_outbound_queue_expiration(queued.class, now);
                }
                expired.push(queued);
            } else {
                retained.push_back(queued);
            }
        }
        self.messages = retained;
        #[cfg(test)]
        self.record_probe();
        expired
    }

    fn has_handoff_movement(&self) -> bool {
        self.messages.iter().any(|queued| queued.movement == MovementSend::Handoff)
    }

    fn cancel_handoff_movement(&mut self) {
        #[cfg(test)]
        self.record_probe();
        let mut retained = VecDeque::with_capacity(self.messages.len());
        while let Some(queued) = self.messages.pop_front() {
            if queued.movement == MovementSend::Handoff {
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
            } else {
                retained.push_back(queued);
            }
        }
        self.messages = retained;
        #[cfg(test)]
        self.record_probe();
    }

    fn drain(&mut self) -> Vec<QueuedMessage> {
        #[cfg(test)]
        self.record_probe();
        self.charged_bytes = 0;
        let drained = self.messages.drain(..).collect();
        #[cfg(test)]
        self.record_probe();
        drained
    }
}

type PendingSendFuture = Pin<Box<dyn Future<Output = Result<(Instant, PreparedDatagram), String>> + Send>>;

struct PendingSend {
    meta: PreparedMeta,
    result: PendingSendFuture,
    class: TrafficClass,
    queued_message: Option<QueuedMessage>,
    retry_deadline: Option<Instant>,
    #[cfg(test)]
    prepared: PreparedDatagram,
    #[cfg(test)]
    probe: Option<ProbeConnection>,
}

#[derive(Clone, Copy)]
struct PreparedMeta {
    sequence: u32,
    is_acknowledgement: bool,
    is_retransmission: bool,
}

impl From<&PreparedDatagram> for PreparedMeta {
    fn from(prepared: &PreparedDatagram) -> Self {
        Self {
            sequence: prepared.sequence(),
            is_acknowledgement: prepared.is_acknowledgement(),
            is_retransmission: prepared.is_retransmission(),
        }
    }
}

fn pending_send_result(
    socket: Arc<UdpTransport>,
    pacer: PacerHandle,
    prepared: PreparedDatagram,
    class: TrafficClass,
    retransmission: bool,
) -> PendingSendFuture {
    Box::pin(async move {
        let sent_at = if retransmission {
            pacer.send_retransmission(&socket, prepared.bytes(), class).await?
        } else {
            pacer.send(&socket, prepared.bytes(), class).await?
        };
        Ok((sent_at, prepared))
    })
}

fn begin_send(
    socket: Arc<UdpTransport>,
    pacer: PacerHandle,
    prepared: PreparedDatagram,
    class: TrafficClass,
    queued_message: Option<QueuedMessage>,
    #[cfg(test)] probe: Option<ProbeConnection>,
) -> PendingSend {
    let meta = PreparedMeta::from(&prepared);
    #[cfg(test)]
    let test_prepared = prepared.clone();
    let result = pending_send_result(socket, pacer, prepared, class, false);
    PendingSend {
        meta,
        result,
        class,
        queued_message,
        retry_deadline: None,
        #[cfg(test)]
        prepared: test_prepared,
        #[cfg(test)]
        probe,
    }
}

fn begin_retry_send(
    socket: Arc<UdpTransport>,
    pacer: PacerHandle,
    prepared: PreparedDatagram,
    retry_deadline: Instant,
    #[cfg(test)] probe: Option<ProbeConnection>,
) -> PendingSend {
    let meta = PreparedMeta::from(&prepared);
    #[cfg(test)]
    let test_prepared = prepared.clone();
    let result = pending_send_result(socket, pacer, prepared, TrafficClass::Fair, true);
    PendingSend {
        meta,
        result,
        class: TrafficClass::Fair,
        queued_message: None,
        retry_deadline: Some(retry_deadline),
        #[cfg(test)]
        prepared: test_prepared,
        #[cfg(test)]
        probe,
    }
}

fn active_queue_class(pending: Option<&PendingSend>) -> Option<TrafficClass> {
    pending?.queued_message.as_ref().map(|queued| queued.class)
}

fn traffic_priority(class: TrafficClass) -> u8 {
    match class {
        TrafficClass::Control => 0,
        TrafficClass::Deadline => 1,
        TrafficClass::Fair => 2,
    }
}

enum PendingSendExpiration {
    QueuedMessage(QueuedMessage),
    ReliableRetry,
}

fn expire_pending_send(pending: &mut Option<PendingSend>, now: Instant) -> Option<PendingSendExpiration> {
    let current = pending.as_ref()?;
    let queued_expired =
        current.queued_message.as_ref().is_some_and(|queued| now.saturating_duration_since(queued.queued_at) >= MAX_SEND_AGE);
    let retry_expired = current.retry_deadline.is_some_and(|deadline| now >= deadline);
    if !queued_expired && !retry_expired {
        return None;
    }

    #[cfg(test)]
    if let Some(probe) = &current.probe {
        if let Some(queued) = current.queued_message.as_ref().filter(|_| queued_expired) {
            probe.record_queue_age(queued.class, now.saturating_duration_since(queued.queued_at), now);
            probe.record_pending_send_expiration(queued.class, now);
        }
        if retry_expired && current.meta.is_retransmission {
            probe.record_active_retry_expiration(now);
        }
    }

    match pending.take()?.queued_message {
        Some(queued) => Some(PendingSendExpiration::QueuedMessage(queued)),
        None if retry_expired => Some(PendingSendExpiration::ReliableRetry),
        None => None,
    }
}

fn expire_deferred_retries(
    circuit: &Circuit,
    deferred_retries: &mut HashMap<u32, Instant>,
    now: Instant,
    #[cfg(test)] active_retry: Option<u32>,
    #[cfg(test)] probe: Option<&ProbeConnection>,
) -> Result<(), String> {
    deferred_retries.retain(|sequence, _| circuit.has_pending(*sequence));
    let mut expired = false;
    #[cfg(test)]
    let mut expired_deferred = 0usize;
    #[cfg(test)]
    let mut expired_active = false;
    for (&_sequence, &started_at) in deferred_retries.iter() {
        if now.saturating_duration_since(started_at) >= MAX_SEND_AGE {
            expired = true;
            #[cfg(test)]
            if active_retry == Some(_sequence) {
                expired_active = true;
            } else {
                expired_deferred = expired_deferred.saturating_add(1);
            }
        }
    }
    #[cfg(test)]
    if expired_deferred > 0 || expired_active {
        if let Some(probe) = probe {
            probe.record_deferred_retry_expirations(expired_deferred, now);
            if expired_active {
                probe.record_active_retry_expiration(now);
            }
        }
        return Err(RELIABLE_RETRY_EXPIRED.into());
    }
    if expired {
        return Err(RELIABLE_RETRY_EXPIRED.into());
    }
    Ok(())
}

async fn send_prepared(
    socket: &UdpTransport,
    circuit: &mut Circuit,
    pacer: &ConnectionPacer,
    prepared: PreparedDatagram,
    class: TrafficClass,
) -> Result<u32, String> {
    let sequence = prepared.sequence();
    let sent_at = pacer.handle().send(socket, prepared.bytes(), class).await?;
    circuit.commit_sent(prepared, sent_at)?;
    Ok(sequence)
}

fn logout_message(session: &AgentSession) -> Message {
    Message::LogoutRequest(LogoutRequest {
        agent_data: logout_request::AgentData { agent_id: session.agent_id, session_id: session.session_id },
    })
}

fn agent_throttle_message(session: &AgentSession) -> Message {
    Message::AgentThrottle(AgentThrottle {
        agent_data: AgentData { agent_id: session.agent_id, session_id: session.session_id, circuit_code: session.circuit_code },
        throttle: Throttle { gen_counter: 0, throttles: DEFAULT_AGENT_THROTTLE_RATES },
    })
}

fn queue_internal(
    queue: &mut OutboundQueue,
    message: Message,
    delivery: Delivery,
    class: TrafficClass,
    movement: MovementSend,
    active_class: Option<TrafficClass>,
) -> Result<(), String> {
    let queued = QueuedMessage {
        message,
        delivery,
        class,
        queued_at: Instant::now(),
        completion: None,
        cancel_target: None,
        report_failure: false,
        movement,
    };
    queue.push(queued, active_class).map_err(|_| "The simulator send queue is full.".to_string())
}

fn bound_send_error(mut error: String) -> String {
    if error.len() > MAX_SEND_ERROR_BYTES {
        let mut end = MAX_SEND_ERROR_BYTES;
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
    }
    error
}

fn queue_send_notification(
    events: &mpsc::Sender<NeighborEvent>,
    pending: &mut VecDeque<NeighborEvent>,
    simulator_id: SimulatorId,
    cancel_target: Option<super::HandoffCancellation>,
    error: Option<String>,
    report_failure: bool,
) -> Result<(), String> {
    if !report_failure && cancel_target.is_none() {
        return Ok(());
    }
    let event = NeighborEvent::SendCompleted { simulator_id, cancel_target, error: error.map(bound_send_error), report_failure };
    if !pending.is_empty() {
        return push_send_notification(pending, event);
    }

    match events.try_send(event) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(event)) => push_send_notification(pending, event),
        Err(mpsc::error::TrySendError::Closed(_)) => Err("The simulator manager has closed.".into()),
    }
}

fn push_send_notification(pending: &mut VecDeque<NeighborEvent>, event: NeighborEvent) -> Result<(), String> {
    if pending.len() >= MAX_PENDING_SEND_NOTIFICATIONS {
        return Err("The simulator send notification queue is full.".into());
    }
    pending.push_back(event);
    Ok(())
}

fn report_queued_failure(
    events: &mpsc::Sender<NeighborEvent>,
    pending: &mut VecDeque<NeighborEvent>,
    simulator_id: SimulatorId,
    mut queued: QueuedMessage,
    error: String,
) -> Result<(), String> {
    let has_completion = queued.completion.is_some();
    if let Some(completion) = queued.completion.take() {
        let _ = completion.send(Err(Error::SimulatorSend(error.clone())));
    }
    if queued.report_failure || queued.cancel_target.is_some() {
        queue_send_notification(events, pending, simulator_id, queued.cancel_target, Some(error), queued.report_failure)?;
    } else if !has_completion {
        return Err(error);
    }
    Ok(())
}

fn preempt_deferred_send(
    pending: &mut Option<PendingSend>,
    queue: &mut OutboundQueue,
    circuit: &Circuit,
    ack_deadline_due: bool,
) -> Result<(), String> {
    let Some(current) = pending.as_ref() else { return Ok(()) };
    let obsolete_retry = current.meta.is_retransmission && !circuit.has_pending(current.meta.sequence);
    let ack_preempts = ack_deadline_due && !current.meta.is_acknowledgement && circuit.has_pending_acks();
    let class_preempts = queue.has_higher_priority_matching(current.class, |queued| can_prepare_queued(circuit, queued));
    if obsolete_retry || ack_preempts || class_preempts {
        let mut canceled = pending.take().expect("pending send was checked");
        if (ack_preempts || class_preempts)
            && let Some(queued) = canceled.queued_message.take()
        {
            queue.push_front(queued).map_err(|_| "The simulator send queue is full while prioritizing traffic.".to_string())?;
        }
    }
    Ok(())
}

fn fail_queued_on_close(
    events: &mpsc::Sender<NeighborEvent>,
    pending: &mut VecDeque<NeighborEvent>,
    simulator_id: SimulatorId,
    mut queued: QueuedMessage,
) {
    let error = Error::SimulatorConnectionClosed.to_string();
    if let Some(completion) = queued.completion.take() {
        let _ = completion.send(Err(Error::SimulatorConnectionClosed));
    }
    if queued.report_failure || queued.cancel_target.is_some() {
        let _ = queue_send_notification(events, pending, simulator_id, queued.cancel_target, Some(error), queued.report_failure);
    }
}

async fn flush_send_notifications(
    events: &mpsc::Sender<NeighborEvent>,
    pending: &mut VecDeque<NeighborEvent>,
    shutdown: &mut watch::Receiver<bool>,
) {
    let deadline = Instant::now() + LOGOUT_TIMEOUT;
    while !pending.is_empty() && !*shutdown.borrow() && !events.is_closed() {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow_and_update() {
                    return;
                }
            }
            _ = events.closed() => return,
            _ = time::sleep_until(deadline) => return,
            permit = events.reserve() => {
                match permit {
                    Ok(permit) => {
                        if let Some(event) = pending.pop_front() {
                            permit.send(event);
                        }
                    }
                    Err(_) => return,
                }
            }
        }
    }
}

fn prepare_next_send(
    socket: &Arc<UdpTransport>,
    pacer: &ConnectionPacer,
    circuit: &mut Circuit,
    queue: &mut OutboundQueue,
    deferred_retries: &mut HashMap<u32, Instant>,
    send_ack_now: bool,
    last_send_was_retry: bool,
) -> Result<Option<PendingSend>, (Option<Box<QueuedMessage>>, String)> {
    if send_ack_now && circuit.has_pending_acks() {
        let prepared = circuit.prepare_acknowledgements().map_err(|error| (None, error))?;
        if let Some(prepared) = prepared {
            return Ok(Some(begin_send(
                Arc::clone(socket),
                pacer.handle(),
                prepared,
                TrafficClass::Control,
                None,
                #[cfg(test)]
                None,
            )));
        }
    }

    let now = Instant::now();
    let prepared = circuit.prepare_retransmission(now).map_err(|error| (None, error))?;
    if let Some(prepared) = prepared {
        let retry_started_at = match deferred_retries.entry(prepared.sequence()) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                pacer.handle().report_loss();
                *entry.insert(now)
            }
        };
        if let Some(queued) = queue.pop_priority_matching(|queued| can_prepare_queued(circuit, queued)) {
            let message = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
                Ok(prepared) => prepared,
                Err(error) => return Err((Some(Box::new(queued)), error)),
            };
            return Ok(Some(begin_send(
                Arc::clone(socket),
                pacer.handle(),
                message,
                queued.class,
                Some(queued),
                #[cfg(test)]
                queue.probe.clone(),
            )));
        }
        if last_send_was_retry && let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued)) {
            let message = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
                Ok(prepared) => prepared,
                Err(error) => return Err((Some(Box::new(queued)), error)),
            };
            return Ok(Some(begin_send(
                Arc::clone(socket),
                pacer.handle(),
                message,
                queued.class,
                Some(queued),
                #[cfg(test)]
                queue.probe.clone(),
            )));
        }
        return Ok(Some(begin_retry_send(
            Arc::clone(socket),
            pacer.handle(),
            prepared,
            retry_started_at + MAX_SEND_AGE,
            #[cfg(test)]
            queue.probe.clone(),
        )));
    }

    if circuit.has_pending_acks()
        && let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued))
    {
        let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
            Ok(prepared) => prepared,
            Err(error) => return Err((Some(Box::new(queued)), error)),
        };
        return Ok(Some(begin_send(
            Arc::clone(socket),
            pacer.handle(),
            prepared,
            queued.class,
            Some(queued),
            #[cfg(test)]
            queue.probe.clone(),
        )));
    }

    let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued)) else { return Ok(None) };
    let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
        Ok(prepared) => prepared,
        Err(error) => return Err((Some(Box::new(queued)), error)),
    };
    let class = queued.class;
    Ok(Some(begin_send(
        Arc::clone(socket),
        pacer.handle(),
        prepared,
        class,
        Some(queued),
        #[cfg(test)]
        queue.probe.clone(),
    )))
}

fn can_prepare_queued(circuit: &Circuit, queued: &QueuedMessage) -> bool {
    !queued.delivery.tracks_reliable() || circuit.has_reliable_capacity()
}

fn prepare_shutdown_send(
    socket: &Arc<UdpTransport>,
    pacer: &ConnectionPacer,
    circuit: &mut Circuit,
    queue: &mut OutboundQueue,
    send_ack_now: bool,
) -> Result<Option<PendingSend>, (Option<Box<QueuedMessage>>, String)> {
    if send_ack_now && circuit.has_pending_acks() {
        let prepared = circuit.prepare_acknowledgements().map_err(|error| (None, error))?;
        if let Some(prepared) = prepared {
            return Ok(Some(begin_send(
                Arc::clone(socket),
                pacer.handle(),
                prepared,
                TrafficClass::Control,
                None,
                #[cfg(test)]
                None,
            )));
        }
    }

    if let Some(queued) = queue.pop_control_matching(|queued| can_prepare_queued(circuit, queued)) {
        let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
            Ok(prepared) => prepared,
            Err(error) => return Err((Some(Box::new(queued)), error)),
        };
        return Ok(Some(begin_send(
            Arc::clone(socket),
            pacer.handle(),
            prepared,
            queued.class,
            Some(queued),
            #[cfg(test)]
            queue.probe.clone(),
        )));
    }

    Ok(None)
}

fn message_class(message: &Message) -> TrafficClass {
    match message {
        Message::AgentUpdate(_) | Message::CompleteAgentMovement(_) => TrafficClass::Deadline,
        Message::UseCircuitCode(_)
        | Message::RegionHandshakeReply(_)
        | Message::CompletePingCheck(_)
        | Message::LogoutRequest(_)
        | Message::TeleportCancel(_) => TrafficClass::Control,
        _ => TrafficClass::Fair,
    }
}

impl Drop for NeighborConnection {
    fn drop(&mut self) {
        self.coverage_losses.retire(self.simulator_id, CoverageSource::Lludp);
    }
}

impl NeighborConnection {
    fn current_primary_state(&self) -> PrimaryState {
        *self.primary_state_receiver.borrow()
    }

    fn clear_handoff(&mut self, outbound: &mut OutboundQueue, pending_send: &mut Option<PendingSend>) {
        self.handoff = None;
        self.handoff_role_generation = None;
        self.movement_requested = false;
        if pending_send
            .as_ref()
            .is_some_and(|pending| pending.queued_message.as_ref().is_some_and(|queued| queued.movement == MovementSend::Handoff))
        {
            *pending_send = None;
        }
        outbound.cancel_handoff_movement();
    }

    fn invalidate_stale_handoff(&mut self, role_generation: u64, outbound: &mut OutboundQueue, pending_send: &mut Option<PendingSend>) {
        if self.handoff_role_generation.is_some_and(|generation| generation != role_generation) {
            self.clear_handoff(outbound, pending_send);
        }
    }

    #[cfg(test)]
    pub(super) async fn connect(
        simulator_id: SimulatorId,
        session: AgentSession,
        region_handle: u64,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
    ) -> Result<Self, String> {
        Self::connect_with_coverage(simulator_id, session, region_handle, capture, status, CoverageLossLedger::new()).await
    }

    #[cfg(test)]
    pub(super) async fn connect_with_coverage(
        simulator_id: SimulatorId,
        session: AgentSession,
        region_handle: u64,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
    ) -> Result<Self, String> {
        let budget = OutboundBudget::new(crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
        let pacer = budget.register(simulator_id.get(), false).await?;
        Self::connect_with_role(
            simulator_id,
            session,
            Some(region_handle),
            ConnectionRole::Neighbor,
            capture,
            status,
            coverage_losses,
            None,
            pacer,
            Some(budget),
            #[cfg(test)]
            None,
        )
        .await
    }

    pub(super) async fn connect_with_primary_state(
        simulator_id: SimulatorId,
        session: AgentSession,
        region_handle: u64,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
        primary_state_receiver: watch::Receiver<PrimaryState>,
        budget: OutboundBudgetHandle,
    ) -> Result<Self, String> {
        let pacer = budget.register(simulator_id.get(), false).await?;
        Self::connect_with_role(
            simulator_id,
            session,
            Some(region_handle),
            ConnectionRole::Neighbor,
            capture,
            status,
            coverage_losses,
            Some(primary_state_receiver),
            pacer,
            None,
            #[cfg(test)]
            None,
        )
        .await
    }

    pub(super) async fn connect_primary_with_state(
        simulator_id: SimulatorId,
        session: AgentSession,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
        primary_state_receiver: watch::Receiver<PrimaryState>,
        budget: OutboundBudgetHandle,
        #[cfg(test)] send_gate: Option<SendGate>,
    ) -> Result<Self, String> {
        let pacer = budget.register(simulator_id.get(), true).await?;
        Self::connect_with_role(
            simulator_id,
            session,
            None,
            ConnectionRole::Primary,
            capture,
            status,
            coverage_losses,
            Some(primary_state_receiver),
            pacer,
            None,
            #[cfg(test)]
            send_gate,
        )
        .await
    }

    #[cfg(test)]
    pub(super) async fn connect_primary_with_coverage(
        simulator_id: SimulatorId,
        session: AgentSession,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
    ) -> Result<Self, String> {
        let budget = OutboundBudget::new(crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
        let pacer = budget.register(simulator_id.get(), true).await?;
        Self::connect_with_role(
            simulator_id,
            session,
            None,
            ConnectionRole::Primary,
            capture,
            status,
            coverage_losses,
            None,
            pacer,
            Some(budget),
            #[cfg(test)]
            None,
        )
        .await
    }

    async fn connect_with_role(
        simulator_id: SimulatorId,
        session: AgentSession,
        region_handle: Option<u64>,
        role: ConnectionRole,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
        primary_state: Option<watch::Receiver<PrimaryState>>,
        pacer: ConnectionPacer,
        owned_budget: Option<OutboundBudget>,
        #[cfg(test)] send_gate: Option<SendGate>,
    ) -> Result<Self, String> {
        let primary_state_receiver = primary_state.unwrap_or_else(|| {
            let initial_state = if role == ConnectionRole::Primary { PrimaryState::PRIMARY_CONNECTING } else { PrimaryState::NEIGHBOR };
            watch::channel(initial_state).1
        });
        let socket =
            UdpTransport::connect_with_status(session.simulator_address, session.circuit_code, capture.clone(), status.clone()).await?;
        #[cfg(test)]
        let socket = {
            let mut socket = socket;
            if let Some(send_gate) = send_gate {
                socket.set_send_gate(send_gate);
            }
            socket
        };
        let socket = Arc::new(socket);
        let circuit_status = match role {
            ConnectionRole::Primary => status.clone(),
            ConnectionRole::Neighbor => status.for_circuit(),
        };
        let mut circuit = Circuit::with_status(circuit_status);
        let prepared = circuit.prepare_message(
            Message::UseCircuitCode(UseCircuitCode {
                circuit_code: CircuitCode { code: session.circuit_code, session_id: session.session_id, agent_id: session.agent_id },
            }),
            Delivery::Reliable,
        )?;
        let use_circuit_sequence = send_prepared(&socket, &mut circuit, &pacer, prepared, TrafficClass::Control).await?;
        Ok(Self {
            simulator_id,
            session,
            region_handle,
            role,
            socket,
            circuit,
            use_circuit_sequence,
            handoff: None,
            handoff_role_generation: None,
            movement_requested: false,
            initial_movement_sent: false,
            primary_active: role == ConnectionRole::Primary,
            session_ready: false,
            agent_throttle_sent: false,
            capture,
            coverage_losses,
            pacer,
            _owned_budget: owned_budget,
            primary_state_receiver,
        })
    }

    #[tracing::instrument(name = "simulator.neighbor", level = "info", skip_all)]
    pub(super) async fn run(
        mut self,
        events: mpsc::Sender<NeighborEvent>,
        mut shutdown: watch::Receiver<bool>,
        neighbor_requests: mpsc::Sender<NeighborRequest>,
        mut commands: mpsc::Receiver<NeighborCommand>,
        mut logout_commands: mpsc::Receiver<oneshot::Sender<Result<(), Error>>>,
        packet_memory: SimulatorMemoryBudget,
    ) -> Result<(), String> {
        let mut receive_buffer = [0u8; 65_535];
        let mut established = false;
        let mut preconnect_messages: VecDeque<EventEnvelope> = VecDeque::with_capacity(MAX_PRECONNECT_EVENTS);
        let mut pending_events = VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS);
        let mut pending_notifications = VecDeque::with_capacity(MAX_PENDING_SEND_NOTIFICATIONS);
        #[cfg(test)]
        let mut outbound = OutboundQueue::with_probe(self.pacer.probe());
        #[cfg(not(test))]
        let mut outbound = OutboundQueue::new();
        let mut pending_send = None::<PendingSend>;
        let mut deferred_retries = HashMap::<u32, Instant>::new();
        let mut priority_logout = None::<QueuedMessage>;
        let mut logout_waiters = Vec::<oneshot::Sender<Result<(), Error>>>::new();
        let mut closing = false;
        let mut logout_sent = false;
        let mut logout_local_send_completed = false;
        let mut logout_commands_closed = false;
        let mut teardown_deadline = None;
        let mut close_reason = None;
        let mut ack_timer = time::interval_at(Instant::now() + ACK_INTERVAL, ACK_INTERVAL);
        ack_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut ack_timer_due = false;
        let mut ping_timer = time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
        ping_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut last_udp_received = Instant::now();
        let mut last_send_was_retry = false;
        let mut saw_region_handshake = false;
        let mut finish_after_ack = false;
        let mut primary_state_open = true;
        let mut event_delivery_turn = false;

        let result = async {
        loop {
            if *shutdown.borrow() || events.is_closed() {
                return finish_logout(close_reason);
            }
            let retry_check_at = Instant::now();
            expire_deferred_retries(
                &self.circuit,
                &mut deferred_retries,
                retry_check_at,
                #[cfg(test)] pending_send.as_ref().filter(|pending| pending.meta.is_retransmission).map(|pending| pending.meta.sequence),
                #[cfg(test)] outbound.probe.as_ref(),
            )?;
            if pending_notifications.is_empty() {
                let expired = outbound.expire(Instant::now());
                for queued in expired {
                    report_queued_failure(
                        &events,
                        &mut pending_notifications,
                        self.simulator_id,
                        queued,
                        SEND_QUEUE_EXPIRED.into(),
                    )?;
                }
            }
            match expire_pending_send(&mut pending_send, Instant::now()) {
                Some(PendingSendExpiration::QueuedMessage(queued)) => {
                    report_queued_failure(
                        &events,
                        &mut pending_notifications,
                        self.simulator_id,
                        queued,
                        SEND_QUEUE_EXPIRED.into(),
                    )?;
                }
                Some(PendingSendExpiration::ReliableRetry) => return Err(RELIABLE_RETRY_EXPIRED.into()),
                None => {}
            }
            if closing && pending_events.is_empty() && !logout_sent {
                finish_after_ack = true;
            }
            if finish_after_ack {
                for queued in outbound.drain() {
                    fail_queued_on_close(&events, &mut pending_notifications, self.simulator_id, queued);
                }
                if pending_send.as_ref().is_some_and(|pending| !pending.meta.is_acknowledgement)
                    && let Some(mut canceled) = pending_send.take()
                    && let Some(queued) = canceled.queued_message.take()
                {
                    fail_queued_on_close(&events, &mut pending_notifications, self.simulator_id, queued);
                }
            }
            if finish_after_ack && pending_send.is_none() && !self.circuit.has_pending_acks() && pending_notifications.is_empty() {
                return finish_logout(close_reason);
            }
            if pending_send.is_none() {
                if let Some(queued) = priority_logout.take() {
                    let prepared = match self.circuit.prepare_message(queued.message.clone(), queued.delivery) {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            let is_logout = matches!(&queued.message, Message::LogoutRequest(_));
                            report_queued_failure(&events, &mut pending_notifications, self.simulator_id, queued, error.clone())?;
                            if is_logout {
                                for completed in logout_waiters.drain(..) {
                                    let _ = completed.send(Err(Error::SimulatorSend(error.clone())));
                                }
                            }
                            return Err(error);
                        }
                    };
                    pending_send = Some(begin_send(
                        Arc::clone(&self.socket),
                        self.pacer.handle(),
                        prepared,
                        TrafficClass::Control,
                        Some(queued),
                        #[cfg(test)]
                        outbound.probe.clone(),
                    ));
                } else {
                    let next = if logout_sent {
                        prepare_shutdown_send(&self.socket, &self.pacer, &mut self.circuit, &mut outbound, ack_timer_due)
                    } else {
                        prepare_next_send(
                            &self.socket,
                            &self.pacer,
                            &mut self.circuit,
                            &mut outbound,
                            &mut deferred_retries,
                            ack_timer_due || finish_after_ack,
                            last_send_was_retry,
                        )
                    };
                    match next {
                        Ok(next) => pending_send = next,
                        Err((Some(queued), error)) => {
                            report_queued_failure(&events, &mut pending_notifications, self.simulator_id, *queued, error)?;
                            continue;
                        }
                        Err((None, error)) => return Err(error),
                    }
                }
            }
            tokio::select! {
                biased;
                sent = async { pending_send.as_mut().expect("send branch is guarded").result.as_mut().await }, if pending_send.is_some() => {
                    let mut completed = pending_send.take().expect("send branch is guarded");
                    match sent {
                        Ok((sent_at, prepared)) => {
                            let was_retry = completed.meta.is_retransmission;
                            let sequence = completed.meta.sequence;
                            let was_logout = completed
                                .queued_message
                                .as_ref()
                                .is_some_and(|queued| matches!(&queued.message, Message::LogoutRequest(_)));
                            self.circuit.commit_sent(prepared, sent_at)?;
                            #[cfg(test)]
                            if let (Some(probe), Some(queued)) = (&completed.probe, &completed.queued_message) {
                                probe.record_queue_age(queued.class, sent_at.saturating_duration_since(queued.queued_at), sent_at);
                            }
                            if was_retry {
                                deferred_retries.remove(&sequence);
                            }
                            last_send_was_retry = was_retry;
                            if !self.circuit.has_pending_acks() {
                                ack_timer_due = false;
                            }
                            if let Some(queued) = &completed.queued_message
                                && let Some(cancel_target) = queued.cancel_target
                            {
                                queue_send_notification(
                                    &events,
                                    &mut pending_notifications,
                                    self.simulator_id,
                                    Some(cancel_target),
                                    None,
                                    false,
                                )?;
                            }
                            if let Some(queued) = &mut completed.queued_message
                                && let Some(completion) = queued.completion.take()
                            {
                                let _ = completion.send(Ok(()));
                            }
                            if was_logout {
                                logout_local_send_completed = true;
                                for completed in logout_waiters.drain(..) {
                                    let _ = completed.send(Ok(()));
                                }
                            }
                            if let Some(queued) = &completed.queued_message {
                                match queued.movement {
                                    MovementSend::Initial => self.initial_movement_sent = true,
                                    MovementSend::Handoff => self.movement_requested = true,
                                    MovementSend::None => {}
                                }
                            }
                        }
                        Err(error) => {
                            let was_logout = completed
                                .queued_message
                                .as_ref()
                                .is_some_and(|queued| matches!(&queued.message, Message::LogoutRequest(_)));
                            if let Some(queued) = completed.queued_message.take() {
                                report_queued_failure(&events, &mut pending_notifications, self.simulator_id, queued, error.clone())?;
                            }
                            if was_logout {
                                for completed in logout_waiters.drain(..) {
                                    let _ = completed.send(Err(Error::SimulatorSend(error.clone())));
                                }
                            }
                            return Err(error);
                        }
                    }
                }
                _ = shutdown.changed() => return Ok(()),
                _ = events.closed() => return Ok(()),
                logout = logout_commands.recv(), if !logout_commands_closed => {
                    match logout {
                        Some(completed) if logout_sent => {
                            if logout_local_send_completed {
                                let _ = completed.send(Ok(()));
                            } else {
                                logout_waiters.push(completed);
                            }
                        }
                        Some(completed) => {
                            if let Some(mut canceled) = pending_send.take()
                                && let Some(queued) = canceled.queued_message.take()
                            {
                                fail_queued_on_close(&events, &mut pending_notifications, self.simulator_id, queued);
                            }
                            for queued in outbound.drain() {
                                fail_queued_on_close(&events, &mut pending_notifications, self.simulator_id, queued);
                            }
                            let queued = QueuedMessage {
                                message: logout_message(&self.session),
                                delivery: Delivery::ReliableNoRetry,
                                class: TrafficClass::Control,
                                queued_at: Instant::now(),
                                completion: Some(completed),
                                cancel_target: None,
                                report_failure: false,
                                movement: MovementSend::None,
                            };
                            priority_logout = Some(queued);
                            logout_sent = true;
                            closing = true;
                            teardown_deadline.get_or_insert_with(|| Instant::now() + LOGOUT_TIMEOUT);
                        }
                        None => logout_commands_closed = true,
                    }
                }
                _ = time::sleep_until(teardown_deadline.unwrap_or_else(|| Instant::now() + LOGOUT_TIMEOUT)), if teardown_deadline.is_some() => {
                    return finish_logout(close_reason);
                }
                _ = time::sleep_until(last_udp_received + KEEPALIVE_TIMEOUT),
                    if established
                        && !closing
                        && (self.role == ConnectionRole::Neighbor || !self.primary_active || self.session_ready) => {
                    return Err("The simulator stopped responding to LLUDP keepalive traffic.".into());
                }
                changed = self.primary_state_receiver.changed(), if primary_state_open => {
                    match changed {
                        Ok(()) => {
                            let state = *self.primary_state_receiver.borrow_and_update();
                            self.invalidate_stale_handoff(state.role_generation, &mut outbound, &mut pending_send);
                            self.primary_active = state.active;
                            self.session_ready = state.session_ready;
                            self.pacer.set_primary(state.active).await?;
                        }
                        Err(_) => primary_state_open = false,
                    }
                }
                _ = ack_timer.tick() => {
                    ack_timer_due = self.circuit.has_pending_acks();
                    preempt_deferred_send(
                        &mut pending_send,
                        &mut outbound,
                        &self.circuit,
                        ack_timer_due || finish_after_ack,
                    )?;
                }
                permit = events.reserve(), if event_delivery_turn && (!pending_notifications.is_empty() || !pending_events.is_empty()) => {
                    match permit {
                        Ok(permit) => {
                            if let Some(notification) = pending_notifications.pop_front() {
                                permit.send(notification);
                            } else if let Some(event) = pending_events.pop_front() {
                                permit.send(NeighborEvent::Simulator(event));
                            }
                            event_delivery_turn = false;
                        }
                        Err(_) => return Ok(()),
                    }
                }
                _ = ping_timer.tick(), if pending_send.is_none() && !logout_sent && established && (self.role == ConnectionRole::Neighbor || self.initial_movement_sent) => {
                    if let Some(prepared) = self.circuit.prepare_ping(Instant::now())? {
                        pending_send = Some(begin_send(
                            Arc::clone(&self.socket),
                            self.pacer.handle(),
                            prepared,
                            TrafficClass::Control,
                            None,
                            #[cfg(test)]
                            outbound.probe.clone(),
                        ));
                    }
                }
                command = commands.recv(), if !commands.is_closed() && pending_notifications.is_empty() => {
                    match command {
                        Some(NeighborCommand::Send { message, delivery, completed, cancel_target, report_failure }) => {
                            let message_name = message.name();
                            let message_traffic_class = message_class(&message);
                            let mut failed_completion = completed;
                            let mut failed_cancel_target = cancel_target;
                            let mut failed_report = report_failure;
                            let error = if closing {
                                Some(Error::SimulatorConnectionClosed)
                            } else if !established {
                                Some(Error::SimulatorNotReady)
                            } else {
                                let queued = QueuedMessage {
                                    class: message_traffic_class,
                                    message,
                                    delivery,
                                    queued_at: Instant::now(),
                                    completion: failed_completion.take(),
                                    cancel_target: failed_cancel_target.take(),
                                    report_failure,
                                    movement: MovementSend::None,
                                };
                                match outbound.push(queued, active_queue_class(pending_send.as_ref())) {
                                    Ok(()) => {
                                        preempt_deferred_send(
                                            &mut pending_send,
                                            &mut outbound,
                                            &self.circuit,
                                            ack_timer_due || finish_after_ack,
                                        )?;
                                        None
                                    }
                                    Err(mut queued) => {
                                        failed_completion = queued.completion.take();
                                        failed_cancel_target = queued.cancel_target.take();
                                        failed_report = queued.report_failure;
                                        Some(Error::SimulatorCommandQueueFull)
                                    }
                                }
                            };
                            if let Some(error) = error {
                                if failed_report && matches!(&error, Error::SimulatorCommandQueueFull) {
                                    let active_class = active_queue_class(pending_send.as_ref());
                                    let noncontrol_sends = outbound.messages.iter().filter(|queued| queued.class != TrafficClass::Control).count()
                                        + usize::from(active_class.is_some_and(|class| class != TrafficClass::Control));
                                    tracing::trace!(
                                        simulator_id = self.simulator_id.get(),
                                        message = message_name,
                                        delivery = ?delivery,
                                        traffic_class = ?message_traffic_class,
                                        queue_stage = "outbound",
                                        queued_sends = outbound.messages.len(),
                                        active_send = pending_send.is_some(),
                                        queued_noncontrol_sends = noncontrol_sends,
                                        queued_bytes = outbound.charged_bytes,
                                        max_pending_sends = MAX_PENDING_SENDS,
                                        max_noncontrol_sends = MAX_NONCONTROL_SENDS,
                                        max_pending_bytes = MAX_PENDING_SEND_BYTES,
                                        "Simulator send rejected by the bounded outbound queue"
                                    );
                                }
                                let message = error.to_string();
                                if let Some(completed) = failed_completion {
                                    let _ = completed.send(Err(error));
                                }
                                if failed_report || failed_cancel_target.is_some() {
                                    queue_send_notification(
                                        &events,
                                        &mut pending_notifications,
                                        self.simulator_id,
                                        failed_cancel_target,
                                        Some(message),
                                        failed_report,
                                    )?;
                                }
                            }
                        }
                        Some(NeighborCommand::PrepareHandoff(target)) => {
                            if Some(target.region_handle) != self.region_handle
                                || target.endpoint != self.session.simulator_address
                                || target.agent_id != self.session.agent_id
                                || target.session_id != self.session.session_id
                            {
                                continue;
                            }
                            let role_generation = self.current_primary_state().role_generation;
                            self.invalidate_stale_handoff(role_generation, &mut outbound, &mut pending_send);
                            if self.handoff.as_ref() == Some(&target) {
                                continue;
                            }
                            self.clear_handoff(&mut outbound, &mut pending_send);
                            self.session.seed_capability = target.seed_capability.clone();
                            self.handoff = Some(target);
                            self.handoff_role_generation = Some(role_generation);
                            if established && saw_region_handshake {
                                self.queue_handoff_movement(&mut outbound, pending_send.as_ref())?;
                            }
                        }
                        Some(NeighborCommand::CancelHandoff) => {
                            self.clear_handoff(&mut outbound, &mut pending_send);
                        }
                        None => return Ok(()),
                    }
                }
                received = self.socket.recv(&mut receive_buffer) => {
                    event_delivery_turn = !pending_notifications.is_empty() || !pending_events.is_empty();
                    let size = received.map_err(|error| format!("Neighbor simulator connection failed: {error}"))?;
                    if *shutdown.borrow() || events.is_closed() {
                        return Ok(());
                    }
                    let frame = match Packet::inspect(&receive_buffer[..size]) {
                        Ok(frame) => frame,
                        Err(_) => {
                            record_coverage_loss(
                                &self.coverage_losses,
                                self.simulator_id,
                                CoverageSource::Lludp,
                                CoverageLossReason::PreflightRejected,
                            );
                            self.socket.record_received(&receive_buffer[..size], None);
                            continue;
                        }
                    };
                    let admission = match self.circuit.prepare_receive(
                        frame.sequence(),
                        frame.is_reliable(),
                        frame.appended_acks(),
                        Instant::now(),
                    ) {
                        Ok(admission) => admission,
                        Err(error) => {
                            tracing::debug!(simulator_id = self.simulator_id.get(), reason = %error, "Neighbor packet rejected before decode admission");
                            record_coverage_loss(
                                &self.coverage_losses,
                                self.simulator_id,
                                CoverageSource::Lludp,
                                CoverageLossReason::AcknowledgementAdmission,
                            );
                            self.socket.record_received(&receive_buffer[..size], None);
                            continue;
                        }
                    };
                    if matches!(admission, ReceiveAdmission::Duplicate(_)) {
                        preempt_deferred_send(
                            &mut pending_send,
                            &mut outbound,
                            &self.circuit,
                            ack_timer_due || finish_after_ack,
                        )?;
                        self.socket.record_received(&receive_buffer[..size], None);
                        last_udp_received = Instant::now();
                        continue;
                    }
                    let message_id = match frame.message_id() {
                        Ok(id) => id,
                        Err(_) => {
                            record_coverage_loss(
                                &self.coverage_losses,
                                self.simulator_id,
                                CoverageSource::Lludp,
                                CoverageLossReason::PreflightRejected,
                            );
                            self.socket.record_received(&receive_buffer[..size], None);
                            continue;
                        }
                    };
                    let is_packet_ack = message_id == PACKET_ACK;
                    let decode_permit = if is_packet_ack {
                        None
                    } else {
                        let Some(permit) = packet_memory.try_acquire_decode_slot() else {
                            tracing::debug!(simulator_id = self.simulator_id.get(), "Neighbor packet rejected because all decoder slots are occupied");
                            record_coverage_loss(
                                &self.coverage_losses,
                                self.simulator_id,
                                CoverageSource::Lludp,
                                CoverageLossReason::DecodeAdmission,
                            );
                            self.socket.record_received(&receive_buffer[..size], None);
                            continue;
                        };
                        Some(permit)
                    };
                    let preflighted = match frame.preflighted() {
                        Ok(preflighted) => preflighted,
                        Err(error) => {
                            if matches!(error, LLUdpError::DecodedPacketTooLarge | LLUdpError::AllocationBoundOverflow) {
                                tracing::debug!(simulator_id = self.simulator_id.get(), reason = %error, "Neighbor packet rejected by decoded-memory preflight");
                            }
                            record_coverage_loss(
                                &self.coverage_losses,
                                self.simulator_id,
                                CoverageSource::Lludp,
                                preflight_loss_reason(error, message_id),
                            );
                            self.socket.record_received(&receive_buffer[..size], None);
                            drop(decode_permit);
                            continue;
                        }
                    };
                    let preflight = preflighted.preflight();
                    let _ack_decode_permit = if is_packet_ack {
                        let Some(permit) = packet_memory.try_acquire_ack_decode_slot(preflight) else {
                            tracing::debug!(simulator_id = self.simulator_id.get(), "PacketAck rejected because the control decode budget is full");
                            record_coverage_loss(
                                &self.coverage_losses,
                                self.simulator_id,
                                CoverageSource::Lludp,
                                CoverageLossReason::DecodeAdmission,
                            );
                            self.socket.record_received(&receive_buffer[..size], None);
                            drop(decode_permit);
                            continue;
                        };
                        Some(permit)
                    } else {
                        None
                    };
                    let mut memory_reservation = if is_packet_ack {
                        None
                    } else {
                        match packet_memory.reserve_packet(preflight) {
                            Ok(reservation) => Some(reservation),
                            Err(error) => {
                                tracing::debug!(simulator_id = self.simulator_id.get(), ?error, "Neighbor packet rejected by retained-memory admission");
                                record_coverage_loss(
                                    &self.coverage_losses,
                                    self.simulator_id,
                                    CoverageSource::Lludp,
                                    CoverageLossReason::RetainedMemoryAdmission,
                                );
                                self.socket.record_received(&receive_buffer[..size], None);
                                drop(decode_permit);
                                continue;
                            }
                        }
                    };
                    let packet = match preflighted.decode() {
                        Ok(packet) => packet,
                        Err(error) => {
                            let reason = if matches!(error, LLUdpError::DecodedPacketTooLarge) {
                                CoverageLossReason::AllocationEstimateMismatch
                            } else {
                                CoverageLossReason::DecodeRejected
                            };
                            record_coverage_loss(
                                &self.coverage_losses,
                                self.simulator_id,
                                CoverageSource::Lludp,
                                reason,
                            );
                            self.socket.record_received(&receive_buffer[..size], None);
                            drop(memory_reservation);
                            drop(decode_permit);
                            continue;
                        }
                    };
                    if let Some(reservation) = &mut memory_reservation
                        && let Err(error) = packet_memory.trim_packet_reservation(reservation, &packet)
                    {
                        tracing::error!(simulator_id = self.simulator_id.get(), ?error, "Decoded neighbor packet exceeded its preflight reservation");
                        record_coverage_loss(
                            &self.coverage_losses,
                            self.simulator_id,
                            CoverageSource::Lludp,
                            CoverageLossReason::AllocationEstimateMismatch,
                        );
                        self.socket.record_received(&receive_buffer[..size], None);
                        drop(memory_reservation);
                        drop(decode_permit);
                        continue;
                    }
                    let message = packet.message();
                    let primary_state = self.current_primary_state();
                    self.invalidate_stale_handoff(primary_state.role_generation, &mut outbound, &mut pending_send);
                    let connection_role = primary_state.connection_role();
                    let dispatch_message = connection_message_has_application_event(
                        message,
                        connection_role,
                        self.handoff.as_ref(),
                        self.movement_requested,
                    );
                    let event_queue_full = if established {
                        pending_events.len() >= MAX_QUEUED_SIMULATOR_EVENTS
                    } else {
                        preconnect_messages.len() >= MAX_PRECONNECT_EVENTS
                    };
                    if dispatch_message && (closing || events.is_closed() || event_queue_full) {
                        if event_queue_full && !closing {
                            close_reason = Some(if established {
                                EVENT_BACKLOG_DISCONNECT_REASON
                            } else {
                                NEIGHBOR_PRECONNECT_OVERFLOW_REASON
                            });
                            closing = true;
                            teardown_deadline.get_or_insert_with(|| Instant::now() + LOGOUT_TIMEOUT);
                        }
                        record_coverage_loss(
                            &self.coverage_losses,
                            self.simulator_id,
                            CoverageSource::Lludp,
                            CoverageLossReason::EventAdmission,
                        );
                        self.socket.record_received(&receive_buffer[..size], Some(&packet));
                        last_udp_received = Instant::now();
                        drop(memory_reservation);
                        drop(decode_permit);
                        continue;
                    }
                    self.socket.record_received(&receive_buffer[..size], Some(&packet));
                    last_udp_received = Instant::now();
                    self.circuit.acknowledge_message(packet.message());
                    let receive_outcome = self.circuit.commit_receive(admission);
                    preempt_deferred_send(
                        &mut pending_send,
                        &mut outbound,
                        &self.circuit,
                        ack_timer_due || finish_after_ack,
                    )?;
                    drop(decode_permit);
                    let is_new_packet = receive_outcome.is_new_packet;
                    let dispatch_allowed = !closing;
                    if !established && !self.circuit.has_pending(self.use_circuit_sequence) {
                        established = true;
                        if !queue_neighbor_event(
                            &events,
                            &mut pending_events,
                            Event::SimulatorConnected {
                                simulator_id: self.simulator_id,
                                region_handle: self.region_handle,
                                endpoint: self.session.simulator_address,
                            },
                            &mut closing,
                            &mut teardown_deadline,
                            &mut close_reason,
                        ) {
                            return Ok(());
                        }
                        while let Some(event) = preconnect_messages.pop_front() {
                            let admission_rejected = events.is_closed() || pending_events.len() >= MAX_QUEUED_SIMULATOR_EVENTS;
                            let watermark = event.coverage_watermark();
                            if !queue_neighbor_event(
                                &events,
                                &mut pending_events,
                                event,
                                &mut closing,
                                &mut teardown_deadline,
                                &mut close_reason,
                            ) {
                                if let Some(watermark) = watermark {
                                    record_coverage_loss(
                                        &self.coverage_losses,
                                        watermark.simulator_id,
                                        watermark.source,
                                        CoverageLossReason::EventAdmission,
                                    );
                                }
                                for queued in pending_events.drain(..) {
                                    record_event_envelope_loss(
                                        &self.coverage_losses,
                                        queued,
                                        CoverageLossReason::EventAdmission,
                                    );
                                }
                                return Ok(());
                            }
                            if admission_rejected
                                && let Some(watermark) = watermark
                            {
                                record_coverage_loss(
                                    &self.coverage_losses,
                                    watermark.simulator_id,
                                    watermark.source,
                                    CoverageLossReason::EventAdmission,
                                );
                            }
                            if close_reason.is_some() {
                                for queued in preconnect_messages.drain(..) {
                                    if let Some(watermark) = queued.coverage_watermark() {
                                        record_coverage_loss(
                                            &self.coverage_losses,
                                            watermark.simulator_id,
                                            watermark.source,
                                            CoverageLossReason::EventAdmission,
                                        );
                                    }
                                }
                                break;
                            }
                        }
                        if self.role == ConnectionRole::Primary {
                            if let Some(capture) = &self.capture {
                                capture.circuit_lifecycle("use_circuit_code_acked", self.session.circuit_code);
                            }
                            queue_internal(
                                &mut outbound,
                                Message::CompleteAgentMovement(CompleteAgentMovement {
                                    agent_data: complete_agent_movement::AgentData {
                                        agent_id: self.session.agent_id,
                                        session_id: self.session.session_id,
                                        circuit_code: self.session.circuit_code,
                                    },
                                }),
                                Delivery::Reliable,
                                TrafficClass::Deadline,
                                MovementSend::Initial,
                                active_queue_class(pending_send.as_ref()),
                            )?;
                        }
                        if saw_region_handshake && self.handoff.is_some() {
                            self.queue_handoff_movement(&mut outbound, pending_send.as_ref())?;
                        }
                    }
                    if !is_new_packet {
                        continue;
                    }

                    let mut dispatch_message = true;
                    match message {
                        Message::EnableSimulator(enabled) => {
                            let info = &enabled.simulator_info;
                            let endpoint = SocketAddr::from((info.ip, info.port));
                            if let Err(error) = neighbor_requests.try_send(NeighborRequest::Enable {
                                simulator_id: self.simulator_id,
                                region_handle: info.handle,
                                endpoint,
                            }) {
                                let reason = match error {
                                    mpsc::error::TrySendError::Full(_) => "The neighbor request queue is full.",
                                    mpsc::error::TrySendError::Closed(_) => "The simulator manager has closed.",
                                };
                                return Err(reason.into());
                            }
                        }
                        Message::DisableSimulator(_) => {
                            closing = true;
                            teardown_deadline.get_or_insert_with(|| Instant::now() + LOGOUT_TIMEOUT);
                            dispatch_message = false;
                        }
                        Message::RegionHandshake(_) => {
                            saw_region_handshake = true;
                            queue_internal(
                                &mut outbound,
                                Message::RegionHandshakeReply(RegionHandshakeReply {
                                    agent_data: region_handshake_reply::AgentData {
                                        agent_id: self.session.agent_id,
                                        session_id: self.session.session_id,
                                    },
                                    region_info: region_handshake_reply::RegionInfo { flags: 0 },
                                }),
                                Delivery::Reliable,
                                TrafficClass::Control,
                                MovementSend::None,
                                active_queue_class(pending_send.as_ref()),
                            )?;
                            if established && self.handoff.is_some() {
                                self.queue_handoff_movement(&mut outbound, pending_send.as_ref())?;
                            }
                        }
                        Message::AgentMovementComplete(message) => {
                            let matches_current_movement = match connection_role {
                                ConnectionRole::Primary => {
                                    message.agent_data.agent_id == self.session.agent_id
                                        && message.agent_data.session_id == self.session.session_id
                                }
                                ConnectionRole::Neighbor => self.handoff.as_ref().is_some_and(|handoff| {
                                    self.movement_requested
                                        && message.agent_data.agent_id == handoff.agent_id
                                        && message.agent_data.session_id == handoff.session_id
                                        && message.data.region_handle == handoff.region_handle
                                }),
                            };
                            if !matches_current_movement {
                                dispatch_message = false;
                            } else {
                                if connection_role == ConnectionRole::Primary {
                                    self.region_handle = Some(message.data.region_handle);
                                }
                                if dispatch_allowed && !self.agent_throttle_sent {
                                queue_internal(
                                    &mut outbound,
                                    agent_throttle_message(&self.session),
                                    Delivery::Reliable,
                                    TrafficClass::Control,
                                    MovementSend::None,
                                    active_queue_class(pending_send.as_ref()),
                                )?;
                                self.agent_throttle_sent = true;
                                }
                            }
                        }
                        Message::StartPingCheck(message) => {
                            dispatch_message = false;
                            self.circuit.clear_duplicate_list(message.oldest_unacked, Instant::now());
                            queue_internal(
                                &mut outbound,
                                Message::CompletePingCheck(CompletePingCheck {
                                    ping_id: PingId { ping_id: message.ping_id.ping_id },
                                }),
                                Delivery::Unreliable,
                                TrafficClass::Control,
                                MovementSend::None,
                                active_queue_class(pending_send.as_ref()),
                            )?;
                        }
                        Message::CompletePingCheck(message) => {
                            dispatch_message = false;
                            self.circuit.record_ping_response(message.ping_id.ping_id, Instant::now());
                        }
                        Message::LogoutReply(message)
                            if closing
                                && message.agent_data.agent_id == self.session.agent_id
                                && message.agent_data.session_id == self.session.session_id =>
                        {
                            dispatch_message = false;
                            finish_after_ack = true;
                            closing = true;
                        }
                        Message::PacketAck(_) => dispatch_message = false,
                        _ => {}
                    }
                    preempt_deferred_send(
                        &mut pending_send,
                        &mut outbound,
                        &self.circuit,
                        ack_timer_due || finish_after_ack,
                    )?;

                    if dispatch_message && dispatch_allowed {
                        let event = EventEnvelope::with_retention_and_coverage(
                            Event::SimulatorMessage {
                                simulator_id: self.simulator_id,
                                source: self.session.simulator_address,
                                sequence: packet.sequence(),
                                packet_order: receive_outcome.packet_order,
                                message: packet.into_message(),
                            },
                            memory_reservation.take().expect("application packet was admitted").into_retention(),
                            self.coverage_losses.watermark(self.simulator_id, CoverageSource::Lludp),
                        );
                        if established {
                            let admission_rejected = events.is_closed() || pending_events.len() >= MAX_QUEUED_SIMULATOR_EVENTS;
                            let watermark = event.coverage_watermark();
                            if !queue_neighbor_event(
                                &events,
                                &mut pending_events,
                                event,
                                &mut closing,
                                &mut teardown_deadline,
                                &mut close_reason,
                            ) {
                                if let Some(watermark) = watermark {
                                    record_coverage_loss(
                                        &self.coverage_losses,
                                        watermark.simulator_id,
                                        watermark.source,
                                        CoverageLossReason::EventAdmission,
                                    );
                                }
                                for queued in pending_events.drain(..) {
                                    record_event_envelope_loss(
                                        &self.coverage_losses,
                                        queued,
                                        CoverageLossReason::EventAdmission,
                                    );
                                }
                                return Ok(());
                            }
                            if admission_rejected
                                && let Some(watermark) = watermark
                            {
                                record_coverage_loss(
                                    &self.coverage_losses,
                                    watermark.simulator_id,
                                    watermark.source,
                                    CoverageLossReason::EventAdmission,
                                );
                            }
                        } else if preconnect_messages.len() < MAX_PRECONNECT_EVENTS {
                            preconnect_messages.push_back(event);
                        } else {
                            close_reason = Some(NEIGHBOR_PRECONNECT_OVERFLOW_REASON);
                            closing = true;
                            teardown_deadline.get_or_insert_with(|| Instant::now() + LOGOUT_TIMEOUT);
                            if let Some(watermark) = event.coverage_watermark() {
                                record_coverage_loss(
                                    &self.coverage_losses,
                                    watermark.simulator_id,
                                    watermark.source,
                                    CoverageLossReason::EventAdmission,
                                );
                            }
                            for queued in preconnect_messages.drain(..) {
                                if let Some(watermark) = queued.coverage_watermark() {
                                    record_coverage_loss(
                                        &self.coverage_losses,
                                        watermark.simulator_id,
                                        watermark.source,
                                        CoverageLossReason::EventAdmission,
                                    );
                                }
                            }
                        };
                    } else if dispatch_message && is_new_packet {
                        record_coverage_loss(
                            &self.coverage_losses,
                            self.simulator_id,
                            CoverageSource::Lludp,
                            CoverageLossReason::EventAdmission,
                        );
                    }
                }
                _ = yield_now(), if !event_delivery_turn && (!pending_notifications.is_empty() || !pending_events.is_empty()) => {
                    event_delivery_turn = true;
                }
            }
        }
        }
        .await;
        flush_send_notifications(&events, &mut pending_notifications, &mut shutdown).await;
        result
    }

    fn queue_handoff_movement(&mut self, outbound: &mut OutboundQueue, pending_send: Option<&PendingSend>) -> Result<(), String> {
        if self.movement_requested
            || self.handoff.is_none()
            || outbound.has_handoff_movement()
            || pending_send
                .is_some_and(|pending| pending.queued_message.as_ref().is_some_and(|queued| queued.movement == MovementSend::Handoff))
        {
            return Ok(());
        }
        queue_internal(
            outbound,
            Message::CompleteAgentMovement(CompleteAgentMovement {
                agent_data: complete_agent_movement::AgentData {
                    agent_id: self.session.agent_id,
                    session_id: self.session.session_id,
                    circuit_code: self.session.circuit_code,
                },
            }),
            Delivery::Reliable,
            TrafficClass::Deadline,
            MovementSend::Handoff,
            active_queue_class(pending_send),
        )
    }
}

fn preflight_loss_reason(error: LLUdpError, message_id: MessageId) -> CoverageLossReason {
    match error {
        LLUdpError::DecodedPacketTooLarge | LLUdpError::AllocationBoundOverflow => CoverageLossReason::DecodeLimit,
        LLUdpError::InvalidMessageId if Message::allocation_strategy(message_id) == AllocationStrategy::Specialized => {
            CoverageLossReason::AllocationEstimateMismatch
        }
        _ => CoverageLossReason::PreflightRejected,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        future, io,
        iter::once,
        net::SocketAddr,
        num::NonZeroU64,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use base::UUID;
    use protocol::{
        ChatType,
        lludp::{Message, Packet},
        messages::{
            AgentMovementComplete, ChatFromViewer, CompletePingCheck, PacketAck, PingId, agent_movement_complete, chat_from_viewer,
            packet_ack::Packets,
        },
    };
    use testing::{lludp::vectors, parse_hex_fixture};
    use tokio::{
        net::UdpSocket,
        spawn,
        sync::{mpsc, oneshot, watch},
        task::JoinHandle,
        time::{advance, timeout},
    };
    use url::Url;

    use crate::{
        AgentSession, Grid,
        coverage::{CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageSource},
        error::Error,
        event::{Event, SimulatorId},
        status::NetworkStatusHandle,
        transport::{
            lludp::{MAX_PENDING_PACKETS, NEW_RELIABLE_ACK_CAPACITY},
            pacing::OutboundBudget,
        },
    };

    use super::super::handoff::{HandoffEvidence, HandoffGeneration, HandoffState, PendingPromotion};
    use super::super::{
        MAX_CONCURRENT_PACKET_DECODES, MAX_NEIGHBOR_CONNECTION_EVENT_BYTES, MAX_QUEUED_NEIGHBOR_EVENT_BYTES,
        MAX_QUEUED_SIMULATOR_EVENT_BYTES, SimulatorMemoryBudget,
    };
    use super::*;

    const WAIT: Duration = Duration::from_secs(2);

    #[tokio::test]
    async fn primary_sends_agent_throttle_after_matching_movement_complete() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let simulator_address = simulator.local_addr().expect("read simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let mut agent_session = session(simulator_address);
        agent_session.agent_id = UUID::from_bytes([1; 16]);
        agent_session.session_id = UUID::from_bytes([2; 16]);
        agent_session.circuit_code = 0x1234_5678;
        let budget = OutboundBudget::new(crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
        let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::PRIMARY_CONNECTING);
        let connection = NeighborConnection::connect_primary_with_state(
            simulator_id,
            agent_session.clone(),
            None,
            NetworkStatusHandle::default(),
            CoverageLossLedger::new(),
            primary_state_receiver,
            budget.handle(),
            None,
        )
        .await
        .expect("connect primary");
        let (events, mut received_events) = mpsc::channel(8);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (requests, _requests) = mpsc::channel(1);
        let (_commands, command_receiver) = mpsc::channel(1);
        let (_logout, logout_receiver) = mpsc::channel(1);
        let worker =
            spawn(connection.run(events, shutdown_receiver, requests, command_receiver, logout_receiver, SimulatorMemoryBudget::new()));

        let (use_circuit, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit.sequence()).await;
        receive_connected(&mut received_events).await;
        let (movement_request, _) = receive_packet(&simulator).await;
        assert!(matches!(movement_request.message(), Message::CompleteAgentMovement(_)));
        send_ack(&simulator, peer, movement_request.sequence()).await;

        let movement_sequence = 0x0102_0304;
        let movement = AgentMovementComplete {
            agent_data: agent_movement_complete::AgentData { agent_id: agent_session.agent_id, session_id: agent_session.session_id },
            data: agent_movement_complete::Data { region_handle: 42, ..Default::default() },
            sim_data: agent_movement_complete::SimData::default(),
        };
        let bytes = Packet::new(movement_sequence, Message::AgentMovementComplete(movement))
            .with_reliable()
            .encode()
            .expect("encode movement completion");
        simulator.send_to(&bytes, peer).await.expect("send movement completion");
        assert!(matches!(
            receive_message_event(&mut received_events, movement_sequence).await,
            Event::SimulatorMessage { message: Message::AgentMovementComplete(_), .. }
        ));

        let (throttle, _) = receive_packet(&simulator).await;
        assert!(throttle.is_reliable(), "AgentThrottle must use LLUDP reliable delivery");
        let Message::AgentThrottle(throttle_message) = throttle.message() else { panic!("expected AgentThrottle") };
        assert_eq!(throttle_message.agent_data.agent_id, agent_session.agent_id);
        assert_eq!(throttle_message.agent_data.session_id, agent_session.session_id);
        assert_eq!(throttle_message.agent_data.circuit_code, agent_session.circuit_code);
        assert_eq!(throttle_message.throttle.gen_counter, 0);
        assert_eq!(throttle_message.throttle.throttles, DEFAULT_AGENT_THROTTLE_RATES);

        shutdown.send_replace(true);
        assert!(timeout(WAIT, worker).await.expect("join connection worker").expect("connection worker").is_ok());
        drop(primary_state);
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn demoted_primary_times_out() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let simulator_id = SimulatorId::allocate().expect("simulator ID");
        let budget = OutboundBudget::new(crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
        let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::PRIMARY_CONNECTING);
        let connection = NeighborConnection::connect_primary_with_state(
            simulator_id,
            session(simulator.local_addr().expect("simulator address")),
            None,
            NetworkStatusHandle::default(),
            CoverageLossLedger::new(),
            primary_state_receiver,
            budget.handle(),
            None,
        )
        .await
        .expect("connect primary");
        let (events, mut received) = mpsc::channel(4);
        let (_shutdown, shutdown) = watch::channel(false);
        let (requests, _requests) = mpsc::channel(1);
        let (commands, command_receiver) = mpsc::channel(2);
        commands.try_send(NeighborCommand::CancelHandoff).expect("fill first command slot");
        commands.try_send(NeighborCommand::CancelHandoff).expect("fill second command slot");
        let (_logout, logout) = mpsc::channel(1);
        primary_state.send_replace(PrimaryState::demoted(1));
        drop(primary_state);
        let worker = spawn(connection.run(events, shutdown, requests, command_receiver, logout, SimulatorMemoryBudget::new()));

        let (circuit, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, circuit.sequence()).await;
        receive_connected(&mut received).await;
        let (movement, _) = receive_packet(&simulator).await;
        assert!(matches!(movement.message(), Message::CompleteAgentMovement(_)));
        send_ack(&simulator, peer, movement.sequence()).await;

        let (completed, result) = oneshot::channel();
        commands
            .send(NeighborCommand::Send {
                message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 9 } }),
                delivery: Delivery::Unreliable,
                completed: Some(completed),
                cancel_target: None,
                report_failure: false,
            })
            .await
            .expect("queue command after demotion");
        assert!(timeout(WAIT, result).await.expect("send is not starved after role watch closes").expect("demotion barrier").is_ok());
        let _ = receive_packet(&simulator).await;
        let ping = parse_hex_fixture(vectors::START_PING_CHECK).expect("ping fixture");
        simulator.send_to(&ping, peer).await.expect("send activity barrier");
        let (reply, _) = receive_packet(&simulator).await;
        assert!(matches!(reply.message(), Message::CompletePingCheck(_)));

        advance(KEEPALIVE_TIMEOUT).await;
        let result = timeout(WAIT, worker).await.expect("demoted connection expires").expect("connection worker");
        assert_eq!(result, Err("The simulator stopped responding to LLUDP keepalive traffic.".into()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn promoted_logout_bypasses_full_queue() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let simulator_address = simulator.local_addr().expect("simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let agent_session = session(simulator_address);
        let agent_id = agent_session.agent_id;
        let session_id = agent_session.session_id;
        let budget = OutboundBudget::new(1);
        let pacer = budget.register(simulator_id.get(), false).await.expect("register simulator pacer");
        let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::NEIGHBOR);
        let promote_connection = primary_state.clone();
        let mut connection = NeighborConnection::connect_with_role(
            simulator_id,
            agent_session,
            Some(42),
            ConnectionRole::Neighbor,
            None,
            NetworkStatusHandle::default(),
            CoverageLossLedger::new(),
            Some(primary_state_receiver),
            pacer,
            None,
            None,
        )
        .await
        .expect("connect primary");
        let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
        Arc::get_mut(&mut connection.socket).expect("test connection owns its socket").set_send_gate(send_gate);
        let (events, mut received_events) = mpsc::channel(16);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (requests, _requests) = mpsc::channel(1);
        let (commands, command_receiver) = mpsc::channel(16);
        let (logout_commands, logout_receiver) = mpsc::channel(1);
        let worker =
            spawn(connection.run(events, shutdown_receiver, requests, command_receiver, logout_receiver, SimulatorMemoryBudget::new()));

        let (use_circuit, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit.sequence()).await;
        receive_connected(&mut received_events).await;
        promote_connection.send_replace(PrimaryState::PRIMARY_READY);

        let (ready_completed, ready_response) = oneshot::channel();
        commands
            .send(NeighborCommand::Send {
                message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 8 } }),
                delivery: Delivery::Unreliable,
                completed: Some(ready_completed),
                cancel_target: None,
                report_failure: false,
            })
            .await
            .expect("queue a send after promotion");
        assert!(ready_response.await.expect("promoted send completion").is_ok());
        let (promoted_send, _) = receive_packet(&simulator).await;
        assert!(matches!(promoted_send.message(), Message::CompletePingCheck(_)));

        let make_chat = |message: String| {
            Message::ChatFromViewer(ChatFromViewer {
                agent_data: chat_from_viewer::AgentData { agent_id, session_id },
                chat_data: chat_from_viewer::ChatData { message, chat_type: ChatType::Normal, channel: 0 },
            })
        };
        let first = make_chat("x".repeat(7_900));
        let wire_bytes = Packet::new(100, first.clone()).encode().expect("encode burst-filling chat");
        assert!(wire_bytes.len() < 8 * 1024 && wire_bytes.len() > 7_500, "fixture should consume most of the initial pacing burst");
        commands
            .send(NeighborCommand::Send {
                message: first,
                delivery: Delivery::Unreliable,
                completed: None,
                cancel_target: None,
                report_failure: false,
            })
            .await
            .expect("queue first chat");
        let (first_sent, _) = receive_packet(&simulator).await;
        assert!(matches!(first_sent.message(), Message::ChatFromViewer(_)));

        let (pending_completed, pending_response) = oneshot::channel();
        commands
            .send(NeighborCommand::Send {
                message: make_chat("y".repeat(7_900)),
                delivery: Delivery::Unreliable,
                completed: Some(pending_completed),
                cancel_target: None,
                report_failure: false,
            })
            .await
            .expect("queue rate-limited chat");
        for _ in 0..MAX_PENDING_SENDS {
            commands
                .send(NeighborCommand::Send {
                    message: make_chat("queued".into()),
                    delivery: Delivery::Unreliable,
                    completed: None,
                    cancel_target: None,
                    report_failure: false,
                })
                .await
                .expect("queue ordinary chat");
        }
        let (full_completed, full_response) = oneshot::channel();
        commands
            .send(NeighborCommand::Send {
                message: make_chat("full".into()),
                delivery: Delivery::Unreliable,
                completed: Some(full_completed),
                cancel_target: None,
                report_failure: false,
            })
            .await
            .expect("send command to full outbound queue");
        assert!(matches!(
            timeout(WAIT, full_response).await.expect("full queue response").expect("completion sender"),
            Err(Error::SimulatorCommandQueueFull)
        ));

        pause_sends.send_replace(true);
        let (logout_completed, logout_response) = oneshot::channel();
        logout_commands.send(logout_completed).await.expect("request priority logout");
        timeout(WAIT, send_entered.notified()).await.expect("logout should reach the paused local send");

        let (repeated_logout_completed, mut repeated_logout_response) = oneshot::channel();
        logout_commands.send(repeated_logout_completed).await.expect("repeat priority logout");
        assert!(timeout(Duration::from_millis(25), &mut repeated_logout_response).await.is_err());

        pause_sends.send_replace(false);
        let (logout_packet, _) = receive_packet(&simulator).await;
        assert!(matches!(logout_packet.message(), Message::LogoutRequest(_)));
        assert!(logout_response.await.expect("logout completion").is_ok(), "logout local send completes despite a full ordinary queue");
        assert!(repeated_logout_response.await.expect("repeated logout completion").is_ok());
        assert!(matches!(pending_response.await, Ok(Err(Error::SimulatorConnectionClosed))));

        shutdown.send_replace(true);
        assert!(timeout(WAIT, worker).await.expect("stop connection worker").expect("join connection worker").is_ok());
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn saturated_neighbor_events_preserve_ack_and_generation_completion() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let simulator_address = simulator.local_addr().expect("simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let connection = NeighborConnection::connect_with_coverage(
            simulator_id,
            session(simulator_address),
            42,
            None,
            NetworkStatusHandle::default(),
            CoverageLossLedger::new(),
        )
        .await
        .expect("connect neighbor simulator");
        let (events, mut received_events) = mpsc::channel(1);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
        let (commands, command_receiver) = mpsc::channel(1);
        let (_logout, logout) = mpsc::channel(1);
        let task = spawn(connection.run(
            events.clone(),
            shutdown_receiver,
            neighbor_requests,
            command_receiver,
            logout,
            SimulatorMemoryBudget::new().for_neighbor(),
        ));

        let (use_circuit_code, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit_code.sequence()).await;
        receive_connected(&mut received_events).await;

        let (completed, response) = oneshot::channel();
        commands
            .send(NeighborCommand::Send {
                message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 12 } }),
                delivery: Delivery::Unreliable,
                completed: Some(completed),
                cancel_target: None,
                report_failure: true,
            })
            .await
            .expect("queue send that reports failures only");
        let (sent, _) = receive_packet(&simulator).await;
        assert!(matches!(sent.message(), Message::CompletePingCheck(_)));
        assert!(response.await.expect("local send result").is_ok());
        assert!(
            matches!(received_events.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "a successful send without a generation target must not emit a completion event"
        );

        events
            .try_send(NeighborEvent::SendCompleted { simulator_id, cancel_target: None, error: None, report_failure: false })
            .expect("fill the neighbor event channel");
        let (promotion, cancel_target) = handoff_cancellation_for_test(simulator_id, simulator_address);
        let handoff_state = HandoffState::Preparing(promotion);
        let (completed, response) = oneshot::channel();
        commands
            .send(NeighborCommand::Send {
                message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 13 } }),
                delivery: Delivery::Unreliable,
                completed: Some(completed),
                cancel_target: Some(cancel_target),
                report_failure: true,
            })
            .await
            .expect("queue reportable handoff control message");
        let (sent, _) = receive_packet(&simulator).await;
        assert!(matches!(sent.message(), Message::CompletePingCheck(_)));
        assert!(response.await.expect("local send result").is_ok());

        let reliable_sequence = 40_001_u32;
        let mut reliable_chat = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse chat fixture");
        reliable_chat[0] |= 0x40;
        reliable_chat[1..5].copy_from_slice(&reliable_sequence.to_be_bytes());
        simulator.send_to(&reliable_chat, peer).await.expect("send reliable simulator event");
        for _ in 0..16 {
            yield_now().await;
        }
        advance(ACK_INTERVAL).await;
        receive_ack(&simulator, reliable_sequence).await;

        assert!(matches!(received_events.try_recv(), Ok(NeighborEvent::SendCompleted { .. })), "remove the channel filler");
        let notification = timeout(WAIT, async {
            loop {
                if let Some(NeighborEvent::SendCompleted {
                    simulator_id: completed_id,
                    cancel_target: Some(actual),
                    error,
                    report_failure,
                }) = received_events.recv().await
                {
                    assert_eq!(completed_id, simulator_id);
                    assert!(error.is_none(), "successful local send has no error");
                    assert!(!report_failure, "successful send completion does not report a failure");
                    assert!(super::super::handoff::matches_handoff_cancellation(&handoff_state, actual));
                    break;
                }
            }
        })
        .await;
        assert!(notification.is_ok(), "the generation-scoped send completion must drain after capacity returns");

        shutdown.send_replace(true);
        assert!(timeout(WAIT, task).await.expect("join connection task").expect("connection task").is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn saturated_event_channel_delivers_expired_send_failure() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let simulator_address = simulator.local_addr().expect("simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let mut connection = NeighborConnection::connect_with_coverage(
            simulator_id,
            session(simulator_address),
            42,
            None,
            NetworkStatusHandle::default(),
            CoverageLossLedger::new(),
        )
        .await
        .expect("connect neighbor simulator");
        let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
        Arc::get_mut(&mut connection.socket).expect("connection owns socket").set_send_gate(send_gate);
        let (events, mut received_events) = mpsc::channel(1);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
        let (commands, command_receiver) = mpsc::channel(1);
        let (_logout, logout) = mpsc::channel(1);
        let task = spawn(connection.run(
            events.clone(),
            shutdown_receiver,
            neighbor_requests,
            command_receiver,
            logout,
            SimulatorMemoryBudget::new().for_neighbor(),
        ));

        let (use_circuit_code, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit_code.sequence()).await;
        receive_connected(&mut received_events).await;
        events
            .try_send(NeighborEvent::SendCompleted { simulator_id, cancel_target: None, error: None, report_failure: false })
            .expect("fill the neighbor event channel");

        let (promotion, cancel_target) = handoff_cancellation_for_test(simulator_id, simulator_address);
        let handoff_state = HandoffState::Preparing(promotion);
        pause_sends.send_replace(true);
        let (completed, response) = oneshot::channel();
        commands
            .send(NeighborCommand::Send {
                message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 14 } }),
                delivery: Delivery::Unreliable,
                completed: Some(completed),
                cancel_target: Some(cancel_target),
                report_failure: true,
            })
            .await
            .expect("queue reportable send");
        timeout(WAIT, send_entered.notified()).await.expect("send should enter the gate");
        advance(MAX_SEND_AGE).await;
        assert!(matches!(response.await.expect("expired local send result"), Err(Error::SimulatorSend(_))));

        pause_sends.send_replace(false);
        let reliable_sequence = 40_002_u32;
        let mut reliable_chat = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse chat fixture");
        reliable_chat[0] |= 0x40;
        reliable_chat[1..5].copy_from_slice(&reliable_sequence.to_be_bytes());
        simulator.send_to(&reliable_chat, peer).await.expect("send reliable simulator event while notification is buffered");
        for _ in 0..16 {
            yield_now().await;
        }
        advance(ACK_INTERVAL).await;
        receive_ack(&simulator, reliable_sequence).await;

        assert!(matches!(received_events.try_recv(), Ok(NeighborEvent::SendCompleted { .. })), "remove the channel filler");
        timeout(WAIT, async {
            loop {
                if let Some(NeighborEvent::SendCompleted {
                    simulator_id: completed_id,
                    cancel_target: Some(actual),
                    error,
                    report_failure,
                }) = received_events.recv().await
                {
                    assert_eq!(completed_id, simulator_id);
                    assert!(error.is_some(), "expired send reports its failure");
                    assert!(report_failure);
                    assert!(super::super::handoff::matches_handoff_cancellation(&handoff_state, actual));
                    break;
                }
            }
        })
        .await
        .expect("send failure notification should be delivered after channel capacity returns");

        shutdown.send_replace(true);
        assert!(timeout(WAIT, task).await.expect("join connection task").expect("connection task").is_ok());
    }

    fn session(simulator_address: SocketAddr) -> AgentSession {
        AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address,
            agent_id: UUID::nil(),
            session_id: UUID::nil(),
            circuit_code: 1,
            seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("valid seed URL"),
        }
    }

    fn handoff_cancellation_for_test(
        source: SimulatorId,
        endpoint: SocketAddr,
    ) -> (PendingPromotion, super::super::handoff::HandoffCancellation) {
        let handoff = HandoffTarget {
            agent_id: UUID::nil(),
            session_id: UUID::nil(),
            region_handle: 42,
            endpoint,
            seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("valid seed URL"),
        };
        let pending = PendingPromotion {
            source,
            target: SimulatorId::allocate().expect("allocate target simulator ID"),
            generation: HandoffGeneration::new(NonZeroU64::new(7).expect("nonzero generation")),
            handoff,
            evidence: HandoffEvidence::default(),
        };
        let cancellation = super::super::handoff::handoff_cancellation(&pending);
        (pending, cancellation)
    }

    fn queued_for_test(class: TrafficClass) -> QueuedMessage {
        queued_with_delivery_for_test(class, Delivery::Unreliable)
    }

    fn queued_with_delivery_for_test(class: TrafficClass, delivery: Delivery) -> QueuedMessage {
        QueuedMessage {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }),
            delivery,
            class,
            queued_at: Instant::now(),
            completion: None,
            cancel_target: None,
            report_failure: false,
            movement: MovementSend::None,
        }
    }

    fn pending_for_test(circuit: &Circuit, queued: QueuedMessage, class: TrafficClass) -> PendingSend {
        let prepared = circuit.prepare_message(queued.message.clone(), queued.delivery).expect("prepare test datagram");
        let meta = PreparedMeta::from(&prepared);
        let result: PendingSendFuture = Box::pin(future::pending());
        PendingSend { meta, prepared, result, class, queued_message: Some(queued), retry_deadline: None, probe: None }
    }

    #[test]
    fn active_send_uses_queue_capacity() {
        let mut queue = OutboundQueue::new();
        for _ in 0..MAX_NONCONTROL_SENDS {
            assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
        }
        assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());

        assert!(queue.push(queued_for_test(TrafficClass::Control), Some(TrafficClass::Control)).is_err());
        assert_eq!(queue.charged_bytes, (MAX_PENDING_SENDS - 1) * OUTBOUND_SEND_CHARGE_BYTES);
    }

    #[test]
    fn control_sends_keep_reserved_capacity() {
        let mut queue = OutboundQueue::new();
        for _ in 0..MAX_NONCONTROL_SENDS {
            assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
        }

        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_err());
        assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());
        assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());
        assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn full_reliable_window_keeps_reliable_work_queued_until_ack() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
        let budget = OutboundBudget::new(8_000);
        let pacer = budget.register(1, false).await.expect("register simulator pacer");
        let mut circuit = Circuit::new();
        let message = queued_for_test(TrafficClass::Fair).message;
        let mut first_sequence = None;
        for _ in 0..MAX_PENDING_PACKETS {
            let prepared = circuit.prepare_message(message.clone(), Delivery::Reliable).expect("prepare reliable packet");
            first_sequence.get_or_insert(prepared.sequence());
            circuit.commit_sent(prepared, Instant::now()).expect("record reliable packet");
        }
        assert!(!circuit.has_reliable_capacity());

        let mut queue = OutboundQueue::new();
        assert!(queue.push(queued_with_delivery_for_test(TrafficClass::Fair, Delivery::Reliable), None).is_ok());
        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
        let mut deferred_retries = HashMap::new();
        let untracked = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("untracked work should remain eligible")
            .expect("untracked work should bypass a blocked reliable packet");
        let packet = Packet::decode(untracked.prepared.bytes()).expect("decode untracked datagram");
        assert!(!packet.is_reliable());
        drop(untracked);
        assert_eq!(queue.messages.len(), 1, "tracked reliable work remains bounded in the local queue");
        assert!(queue.messages.front().expect("reliable work remains").delivery.tracks_reliable());
        assert!(
            prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
                .map_err(|(_, error)| error)
                .expect("a full window defers queued reliable work")
                .is_none()
        );

        let ack = circuit
            .prepare_receive(50_000, false, [first_sequence.expect("at least one packet")], Instant::now())
            .expect("process ACK for the first outstanding packet");
        circuit.commit_receive(ack);
        let reliable = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("send queued reliable work after ACK")
            .expect("the ACK should open one in-flight slot");
        let packet = Packet::decode(reliable.prepared.bytes()).expect("decode reliable datagram");
        assert!(packet.is_reliable());
        assert!(queue.messages.is_empty());

        drop(reliable);
        drop(pacer);
        budget.shutdown().await;
    }

    #[test]
    fn paced_send_expires() {
        let circuit = Circuit::new();
        let queued = queued_for_test(TrafficClass::Fair);
        let expired_at = queued.queued_at + MAX_SEND_AGE;
        let mut pending = Some(pending_for_test(&circuit, queued, TrafficClass::Fair));

        let PendingSendExpiration::QueuedMessage(expired) = expire_pending_send(&mut pending, expired_at).expect("expired pending message")
        else {
            panic!("application message expiration should return its queue owner");
        };

        assert!(pending.is_none(), "expiration cancels the deferred pacer request");
        assert_eq!(expired.class, TrafficClass::Fair);
    }

    #[test]
    fn deferred_reliable_retry_expires() {
        let mut circuit = Circuit::new();
        let original =
            circuit.prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable).expect("prepare reliable send");
        circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("record original local send");
        let prepared = circuit.prepare_retransmission(Instant::now()).expect("prepare due retry").expect("retry should be due");
        assert!(prepared.is_retransmission());

        let started_at = Instant::now();
        let meta = PreparedMeta::from(&prepared);
        let result: PendingSendFuture = Box::pin(future::pending());
        let mut pending = Some(PendingSend {
            meta,
            prepared,
            result,
            class: TrafficClass::Fair,
            queued_message: None,
            retry_deadline: Some(started_at + MAX_SEND_AGE),
            probe: None,
        });

        let expiration = expire_pending_send(&mut pending, started_at + MAX_SEND_AGE).expect("retry age is bounded");

        assert!(matches!(expiration, PendingSendExpiration::ReliableRetry));
        assert!(pending.is_none(), "expiry cancels the pacer request");
    }

    #[tokio::test(start_paused = true)]
    async fn probe_records_class_queue_ages_and_each_expiration_once() {
        let start = Instant::now();
        let (probe, handle) = ProbeConnection::new_for_test(77);
        let mut queue = OutboundQueue::with_probe(Some(probe.clone()));
        assert!(queue.push(queued_for_test(TrafficClass::Deadline), None).is_ok());
        let deadline_queued = queue.pop_next().expect("pop deadline message into PendingSend");
        let circuit = Circuit::new();
        let mut pending = Some(pending_for_test(&circuit, deadline_queued, TrafficClass::Deadline));
        pending.as_mut().expect("pending deadline message").probe = Some(probe.clone());

        assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());
        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
        advance(MAX_SEND_AGE).await;
        let expired_at = Instant::now();

        let expired_queue = queue.expire(expired_at);
        assert_eq!(expired_queue.len(), 2, "the queued Control and Fair messages expire once each");
        assert!(matches!(
            expire_pending_send(&mut pending, expired_at),
            Some(PendingSendExpiration::QueuedMessage(QueuedMessage { class: TrafficClass::Deadline, .. }))
        ));
        assert!(pending.is_none());

        let mut deferred_circuit = Circuit::new();
        let deferred_original = deferred_circuit
            .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
            .expect("prepare deferred reliable packet");
        deferred_circuit.commit_sent(deferred_original, Instant::now() - Duration::from_secs(6)).expect("record deferred reliable packet");
        let deferred_retry = deferred_circuit
            .prepare_retransmission(Instant::now())
            .expect("prepare deferred retry")
            .expect("reliable packet is due for retry");
        let deferred_sequence = deferred_retry.sequence();
        let mut deferred_retries = HashMap::from([(deferred_sequence, Instant::now() - MAX_SEND_AGE)]);
        assert_eq!(
            expire_deferred_retries(&deferred_circuit, &mut deferred_retries, Instant::now(), None, Some(&probe),),
            Err(RELIABLE_RETRY_EXPIRED.into())
        );
        drop(deferred_retry);

        let mut active_circuit = Circuit::new();
        let active_original = active_circuit
            .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
            .expect("prepare active reliable packet");
        active_circuit.commit_sent(active_original, Instant::now() - Duration::from_secs(6)).expect("record active reliable packet");
        let active_retry =
            active_circuit.prepare_retransmission(Instant::now()).expect("prepare active retry").expect("reliable packet is due for retry");
        let active_started_at = Instant::now();
        let meta = PreparedMeta::from(&active_retry);
        let result: PendingSendFuture = Box::pin(future::pending());
        let mut active_pending = Some(PendingSend {
            meta,
            prepared: active_retry,
            result,
            class: TrafficClass::Fair,
            queued_message: None,
            retry_deadline: Some(active_started_at + MAX_SEND_AGE),
            probe: Some(probe),
        });
        advance(MAX_SEND_AGE).await;
        assert!(matches!(expire_pending_send(&mut active_pending, Instant::now()), Some(PendingSendExpiration::ReliableRetry)));

        let snapshot = handle.snapshot(start, Instant::now() + Duration::from_nanos(1));
        let connection = snapshot.connections.get(&77).expect("probe connection snapshot");
        for class in [TrafficClass::Control, TrafficClass::Deadline, TrafficClass::Fair] {
            assert_eq!(
                connection.classes.get(&class).expect("class snapshot").peak_queue_age,
                MAX_SEND_AGE,
                "the class-specific queue age includes time until expiration"
            );
        }
        assert_eq!(connection.classes[&TrafficClass::Control].outbound_queue_expirations, 1);
        assert_eq!(connection.classes[&TrafficClass::Fair].outbound_queue_expirations, 1);
        assert_eq!(connection.classes[&TrafficClass::Deadline].pending_send_expirations, 1);
        assert_eq!(connection.deferred_retry_expirations, 1);
        assert_eq!(connection.active_retry_expirations, 1);
    }

    #[tokio::test]
    async fn control_traffic_cannot_restart_retry_deadline() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
        let budget = OutboundBudget::new(8_000);
        let pacer = budget.register(1, false).await.expect("register simulator pacer");
        let mut circuit = Circuit::new();
        let original = circuit
            .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
            .expect("prepare original reliable send");
        let sequence = original.sequence();
        circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("record original local send");
        let mut queue = OutboundQueue::new();
        let mut deferred_retries = HashMap::new();

        for _ in 0..3 {
            assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());
            let control = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
                .map_err(|(_, error)| error)
                .expect("prepare priority control work")
                .expect("control work should be ready");
            assert!(!control.prepared.is_retransmission(), "queued Control work retains priority");
            circuit.commit_sent(control.prepared, Instant::now()).expect("record control send");
        }

        let deferred_since = deferred_retries[&sequence];
        assert_eq!(
            expire_deferred_retries(&circuit, &mut deferred_retries, deferred_since + MAX_SEND_AGE, None, None),
            Err(RELIABLE_RETRY_EXPIRED.into()),
            "repeated priority sends cannot extend a due retry's deadline"
        );

        drop(pacer);
        budget.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_limits_sends_to_control_and_acks() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
        let budget = OutboundBudget::new(8_000);
        let pacer = budget.register(1, false).await.expect("register simulator pacer");
        let mut circuit = Circuit::new();
        let original = circuit
            .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
            .expect("prepare original reliable send");
        circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("record original send");
        let mut queue = OutboundQueue::new();
        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());
        assert!(queue.push(queued_for_test(TrafficClass::Control), None).is_ok());

        let control = prepare_shutdown_send(&socket, &pacer, &mut circuit, &mut queue, false)
            .map_err(|(_, error)| error)
            .expect("prepare shutdown control")
            .expect("queued control should be sent");
        assert!(!control.prepared.is_retransmission());
        assert_eq!(
            Packet::decode(control.prepared.bytes()).expect("decode control").message(),
            &Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } })
        );
        drop(control);
        assert_eq!(queue.messages.len(), 1, "shutdown leaves Fair work queued");
        assert!(
            prepare_shutdown_send(&socket, &pacer, &mut circuit, &mut queue, false)
                .map_err(|(_, error)| error)
                .expect("do not retry or send Fair work")
                .is_none()
        );

        let admission = circuit.prepare_receive(42, true, [], Instant::now()).expect("prepare inbound ACK requirement");
        circuit.commit_receive(admission);
        let ack = prepare_shutdown_send(&socket, &pacer, &mut circuit, &mut queue, true)
            .map_err(|(_, error)| error)
            .expect("prepare shutdown ACK")
            .expect("pending acknowledgement should be sent");
        assert_eq!(
            Packet::decode(ack.prepared.bytes()).expect("decode ACK").message(),
            &Message::PacketAck(PacketAck { packets: vec![Packets { id: 42 }] })
        );
        drop(ack);

        drop(pacer);
        budget.shutdown().await;
    }

    #[tokio::test]
    async fn ack_piggyback_keeps_payload_class() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
        let budget = OutboundBudget::new(8_000);
        let pacer = budget.register(1, false).await.expect("register simulator pacer");
        let mut circuit = Circuit::new();
        let admission = circuit.prepare_receive(42, true, [], Instant::now()).expect("reserve reliable receive");
        circuit.commit_receive(admission);
        let mut queue = OutboundQueue::new();
        let mut deferred_retries = HashMap::new();
        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());

        let pending = match prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false) {
            Ok(Some(pending)) => pending,
            Ok(None) => panic!("pending ACK and payload should be prepared"),
            Err((_, error)) => panic!("prepare piggyback: {error}"),
        };

        assert_eq!(pending.class, TrafficClass::Fair);
        let packet = Packet::decode(pending.prepared.bytes()).expect("decode prepared packet");
        assert!(packet.appended_acks().contains(&42));
        drop(pending);
        drop(pacer);
        budget.shutdown().await;
    }

    #[tokio::test]
    async fn fair_sends_follow_retries() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
        let budget = OutboundBudget::new(8_000);
        let pacer = budget.register(1, false).await.expect("register simulator pacer");
        let mut circuit = Circuit::new();
        let original = circuit
            .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
            .expect("prepare original reliable send");
        circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("track original send");
        let admission = circuit.prepare_receive(42, true, [], Instant::now()).expect("reserve ACK");
        circuit.commit_receive(admission);
        let mut queue = OutboundQueue::new();
        let mut deferred_retries = HashMap::new();
        assert!(queue.push(queued_for_test(TrafficClass::Fair), None).is_ok());

        let retry = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("prepare retry")
            .expect("due retry should be ready");
        assert!(retry.prepared.is_retransmission());
        drop(retry);

        let fair = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, true)
            .map_err(|(_, error)| error)
            .expect("prepare Fair work after retry")
            .expect("queued Fair work should run before another retry");
        assert!(!fair.prepared.is_retransmission());
        assert_eq!(fair.class, TrafficClass::Fair);
        assert!(fair.queued_message.is_some());
        assert!(Packet::decode(fair.prepared.bytes()).expect("decode Fair payload").appended_acks().contains(&42));

        drop(fair);
        drop(pacer);
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn retry_loss_is_reported_once_per_reliable_generation() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
        let budget = OutboundBudget::new(20_000);
        let pacer = budget.register(1, false).await.expect("register simulator pacer");
        let handle = pacer.handle();
        let mut circuit = Circuit::new();
        let original = circuit
            .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
            .expect("prepare original reliable send");
        circuit.commit_sent(original, Instant::now() - Duration::from_secs(6)).expect("track original send");
        let mut queue = OutboundQueue::new();
        let mut deferred_retries = HashMap::new();

        let first = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("prepare first retry")
            .expect("first retry should be due");
        assert_eq!(handle.rate_for_test(), 10_000);
        drop(first);

        advance(Duration::from_secs(2)).await;
        let second = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("prepare deferred retry again")
            .expect("the unsent retry remains due");
        assert_eq!(handle.rate_for_test(), 10_000, "preemption does not report the same retry generation twice");
        let sequence = second.meta.sequence;
        circuit.commit_sent(second.prepared, Instant::now()).expect("commit the locally sent retry");
        deferred_retries.remove(&sequence);

        advance(Duration::from_secs(60)).await;
        let next_generation = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, false, false)
            .map_err(|(_, error)| error)
            .expect("prepare next retry generation")
            .expect("next retry should be due");
        assert_eq!(handle.rate_for_test(), 5_000, "a later retry generation reports a new loss");
        drop(next_generation);

        drop(pacer);
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn primary_priority_sends_during_neighbor_retry_storm() {
        const NEIGHBOR_COUNT: usize = 4;
        const RETRIES_PER_NEIGHBOR: usize = 64;

        let mut simulator_sockets = Vec::new();
        let mut transports = Vec::new();
        for _ in 0..=NEIGHBOR_COUNT {
            let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
            let address = simulator.local_addr().expect("simulator address");
            transports.push(Arc::new(UdpTransport::connect(address).await.expect("connect simulator transport")));
            simulator_sockets.push(simulator);
        }

        let budget = OutboundBudget::new(1_000);
        let mut pacers = Vec::new();
        for id in 1..=NEIGHBOR_COUNT + 1 {
            pacers.push(budget.register(id as u64, id == 1).await.expect("register simulator pacer"));
        }

        let burst = vec![0; crate::transport::pacing::MAX_BURST_BYTES as usize];
        pacers[0].handle().send(&transports[0], &burst, TrafficClass::Fair).await.expect("drain the initial shared pacing burst");

        let now = Instant::now();
        let original_sent_at = now - Duration::from_secs(6);
        let mut retry_workers = Vec::new();
        let mut retry_counts = Vec::new();
        for neighbor in 0..NEIGHBOR_COUNT {
            let mut retries = Vec::new();
            for offset in 0..RETRIES_PER_NEIGHBOR {
                let sequence = u32::try_from(neighbor * RETRIES_PER_NEIGHBOR + offset + 1).expect("small test sequence");
                let mut circuit = Circuit::with_sequence(sequence);
                let original = circuit
                    .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
                    .expect("prepare overdue reliable packet");
                circuit.commit_sent(original, original_sent_at).expect("track overdue reliable packet");
                let retry = circuit.prepare_retransmission(now).expect("prepare due retransmission").expect("retry is due");
                let packet = Packet::decode(retry.bytes()).expect("decode prepared retransmission");
                assert!(packet.is_reliable() && packet.is_resent(), "the storm consists of actual Circuit retry datagrams");
                retries.push((circuit, retry));
            }

            let socket = Arc::clone(&transports[neighbor + 1]);
            let pacer = pacers[neighbor + 1].handle();
            let sent = Arc::new(AtomicUsize::new(0));
            let sent_by_worker = Arc::clone(&sent);
            retry_workers.push(spawn(async move {
                for (mut circuit, retry) in retries {
                    let pending = begin_retry_send(Arc::clone(&socket), pacer.clone(), retry, Instant::now() + MAX_SEND_AGE, None);
                    let PendingSend { result, .. } = pending;
                    let (sent_at, prepared) = result.await?;
                    circuit.commit_sent(prepared, sent_at)?;
                    sent_by_worker.fetch_add(1, Ordering::Relaxed);
                }
                Ok::<(), String>(())
            }));
            retry_counts.push(sent);
        }

        let control_bytes =
            Packet::new(90_001, Message::PacketAck(PacketAck { packets: vec![Packets { id: 42 }] })).encode().expect("encode primary ACK");
        let control_pacer = pacers[0].handle();
        let control_socket = Arc::clone(&transports[0]);
        let control = spawn(async move { control_pacer.send(&control_socket, &control_bytes, TrafficClass::Control).await });

        for _ in 0..100 {
            if control.is_finished() {
                break;
            }
            advance(Duration::from_millis(10)).await;
            for _ in 0..8 {
                yield_now().await;
            }
        }
        assert!(control.is_finished(), "primary control must pass the queued neighbor retry load");
        control.await.expect("primary control task").expect("send primary ACK");

        let deadline_bytes = parse_hex_fixture(vectors::AGENT_UPDATE_INITIAL).expect("parse AgentUpdate fixture");
        assert!(matches!(Packet::decode(&deadline_bytes).expect("decode AgentUpdate fixture").message(), Message::AgentUpdate(_)));
        let deadline_pacer = pacers[0].handle();
        let deadline_socket = Arc::clone(&transports[0]);
        let deadline = spawn(async move { deadline_pacer.send(&deadline_socket, &deadline_bytes, TrafficClass::Deadline).await });

        for _ in 0..100 {
            if deadline.is_finished() {
                break;
            }
            advance(Duration::from_millis(10)).await;
            for _ in 0..8 {
                yield_now().await;
            }
        }
        assert!(deadline.is_finished(), "primary deadline traffic must pass the queued neighbor retry load");
        deadline.await.expect("primary deadline task").expect("send primary AgentUpdate");

        for _ in 0..100 {
            advance(Duration::from_millis(10)).await;
            for _ in 0..8 {
                yield_now().await;
            }
        }

        assert!(
            retry_counts.iter().all(|sent| sent.load(Ordering::Relaxed) > 0),
            "each neighbor retransmits while primary priority traffic is sent"
        );
        assert!(
            retry_workers.iter().all(|worker| !worker.is_finished()),
            "the neighbor retry queues remain backlogged after primary service"
        );

        for worker in retry_workers {
            worker.abort();
            let _ = worker.await;
        }
        drop(pacers);
        budget.shutdown().await;
        drop(simulator_sockets);
    }

    #[test]
    fn urgent_send_preempts_fair() {
        let circuit = Circuit::new();
        let mut pending = Some(pending_for_test(&circuit, queued_for_test(TrafficClass::Fair), TrafficClass::Fair));
        let mut queue = OutboundQueue::new();
        assert!(queue.push(queued_for_test(TrafficClass::Control), Some(TrafficClass::Fair)).is_ok());

        preempt_deferred_send(&mut pending, &mut queue, &circuit, false).expect("preempt deferred fair send");

        assert!(pending.is_none(), "the lower-class pacer request should be canceled");
        assert_eq!(queue.pop_next().expect("control send first").class, TrafficClass::Control);
        assert_eq!(queue.pop_next().expect("requeued fair send").class, TrafficClass::Fair);
    }

    #[test]
    fn blocked_reliable_control_does_not_preempt_paced_fair_send() {
        let mut circuit = Circuit::new();
        let reliable = queued_for_test(TrafficClass::Fair).message;
        for _ in 0..MAX_PENDING_PACKETS {
            let prepared = circuit.prepare_message(reliable.clone(), Delivery::Reliable).expect("prepare tracked packet");
            circuit.commit_sent(prepared, Instant::now()).expect("track sent packet");
        }
        assert!(!circuit.has_reliable_capacity());

        let fair = queued_for_test(TrafficClass::Fair);
        let mut pending = Some(pending_for_test(&circuit, fair, TrafficClass::Fair));
        let mut queue = OutboundQueue::new();
        assert!(queue.push(queued_with_delivery_for_test(TrafficClass::Control, Delivery::Reliable), None).is_ok());

        preempt_deferred_send(&mut pending, &mut queue, &circuit, false).expect("check pending send priority");

        assert!(pending.is_some(), "ineligible reliable Control work must not cancel the Fair send");
        assert_eq!(queue.messages.len(), 1, "reliable Control work remains queued until an ACK opens a slot");
    }

    #[test]
    fn duplicate_reack_waits_for_ack_deadline_before_preempting_send() {
        let mut circuit = Circuit::new();
        let now = Instant::now();
        let received = circuit.prepare_receive(42, true, [], now).expect("reserve initial reliable packet");
        circuit.commit_receive(received);
        let ack = circuit.prepare_acknowledgements().expect("prepare initial ACK").expect("initial ACK should be pending");
        circuit.commit_sent(ack, now).expect("clear initial ACK");

        let outgoing = circuit
            .prepare_message(Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 2 } }), Delivery::Reliable)
            .expect("prepare an outstanding reliable packet");
        let acknowledged_sequence = outgoing.sequence();
        circuit.commit_sent(outgoing, now).expect("record the outgoing reliable packet");
        assert!(circuit.has_pending(acknowledged_sequence));

        let queued = queued_for_test(TrafficClass::Fair);
        let mut pending = Some(pending_for_test(&circuit, queued, TrafficClass::Fair));
        let mut queue = OutboundQueue::new();
        let duplicate =
            circuit.prepare_receive(42, true, [acknowledged_sequence], now).expect("admit duplicate and process its appended ACK");
        assert!(matches!(duplicate, ReceiveAdmission::Duplicate(_)));
        assert!(!circuit.has_pending(acknowledged_sequence), "a duplicate's appended ACK still acknowledges outgoing data");
        assert!(circuit.has_pending_acks(), "duplicate should enqueue a re-ACK");

        preempt_deferred_send(&mut pending, &mut queue, &circuit, false).expect("defer duplicate re-ACK until its deadline");

        assert!(pending.is_some(), "an early re-ACK should not cancel a packet already waiting for the pacer");
        assert!(queue.messages.is_empty(), "the pending payload stays in flight before the ACK deadline");
    }

    #[tokio::test]
    async fn due_ack_preempts_deferred_payload_and_runs_before_retry() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
        let socket = Arc::new(UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect transport"));
        let budget = OutboundBudget::new(8_000);
        let pacer = budget.register(1, false).await.expect("register simulator pacer");
        let mut circuit = Circuit::new();
        let now = Instant::now();
        let original = circuit
            .prepare_message(queued_for_test(TrafficClass::Fair).message, Delivery::Reliable)
            .expect("prepare outstanding reliable packet");
        circuit.commit_sent(original, now - Duration::from_secs(6)).expect("track outstanding reliable packet");
        let mut pending = Some(pending_for_test(&circuit, queued_for_test(TrafficClass::Fair), TrafficClass::Fair));
        let admission = circuit.prepare_receive(42, true, [], now).expect("reserve reliable receive");
        circuit.commit_receive(admission);
        let mut queue = OutboundQueue::new();

        preempt_deferred_send(&mut pending, &mut queue, &circuit, true).expect("prioritize due ACK");

        assert!(pending.is_none(), "a due standalone ACK cancels a pacer-deferred payload");
        assert!(queue.messages.iter().any(|queued| queued.class == TrafficClass::Fair));
        let mut deferred_retries = HashMap::new();
        let ack = prepare_next_send(&socket, &pacer, &mut circuit, &mut queue, &mut deferred_retries, true, false)
            .map_err(|(_, error)| error)
            .expect("prepare due standalone ACK")
            .expect("due ACK should be prepared first");
        assert!(ack.prepared.is_acknowledgement());
        assert!(matches!(Packet::decode(ack.prepared.bytes()).expect("decode due ACK").message(), Message::PacketAck(_)));
        assert!(queue.messages.iter().any(|queued| queued.class == TrafficClass::Fair));

        drop(ack);
        drop(pacer);
        budget.shutdown().await;
    }

    async fn receive_packet(socket: &UdpSocket) -> (Packet, SocketAddr) {
        let mut buffer = [0; 16 * 1024];
        let (size, peer) = timeout(WAIT, socket.recv_from(&mut buffer)).await.expect("receive simulator packet").expect("read datagram");
        (Packet::decode(&buffer[..size]).expect("decode simulator packet"), peer)
    }

    async fn send_ack(socket: &UdpSocket, peer: SocketAddr, sequence: u32) {
        let bytes = Packet::new(500, Message::PacketAck(PacketAck { packets: vec![Packets { id: sequence }] }))
            .encode()
            .expect("encode simulator acknowledgement");
        socket.send_to(&bytes, peer).await.expect("send simulator acknowledgement");
    }

    async fn receive_ack(socket: &UdpSocket, sequence: u32) {
        timeout(WAIT, async {
            loop {
                let (packet, _) = receive_packet(socket).await;
                if packet.appended_acks().contains(&sequence)
                    || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|ack| ack.id == sequence))
                {
                    break;
                }
            }
        })
        .await
        .expect("receive reliable packet acknowledgement");
    }

    async fn receive_connected(events: &mut mpsc::Receiver<NeighborEvent>) {
        timeout(WAIT, async {
            loop {
                if let Some(NeighborEvent::Simulator(event)) = events.recv().await
                    && matches!(event.into_event(), Event::SimulatorConnected { .. })
                {
                    break;
                }
            }
        })
        .await
        .expect("receive connected event");
    }

    async fn receive_message_event(events: &mut mpsc::Receiver<NeighborEvent>, sequence: u32) -> Event {
        timeout(WAIT, async {
            loop {
                if let Some(NeighborEvent::Simulator(event)) = events.recv().await {
                    let event = event.into_event();
                    if matches!(event, Event::SimulatorMessage { sequence: received, .. } if received == sequence) {
                        break event;
                    }
                }
            }
        })
        .await
        .expect("receive simulator message event")
    }

    async fn run_budget_retry(role: ConnectionRole) {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let simulator_address = simulator.local_addr().expect("read simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let session = session(simulator_address);
        let coverage = CoverageLossLedger::new();
        let status = NetworkStatusHandle::default();
        let connection = match role {
            ConnectionRole::Primary => {
                NeighborConnection::connect_primary_with_coverage(simulator_id, session, None, status.clone(), coverage.clone())
                    .await
                    .expect("connect primary simulator")
            }
            ConnectionRole::Neighbor => {
                NeighborConnection::connect_with_coverage(simulator_id, session, 42, None, status.clone(), coverage.clone())
                    .await
                    .expect("connect neighbor simulator")
            }
        };

        let budget = SimulatorMemoryBudget::new();
        let packet_memory = match role {
            ConnectionRole::Primary => budget.clone(),
            ConnectionRole::Neighbor => budget.for_neighbor(),
        };
        let budget_view = packet_memory.clone();
        let (events, mut received_events) = mpsc::channel(4);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
        let (_commands, commands) = mpsc::channel(1);
        let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
        let task: JoinHandle<Result<(), String>> =
            spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

        let (use_circuit_code, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit_code.sequence()).await;
        receive_connected(&mut received_events).await;
        if role == ConnectionRole::Primary {
            let (movement, peer) = receive_packet(&simulator).await;
            assert!(matches!(movement.message(), Message::CompleteAgentMovement(_)));
            send_ack(&simulator, peer, movement.sequence()).await;
        }

        let full_budget = budget.reserve_bytes_for_test(MAX_QUEUED_SIMULATOR_EVENT_BYTES).expect("occupy the shared byte budget");
        let mut wire = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse simulator chat fixture");
        let sequence = 700u32;
        wire[0] |= 0x40;
        wire[1..5].copy_from_slice(&sequence.to_be_bytes());
        simulator.send_to(&wire, peer).await.expect("send packet while memory admission is full");

        let admission_loss = timeout(Duration::from_millis(100), async {
            loop {
                let (packet, _) = receive_packet(&simulator).await;
                if packet.appended_acks().contains(&sequence)
                    || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|ack| ack.id == sequence))
                {
                    break;
                }
            }
        })
        .await;
        assert!(admission_loss.is_err(), "memory-rejected reliable packet must not be acknowledged");
        assert!(
            coverage.snapshot().losses.iter().any(|loss| matches!(
                loss,
                CoverageLoss::Source(record)
                    if record.watermark.simulator_id == simulator_id
                        && record.watermark.source == CoverageSource::Lludp
                        && record.reason == CoverageLossReason::RetainedMemoryAdmission
            )),
            "memory rejection must remain visible to Session coverage handling"
        );

        drop(full_budget);
        simulator.send_to(&wire, peer).await.expect("retry after byte budget recovers");
        receive_ack(&simulator, sequence).await;
        assert!(matches!(receive_message_event(&mut received_events, sequence).await, Event::SimulatorMessage { .. }));

        shutdown.send_replace(true);
        assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
        assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
        if role == ConnectionRole::Neighbor {
            assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
            assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
        }
    }

    #[test]
    fn preflight_resource_classification() {
        for error in [LLUdpError::DecodedPacketTooLarge, LLUdpError::AllocationBoundOverflow] {
            assert_eq!(super::preflight_loss_reason(error, protocol::schema::ids::OBJECT_UPDATE), CoverageLossReason::DecodeLimit);
        }
        for error in [LLUdpError::TruncatedPacket, LLUdpError::InvalidMessage, LLUdpError::InvalidZeroCode] {
            assert_eq!(super::preflight_loss_reason(error, MessageId::high(0x71)), CoverageLossReason::PreflightRejected);
        }
        assert_eq!(
            super::preflight_loss_reason(LLUdpError::InvalidMessageId, protocol::schema::ids::OBJECT_UPDATE),
            CoverageLossReason::AllocationEstimateMismatch,
            "an unsupported specialized route is an internal preflight defect"
        );
        assert_eq!(
            super::preflight_loss_reason(LLUdpError::InvalidMessageId, MessageId::high(0x71)),
            CoverageLossReason::PreflightRejected
        );
    }

    #[tokio::test]
    async fn byte_pressure_retries_for_both_roles() {
        run_budget_retry(ConnectionRole::Primary).await;
        run_budget_retry(ConnectionRole::Neighbor).await;
    }

    #[tokio::test]
    async fn rejected_packet_retries() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let simulator_address = simulator.local_addr().expect("read simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let coverage = CoverageLossLedger::new();
        let connection = NeighborConnection::connect_with_coverage(
            simulator_id,
            session(simulator_address),
            42,
            None,
            NetworkStatusHandle::default(),
            coverage.clone(),
        )
        .await
        .expect("connect neighbor simulator");
        let budget = SimulatorMemoryBudget::new();
        let packet_memory = budget.for_neighbor();
        let budget_view = packet_memory.clone();
        let (events, mut received_events) = mpsc::channel(4);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
        let (_commands, commands) = mpsc::channel(1);
        let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
        let task = spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

        let (use_circuit_code, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit_code.sequence()).await;
        receive_connected(&mut received_events).await;

        let sequence = 700u32;
        let mut malformed = parse_hex_fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE).expect("parse terse update fixture");
        malformed[0] |= 0x40;
        malformed[1..5].copy_from_slice(&sequence.to_be_bytes());
        malformed[18] = u8::MAX;
        assert_eq!(
            protocol::lludp::Packet::preflight(&malformed),
            Err(LLUdpError::TruncatedPacket),
            "preflight rejects an object field that exceeds its packet frame"
        );
        assert!(Packet::decode(&malformed).is_err(), "decoder must reject malformed object framing");
        simulator.send_to(&malformed, peer).await.expect("send malformed reliable update");

        let mut buffer = [0; 2048];
        assert!(
            timeout(Duration::from_millis(100), simulator.recv_from(&mut buffer)).await.is_err(),
            "preflight-rejected packet must not be acknowledged"
        );
        assert!(coverage.snapshot().losses.iter().any(|loss| matches!(
            loss,
            CoverageLoss::Source(record)
                if record.watermark.simulator_id == simulator_id
                    && record.watermark.source == CoverageSource::Lludp
                && record.reason == CoverageLossReason::PreflightRejected
        )));
        assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
        assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

        let mut retry = parse_hex_fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE).expect("parse terse update fixture");
        retry[0] |= 0x40;
        retry[1..5].copy_from_slice(&sequence.to_be_bytes());
        simulator.send_to(&retry, peer).await.expect("retry reliable update");
        receive_ack(&simulator, sequence).await;
        assert!(matches!(receive_message_event(&mut received_events, sequence).await, Event::SimulatorMessage { .. }));

        shutdown.send_replace(true);
        assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
        assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
        assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    }

    #[tokio::test]
    async fn typed_decode_retries() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let simulator_address = simulator.local_addr().expect("read simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let coverage = CoverageLossLedger::new();
        let connection = NeighborConnection::connect_with_coverage(
            simulator_id,
            session(simulator_address),
            42,
            None,
            NetworkStatusHandle::default(),
            coverage.clone(),
        )
        .await
        .expect("connect neighbor simulator");
        let packet_memory = SimulatorMemoryBudget::new().for_neighbor();
        let budget_view = packet_memory.clone();
        let (events, mut received_events) = mpsc::channel(4);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
        let (_commands, commands) = mpsc::channel(1);
        let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
        let task = spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

        let (use_circuit_code, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit_code.sequence()).await;
        receive_connected(&mut received_events).await;

        let sequence = 701u32;
        let mut malformed = parse_hex_fixture(vectors::START_PING_CHECK).expect("parse ping fixture");
        malformed[0] |= 0x40;
        malformed[1..5].copy_from_slice(&sequence.to_be_bytes());
        malformed.pop();
        assert!(Packet::preflight(&malformed).is_ok(), "allocation preflight accepts the bounded no-allocation payload");
        assert!(Packet::decode(&malformed).is_err(), "typed decoder rejects the truncated ping field");
        simulator.send_to(&malformed, peer).await.expect("send packet rejected by typed decode");

        let mut buffer = [0; 2048];
        assert!(
            timeout(Duration::from_millis(100), simulator.recv_from(&mut buffer)).await.is_err(),
            "packet rejected by typed decode must not be acknowledged or answered"
        );
        assert!(coverage.snapshot().losses.iter().any(|loss| matches!(
            loss,
            CoverageLoss::Source(record)
                if record.watermark.simulator_id == simulator_id
                    && record.watermark.source == CoverageSource::Lludp
                    && record.reason == CoverageLossReason::DecodeRejected
        )));
        assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
        assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

        let mut retry = parse_hex_fixture(vectors::START_PING_CHECK).expect("parse ping fixture");
        retry[0] |= 0x40;
        retry[1..5].copy_from_slice(&sequence.to_be_bytes());
        simulator.send_to(&retry, peer).await.expect("retry packet with the same sequence");
        receive_ack(&simulator, sequence).await;

        shutdown.send_replace(true);
        assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
        assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
        assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    }

    #[tokio::test(start_paused = true)]
    async fn ack_queue_full_retries() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let simulator_address = simulator.local_addr().expect("read simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let coverage = CoverageLossLedger::new();
        let connection = NeighborConnection::connect_with_coverage(
            simulator_id,
            session(simulator_address),
            42,
            None,
            NetworkStatusHandle::default(),
            coverage.clone(),
        )
        .await
        .expect("connect neighbor simulator");
        let shared_budget = SimulatorMemoryBudget::new();
        let packet_memory = shared_budget.for_neighbor();
        let budget_view = packet_memory.clone();
        let (events, mut received_events) = mpsc::channel(4);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
        let (_commands, commands) = mpsc::channel(1);
        let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
        let task = spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

        let (use_circuit_code, peer) = receive_packet(&simulator).await;
        send_ack(&simulator, peer, use_circuit_code.sequence()).await;
        receive_connected(&mut received_events).await;

        let duplicate_sequence = 9_000u32;
        let mut duplicate = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse simulator chat fixture");
        duplicate[0] |= 0x40;
        duplicate[1..5].copy_from_slice(&duplicate_sequence.to_be_bytes());
        simulator.send_to(&duplicate, peer).await.expect("send reliable chat");
        receive_ack(&simulator, duplicate_sequence).await;
        let duplicate_retention = timeout(WAIT, async {
            loop {
                let Some(NeighborEvent::Simulator(envelope)) = received_events.recv().await else { continue };
                let (event, retention, _) = envelope.into_parts();
                if matches!(event, Event::SimulatorMessage { sequence, .. } if sequence == duplicate_sequence) {
                    break retention.expect("received chat retains its packet memory");
                }
            }
        })
        .await
        .expect("receive the retained chat");

        let first_sequence = 10_000;
        let rejected_sequence = first_sequence + NEW_RELIABLE_ACK_CAPACITY as u32;
        for sequence in first_sequence..rejected_sequence {
            let bytes = Packet::new(sequence, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 0 } }))
                .with_reliable()
                .encode()
                .expect("encode reliable ping response");
            simulator.send_to(&bytes, peer).await.expect("send reliable ping response");
            if sequence % 128 == 0 {
                yield_now().await;
            }
        }
        let mut rejected = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse chat fixture");
        rejected[0] |= 0x40;
        rejected[1..5].copy_from_slice(&rejected_sequence.to_be_bytes());
        simulator.send_to(&rejected, peer).await.expect("send packet with a full ACK queue");

        let mut overflow_recorded = false;
        for _ in 0..100_000 {
            overflow_recorded = coverage.snapshot().losses.iter().any(|loss| {
                matches!(
                    loss,
                    CoverageLoss::Source(record)
                        if record.watermark.simulator_id == simulator_id
                            && record.watermark.source == CoverageSource::Lludp
                            && record.reason == CoverageLossReason::AcknowledgementAdmission
                )
            });
            if overflow_recorded {
                break;
            }
            yield_now().await;
        }
        assert!(overflow_recorded, "full ACK capacity must leave the reliable packet unaccepted");

        let global_bytes = shared_budget
            .reserve_bytes_for_test(budget_view.global_available_bytes())
            .expect("hold the remaining global packet-memory budget");
        assert_eq!(budget_view.global_available_bytes(), 0);
        duplicate[0] |= 0x80;
        simulator.send_to(&duplicate, peer).await.expect("resend accepted chat while both budgets are full");

        let mut buffer = [0; 2048];
        assert!(
            matches!(simulator.try_recv_from(&mut buffer), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
            "no queued ACK should be sent before its timer or another outbound packet"
        );

        advance(ACK_INTERVAL).await;
        let mut acknowledged = HashSet::with_capacity(NEW_RELIABLE_ACK_CAPACITY + 1);
        while acknowledged.len() < NEW_RELIABLE_ACK_CAPACITY + 1 {
            let (packet, _) = receive_packet(&simulator).await;
            let Message::PacketAck(message) = packet.message() else { panic!("expected a standalone ACK packet") };
            for ack in &message.packets {
                assert!(acknowledged.insert(ack.id), "an accepted reliable sequence is ACKed once");
            }
        }
        for sequence in first_sequence..rejected_sequence {
            assert!(acknowledged.contains(&sequence), "accepted sequence {sequence} must be ACKed after capacity drains");
        }
        assert!(acknowledged.contains(&duplicate_sequence), "duplicate reliable packets remain re-ACKable at ACK capacity");
        assert!(!acknowledged.contains(&rejected_sequence), "the unadmitted sequence must not be ACKed");
        assert!(received_events.try_recv().is_err(), "the packet rejected by ACK admission must not be delivered");
        assert!(
            !coverage.snapshot().losses.iter().any(|loss| matches!(
                loss,
                CoverageLoss::Source(record)
                    if record.watermark.simulator_id == simulator_id
                        && record.watermark.source == CoverageSource::Lludp
                        && record.reason == CoverageLossReason::RetainedMemoryAdmission
            )),
            "a duplicate should bypass payload-memory admission"
        );

        simulator.send_to(&duplicate, peer).await.expect("resend accepted chat with only byte memory exhausted");
        advance(ACK_INTERVAL).await;
        receive_ack(&simulator, duplicate_sequence).await;
        assert!(received_events.try_recv().is_err(), "re-ACKing a duplicate must not redeliver it");

        drop(global_bytes);
        drop(duplicate_retention);

        simulator.send_to(&rejected, peer).await.expect("retry after ACK capacity drains");
        advance(ACK_INTERVAL).await;
        receive_ack(&simulator, rejected_sequence).await;
        assert!(matches!(receive_message_event(&mut received_events, rejected_sequence).await, Event::SimulatorMessage { .. }));

        shutdown.send_replace(true);
        assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
    }

    #[tokio::test]
    async fn ack_control_survives_memory_pressure() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let simulator_address = simulator.local_addr().expect("read simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let coverage = CoverageLossLedger::new();
        let status = NetworkStatusHandle::default();
        let connection =
            NeighborConnection::connect_with_coverage(simulator_id, session(simulator_address), 42, None, status, coverage.clone())
                .await
                .expect("connect neighbor simulator");

        let shared_budget = SimulatorMemoryBudget::new();
        let packet_memory = shared_budget.for_neighbor();
        let (events, mut received_events) = mpsc::channel(4);
        let (shutdown, shutdown_receiver) = watch::channel(false);
        let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
        let (_commands, commands) = mpsc::channel(1);
        let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
        let task: JoinHandle<Result<(), String>> =
            spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory.clone()));

        let (use_circuit_code, peer) = receive_packet(&simulator).await;
        let _decode_slots = packet_memory
            .decode_slots
            .clone()
            .acquire_many_owned(MAX_CONCURRENT_PACKET_DECODES.try_into().expect("decode slot count fits u32"))
            .await
            .expect("occupy every decoder slot");
        let _global_bytes = shared_budget.reserve_bytes_for_test(MAX_QUEUED_SIMULATOR_EVENT_BYTES).expect("occupy global packet memory");

        let mut malformed_ack =
            Packet::new(700, Message::PacketAck(PacketAck { packets: vec![Packets { id: use_circuit_code.sequence() }] }))
                .encode()
                .expect("encode circuit ACK");
        malformed_ack.truncate(11);
        assert!(Packet::preflight(&malformed_ack).is_ok(), "preflight should bound the fixed-width ACK allocation");
        assert!(Packet::decode(&malformed_ack).is_err(), "the typed ACK decoder should reject its truncated entry");
        simulator.send_to(&malformed_ack, peer).await.expect("send malformed circuit ACK under memory pressure");
        timeout(WAIT, async {
            loop {
                if coverage
                    .snapshot()
                    .losses
                    .iter()
                    .any(|loss| matches!(loss, CoverageLoss::Source(record) if record.reason == CoverageLossReason::DecodeRejected))
                {
                    break;
                }
                yield_now().await;
            }
        })
        .await
        .expect("malformed ACK should be rejected");

        let packets = once(Packets { id: use_circuit_code.sequence() }).chain((0..254).map(|id| Packets { id })).collect();
        let ack = Packet::new(701, Message::PacketAck(PacketAck { packets })).encode().expect("encode maximum-sized circuit ACK");
        simulator.send_to(&ack, peer).await.expect("send circuit ACK under memory pressure");
        receive_connected(&mut received_events).await;

        shutdown.send_replace(true);
        assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
    }
}
