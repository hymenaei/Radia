use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use protocol::{
    lludp::Message,
    messages::{
        AgentThrottle, LogoutRequest,
        agent::agent_throttle::{AgentData, Throttle},
        agent::logout_request,
    },
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{self, Instant},
};

use super::{
    ConnectionError, DEFAULT_AGENT_THROTTLE_RATES, LOGOUT_TIMEOUT, MAX_NONCONTROL_SENDS, MAX_PENDING_SEND_BYTES,
    MAX_PENDING_SEND_NOTIFICATIONS, MAX_PENDING_SENDS, MAX_SEND_AGE, MAX_SEND_ERROR_BYTES, OUTBOUND_SEND_CHARGE_BYTES, SimulatorEvent,
};
use crate::{
    AgentSession,
    circuit::{
        UdpTransport,
        lludp::{Circuit, Delivery, PreparedDatagram},
        pacing::{ConnectionPacer, PacerHandle, TrafficClass},
    },
    error::Error,
    event::SimulatorId,
};

pub(super) type PendingSendFuture = Pin<Box<dyn Future<Output = Result<(Instant, PreparedDatagram), ConnectionError>> + Send>>;

pub(super) struct QueuedMessage {
    pub(super) message: Message,
    pub(super) delivery: Delivery,
    pub(super) class: TrafficClass,
    pub(super) queued_at: Instant,
    pub(super) completion: Option<oneshot::Sender<Result<(), Error>>>,
    pub(super) cancel_target: Option<super::HandoffCancellation>,
    pub(super) report_failure: bool,
    pub(super) movement: MovementSend,
}

impl QueuedMessage {
    pub(super) fn age_exempt(&self) -> bool {
        matches!(&self.message, Message::RegionHandshakeReply(_))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MovementSend {
    None,
    Initial,
    Handoff,
}

pub(super) struct OutboundQueue {
    pub(super) messages: VecDeque<QueuedMessage>,
    pub(super) charged_bytes: usize,
    pub(super) class_cursor: usize,
}

impl OutboundQueue {
    pub(super) fn new() -> Self {
        Self { messages: VecDeque::with_capacity(MAX_PENDING_SENDS), charged_bytes: 0, class_cursor: 0 }
    }

    pub(super) fn push(&mut self, queued: QueuedMessage, active_class: Option<TrafficClass>) -> Result<(), Box<QueuedMessage>> {
        if self.is_full(queued.class, active_class) {
            return Err(Box::new(queued));
        }
        self.charged_bytes += OUTBOUND_SEND_CHARGE_BYTES;
        self.messages.push_back(queued);
        Ok(())
    }

    pub(super) fn push_front(&mut self, queued: QueuedMessage) -> Result<(), Box<QueuedMessage>> {
        if self.is_full(queued.class, None) {
            return Err(Box::new(queued));
        }
        self.charged_bytes += OUTBOUND_SEND_CHARGE_BYTES;
        self.messages.push_front(queued);
        Ok(())
    }

    pub(super) fn is_full(&self, class: TrafficClass, active_class: Option<TrafficClass>) -> bool {
        let active_count = usize::from(active_class.is_some());
        let active_bytes = active_count * OUTBOUND_SEND_CHARGE_BYTES;
        let noncontrol_count = self.messages.iter().filter(|queued| queued.class != TrafficClass::Control).count()
            + usize::from(active_class.is_some_and(|active| active != TrafficClass::Control));
        self.messages.len().saturating_add(active_count) >= MAX_PENDING_SENDS
            || (class != TrafficClass::Control && noncontrol_count >= MAX_NONCONTROL_SENDS)
            || self.charged_bytes.saturating_add(active_bytes).saturating_add(OUTBOUND_SEND_CHARGE_BYTES) > MAX_PENDING_SEND_BYTES
    }

    pub(super) fn has_higher_priority_matching(&self, class: TrafficClass, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> bool {
        self.messages.iter().any(|queued| traffic_priority(queued.class) < traffic_priority(class) && eligible(queued))
    }

    pub(super) fn pop_priority_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
        for class in [TrafficClass::Control, TrafficClass::Deadline] {
            if let Some(index) = self.messages.iter().position(|queued| queued.class == class && eligible(queued)) {
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                let removed = self.messages.remove(index);
                return removed;
            }
        }
        None
    }

    pub(super) fn pop_control_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
        let index = self.messages.iter().position(|queued| queued.class == TrafficClass::Control && eligible(queued))?;
        self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
        self.messages.remove(index)
    }

    pub(super) fn pop_next_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
        const ORDER: [TrafficClass; 5] =
            [TrafficClass::Control, TrafficClass::Control, TrafficClass::Deadline, TrafficClass::Fair, TrafficClass::Fair];
        for _ in 0..ORDER.len() {
            let class = ORDER[self.class_cursor];
            self.class_cursor = (self.class_cursor + 1) % ORDER.len();
            if let Some(index) = self.messages.iter().position(|queued| queued.class == class && eligible(queued)) {
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                let removed = self.messages.remove(index);
                return removed;
            }
        }
        None
    }

