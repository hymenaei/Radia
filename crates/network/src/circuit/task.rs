use std::{
    collections::{HashMap, VecDeque},
    future::pending,
    sync::Arc,
};

use protocol::lludp::Message;
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::{self, Instant},
};

use super::{
    ACK_INTERVAL, ConnectionError, ConnectionRole, ConnectionTask, LOGOUT_TIMEOUT, MAX_PENDING_SEND_NOTIFICATIONS,
    MAX_QUEUED_SIMULATOR_EVENTS, MAX_SEND_AGE, PrimaryState, SimulatorCommand, SimulatorEvent, SimulatorMemoryBudget, SimulatorRequest,
    active_queue_class, expire_deferred_retries, expire_pending_send, fail_queued_on_close, flush_send_notifications,
    lifecycle::{LogoutAction, Phase, RunControl},
    logout_message, message_class,
    outbound::{MovementSend, PendingSendExpiration, QueuedMessage, begin_send},
    preempt_deferred_send, prepare_next_send, prepare_shutdown_send, queue_send_notification,
    receive::PacketRejections,
    report_queued_failure,
};
use crate::{
    circuit::{
        lludp::{Delivery, KEEPALIVE_TIMEOUT, PING_INTERVAL, PreparedDatagram},
        pacing::TrafficClass,
    },
    error::Error,
    event::EventEnvelope,
};

enum SelectedEvent {
    Sent(Result<(Instant, PreparedDatagram), ConnectionError>),
    Shutdown,
    EventReceiverClosed,
    Logout(Option<oneshot::Sender<Result<(), Error>>>),
    TeardownDeadline,
    KeepaliveTimeout,
    PrimaryStateChanged(Result<(), watch::error::RecvError>),
    AckTick,
    EventDelivery(Result<mpsc::OwnedPermit<SimulatorEvent>, mpsc::error::SendError<()>>),
    PingTick,
    SendAgeDeadline,
    Command(Option<SimulatorCommand>),
    Datagram(std::io::Result<usize>),
    Yield,
}

impl ConnectionTask {
    async fn run(mut self) -> Result<(), ConnectionError> {
        let result = self.run_loop().await;
        self.packet_rejections.flush();
        flush_send_notifications(&self.events, &mut self.pending_notifications, &mut self.shutdown).await;
        tracing::info!(
            role = ?self.connection.current_primary_state().connection_role(),
            "simulator disconnected"
        );
        result
    }

