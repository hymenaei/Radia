mod capability_events;
mod connection;
mod event_routing;
mod handoff;
mod logout;
mod neighbors;
mod runtime;
mod startup;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::SocketAddr,
    num::NonZeroU64,
    time::Duration,
};

use llsd::Value;
use protocol::{lludp::Message, login::ResponseData};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{AbortHandle, Id, JoinSet},
    time::{self, Instant},
};
use url::Url;
use uuid::Uuid;

use super::{ConnectionRole, MAX_SIMULATOR_CONNECTIONS, admission::SimulatorMemoryBudget};
use crate::{
    AgentSession,
    capabilities::{CapabilityBudget, Client, EventQueueExit, EventQueueRoleHandle},
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource},
    error::Error,
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, SimulatorId},
    session::{Command, SessionOptions},
    transport::{UdpTransport, lludp::Delivery, pacing::OutboundBudget},
};
use capability_events::{decode_agent_communication, decode_enable_simulators};
use connection::{SimulatorConnection, SimulatorConnectionOptions};
use event_routing::{
    EventQueueOutcome, PreconnectEvents, PreconnectOutcome, PrimaryEventBacklog, primary_event_queue_size, queue_neighbor_event,
    queue_simulator_event, queue_until_connected,
};
use handoff::{
    HandoffAnnouncement, HandoffCancellation, HandoffGeneration, HandoffOrder, HandoffState, HandoffTarget, PromotionSetup,
    cancel_pending_handoff, connection_message_has_application_event, decode_crossed_region, decode_event_teleport_failed,
    decode_event_teleport_finish, is_teleport_cancel, teleport_failure_matches,
};
use logout::{LogoutUnavailable, PendingLogout, finish_logout};
use neighbors::{is_current_neighbor, neighbor_completion_event, neighbor_event_queue_failure};
use runtime::CoordinatorRuntime;
use startup::SessionStartup;

const ACK_INTERVAL: Duration = Duration::from_millis(100);
const SESSION_TIMEOUT: Duration = Duration::from_secs(40);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PENDING_SIMULATOR_EVENTS: usize = 16;
const EVENT_BACKLOG_WARNING_THRESHOLD: usize = MAX_PENDING_SIMULATOR_EVENTS / 2;
const EVENT_BACKLOG_WARNING_INTERVAL: Duration = Duration::from_secs(10);
const MAX_EVENTS_PER_PACKET: usize = 2;
const MAX_QUEUED_SIMULATOR_EVENTS: usize = MAX_PENDING_SIMULATOR_EVENTS + MAX_EVENTS_PER_PACKET - 1;
const MAX_TRACKED_CAPABILITY_CLIENTS: usize = MAX_SIMULATOR_CONNECTIONS;
const MAX_PRECONNECT_EVENTS: usize = 64;
const NEIGHBOR_COMMAND_CAPACITY: usize = 8;
const HANDOFF_COMMAND_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const EVENT_BACKLOG_DISCONNECT_REASON: &str = "The simulator connection closed because the viewer could not keep up with incoming events.";
const NEIGHBOR_PRECONNECT_OVERFLOW_REASON: &str = "The neighbor simulator exceeded its bounded pre-connect event buffer.";

type ConnectionTaskResult = (SimulatorId, Option<u64>, SocketAddr, SimulatorTaskOutcome);

struct ConnectionQueueFailure {
    error: Error,
    completion: Option<oneshot::Sender<Result<(), Error>>>,
}

enum SimulatorRequest {
    Enable { simulator_id: SimulatorId, region_handle: u64, endpoint: SocketAddr },
}

#[derive(Clone)]
enum SimulatorTaskOutcome {
    Finished(Result<(), String>),
    ConnectFailed(String),
}

enum SimulatorEvent {
    Simulator(EventEnvelope),
    SendCompleted { simulator_id: SimulatorId, cancel_target: Option<HandoffCancellation>, error: Option<String>, report_failure: bool },
    Finished { simulator_id: SimulatorId, region_handle: Option<u64>, endpoint: SocketAddr, outcome: SimulatorTaskOutcome },
}

