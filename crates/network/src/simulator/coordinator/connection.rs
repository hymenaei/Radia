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
        agent::agent_throttle::{AgentData, Throttle},
        agent::complete_agent_movement,
        agent::logout_request,
        circuit::use_circuit_code::CircuitCode,
        region::region_handshake_reply,
    },
    schema::ids::PACKET_ACK,
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::yield_now,
    time::{self, Instant},
};

use super::{
    ACK_INTERVAL, ConnectionRole, EVENT_BACKLOG_DISCONNECT_REASON, HandoffTarget, LOGOUT_TIMEOUT, MAX_PRECONNECT_EVENTS,
    MAX_QUEUED_SIMULATOR_EVENTS, NEIGHBOR_PRECONNECT_OVERFLOW_REASON, PrimaryState, SimulatorCommand, SimulatorEvent,
    SimulatorMemoryBudget, SimulatorRequest, connection_message_has_application_event, finish_logout, queue_neighbor_event,
    record_coverage_loss, record_event_envelope_loss,
};
#[cfg(test)]
use crate::transport::SendGate;
use crate::{
    AgentSession,
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
const DEFAULT_AGENT_THROTTLE_RATES: ThrottleRates = ThrottleRates {
    resend_bits_per_second: 460_800.0,
    land_bits_per_second: 317_440.0,
    wind_bits_per_second: 63_488.0,
    cloud_bits_per_second: 63_488.0,
    task_bits_per_second: 1_564_672.0,
    texture_bits_per_second: 1_564_672.0,
    asset_bits_per_second: 573_440.0,
};

type PendingSendFuture = Pin<Box<dyn Future<Output = Result<(Instant, PreparedDatagram), String>> + Send>>;

pub(super) struct SimulatorConnectionOptions {
    pub(super) simulator_id: SimulatorId,
    pub(super) region_handle: Option<u64>,
    pub(super) status: NetworkStatusHandle,
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) primary_state: Option<watch::Receiver<PrimaryState>>,
    pub(super) budget: OutboundBudgetHandle,
    pub(super) owned_budget: Option<OutboundBudget>,
    #[cfg(test)]
    pub(super) send_gate: Option<SendGate>,
}

pub(super) struct SimulatorConnection {
    pub(super) simulator_id: SimulatorId,
    pub(super) session: AgentSession,
    pub(super) region_handle: Option<u64>,
    initial_role: ConnectionRole,
    pub(super) socket: Arc<UdpTransport>,
    pub(super) circuit: Circuit,
    pub(super) use_circuit_sequence: u32,
    pub(super) handoff: Option<HandoffTarget>,
    handoff_role_generation: Option<u64>,
    pub(super) movement_requested: bool,
    pub(super) initial_movement_sent: bool,
    primary_state: PrimaryState,
    agent_throttle_sent: bool,
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) pacer: ConnectionPacer,
    _owned_budget: Option<OutboundBudget>,
    primary_state_receiver: watch::Receiver<PrimaryState>,
}

impl Drop for SimulatorConnection {
    fn drop(&mut self) {
        self.coverage_losses.retire(self.simulator_id, CoverageSource::Lludp);
    }
}

impl SimulatorConnection {
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