    async fn run_loop(&mut self) -> Result<(), ConnectionError> {
        let mut receive_buffer = [0u8; 65_535];
        let event_sender = self.events.clone();
        loop {
            if *self.shutdown.borrow() || self.events.is_closed() {
                return finish_connection_logout(self.phase.close_reason());
            }
            self.expire_sends()?;
            self.begin_draining_when_ready();
            self.discard_application_sends_while_draining();
            if self.drain_is_complete() {
                return finish_connection_logout(self.phase.close_reason());
            }
            self.admit_pending_handshake_reply()?;
            if !self.prepare_send()? {
                continue;
            }

            let pending_send_deadline = self.pending_send_deadline();
            let selected = tokio::select! {
                biased;
                sent = async {
                    self.pending_send
                        .as_mut()
                        .expect("send branch is guarded")
                        .result
                        .as_mut()
                        .await
                }, if self.pending_send.is_some() => SelectedEvent::Sent(sent),
                _ = self.shutdown.changed() => SelectedEvent::Shutdown,
                _ = self.events.closed() => SelectedEvent::EventReceiverClosed,
                logout = async {
                    match self.logout_commands.as_mut() {
                        Some(commands) => commands.recv().await,
                        None => pending().await,
                    }
                }, if self.logout_commands.is_some() => SelectedEvent::Logout(logout),
                _ = async {
                    match pending_send_deadline {
                        Some(deadline) => time::sleep_until(deadline).await,
                        None => pending().await,
                    }
                }, if pending_send_deadline.is_some() => SelectedEvent::SendAgeDeadline,
                _ = time::sleep_until(self.phase.deadline().unwrap_or_else(|| Instant::now() + LOGOUT_TIMEOUT)),
                    if self.phase.deadline().is_some() => SelectedEvent::TeardownDeadline,
                _ = time::sleep_until(self.last_udp_received + KEEPALIVE_TIMEOUT),
                    if self.phase.is_established()
                        && !self.phase.is_closing()
                        && (self.connection.initial_role == ConnectionRole::Neighbor
                            || !matches!(self.connection.primary_state, PrimaryState::Connecting)) => SelectedEvent::KeepaliveTimeout,
                changed = async {
                    match self.connection.primary_state_receiver.as_mut() {
                        Some(receiver) => receiver.changed().await,
                        None => pending().await,
                    }
                }, if self.connection.primary_state_receiver.is_some() => SelectedEvent::PrimaryStateChanged(changed),
                _ = self.ack_timer.tick() => SelectedEvent::AckTick,
                permit = event_sender.clone().reserve_owned(),
                    if self.event_delivery_turn && (!self.pending_notifications.is_empty() || !self.pending_events.is_empty()) => {
                    SelectedEvent::EventDelivery(permit)
                }
                _ = self.ping_timer.tick(),
                    if self.pending_send.is_none()
                        && !self.phase.logout_requested()
                        && self.phase.is_established()
                        && (self.connection.initial_role == ConnectionRole::Neighbor || self.connection.initial_movement_sent) => SelectedEvent::PingTick,
                command = self.commands.recv(), if !self.commands.is_closed() && self.pending_notifications.is_empty() => SelectedEvent::Command(command),
                received = self.connection.socket.recv(&mut receive_buffer) => SelectedEvent::Datagram(received),
                _ = tokio::task::yield_now(),
                    if !self.event_delivery_turn && (!self.pending_notifications.is_empty() || !self.pending_events.is_empty()) => SelectedEvent::Yield
            };
            let control = match selected {
                SelectedEvent::Sent(sent) => self.send_completed(sent)?,
                SelectedEvent::Shutdown | SelectedEvent::EventReceiverClosed => RunControl::Finish(None),
                SelectedEvent::Logout(command) => self.logout_command(command)?,
                SelectedEvent::TeardownDeadline => self.teardown_deadline(),
                SelectedEvent::KeepaliveTimeout => self.keepalive_timeout()?,
                SelectedEvent::PrimaryStateChanged(changed) => self.primary_state_changed(changed).await?,
                SelectedEvent::AckTick => self.ack_tick()?,
                SelectedEvent::EventDelivery(permit) => {
                    Self::deliver_event(permit, &mut self.pending_notifications, &mut self.pending_events, &mut self.event_delivery_turn)
                }
                SelectedEvent::PingTick => self.ping_tick()?,
                SelectedEvent::SendAgeDeadline => {
                    self.expire_sends()?;
                    RunControl::Continue
                }
                SelectedEvent::Command(command) => self.command(command)?,
                SelectedEvent::Datagram(received) => self.receive_datagram(received, &receive_buffer)?,
                SelectedEvent::Yield => {
                    self.event_delivery_turn = true;
                    RunControl::Continue
                }
            };
            if let RunControl::Finish(reason) = control {
                return finish_connection_logout(reason);
            }
        }
    }

    fn expire_sends(&mut self) -> Result<(), ConnectionError> {
        let now = Instant::now();
        expire_deferred_retries(&self.connection.circuit, &mut self.deferred_retries, now)?;
        if self.pending_handshake_reply.as_ref().is_some_and(|queued| now.saturating_duration_since(queued.queued_at) >= MAX_SEND_AGE) {
            if let Some(mut queued) = self.pending_handshake_reply.take()
                && let Some(completed) = queued.completion.take()
                && let Err(error) = completed.send(Err(Error::SimulatorCommandQueueFull))
            {
                drop(error);
            }
            return Err(ConnectionError::SimulatorCommandQueueFull);
        }
        if self.pending_notifications.is_empty() {
            for queued in self.outbound.expire(Instant::now()) {
                let error = send_age_expiry_error(&queued);
                report_queued_failure(&self.events, &mut self.pending_notifications, self.connection.simulator_id, queued, error)?;
            }
        }
        match expire_pending_send(&mut self.pending_send, Instant::now()) {
            Some(PendingSendExpiration::QueuedMessage(queued)) => {
                let error = send_age_expiry_error(&queued);
                report_queued_failure(&self.events, &mut self.pending_notifications, self.connection.simulator_id, queued, error)?;
            }
            Some(PendingSendExpiration::ReliableRetry) => return Err(ConnectionError::ReliableRetryExpired),
            None => {}
        }
        Ok(())
    }