struct SimulatorConnectionState {
    simulator_id: SimulatorId,
    endpoint: SocketAddr,
    abort: AbortHandle,
    commands: mpsc::Sender<SimulatorCommand>,
    logout: mpsc::Sender<oneshot::Sender<Result<(), Error>>>,
    pending_handoff_command: Option<HandoffCommand>,
    primary_state: Option<watch::Sender<PrimaryState>>,
    packet_memory: SimulatorMemoryBudget,
}

impl SimulatorConnectionState {
    fn connection_role(&self) -> ConnectionRole {
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

    fn queue_handoff_command(&mut self, command: HandoffCommand) -> bool {
        self.pending_handoff_command = Some(command);
        self.flush_handoff_command();
        !self.commands.is_closed()
    }

    fn flush_handoff_command(&mut self) {
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
                    SimulatorCommand::Send { .. } => unreachable!("only handoff control commands are deferred"),
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

struct PrimaryRoute {
    simulator_id: SimulatorId,
    memory: SimulatorMemoryBudget,
    endpoint: SocketAddr,
    region_handle: Option<u64>,
    commands: mpsc::Sender<SimulatorCommand>,
    state: watch::Sender<PrimaryState>,
    logout: mpsc::Sender<oneshot::Sender<Result<(), Error>>>,
}

impl PrimaryRoute {
    fn from_neighbor(region_handle: u64, connection: &SimulatorConnectionState) -> Option<Self> {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PrimaryState {
    Neighbor { generation: u64 },
    Connecting,
    Ready { generation: u64 },
}

impl PrimaryState {
    pub(super) fn role_generation(self) -> u64 {
        match self {
            Self::Neighbor { generation } | Self::Ready { generation } => generation,
            Self::Connecting => 0,
        }
    }

    pub(super) fn connection_role(self) -> ConnectionRole {
        match self {
            Self::Neighbor { .. } => ConnectionRole::Neighbor,
            Self::Connecting | Self::Ready { .. } => ConnectionRole::Primary,
        }
    }
}

enum SimulatorCommand {
    Send {
        message: Message,
        delivery: Delivery,
        completed: Option<oneshot::Sender<Result<(), Error>>>,
        cancel_target: Option<HandoffCancellation>,
        report_failure: bool,
    },
    PrepareHandoff(HandoffTarget),
    CancelHandoff,
}

enum HandoffCommand {
    Prepare(HandoffTarget),
    Cancel,
}

pub(crate) struct SimulatorCoordinatorOptions {
    pub(crate) coverage_losses: CoverageLossLedger,
    pub(crate) session: SessionOptions,
    pub(crate) startup_disconnect: Option<watch::Receiver<bool>>,
    pub(crate) root_transport: Option<UdpTransport>,
}

pub(crate) struct SimulatorCoordinator {
    simulator_id: SimulatorId,
    primary: PrimaryRoute,
    root_connection_abort: Option<AbortHandle>,
    neighbor_events: mpsc::Sender<SimulatorEvent>,
    neighbor_event_receiver: Option<mpsc::Receiver<SimulatorEvent>>,
    neighbor_requests: mpsc::Sender<SimulatorRequest>,
    neighbor_request_receiver: Option<mpsc::Receiver<SimulatorRequest>>,
    root_connection_workers: Option<JoinSet<ConnectionTaskResult>>,
    neighbor_cancellation: watch::Sender<bool>,
    root_connection_failed: bool,
    logout_request_sent: bool,
    logout_attempted: bool,
    logout_unavailable: Option<LogoutUnavailable>,
    pending_primary_logout: Option<PendingLogout>,
    session: AgentSession,
    capabilities: Client,
    capability_budget: CapabilityBudget,
    capability_request_waiters: JoinSet<()>,
    login_data: ResponseData,
    event_backlog: PrimaryEventBacklog,
    packet_memory: SimulatorMemoryBudget,
    coverage_losses: CoverageLossLedger,
    outbound_budget: Option<OutboundBudget>,
    handoff_generation: u64,
}

impl Drop for SimulatorCoordinator {
    fn drop(&mut self) {
        self.neighbor_cancellation.send_replace(true);
        self.coverage_losses.retire(self.simulator_id, CoverageSource::Lludp);
        self.capabilities.close_session();
    }
}

impl SimulatorCoordinator {
    fn promote_primary(&mut self, next: PrimaryRoute, role_generation: u64) {
        self.primary.memory.promote_to(&next.memory);
        self.primary.state.send_replace(PrimaryState::Neighbor { generation: role_generation });
        next.state.send_replace(PrimaryState::Ready { generation: role_generation });
        self.primary = next;
    }

    fn next_handoff_generation(&mut self) -> Option<HandoffGeneration> {
        self.handoff_generation = self.handoff_generation.checked_add(1)?;
        NonZeroU64::new(self.handoff_generation).map(HandoffGeneration::new)
    }

    #[tracing::instrument(name = "simulator.connect", level = "info", skip_all)]
    pub(crate) async fn connect(
        session: AgentSession,
        login_data: ResponseData,
        options: SimulatorCoordinatorOptions,
    ) -> Result<Option<Self>, String> {
        Self::connect_inner(session, login_data, options).await
    }

    pub(crate) async fn run(
        mut self,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        disconnect_requested: watch::Receiver<bool>,
        terminal_event_permit: mpsc::OwnedPermit<EventEnvelope>,
    ) -> Result<(), String> {
        let simulator_id = self.simulator_id;
        let result = self.run_inner(commands, events, disconnect_requested).await;
        if let Some(outbound_budget) = self.outbound_budget.take() {
            outbound_budget.shutdown().await;
        }
        self.finish_capability_request_waiters().await;
        if !self.root_connection_failed || self.primary.simulator_id != simulator_id {
            Self::publish_primary_terminal_event(
                terminal_event_permit,
                self.primary.simulator_id,
                self.primary.region_handle,
                self.primary.endpoint,
                &result,
            );
        }
        result
    }

    fn new_capability_client(&mut self, seed: Url) -> Result<Client, String> {
        if self.capability_request_waiters.len() >= MAX_TRACKED_CAPABILITY_CLIENTS {
            return Err("The capability client limit has been reached.".into());
        }
        let client = Client::new_with_budget(seed, self.capability_budget.clone())?;
        track_capability_requests(&mut self.capability_request_waiters, &client);
        Ok(client)
    }

    async fn finish_capability_request_waiters(&mut self) {
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
        result: &Result<(), String>,
    ) {
        let event = match result {
            Ok(()) => Event::SimulatorDisconnected { simulator_id, region_handle, endpoint },
            Err(reason) => Event::SimulatorFailed { simulator_id, region_handle, endpoint, reason: reason.clone() },
        };
        permit.send(EventEnvelope::new(event));
    }

    async fn run_inner(
        &mut self,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        disconnect_requested: watch::Receiver<bool>,
    ) -> Result<(), String> {
        CoordinatorRuntime::start(self, commands, events, disconnect_requested).await?.run().await
    }
    async fn publish_event(
        &mut self,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        event: impl Into<EventEnvelope>,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        let event = event.into();
        if logout_deadline.is_some() && is_application_event(&event.event) {
            if let Some(watermark) = event.coverage_watermark() {
                record_coverage_loss(&self.coverage_losses, watermark.simulator_id, watermark.source, CoverageLossReason::EventAdmission);
            }
            return Ok(());
        }
        let watermark = event.coverage_watermark();
        let outcome = queue_simulator_event(events, pending_events, event);
        let should_logout = match outcome {
            EventQueueOutcome::Queued => pending_events.len() == MAX_PENDING_SIMULATOR_EVENTS,
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
                true
            }
        };

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

        if pending_events.len() == MAX_PENDING_SIMULATOR_EVENTS {
            let first_backlog_disconnect = *disconnect_reason != Some(EVENT_BACKLOG_DISCONNECT_REASON);
            *disconnect_reason = Some(EVENT_BACKLOG_DISCONNECT_REASON);
            if first_backlog_disconnect {
                let (queue_occupancy, queue_capacity) = primary_event_queue_size(events, pending_events.len());
                let backlog_duration_ms = self.event_backlog.duration_ms(now);
                tracing::warn!(
                    queue_occupancy,
                    queue_capacity,
                    backlog_duration_ms,
                    "disconnecting primary simulator because its application-event queue reached the limit"
                );
            }
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

    async fn fail_preconnect_overflow(
        &mut self,
        simulator_id: SimulatorId,
        events: &mpsc::Sender<EventEnvelope>,
        state: &mut CoordinatorRunState,
    ) -> Result<(), String> {
        for event in state.preconnect_events.remove(simulator_id) {
            record_event_envelope_loss(&self.coverage_losses, event, CoverageLossReason::EventAdmission);
        }
        state.connected_simulators.remove(&simulator_id);
        let error = format!("A simulator connection exceeded the {MAX_PRECONNECT_EVENTS}-event pre-connect buffer.");
        if simulator_id == self.simulator_id {
            return Err(error);
        }

        let region_handle = state
            .neighbor_connections
            .iter()
            .find_map(|(region_handle, connection)| (connection.simulator_id == simulator_id).then_some(*region_handle));
        let Some(region_handle) = region_handle else {
            return Ok(());
        };
        let Some(connection) = state.neighbor_connections.remove(&region_handle) else {
            return Ok(());
        };
        if state.connected_simulators.contains(&simulator_id) && !connection.abort.is_finished() {
            connection.log_disconnected(region_handle);
        }
        connection.abort.abort();
        remove_simulator_capability(&mut state.simulator_capabilities, simulator_id);
        self.publish_event(
            events,
            &mut state.pending_events,
            Event::SimulatorFailed { simulator_id, region_handle: Some(region_handle), endpoint: connection.endpoint, reason: error },
            &mut state.logout_deadline,
            &mut state.disconnect_reason,
        )
        .await
    }

    async fn publish_simulator_event(
        &mut self,
        simulator_id: SimulatorId,
        event: EventEnvelope,
        events: &mpsc::Sender<EventEnvelope>,
        state: &mut CoordinatorRunState,
    ) -> Result<(), String> {
        match queue_until_connected(&state.connected_simulators, &mut state.preconnect_events, simulator_id, event) {
            PreconnectOutcome::Ready(event) => {
                self.publish_event(events, &mut state.pending_events, event, &mut state.logout_deadline, &mut state.disconnect_reason).await
            }
            PreconnectOutcome::Buffered => Ok(()),
            PreconnectOutcome::Full => self.fail_preconnect_overflow(simulator_id, events, state).await,
        }
    }

    async fn publish_simulator_connected(
        &mut self,
        simulator_id: SimulatorId,
        event: impl Into<EventEnvelope>,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        preconnect_events: &mut PreconnectEvents,
        connected_simulators: &mut HashSet<SimulatorId>,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        self.publish_event(events, pending_events, event, logout_deadline, disconnect_reason).await?;
        connected_simulators.insert(simulator_id);
        for event in preconnect_events.take(simulator_id) {
            self.publish_event(events, pending_events, event, logout_deadline, disconnect_reason).await?;
        }
        Ok(())
    }

    async fn publish_session_ready(
        &mut self,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        self.publish_event(
            events,
            pending_events,
            Event::SessionReady {
                simulator_id: self.simulator_id,
                session: Box::new(self.session.clone()),
                capabilities: self.capabilities.clone(),
                login_data: self.login_data.clone(),
            },
            logout_deadline,
            disconnect_reason,
        )
        .await
    }

    async fn process_capability_event(
        &mut self,
        simulator_id: SimulatorId,
        message: &str,
        body: &Value,
        order: Option<HandoffOrder>,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), String> {
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

    async fn process_enable_simulators(
        &mut self,
        simulator_id: SimulatorId,
        body: &Value,
        setup: &mut PromotionSetup<'_>,
    ) -> Result<(), String> {
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

    async fn process_crossed_region(
        &mut self,
        simulator_id: SimulatorId,
        body: &Value,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), String> {
        match decode_crossed_region(body, self.session.agent_id, self.session.session_id) {
            Ok(handoff) => self.request_handoff(simulator_id, handoff, HandoffAnnouncement::CrossedRegion, setup, handoff_state).await,
            Err(_reason) => Ok(()),
        }
    }

    async fn process_teleport_finish(
        &mut self,
        simulator_id: SimulatorId,
        body: &Value,
        order: Option<HandoffOrder>,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), String> {
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

    fn process_teleport_failed(
        &mut self,
        body: &Value,
        order: Option<HandoffOrder>,
        setup: &mut PromotionSetup<'_>,
        handoff_state: &mut HandoffState,
    ) -> Result<(), String> {
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

    async fn process_agent_communication(&mut self, body: &Value, setup: &mut PromotionSetup<'_>) -> Result<(), String> {
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

    pub(crate) fn note_primary_event_queue_exit(&self, exit: EventQueueExit) {
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

struct SimulatorCapabilitySession {
    endpoint: SocketAddr,
    seed_capability: Url,
    client: Client,
    event_queue_role: EventQueueRoleHandle,
    event_queue_task: Option<Id>,
    event_queue_running: bool,
}

impl Drop for SimulatorCapabilitySession {
    fn drop(&mut self) {
        self.client.close_session();
    }
}

struct CoordinatorRunState {
    neighbors: JoinSet<(SimulatorId, Option<u64>, SocketAddr, SimulatorTaskOutcome)>,
    neighbor_event_queues: JoinSet<(SimulatorId, SocketAddr, EventQueueExit)>,
    neighbor_event_queue_owners: HashMap<Id, (SimulatorId, SocketAddr)>,
    neighbor_connections: HashMap<u64, SimulatorConnectionState>,
    retiring_simulators: HashSet<SimulatorId>,
    neighbor_task_owners: HashMap<Id, (SimulatorId, Option<u64>, SocketAddr)>,
    simulator_capabilities: HashMap<SimulatorId, SimulatorCapabilitySession>,
    connected_simulators: HashSet<SimulatorId>,
    preconnect_events: PreconnectEvents,
    pending_events: VecDeque<EventEnvelope>,
    logout_deadline: Option<Instant>,
    disconnect_reason: Option<&'static str>,
}

impl CoordinatorRunState {
    fn new() -> Self {
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
            pending_events: VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS),
            logout_deadline: None,
            disconnect_reason: None,
        }
    }
}

fn record_coverage_loss(ledger: &CoverageLossLedger, simulator_id: SimulatorId, source: CoverageSource, reason: CoverageLossReason) {
    let _ = ledger.record(simulator_id, source, reason);
}

fn is_application_event(event: &Event) -> bool {
    matches!(event, Event::CapabilityEvent { .. } | Event::CapabilityEventBatch { .. } | Event::SimulatorMessage { .. })
}

fn record_event_envelope_loss(ledger: &CoverageLossLedger, event: EventEnvelope, reason: CoverageLossReason) {
    if let Some(watermark) = event.coverage_watermark() {
        record_coverage_loss(ledger, watermark.simulator_id, watermark.source, reason);
    }
}

async fn retry_pending_handoff_commands(retry: &mut time::Interval, connections: &mut HashMap<u64, SimulatorConnectionState>) {
    retry.tick().await;
    for connection in connections.values_mut() {
        connection.flush_handoff_command();
    }
}

fn queue_connection_message(
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

fn queue_primary_message(
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

fn track_capability_requests(workers: &mut JoinSet<()>, client: &Client) {
    let client = client.clone();
    workers.spawn(async move { client.wait_for_requests().await });
}

fn remove_simulator_capability(capabilities: &mut HashMap<SimulatorId, SimulatorCapabilitySession>, simulator_id: SimulatorId) {
    capabilities.remove(&simulator_id);
}

fn is_current_simulator(simulator_id: SimulatorId, primary_id: SimulatorId, connections: &HashMap<u64, SimulatorConnectionState>) -> bool {
    simulator_id == primary_id || connections.values().any(|current| current.simulator_id == simulator_id)
}

#[cfg(test)]
pub(crate) mod tests;