    pub(super) fn expire(&mut self, now: Instant) -> Vec<QueuedMessage> {
        let mut expired = Vec::new();
        let mut retained = VecDeque::with_capacity(self.messages.len());
        while let Some(queued) = self.messages.pop_front() {
            if !queued.age_exempt() && now.saturating_duration_since(queued.queued_at) >= MAX_SEND_AGE {
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                expired.push(queued);
            } else {
                retained.push_back(queued);
            }
        }
        self.messages = retained;
        expired
    }

    pub(super) fn has_handoff_movement(&self) -> bool {
        self.messages.iter().any(|queued| queued.movement == MovementSend::Handoff)
    }

    pub(super) fn cancel_handoff_movement(&mut self) {
        let mut retained = VecDeque::with_capacity(self.messages.len());
        while let Some(queued) = self.messages.pop_front() {
            if queued.movement == MovementSend::Handoff {
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
            } else {
                retained.push_back(queued);
            }
        }
        self.messages = retained;
    }

    pub(super) fn drain(&mut self) -> Vec<QueuedMessage> {
        self.charged_bytes = 0;
        self.messages.drain(..).collect()
    }
}

pub(super) struct PendingSend {
    pub(super) meta: PreparedMeta,
    pub(super) result: PendingSendFuture,
    pub(super) class: TrafficClass,
    pub(super) queued_message: Option<QueuedMessage>,
    pub(super) retry_deadline: Option<Instant>,
}

#[derive(Clone, Copy)]
pub(super) struct PreparedMeta {
    pub(super) sequence: u32,
    pub(super) is_acknowledgement: bool,
    pub(super) is_retransmission: bool,
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

pub(super) enum PendingSendExpiration {
    QueuedMessage(QueuedMessage),
    ReliableRetry,
}

pub(super) fn pending_send_result(
    socket: Arc<UdpTransport>,
    pacer: PacerHandle,
    prepared: PreparedDatagram,
    class: TrafficClass,
) -> PendingSendFuture {
    Box::pin(async move {
        let sent_at = pacer.send(&socket, prepared.bytes(), class).await?;
        Ok((sent_at, prepared))
    })
}

pub(super) fn begin_send(
    socket: Arc<UdpTransport>,
    pacer: PacerHandle,
    prepared: PreparedDatagram,
    class: TrafficClass,
    queued_message: Option<QueuedMessage>,
) -> PendingSend {
    let meta = PreparedMeta::from(&prepared);
    let result = pending_send_result(socket, pacer, prepared, class);
    PendingSend { meta, result, class, queued_message, retry_deadline: None }
}

pub(super) fn begin_retry_send(
    socket: Arc<UdpTransport>,
    pacer: PacerHandle,
    prepared: PreparedDatagram,
    retry_deadline: Instant,
) -> PendingSend {
    let meta = PreparedMeta::from(&prepared);
    let result = pending_send_result(socket, pacer, prepared, TrafficClass::Fair);
    PendingSend { meta, result, class: TrafficClass::Fair, queued_message: None, retry_deadline: Some(retry_deadline) }
}

pub(super) fn active_queue_class(pending: Option<&PendingSend>) -> Option<TrafficClass> {
    pending?.queued_message.as_ref().map(|queued| queued.class)
}

pub(super) fn traffic_priority(class: TrafficClass) -> u8 {
    match class {
        TrafficClass::Control => 0,
        TrafficClass::Deadline => 1,
        TrafficClass::Fair => 2,
    }
}

pub(super) fn expire_pending_send(pending: &mut Option<PendingSend>, now: Instant) -> Option<PendingSendExpiration> {
    let current = pending.as_ref()?;
    let queued_expired = current
        .queued_message
        .as_ref()
        .is_some_and(|queued| !queued.age_exempt() && now.saturating_duration_since(queued.queued_at) >= MAX_SEND_AGE);
    let retry_expired = current.retry_deadline.is_some_and(|deadline| now >= deadline);
    if !queued_expired && !retry_expired {
        return None;
    }

    match pending.take()?.queued_message {
        Some(queued) => Some(PendingSendExpiration::QueuedMessage(queued)),
        None if retry_expired => Some(PendingSendExpiration::ReliableRetry),
        None => None,
    }
}

pub(super) fn expire_deferred_retries(
    circuit: &Circuit,
    deferred_retries: &mut HashMap<u32, Instant>,
    now: Instant,
) -> Result<(), ConnectionError> {
    deferred_retries.retain(|sequence, _| circuit.has_pending(*sequence));
    if deferred_retries.values().any(|started_at| now.saturating_duration_since(*started_at) >= MAX_SEND_AGE) {
        return Err(ConnectionError::ReliableRetryExpired);
    }
    Ok(())
}

pub(super) async fn send_prepared(
    socket: &UdpTransport,
    circuit: &mut Circuit,
    pacer: &ConnectionPacer,
    prepared: PreparedDatagram,
    class: TrafficClass,
) -> Result<u32, ConnectionError> {
    let sequence = prepared.sequence();
    let sent_at = pacer.handle().send(socket, prepared.bytes(), class).await?;
    circuit.commit_sent(prepared, sent_at)?;
    Ok(sequence)
}

pub(super) fn logout_message(session: &AgentSession) -> Message {
    Message::LogoutRequest(LogoutRequest {
        agent_data: logout_request::AgentData { agent_id: session.agent_id, session_id: session.session_id },
    })
}

pub(super) fn agent_throttle_message(session: &AgentSession) -> Message {
    Message::AgentThrottle(AgentThrottle {
        agent_data: AgentData { agent_id: session.agent_id, session_id: session.session_id, circuit_code: session.circuit_code },
        throttle: Throttle { gen_counter: 0, throttles: DEFAULT_AGENT_THROTTLE_RATES },
    })
}

pub(super) fn queue_internal(
    queue: &mut OutboundQueue,
    message: Message,
    delivery: Delivery,
    class: TrafficClass,
    movement: MovementSend,
    active_class: Option<TrafficClass>,
) -> Result<(), ConnectionError> {
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
    queue.push(queued, active_class).map_err(|_| ConnectionError::SendQueueFull)
}

pub(crate) fn bound_send_error(mut error: String) -> String {
    if error.len() > MAX_SEND_ERROR_BYTES {
        let mut end = MAX_SEND_ERROR_BYTES;
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        error.truncate(end);
    }
    error
}

pub(super) fn queue_send_notification(
    events: &mpsc::Sender<SimulatorEvent>,
    pending: &mut VecDeque<SimulatorEvent>,
    simulator_id: SimulatorId,
    cancel_target: Option<super::HandoffCancellation>,
    error: Option<ConnectionError>,
    report_failure: bool,
) -> Result<(), ConnectionError> {
    if !report_failure && cancel_target.is_none() {
        return Ok(());
    }
    let event = SimulatorEvent::SendCompleted { simulator_id, cancel_target, error, report_failure };
    if !pending.is_empty() {
        return push_send_notification(pending, event);
    }

    match events.try_send(event) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(event)) => push_send_notification(pending, event),
        Err(mpsc::error::TrySendError::Closed(_)) => Err(ConnectionError::SimulatorManagerClosed),
    }
}