    fn pending_send_deadline(&self) -> Option<Instant> {
        let pending = self.pending_send.as_ref()?;
        pending.queued_message.as_ref().map(|queued| queued.queued_at + MAX_SEND_AGE).or(pending.retry_deadline)
    }

    fn begin_draining_when_ready(&mut self) {
        if self.phase.is_closing() && self.pending_events.is_empty() && !self.phase.logout_requested() {
            self.phase.begin_draining();
        }
    }

    fn discard_application_sends_while_draining(&mut self) {
        if !self.phase.is_draining() {
            return;
        }
        for queued in self.outbound.drain() {
            fail_queued_on_close(&self.events, &mut self.pending_notifications, self.connection.simulator_id, queued);
        }
        if let Some(queued) = self.pending_handshake_reply.take() {
            fail_queued_on_close(&self.events, &mut self.pending_notifications, self.connection.simulator_id, queued);
        }
        if self.pending_send.as_ref().is_some_and(|pending| !pending.meta.is_acknowledgement)
            && let Some(mut canceled) = self.pending_send.take()
            && let Some(queued) = canceled.queued_message.take()
        {
            fail_queued_on_close(&self.events, &mut self.pending_notifications, self.connection.simulator_id, queued);
        }
    }

    fn drain_is_complete(&self) -> bool {
        self.phase.is_draining()
            && self.pending_send.is_none()
            && self.pending_handshake_reply.is_none()
            && !self.connection.circuit.has_pending_acks()
            && self.pending_notifications.is_empty()
    }

    fn prepare_send(&mut self) -> Result<bool, ConnectionError> {
        if self.pending_send.is_some() {
            return Ok(true);
        }
        if let Some(queued) = self.priority_logout.take() {
            let prepared = match self.connection.circuit.prepare_message(queued.message.clone(), queued.delivery) {
                Ok(prepared) => prepared,
                Err(error) => {
                    let error = ConnectionError::from(error);
                    self.fail_logout(error.clone());
                    report_queued_failure(
                        &self.events,
                        &mut self.pending_notifications,
                        self.connection.simulator_id,
                        queued,
                        error.clone(),
                    )?;
                    return Err(error);
                }
            };
            self.pending_send = Some(begin_send(
                Arc::clone(&self.connection.socket),
                self.connection.pacer.handle(),
                prepared,
                TrafficClass::Control,
                Some(queued),
            ));
            return Ok(true);
        }
        let next = if self.phase.logout_requested() {
            prepare_shutdown_send(
                &self.connection.socket,
                &self.connection.pacer,
                &mut self.connection.circuit,
                &mut self.outbound,
                self.ack_timer_due,
            )
        } else {
            prepare_next_send(
                &self.connection.socket,
                &self.connection.pacer,
                &mut self.connection.circuit,
                &mut self.outbound,
                &mut self.deferred_retries,
                self.ack_timer_due || self.phase.is_draining(),
                self.last_send_was_retry,
            )
        };
        match next {
            Ok(next) => self.pending_send = next,
            Err((Some(queued), error)) => {
                report_queued_failure(&self.events, &mut self.pending_notifications, self.connection.simulator_id, *queued, error)?;
                return Ok(false);
            }
            Err((None, error)) => return Err(error),
        }
        Ok(true)
    }

    fn admit_pending_handshake_reply(&mut self) -> Result<(), ConnectionError> {
        let Some(queued) = self.pending_handshake_reply.take() else { return Ok(()) };
        match self.outbound.push(queued, active_queue_class(self.pending_send.as_ref())) {
            Ok(()) => {
                if self.phase.has_region_handshake() {
                    self.connection.region_handshake_reply_sent = true;
                }
                preempt_deferred_send(
                    &mut self.pending_send,
                    &mut self.outbound,
                    &self.connection.circuit,
                    self.ack_timer_due || self.phase.is_draining(),
                )?;
                if self.phase.is_established()
                    && self.phase.has_region_handshake()
                    && self.connection.region_handshake_reply_sent
                    && self.connection.handoff.is_some()
                {
                    self.connection.queue_handoff_movement(&mut self.outbound, self.pending_send.as_ref())?;
                }
            }
            Err(queued) => self.pending_handshake_reply = Some(*queued),
        }
        Ok(())
    }

