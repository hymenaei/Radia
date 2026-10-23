use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
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

#[cfg(test)]
use tokio::task::{self, JoinError};

#[cfg(test)]
use super::admission::{
    MAX_CONCURRENT_PACKET_DECODES, MAX_NEIGHBOR_CONNECTION_EVENT_BYTES, MAX_QUEUED_NEIGHBOR_EVENT_BYTES, MAX_QUEUED_SIMULATOR_EVENT_BYTES,
};
use super::{
    ConnectionRole, MAX_NEIGHBOR_CONNECTION_TASKS, MAX_NEIGHBOR_SIMULATORS, MAX_SIMULATOR_CONNECTIONS, admission::SimulatorMemoryBudget,
};
#[cfg(test)]
use crate::event::send_failure_event;
#[cfg(test)]
use crate::transport::SendGate;
use crate::{
    AgentSession,
    capabilities::{CapabilityBudget, Client, EVENT_QUEUE_NAME, EventQueueExit, EventQueueRoleHandle},
    capture::CaptureRecorder,
    coverage::{CoverageLossLedger, CoverageLossReason, CoverageSource},
    error::Error,
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, SimulatorId},
    session::{Command, SessionOptions},
    status::{NetworkLifecycle, NetworkStatusHandle},
    transport::{
        lludp::Delivery,
        pacing::{OutboundBudget, OutboundBudgetHandle},
    },
};

#[path = "connection.rs"]
mod connection;
use connection::NeighborConnection;
#[path = "capability_events.rs"]
mod capability_events;
use capability_events::{decode_agent_communication, decode_enable_simulators};
#[path = "event_routing.rs"]
mod event_routing;
use event_routing::{
    EventQueueOutcome, PreconnectEvents, PreconnectOutcome, PrimaryEventBacklog, primary_event_queue_size, queue_neighbor_event,
    queue_simulator_event, queue_until_connected,
};
#[path = "handoff.rs"]
mod handoff;
#[path = "runtime.rs"]
mod runtime;
#[cfg(test)]
use handoff::{
    FinishOrder, HandoffDecision, HandoffEvidence, PendingPromotion, arbitrate_handoff, decode_teleport_finish, handoff_cancellation,
    same_handoff_destination,
};
use handoff::{
    HandoffAnnouncement, HandoffCancellation, HandoffGeneration, HandoffOrder, HandoffState, HandoffTarget, PromotionSetup,
    cancel_pending_handoff, connection_message_has_application_event, decode_crossed_region, decode_event_teleport_failed,
    decode_event_teleport_finish, is_teleport_cancel, teleport_failure_matches,
};
use runtime::CoordinatorRuntime;

const ACK_INTERVAL: Duration = Duration::from_millis(100);
const SESSION_TIMEOUT: Duration = Duration::from_secs(40);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PENDING_SIMULATOR_EVENTS: usize = 16;
const EVENT_BACKLOG_WARNING_THRESHOLD: usize = MAX_PENDING_SIMULATOR_EVENTS / 2;
const EVENT_BACKLOG_WARNING_INTERVAL: Duration = Duration::from_secs(10);
const MAX_EVENTS_PER_PACKET: usize = 2;
const MAX_QUEUED_SIMULATOR_EVENTS: usize = MAX_PENDING_SIMULATOR_EVENTS + MAX_EVENTS_PER_PACKET - 1;
const MAX_CAPABILITY_WORKERS: usize = MAX_SIMULATOR_CONNECTIONS;
const MAX_PRECONNECT_EVENTS: usize = 64;
const NEIGHBOR_COMMAND_CAPACITY: usize = 8;
const HANDOFF_COMMAND_RETRY_INTERVAL: Duration = Duration::from_millis(10);
const EVENT_BACKLOG_DISCONNECT_REASON: &str = "The simulator connection closed because the viewer could not keep up with incoming events.";
const NEIGHBOR_PRECONNECT_OVERFLOW_REASON: &str = "The neighbor simulator exceeded its bounded pre-connect event buffer.";

