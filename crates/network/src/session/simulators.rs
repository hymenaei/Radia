use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    num::NonZeroU64,
    ops::{Deref, DerefMut},
    time::Duration,
};

use llsd::Value;
use protocol::{lludp::Message, login::ResponseData};
#[cfg(test)]
use tokio::time;
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{AbortHandle, Id, JoinError, JoinSet},
    time::Instant,
};
use url::Url;
use uuid::Uuid;

use super::capability_events::{decode_agent_communication, decode_enable_simulators};
use super::handoff::{HandoffAnnouncement, HandoffOrder, HandoffState, PromotionSetup, cancel_pending_handoff, teleport_failure_matches};
use super::logout::{LogoutUnavailable, PendingLogout};
use super::routing::{
    EventQueueOutcome, PreconnectEvents, PreconnectOutcome, PrimaryEventBacklog, primary_event_queue_size, queue_simulator_event,
    queue_until_connected,
};
use super::runtime::SessionDispatcher;
use super::teleport::{decode_crossed_region, decode_event_teleport_failed, decode_event_teleport_finish, is_teleport_cancel};
use super::{Command, SessionError, SessionOptions};
use crate::{
    AgentSession,
    capabilities::{CapabilityBudget, Client, EventQueueExit, EventQueueRoleHandle},
    circuit::{
        HandoffCancellation, HandoffGeneration, HandoffTarget, LOGOUT_TIMEOUT, MAX_PENDING_SIMULATOR_EVENTS, MAX_SIMULATOR_CONNECTIONS,
        PrimaryState, SimulatorCommand, SimulatorEvent, SimulatorRequest, SimulatorTaskOutcome, UdpTransport,
        admission::{ConnectionRole, SESSION_APPLICATION_EVENT_CREDIT_CAPACITY, SimulatorMemoryBudget},
        lludp::Delivery,
        pacing::OutboundBudget,
    },
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource, record_coverage_loss, record_event_envelope_loss},
    error::Error,
    event::{Event, EventEnvelope, SimulatorId},
};

#[cfg(test)]
pub(in crate::session) use tests::publish_primary_terminal_event;

pub(super) const SESSION_TIMEOUT: Duration = Duration::from_secs(40);
pub(super) const EVENT_BACKLOG_WARNING_THRESHOLD: usize = MAX_PENDING_SIMULATOR_EVENTS / 2;
pub(super) const EVENT_BACKLOG_WARNING_INTERVAL: Duration = Duration::from_secs(10);
pub(super) const MAX_PENDING_REGION_HANDSHAKE_REPLIES: usize = MAX_SIMULATOR_CONNECTIONS;
pub(super) const MAX_PENDING_LIFECYCLE_EVENTS: usize = 32;
pub(super) const MAX_PENDING_SESSION_EVENTS: usize = SESSION_APPLICATION_EVENT_CREDIT_CAPACITY + MAX_PENDING_LIFECYCLE_EVENTS;
pub(super) const APPLICATION_EVENT_PRESSURE_GRACE_PERIOD: Duration = Duration::from_secs(30);
pub(super) const MAX_TRACKED_CAPABILITY_CLIENTS: usize = MAX_SIMULATOR_CONNECTIONS;
pub(super) const NEIGHBOR_COMMAND_CAPACITY: usize = 8;
pub(super) const HANDOFF_COMMAND_RETRY_INTERVAL: Duration = Duration::from_millis(10);

pub(super) type ConnectionTaskResult = (SimulatorId, Option<u64>, SocketAddr, SimulatorTaskOutcome);
pub(super) type NeighborEventQueueJoin = Option<Result<(Id, (SimulatorId, SocketAddr, EventQueueExit)), JoinError>>;

pub(super) struct ConnectionQueueFailure {
    pub(super) error: Error,
    pub(super) completion: Option<oneshot::Sender<Result<(), Error>>>,
}

pub(super) struct SimulatorConnectionState {
    pub(super) simulator_id: SimulatorId,
    pub(super) endpoint: SocketAddr,
    pub(super) abort: AbortHandle,
    pub(super) commands: mpsc::Sender<SimulatorCommand>,
    pub(super) logout: mpsc::Sender<oneshot::Sender<Result<(), Error>>>,
    pub(super) pending_handoff_command: Option<HandoffCommand>,
    pub(super) primary_state: Option<watch::Sender<PrimaryState>>,
    pub(super) packet_memory: SimulatorMemoryBudget,
}