    fn send_completed(&mut self, sent: Result<(Instant, PreparedDatagram), ConnectionError>) -> Result<RunControl, ConnectionError> {
        let mut completed = self.pending_send.take().expect("send branch is guarded");
        match sent {
            Ok((sent_at, prepared)) => {
                let was_retry = completed.meta.is_retransmission;
                let sequence = completed.meta.sequence;
                let was_logout =
                    completed.queued_message.as_ref().is_some_and(|queued| matches!(&queued.message, Message::LogoutRequest(_)));
                self.connection.circuit.commit_sent(prepared, sent_at)?;
                if was_retry {
                    self.deferred_retries.remove(&sequence);
                }
                self.last_send_was_retry = was_retry;
                if !self.connection.circuit.has_pending_acks() {
                    self.ack_timer_due = false;
                }
                if let Some(queued) = &completed.queued_message
                    && let Some(cancel_target) = queued.cancel_target
                {
                    queue_send_notification(
                        &self.events,
                        &mut self.pending_notifications,
                        self.connection.simulator_id,
                        Some(cancel_target),
                        None,
                        false,
                    )?;
                }
                if let Some(queued) = &mut completed.queued_message
                    && let Some(completion) = queued.completion.take()
                    && let Err(error) = completion.send(Ok(()))
                {
                    drop(error);
                }
                if was_logout {
                    self.phase.complete_logout(None);
                }
                if let Some(queued) = &completed.queued_message {
                    match queued.movement {
                        MovementSend::Initial => self.connection.initial_movement_sent = true,
                        MovementSend::Handoff => self.connection.movement_requested = true,
                        MovementSend::None => {}
                    }
                    if self.phase.is_established()
                        && self.phase.has_region_handshake()
                        && self.connection.region_handshake_reply_sent
                        && self.connection.handoff.is_some()
                    {
                        self.connection.queue_handoff_movement(&mut self.outbound, self.pending_send.as_ref())?;
                    }
                }
            }
            Err(error) => {
                let was_logout =
                    completed.queued_message.as_ref().is_some_and(|queued| matches!(&queued.message, Message::LogoutRequest(_)));
                if let Some(queued) = completed.queued_message.take() {
                    report_queued_failure(
                        &self.events,
                        &mut self.pending_notifications,
                        self.connection.simulator_id,
                        queued,
                        error.clone(),
                    )?;
                }
                if was_logout {
                    self.fail_logout(error.clone());
                }
                return Err(error);
            }
        }
        Ok(RunControl::Continue)
    }

    fn logout_command(&mut self, command: Option<oneshot::Sender<Result<(), Error>>>) -> Result<RunControl, ConnectionError> {
        let Some(completed) = command else {
            self.logout_commands = None;
            return Ok(RunControl::Continue);
        };
        match self.phase.request_logout(completed) {
            LogoutAction::Queue(completed) => {
                if let Some(mut canceled) = self.pending_send.take()
                    && let Some(queued) = canceled.queued_message.take()
                {
                    fail_queued_on_close(&self.events, &mut self.pending_notifications, self.connection.simulator_id, queued);
                }
                for queued in self.outbound.drain() {
                    fail_queued_on_close(&self.events, &mut self.pending_notifications, self.connection.simulator_id, queued);
                }
                self.priority_logout = Some(QueuedMessage {
                    message: logout_message(&self.connection.session),
                    delivery: Delivery::ReliableNoRetry,
                    class: TrafficClass::Control,
                    queued_at: Instant::now(),
                    completion: Some(completed),
                    cancel_target: None,
                    report_failure: false,
                    movement: MovementSend::None,
                });
            }
            LogoutAction::Complete(completed) => {
                if let Err(error) = completed.send(Ok(())) {
                    drop(error);
                }
            }
            LogoutAction::Waiting => {}
        }
        Ok(RunControl::Continue)
    }

    fn teardown_deadline(&self) -> RunControl {
        RunControl::Finish(self.phase.close_reason())
    }

    fn keepalive_timeout(&self) -> Result<RunControl, ConnectionError> {
        Err(ConnectionError::KeepaliveTimeout)
    }

    async fn primary_state_changed(&mut self, changed: Result<(), watch::error::RecvError>) -> Result<RunControl, ConnectionError> {
        match changed {
            Ok(()) => {
                let receiver = self.connection.primary_state_receiver.as_mut().expect("primary-state branch is guarded");
                let state = *receiver.borrow_and_update();
                self.connection.invalidate_stale_handoff(state.role_generation(), &mut self.outbound, &mut self.pending_send);
                self.connection.primary_state = state;
                self.connection.pacer.set_primary(state.connection_role() == ConnectionRole::Primary).await?;
            }
            Err(_) => self.connection.primary_state_receiver = None,
        }
        Ok(RunControl::Continue)
    }