struct ConnectionQueueFailure {
    error: Error,
    completion: Option<oneshot::Sender<Result<(), Error>>>,
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

enum NeighborRequest {
    Enable { simulator_id: SimulatorId, region_handle: u64, endpoint: SocketAddr },
}

#[derive(Clone)]
enum NeighborTaskOutcome {
    Finished(Result<(), String>),
    ConnectFailed(String),
}

type ConnectionTaskResult = (SimulatorId, Option<u64>, SocketAddr, NeighborTaskOutcome);

fn neighbor_completion_event(
    simulator_id: SimulatorId,
    region_handle: Option<u64>,
    endpoint: SocketAddr,
    outcome: &NeighborTaskOutcome,
) -> Event {
    match outcome {
        NeighborTaskOutcome::Finished(Ok(())) => Event::SimulatorDisconnected { simulator_id, region_handle, endpoint },
        NeighborTaskOutcome::Finished(Err(reason)) => {
            Event::SimulatorFailed { simulator_id, region_handle, endpoint, reason: reason.clone() }
        }
        NeighborTaskOutcome::ConnectFailed(reason) => match region_handle {
            Some(region_handle) => Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
            None => Event::SimulatorFailed { simulator_id, region_handle: None, endpoint, reason: reason.clone() },
        },
    }
}

enum NeighborEvent {
    Simulator(EventEnvelope),
    SendCompleted { simulator_id: SimulatorId, cancel_target: Option<HandoffCancellation>, error: Option<String>, report_failure: bool },
    Finished { simulator_id: SimulatorId, region_handle: Option<u64>, endpoint: SocketAddr, outcome: NeighborTaskOutcome },
}

struct NeighborConnectionState {
    simulator_id: SimulatorId,
    endpoint: SocketAddr,
    abort: AbortHandle,
    commands: mpsc::Sender<NeighborCommand>,
    logout: mpsc::Sender<oneshot::Sender<Result<(), Error>>>,
    pending_handoff_command: Option<NeighborHandoffCommand>,
    primary_state: Option<watch::Sender<PrimaryState>>,
    packet_memory: SimulatorMemoryBudget,
}

struct PrimaryRoute {
    simulator_id: SimulatorId,
    memory: SimulatorMemoryBudget,
    endpoint: SocketAddr,
    region_handle: Option<u64>,
    commands: mpsc::Sender<NeighborCommand>,
    state: watch::Sender<PrimaryState>,
    logout: mpsc::Sender<oneshot::Sender<Result<(), Error>>>,
}

impl PrimaryRoute {
    fn from_neighbor(region_handle: u64, connection: &NeighborConnectionState) -> Option<Self> {
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
pub(super) struct PrimaryState {
    active: bool,
    session_ready: bool,
    role_generation: u64,
}

impl PrimaryState {
    const NEIGHBOR: Self = Self { active: false, session_ready: false, role_generation: 0 };
    const PRIMARY_CONNECTING: Self = Self { active: true, session_ready: false, role_generation: 0 };
    const PRIMARY_READY: Self = Self { active: true, session_ready: true, role_generation: 0 };

    fn promoted(role_generation: u64) -> Self {
        Self { active: true, session_ready: true, role_generation }
    }

    fn demoted(role_generation: u64) -> Self {
        Self { active: false, session_ready: false, role_generation }
    }

    pub(super) fn connection_role(self) -> ConnectionRole {
        if self.active { ConnectionRole::Primary } else { ConnectionRole::Neighbor }
    }
}

enum NeighborCommand {
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

enum NeighborHandoffCommand {
    Prepare(HandoffTarget),
    Cancel,
}

impl NeighborConnectionState {
    fn queue_handoff_command(&mut self, command: NeighborHandoffCommand) -> bool {
        self.pending_handoff_command = Some(command);
        self.flush_handoff_command();
        !self.commands.is_closed()
    }

    fn flush_handoff_command(&mut self) {
        let Some(command) = self.pending_handoff_command.take() else { return };
        let command = match command {
            NeighborHandoffCommand::Prepare(target) => NeighborCommand::PrepareHandoff(target),
            NeighborHandoffCommand::Cancel => NeighborCommand::CancelHandoff,
        };
        match self.commands.try_send(command) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(command)) => {
                self.pending_handoff_command = Some(match command {
                    NeighborCommand::PrepareHandoff(target) => NeighborHandoffCommand::Prepare(target),
                    NeighborCommand::CancelHandoff => NeighborHandoffCommand::Cancel,
                    NeighborCommand::Send { .. } => unreachable!("only handoff control commands are deferred"),
                });
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

async fn retry_pending_handoff_commands(retry: &mut time::Interval, connections: &mut HashMap<u64, NeighborConnectionState>) {
    retry.tick().await;
    for connection in connections.values_mut() {
        connection.flush_handoff_command();
    }
}

pub(crate) struct SimulatorCoordinator {
    simulator_id: SimulatorId,
    primary: PrimaryRoute,
    root_connection_abort: Option<AbortHandle>,
    neighbor_events: mpsc::Sender<NeighborEvent>,
    neighbor_event_receiver: Option<mpsc::Receiver<NeighborEvent>>,
    neighbor_requests: mpsc::Sender<NeighborRequest>,
    neighbor_request_receiver: Option<mpsc::Receiver<NeighborRequest>>,
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
    capability_workers: JoinSet<()>,
    login_data: ResponseData,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
    event_backlog: PrimaryEventBacklog,
    packet_memory: SimulatorMemoryBudget,
    coverage_losses: CoverageLossLedger,
    outbound_budget: Option<OutboundBudget>,
    handoff_generation: u64,
    #[cfg(test)]
    promotion_gate: Option<CoordinatorGate>,
    #[cfg(test)]
    replacement_gate: Option<ReplacementGate>,
}

#[cfg(test)]
struct CoordinatorGate {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
}

#[cfg(test)]
struct ReplacementGate {
    entered: oneshot::Sender<()>,
    release: oneshot::Receiver<()>,
    armed: oneshot::Receiver<()>,
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
        self.primary.state.send_replace(PrimaryState::demoted(role_generation));
        next.state.send_replace(PrimaryState::promoted(role_generation));
        self.primary = next;
    }

    fn next_handoff_generation(&mut self) -> Option<HandoffGeneration> {
        self.handoff_generation = self.handoff_generation.checked_add(1)?;
        NonZeroU64::new(self.handoff_generation).map(HandoffGeneration::new)
    }

    #[cfg(test)]
    #[tracing::instrument(name = "simulator.connect", level = "info", skip_all)]
    pub(crate) async fn connect_with_status(
        session: AgentSession,
        login_data: ResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
    ) -> Result<Self, String> {
        Self::connect_with_coverage(session, login_data, capture, status, CoverageLossLedger::new()).await
    }

    #[cfg(test)]
    pub(crate) async fn connect_with_coverage(
        session: AgentSession,
        login_data: ResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
    ) -> Result<Self, String> {
        Self::connect_with_options(session, login_data, capture, status, coverage_losses, SessionOptions::default(), None)
            .await?
            .ok_or_else(|| "The primary simulator startup was canceled unexpectedly.".to_string())
    }

    pub(crate) async fn connect_with_options(
        session: AgentSession,
        login_data: ResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
        options: SessionOptions,
        startup_disconnect: Option<watch::Receiver<bool>>,
    ) -> Result<Option<Self>, String> {
        Self::connect_with_options_and_gate(
            session,
            login_data,
            capture,
            status,
            coverage_losses,
            options,
            startup_disconnect,
            #[cfg(test)]
            None,
        )
        .await
    }

    #[cfg(test)]
    pub(crate) async fn connect_with_status_and_send_gate(
        session: AgentSession,
        login_data: ResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        send_gate: SendGate,
    ) -> Result<Self, String> {
        Self::connect_with_options_and_gate(
            session,
            login_data,
            capture,
            status,
            CoverageLossLedger::new(),
            SessionOptions::default(),
            None,
            Some(send_gate),
        )
        .await
        .map(|connection| connection.expect("startup cancellation is disabled"))
    }

    #[cfg(test)]
    async fn connect_with_gate_and_disconnect(
        session: AgentSession,
        login_data: ResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        send_gate: SendGate,
        disconnect_requested: watch::Receiver<bool>,
    ) -> Result<Option<Self>, String> {
        Self::connect_with_options_and_gate(
            session,
            login_data,
            capture,
            status,
            CoverageLossLedger::new(),
            SessionOptions::default(),
            Some(disconnect_requested),
            Some(send_gate),
        )
        .await
    }

    async fn connect_with_options_and_gate(
        session: AgentSession,
        login_data: ResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
        options: SessionOptions,
        startup_disconnect: Option<watch::Receiver<bool>>,
        #[cfg(test)] root_send_gate: Option<SendGate>,
    ) -> Result<Option<Self>, String> {
        status.set_simulator_endpoint(session.simulator_address);
        status.set_lifecycle(NetworkLifecycle::Connecting);
        let result = Self::connect_inner(
            session,
            login_data,
            capture,
            status.clone(),
            coverage_losses,
            options,
            startup_disconnect,
            #[cfg(test)]
            root_send_gate,
        )
        .await;
        if let Err(error) = &result {
            status.clear_simulator();
            status.record_error(error);
            status.set_lifecycle(NetworkLifecycle::Failed);
        }
        if matches!(&result, Ok(None)) {
            status.clear_simulator();
            status.set_lifecycle(NetworkLifecycle::Disconnected);
        }
        result
    }

    #[tracing::instrument(name = "simulator.bootstrap", level = "debug", skip_all)]
    async fn connect_inner(
        session: AgentSession,
        login_data: ResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
        coverage_losses: CoverageLossLedger,
        options: SessionOptions,
        mut startup_disconnect: Option<watch::Receiver<bool>>,
        #[cfg(test)] root_send_gate: Option<SendGate>,
    ) -> Result<Option<Self>, String> {
        if startup_disconnect.as_ref().is_some_and(|disconnect| *disconnect.borrow()) {
            return Ok(None);
        }
        let simulator_id = SimulatorId::allocate().ok_or_else(|| "Simulator connection ID space is exhausted.".to_string())?;
        if let Some(capture) = &capture {
            capture.circuit_lifecycle("circuit_starting", session.circuit_code);
        }
        let capability_budget = CapabilityBudget::new();
        let capabilities = Client::new_with_status_and_budget(
            session.seed_capability.clone(),
            capture.clone(),
            status.clone(),
            capability_budget.clone(),
        )?;
        let outbound_budget = OutboundBudget::new(options.upstream_bytes_per_second().get());
        let mut capability_workers = JoinSet::new();
        track_capability_worker(&mut capability_workers, &capabilities);
        let (neighbor_events, neighbor_event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (neighbor_requests, neighbor_request_receiver) = mpsc::channel(32);
        let (root_commands, root_command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
        let (root_logout, root_logout_receiver) = mpsc::channel(1);
        let (primary_state_sender, primary_state_receiver) = watch::channel(PrimaryState::PRIMARY_CONNECTING);
        let (neighbor_cancellation, _) = watch::channel(false);
        let packet_memory = SimulatorMemoryBudget::new();
        let (ready_sender, mut ready_receiver) = oneshot::channel::<Result<(), String>>();
        let mut root_connection_workers = JoinSet::<ConnectionTaskResult>::new();
        let root_id = simulator_id;
        let root_endpoint = session.simulator_address;
        let circuit_code = session.circuit_code;
        let root_events = neighbor_events.clone();
        let root_neighbor_requests = neighbor_requests.clone();
        let root_shutdown = neighbor_cancellation.subscribe();
        let root_memory = packet_memory.clone();
        let root_session = session.clone();
        let root_capture = capture.clone();
        let root_status = status.clone();
        let root_coverage_losses = coverage_losses.clone();
        let root_budget: OutboundBudgetHandle = outbound_budget.handle();
        let root_connection_abort = root_connection_workers.spawn(async move {
            let outcome = match NeighborConnection::connect_primary_with_state(
                root_id,
                root_session,
                root_capture.clone(),
                root_status,
                root_coverage_losses,
                primary_state_receiver,
                root_budget,
                #[cfg(test)]
                root_send_gate,
            )
            .await
            {
                Ok(connection) => {
                    if let Some(capture) = &root_capture {
                        capture.circuit_lifecycle("use_circuit_code_sent", circuit_code);
                    }
                    let _ = ready_sender.send(Ok(()));
                    NeighborTaskOutcome::Finished(
                        connection
                            .run(
                                root_events.clone(),
                                root_shutdown,
                                root_neighbor_requests,
                                root_command_receiver,
                                root_logout_receiver,
                                root_memory,
                            )
                            .await,
                    )
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(error.clone()));
                    NeighborTaskOutcome::ConnectFailed(error)
                }
            };
            let _ = root_events
                .send(NeighborEvent::Finished {
                    simulator_id: root_id,
                    region_handle: None,
                    endpoint: root_endpoint,
                    outcome: outcome.clone(),
                })
                .await;
            (root_id, None, root_endpoint, outcome)
        });
        let startup_result = match startup_disconnect.as_mut() {
            Some(disconnect) => tokio::select! {
                biased;
                result = &mut ready_receiver => Some(result),
                _ = disconnect.changed() => None,
            },
            None => Some(ready_receiver.await),
        };
        let startup_result = match startup_result {
            Some(Ok(result)) => result,
            Some(Err(_)) => Err("The primary simulator connection task stopped during startup.".to_string()),
            None => {
                root_connection_workers.shutdown().await;
                capabilities.close_session_and_wait().await;
                while capability_workers.join_next().await.is_some() {}
                outbound_budget.shutdown().await;
                return Ok(None);
            }
        };
        if let Err(error) = startup_result {
            root_connection_workers.shutdown().await;
            capabilities.close_session_and_wait().await;
            while capability_workers.join_next().await.is_some() {}
            outbound_budget.shutdown().await;
            return Err(error);
        }
        Ok(Some(Self {
            simulator_id,
            primary: PrimaryRoute {
                simulator_id,
                memory: packet_memory.clone(),
                endpoint: session.simulator_address,
                region_handle: None,
                commands: root_commands,
                state: primary_state_sender,
                logout: root_logout,
            },
            root_connection_abort: Some(root_connection_abort),
            neighbor_events,
            neighbor_event_receiver: Some(neighbor_event_receiver),
            neighbor_requests,
            neighbor_request_receiver: Some(neighbor_request_receiver),
            root_connection_workers: Some(root_connection_workers),
            neighbor_cancellation,
            root_connection_failed: false,
            logout_request_sent: false,
            logout_attempted: false,
            logout_unavailable: None,
            pending_primary_logout: None,
            session,
            capabilities,
            capability_budget,
            capability_workers,
            login_data,
            capture,
            status,
            event_backlog: PrimaryEventBacklog::default(),
            packet_memory,
            coverage_losses,
            outbound_budget: Some(outbound_budget),
            handoff_generation: 0,
            #[cfg(test)]
            promotion_gate: None,
            #[cfg(test)]
            replacement_gate: None,
        }))
    }

    fn register_root_connection(&mut self, region_handle: u64, connections: &mut HashMap<u64, NeighborConnectionState>) {
        if let Some(current_region) = connections
            .iter()
            .find_map(|(current_region, connection)| (connection.simulator_id == self.simulator_id).then_some(*current_region))
        {
            if current_region != region_handle
                && let Some(connection) = connections.remove(&current_region)
            {
                connections.insert(region_handle, connection);
            }
        } else {
            let Some(abort) = self.root_connection_abort.take() else {
                return;
            };
            connections.insert(
                region_handle,
                NeighborConnectionState {
                    simulator_id: self.simulator_id,
                    endpoint: self.session.simulator_address,
                    abort,
                    commands: self.primary.commands.clone(),
                    logout: self.primary.logout.clone(),
                    pending_handoff_command: None,
                    primary_state: Some(self.primary.state.clone()),
                    packet_memory: self.packet_memory.clone(),
                },
            );
        }
        if self.primary.simulator_id == self.simulator_id {
            self.primary.region_handle = Some(region_handle);
        }
    }

    pub(crate) async fn run(
        mut self,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        disconnect_requested: watch::Receiver<bool>,
        terminal_event_permit: mpsc::OwnedPermit<EventEnvelope>,
    ) -> Result<(), String> {
        let status = self.status.clone();
        let simulator_id = self.simulator_id;
        let result = self.run_inner(commands, events, disconnect_requested).await;
        if let Some(outbound_budget) = self.outbound_budget.take() {
            outbound_budget.shutdown().await;
        }
        self.finish_capability_workers().await;
        status.clear_simulator();
        match &result {
            Ok(()) => status.set_lifecycle(NetworkLifecycle::Disconnected),
            Err(error) => {
                status.record_error(error);
                status.set_lifecycle(NetworkLifecycle::Failed);
            }
        }
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
        if self.capability_workers.len() >= MAX_CAPABILITY_WORKERS {
            return Err("The capability worker limit has been reached.".into());
        }
        let client = Client::new_with_status_and_budget(seed, self.capture.clone(), self.status.clone(), self.capability_budget.clone())?;
        track_capability_worker(&mut self.capability_workers, &client);
        Ok(client)
    }

    async fn finish_capability_workers(&mut self) {
        self.capabilities.close_session();
        while self.capability_workers.join_next().await.is_some() {}
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

    #[tracing::instrument(name = "simulator.session", level = "info", skip_all)]
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
                "Primary simulator application-event queue is under pressure"
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
                    "Disconnecting primary simulator because its application-event queue reached the limit"
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
        self.status.record_error(&error);
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

    async fn start_neighbor(
        &mut self,
        origin: SimulatorId,
        region_handle: u64,
        endpoint: SocketAddr,
        handoff: Option<HandoffTarget>,
        setup: &mut PromotionSetup<'_>,
    ) -> Result<bool, String> {
        let PromotionSetup { state, neighbor_requests, application_events, neighbor_events, .. } = setup;
        let CoordinatorRunState {
            neighbors,
            neighbor_connections: connections,
            retiring_simulators,
            neighbor_task_owners,
            preconnect_events,
            simulator_capabilities,
            pending_events,
            logout_deadline,
            disconnect_reason,
            ..
        } = &mut **state;
        let neighbor_requests = (*neighbor_requests).clone();
        let application_events = (*application_events).clone();
        let neighbor_events = (*neighbor_events).clone();
        let shutdown = self.neighbor_cancellation.subscribe();
        if origin != self.primary.simulator_id || !is_current_simulator(origin, self.simulator_id, connections) {
            return Ok(false);
        }
        if handoff.as_ref().is_some_and(|target| target.region_handle != region_handle || target.endpoint != endpoint) {
            self.status.record_error("Ignored a handoff target that did not match its simulator announcement.");
            return Ok(false);
        }
        let simulator_id = SimulatorId::allocate().ok_or_else(|| "Simulator connection ID space is exhausted.".to_string())?;
        if endpoint.port() == 0
            || endpoint.ip().is_unspecified()
            || endpoint.ip().is_multicast()
            || endpoint.ip() == IpAddr::V4(Ipv4Addr::BROADCAST)
        {
            self.status.record_error("The simulator announced an invalid neighbor endpoint.");
            self.publish_event(
                &application_events,
                pending_events,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(false);
        }
        if let Some(current) = connections.get(&region_handle)
            && current.endpoint == endpoint
        {
            if let Some(handoff) = &handoff {
                if connections
                    .get_mut(&region_handle)
                    .is_some_and(|connection| connection.queue_handoff_command(NeighborHandoffCommand::Prepare(handoff.clone())))
                {
                    return Ok(true);
                }
            } else {
                return Ok(true);
            }
        }
        if connections.get(&region_handle).is_some_and(|current| current.simulator_id == self.simulator_id) {
            self.status.record_error("The original simulator connection cannot be replaced before its handoff completes.");
            self.publish_event(
                &application_events,
                pending_events,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(false);
        }
        if (endpoint == self.session.simulator_address && handoff.is_none())
            || connections.iter().any(|(active_region, active)| {
                *active_region != region_handle
                    && active.endpoint == endpoint
                    && (active.simulator_id != self.simulator_id || handoff.is_none())
            })
        {
            self.status.record_error("The simulator announced an endpoint that is already in use.");
            self.publish_event(
                &application_events,
                pending_events,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(false);
        }
        let root_task_count =
            if neighbor_task_owners.values().any(|(task_simulator_id, _, _)| *task_simulator_id == self.simulator_id) { 1 } else { 0 };
        if neighbors.len().saturating_sub(root_task_count) >= MAX_NEIGHBOR_CONNECTION_TASKS {
            self.status.record_error("The neighbor connection task limit has been reached.");
            self.publish_event(
                &application_events,
                pending_events,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(false);
        }
        if let Some(current) = connections.remove(&region_handle) {
            retiring_simulators.insert(current.simulator_id);
            for event in preconnect_events.remove(current.simulator_id) {
                record_event_envelope_loss(&self.coverage_losses, event, CoverageLossReason::EventAdmission);
            }
            remove_simulator_capability(simulator_capabilities, current.simulator_id);
            current.abort.abort();
        }
        let neighbor_count = connections.values().filter(|connection| connection.simulator_id != self.simulator_id).count();
        if neighbor_count >= MAX_NEIGHBOR_SIMULATORS {
            self.status.record_error("The simulator announced more neighbor regions than the client can track.");
            self.publish_event(
                &application_events,
                pending_events,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(false);
        }

        let mut session = self.session.clone();
        session.simulator_address = endpoint;
        if let Some(handoff) = &handoff {
            session.seed_capability = handoff.seed_capability.clone();
        }
        let capture = self.capture.clone();
        let status = self.status.clone();
        let coverage_losses = self.coverage_losses.clone();
        let packet_memory = self.packet_memory.for_neighbor();
        let connection_packet_memory = packet_memory.clone();
        let outbound_budget = self.outbound_budget.as_ref().expect("session pacing budget exists").handle();
        let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::NEIGHBOR);
        let (command_sender, command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
        let (logout_sender, logout_receiver) = mpsc::channel(1);
        if let Some(handoff) = handoff {
            command_sender.try_send(NeighborCommand::PrepareHandoff(handoff)).expect("new neighbor command queue has capacity");
        }
        let finish_events = neighbor_events.clone();
        let task = neighbors.spawn(async move {
            let outcome = match NeighborConnection::connect_with_primary_state(
                simulator_id,
                session,
                region_handle,
                capture,
                status,
                coverage_losses,
                primary_state_receiver,
                outbound_budget,
            )
            .await
            {
                Ok(connection) => NeighborTaskOutcome::Finished(
                    connection.run(neighbor_events, shutdown, neighbor_requests, command_receiver, logout_receiver, packet_memory).await,
                ),
                Err(error) => NeighborTaskOutcome::ConnectFailed(error),
            };
            let _ = finish_events
                .send(NeighborEvent::Finished { simulator_id, region_handle: Some(region_handle), endpoint, outcome: outcome.clone() })
                .await;
            (simulator_id, Some(region_handle), endpoint, outcome)
        });
        neighbor_task_owners.insert(task.id(), (simulator_id, Some(region_handle), endpoint));
        connections.insert(
            region_handle,
            NeighborConnectionState {
                simulator_id,
                endpoint,
                abort: task,
                commands: command_sender,
                logout: logout_sender,
                pending_handoff_command: None,
                primary_state: Some(primary_state),
                packet_memory: connection_packet_memory,
            },
        );
        #[cfg(test)]
        {
            let pause_replacement = self.replacement_gate.as_mut().is_some_and(|gate| matches!(gate.armed.try_recv(), Ok(())));
            if pause_replacement {
                let gate = self.replacement_gate.take().expect("armed replacement gate remains installed");
                let _ = gate.entered.send(());
                let _ = gate.release.await;
            }
        }
        Ok(true)
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
        if message == "EnableSimulator" {
            match decode_enable_simulators(body) {
                Ok(simulators) => {
                    for (region_handle, endpoint) in simulators {
                        if setup.state.logout_deadline.is_some() {
                            break;
                        }
                        self.start_neighbor(simulator_id, region_handle, endpoint, None, setup).await?;
                    }
                }
                Err(reason) => self.status.record_error(format!("Ignored malformed EnableSimulator event: {reason}")),
            }
        } else if message == "CrossedRegion" {
            match decode_crossed_region(body, self.session.agent_id, self.session.session_id) {
                Ok(handoff) => {
                    self.request_handoff(simulator_id, handoff, HandoffAnnouncement::CrossedRegion, setup, handoff_state).await?;
                }
                Err(reason) => self.status.record_error(format!("Ignored malformed CrossedRegion event: {reason}")),
            }
        } else if message == "TeleportFinish" {
            match decode_event_teleport_finish(body, self.session.agent_id, self.session.session_id) {
                Ok(handoff) => {
                    self.request_handoff(
                        simulator_id,
                        handoff,
                        HandoffAnnouncement::TeleportFinish(order.unwrap_or(HandoffOrder::Unavailable)),
                        setup,
                        handoff_state,
                    )
                    .await?;
                }
                Err(reason) => self.status.record_error(format!("Ignored malformed TeleportFinish event: {reason}")),
            }
        } else if message == "TeleportFailed" {
            match decode_event_teleport_failed(body, self.session.agent_id) {
                Ok(()) if teleport_failure_matches(handoff_state.pending(), order.unwrap_or(HandoffOrder::Unavailable)) => {
                    if let Some(pending) = handoff_state.take_pending() {
                        cancel_pending_handoff(
                            pending,
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
                Err(reason) => self.status.record_error(format!("Ignored malformed TeleportFailed event: {reason}")),
            }
        } else if message == "EstablishAgentCommunication" {
            match decode_agent_communication(body, self.session.agent_id) {
                Ok((endpoint, _)) if endpoint == self.primary.endpoint => {
                    self.status.record_error("Ignored an agent communication event for the primary simulator.");
                }
                Ok((endpoint, _)) if !setup.state.neighbor_connections.values().any(|active| active.endpoint == endpoint) => {
                    self.status.record_error("Ignored a seed capability for an untracked simulator.");
                }
                Ok((endpoint, seed)) => {
                    let Some(target_id) = setup
                        .state
                        .neighbor_connections
                        .values()
                        .find(|active| active.endpoint == endpoint)
                        .map(|active| active.simulator_id)
                    else {
                        return Ok(());
                    };
                    if setup.state.simulator_capabilities.contains_key(&target_id) {
                        return Ok(());
                    }
                    match self.new_capability_client(seed.clone()) {
                        Ok(client) => match client.start_event_queue(
                            target_id,
                            endpoint,
                            setup.capability_events.clone(),
                            setup.shutdown.clone(),
                            self.coverage_losses.clone(),
                        ) {
                            Ok(event_queue) => {
                                let event_queue_role = event_queue.role_handle();
                                let task = setup.state.neighbor_event_queues.spawn(async move {
                                    let exit = event_queue.wait().await;
                                    (target_id, endpoint, exit)
                                });
                                let task_id = task.id();
                                setup.state.neighbor_event_queue_owners.insert(task_id, (target_id, endpoint));
                                setup.state.simulator_capabilities.insert(
                                    target_id,
                                    SimulatorCapabilitySession {
                                        endpoint,
                                        seed_capability: seed,
                                        client: client.clone(),
                                        event_queue_role,
                                        event_queue_task: Some(task_id),
                                        event_queue_running: true,
                                    },
                                );
                                let application_events = setup.application_events.clone();
                                self.publish_simulator_event(
                                    target_id,
                                    Event::SimulatorCapabilities { simulator_id: target_id, source: endpoint, capabilities: client }.into(),
                                    &application_events,
                                    setup.state,
                                )
                                .await?;
                            }
                            Err(error) => {
                                client.close_session();
                                self.status.record_error(error);
                            }
                        },
                        Err(error) => self.status.record_error(error),
                    }
                }
                Err(reason) => self.status.record_error(format!("Ignored malformed agent communication event: {reason}")),
            }
        }
        Ok(())
    }

    async fn disconnect_once(&mut self, logout_already_sent: bool) -> Result<LogoutOutcome, String> {
        tracing::info!(logout_already_sent, "Simulator disconnect started");
        self.status.set_lifecycle(NetworkLifecycle::Disconnecting);
        if let Some(capture) = &self.capture {
            capture.circuit_lifecycle("disconnect_started", self.session.circuit_code);
        }
        self.finish_pending_logout().await?;
        let outcome = if logout_already_sent || self.logout_request_sent {
            LogoutOutcome::AlreadySent
        } else if let Some(reason) = self.logout_unavailable {
            LogoutOutcome::Unavailable(reason)
        } else if self.logout_attempted {
            LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionEnded)
        } else if self.root_connection_failed && self.primary.simulator_id == self.simulator_id {
            self.logout_attempted = true;
            self.record_logout_unavailable(LogoutUnavailable::ConnectionClosed);
            LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionClosed)
        } else {
            self.logout_attempted = true;
            let (completed, response) = oneshot::channel();
            let result = send_logout_with_deadline(
                async {
                    self.primary
                        .logout
                        .send(completed)
                        .await
                        .map_err(|_| LogoutAttemptError::Unavailable(LogoutUnavailable::ConnectionClosed))?;
                    response
                        .await
                        .map_err(|_| LogoutAttemptError::Unavailable(LogoutUnavailable::ConnectionEnded))?
                        .map_err(|error| LogoutAttemptError::Unexpected(error.to_string()))
                },
                LogoutUnavailable::Deadline,
            )
            .await;
            self.apply_logout_attempt_result(result)?;
            if self.logout_request_sent {
                LogoutOutcome::Sent
            } else {
                LogoutOutcome::Unavailable(self.logout_unavailable.unwrap_or(LogoutUnavailable::ConnectionEnded))
            }
        };
        match outcome {
            LogoutOutcome::Sent => {
                tracing::debug!(local_send_completed = true, remote_receipt_confirmed = false, "Simulator logout request sent");
            }
            LogoutOutcome::AlreadySent => {
                tracing::debug!(local_send_completed = true, remote_receipt_confirmed = false, "Simulator logout request was already sent");
            }
            LogoutOutcome::Unavailable(reason) => {
                self.record_logout_unavailable(reason);
            }
        }
        Ok(outcome)
    }

    fn apply_logout_attempt_result(&mut self, result: Result<(), LogoutAttemptError>) -> Result<(), String> {
        match result {
            Ok(()) => {
                self.logout_request_sent = true;
                self.logout_unavailable = None;
                Ok(())
            }
            Err(LogoutAttemptError::Unavailable(reason)) => {
                self.record_logout_unavailable(reason);
                Ok(())
            }
            Err(LogoutAttemptError::Unexpected(error)) => Err(error),
        }
    }

    async fn finish_pending_logout(&mut self) -> Result<(), String> {
        if let Some(mut pending) = self.pending_primary_logout.take() {
            let result = pending.wait().await;
            self.apply_logout_attempt_result(result)?;
        }
        Ok(())
    }

    fn record_logout_unavailable(&mut self, reason: LogoutUnavailable) {
        if self.logout_unavailable == Some(reason) {
            return;
        }
        self.logout_unavailable = Some(reason);
        let message = format!("Simulator logout request was not sent before disconnect completed: {}", reason.description());
        tracing::warn!(?reason, "Simulator disconnect completed without sending logout");
        self.status.record_error(message);
    }

    async fn graceful_disconnect(&mut self, logout_already_sent: bool) -> Result<(), String> {
        let result = self.disconnect_once(logout_already_sent).await;
        self.neighbor_cancellation.send_replace(true);
        result.map(|_| ())
    }

    pub(crate) fn note_primary_event_queue_exit(&self, exit: EventQueueExit) {
        let reason = match exit {
            EventQueueExit::Stopped => return,
            EventQueueExit::StoppedWithCoverageLoss => "The primary simulator EventQueueGet stopped after losing event coverage.",
            EventQueueExit::Unavailable => "The primary simulator does not have a usable EventQueueGet capability.",
            EventQueueExit::RetryLimitReached => "The primary simulator EventQueueGet stopped after repeated errors.",
            EventQueueExit::TaskFailed => "The primary simulator EventQueueGet task ended unexpectedly.",
        };

        self.status.record_error(reason);
        tracing::warn!(
            simulator_id = self.primary.simulator_id.get(),
            ?exit,
            reason,
            "Primary simulator EventQueueGet stopped; keeping the authenticated session active"
        );
    }
}

fn queue_connection_message(
    simulator_id: SimulatorId,
    commands: &mpsc::Sender<NeighborCommand>,
    message: Message,
    delivery: Delivery,
    completed: Option<oneshot::Sender<Result<(), Error>>>,
    cancel_target: Option<HandoffCancellation>,
    report_failure: bool,
) -> Result<(), ConnectionQueueFailure> {
    let message_name = message.name();
    let command = NeighborCommand::Send { message, delivery, completed, cancel_target, report_failure };
    match commands.try_send(command) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(NeighborCommand::Send { completed, .. })) => {
            if report_failure {
                tracing::trace!(
                    simulator_id = simulator_id.get(),
                    message = message_name,
                    delivery = ?delivery,
                    queue_stage = "connection_commands",
                    queued_commands = commands.max_capacity() - commands.capacity(),
                    queue_capacity = commands.max_capacity(),
                    "Simulator send rejected by a full connection command queue"
                );
            }
            Err(ConnectionQueueFailure { error: Error::SimulatorCommandQueueFull, completion: completed })
        }
        Err(mpsc::error::TrySendError::Closed(NeighborCommand::Send { completed, .. })) => {
            Err(ConnectionQueueFailure { error: Error::SimulatorConnectionClosed, completion: completed })
        }
        Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
            unreachable!("only send commands are queued through this helper")
        }
    }
}

fn queue_primary_message(
    simulator_id: SimulatorId,
    commands: &mpsc::Sender<NeighborCommand>,
    agent_id: Uuid,
    session_id: Uuid,
    message: Message,
    delivery: Delivery,
    cancellation: Option<HandoffCancellation>,
) -> Result<(), ConnectionQueueFailure> {
    let cancel_handoff = is_teleport_cancel(&message, agent_id, session_id);
    let cancel_target = cancel_handoff.then_some(cancellation).flatten();
    queue_connection_message(simulator_id, commands, message, delivery, None, cancel_target, true)
}

fn track_capability_worker(workers: &mut JoinSet<()>, client: &Client) {
    let client = client.clone();
    workers.spawn(async move { client.wait_for_worker().await });
}

struct SimulatorCapabilitySession {
    endpoint: SocketAddr,
    seed_capability: Url,
    client: Client,
    event_queue_role: EventQueueRoleHandle,
    event_queue_task: Option<Id>,
    event_queue_running: bool,
}

struct CoordinatorRunState {
    neighbors: JoinSet<(SimulatorId, Option<u64>, SocketAddr, NeighborTaskOutcome)>,
    neighbor_event_queues: JoinSet<(SimulatorId, SocketAddr, EventQueueExit)>,
    neighbor_event_queue_owners: HashMap<Id, (SimulatorId, SocketAddr)>,
    neighbor_connections: HashMap<u64, NeighborConnectionState>,
    retiring_simulators: HashSet<SimulatorId>,
    neighbor_task_owners: HashMap<Id, (SimulatorId, Option<u64>, SocketAddr)>,
    simulator_capabilities: HashMap<SimulatorId, SimulatorCapabilitySession>,
    connected_simulators: HashSet<SimulatorId>,
    preconnect_events: PreconnectEvents,
    pending_events: VecDeque<EventEnvelope>,
    logout_deadline: Option<Instant>,
    disconnect_reason: Option<&'static str>,
}

enum SessionStartup {
    Connecting { region_handshake: bool, movement_complete: bool, primary_announced: bool },
    Ready { primary_announced: bool, ready_published: bool },
}

impl Default for SessionStartup {
    fn default() -> Self {
        Self::Connecting { region_handshake: false, movement_complete: false, primary_announced: false }
    }
}

impl SessionStartup {
    fn observe_primary_message(&mut self, message: &Message, agent_id: Uuid, session_id: Uuid) -> bool {
        let Self::Connecting { region_handshake: seen_handshake, movement_complete: seen_movement, primary_announced } = self else {
            return false;
        };
        let region_handshake = matches!(message, Message::RegionHandshake(_));
        let movement_complete = matches!(message, Message::AgentMovementComplete(movement)
            if movement.agent_data.agent_id == agent_id && movement.agent_data.session_id == session_id);
        *seen_handshake |= region_handshake;
        *seen_movement |= movement_complete;
        if *seen_handshake && *seen_movement {
            let primary_announced = *primary_announced;
            *self = Self::Ready { primary_announced, ready_published: false };
            true
        } else {
            false
        }
    }

    fn mark_primary_announced(&mut self) {
        match self {
            Self::Connecting { primary_announced, .. } | Self::Ready { primary_announced, .. } => *primary_announced = true,
        }
    }

    fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    fn should_publish_ready(&self) -> bool {
        matches!(self, Self::Ready { primary_announced: true, ready_published: false })
    }

    fn mark_ready_published(&mut self) {
        if let Self::Ready { ready_published, .. } = self {
            *ready_published = true;
        }
    }
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

impl Drop for SimulatorCapabilitySession {
    fn drop(&mut self) {
        self.client.close_session();
    }
}

fn remove_simulator_capability(capabilities: &mut HashMap<SimulatorId, SimulatorCapabilitySession>, simulator_id: SimulatorId) {
    capabilities.remove(&simulator_id);
}

fn is_current_simulator(simulator_id: SimulatorId, primary_id: SimulatorId, connections: &HashMap<u64, NeighborConnectionState>) -> bool {
    simulator_id == primary_id || connections.values().any(|current| current.simulator_id == simulator_id)
}

fn is_current_neighbor(region_handle: u64, simulator_id: SimulatorId, connections: &HashMap<u64, NeighborConnectionState>) -> bool {
    connections.get(&region_handle).is_some_and(|current| current.simulator_id == simulator_id)
}

fn neighbor_event_queue_failure(
    simulator_id: SimulatorId,
    source: SocketAddr,
    exit: EventQueueExit,
    primary_id: SimulatorId,
    connections: &HashMap<u64, NeighborConnectionState>,
) -> Option<Event> {
    let reason = match exit {
        EventQueueExit::Stopped => return None,
        EventQueueExit::StoppedWithCoverageLoss => "The neighbor simulator EventQueueGet stopped after losing event coverage.",
        EventQueueExit::Unavailable => "The neighbor simulator does not have a usable EventQueueGet capability.",
        EventQueueExit::RetryLimitReached => "The neighbor simulator EventQueueGet stopped after repeated errors.",
        EventQueueExit::TaskFailed => "The neighbor simulator EventQueueGet task ended unexpectedly.",
    };
    is_current_simulator(simulator_id, primary_id, connections).then(|| Event::CapabilityError {
        simulator_id,
        source,
        capability: EVENT_QUEUE_NAME.to_owned(),
        reason: reason.into(),
    })
}

#[cfg(test)]
fn neighbor_event_queue_task_failure(
    error: &JoinError,
    owners: &mut HashMap<Id, (SimulatorId, SocketAddr)>,
    primary_id: SimulatorId,
    connections: &HashMap<u64, NeighborConnectionState>,
) -> Option<Event> {
    let (simulator_id, source) = owners.remove(&error.id())?;
    neighbor_event_queue_failure(simulator_id, source, EventQueueExit::TaskFailed, primary_id, connections)
}

#[cfg(test)]
fn remove_failed_neighbor(
    error: &JoinError,
    connections: &mut HashMap<u64, NeighborConnectionState>,
) -> Option<(u64, SimulatorId, SocketAddr)> {
    let region_handle =
        connections.iter().find(|(_, connection)| connection.abort.id() == error.id()).map(|(region_handle, _)| *region_handle)?;
    let connection = connections.remove(&region_handle)?;
    Some((region_handle, connection.simulator_id, connection.endpoint))
}

fn finish_logout(reason: Option<&'static str>) -> Result<(), String> {
    reason.map_or(Ok(()), |reason| Err(reason.to_owned()))
}

#[cfg(test)]
pub(crate) async fn run_session(
    session: AgentSession,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<EventEnvelope>,
    disconnect_requested: watch::Receiver<bool>,
) -> Result<(), String> {
    run_session_with_status(session, commands, events, disconnect_requested, NetworkStatusHandle::default(), None).await
}

#[cfg(test)]
async fn run_session_with_status(
    session: AgentSession,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<EventEnvelope>,
    disconnect_requested: watch::Receiver<bool>,
    status: NetworkStatusHandle,
    packet_memory_sender: Option<oneshot::Sender<SimulatorMemoryBudget>>,
) -> Result<(), String> {
    let failure_events = events.clone();
    let mut failure_shutdown = disconnect_requested.clone();
    let terminal_event_permit = events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let result = async {
        let connection = SimulatorCoordinator::connect_with_status(session, ResponseData::default(), None, status).await?;
        if let Some(sender) = packet_memory_sender {
            let _ = sender.send(connection.packet_memory.clone());
        }
        connection.run(commands, events, disconnect_requested, terminal_event_permit).await
    }
    .await;
    if let Err(error) = &result {
        send_failure_event(&failure_events, error.clone(), &mut failure_shutdown).await;
    }
    result
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LogoutOutcome {
    Sent,
    AlreadySent,
    Unavailable(LogoutUnavailable),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LogoutUnavailable {
    ConnectionClosed,
    ConnectionEnded,
    Deadline,
    QueueFull,
}

struct PendingLogout {
    completion: oneshot::Receiver<Result<(), Error>>,
    deadline: Instant,
}

impl PendingLogout {
    async fn wait(&mut self) -> Result<(), LogoutAttemptError> {
        match time::timeout_at(self.deadline, &mut self.completion).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(LogoutAttemptError::Unexpected(error.to_string())),
            Ok(Err(_)) => Err(LogoutAttemptError::Unavailable(LogoutUnavailable::ConnectionEnded)),
            Err(_) => Err(LogoutAttemptError::Unavailable(LogoutUnavailable::Deadline)),
        }
    }
}

impl LogoutUnavailable {
    fn description(self) -> &'static str {
        match self {
            Self::ConnectionClosed => "the active simulator connection is closed",
            Self::ConnectionEnded => "the active simulator connection ended before sending logout",
            Self::Deadline => "the bounded logout send deadline expired",
            Self::QueueFull => "the active simulator logout queue was full",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum LogoutAttemptError {
    Unavailable(LogoutUnavailable),
    Unexpected(String),
}

async fn send_logout_with_deadline(
    send: impl Future<Output = Result<(), LogoutAttemptError>>,
    deadline_reason: LogoutUnavailable,
) -> Result<(), LogoutAttemptError> {
    time::timeout(LOGOUT_TIMEOUT, send).await.map_err(|_| LogoutAttemptError::Unavailable(deadline_reason))?
}

#[cfg(test)]
mod tests;