impl SimulatorConnectionState {
    pub(super) fn connection_role(&self) -> ConnectionRole {
        self.primary_state.as_ref().map_or(ConnectionRole::Neighbor, |state| state.borrow().connection_role())
    }

    pub(super) fn log_disconnected(&self, region_handle: u64) {
        tracing::info!(
            simulator = self.simulator_id.get(),
            region = region_handle,
            role = ?self.connection_role(),
            "simulator disconnected"
        );
    }

    pub(super) fn queue_handoff_command(&mut self, command: HandoffCommand) -> bool {
        self.pending_handoff_command = Some(command);
        self.flush_handoff_command();
        !self.commands.is_closed()
    }

    pub(super) fn flush_handoff_command(&mut self) {
        let Some(command) = self.pending_handoff_command.take() else { return };
        let command = match command {
            HandoffCommand::Prepare(target) => SimulatorCommand::PrepareHandoff(target),
            HandoffCommand::Cancel => SimulatorCommand::CancelHandoff,
        };
        match self.commands.try_send(command) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(command)) => {
                self.pending_handoff_command = Some(match command {
                    SimulatorCommand::PrepareHandoff(target) => HandoffCommand::Prepare(target),
                    SimulatorCommand::CancelHandoff => HandoffCommand::Cancel,
                    SimulatorCommand::ReplyRegionHandshake { .. } => unreachable!("handshake replies are not deferred handoff commands"),
                    SimulatorCommand::Send { .. } => unreachable!("only handoff control commands are deferred"),
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

pub(super) struct PrimaryRoute {
    pub(super) simulator_id: SimulatorId,
    pub(super) memory: SimulatorMemoryBudget,
    pub(super) endpoint: SocketAddr,
    pub(super) region_handle: Option<u64>,
    pub(super) commands: mpsc::Sender<SimulatorCommand>,
    pub(super) state: watch::Sender<PrimaryState>,
    pub(super) logout: mpsc::Sender<oneshot::Sender<Result<(), Error>>>,
}

impl PrimaryRoute {
    pub(super) fn from_neighbor(region_handle: u64, connection: &SimulatorConnectionState) -> Option<Self> {
        Some(Self {
            simulator_id: connection.simulator_id,
            memory: connection.packet_memory.clone(),
            endpoint: connection.endpoint,
            region_handle: Some(region_handle),
            commands: connection.commands.clone(),
            state: connection.primary_state.clone()?,
            logout: connection.logout.clone(),
        })
    }
}

pub(super) enum HandoffCommand {
    Prepare(HandoffTarget),
    Cancel,
}

pub(super) struct SessionRuntimeOptions {
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) session: SessionOptions,
    pub(super) startup_disconnect: Option<watch::Receiver<bool>>,
    pub(super) root_transport: Option<UdpTransport>,
    #[cfg(test)]
    pub(super) application_event_capacity: usize,
}

impl SessionRuntimeOptions {
    pub(super) fn application_event_capacity(&self) -> usize {
        #[cfg(test)]
        {
            self.application_event_capacity
        }
        #[cfg(not(test))]
        {
            SESSION_APPLICATION_EVENT_CREDIT_CAPACITY
        }
    }
}

pub(super) struct SessionRuntime {
    pub(super) state: SessionState,
    pub(super) runtime_inputs: RuntimeInputs,
}

pub(super) struct SessionState {
    pub(super) simulator_id: SimulatorId,
    pub(super) primary: PrimaryRoute,
    pub(super) root_connection_abort: Option<AbortHandle>,
    pub(super) neighbor_events: mpsc::Sender<SimulatorEvent>,
    pub(super) neighbor_requests: mpsc::Sender<SimulatorRequest>,
    pub(super) neighbor_cancellation: watch::Sender<bool>,
    pub(super) root_connection_failed: bool,
    pub(super) logout_request_sent: bool,
    pub(super) logout_attempted: bool,
    pub(super) logout_unavailable: Option<LogoutUnavailable>,
    pub(super) pending_primary_logout: Option<PendingLogout>,
    pub(super) session: AgentSession,
    pub(super) capabilities: Client,
    pub(super) capability_budget: CapabilityBudget,
    pub(super) capability_request_waiters: JoinSet<()>,
    pub(super) login_data: ResponseData,
    pub(super) event_backlog: PrimaryEventBacklog,
    pub(super) packet_memory: SimulatorMemoryBudget,
    pub(super) coverage_losses: CoverageLossLedger,
    pub(super) outbound_budget: Option<OutboundBudget>,
    pub(super) handoff_generation: Option<u64>,
}

pub(super) struct RuntimeInputs {
    pub(super) neighbor_event_receiver: mpsc::Receiver<SimulatorEvent>,
    pub(super) neighbor_request_receiver: mpsc::Receiver<SimulatorRequest>,
    pub(super) root_connection_workers: JoinSet<ConnectionTaskResult>,
}

impl Deref for SessionRuntime {
    type Target = SessionState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for SessionRuntime {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Drop for SessionState {
    fn drop(&mut self) {
        self.neighbor_cancellation.send_replace(true);
        self.coverage_losses.retire(self.simulator_id, CoverageSource::Lludp);
        self.capabilities.close_session();
    }
}

impl SessionState {
    pub(super) fn promote_primary(&mut self, next: PrimaryRoute, role_generation: u64) {
        self.primary.memory.promote_to(&next.memory);
        self.primary.state.send_replace(PrimaryState::Neighbor { generation: role_generation });
        next.state.send_replace(PrimaryState::Ready { generation: role_generation });
        self.primary = next;
    }

    pub(super) fn next_handoff_generation(&mut self) -> Option<HandoffGeneration> {
        let Some(generation) = self.handoff_generation.and_then(|generation| generation.checked_add(1)) else {
            if self.handoff_generation.is_some() {
                self.handoff_generation = None;
                tracing::error!(simulator = self.simulator_id.get(), "simulator handoff generation space is exhausted");
            }
            return None;
        };
        self.handoff_generation = Some(generation);
        NonZeroU64::new(generation).map(HandoffGeneration::new)
    }
}

impl SessionRuntime {
    #[tracing::instrument(name = "simulator.connect", level = "info", skip_all)]
    pub(super) async fn connect(
        session: AgentSession,
        login_data: ResponseData,
        options: SessionRuntimeOptions,
    ) -> Result<Option<Self>, SessionError> {
        Self::connect_inner(session, login_data, options).await
    }

    pub(super) async fn run(
        self,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        disconnect_requested: watch::Receiver<bool>,
        terminal_event_permit: mpsc::OwnedPermit<EventEnvelope>,
    ) -> Result<(), SessionError> {
        let Self { mut state, runtime_inputs } = self;
        let simulator_id = state.simulator_id;
        let result = match SessionDispatcher::start(&mut state, runtime_inputs, commands, events, disconnect_requested).await {
            Ok(mut runtime) => {
                let result = runtime.run().await;
                runtime.shutdown().await;
                result
            }
            Err(error) => Err(error),
        };
        if let Some(outbound_budget) = state.outbound_budget.take() {
            outbound_budget.shutdown().await;
        }
        state.finish_capability_request_waiters().await;
        if !state.root_connection_failed || state.primary.simulator_id != simulator_id {
            SessionState::publish_primary_terminal_event(
                terminal_event_permit,
                state.primary.simulator_id,
                state.primary.region_handle,
                state.primary.endpoint,
                &result,
            );
        }
        result
    }
}

impl SessionState {
    pub(super) fn new_capability_client(&mut self, seed: Url) -> Result<Client, SessionError> {
        if self.capability_request_waiters.len() >= MAX_TRACKED_CAPABILITY_CLIENTS {
            return Err(SessionError::CapabilityClientLimit);
        }
        let client = Client::new_with_budget(seed, self.capability_budget.clone())?;
        track_capability_requests(&mut self.capability_request_waiters, &client);
        Ok(client)
    }

    pub(super) async fn finish_capability_request_waiters(&mut self) {
        self.capabilities.close_session();
        while let Some(result) = self.capability_request_waiters.join_next().await {
            if result.is_err() {
                tracing::error!("Capability request waiter task failed");
            }
        }
    }

    fn publish_primary_terminal_event(
        permit: mpsc::OwnedPermit<EventEnvelope>,
        simulator_id: SimulatorId,
        region_handle: Option<u64>,
        endpoint: SocketAddr,
        result: &Result<(), SessionError>,
    ) {
        let event = match result {
            Ok(()) => Event::SimulatorDisconnected { simulator_id, region_handle, endpoint },
            Err(reason) => Event::SimulatorFailed { simulator_id, region_handle, endpoint, reason: reason.to_string() },
        };
        permit.send(EventEnvelope::new(event));
    }

    pub(super) async fn publish_event(
        &mut self,
        events: &mpsc::Sender<EventEnvelope>,
        state: &mut RunState,
        event: impl Into<EventEnvelope>,
    ) -> Result<(), SessionError> {
        let RunState { pending_events, pending_lifecycle_events, logout_deadline, .. } = state;
        let mut event = event.into();
        if logout_deadline.is_some() && event.event.is_application() {
            if let Some(watermark) = event.coverage_watermark() {
                record_coverage_loss(&self.coverage_losses, watermark.simulator_id, watermark.source, CoverageLossReason::EventAdmission);
            }
            return Ok(());
        }
        let is_application = event.event.is_application();
        let acquired_application_credit = is_application && !event.has_application_credit();
        if acquired_application_credit {
            let Some(permit) = self.packet_memory.application_events().try_acquire() else {
                if self.packet_memory.application_events().note_failure() {
                    tracing::warn!("application event intake is under pressure");
                }
                if let Some(watermark) = event.coverage_watermark() {
                    record_coverage_loss(
                        &self.coverage_losses,
                        watermark.simulator_id,
                        watermark.source,
                        CoverageLossReason::EventAdmission,
                    );
                }
                return Ok(());
            };
            event.attach_application_credit(permit);
        }
        let watermark = event.coverage_watermark();
        let outcome = queue_simulator_event(events, pending_events, pending_lifecycle_events, event);
        let should_logout = match outcome {
            EventQueueOutcome::Queued => false,
            EventQueueOutcome::ReceiverClosed => {
                if let Some(watermark) = watermark {
                    record_coverage_loss(
                        &self.coverage_losses,
                        watermark.simulator_id,
                        watermark.source,
                        CoverageLossReason::EventAdmission,
                    );
                }
                for queued in pending_events.drain(..) {
                    record_event_envelope_loss(&self.coverage_losses, queued, CoverageLossReason::EventAdmission);
                }
                *pending_lifecycle_events = 0;
                true
            }
            EventQueueOutcome::BacklogFull => {
                if let Some(watermark) = watermark {
                    record_coverage_loss(
                        &self.coverage_losses,
                        watermark.simulator_id,
                        watermark.source,
                        CoverageLossReason::EventAdmission,
                    );
                }
                return Err(SessionError::ApplicationEventBacklogExceeded);
            }
            EventQueueOutcome::LifecycleBacklogFull => {
                return Err(SessionError::LifecycleEventBacklogFull);
            }
        };

        if acquired_application_credit && !should_logout {
            self.packet_memory.application_events().note_success();
        }

        let now = Instant::now();
        if let Some(backlog_duration_ms) = self.event_backlog.observe(pending_events.len(), now) {
            let (queue_occupancy, queue_capacity) = primary_event_queue_size(events, pending_events.len());
            tracing::warn!(
                queue_occupancy,
                queue_capacity,
                backlog_duration_ms,
                "primary simulator application-event queue is under pressure"
            );
        }

        if should_logout && logout_deadline.is_none() {
            let deadline = Instant::now() + LOGOUT_TIMEOUT;
            *logout_deadline = Some(deadline);
            self.logout_attempted = true;
            let (completed, response) = oneshot::channel();
            match self.primary.logout.try_send(completed) {
                Ok(()) => self.pending_primary_logout = Some(PendingLogout { completion: response, deadline }),
                Err(mpsc::error::TrySendError::Full(_)) => self.record_logout_unavailable(LogoutUnavailable::QueueFull),
                Err(mpsc::error::TrySendError::Closed(_)) => self.record_logout_unavailable(LogoutUnavailable::ConnectionClosed),
            }
        }
        Ok(())
    }

    pub(super) async fn publish_simulator_event(
        &mut self,
        simulator_id: SimulatorId,
        event: EventEnvelope,
        events: &mpsc::Sender<EventEnvelope>,
        state: &mut RunState,
    ) -> Result<(), SessionError> {
        let watermark = event.coverage_watermark();
        match queue_until_connected(&state.connected_simulators, &mut state.preconnect_events, simulator_id, event) {
            PreconnectOutcome::Ready(event) => self.publish_event(events, state, event).await,
            PreconnectOutcome::Buffered => Ok(()),
            PreconnectOutcome::Full => {
                if let Some(watermark) = watermark {
                    record_coverage_loss(
                        &self.coverage_losses,
                        watermark.simulator_id,
                        watermark.source,
                        CoverageLossReason::EventAdmission,
                    );
                }
                Ok(())
            }
        }
    }

    pub(super) async fn publish_simulator_connected(
        &mut self,
        simulator_id: SimulatorId,
        event: impl Into<EventEnvelope>,
        events: &mpsc::Sender<EventEnvelope>,
        state: &mut RunState,
    ) -> Result<(), SessionError> {
        self.publish_event(events, state, event).await?;
        state.connected_simulators.insert(simulator_id);
        for event in state.preconnect_events.take(simulator_id) {
            self.publish_event(events, state, event).await?;
        }
        Ok(())
    }

    pub(super) async fn publish_session_ready(
        &mut self,
        events: &mpsc::Sender<EventEnvelope>,
        state: &mut RunState,
    ) -> Result<(), SessionError> {
        self.publish_event(
            events,
            state,
            Event::SessionReady {
                simulator_id: self.simulator_id,
                session: Box::new(self.session.clone()),
                capabilities: self.capabilities.clone(),
                login_data: self.login_data.clone(),
            },
        )
        .await
    }

    pub(super) async fn process_capability_event(
        &mut self,
        simulator_id: SimulatorId,
        message: &str,
        body: &Value,
        order: Option<HandoffOrder>,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), SessionError> {
        if setup.state.logout_deadline.is_some() {
            return Ok(());
        }
        match message {
            "EnableSimulator" => self.process_enable_simulators(simulator_id, body, setup).await,
            "CrossedRegion" => self.process_crossed_region(simulator_id, body, setup, handoff_state).await,
            "TeleportFinish" => self.process_teleport_finish(simulator_id, body, order, setup, handoff_state).await,
            "TeleportFailed" => self.process_teleport_failed(body, order, setup, handoff_state),
            "EstablishAgentCommunication" => self.process_agent_communication(body, setup).await,
            _ => Ok(()),
        }
    }

    pub(super) async fn process_enable_simulators(
        &mut self,
        simulator_id: SimulatorId,
        body: &Value,
        setup: &mut PromotionSetup<'_>,
    ) -> Result<(), SessionError> {
        match decode_enable_simulators(body) {
            Ok(simulators) => {
                for (region_handle, endpoint) in simulators {
                    if setup.state.logout_deadline.is_some() {
                        break;
                    }
                    self.start_neighbor(simulator_id, region_handle, endpoint, None, setup).await?;
                }
            }
            Err(_reason) => {}
        }
        Ok(())
    }

    pub(super) async fn process_crossed_region(
        &mut self,
        simulator_id: SimulatorId,
        body: &Value,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), SessionError> {
        match decode_crossed_region(body, self.session.agent_id, self.session.session_id) {
            Ok(handoff) => self.request_handoff(simulator_id, handoff, HandoffAnnouncement::CrossedRegion, setup, handoff_state).await,
            Err(_reason) => Ok(()),
        }
    }

    pub(super) async fn process_teleport_finish(
        &mut self,
        simulator_id: SimulatorId,
        body: &Value,
        order: Option<HandoffOrder>,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), SessionError> {
        match decode_event_teleport_finish(body, self.session.agent_id, self.session.session_id) {
            Ok(handoff) => {
                self.request_handoff(
                    simulator_id,
                    handoff,
                    HandoffAnnouncement::TeleportFinish(order.unwrap_or(HandoffOrder::Unavailable)),
                    setup,
                    handoff_state,
                )
                .await
            }
            Err(_reason) => Ok(()),
        }
    }

    pub(super) fn process_teleport_failed(
        &mut self,
        body: &Value,
        order: Option<HandoffOrder>,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), SessionError> {
        match decode_event_teleport_failed(body, self.session.agent_id) {
            Ok(()) if teleport_failure_matches(handoff_state.pending(), order.unwrap_or(HandoffOrder::Unavailable)) => {
                if let Some(pending) = handoff_state.take_pending() {
                    cancel_pending_handoff(
                        pending,
                        "teleport failed",
                        self.simulator_id,
                        &mut setup.state.neighbor_connections,
                        &mut setup.state.retiring_simulators,
                        &mut setup.state.preconnect_events,
                        &mut setup.state.connected_simulators,
                        &mut setup.state.simulator_capabilities,
                        &self.coverage_losses,
                    );
                }
            }
            Ok(()) => {}
            Err(_reason) => {}
        }
        Ok(())
    }

    pub(super) async fn process_agent_communication(&mut self, body: &Value, setup: &mut PromotionSetup<'_>) -> Result<(), SessionError> {
        let (endpoint, seed) = match decode_agent_communication(body, self.session.agent_id) {
            Ok(event) => event,
            Err(_reason) => {
                return Ok(());
            }
        };
        if endpoint == self.primary.endpoint {
            return Ok(());
        }
        let Some(target_id) =
            setup.state.neighbor_connections.values().find(|active| active.endpoint == endpoint).map(|active| active.simulator_id)
        else {
            return Ok(());
        };
        if setup.state.simulator_capabilities.contains_key(&target_id) {
            return Ok(());
        }
        self.start_neighbor_capabilities(target_id, endpoint, seed, setup).await
    }

    pub(super) fn note_primary_event_queue_exit(&self, exit: EventQueueExit) {
        let reason = match exit {
            EventQueueExit::Stopped => return,
            EventQueueExit::StoppedWithCoverageLoss => "The primary simulator EventQueueGet stopped after losing event coverage.",
            EventQueueExit::Unavailable => "The primary simulator does not have a usable EventQueueGet capability.",
            EventQueueExit::RetryLimitReached => "The primary simulator EventQueueGet stopped after repeated errors.",
            EventQueueExit::TaskFailed => "The primary simulator EventQueueGet task ended unexpectedly.",
        };

        tracing::warn!(
            simulator = self.primary.simulator_id.get(),
            ?exit,
            reason,
            "primary simulator event queue stopped; keeping the authenticated session active"
        );
    }
}

pub(super) struct SimulatorCapabilitySession {
    pub(super) endpoint: SocketAddr,
    pub(super) seed_capability: Url,
    pub(super) client: Client,
    pub(super) event_queue_role: EventQueueRoleHandle,
    pub(super) event_queue_task: Option<Id>,
    pub(super) event_queue_running: bool,
}

impl Drop for SimulatorCapabilitySession {
    fn drop(&mut self) {
        self.client.close_session();
    }
}

pub(super) struct RunState {
    pub(super) neighbors: JoinSet<(SimulatorId, Option<u64>, SocketAddr, SimulatorTaskOutcome)>,
    pub(super) neighbor_event_queues: JoinSet<(SimulatorId, SocketAddr, EventQueueExit)>,
    pub(super) neighbor_event_queue_owners: HashMap<Id, (SimulatorId, SocketAddr)>,
    pub(super) neighbor_connections: HashMap<u64, SimulatorConnectionState>,
    pub(super) retiring_simulators: HashSet<SimulatorId>,
    pub(super) neighbor_task_owners: HashMap<Id, (SimulatorId, Option<u64>, SocketAddr)>,
    pub(super) simulator_capabilities: HashMap<SimulatorId, SimulatorCapabilitySession>,
    pub(super) connected_simulators: HashSet<SimulatorId>,
    pub(super) preconnect_events: PreconnectEvents,
    pub(super) pending_events: VecDeque<EventEnvelope>,
    pub(super) pending_lifecycle_events: usize,
    pub(super) pending_region_handshake_replies: VecDeque<(SimulatorId, SimulatorCommand)>,
    pub(super) logout_deadline: Option<Instant>,
}

impl RunState {
    pub(super) fn new() -> Self {
        Self {
            neighbors: JoinSet::new(),
            neighbor_event_queues: JoinSet::new(),
            neighbor_event_queue_owners: HashMap::new(),
            neighbor_connections: HashMap::new(),
            retiring_simulators: HashSet::new(),
            neighbor_task_owners: HashMap::new(),
            simulator_capabilities: HashMap::new(),
            connected_simulators: HashSet::new(),
            preconnect_events: PreconnectEvents::default(),
            pending_events: VecDeque::with_capacity(MAX_PENDING_SESSION_EVENTS),
            pending_lifecycle_events: 0,
            pending_region_handshake_replies: VecDeque::with_capacity(MAX_PENDING_REGION_HANDSHAKE_REPLIES),
            logout_deadline: None,
        }
    }
}

#[cfg(test)]
pub(super) async fn retry_pending_handoff_commands(retry: &mut time::Interval, connections: &mut HashMap<u64, SimulatorConnectionState>) {
    retry.tick().await;
    for connection in connections.values_mut() {
        connection.flush_handoff_command();
    }
}

pub(super) fn queue_connection_message(
    commands: &mpsc::Sender<SimulatorCommand>,
    message: Message,
    delivery: Delivery,
    completed: Option<oneshot::Sender<Result<(), Error>>>,
    cancel_target: Option<HandoffCancellation>,
    report_failure: bool,
) -> Result<(), ConnectionQueueFailure> {
    let command = SimulatorCommand::Send { message, delivery, completed, cancel_target, report_failure };
    match commands.try_send(command) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(SimulatorCommand::Send { completed, .. })) => {
            Err(ConnectionQueueFailure { error: Error::SimulatorCommandQueueFull, completion: completed })
        }
        Err(mpsc::error::TrySendError::Closed(SimulatorCommand::Send { completed, .. })) => {
            Err(ConnectionQueueFailure { error: Error::SimulatorConnectionClosed, completion: completed })
        }
        Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
            unreachable!("only send commands are queued through this helper")
        }
    }
}

pub(super) fn queue_primary_message(
    commands: &mpsc::Sender<SimulatorCommand>,
    agent_id: Uuid,
    session_id: Uuid,
    message: Message,
    delivery: Delivery,
    cancellation: Option<HandoffCancellation>,
) -> Result<(), ConnectionQueueFailure> {
    let cancel_handoff = is_teleport_cancel(&message, agent_id, session_id);
    let cancel_target = cancel_handoff.then_some(cancellation).flatten();
    queue_connection_message(commands, message, delivery, None, cancel_target, true)
}

pub(super) fn track_capability_requests(workers: &mut JoinSet<()>, client: &Client) {
    let client = client.clone();
    workers.spawn(async move { client.wait_for_requests().await });
}

pub(super) fn remove_simulator_capability(capabilities: &mut HashMap<SimulatorId, SimulatorCapabilitySession>, simulator_id: SimulatorId) {
    capabilities.remove(&simulator_id);
}

pub(super) fn is_current_simulator(
    simulator_id: SimulatorId,
    primary_id: SimulatorId,
    connections: &HashMap<u64, SimulatorConnectionState>,
) -> bool {
    simulator_id == primary_id || connections.values().any(|current| current.simulator_id == simulator_id)
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use tokio::sync::mpsc;

    use crate::{event::EventEnvelope, event::SimulatorId, session::error::SessionError};

    use super::SessionState;

    pub(in crate::session) fn publish_primary_terminal_event(
        permit: mpsc::OwnedPermit<EventEnvelope>,
        simulator_id: SimulatorId,
        region_handle: Option<u64>,
        endpoint: SocketAddr,
        result: &Result<(), SessionError>,
    ) {
        SessionState::publish_primary_terminal_event(permit, simulator_id, region_handle, endpoint, result);
    }
}