    fn ack_tick(&mut self) -> Result<RunControl, ConnectionError> {
        self.ack_timer_due = self.connection.circuit.has_pending_acks();
        preempt_deferred_send(
            &mut self.pending_send,
            &mut self.outbound,
            &self.connection.circuit,
            self.ack_timer_due || self.phase.is_draining(),
        )?;
        Ok(RunControl::Continue)
    }

    fn deliver_event(
        permit: Result<mpsc::OwnedPermit<SimulatorEvent>, mpsc::error::SendError<()>>,
        pending_notifications: &mut VecDeque<SimulatorEvent>,
        pending_events: &mut VecDeque<EventEnvelope>,
        event_delivery_turn: &mut bool,
    ) -> RunControl {
        match permit {
            Ok(permit) => {
                if let Some(notification) = pending_notifications.pop_front() {
                    permit.send(notification);
                } else if let Some(event) = pending_events.pop_front() {
                    permit.send(SimulatorEvent::Simulator(event));
                }
                *event_delivery_turn = false;
                RunControl::Continue
            }
            Err(_) => RunControl::Finish(None),
        }
    }

    fn ping_tick(&mut self) -> Result<RunControl, ConnectionError> {
        if let Some(prepared) = self.connection.circuit.prepare_ping(Instant::now())? {
            self.pending_send = Some(begin_send(
                Arc::clone(&self.connection.socket),
                self.connection.pacer.handle(),
                prepared,
                TrafficClass::Control,
                None,
            ));
        }
        Ok(RunControl::Continue)
    }

    fn command(&mut self, command: Option<SimulatorCommand>) -> Result<RunControl, ConnectionError> {
        match command {
            Some(SimulatorCommand::ReplyRegionHandshake { message, completed }) => {
                if self.phase.is_closing() {
                    if let Err(error) = completed.send(Err(Error::SimulatorConnectionClosed)) {
                        drop(error);
                    }
                } else {
                    let queued = QueuedMessage {
                        class: TrafficClass::Control,
                        message,
                        delivery: Delivery::Reliable,
                        queued_at: Instant::now(),
                        completion: Some(completed),
                        cancel_target: None,
                        report_failure: false,
                        movement: MovementSend::None,
                    };
                    match self.outbound.push(queued, active_queue_class(self.pending_send.as_ref())) {
                        Ok(()) => {
                            if self.phase.has_region_handshake() {
                                self.connection.region_handshake_reply_sent = true;
                            }
                            preempt_deferred_send(
                                &mut self.pending_send,
                                &mut self.outbound,
                                &self.connection.circuit,
                                self.ack_timer_due || self.phase.is_draining(),
                            )?;
                            if self.phase.is_established()
                                && self.phase.has_region_handshake()
                                && self.connection.region_handshake_reply_sent
                                && self.connection.handoff.is_some()
                            {
                                self.connection.queue_handoff_movement(&mut self.outbound, self.pending_send.as_ref())?;
                            }
                        }
                        Err(mut queued) => {
                            if self.pending_handshake_reply.is_some() {
                                if let Some(completed) = queued.completion.take()
                                    && let Err(error) = completed.send(Err(Error::SimulatorCommandQueueFull))
                                {
                                    drop(error);
                                }
                                return Err(ConnectionError::SimulatorCommandQueueFull);
                            }
                            self.pending_handshake_reply = Some(*queued);
                        }
                    }
                }
            }
            Some(SimulatorCommand::Send { message, delivery, completed, cancel_target, report_failure }) => {
                let message_traffic_class = message_class(&message);
                let mut failed_completion = completed;
                let mut failed_cancel_target = cancel_target;
                let failed_report = report_failure;
                let error = if self.phase.is_closing() {
                    Some(Error::SimulatorConnectionClosed)
                } else if !self.phase.is_established() {
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
                    match self.outbound.push(queued, active_queue_class(self.pending_send.as_ref())) {
                        Ok(()) => {
                            preempt_deferred_send(
                                &mut self.pending_send,
                                &mut self.outbound,
                                &self.connection.circuit,
                                self.ack_timer_due || self.phase.is_draining(),
                            )?;
                            None
                        }
                        Err(mut queued) => {
                            failed_completion = queued.completion.take();
                            failed_cancel_target = queued.cancel_target.take();
                            Some(Error::SimulatorCommandQueueFull)
                        }
                    }
                };
                if let Some(error) = error {
                    let message = match &error {
                        Error::SimulatorConnectionClosed => ConnectionError::SimulatorConnectionClosed,
                        Error::SimulatorNotReady => ConnectionError::SimulatorNotReady,
                        Error::SimulatorCommandQueueFull => ConnectionError::SimulatorCommandQueueFull,
                        _ => return Err(ConnectionError::SimulatorManagerClosed),
                    };
                    if let Some(completed) = failed_completion
                        && let Err(error) = completed.send(Err(error))
                    {
                        drop(error);
                    }
                    if failed_report || failed_cancel_target.is_some() {
                        queue_send_notification(
                            &self.events,
                            &mut self.pending_notifications,
                            self.connection.simulator_id,
                            failed_cancel_target,
                            Some(message),
                            failed_report,
                        )?;
                    }
                }
            }
            Some(SimulatorCommand::PrepareHandoff(target)) => {
                if Some(target.region_handle) != self.connection.region_handle
                    || target.endpoint != self.connection.session.simulator_address
                    || target.agent_id != self.connection.session.agent_id
                    || target.session_id != self.connection.session.session_id
                {
                    return Ok(RunControl::Continue);
                }
                let role_generation = self.connection.current_primary_state().role_generation();
                self.connection.invalidate_stale_handoff(role_generation, &mut self.outbound, &mut self.pending_send);
                if self.connection.handoff.as_ref() == Some(&target) {
                    return Ok(RunControl::Continue);
                }
                self.connection.clear_handoff(&mut self.outbound, &mut self.pending_send);
                self.connection.session.seed_capability = target.seed_capability.clone();
                self.connection.handoff = Some(target);
                self.connection.handoff_role_generation = Some(role_generation);
                if self.phase.is_established() && self.phase.has_region_handshake() && self.connection.region_handshake_reply_sent {
                    self.connection.queue_handoff_movement(&mut self.outbound, self.pending_send.as_ref())?;
                }
            }
            Some(SimulatorCommand::CancelHandoff) => {
                self.connection.clear_handoff(&mut self.outbound, &mut self.pending_send);
            }
            None => return Ok(RunControl::Finish(None)),
        }
        Ok(RunControl::Continue)
    }