pub(super) fn push_send_notification(pending: &mut VecDeque<SimulatorEvent>, event: SimulatorEvent) -> Result<(), ConnectionError> {
    if pending.len() >= MAX_PENDING_SEND_NOTIFICATIONS {
        return Err(ConnectionError::SendNotificationQueueFull);
    }
    pending.push_back(event);
    Ok(())
}

pub(super) fn report_queued_failure(
    events: &mpsc::Sender<SimulatorEvent>,
    pending: &mut VecDeque<SimulatorEvent>,
    simulator_id: SimulatorId,
    mut queued: QueuedMessage,
    error: ConnectionError,
) -> Result<(), ConnectionError> {
    let has_completion = queued.completion.is_some();
    if let Some(completion) = queued.completion.take()
        && let Err(failure) = completion.send(Err(error.to_network_error()))
    {
        drop(failure);
    }
    if queued.report_failure || queued.cancel_target.is_some() {
        queue_send_notification(events, pending, simulator_id, queued.cancel_target, Some(error), queued.report_failure)?;
    } else if !has_completion {
        return Err(error);
    }
    Ok(())
}

pub(super) fn preempt_deferred_send(
    pending: &mut Option<PendingSend>,
    queue: &mut OutboundQueue,
    circuit: &Circuit,
    ack_deadline_due: bool,
) -> Result<(), ConnectionError> {
    let Some(current) = pending.as_ref() else { return Ok(()) };
    let obsolete_retry = current.meta.is_retransmission && !circuit.has_pending(current.meta.sequence);
    let ack_preempts = ack_deadline_due && !current.meta.is_acknowledgement && circuit.has_pending_acks();
    let class_preempts = queue.has_higher_priority_matching(current.class, |queued| can_prepare_queued(circuit, queued));
    if obsolete_retry || ack_preempts || class_preempts {
        let mut canceled = pending.take().expect("pending send was checked");
        if (ack_preempts || class_preempts)
            && let Some(queued) = canceled.queued_message.take()
        {
            queue.push_front(queued).map_err(|_| ConnectionError::SendQueueFullPrioritizing)?;
        }
    }
    Ok(())
}