    pub(super) async fn connect(session: AgentSession, options: SimulatorConnectionOptions) -> Result<Self, String> {
        let SimulatorConnectionOptions {
            simulator_id,
            region_handle,
            status,
            coverage_losses,
            primary_state,
            budget,
            owned_budget,
            #[cfg(test)]
            send_gate,
        } = options;
        let is_primary = region_handle.is_none();
        let pacer = budget.register(simulator_id.get(), is_primary).await?;
        let primary_state_receiver = primary_state.unwrap_or_else(|| {
            let initial_state = if is_primary { PrimaryState::Connecting } else { PrimaryState::Neighbor { generation: 0 } };
            watch::channel(initial_state).1
        });
        let socket = UdpTransport::connect_with_status(session.simulator_address, status.clone()).await?;
        #[cfg(test)]
        let socket = {
            let mut socket = socket;
            if let Some(send_gate) = send_gate {
                socket.set_send_gate(send_gate);
            }
            socket
        };
        let socket = Arc::new(socket);
        let circuit_status = if is_primary { status.clone() } else { status.for_circuit() };
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
            initial_role: if is_primary { ConnectionRole::Primary } else { ConnectionRole::Neighbor },
            socket,
            circuit,
            use_circuit_sequence,
            handoff: None,
            handoff_role_generation: None,
            movement_requested: false,
            initial_movement_sent: false,
            primary_state: if is_primary { PrimaryState::Connecting } else { PrimaryState::Neighbor { generation: 0 } },
            agent_throttle_sent: false,
            coverage_losses,
            pacer,
            _owned_budget: owned_budget,
            primary_state_receiver,
        })
    }

    #[tracing::instrument(name = "simulator.connection", level = "info", skip_all, fields(simulator = self.simulator_id.get(), initial_role = ?self.initial_role))]
    pub(super) async fn run(
        mut self,
        events: mpsc::Sender<SimulatorEvent>,
        mut shutdown: watch::Receiver<bool>,
        neighbor_requests: mpsc::Sender<SimulatorRequest>,
        mut commands: mpsc::Receiver<SimulatorCommand>,
        mut logout_commands: mpsc::Receiver<oneshot::Sender<Result<(), Error>>>,
        packet_memory: SimulatorMemoryBudget,
    ) -> Result<(), String> {
        let mut receive_buffer = [0u8; 65_535];
        let mut established = false;
        let mut preconnect_messages: VecDeque<EventEnvelope> = VecDeque::with_capacity(MAX_PRECONNECT_EVENTS);
        let mut pending_events = VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS);
        let mut pending_notifications = VecDeque::with_capacity(MAX_PENDING_SEND_NOTIFICATIONS);
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
            expire_deferred_retries(&self.circuit, &mut deferred_retries, retry_check_at)?;
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
                        && (self.initial_role == ConnectionRole::Neighbor || !matches!(self.primary_state, PrimaryState::Connecting)) => {
                    return Err("The simulator stopped responding to LLUDP keepalive traffic.".into());
                }
                changed = self.primary_state_receiver.changed(), if primary_state_open => {
                    match changed {
                        Ok(()) => {
                            let state = *self.primary_state_receiver.borrow_and_update();
                            self.invalidate_stale_handoff(state.role_generation(), &mut outbound, &mut pending_send);
                            self.primary_state = state;
                            self.pacer.set_primary(state.connection_role() == ConnectionRole::Primary).await?;
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
                                permit.send(SimulatorEvent::Simulator(event));
                            }
                            event_delivery_turn = false;
                        }
                        Err(_) => return Ok(()),
                    }
                }
                _ = ping_timer.tick(), if pending_send.is_none() && !logout_sent && established && (self.initial_role == ConnectionRole::Neighbor || self.initial_movement_sent) => {
                    if let Some(prepared) = self.circuit.prepare_ping(Instant::now())? {
                        pending_send = Some(begin_send(
                            Arc::clone(&self.socket),
                            self.pacer.handle(),
                            prepared,
                            TrafficClass::Control,
                            None,
                        ));
                    }
                }
                command = commands.recv(), if !commands.is_closed() && pending_notifications.is_empty() => {
                    match command {
                        Some(SimulatorCommand::Send { message, delivery, completed, cancel_target, report_failure }) => {
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
                        Some(SimulatorCommand::PrepareHandoff(target)) => {
                            if Some(target.region_handle) != self.region_handle
                                || target.endpoint != self.session.simulator_address
                                || target.agent_id != self.session.agent_id
                                || target.session_id != self.session.session_id
                            {
                                continue;
                            }
                            let role_generation = self.current_primary_state().role_generation();
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
                        Some(SimulatorCommand::CancelHandoff) => {
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
                        drop(memory_reservation);
                        drop(decode_permit);
                        continue;
                    }
                    let message = packet.message();
                    let primary_state = self.current_primary_state();
                    self.invalidate_stale_handoff(primary_state.role_generation(), &mut outbound, &mut pending_send);
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
                        last_udp_received = Instant::now();
                        drop(memory_reservation);
                        drop(decode_permit);
                        continue;
                    }
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
                        if self.initial_role == ConnectionRole::Primary {
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
                            if let Err(error) = neighbor_requests.try_send(SimulatorRequest::Enable {
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
}

impl OutboundQueue {
    fn new() -> Self {
        Self { messages: VecDeque::with_capacity(MAX_PENDING_SENDS), charged_bytes: 0, class_cursor: 0 }
    }

    fn push(&mut self, queued: QueuedMessage, active_class: Option<TrafficClass>) -> Result<(), Box<QueuedMessage>> {
        if self.is_full(queued.class, active_class) {
            return Err(Box::new(queued));
        }
        self.charged_bytes += OUTBOUND_SEND_CHARGE_BYTES;
        self.messages.push_back(queued);
        Ok(())
    }

    fn push_front(&mut self, queued: QueuedMessage) -> Result<(), Box<QueuedMessage>> {
        if self.is_full(queued.class, None) {
            return Err(Box::new(queued));
        }
        self.charged_bytes += OUTBOUND_SEND_CHARGE_BYTES;
        self.messages.push_front(queued);
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
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                let removed = self.messages.remove(index);
                return removed;
            }
        }
        None
    }

    fn pop_control_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
        let index = self.messages.iter().position(|queued| queued.class == TrafficClass::Control && eligible(queued))?;
        self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
        self.messages.remove(index)
    }

    fn pop_next_matching(&mut self, mut eligible: impl FnMut(&QueuedMessage) -> bool) -> Option<QueuedMessage> {
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

    #[cfg(test)]
    fn pop_next(&mut self) -> Option<QueuedMessage> {
        self.pop_next_matching(|_| true)
    }

    fn expire(&mut self, now: Instant) -> Vec<QueuedMessage> {
        let mut expired = Vec::new();
        let mut retained = VecDeque::with_capacity(self.messages.len());
        while let Some(queued) = self.messages.pop_front() {
            if now.saturating_duration_since(queued.queued_at) >= MAX_SEND_AGE {
                self.charged_bytes = self.charged_bytes.saturating_sub(OUTBOUND_SEND_CHARGE_BYTES);
                expired.push(queued);
            } else {
                retained.push_back(queued);
            }
        }
        self.messages = retained;
        expired
    }

    fn has_handoff_movement(&self) -> bool {
        self.messages.iter().any(|queued| queued.movement == MovementSend::Handoff)
    }

    fn cancel_handoff_movement(&mut self) {
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

    fn drain(&mut self) -> Vec<QueuedMessage> {
        self.charged_bytes = 0;
        self.messages.drain(..).collect()
    }
}

struct PendingSend {
    meta: PreparedMeta,
    result: PendingSendFuture,
    class: TrafficClass,
    queued_message: Option<QueuedMessage>,
    retry_deadline: Option<Instant>,
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

enum PendingSendExpiration {
    QueuedMessage(QueuedMessage),
    ReliableRetry,
}

fn pending_send_result(
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

fn begin_send(
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

fn begin_retry_send(socket: Arc<UdpTransport>, pacer: PacerHandle, prepared: PreparedDatagram, retry_deadline: Instant) -> PendingSend {
    let meta = PreparedMeta::from(&prepared);
    let result = pending_send_result(socket, pacer, prepared, TrafficClass::Fair);
    PendingSend { meta, result, class: TrafficClass::Fair, queued_message: None, retry_deadline: Some(retry_deadline) }
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

fn expire_pending_send(pending: &mut Option<PendingSend>, now: Instant) -> Option<PendingSendExpiration> {
    let current = pending.as_ref()?;
    let queued_expired =
        current.queued_message.as_ref().is_some_and(|queued| now.saturating_duration_since(queued.queued_at) >= MAX_SEND_AGE);
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

fn expire_deferred_retries(circuit: &Circuit, deferred_retries: &mut HashMap<u32, Instant>, now: Instant) -> Result<(), String> {
    deferred_retries.retain(|sequence, _| circuit.has_pending(*sequence));
    if deferred_retries.values().any(|started_at| now.saturating_duration_since(*started_at) >= MAX_SEND_AGE) {
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
    events: &mpsc::Sender<SimulatorEvent>,
    pending: &mut VecDeque<SimulatorEvent>,
    simulator_id: SimulatorId,
    cancel_target: Option<super::HandoffCancellation>,
    error: Option<String>,
    report_failure: bool,
) -> Result<(), String> {
    if !report_failure && cancel_target.is_none() {
        return Ok(());
    }
    let event = SimulatorEvent::SendCompleted { simulator_id, cancel_target, error: error.map(bound_send_error), report_failure };
    if !pending.is_empty() {
        return push_send_notification(pending, event);
    }

    match events.try_send(event) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(event)) => push_send_notification(pending, event),
        Err(mpsc::error::TrySendError::Closed(_)) => Err("The simulator manager has closed.".into()),
    }
}

fn push_send_notification(pending: &mut VecDeque<SimulatorEvent>, event: SimulatorEvent) -> Result<(), String> {
    if pending.len() >= MAX_PENDING_SEND_NOTIFICATIONS {
        return Err("The simulator send notification queue is full.".into());
    }
    pending.push_back(event);
    Ok(())
}

fn report_queued_failure(
    events: &mpsc::Sender<SimulatorEvent>,
    pending: &mut VecDeque<SimulatorEvent>,
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
    events: &mpsc::Sender<SimulatorEvent>,
    pending: &mut VecDeque<SimulatorEvent>,
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
            return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, TrafficClass::Control, None)));
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
            return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), message, queued.class, Some(queued))));
        }
        if last_send_was_retry && let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued)) {
            let message = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
                Ok(prepared) => prepared,
                Err(error) => return Err((Some(Box::new(queued)), error)),
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
            Err(error) => return Err((Some(Box::new(queued)), error)),
        };
        return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, queued.class, Some(queued))));
    }

    let Some(queued) = queue.pop_next_matching(|queued| can_prepare_queued(circuit, queued)) else { return Ok(None) };
    let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
        Ok(prepared) => prepared,
        Err(error) => return Err((Some(Box::new(queued)), error)),
    };
    let class = queued.class;
    Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, class, Some(queued))))
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
            return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, TrafficClass::Control, None)));
        }
    }

    if let Some(queued) = queue.pop_control_matching(|queued| can_prepare_queued(circuit, queued)) {
        let prepared = match circuit.prepare_message(queued.message.clone(), queued.delivery) {
            Ok(prepared) => prepared,
            Err(error) => return Err((Some(Box::new(queued)), error)),
        };
        return Ok(Some(begin_send(Arc::clone(socket), pacer.handle(), prepared, queued.class, Some(queued))));
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
#[path = "connection_tests.rs"]
mod tests;