    fn fail_logout(&mut self, error: ConnectionError) {
        self.phase.complete_logout(Some(error));
    }
}

fn send_age_expiry_error(queued: &QueuedMessage) -> ConnectionError {
    ConnectionError::SendAgeExpired { logout: matches!(&queued.message, Message::LogoutRequest(_)) }
}

fn finish_connection_logout(reason: Option<&'static str>) -> Result<(), ConnectionError> {
    match reason {
        Some(reason) => Err(ConnectionError::ShutdownReason(reason)),
        None => Ok(()),
    }
}

pub(super) async fn run(
    connection: super::SimulatorConnection,
    events: mpsc::Sender<SimulatorEvent>,
    shutdown: watch::Receiver<bool>,
    neighbor_requests: mpsc::Sender<SimulatorRequest>,
    commands: mpsc::Receiver<SimulatorCommand>,
    logout_commands: mpsc::Receiver<oneshot::Sender<Result<(), Error>>>,
    packet_memory: SimulatorMemoryBudget,
) -> Result<(), ConnectionError> {
    let mut ack_timer = time::interval_at(Instant::now() + ACK_INTERVAL, ACK_INTERVAL);
    ack_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    let mut ping_timer = time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
    ping_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
    ConnectionTask {
        connection,
        events,
        shutdown,
        neighbor_requests,
        commands,
        logout_commands: Some(logout_commands),
        packet_memory,
        phase: Phase::new(),
        preconnect_messages: VecDeque::new(),
        pending_events: VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS),
        pending_notifications: VecDeque::with_capacity(MAX_PENDING_SEND_NOTIFICATIONS),
        outbound: super::OutboundQueue::new(),
        pending_handshake_reply: None,
        pending_send: None,
        deferred_retries: HashMap::new(),
        priority_logout: None,
        ack_timer,
        ack_timer_due: false,
        ping_timer,
        last_udp_received: Instant::now(),
        last_send_was_retry: false,
        event_delivery_turn: false,
        packet_rejections: PacketRejections::default(),
    }
    .run()
    .await
}