pub(super) fn fail_queued_on_close(
    events: &mpsc::Sender<SimulatorEvent>,
    pending: &mut VecDeque<SimulatorEvent>,
    simulator_id: SimulatorId,
    mut queued: QueuedMessage,
) {
    let failure = ConnectionError::SimulatorConnectionClosed;
    if let Some(completion) = queued.completion.take()
        && let Err(error) = completion.send(Err(Error::SimulatorConnectionClosed))
    {
        drop(error);
    }
    if (queued.report_failure || queued.cancel_target.is_some())
        && let Err(error) =
            queue_send_notification(events, pending, simulator_id, queued.cancel_target, Some(failure), queued.report_failure)
    {
        tracing::warn!(simulator = simulator_id.get(), %error, "could not queue a simulator send failure notification");
    }
}

pub(super) async fn flush_send_notifications(
    events: &mpsc::Sender<SimulatorEvent>,
    pending: &mut VecDeque<SimulatorEvent>,
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

pub(super) fn prepare_next_send(
    socket: &Arc<UdpTransport>,
    pacer: &ConnectionPacer,
    circuit: &mut Circuit,
    queue: &mut OutboundQueue,
    deferred_retries: &mut HashMap<u32, Instant>,
    send_ack_now: bool,
    last_send_was_retry: bool,
) -> Result<Option<PendingSend>, (Option<Box<QueuedMessage>>, ConnectionError)> {
    if send_ack_now && circuit.has_pending_acks() {
        let prepared = circuit.prepare_acknowledgements().map_err(|error| (None, error.into()))?;
        if let Some(prepared) = prepared {
            return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, TrafficClass::Control, None)));
        }
    }

    let now = Instant::now();
    let prepared = circuit.prepare_retransmission(now).map_err(|error| (None, error.into()))?;
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
                Err(error) => return Err((Some(Box::new(queued)), error.into())),
            };
            return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), message, queued.class, Some(queued))));
        }
        if last_send_was_retry && let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued)) {
            let message = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
                Ok(prepared) => prepared,
                Err(error) => return Err((Some(Box::new(queued)), error.into())),
            };
            return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), message, queued.class, Some(queued))));
        }
        return Ok(Some(begin_retry_send(Arc::clone(socket), pacer.handle(), prepared, retry_started_at + MAX_SEND_AGE)));
    }

    if circuit.has_pending_acks()
        && let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued))
    {
        let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
            Ok(prepared) => prepared,
            Err(error) => return Err((Some(Box::new(queued)), error.into())),
        };
        return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, queued.class, Some(queued))));
    }

    let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued)) else { return Ok(None) };
    let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
        Ok(prepared) => prepared,
        Err(error) => return Err((Some(Box::new(queued)), error.into())),
    };
    let class = queued.class;
    Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, class, Some(queued))))
}

pub(super) fn can_prepare_queued(circuit: &Circuit, queued: &QueuedMessage) -> bool {
    !queued.delivery.tracks_reliable() || circuit.has_reliable_capacity()
}

pub(super) fn prepare_shutdown_send(
    socket: &Arc<UdpTransport>,
    pacer: &ConnectionPacer,
    circuit: &mut Circuit,
    queue: &mut OutboundQueue,
    send_ack_now: bool,
) -> Result<Option<PendingSend>, (Option<Box<QueuedMessage>>, ConnectionError)> {
    if send_ack_now && circuit.has_pending_acks() {
        let prepared = circuit.prepare_acknowledgements().map_err(|error| (None, error.into()))?;
        if let Some(prepared) = prepared {
            return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, TrafficClass::Control, None)));
        }
    }

    if let Some(queued) = queue.pop_control_matching(|queued| can_prepare_queued(circuit, queued)) {
        let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
            Ok(prepared) => prepared,
            Err(error) => return Err((Some(Box::new(queued)), error.into())),
        };
        return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, queued.class, Some(queued))));
    }

    Ok(None)
}

pub(super) fn message_class(message: &Message) -> TrafficClass {
    match message {
        Message::AgentUpdate(_) | Message::CompleteAgentMovement(_) => TrafficClass::Deadline,
        Message::SetAlwaysRun(_)
        | Message::UseCircuitCode(_)
        | Message::RegionHandshakeReply(_)
        | Message::CompletePingCheck(_)
        | Message::LogoutRequest(_)
        | Message::TeleportCancel(_) => TrafficClass::Control,
        _ => TrafficClass::Fair,
    }
}
