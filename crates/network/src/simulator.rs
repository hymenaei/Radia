use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    str::from_utf8,
    time::Duration,
};

use base::UUID;
use llsd::Value;
use protocol::{
    FieldValue, SchemaMessage,
    lludp::{Message, Packet},
    messages::{
        AgentMovementComplete, CompleteAgentMovement, CompletePingCheck, LogoutRequest, PingId, RegionHandshakeReply, UseCircuitCode,
        complete_agent_movement, logout_request, region_handshake_reply, use_circuit_code::CircuitCode,
    },
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::{AbortHandle, Id, JoinError, JoinSet},
    time::{self, Instant},
};
use url::Url;

#[cfg(test)]
use crate::event::send_failure_event;
use crate::{
    AgentSession, LoginResponseData,
    capabilities::{CapabilityBudget, Client, EVENT_QUEUE_NAME, EventQueueExit},
    capture::CaptureRecorder,
    circuit::{Circuit, Delivery, KEEPALIVE_TIMEOUT, PING_INTERVAL},
    client::Command,
    error::Error,
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, PacketOrder, SimulatorId},
    status::{NetworkLifecycle, NetworkStatusHandle},
    udp::UdpTransport,
};

const ACK_INTERVAL: Duration = Duration::from_millis(250);
const SESSION_TIMEOUT: Duration = Duration::from_secs(40);
const LOGOUT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_PENDING_SIMULATOR_EVENTS: usize = 16;
const EVENT_BACKLOG_WARNING_THRESHOLD: usize = MAX_PENDING_SIMULATOR_EVENTS / 2;
const EVENT_BACKLOG_WARNING_INTERVAL: Duration = Duration::from_secs(10);
const MAX_EVENTS_PER_PACKET: usize = 2;
const MAX_QUEUED_SIMULATOR_EVENTS: usize = MAX_PENDING_SIMULATOR_EVENTS + MAX_EVENTS_PER_PACKET - 1;
const MAX_SIMULATORS_PER_ENABLE_EVENT: usize = 32;
const MAX_NEIGHBOR_SIMULATORS: usize = 32;
const MAX_NEIGHBOR_CONNECTION_TASKS: usize = MAX_NEIGHBOR_SIMULATORS * 2;
const MAX_PRECONNECT_EVENTS: usize = 64;
const NEIGHBOR_COMMAND_CAPACITY: usize = 8;
const EVENT_BACKLOG_DISCONNECT_REASON: &str = "The simulator connection closed because the viewer could not keep up with incoming events.";
const NEIGHBOR_PRECONNECT_OVERFLOW_REASON: &str = "The neighbor simulator exceeded its bounded pre-connect event buffer.";

#[derive(Default)]
struct PrimaryEventBacklog {
    nonempty_since: Option<Instant>,
    last_warning: Option<Instant>,
}

impl PrimaryEventBacklog {
    fn observe(&mut self, pending_events: usize, now: Instant) -> Option<u64> {
        self.update_pending(pending_events, now);
        if pending_events < EVENT_BACKLOG_WARNING_THRESHOLD {
            return None;
        }
        if self.last_warning.is_some_and(|last| now.saturating_duration_since(last) < EVENT_BACKLOG_WARNING_INTERVAL) {
            return None;
        }

        self.last_warning = Some(now);
        Some(self.duration_ms(now))
    }

    fn update_pending(&mut self, pending_events: usize, now: Instant) {
        if pending_events == 0 {
            self.nonempty_since = None;
        } else if self.nonempty_since.is_none() {
            self.nonempty_since = Some(now);
        }
    }

    fn duration_ms(&self, now: Instant) -> u64 {
        self.nonempty_since.map(|since| now.saturating_duration_since(since).as_millis().try_into().unwrap_or(u64::MAX)).unwrap_or_default()
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

fn neighbor_completion_event(simulator_id: SimulatorId, region_handle: u64, endpoint: SocketAddr, outcome: &NeighborTaskOutcome) -> Event {
    match outcome {
        NeighborTaskOutcome::Finished(Ok(())) => {
            Event::SimulatorDisconnected { simulator_id, region_handle: Some(region_handle), endpoint }
        }
        NeighborTaskOutcome::Finished(Err(reason)) => {
            Event::SimulatorFailed { simulator_id, region_handle: Some(region_handle), endpoint, reason: reason.clone() }
        }
        NeighborTaskOutcome::ConnectFailed(_) => Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
    }
}

enum NeighborEvent {
    Simulator(Event),
    Finished { simulator_id: SimulatorId, region_handle: u64, endpoint: SocketAddr, outcome: NeighborTaskOutcome },
}

struct NeighborConnectionState {
    simulator_id: SimulatorId,
    endpoint: SocketAddr,
    abort: AbortHandle,
    commands: mpsc::Sender<NeighborCommand>,
}

#[derive(Default)]
struct PreconnectEvents {
    by_simulator: HashMap<SimulatorId, VecDeque<EventEnvelope>>,
    len: usize,
}

impl PreconnectEvents {
    fn push(&mut self, simulator_id: SimulatorId, event: EventEnvelope) -> bool {
        if self.len >= MAX_PRECONNECT_EVENTS {
            return false;
        }
        self.by_simulator.entry(simulator_id).or_default().push_back(event);
        self.len += 1;
        true
    }

    fn take(&mut self, simulator_id: SimulatorId) -> VecDeque<EventEnvelope> {
        let events = self.by_simulator.remove(&simulator_id).unwrap_or_default();
        self.len -= events.len();
        events
    }

    fn remove(&mut self, simulator_id: SimulatorId) {
        drop(self.take(simulator_id));
    }
}

enum PreconnectOutcome {
    Ready(EventEnvelope),
    Buffered,
    Full,
}

fn queue_until_connected(
    connected: &HashSet<SimulatorId>,
    pending: &mut PreconnectEvents,
    simulator_id: SimulatorId,
    event: EventEnvelope,
) -> PreconnectOutcome {
    if connected.contains(&simulator_id) {
        return PreconnectOutcome::Ready(event);
    }
    if pending.push(simulator_id, event) { PreconnectOutcome::Buffered } else { PreconnectOutcome::Full }
}

enum NeighborCommand {
    Send { message: Message, delivery: Delivery, completed: oneshot::Sender<Result<(), Error>> },
    PrepareHandoff(HandoffTarget),
    CancelHandoff,
    Logout(oneshot::Sender<Result<(), Error>>),
}

#[derive(Clone, Eq, PartialEq)]
struct HandoffTarget {
    agent_id: UUID,
    session_id: UUID,
    region_handle: u64,
    endpoint: SocketAddr,
    seed_capability: Url,
}

#[derive(Clone)]
struct PendingPromotion {
    source: SimulatorId,
    target: SimulatorId,
    handoff: HandoffTarget,
    evidence: HandoffEvidence,
}

#[derive(Clone, Copy)]
enum HandoffAnnouncement {
    CrossedRegion,
    TeleportFinish(PacketOrder),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FinishOrder {
    Ordered(u64),
    Unusable,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct HandoffEvidence {
    crossed_region: bool,
    teleport_finish: Option<FinishOrder>,
}

impl HandoffEvidence {
    fn record(&mut self, announcement: HandoffAnnouncement) {
        match announcement {
            HandoffAnnouncement::CrossedRegion => self.crossed_region = true,
            HandoffAnnouncement::TeleportFinish(PacketOrder::Ordered(sequence)) => {
                self.teleport_finish = Some(match self.teleport_finish {
                    None => FinishOrder::Ordered(sequence),
                    Some(FinishOrder::Ordered(current)) => FinishOrder::Ordered(current.max(sequence)),
                    Some(FinishOrder::Unusable) => FinishOrder::Unusable,
                });
            }
            HandoffAnnouncement::TeleportFinish(PacketOrder::Ambiguous | PacketOrder::Degraded) => {
                self.teleport_finish = Some(FinishOrder::Unusable);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HandoffDecision {
    SameDestination,
    ReplaceNewer,
    IgnoreStale,
    Conflict,
}

fn same_handoff_destination(left: &HandoffTarget, right: &HandoffTarget) -> bool {
    left.agent_id == right.agent_id
        && left.session_id == right.session_id
        && left.region_handle == right.region_handle
        && left.endpoint == right.endpoint
}

fn arbitrate_handoff(current: &PendingPromotion, incoming: &HandoffTarget, announcement: HandoffAnnouncement) -> HandoffDecision {
    if same_handoff_destination(&current.handoff, incoming) {
        return HandoffDecision::SameDestination;
    }
    if current.evidence.crossed_region {
        return HandoffDecision::Conflict;
    }
    let Some(FinishOrder::Ordered(current_sequence)) = current.evidence.teleport_finish else {
        return HandoffDecision::Conflict;
    };
    let HandoffAnnouncement::TeleportFinish(PacketOrder::Ordered(incoming_sequence)) = announcement else {
        return HandoffDecision::Conflict;
    };
    match incoming_sequence.cmp(&current_sequence) {
        Ordering::Greater => HandoffDecision::ReplaceNewer,
        Ordering::Less => HandoffDecision::IgnoreStale,
        Ordering::Equal => HandoffDecision::Conflict,
    }
}

fn teleport_failure_matches(pending: Option<&PendingPromotion>, packet_order: PacketOrder) -> bool {
    let Some(pending) = pending else { return false };
    if pending.evidence.crossed_region {
        return false;
    }
    matches!(
        (pending.evidence.teleport_finish, packet_order),
        (Some(FinishOrder::Ordered(finish)), PacketOrder::Ordered(failed)) if failed > finish
    )
}

struct PromotionSetup<'a> {
    neighbors: &'a mut JoinSet<(SimulatorId, u64, SocketAddr, NeighborTaskOutcome)>,
    neighbor_connections: &'a mut HashMap<u64, NeighborConnectionState>,
    retiring_simulators: &'a mut HashSet<SimulatorId>,
    neighbor_task_owners: &'a mut HashMap<Id, (SimulatorId, u64, SocketAddr)>,
    preconnect_events: &'a mut PreconnectEvents,
    connected_simulators: &'a mut HashSet<SimulatorId>,
    simulator_capabilities: &'a mut HashMap<SocketAddr, SimulatorCapabilitySession>,
    neighbor_requests: mpsc::Sender<NeighborRequest>,
    application_events: mpsc::Sender<EventEnvelope>,
    neighbor_events: mpsc::Sender<NeighborEvent>,
    shutdown: watch::Receiver<bool>,
    pending_events: &'a mut VecDeque<EventEnvelope>,
    drop_application_messages: &'a mut bool,
    logout_deadline: &'a mut Option<Instant>,
    disconnect_reason: &'a mut Option<&'static str>,
    capability_events: mpsc::Sender<EventEnvelope>,
    neighbor_event_queues: &'a mut JoinSet<(SimulatorId, SocketAddr, EventQueueExit)>,
    neighbor_event_queue_owners: &'a mut HashMap<Id, (SimulatorId, SocketAddr)>,
}

fn cancel_pending_handoff(
    pending: &mut Option<PendingPromotion>,
    neighbor_connections: &mut HashMap<u64, NeighborConnectionState>,
    retiring_simulators: &mut HashSet<SimulatorId>,
    preconnect_events: &mut PreconnectEvents,
    connected_simulators: &mut HashSet<SimulatorId>,
    simulator_capabilities: &mut HashMap<SocketAddr, SimulatorCapabilitySession>,
) {
    let Some(current) = pending.take() else { return };
    if neighbor_connections.get(&current.handoff.region_handle).is_some_and(|connection| connection.simulator_id == current.target)
        && let Some(connection) = neighbor_connections.remove(&current.handoff.region_handle)
    {
        retiring_simulators.insert(connection.simulator_id);
        preconnect_events.remove(connection.simulator_id);
        connected_simulators.remove(&connection.simulator_id);
        remove_simulator_capability(simulator_capabilities, connection.endpoint, connection.simulator_id);
        connection.abort.abort();
    }
}

fn matches_pending_promotion(
    pending: Option<&PendingPromotion>,
    active_primary: SimulatorId,
    simulator_id: SimulatorId,
    movement: &AgentMovementComplete,
) -> bool {
    pending.is_some_and(|pending| {
        pending.source == active_primary
            && pending.target == simulator_id
            && movement.agent_data.agent_id == pending.handoff.agent_id
            && movement.agent_data.session_id == pending.handoff.session_id
            && movement.data.region_handle == pending.handoff.region_handle
    })
}

pub(crate) struct SimulatorConnection {
    simulator_id: SimulatorId,
    active_primary_id: SimulatorId,
    active_primary_endpoint: SocketAddr,
    active_primary_region_handle: Option<u64>,
    active_primary_commands: Option<mpsc::Sender<NeighborCommand>>,
    root_connection_failed: bool,
    session: AgentSession,
    socket: UdpTransport,
    circuit: Circuit,
    capabilities: Client,
    capability_budget: CapabilityBudget,
    login_data: LoginResponseData,
    use_circuit_sequence: u32,
    capture: Option<CaptureRecorder>,
    status: NetworkStatusHandle,
    event_backlog: PrimaryEventBacklog,
}

impl Drop for SimulatorConnection {
    fn drop(&mut self) {
        self.capabilities.close_session();
    }
}

impl SimulatorConnection {
    #[tracing::instrument(name = "simulator.connect", level = "info", skip_all)]
    pub(crate) async fn connect_with_status(
        session: AgentSession,
        login_data: LoginResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
    ) -> Result<Self, String> {
        status.set_simulator_endpoint(session.simulator_address);
        status.set_lifecycle(NetworkLifecycle::Connecting);
        let result = Self::connect_inner(session, login_data, capture, status.clone()).await;
        if let Err(error) = &result {
            status.clear_simulator();
            status.record_error(error);
            status.set_lifecycle(NetworkLifecycle::Failed);
        }
        result
    }

    #[tracing::instrument(name = "simulator.bootstrap", level = "debug", skip_all)]
    async fn connect_inner(
        session: AgentSession,
        login_data: LoginResponseData,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
    ) -> Result<Self, String> {
        let simulator_id = SimulatorId::allocate().ok_or_else(|| "Simulator connection ID space is exhausted.".to_string())?;
        if let Some(capture) = &capture {
            capture.circuit_lifecycle("circuit_starting", session.circuit_code);
        }
        let socket =
            UdpTransport::connect_with_status(session.simulator_address, session.circuit_code, capture.clone(), status.clone()).await?;
        let capability_budget = CapabilityBudget::new();
        let capabilities = Client::new_with_status_and_budget(
            session.seed_capability.clone(),
            capture.clone(),
            status.clone(),
            capability_budget.clone(),
        )?;
        let mut circuit = Circuit::with_status(status.clone());
        let use_circuit_sequence = circuit
            .send(
                &socket,
                Message::UseCircuitCode(UseCircuitCode {
                    circuit_code: CircuitCode { code: session.circuit_code, session_id: session.session_id, agent_id: session.agent_id },
                }),
                Delivery::Reliable,
            )
            .await?;
        if let Some(capture) = &capture {
            capture.circuit_lifecycle("use_circuit_code_sent", session.circuit_code);
        }
        Ok(Self {
            simulator_id,
            active_primary_id: simulator_id,
            active_primary_endpoint: session.simulator_address,
            active_primary_region_handle: None,
            active_primary_commands: None,
            root_connection_failed: false,
            session,
            socket,
            circuit,
            capabilities,
            capability_budget,
            login_data,
            use_circuit_sequence,
            capture,
            status,
            event_backlog: PrimaryEventBacklog::default(),
        })
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
        status.clear_simulator();
        match &result {
            Ok(()) => status.set_lifecycle(NetworkLifecycle::Disconnected),
            Err(error) => {
                status.record_error(error);
                status.set_lifecycle(NetworkLifecycle::Failed);
            }
        }
        if !self.root_connection_failed || self.active_primary_id != simulator_id {
            Self::publish_primary_terminal_event(
                terminal_event_permit,
                self.active_primary_id,
                self.active_primary_region_handle,
                self.active_primary_endpoint,
                &result,
            );
        }
        result
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
        mut commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        mut disconnect_requested: watch::Receiver<bool>,
    ) -> Result<(), String> {
        if *disconnect_requested.borrow() {
            self.disconnect_once(false).await?;
            return Ok(());
        }
        let (capability_events, mut capability_event_rx) = mpsc::channel(CLIENT_EVENT_CAPACITY);
        let (neighbor_events, mut neighbor_event_rx) = mpsc::channel::<NeighborEvent>(CLIENT_EVENT_CAPACITY);
        let mut event_queue = self.capabilities.start_event_queue(
            self.simulator_id,
            self.session.simulator_address,
            capability_events.clone(),
            disconnect_requested.clone(),
        )?;
        let mut event_queue_finished = false;
        let mut capability_events_closed = false;
        let mut neighbor_events_closed = false;
        let mut agent_movement_sent = false;
        let mut receive_buffer = [0u8; 65_535];
        let mut saw_region_handshake = false;
        let mut saw_movement_complete = false;
        let mut primary_announced = false;
        let mut session_ready = false;
        let mut session_ready_published = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;
        let mut pending_events = VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS);
        let mut drop_application_messages = false;
        let (neighbor_requests, mut neighbor_request_rx) = mpsc::channel(32);
        let mut neighbors = JoinSet::<(SimulatorId, u64, SocketAddr, NeighborTaskOutcome)>::new();
        let mut neighbor_event_queues = JoinSet::<(SimulatorId, SocketAddr, EventQueueExit)>::new();
        let mut neighbor_event_queue_owners = HashMap::<Id, (SimulatorId, SocketAddr)>::new();
        let mut neighbor_connections: HashMap<u64, NeighborConnectionState> = HashMap::new();
        let mut retiring_simulators = HashSet::new();
        let mut neighbor_task_owners = HashMap::<Id, (SimulatorId, u64, SocketAddr)>::new();
        let mut pending_neighbor_finishes = VecDeque::new();
        let mut simulator_capabilities: HashMap<SocketAddr, SimulatorCapabilitySession> = HashMap::new();
        let mut connected_simulators = HashSet::new();
        let mut preconnect_events = PreconnectEvents::default();
        let mut retry_timer = time::interval(ACK_INTERVAL);
        retry_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut ping_timer = time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
        ping_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut last_udp_received = Instant::now();
        let mut root_connection_active = true;
        let mut pending_promotion = None::<PendingPromotion>;
        let mut handoff_conflicted = false;
        let session_deadline = time::sleep(SESSION_TIMEOUT);
        tokio::pin!(session_deadline);

        loop {
            tokio::select! {
                _ = disconnect_requested.changed() => {
                    self.disconnect_once(logout_deadline.is_some()).await?;
                    return Ok(());
                }
                _ = &mut session_deadline, if !session_ready && !*disconnect_requested.borrow() => {
                    tracing::warn!("Simulator session timed out");
                    if let Some(capture) = &self.capture {
                        capture.circuit_lifecycle("session_timeout", self.session.circuit_code);
                    }
                    return Err("Timed out while establishing the simulator session.".into());
                }
                _ = time::sleep_until(logout_deadline.unwrap_or_else(|| Instant::now() + LOGOUT_TIMEOUT)), if logout_deadline.is_some() => {
                    return finish_logout(disconnect_reason);
                }
                _ = events.closed(), if logout_deadline.is_none() => {
                    self.disconnect_once(false).await?;
                    return Ok(());
                }
                _ = time::sleep_until(last_udp_received + KEEPALIVE_TIMEOUT), if root_connection_active && self.active_primary_id == self.simulator_id && session_ready && logout_deadline.is_none() && !*disconnect_requested.borrow() => {
                    return Err("The simulator stopped responding to LLUDP keepalive traffic.".into());
                }
                capability_event = capability_event_rx.recv(), if !capability_events_closed => {
                    match capability_event {
                        Some(envelope) => {
                            let (event, retained_bytes) = envelope.into_parts();
                            match event {
                        Event::CapabilityEvent { simulator_id, source, message, body } => {
                            if !is_current_simulator(simulator_id, self.simulator_id, &neighbor_connections) {
                                continue;
                            }
                            if message == "EnableSimulator" {
                                match decode_enable_simulators(&body) {
                                    Ok(simulators) => {
                                        for (region_handle, endpoint) in simulators {
                                            Box::pin(self.start_neighbor(
                                                simulator_id,
                                                region_handle,
                                                endpoint,
                                                None,
                                                &mut neighbors,
                                                &mut neighbor_connections,
                                                &mut retiring_simulators,
                                                &mut neighbor_task_owners,
                                                &mut preconnect_events,
                                                &mut simulator_capabilities,
                                                neighbor_requests.clone(),
                                                events.clone(),
                                                neighbor_events.clone(),
                                                disconnect_requested.clone(),
                                                &mut pending_events,
                                                &mut drop_application_messages,
                                                &mut logout_deadline,
                                                &mut disconnect_reason,
                                            )).await?;
                                        }
                                    }
                                    Err(reason) => self.status.record_error(format!("Ignored malformed EnableSimulator event: {reason}")),
                                }
                            } else if message == "CrossedRegion" {
                                match decode_crossed_region(&body, self.session.agent_id, self.session.session_id) {
                                    Ok(handoff) => {
                                        let mut setup = PromotionSetup {
                                            neighbors: &mut neighbors,
                                            neighbor_connections: &mut neighbor_connections,
                                            retiring_simulators: &mut retiring_simulators,
                                            neighbor_task_owners: &mut neighbor_task_owners,
                                            preconnect_events: &mut preconnect_events,
                                            connected_simulators: &mut connected_simulators,
                                            simulator_capabilities: &mut simulator_capabilities,
                                            neighbor_requests: neighbor_requests.clone(),
                                            application_events: events.clone(),
                                            neighbor_events: neighbor_events.clone(),
                                            shutdown: disconnect_requested.clone(),
                                            pending_events: &mut pending_events,
                                            drop_application_messages: &mut drop_application_messages,
                                            logout_deadline: &mut logout_deadline,
                                            disconnect_reason: &mut disconnect_reason,
                                            capability_events: capability_events.clone(),
                                            neighbor_event_queues: &mut neighbor_event_queues,
                                            neighbor_event_queue_owners: &mut neighbor_event_queue_owners,
                                        };
                                        self.request_handoff(
                                            simulator_id,
                                            handoff,
                                            HandoffAnnouncement::CrossedRegion,
                                            &mut setup,
                                            &mut pending_promotion,
                                            &mut handoff_conflicted,
                                        )
                                        .await?;
                                    }
                                    Err(reason) => self.status.record_error(format!("Ignored malformed CrossedRegion event: {reason}")),
                                }
                            } else if message == "EstablishAgentCommunication" {
                                match decode_agent_communication(&body, self.session.agent_id) {
                                    Ok((endpoint, _)) if endpoint == self.session.simulator_address => {
                                        self.status.record_error("Ignored an agent communication event for the primary simulator.");
                                    }
                                    Ok((endpoint, _))
                                        if !neighbor_connections.values().any(|active| active.endpoint == endpoint) =>
                                    {
                                        self.status.record_error("Ignored a seed capability for an untracked simulator.");
                                    }
                                    Ok((endpoint, seed)) if !simulator_capabilities.contains_key(&endpoint) => {
                                        let Some(target_id) = neighbor_connections
                                            .values()
                                            .find(|active| active.endpoint == endpoint)
                                            .map(|active| active.simulator_id)
                                        else {
                                            continue;
                                        };
                                        match Client::new_with_status_and_budget(
                                            seed.clone(),
                                            self.capture.clone(),
                                            self.status.clone(),
                                            self.capability_budget.clone(),
                                        ) {
                                            Ok(client) => match client.start_event_queue(
                                                target_id,
                                                endpoint,
                                                capability_events.clone(),
                                                disconnect_requested.clone(),
                                            ) {
                                                Ok(event_queue) => {
                                                    let task = neighbor_event_queues.spawn(async move {
                                                        let exit = event_queue.wait().await;
                                                        (target_id, endpoint, exit)
                                                    });
                                                    neighbor_event_queue_owners.insert(task.id(), (target_id, endpoint));
                                                    simulator_capabilities.insert(
                                                        endpoint,
                                                        SimulatorCapabilitySession {
                                                            simulator_id: target_id,
                                                            seed_capability: seed,
                                                            client: client.clone(),
                                                        },
                                                    );
                                                    self.publish_simulator_event(
                                                        target_id,
                                                        Event::SimulatorCapabilities {
                                                            simulator_id: target_id,
                                                            source: endpoint,
                                                            capabilities: client,
                                                        }
                                                        .into(),
                                                        &events,
                                                        &mut pending_events,
                                                        &mut preconnect_events,
                                                        &mut connected_simulators,
                                                        &mut neighbor_connections,
                                                        &mut simulator_capabilities,
                                                        &mut drop_application_messages,
                                                        &mut logout_deadline,
                                                        &mut disconnect_reason,
                                                    ).await?;
                                                }
                                                Err(error) => self.status.record_error(error),
                                            },
                                            Err(error) => self.status.record_error(error),
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(reason) => self.status.record_error(format!("Ignored malformed agent communication event: {reason}")),
                                }
                            }
                            self.publish_simulator_event(
                                simulator_id,
                                EventEnvelope::from_parts(
                                    Event::CapabilityEvent {
                                        simulator_id,
                                        source,
                                        message: message.clone(),
                                        body: redact_seed_capability(&body),
                                    },
                                    retained_bytes,
                                ),
                                &events,
                                &mut pending_events,
                                &mut preconnect_events,
                                &mut connected_simulators,
                                &mut neighbor_connections,
                                &mut simulator_capabilities,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            ).await?;
                        }
                        Event::CapabilityError { simulator_id, source, capability, reason }
                            if is_current_simulator(simulator_id, self.simulator_id, &neighbor_connections) =>
                        {
                            self.publish_simulator_event(
                                simulator_id,
                                EventEnvelope::from_parts(
                                    Event::CapabilityError { simulator_id, source, capability, reason },
                                    retained_bytes,
                                ),
                                &events,
                                &mut pending_events,
                                &mut preconnect_events,
                                &mut connected_simulators,
                                &mut neighbor_connections,
                                &mut simulator_capabilities,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            ).await?;
                        }
                        Event::CapabilityError { .. } => {}
                        event => {
                            self.publish_event(
                                &events,
                                &mut pending_events,
                                &mut drop_application_messages,
                                EventEnvelope::from_parts(event, retained_bytes),
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            ).await?;
                        }
                            }
                        }
                        None => capability_events_closed = true,
                    }
                }
                neighbor_event = neighbor_event_rx.recv(), if !neighbor_events_closed => {
                    match neighbor_event {
                        Some(NeighborEvent::Simulator(event)) => {
                            let is_current = match &event {
                                Event::SimulatorConnected { simulator_id, .. }
                                | Event::SimulatorMessage { simulator_id, .. } => {
                                    is_current_simulator(*simulator_id, self.simulator_id, &neighbor_connections)
                                }
                                _ => false,
                            };
                            if is_current {
                                if let Event::SimulatorMessage { simulator_id, packet_order, message, .. } = &event
                                    && is_handoff_message(message)
                                {
                                    let mut setup = PromotionSetup {
                                        neighbors: &mut neighbors,
                                        neighbor_connections: &mut neighbor_connections,
                                        retiring_simulators: &mut retiring_simulators,
                                        neighbor_task_owners: &mut neighbor_task_owners,
                                        preconnect_events: &mut preconnect_events,
                                        connected_simulators: &mut connected_simulators,
                                        simulator_capabilities: &mut simulator_capabilities,
                                        neighbor_requests: neighbor_requests.clone(),
                                        application_events: events.clone(),
                                        neighbor_events: neighbor_events.clone(),
                                        shutdown: disconnect_requested.clone(),
                                        pending_events: &mut pending_events,
                                        drop_application_messages: &mut drop_application_messages,
                                        logout_deadline: &mut logout_deadline,
                                        disconnect_reason: &mut disconnect_reason,
                                        capability_events: capability_events.clone(),
                                        neighbor_event_queues: &mut neighbor_event_queues,
                                        neighbor_event_queue_owners: &mut neighbor_event_queue_owners,
                                    };
                                    self.handle_handoff_message(
                                        *simulator_id,
                                        message,
                                        *packet_order,
                                        &mut setup,
                                        &mut pending_promotion,
                                        &mut handoff_conflicted,
                                    )
                                    .await?;
                                }
                                if let Event::SimulatorMessage {
                                    simulator_id,
                                    message: Message::AgentMovementComplete(movement),
                                    ..
                                } = &event
                                    && matches_pending_promotion(
                                        pending_promotion.as_ref(),
                                        self.active_primary_id,
                                        *simulator_id,
                                        movement,
                                    )
                                {
                                    let promoted_simulator_id = *simulator_id;
                                    let promotion = pending_promotion.take().expect("matching promotion is pending");
                                    handoff_conflicted = false;
                                    let Some(connection) = neighbor_connections
                                        .get(&promotion.handoff.region_handle)
                                        .filter(|connection| {
                                            connection.simulator_id == promotion.target
                                                && connection.endpoint == promotion.handoff.endpoint
                                        })
                                    else {
                                        self.status.record_error("Ignored movement completion from a stale destination circuit.");
                                        continue;
                                    };
                                    let active_commands = connection.commands.clone();
                                    let Some(capabilities) = simulator_capabilities
                                        .get(&promotion.handoff.endpoint)
                                        .filter(|capabilities| capabilities.simulator_id == promotion.target)
                                        .map(|capabilities| capabilities.client.clone())
                                    else {
                                        self.status.record_error("Ignored movement completion without destination capabilities.");
                                        continue;
                                    };
                                    let previous_simulator_id = self.active_primary_id;
                                    self.publish_simulator_event(
                                        promoted_simulator_id,
                                        EventEnvelope::new(event),
                                        &events,
                                        &mut pending_events,
                                        &mut preconnect_events,
                                        &mut connected_simulators,
                                        &mut neighbor_connections,
                                        &mut simulator_capabilities,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    )
                                    .await?;
                                    let mut session = self.session.clone();
                                    session.simulator_address = promotion.handoff.endpoint;
                                    session.seed_capability = promotion.handoff.seed_capability;
                                    self.active_primary_id = promotion.target;
                                    self.active_primary_endpoint = session.simulator_address;
                                    self.active_primary_region_handle = Some(promotion.handoff.region_handle);
                                    self.active_primary_commands = Some(active_commands);
                                    self.status.set_simulator_endpoint(session.simulator_address);
                                    self.status.set_circuit_established(true);
                                    self.status.set_lifecycle(NetworkLifecycle::Connected);
                                    self.publish_event(
                                        &events,
                                        &mut pending_events,
                                        &mut drop_application_messages,
                                        Event::SimulatorPromoted {
                                            previous_simulator_id,
                                            simulator_id: promotion.target,
                                            session: Box::new(session),
                                            capabilities,
                                        },
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    )
                                    .await?;
                                    continue;
                                }
                                if let Event::SimulatorConnected { simulator_id, .. } = &event {
                                    let simulator_id = *simulator_id;
                                    self.publish_simulator_connected(
                                        simulator_id,
                                        event,
                                        &events,
                                        &mut pending_events,
                                        &mut preconnect_events,
                                        &mut connected_simulators,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    ).await?;
                                } else if let Event::SimulatorMessage { simulator_id, .. } = &event {
                                    self.publish_simulator_event(
                                        *simulator_id,
                                        EventEnvelope::new(event),
                                        &events,
                                        &mut pending_events,
                                        &mut preconnect_events,
                                        &mut connected_simulators,
                                        &mut neighbor_connections,
                                        &mut simulator_capabilities,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    ).await?;
                                }
                            }
                        }
                        Some(NeighborEvent::Finished { simulator_id, region_handle, endpoint, outcome }) => {
                            if is_current_simulator(simulator_id, self.simulator_id, &neighbor_connections)
                                || retiring_simulators.contains(&simulator_id)
                            {
                                if pending_promotion.as_ref().is_some_and(|pending| pending.target == simulator_id) {
                                    pending_promotion = None;
                                }
                                if is_current_neighbor(region_handle, simulator_id, &neighbor_connections) {
                                    neighbor_connections.remove(&region_handle);
                                }
                                retiring_simulators.remove(&simulator_id);
                                remove_simulator_capability(&mut simulator_capabilities, endpoint, simulator_id);
                                preconnect_events.remove(simulator_id);
                                connected_simulators.remove(&simulator_id);
                                let event = neighbor_completion_event(simulator_id, region_handle, endpoint, &outcome);
                                if let NeighborTaskOutcome::Finished(Err(reason)) | NeighborTaskOutcome::ConnectFailed(reason) = &outcome {
                                    self.status.record_error(reason);
                                }
                                if simulator_id == self.active_primary_id {
                                    return match outcome {
                                        NeighborTaskOutcome::Finished(Ok(())) => finish_logout(disconnect_reason),
                                        NeighborTaskOutcome::Finished(Err(reason)) | NeighborTaskOutcome::ConnectFailed(reason) => {
                                            Err(reason)
                                        }
                                    };
                                }
                                self.publish_event(
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    event,
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                ).await?;
                            }
                        }
                        None => neighbor_events_closed = true,
                    }
                }
                permit = neighbor_events.reserve(), if !pending_neighbor_finishes.is_empty() && !neighbor_events.is_closed() => {
                    match permit {
                        Ok(permit) => {
                            if let Some(finish) = pending_neighbor_finishes.pop_front() {
                                permit.send(finish);
                            }
                        }
                        Err(_) => pending_neighbor_finishes.clear(),
                    }
                }
                joined = neighbor_event_queues.join_next(), if !neighbor_event_queues.is_empty() => {
                    match joined {
                        Some(Ok((simulator_id, source, exit))) => {
                            neighbor_event_queue_owners.retain(|_, owner| *owner != (simulator_id, source));
                            if simulator_id == self.active_primary_id
                                && simulator_id != self.simulator_id
                                && !matches!(exit, EventQueueExit::Stopped | EventQueueExit::PollCanceled)
                            {
                                self.disconnect_once(false).await?;
                                return Err("The active simulator EventQueueGet stopped unexpectedly.".into());
                            }
                            if let Some(event) = neighbor_event_queue_failure(simulator_id, source, exit, self.simulator_id, &neighbor_connections) {
                                self.publish_event(
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    event,
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                ).await?;
                            }
                        }
                        Some(Err(error)) => {
                            let active_primary_queue = neighbor_event_queue_owners
                                .get(&error.id())
                                .is_some_and(|(simulator_id, _)| *simulator_id == self.active_primary_id);
                            if active_primary_queue && self.active_primary_id != self.simulator_id {
                                self.disconnect_once(false).await?;
                                return Err("The active simulator EventQueueGet task failed unexpectedly.".into());
                            }
                            if let Some(event) = neighbor_event_queue_task_failure(
                                &error,
                                &mut neighbor_event_queue_owners,
                                self.simulator_id,
                                &neighbor_connections,
                            ) {
                                self.publish_event(
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    event,
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                ).await?;
                            }
                        }
                        None => {}
                    }
                }
                exit = event_queue.completion(), if !event_queue_finished => {
                    event_queue_finished = true;
                    if self.active_primary_id == self.simulator_id {
                        self.handle_primary_event_queue_exit(exit, logout_deadline.is_some()).await?;
                    } else if matches!(exit, EventQueueExit::TaskFailed | EventQueueExit::RetryLimitReached | EventQueueExit::Unavailable) {
                        self.status.record_error("The former primary simulator EventQueueGet ended after promotion.");
                    }
                }
                permit = events.reserve(), if !pending_events.is_empty() && !events.is_closed() => {
                    match permit {
                        Ok(permit) => {
                            if let Some(event) = pending_events.pop_front() {
                                permit.send(event);
                            }
                            self.event_backlog.update_pending(pending_events.len(), Instant::now());
                        }
                        Err(_) if logout_deadline.is_none() => {
                            self.disconnect_once(false).await?;
                            return Ok(());
                        }
                        Err(_) => {}
                    }
                }
                request = neighbor_request_rx.recv(), if logout_deadline.is_none() => {
                    if let Some(NeighborRequest::Enable { simulator_id, region_handle, endpoint }) = request
                        && is_current_simulator(simulator_id, self.simulator_id, &neighbor_connections)
                    {
                        Box::pin(self.start_neighbor(
                            simulator_id,
                            region_handle,
                            endpoint,
                            None,
                            &mut neighbors,
                            &mut neighbor_connections,
                            &mut retiring_simulators,
                            &mut neighbor_task_owners,
                            &mut preconnect_events,
                            &mut simulator_capabilities,
                            neighbor_requests.clone(),
                            events.clone(),
                            neighbor_events.clone(),
                            disconnect_requested.clone(),
                            &mut pending_events,
                            &mut drop_application_messages,
                            &mut logout_deadline,
                            &mut disconnect_reason,
                        )).await?;
                    }
                }
                joined = neighbors.join_next(), if !neighbors.is_empty() => {
                    match joined {
                        Some(Ok((simulator_id, _region_handle, _endpoint, _outcome))) => {
                            neighbor_task_owners.retain(|_, owner| owner.0 != simulator_id);
                        }
                        Some(Err(error)) => {
                            self.status.record_error(format!("A simulator connection task failed: {error}"));
                            if let Some((simulator_id, region_handle, endpoint)) = neighbor_task_owners.remove(&error.id())
                                && (is_current_neighbor(region_handle, simulator_id, &neighbor_connections)
                                    || retiring_simulators.contains(&simulator_id))
                            {
                                let outcome = if error.is_cancelled() && retiring_simulators.contains(&simulator_id) {
                                    NeighborTaskOutcome::Finished(Ok(()))
                                } else {
                                    NeighborTaskOutcome::Finished(Err(error.to_string()))
                                };
                                let finish = NeighborEvent::Finished { simulator_id, region_handle, endpoint, outcome };
                                match neighbor_events.try_send(finish) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(finish)) => pending_neighbor_finishes.push_back(finish),
                                    Err(mpsc::error::TrySendError::Closed(_)) => {}
                                }
                            }
                        }
                        None => {}
                    }
                }
                command = commands.recv(), if logout_deadline.is_none() => {
                    if *disconnect_requested.borrow() {
                        self.disconnect_once(false).await?;
                        return Ok(());
                    }
                    match command {
                        Some(Command::SubmitMfaCode(_)) => {}
                        Some(Command::SendMessage { message, delivery }) => {
                            if !session_ready {
                                self.publish_event(
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    Event::SendFailed("The simulator session is still starting.".into()),
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                ).await?;
                                continue;
                            }

                            let cancel_handoff = is_teleport_cancel(&message, self.session.agent_id, self.session.session_id);
                            let result = if let Some(commands) = &self.active_primary_commands {
                                send_neighbor_message(commands, message, delivery).await
                            } else if root_connection_active {
                                self.circuit.send(&self.socket, message, delivery).await.map(|_| ()).map_err(Error::SimulatorSend)
                            } else {
                                Err(Error::SimulatorConnectionClosed)
                            };
                            if result.is_ok() && cancel_handoff {
                                cancel_pending_handoff(
                                    &mut pending_promotion,
                                    &mut neighbor_connections,
                                    &mut retiring_simulators,
                                    &mut preconnect_events,
                                    &mut connected_simulators,
                                    &mut simulator_capabilities,
                                );
                                handoff_conflicted = false;
                            }
                            if let Err(error) = result {
                                self.publish_event(
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    Event::SendFailed(error.to_string()),
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                ).await?;
                            }
                        }
                        Some(Command::SendTo { simulator_id, message, delivery, completed }) => {
                            let cancel_handoff = simulator_id == self.active_primary_id
                                && is_teleport_cancel(&message, self.session.agent_id, self.session.session_id);
                            if simulator_id == self.simulator_id {
                                let result = if root_connection_active && session_ready {
                                    self.circuit
                                        .send(&self.socket, message, delivery)
                                        .await
                                        .map(|_| ())
                                        .map_err(Error::SimulatorSend)
                                } else {
                                    Err(if root_connection_active { Error::SimulatorNotReady } else { Error::SimulatorConnectionClosed })
                                };
                                let sent = result.is_ok();
                                let _ = completed.send(result);
                                if cancel_handoff && sent {
                                    cancel_pending_handoff(
                                        &mut pending_promotion,
                                        &mut neighbor_connections,
                                        &mut retiring_simulators,
                                        &mut preconnect_events,
                                        &mut connected_simulators,
                                        &mut simulator_capabilities,
                                    );
                                    handoff_conflicted = false;
                                }
                            } else if let Some(connection) = neighbor_connections
                                .values()
                                .find(|connection| connection.simulator_id == simulator_id)
                            {
                                let commands = connection.commands.clone();
                                let result = send_neighbor_message(&commands, message, delivery).await;
                                let sent = result.is_ok();
                                let _ = completed.send(result);
                                if cancel_handoff && sent {
                                    cancel_pending_handoff(
                                        &mut pending_promotion,
                                        &mut neighbor_connections,
                                        &mut retiring_simulators,
                                        &mut preconnect_events,
                                        &mut connected_simulators,
                                        &mut simulator_capabilities,
                                    );
                                    handoff_conflicted = false;
                                }
                            } else {
                                let _ = completed.send(Err(Error::SimulatorUnavailable));
                            }
                        }
                        None => {
                            self.disconnect_once(logout_deadline.is_some()).await?;
                            return Ok(());
                        }
                    }
                }
                received = self.socket.recv(&mut receive_buffer), if root_connection_active && (drop_application_messages || pending_events.len() < MAX_PENDING_SIMULATOR_EVENTS) => {
                    let size = match received {
                        Ok(size) => size,
                        Err(error) => {
                            if self
                                .isolate_former_primary_failure(
                                    &mut root_connection_active,
                                    format!("Simulator connection failed: {error}"),
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                )
                                .await?
                            {
                                continue;
                            }
                            return Err(format!("Simulator connection failed: {error}"));
                        }
                    };
                    if *disconnect_requested.borrow() || events.is_closed() {
                        self.disconnect_once(logout_deadline.is_some()).await?;
                        return Ok(());
                    }
                    let Ok(packet) = Packet::decode(&receive_buffer[..size]) else {
                        continue;
                    };
                    last_udp_received = Instant::now();

                    let message = packet.message();
                    if drop_application_messages
                        && !matches!(message, Message::PacketAck(_) | Message::StartPingCheck(_) | Message::LogoutReply(_))
                    {
                        if let Err(error) = self.circuit.receive(&packet) {
                            if self
                                .isolate_former_primary_failure(
                                    &mut root_connection_active,
                                    error.to_string(),
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                )
                                .await?
                            {
                                continue;
                            }
                            self.disconnect_once(logout_deadline.is_some()).await?;
                            return Err(error.to_string());
                        }
                        if events.capacity() == 0
                            && let Err(error) = self.circuit.flush_pending_acks(&self.socket).await
                        {
                            if self
                                .isolate_former_primary_failure(
                                    &mut root_connection_active,
                                    error.clone(),
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                )
                                .await?
                            {
                                continue;
                            }
                            return Err(error);
                        }
                        continue;
                    }

                    let receive_outcome = match self.circuit.receive(&packet) {
                        Ok(receive_outcome) => receive_outcome,
                        Err(error) => {
                            if self
                                .isolate_former_primary_failure(
                                    &mut root_connection_active,
                                    error.to_string(),
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                )
                                .await?
                            {
                                continue;
                            }
                            self.disconnect_once(logout_deadline.is_some()).await?;
                            return Err(error.to_string());
                        }
                    };
                    let is_new_packet = receive_outcome.is_new_packet;
                    if events.capacity() == 0
                        && let Err(error) = self.circuit.flush_pending_acks(&self.socket).await
                    {
                        if self
                            .isolate_former_primary_failure(
                                &mut root_connection_active,
                                error.clone(),
                                &events,
                                &mut pending_events,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            )
                            .await?
                        {
                            continue;
                        }
                        return Err(error);
                    }
                    if logout_deadline.is_none()
                        && !*disconnect_requested.borrow()
                        && !agent_movement_sent
                        && !self.circuit.has_pending(self.use_circuit_sequence)
                    {
                        tracing::info!("Simulator circuit established");
                        self.status.set_circuit_established(true);
                        if !primary_announced {
                            self.publish_simulator_connected(
                                self.simulator_id,
                                Event::SimulatorConnected {
                                    simulator_id: self.simulator_id,
                                    region_handle: None,
                                    endpoint: self.session.simulator_address,
                                },
                                &events,
                                &mut pending_events,
                                &mut preconnect_events,
                                &mut connected_simulators,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            ).await?;
                            primary_announced = true;
                        }
                        if session_ready && !session_ready_published {
                            self.publish_session_ready(
                                &events,
                                &mut pending_events,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            ).await?;
                            session_ready_published = true;
                        }
                        if let Some(capture) = &self.capture {
                            capture.circuit_lifecycle("use_circuit_code_acked", self.session.circuit_code);
                        }
                        self.circuit.send(
                            &self.socket,
                            Message::CompleteAgentMovement(CompleteAgentMovement {
                                agent_data: complete_agent_movement::AgentData {
                                    agent_id: self.session.agent_id,
                                    session_id: self.session.session_id,
                                    circuit_code: self.session.circuit_code,
                                },
                            }),
                            Delivery::Reliable,
                        ).await?;
                        agent_movement_sent = true;
                    }
                    if !is_new_packet {
                        continue;
                    }
                    if is_handoff_message(message) {
                        let mut setup = PromotionSetup {
                            neighbors: &mut neighbors,
                            neighbor_connections: &mut neighbor_connections,
                            retiring_simulators: &mut retiring_simulators,
                            neighbor_task_owners: &mut neighbor_task_owners,
                            preconnect_events: &mut preconnect_events,
                            connected_simulators: &mut connected_simulators,
                            simulator_capabilities: &mut simulator_capabilities,
                            neighbor_requests: neighbor_requests.clone(),
                            application_events: events.clone(),
                            neighbor_events: neighbor_events.clone(),
                            shutdown: disconnect_requested.clone(),
                            pending_events: &mut pending_events,
                            drop_application_messages: &mut drop_application_messages,
                            logout_deadline: &mut logout_deadline,
                            disconnect_reason: &mut disconnect_reason,
                            capability_events: capability_events.clone(),
                            neighbor_event_queues: &mut neighbor_event_queues,
                            neighbor_event_queue_owners: &mut neighbor_event_queue_owners,
                        };
                        self.handle_handoff_message(
                            self.simulator_id,
                            message,
                            receive_outcome.packet_order,
                            &mut setup,
                            &mut pending_promotion,
                            &mut handoff_conflicted,
                        )
                        .await?;
                    }
                    let mut dispatch_message = true;
                    match message {
                        Message::EnableSimulator(message) => {
                            let info = &message.simulator_info;
                            let endpoint = SocketAddr::from((info.ip, info.port));
                            Box::pin(self.start_neighbor(
                                self.simulator_id,
                                info.handle,
                                endpoint,
                                None,
                                &mut neighbors,
                                &mut neighbor_connections,
                                &mut retiring_simulators,
                                &mut neighbor_task_owners,
                                &mut preconnect_events,
                                &mut simulator_capabilities,
                                neighbor_requests.clone(),
                                events.clone(),
                                neighbor_events.clone(),
                                disconnect_requested.clone(),
                                &mut pending_events,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            )).await?;
                        }
                        Message::DisableSimulator(_) => {
                            if let Err(error) = self.circuit.flush_pending_acks(&self.socket).await {
                                if self
                                    .isolate_former_primary_failure(
                                        &mut root_connection_active,
                                        error.clone(),
                                        &events,
                                        &mut pending_events,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    )
                                    .await?
                                {
                                    continue;
                                }
                                return Err(error);
                            }
                            if let Some(capture) = &self.capture {
                                capture.circuit_lifecycle("disconnected", self.session.circuit_code);
                            }
                            if self.active_primary_id != self.simulator_id {
                                tracing::info!("Former primary simulator disconnected");
                                root_connection_active = false;
                                self.root_connection_failed = true;
                                self.publish_event(
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    Event::SimulatorDisconnected {
                                        simulator_id: self.simulator_id,
                                        region_handle: None,
                                        endpoint: self.session.simulator_address,
                                    },
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                )
                                .await?;
                                continue;
                            }
                            tracing::info!("Simulator disconnected the session");
                            return finish_logout(disconnect_reason);
                        }
                        Message::RegionHandshake(_) => {
                            saw_region_handshake = true;
                            if let Err(error) = self.circuit.send(
                                &self.socket,
                                Message::RegionHandshakeReply(RegionHandshakeReply {
                                    agent_data: region_handshake_reply::AgentData {
                                        agent_id: self.session.agent_id,
                                        session_id: self.session.session_id,
                                    },
                                    region_info: region_handshake_reply::RegionInfo { flags: 0 },
                                }),
                                Delivery::Reliable,
                            ).await {
                                if self
                                    .isolate_former_primary_failure(
                                        &mut root_connection_active,
                                        error.clone(),
                                        &events,
                                        &mut pending_events,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    )
                                    .await?
                                {
                                    continue;
                                }
                                return Err(error);
                            }

                            if saw_movement_complete && !session_ready {
                                tracing::info!("Simulator session ready");
                                session_ready = true;
                                self.status.set_lifecycle(NetworkLifecycle::Connected);
                                if let Some(capture) = &self.capture {
                                    capture.circuit_lifecycle("session_ready", self.session.circuit_code);
                                }
                                if primary_announced {
                                    self.publish_session_ready(
                                        &events,
                                        &mut pending_events,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    ).await?;
                                    session_ready_published = true;
                                }
                            }
                        }
                        Message::AgentMovementComplete(message)
                            if message.agent_data.agent_id == self.session.agent_id
                                && message.agent_data.session_id == self.session.session_id =>
                        {
                            saw_movement_complete = true;
                            if saw_region_handshake && !session_ready {
                                tracing::info!("Simulator session ready");
                                session_ready = true;
                                self.status.set_lifecycle(NetworkLifecycle::Connected);
                                if let Some(capture) = &self.capture {
                                    capture.circuit_lifecycle("session_ready", self.session.circuit_code);
                                }
                                if primary_announced {
                                    self.publish_session_ready(
                                        &events,
                                        &mut pending_events,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    ).await?;
                                    session_ready_published = true;
                                }
                            }
                        }
                        Message::StartPingCheck(message) => {
                            dispatch_message = false;
                            self.circuit.clear_duplicate_list(message.oldest_unacked, Instant::now());
                            if let Err(error) = self.circuit.send(
                                &self.socket,
                                Message::CompletePingCheck(CompletePingCheck {
                                    ping_id: PingId { ping_id: message.ping_id.ping_id },
                                }),
                                Delivery::Unreliable,
                            ).await {
                                if self
                                    .isolate_former_primary_failure(
                                        &mut root_connection_active,
                                        error.clone(),
                                        &events,
                                        &mut pending_events,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    )
                                    .await?
                                {
                                    continue;
                                }
                                return Err(error);
                            }
                        }
                        Message::CompletePingCheck(message) => {
                            dispatch_message = false;
                            self.circuit.record_ping_response(message.ping_id.ping_id, Instant::now());
                        }
                        Message::LogoutReply(message)
                            if logout_deadline.is_some()
                                && message.agent_data.agent_id == self.session.agent_id
                                && message.agent_data.session_id == self.session.session_id =>
                        {
                            if let Err(error) = self.circuit.flush_pending_acks(&self.socket).await {
                                if self
                                    .isolate_former_primary_failure(
                                        &mut root_connection_active,
                                        error.clone(),
                                        &events,
                                        &mut pending_events,
                                        &mut drop_application_messages,
                                        &mut logout_deadline,
                                        &mut disconnect_reason,
                                    )
                                    .await?
                                {
                                    continue;
                                }
                                return Err(error);
                            }
                            if let Some(capture) = &self.capture {
                                capture.circuit_lifecycle("disconnected", self.session.circuit_code);
                            }
                            return finish_logout(disconnect_reason);
                        }
                        Message::PacketAck(_) => dispatch_message = false,
                        _ => {}
                    }
                    if dispatch_message {
                        self.publish_simulator_event(
                            self.simulator_id,
                            EventEnvelope::new(Event::SimulatorMessage {
                                simulator_id: self.simulator_id,
                                source: self.session.simulator_address,
                                sequence: packet.sequence(),
                                packet_order: receive_outcome.packet_order,
                                message: packet.into_message(),
                            }),
                            &events,
                            &mut pending_events,
                            &mut preconnect_events,
                            &mut connected_simulators,
                            &mut neighbor_connections,
                            &mut simulator_capabilities,
                            &mut drop_application_messages,
                            &mut logout_deadline,
                            &mut disconnect_reason,
                        ).await?;
                    }
                }
                _ = retry_timer.tick(), if root_connection_active => {
                    let retransmit = tokio::select! {
                        biased;
                        _ = disconnect_requested.changed() => None,
                        _ = events.closed(), if logout_deadline.is_none() => None,
                        result = self.circuit.retransmit_due(&self.socket) => Some(result),
                    };
                    let Some(retransmit) = retransmit else {
                        self.disconnect_once(logout_deadline.is_some()).await?;
                        return Ok(());
                    };
                    if let Err(error) = retransmit {
                        if self
                            .isolate_former_primary_failure(
                                &mut root_connection_active,
                                error.clone(),
                                &events,
                                &mut pending_events,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            )
                            .await?
                        {
                            continue;
                        }
                        if logout_deadline.is_some() {
                            return finish_logout(disconnect_reason);
                        }
                        return Err(error);
                    }
                    let ack_flush = tokio::select! {
                        biased;
                        _ = disconnect_requested.changed() => None,
                        _ = events.closed(), if logout_deadline.is_none() => None,
                        result = self.circuit.flush_pending_acks(&self.socket) => Some(result),
                    };
                    let Some(ack_flush) = ack_flush else {
                        self.disconnect_once(logout_deadline.is_some()).await?;
                        return Ok(());
                    };
                    if let Err(error) = ack_flush {
                        if self
                            .isolate_former_primary_failure(
                                &mut root_connection_active,
                                error.clone(),
                                &events,
                                &mut pending_events,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            )
                            .await?
                        {
                            continue;
                        }
                        return Err(error);
                    }
                }
                _ = ping_timer.tick(), if root_connection_active && agent_movement_sent && logout_deadline.is_none() && !*disconnect_requested.borrow() => {
                    if let Err(error) = self.circuit.send_ping(&self.socket, Instant::now()).await {
                        if self
                            .isolate_former_primary_failure(
                                &mut root_connection_active,
                                error.clone(),
                                &events,
                                &mut pending_events,
                                &mut drop_application_messages,
                                &mut logout_deadline,
                                &mut disconnect_reason,
                            )
                            .await?
                        {
                            continue;
                        }
                        return Err(error);
                    }
                }
            }
        }
    }

    async fn isolate_former_primary_failure(
        &mut self,
        root_connection_active: &mut bool,
        error: String,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        drop_application_messages: &mut bool,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<bool, String> {
        if self.root_connection_failed {
            *root_connection_active = false;
            return Ok(true);
        }
        if self.active_primary_id == self.simulator_id {
            return Ok(false);
        }

        *root_connection_active = false;
        self.root_connection_failed = true;
        let reason = format!("The former primary simulator connection failed: {error}");
        self.status.record_error(&reason);
        self.publish_event(
            events,
            pending_events,
            drop_application_messages,
            Event::SimulatorFailed {
                simulator_id: self.simulator_id,
                region_handle: None,
                endpoint: self.session.simulator_address,
                reason,
            },
            logout_deadline,
            disconnect_reason,
        )
        .await?;
        Ok(true)
    }

    async fn publish_event(
        &mut self,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        drop_application_messages: &mut bool,
        event: impl Into<EventEnvelope>,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        let should_logout = match queue_simulator_event(events, pending_events, event.into()) {
            EventQueueOutcome::Queued => pending_events.len() == MAX_PENDING_SIMULATOR_EVENTS,
            EventQueueOutcome::ReceiverClosed => {
                pending_events.clear();
                true
            }
            EventQueueOutcome::BacklogFull => true,
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

        if should_logout {
            *drop_application_messages = true;
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
            send_logout(&self.socket, &mut self.circuit, &self.session).await?;
            *logout_deadline = Some(Instant::now() + LOGOUT_TIMEOUT);
        }
        Ok(())
    }

    async fn fail_preconnect_overflow(
        &mut self,
        simulator_id: SimulatorId,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        preconnect_events: &mut PreconnectEvents,
        connected_simulators: &mut HashSet<SimulatorId>,
        neighbor_connections: &mut HashMap<u64, NeighborConnectionState>,
        simulator_capabilities: &mut HashMap<SocketAddr, SimulatorCapabilitySession>,
        drop_application_messages: &mut bool,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        preconnect_events.remove(simulator_id);
        connected_simulators.remove(&simulator_id);
        let error = format!("A simulator connection exceeded the {MAX_PRECONNECT_EVENTS}-event pre-connect buffer.");
        self.status.record_error(&error);
        if simulator_id == self.simulator_id {
            return Err(error);
        }

        let region_handle = neighbor_connections
            .iter()
            .find_map(|(region_handle, connection)| (connection.simulator_id == simulator_id).then_some(*region_handle));
        let Some(region_handle) = region_handle else {
            return Ok(());
        };
        let Some(connection) = neighbor_connections.remove(&region_handle) else {
            return Ok(());
        };
        connection.abort.abort();
        remove_simulator_capability(simulator_capabilities, connection.endpoint, simulator_id);
        self.publish_event(
            events,
            pending_events,
            drop_application_messages,
            Event::SimulatorFailed { simulator_id, region_handle: Some(region_handle), endpoint: connection.endpoint, reason: error },
            logout_deadline,
            disconnect_reason,
        )
        .await
    }

    async fn publish_simulator_event(
        &mut self,
        simulator_id: SimulatorId,
        event: EventEnvelope,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        preconnect_events: &mut PreconnectEvents,
        connected_simulators: &mut HashSet<SimulatorId>,
        neighbor_connections: &mut HashMap<u64, NeighborConnectionState>,
        simulator_capabilities: &mut HashMap<SocketAddr, SimulatorCapabilitySession>,
        drop_application_messages: &mut bool,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        match queue_until_connected(connected_simulators, preconnect_events, simulator_id, event) {
            PreconnectOutcome::Ready(event) => {
                self.publish_event(events, pending_events, drop_application_messages, event, logout_deadline, disconnect_reason).await
            }
            PreconnectOutcome::Buffered => Ok(()),
            PreconnectOutcome::Full => {
                self.fail_preconnect_overflow(
                    simulator_id,
                    events,
                    pending_events,
                    preconnect_events,
                    connected_simulators,
                    neighbor_connections,
                    simulator_capabilities,
                    drop_application_messages,
                    logout_deadline,
                    disconnect_reason,
                )
                .await
            }
        }
    }

    async fn publish_simulator_connected(
        &mut self,
        simulator_id: SimulatorId,
        event: Event,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        preconnect_events: &mut PreconnectEvents,
        connected_simulators: &mut HashSet<SimulatorId>,
        drop_application_messages: &mut bool,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        self.publish_event(events, pending_events, drop_application_messages, event, logout_deadline, disconnect_reason).await?;
        connected_simulators.insert(simulator_id);
        for event in preconnect_events.take(simulator_id) {
            self.publish_event(events, pending_events, drop_application_messages, event, logout_deadline, disconnect_reason).await?;
        }
        Ok(())
    }

    async fn publish_session_ready(
        &mut self,
        events: &mpsc::Sender<EventEnvelope>,
        pending_events: &mut VecDeque<EventEnvelope>,
        drop_application_messages: &mut bool,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        self.publish_event(
            events,
            pending_events,
            drop_application_messages,
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
        neighbors: &mut JoinSet<(SimulatorId, u64, SocketAddr, NeighborTaskOutcome)>,
        connections: &mut HashMap<u64, NeighborConnectionState>,
        retiring_simulators: &mut HashSet<SimulatorId>,
        neighbor_task_owners: &mut HashMap<Id, (SimulatorId, u64, SocketAddr)>,
        preconnect_events: &mut PreconnectEvents,
        simulator_capabilities: &mut HashMap<SocketAddr, SimulatorCapabilitySession>,
        neighbor_requests: mpsc::Sender<NeighborRequest>,
        application_events: mpsc::Sender<EventEnvelope>,
        neighbor_events: mpsc::Sender<NeighborEvent>,
        shutdown: watch::Receiver<bool>,
        pending_events: &mut VecDeque<EventEnvelope>,
        drop_application_messages: &mut bool,
        logout_deadline: &mut Option<Instant>,
        disconnect_reason: &mut Option<&'static str>,
    ) -> Result<(), String> {
        if origin != self.active_primary_id || !is_current_simulator(origin, self.simulator_id, connections) {
            return Ok(());
        }
        if handoff.as_ref().is_some_and(|target| target.region_handle != region_handle || target.endpoint != endpoint) {
            self.status.record_error("Ignored a handoff target that did not match its simulator announcement.");
            return Ok(());
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
                drop_application_messages,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(());
        }
        if let Some(current) = connections.get(&region_handle)
            && current.endpoint == endpoint
        {
            if let Some(handoff) = &handoff {
                if current.commands.send(NeighborCommand::PrepareHandoff(handoff.clone())).await.is_ok() {
                    return Ok(());
                }
            } else {
                return Ok(());
            }
        }
        if (endpoint == self.session.simulator_address && handoff.is_none())
            || connections.iter().any(|(active_region, active)| *active_region != region_handle && active.endpoint == endpoint)
        {
            self.status.record_error("The simulator announced an endpoint that is already in use.");
            self.publish_event(
                &application_events,
                pending_events,
                drop_application_messages,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(());
        }
        if neighbors.len() >= MAX_NEIGHBOR_CONNECTION_TASKS {
            self.status.record_error("The neighbor connection task limit has been reached.");
            self.publish_event(
                &application_events,
                pending_events,
                drop_application_messages,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(());
        }
        if let Some(current) = connections.remove(&region_handle) {
            retiring_simulators.insert(current.simulator_id);
            preconnect_events.remove(current.simulator_id);
            remove_simulator_capability(simulator_capabilities, current.endpoint, current.simulator_id);
            current.abort.abort();
        }
        if connections.len() >= MAX_NEIGHBOR_SIMULATORS {
            self.status.record_error("The simulator announced more neighbor regions than the client can track.");
            self.publish_event(
                &application_events,
                pending_events,
                drop_application_messages,
                Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint },
                logout_deadline,
                disconnect_reason,
            )
            .await?;
            return Ok(());
        }

        let mut session = self.session.clone();
        session.simulator_address = endpoint;
        if let Some(handoff) = &handoff {
            session.seed_capability = handoff.seed_capability.clone();
        }
        let capture = self.capture.clone();
        let status = self.status.clone();
        let (command_sender, command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
        if let Some(handoff) = handoff {
            command_sender.try_send(NeighborCommand::PrepareHandoff(handoff)).expect("new neighbor command queue has capacity");
        }
        let finish_events = neighbor_events.clone();
        let task = neighbors.spawn(async move {
            let outcome = match NeighborConnection::connect(simulator_id, session, region_handle, capture, status).await {
                Ok(connection) => {
                    NeighborTaskOutcome::Finished(connection.run(neighbor_events, shutdown, neighbor_requests, command_receiver).await)
                }
                Err(error) => NeighborTaskOutcome::ConnectFailed(error),
            };
            let _ = finish_events.send(NeighborEvent::Finished { simulator_id, region_handle, endpoint, outcome: outcome.clone() }).await;
            (simulator_id, region_handle, endpoint, outcome)
        });
        neighbor_task_owners.insert(task.id(), (simulator_id, region_handle, endpoint));
        connections.insert(region_handle, NeighborConnectionState { simulator_id, endpoint, abort: task, commands: command_sender });
        Ok(())
    }

    fn ensure_handoff_capabilities(
        &self,
        simulator_id: SimulatorId,
        handoff: &HandoffTarget,
        capability_events: mpsc::Sender<EventEnvelope>,
        shutdown: watch::Receiver<bool>,
        neighbor_event_queues: &mut JoinSet<(SimulatorId, SocketAddr, EventQueueExit)>,
        neighbor_event_queue_owners: &mut HashMap<Id, (SimulatorId, SocketAddr)>,
        simulator_capabilities: &mut HashMap<SocketAddr, SimulatorCapabilitySession>,
    ) -> Result<Client, String> {
        if let Some(capability) = simulator_capabilities.get(&handoff.endpoint)
            && capability.simulator_id == simulator_id
            && capability.seed_capability == handoff.seed_capability
        {
            return Ok(capability.client.clone());
        }
        if let Some(capability) = simulator_capabilities.remove(&handoff.endpoint) {
            capability.client.close_session();
        }
        let client = Client::new_with_status_and_budget(
            handoff.seed_capability.clone(),
            self.capture.clone(),
            self.status.clone(),
            self.capability_budget.clone(),
        )?;
        let event_queue = client.start_event_queue(simulator_id, handoff.endpoint, capability_events, shutdown)?;
        let endpoint = handoff.endpoint;
        let task = neighbor_event_queues.spawn(async move {
            let exit = event_queue.wait().await;
            (simulator_id, endpoint, exit)
        });
        neighbor_event_queue_owners.insert(task.id(), (simulator_id, endpoint));
        simulator_capabilities.insert(
            endpoint,
            SimulatorCapabilitySession { simulator_id, seed_capability: handoff.seed_capability.clone(), client: client.clone() },
        );
        Ok(client)
    }

    async fn request_handoff(
        &mut self,
        source: SimulatorId,
        handoff: HandoffTarget,
        announcement: HandoffAnnouncement,
        setup: &mut PromotionSetup<'_>,
        pending: &mut Option<PendingPromotion>,
        conflicted: &mut bool,
    ) -> Result<(), String> {
        if source != self.active_primary_id || *conflicted {
            return Ok(());
        }
        if self.active_primary_region_handle == Some(handoff.region_handle) && self.active_primary_endpoint == handoff.endpoint {
            return Ok(());
        }

        let mut evidence = HandoffEvidence::default();
        if let Some(current) = pending.as_mut() {
            match arbitrate_handoff(current, &handoff, announcement) {
                HandoffDecision::SameDestination => {
                    current.evidence.record(announcement);
                    let same_live_target = current.source == source
                        && setup.neighbor_connections.get(&current.handoff.region_handle).is_some_and(|connection| {
                            connection.simulator_id == current.target && connection.endpoint == current.handoff.endpoint
                        });
                    if same_live_target {
                        return Ok(());
                    }
                    evidence = current.evidence;
                    cancel_pending_handoff(
                        pending,
                        setup.neighbor_connections,
                        setup.retiring_simulators,
                        setup.preconnect_events,
                        setup.connected_simulators,
                        setup.simulator_capabilities,
                    );
                }
                HandoffDecision::ReplaceNewer => {
                    cancel_pending_handoff(
                        pending,
                        setup.neighbor_connections,
                        setup.retiring_simulators,
                        setup.preconnect_events,
                        setup.connected_simulators,
                        setup.simulator_capabilities,
                    );
                }
                HandoffDecision::IgnoreStale => return Ok(()),
                HandoffDecision::Conflict => {
                    cancel_pending_handoff(
                        pending,
                        setup.neighbor_connections,
                        setup.retiring_simulators,
                        setup.preconnect_events,
                        setup.connected_simulators,
                        setup.simulator_capabilities,
                    );
                    *conflicted = true;
                    self.status.record_error("Conflicting simulator handoff announcements; waiting for TeleportCancel.");
                    return Ok(());
                }
            }
        }
        evidence.record(announcement);

        let source = self.active_primary_id;
        self.start_neighbor(
            source,
            handoff.region_handle,
            handoff.endpoint,
            Some(handoff.clone()),
            setup.neighbors,
            setup.neighbor_connections,
            setup.retiring_simulators,
            setup.neighbor_task_owners,
            setup.preconnect_events,
            setup.simulator_capabilities,
            setup.neighbor_requests.clone(),
            setup.application_events.clone(),
            setup.neighbor_events.clone(),
            setup.shutdown.clone(),
            setup.pending_events,
            setup.drop_application_messages,
            setup.logout_deadline,
            setup.disconnect_reason,
        )
        .await?;

        let Some(connection) =
            setup.neighbor_connections.get(&handoff.region_handle).filter(|connection| connection.endpoint == handoff.endpoint)
        else {
            return Ok(());
        };
        let simulator_id = connection.simulator_id;
        if let Err(error) = self.ensure_handoff_capabilities(
            simulator_id,
            &handoff,
            setup.capability_events.clone(),
            setup.shutdown.clone(),
            setup.neighbor_event_queues,
            setup.neighbor_event_queue_owners,
            setup.simulator_capabilities,
        ) {
            self.status.record_error(format!("Could not start destination simulator capabilities: {error}"));
            if let Some(connection) = setup.neighbor_connections.get(&handoff.region_handle) {
                let _ = connection.commands.try_send(NeighborCommand::CancelHandoff);
            }
            return Ok(());
        }
        *pending = Some(PendingPromotion { source, target: simulator_id, handoff, evidence });
        Ok(())
    }

    async fn handle_handoff_message(
        &mut self,
        source: SimulatorId,
        message: &Message,
        packet_order: PacketOrder,
        setup: &mut PromotionSetup<'_>,
        pending: &mut Option<PendingPromotion>,
        conflicted: &mut bool,
    ) -> Result<(), String> {
        if source != self.active_primary_id || *conflicted {
            return Ok(());
        }
        let Message::Schema(schema) = message else { return Ok(()) };
        match schema.schema().name {
            "TeleportFinish" => match decode_teleport_finish(message, self.session.agent_id, self.session.session_id) {
                Ok(handoff) => {
                    self.request_handoff(source, handoff, HandoffAnnouncement::TeleportFinish(packet_order), setup, pending, conflicted)
                        .await
                }
                Err(reason) => {
                    self.status.record_error(format!("Ignored malformed TeleportFinish message: {reason}"));
                    Ok(())
                }
            },
            "TeleportFailed" => match schema_field(schema, "AgentID") {
                Some(FieldValue::UUID(agent_id)) if *agent_id == self.session.agent_id => {
                    if teleport_failure_matches(pending.as_ref(), packet_order) {
                        cancel_pending_handoff(
                            pending,
                            setup.neighbor_connections,
                            setup.retiring_simulators,
                            setup.preconnect_events,
                            setup.connected_simulators,
                            setup.simulator_capabilities,
                        );
                    }
                    Ok(())
                }
                Some(FieldValue::UUID(_)) => {
                    self.status.record_error("Ignored TeleportFailed for a different agent.");
                    Ok(())
                }
                _ => {
                    self.status.record_error("Ignored TeleportFailed without a valid agent ID.");
                    Ok(())
                }
            },
            _ => Ok(()),
        }
    }

    async fn disconnect_once(&mut self, logout_already_sent: bool) -> Result<(), String> {
        tracing::info!(logout_already_sent, "Simulator disconnect started");
        self.status.set_lifecycle(NetworkLifecycle::Disconnecting);
        if let Some(capture) = &self.capture {
            capture.circuit_lifecycle("disconnect_started", self.session.circuit_code);
        }
        if !logout_already_sent && let Some(commands) = &self.active_primary_commands {
            let (completed, response) = oneshot::channel();
            commands
                .send(NeighborCommand::Logout(completed))
                .await
                .map_err(|_| "The active simulator connection is closed.".to_string())?;
            response
                .await
                .map_err(|_| "The active simulator connection ended before logout was sent.".to_string())?
                .map_err(|error| error.to_string())?;
        } else if !logout_already_sent && !self.root_connection_failed {
            send_logout_once(&self.socket, &mut self.circuit, &self.session).await?;
        }
        Ok(())
    }

    pub(crate) async fn handle_primary_event_queue_exit(&mut self, exit: EventQueueExit, logout_already_sent: bool) -> Result<(), String> {
        let reason = match exit {
            EventQueueExit::Stopped => return Ok(()),
            EventQueueExit::PollCanceled => return Ok(()),
            EventQueueExit::Unavailable => "The primary simulator does not have a usable EventQueueGet capability.",
            EventQueueExit::RetryLimitReached => "The primary simulator EventQueueGet stopped after repeated errors.",
            EventQueueExit::TaskFailed => "The primary simulator EventQueueGet task ended unexpectedly.",
        };

        if let Some(capture) = &self.capture {
            capture.lifecycle("network_failed");
        }
        self.disconnect_once(logout_already_sent).await?;
        Err(reason.into())
    }
}

fn decode_enable_simulators(body: &Value) -> Result<Vec<(u64, SocketAddr)>, &'static str> {
    let Value::Map(body) = body else {
        return Err("event body is not an LLSD map");
    };
    let Some(Value::Array(simulators)) = body.get("SimulatorInfo") else {
        return Err("SimulatorInfo is not an LLSD array");
    };
    if simulators.is_empty() || simulators.len() > MAX_SIMULATORS_PER_ENABLE_EVENT {
        return Err("SimulatorInfo has an invalid number of entries");
    }

    simulators
        .iter()
        .map(|simulator| {
            let Value::Map(simulator) = simulator else {
                return Err("SimulatorInfo entry is not an LLSD map");
            };
            let Some(Value::Binary(handle)) = simulator.get("Handle") else {
                return Err("Handle is not LLSD binary data");
            };
            let handle: [u8; 8] = handle.as_slice().try_into().map_err(|_| "Handle is not 8 bytes")?;
            let Some(Value::Binary(ip)) = simulator.get("IP") else {
                return Err("IP is not LLSD binary data");
            };
            let ip: [u8; 4] = ip.as_slice().try_into().map_err(|_| "IP is not 4 bytes")?;
            let Some(Value::Integer(port)) = simulator.get("Port") else {
                return Err("Port is not an LLSD integer");
            };
            let port = u16::try_from(*port).ok().filter(|port| *port != 0).ok_or("Port is outside the valid range")?;
            let endpoint = SocketAddr::from((Ipv4Addr::from(ip), port));
            Ok((u64::from_be_bytes(handle), endpoint))
        })
        .collect()
}

fn decode_crossed_region(body: &Value, agent_id: UUID, session_id: UUID) -> Result<HandoffTarget, &'static str> {
    let Value::Map(body) = body else {
        return Err("event body is not an LLSD map");
    };
    let Some(Value::Map(agent)) = body.get("AgentData") else {
        return Err("AgentData is not an LLSD map");
    };
    if !matches!(agent.get("AgentID"), Some(Value::UUID(value)) if *value == agent_id) {
        return Err("AgentData.AgentID does not match the authenticated agent");
    }
    if !matches!(agent.get("SessionID"), Some(Value::UUID(value)) if *value == session_id) {
        return Err("AgentData.SessionID does not match the authenticated session");
    }
    let Some(Value::Map(region)) = body.get("RegionData") else {
        return Err("RegionData is not an LLSD map");
    };
    let Some(Value::Binary(raw_handle)) = region.get("RegionHandle") else {
        return Err("RegionData.RegionHandle is not LLSD binary data");
    };
    let raw_handle: [u8; 8] = raw_handle.as_slice().try_into().map_err(|_| "RegionData.RegionHandle is not 8 bytes")?;
    let Some(Value::Binary(raw_ip)) = region.get("SimIP") else {
        return Err("RegionData.SimIP is not LLSD binary data");
    };
    let raw_ip: [u8; 4] = raw_ip.as_slice().try_into().map_err(|_| "RegionData.SimIP is not 4 bytes")?;
    let port = match region.get("SimPort") {
        Some(Value::Integer(port)) => u16::try_from(*port).ok(),
        Some(Value::Binary(port)) => port.as_slice().try_into().ok().map(u16::from_be_bytes),
        _ => None,
    }
    .filter(|port| *port != 0)
    .ok_or("RegionData.SimPort is outside the valid range")?;
    let seed = match region.get("SeedCapability") {
        Some(Value::String(seed)) => seed.as_str(),
        Some(Value::URI(seed)) => seed.as_str(),
        _ => return Err("RegionData.SeedCapability is not an LLSD URI or string"),
    };
    make_handoff_target(agent_id, session_id, u64::from_be_bytes(raw_handle), SocketAddr::from((Ipv4Addr::from(raw_ip), port)), seed)
}

fn decode_teleport_finish(message: &Message, agent_id: UUID, session_id: UUID) -> Result<HandoffTarget, &'static str> {
    let Message::Schema(schema) = message else {
        return Err("message is not a schema message");
    };
    if schema.schema().name != "TeleportFinish" {
        return Err("message is not TeleportFinish");
    }
    if !matches!(schema_field(schema, "AgentID"), Some(FieldValue::UUID(value)) if *value == agent_id) {
        return Err("Info.AgentID does not match the authenticated agent");
    }
    let Some(FieldValue::IpAddr(ip)) = schema_field(schema, "SimIP") else {
        return Err("Info.SimIP is not an IPv4 address");
    };
    let Some(FieldValue::IpPort(port)) = schema_field(schema, "SimPort") else {
        return Err("Info.SimPort is not a port");
    };
    let Some(FieldValue::U64(region_handle)) = schema_field(schema, "RegionHandle") else {
        return Err("Info.RegionHandle is not an unsigned 64-bit value");
    };
    let Some(FieldValue::Bytes(seed)) = schema_field(schema, "SeedCapability") else {
        return Err("Info.SeedCapability is not a byte string");
    };
    let seed = seed.strip_suffix(&[0]).unwrap_or(seed);
    let seed = from_utf8(seed).map_err(|_| "Info.SeedCapability is not UTF-8")?;
    make_handoff_target(agent_id, session_id, *region_handle, SocketAddr::from((Ipv4Addr::from(*ip), u16::from_be_bytes(*port))), seed)
}

fn schema_field<'a>(schema: &'a SchemaMessage, name: &str) -> Option<&'a FieldValue> {
    for (block_schema, block_values) in schema.schema().blocks.iter().zip(schema.blocks()) {
        let instance = block_values.instances.first()?;
        for (field_schema, field_value) in block_schema.fields.iter().zip(&instance.fields) {
            if field_schema.name == name {
                return Some(field_value);
            }
        }
    }
    None
}

fn make_handoff_target(
    agent_id: UUID,
    session_id: UUID,
    region_handle: u64,
    endpoint: SocketAddr,
    seed: &str,
) -> Result<HandoffTarget, &'static str> {
    if endpoint.port() == 0
        || endpoint.ip().is_unspecified()
        || endpoint.ip().is_multicast()
        || endpoint.ip() == IpAddr::V4(Ipv4Addr::BROADCAST)
    {
        return Err("destination endpoint is invalid");
    }
    let seed_capability = Url::parse(seed).map_err(|_| "SeedCapability is not a valid URL")?;
    if !matches!(seed_capability.scheme(), "http" | "https")
        || seed_capability.host().is_none()
        || !seed_capability.username().is_empty()
        || seed_capability.password().is_some()
        || seed_capability.fragment().is_some()
    {
        return Err("SeedCapability is not a valid HTTP(S) URL");
    }
    Ok(HandoffTarget { agent_id, session_id, region_handle, endpoint, seed_capability })
}

async fn send_neighbor_message(commands: &mpsc::Sender<NeighborCommand>, message: Message, delivery: Delivery) -> Result<(), Error> {
    let (completed, response) = oneshot::channel();
    commands.try_send(NeighborCommand::Send { message, delivery, completed }).map_err(|error| match error {
        mpsc::error::TrySendError::Full(_) => Error::SimulatorCommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => Error::SimulatorConnectionClosed,
    })?;
    response.await.map_err(|_| Error::SimulatorConnectionClosed)?
}

fn decode_agent_communication(body: &Value, agent_id: UUID) -> Result<(SocketAddr, Url), &'static str> {
    let Value::Map(body) = body else {
        return Err("event body is not an LLSD map");
    };
    match body.get("agent-id") {
        Some(Value::UUID(value)) if *value == agent_id => {}
        Some(Value::UUID(_)) => return Err("agent-id does not match the logged-in agent"),
        _ => return Err("agent-id is not an LLSD UUID"),
    }
    let Some(Value::String(address)) = body.get("sim-ip-and-port") else {
        return Err("sim-ip-and-port is not an LLSD string");
    };
    let endpoint = address.parse().map_err(|_| "sim-ip-and-port is not a socket address")?;
    let Some(Value::String(seed)) = body.get("seed-capability") else {
        return Err("seed-capability is not an LLSD string");
    };
    let seed = Url::parse(seed).map_err(|_| "seed-capability is not a valid URL")?;
    Ok((endpoint, seed))
}

fn redact_seed_capability(body: &Value) -> Value {
    let Value::Map(fields) = body else {
        return body.clone();
    };
    let mut fields = fields.clone();
    if fields.contains_key("seed-capability") {
        fields.insert("seed-capability".to_owned(), Value::String("[redacted]".to_owned()));
    }
    if let Some(Value::Map(region)) = fields.get("RegionData") {
        let mut region = region.clone();
        if region.contains_key("SeedCapability") {
            region.insert("SeedCapability".to_owned(), Value::String("[redacted]".to_owned()));
            fields.insert("RegionData".to_owned(), Value::Map(region));
        }
    }
    Value::Map(fields)
}

fn is_handoff_message(message: &Message) -> bool {
    matches!(message, Message::Schema(schema) if matches!(schema.schema().name, "TeleportFinish" | "TeleportFailed"))
}

fn is_teleport_cancel(message: &Message, agent_id: UUID, session_id: UUID) -> bool {
    let Message::Schema(schema) = message else { return false };
    schema.schema().name == "TeleportCancel"
        && matches!(schema_field(schema, "AgentID"), Some(FieldValue::UUID(value)) if *value == agent_id)
        && matches!(schema_field(schema, "SessionID"), Some(FieldValue::UUID(value)) if *value == session_id)
}

struct SimulatorCapabilitySession {
    simulator_id: SimulatorId,
    seed_capability: Url,
    client: Client,
}

impl Drop for SimulatorCapabilitySession {
    fn drop(&mut self) {
        self.client.close_session();
    }
}

fn remove_simulator_capability(
    capabilities: &mut HashMap<SocketAddr, SimulatorCapabilitySession>,
    endpoint: SocketAddr,
    simulator_id: SimulatorId,
) {
    if capabilities.get(&endpoint).is_some_and(|current| current.simulator_id == simulator_id) {
        capabilities.remove(&endpoint);
    }
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
    (exit == EventQueueExit::TaskFailed && is_current_simulator(simulator_id, primary_id, connections)).then(|| Event::CapabilityError {
        simulator_id,
        source,
        capability: EVENT_QUEUE_NAME.to_owned(),
        reason: "The neighbor simulator EventQueueGet task ended unexpectedly.".into(),
    })
}

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

struct NeighborConnection {
    simulator_id: SimulatorId,
    session: AgentSession,
    region_handle: u64,
    socket: UdpTransport,
    circuit: Circuit,
    use_circuit_sequence: u32,
    handoff: Option<HandoffTarget>,
    movement_requested: bool,
}

impl NeighborConnection {
    async fn connect(
        simulator_id: SimulatorId,
        session: AgentSession,
        region_handle: u64,
        capture: Option<CaptureRecorder>,
        status: NetworkStatusHandle,
    ) -> Result<Self, String> {
        let socket =
            UdpTransport::connect_with_status(session.simulator_address, session.circuit_code, capture.clone(), status.clone()).await?;
        let mut circuit = Circuit::with_status(status.for_circuit());
        let use_circuit_sequence = circuit
            .send(
                &socket,
                Message::UseCircuitCode(UseCircuitCode {
                    circuit_code: CircuitCode { code: session.circuit_code, session_id: session.session_id, agent_id: session.agent_id },
                }),
                Delivery::Reliable,
            )
            .await?;
        Ok(Self { simulator_id, session, region_handle, socket, circuit, use_circuit_sequence, handoff: None, movement_requested: false })
    }

    #[tracing::instrument(name = "simulator.neighbor", level = "info", skip_all)]
    async fn run(
        mut self,
        events: mpsc::Sender<NeighborEvent>,
        mut shutdown: watch::Receiver<bool>,
        neighbor_requests: mpsc::Sender<NeighborRequest>,
        mut commands: mpsc::Receiver<NeighborCommand>,
    ) -> Result<(), String> {
        let mut receive_buffer = [0u8; 65_535];
        let mut established = false;
        let mut preconnect_messages = VecDeque::with_capacity(MAX_PRECONNECT_EVENTS);
        let mut pending_events = VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS);
        let mut closing = false;
        let mut teardown_deadline = None;
        let mut close_reason = None;
        let mut retry_timer = time::interval(ACK_INTERVAL);
        retry_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut ping_timer = time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
        ping_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut last_udp_received = Instant::now();
        let mut saw_region_handshake = false;

        loop {
            if *shutdown.borrow() || events.is_closed() {
                return finish_logout(close_reason);
            }
            tokio::select! {
                biased;
                _ = shutdown.changed() => return Ok(()),
                _ = events.closed() => return Ok(()),
                _ = time::sleep_until(teardown_deadline.unwrap_or_else(|| Instant::now() + LOGOUT_TIMEOUT)), if teardown_deadline.is_some() => {
                    return finish_logout(close_reason);
                }
                _ = time::sleep_until(last_udp_received + KEEPALIVE_TIMEOUT), if established && !closing => {
                    return Err("The simulator stopped responding to LLUDP keepalive traffic.".into());
                }
                permit = events.reserve(), if !pending_events.is_empty() => {
                    match permit {
                        Ok(permit) => {
                            if let Some(event) = pending_events.pop_front() {
                                permit.send(NeighborEvent::Simulator(event));
                            }
                            if closing && pending_events.is_empty() {
                                return finish_logout(close_reason);
                            }
                        }
                        Err(_) => return Ok(()),
                    }
                }
                _ = retry_timer.tick() => {
                    self.circuit.retransmit_due(&self.socket).await?;
                    self.circuit.flush_pending_acks(&self.socket).await?;
                }
                _ = ping_timer.tick(), if established => {
                    self.circuit.send_ping(&self.socket, Instant::now()).await?;
                }
                command = commands.recv(), if !commands.is_closed() => {
                    match command {
                        Some(NeighborCommand::Send { completed, .. }) if closing => {
                            let _ = completed.send(Err(Error::SimulatorConnectionClosed));
                        }
                        Some(NeighborCommand::Send { completed, .. }) if !established => {
                            let _ = completed.send(Err(Error::SimulatorNotReady));
                        }
                        Some(NeighborCommand::Send { message, delivery, completed }) => {
                            let result = self.circuit
                                .send(&self.socket, message, delivery)
                                .await
                                .map(|_| ())
                                .map_err(Error::SimulatorSend);
                            let failed = result.as_ref().err().map(ToString::to_string);
                            let _ = completed.send(result);
                            if let Some(error) = failed {
                                return Err(error);
                            }
                        }
                        Some(NeighborCommand::PrepareHandoff(target)) => {
                            if target.region_handle != self.region_handle
                                || target.endpoint != self.session.simulator_address
                                || target.agent_id != self.session.agent_id
                                || target.session_id != self.session.session_id
                            {
                                continue;
                            }
                            if self.handoff.as_ref() == Some(&target) {
                                continue;
                            }
                            self.session.seed_capability = target.seed_capability.clone();
                            self.handoff = Some(target);
                            self.movement_requested = false;
                            if established && saw_region_handshake {
                                self.send_handoff_movement().await?;
                            }
                        }
                        Some(NeighborCommand::CancelHandoff) => {
                            self.handoff = None;
                            self.movement_requested = false;
                        }
                        Some(NeighborCommand::Logout(completed)) => {
                            let result = send_logout_once(&self.socket, &mut self.circuit, &self.session)
                                .await
                                .map_err(Error::SimulatorSend);
                            let failed = result.as_ref().err().map(ToString::to_string);
                            let _ = completed.send(result);
                            if let Some(error) = failed {
                                return Err(error);
                            }
                            closing = true;
                            teardown_deadline.get_or_insert_with(|| Instant::now() + LOGOUT_TIMEOUT);
                        }
                        None => return Ok(()),
                    }
                }
                received = self.socket.recv(&mut receive_buffer) => {
                    let size = received.map_err(|error| format!("Neighbor simulator connection failed: {error}"))?;
                    if *shutdown.borrow() || events.is_closed() {
                        return Ok(());
                    }
                    let Ok(packet) = Packet::decode(&receive_buffer[..size]) else {
                        continue;
                    };
                    last_udp_received = Instant::now();
                    let receive_outcome = self.circuit.receive(&packet).map_err(|error| error.to_string())?;
                    let is_new_packet = receive_outcome.is_new_packet;
                    let dispatch_allowed = !closing;
                    if events.capacity() == 0 {
                        self.circuit.flush_pending_acks(&self.socket).await?;
                    }
                    if !established && !self.circuit.has_pending(self.use_circuit_sequence) {
                        established = true;
                        if !queue_neighbor_event(
                            &events,
                            &mut pending_events,
                            Event::SimulatorConnected {
                                simulator_id: self.simulator_id,
                                region_handle: Some(self.region_handle),
                                endpoint: self.session.simulator_address,
                            },
                            &mut closing,
                            &mut teardown_deadline,
                            &mut close_reason,
                        ) {
                            return Ok(());
                        }
                        while let Some(event) = preconnect_messages.pop_front() {
                            if !queue_neighbor_event(
                                &events,
                                &mut pending_events,
                                event,
                                &mut closing,
                                &mut teardown_deadline,
                                &mut close_reason,
                            ) {
                                return Ok(());
                            }
                            if close_reason.is_some() {
                                preconnect_messages.clear();
                                break;
                            }
                        }
                        if saw_region_handshake && self.handoff.is_some() {
                            self.send_handoff_movement().await?;
                        }
                    }
                    if !is_new_packet {
                        continue;
                    }

                    let message = packet.message();
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
                            self.circuit.flush_pending_acks(&self.socket).await?;
                            closing = true;
                            teardown_deadline.get_or_insert_with(|| Instant::now() + LOGOUT_TIMEOUT);
                            dispatch_message = false;
                        }
                        Message::RegionHandshake(_) => {
                            saw_region_handshake = true;
                            self.circuit.send(
                                &self.socket,
                                Message::RegionHandshakeReply(RegionHandshakeReply {
                                    agent_data: region_handshake_reply::AgentData {
                                        agent_id: self.session.agent_id,
                                        session_id: self.session.session_id,
                                    },
                                    region_info: region_handshake_reply::RegionInfo { flags: 0 },
                                }),
                                Delivery::Reliable,
                            ).await?;
                            if established && self.handoff.is_some() {
                                self.send_handoff_movement().await?;
                            }
                        }
                        Message::AgentMovementComplete(message) => {
                            let matches_handoff = self.handoff.as_ref().is_some_and(|handoff| {
                                self.movement_requested
                                    && message.agent_data.agent_id == handoff.agent_id
                                    && message.agent_data.session_id == handoff.session_id
                                    && message.data.region_handle == handoff.region_handle
                            });
                            if !matches_handoff {
                                dispatch_message = false;
                            }
                        }
                        Message::StartPingCheck(message) => {
                            dispatch_message = false;
                            self.circuit.clear_duplicate_list(message.oldest_unacked, Instant::now());
                            self.circuit.send(
                                &self.socket,
                                Message::CompletePingCheck(CompletePingCheck {
                                    ping_id: PingId { ping_id: message.ping_id.ping_id },
                                }),
                                Delivery::Unreliable,
                            ).await?;
                        }
                        Message::CompletePingCheck(message) => {
                            dispatch_message = false;
                            self.circuit.record_ping_response(message.ping_id.ping_id, Instant::now());
                        }
                        Message::PacketAck(_) => dispatch_message = false,
                        _ => {}
                    }

                    if dispatch_message && dispatch_allowed {
                        let event = Event::SimulatorMessage {
                            simulator_id: self.simulator_id,
                            source: self.session.simulator_address,
                            sequence: packet.sequence(),
                            packet_order: receive_outcome.packet_order,
                            message: packet.into_message(),
                        };
                        if established {
                            if !queue_neighbor_event(
                                &events,
                                &mut pending_events,
                                event,
                                &mut closing,
                                &mut teardown_deadline,
                                &mut close_reason,
                            ) {
                                return Ok(());
                            }
                        } else if preconnect_messages.len() < MAX_PRECONNECT_EVENTS {
                            preconnect_messages.push_back(event);
                        } else {
                            close_reason = Some(NEIGHBOR_PRECONNECT_OVERFLOW_REASON);
                            closing = true;
                            teardown_deadline.get_or_insert_with(|| Instant::now() + LOGOUT_TIMEOUT);
                            preconnect_messages.clear();
                        }
                    }
                    if closing && pending_events.is_empty() {
                        self.circuit.flush_pending_acks(&self.socket).await?;
                        return finish_logout(close_reason);
                    }
                }
            }
        }
    }

    async fn send_handoff_movement(&mut self) -> Result<(), String> {
        if self.movement_requested || self.handoff.is_none() {
            return Ok(());
        }
        self.circuit
            .send(
                &self.socket,
                Message::CompleteAgentMovement(CompleteAgentMovement {
                    agent_data: complete_agent_movement::AgentData {
                        agent_id: self.session.agent_id,
                        session_id: self.session.session_id,
                        circuit_code: self.session.circuit_code,
                    },
                }),
                Delivery::Reliable,
            )
            .await?;
        self.movement_requested = true;
        Ok(())
    }
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
    run_session_with_status(session, commands, events, disconnect_requested, NetworkStatusHandle::default()).await
}

#[cfg(test)]
async fn run_session_with_status(
    session: AgentSession,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<EventEnvelope>,
    disconnect_requested: watch::Receiver<bool>,
    status: NetworkStatusHandle,
) -> Result<(), String> {
    let failure_events = events.clone();
    let mut failure_shutdown = disconnect_requested.clone();
    let terminal_event_permit = events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let result = async {
        SimulatorConnection::connect_with_status(session, LoginResponseData::default(), None, status)
            .await?
            .run(commands, events, disconnect_requested, terminal_event_permit)
            .await
    }
    .await;
    if let Err(error) = &result {
        send_failure_event(&failure_events, error.clone(), &mut failure_shutdown).await;
    }
    result
}

enum EventQueueOutcome {
    Queued,
    ReceiverClosed,
    BacklogFull,
}

fn queue_neighbor_event(
    events: &mpsc::Sender<NeighborEvent>,
    pending_events: &mut VecDeque<Event>,
    event: Event,
    closing: &mut bool,
    teardown_deadline: &mut Option<Instant>,
    close_reason: &mut Option<&'static str>,
) -> bool {
    match queue_neighbor_channel_event(events, pending_events, event) {
        EventQueueOutcome::Queued => {
            if pending_events.len() >= MAX_PENDING_SIMULATOR_EVENTS {
                *closing = true;
                *teardown_deadline = Some(Instant::now() + LOGOUT_TIMEOUT);
                *close_reason = Some(EVENT_BACKLOG_DISCONNECT_REASON);
            }
            true
        }
        EventQueueOutcome::ReceiverClosed => false,
        EventQueueOutcome::BacklogFull => {
            *closing = true;
            *teardown_deadline = Some(Instant::now() + LOGOUT_TIMEOUT);
            *close_reason = Some(EVENT_BACKLOG_DISCONNECT_REASON);
            true
        }
    }
}

fn queue_neighbor_channel_event(
    events: &mpsc::Sender<NeighborEvent>,
    pending_events: &mut VecDeque<Event>,
    event: Event,
) -> EventQueueOutcome {
    if events.is_closed() {
        return EventQueueOutcome::ReceiverClosed;
    }

    if !pending_events.is_empty() {
        return if pending_events.len() < MAX_QUEUED_SIMULATOR_EVENTS {
            pending_events.push_back(event);
            EventQueueOutcome::Queued
        } else {
            EventQueueOutcome::BacklogFull
        };
    }

    match events.try_send(NeighborEvent::Simulator(event)) {
        Ok(()) => EventQueueOutcome::Queued,
        Err(mpsc::error::TrySendError::Closed(_)) => EventQueueOutcome::ReceiverClosed,
        Err(mpsc::error::TrySendError::Full(NeighborEvent::Simulator(event))) => {
            pending_events.push_back(event);
            EventQueueOutcome::Queued
        }
        Err(mpsc::error::TrySendError::Full(NeighborEvent::Finished { .. })) => {
            unreachable!("only simulator events enter the neighbor event backlog")
        }
    }
}

fn queue_simulator_event(
    events: &mpsc::Sender<EventEnvelope>,
    pending_events: &mut VecDeque<EventEnvelope>,
    event: EventEnvelope,
) -> EventQueueOutcome {
    if events.is_closed() {
        return EventQueueOutcome::ReceiverClosed;
    }

    if !pending_events.is_empty() {
        return if pending_events.len() < MAX_QUEUED_SIMULATOR_EVENTS {
            pending_events.push_back(event);
            EventQueueOutcome::Queued
        } else {
            EventQueueOutcome::BacklogFull
        };
    }

    match events.try_send(event) {
        Ok(()) => EventQueueOutcome::Queued,
        Err(mpsc::error::TrySendError::Closed(_)) => EventQueueOutcome::ReceiverClosed,
        Err(mpsc::error::TrySendError::Full(event)) => {
            pending_events.push_back(event);
            EventQueueOutcome::Queued
        }
    }
}

fn primary_event_queue_size(events: &mpsc::Sender<EventEnvelope>, pending_events: usize) -> (usize, usize) {
    let reserved_terminal_slots = PRIMARY_TERMINAL_EVENT_CAPACITY;
    let channel_capacity = events.max_capacity().saturating_sub(reserved_terminal_slots);
    let channel_occupancy = events.max_capacity().saturating_sub(events.capacity()).saturating_sub(reserved_terminal_slots);
    (channel_occupancy + pending_events, channel_capacity + MAX_QUEUED_SIMULATOR_EVENTS)
}

async fn send_logout(socket: &UdpTransport, circuit: &mut Circuit, session: &AgentSession) -> Result<(), String> {
    circuit
        .send(
            socket,
            Message::LogoutRequest(LogoutRequest {
                agent_data: logout_request::AgentData { agent_id: session.agent_id, session_id: session.session_id },
            }),
            Delivery::Reliable,
        )
        .await?;
    Ok(())
}

async fn send_logout_once(socket: &UdpTransport, circuit: &mut Circuit, session: &AgentSession) -> Result<(), String> {
    circuit
        .send(
            socket,
            Message::LogoutRequest(LogoutRequest {
                agent_data: logout_request::AgentData { agent_id: session.agent_id, session_id: session.session_id },
            }),
            Delivery::ReliableNoRetry,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_BACKLOG_DISCONNECT_REASON, FinishOrder, HandoffAnnouncement, HandoffDecision, HandoffEvidence, HandoffTarget,
        MAX_SIMULATORS_PER_ENABLE_EVENT, NeighborCommand, NeighborConnectionState, NeighborTaskOutcome, PendingPromotion,
        arbitrate_handoff, decode_agent_communication, decode_crossed_region, decode_enable_simulators, decode_teleport_finish,
        is_teleport_cancel, matches_pending_promotion, neighbor_completion_event, neighbor_event_queue_failure,
        neighbor_event_queue_task_failure, redact_seed_capability, remove_failed_neighbor, same_handoff_destination,
        teleport_failure_matches,
    };
    use crate::{
        capabilities::EventQueueExit,
        event::{
            Event,
            PacketOrder::{Ambiguous, Degraded, Ordered},
            SimulatorId,
        },
    };
    use base::UUID;
    use llsd::{Map, Value};
    use protocol::{
        BlockValues, FieldValue, SchemaMessage,
        lludp::Message,
        messages::{
            AgentMovementComplete,
            agent_movement_complete::{AgentData, Data, SimData},
        },
    };
    use std::{collections::HashMap, future::pending, net::SocketAddr};
    use tokio::{sync::mpsc, task::JoinSet};
    use url::Url;

    fn simulator(handle: Vec<u8>, ip: Vec<u8>, port: i32) -> Value {
        Value::Map(Map::from([
            ("Handle".to_owned(), Value::Binary(handle.into())),
            ("IP".to_owned(), Value::Binary(ip.into())),
            ("Port".to_owned(), Value::Integer(port)),
        ]))
    }

    fn event(entries: Vec<Value>) -> Value {
        Value::Map(Map::from([("SimulatorInfo".to_owned(), Value::Array(entries))]))
    }

    fn crossed_region(agent_id: UUID, session_id: UUID) -> Value {
        Value::Map(Map::from([
            (
                "AgentData".to_owned(),
                Value::Map(Map::from([("AgentID".to_owned(), Value::UUID(agent_id)), ("SessionID".to_owned(), Value::UUID(session_id))])),
            ),
            (
                "RegionData".to_owned(),
                Value::Map(Map::from([
                    ("RegionHandle".to_owned(), Value::Binary(0x1234_u64.to_be_bytes().to_vec().into())),
                    ("SimIP".to_owned(), Value::Binary(vec![192, 0, 2, 4].into())),
                    ("SimPort".to_owned(), Value::Integer(13_000)),
                    ("SeedCapability".to_owned(), Value::URI("https://example.test/seed".into())),
                ])),
            ),
        ]))
    }

    fn teleport_finish(agent_id: UUID, seed: &str) -> Message {
        let schema = SchemaMessage::schema_by_name("TeleportFinish").expect("TeleportFinish schema");
        Message::Schema(
            SchemaMessage::new(
                schema,
                vec![BlockValues::single(vec![
                    FieldValue::UUID(agent_id),
                    FieldValue::U32(0),
                    FieldValue::IpAddr([192, 0, 2, 4]),
                    FieldValue::IpPort(13_000_u16.to_be_bytes()),
                    FieldValue::U64(0x1234),
                    FieldValue::Bytes(seed.as_bytes().to_vec()),
                    FieldValue::U8(0),
                    FieldValue::U32(0),
                ])],
            )
            .expect("valid TeleportFinish"),
        )
    }

    fn handoff(region_handle: u64, endpoint: &str, seed: &str) -> HandoffTarget {
        HandoffTarget {
            agent_id: UUID::from_u128(1),
            session_id: UUID::from_u128(2),
            region_handle,
            endpoint: endpoint.parse().expect("simulator endpoint"),
            seed_capability: Url::parse(seed).expect("seed URL"),
        }
    }

    #[test]
    fn crossed_region_is_bound_to_session() {
        let agent_id = UUID::from_u128(1);
        let session_id = UUID::from_u128(2);
        let handoff = decode_crossed_region(&crossed_region(agent_id, session_id), agent_id, session_id).expect("valid crossing");
        assert_eq!(handoff.region_handle, 0x1234);
        assert_eq!(handoff.endpoint, "192.0.2.4:13000".parse().expect("simulator endpoint"));
        assert!(decode_crossed_region(&crossed_region(UUID::from_u128(3), session_id), agent_id, session_id).is_err());
        assert!(decode_crossed_region(&crossed_region(agent_id, UUID::from_u128(4)), agent_id, session_id).is_err());
    }

    #[test]
    fn teleport_finish_has_checked_destination() {
        let agent_id = UUID::from_u128(1);
        let session_id = UUID::from_u128(2);
        let handoff = decode_teleport_finish(&teleport_finish(agent_id, "https://example.test/seed\0"), agent_id, session_id)
            .expect("valid teleport finish");
        assert_eq!(handoff.region_handle, 0x1234);
        assert!(decode_teleport_finish(&teleport_finish(UUID::from_u128(3), "https://example.test/seed"), agent_id, session_id).is_err());
        assert!(decode_teleport_finish(&teleport_finish(agent_id, "file:///seed"), agent_id, session_id).is_err());
    }

    #[test]
    fn promotion_requires_current_matching_destination() {
        let source = SimulatorId::new(1);
        let target = SimulatorId::new(2);
        let handoff = handoff(0x1234, "192.0.2.4:13000", "https://example.test/seed");
        let pending = PendingPromotion { source, target, handoff, evidence: HandoffEvidence::default() };
        let movement = AgentMovementComplete {
            agent_data: AgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) },
            data: Data { region_handle: 0x1234, ..Default::default() },
            sim_data: SimData::default(),
        };

        assert!(matches_pending_promotion(Some(&pending), source, target, &movement));
        assert!(!matches_pending_promotion(Some(&pending), SimulatorId::new(3), target, &movement));
        assert!(!matches_pending_promotion(Some(&pending), source, SimulatorId::new(3), &movement));
        let mut stale = movement.clone();
        stale.data.region_handle = 0x5678;
        assert!(!matches_pending_promotion(Some(&pending), source, target, &stale));
        let mut wrong_agent = movement;
        wrong_agent.agent_data.session_id = UUID::from_u128(4);
        assert!(!matches_pending_promotion(Some(&pending), source, target, &wrong_agent));
        assert!(!matches_pending_promotion(None, source, target, &wrong_agent));
    }

    #[test]
    fn handoff_duplicates_ignore_seed_changes() {
        let current = handoff(0x1234, "192.0.2.4:13000", "https://example.test/old-seed");
        let announced = handoff(0x1234, "192.0.2.4:13000", "https://example.test/new-seed");
        let pending = PendingPromotion {
            source: SimulatorId::new(1),
            target: SimulatorId::new(2),
            handoff: current.clone(),
            evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Ordered(8)) },
        };

        assert!(same_handoff_destination(&current, &announced));
        assert_eq!(arbitrate_handoff(&pending, &announced, HandoffAnnouncement::CrossedRegion), HandoffDecision::SameDestination);
    }

    #[test]
    fn contradictory_handoffs_need_same_circuit_order() {
        let current = PendingPromotion {
            source: SimulatorId::new(1),
            target: SimulatorId::new(2),
            handoff: handoff(0x1234, "192.0.2.4:13000", "https://example.test/seed"),
            evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Ordered(8)) },
        };
        let other = handoff(0x5678, "192.0.2.5:13000", "https://example.test/other");

        assert_eq!(arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(Ordered(9))), HandoffDecision::ReplaceNewer);
        assert_eq!(arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(Ordered(7))), HandoffDecision::IgnoreStale);
        assert_eq!(arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(Ordered(8))), HandoffDecision::Conflict);
        assert_eq!(arbitrate_handoff(&current, &other, HandoffAnnouncement::TeleportFinish(Ambiguous)), HandoffDecision::Conflict);
        assert_eq!(arbitrate_handoff(&current, &other, HandoffAnnouncement::CrossedRegion), HandoffDecision::Conflict);

        let crossed = PendingPromotion { evidence: HandoffEvidence { crossed_region: true, ..current.evidence }, ..current };
        assert_eq!(arbitrate_handoff(&crossed, &other, HandoffAnnouncement::TeleportFinish(Ordered(9))), HandoffDecision::Conflict);
    }

    #[test]
    fn teleport_failure_needs_newer_local_order() {
        let pending = PendingPromotion {
            source: SimulatorId::new(1),
            target: SimulatorId::new(2),
            handoff: handoff(0x1234, "192.0.2.4:13000", "https://example.test/seed"),
            evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Ordered(8)) },
        };

        assert!(teleport_failure_matches(Some(&pending), Ordered(9)));
        assert!(!teleport_failure_matches(Some(&pending), Ordered(8)));
        assert!(!teleport_failure_matches(Some(&pending), Ordered(7)));
        assert!(!teleport_failure_matches(Some(&pending), Ambiguous));
        assert!(!teleport_failure_matches(Some(&pending), Degraded));

        let crossed = PendingPromotion { evidence: HandoffEvidence { crossed_region: true, ..pending.evidence }, ..pending.clone() };
        assert!(!teleport_failure_matches(Some(&crossed), Ordered(9)));
        let unusable = PendingPromotion {
            evidence: HandoffEvidence { crossed_region: false, teleport_finish: Some(FinishOrder::Unusable) },
            ..pending
        };
        assert!(!teleport_failure_matches(Some(&unusable), Ordered(9)));
    }

    #[test]
    fn teleport_cancel_and_crossing_seed_are_redacted() {
        let schema = SchemaMessage::schema_by_name("TeleportCancel").expect("TeleportCancel schema");
        let message = Message::Schema(
            SchemaMessage::new(
                schema,
                vec![BlockValues::single(vec![FieldValue::UUID(UUID::from_u128(1)), FieldValue::UUID(UUID::from_u128(2))])],
            )
            .expect("valid cancel"),
        );
        assert!(is_teleport_cancel(&message, UUID::from_u128(1), UUID::from_u128(2)));
        assert!(!is_teleport_cancel(&message, UUID::from_u128(1), UUID::from_u128(3)));

        let body = crossed_region(UUID::from_u128(1), UUID::from_u128(2));
        let redacted = redact_seed_capability(&body);
        let Value::Map(fields) = redacted else { panic!("expected map") };
        let Some(Value::Map(region)) = fields.get("RegionData") else { panic!("expected region data") };
        assert_eq!(region.get("SeedCapability"), Some(&Value::String("[redacted]".into())));
    }

    #[test]
    fn enable_event() {
        let value = event(vec![simulator(0x1000_u64.to_be_bytes().to_vec(), [192, 0, 2, 7].to_vec(), 13_000)]);
        assert_eq!(
            decode_enable_simulators(&value).expect("valid neighbor event"),
            vec![(0x1000, "192.0.2.7:13000".parse().expect("simulator endpoint"))]
        );
    }

    #[test]
    fn bad_entry_lengths() {
        let value = event(vec![simulator(vec![0; 7], vec![127, 0, 0], 13_000)]);
        assert_eq!(decode_enable_simulators(&value), Err("Handle is not 8 bytes"));

        let value = event(vec![simulator(vec![0; 8], vec![127, 0, 0], 13_000)]);
        assert_eq!(decode_enable_simulators(&value), Err("IP is not 4 bytes"));
    }

    #[test]
    fn bad_port() {
        let value = event(vec![simulator(vec![0; 8], vec![127, 0, 0, 1], 0)]);
        assert_eq!(decode_enable_simulators(&value), Err("Port is outside the valid range"));
    }

    #[test]
    fn too_many_entries() {
        let entries = (0..=MAX_SIMULATORS_PER_ENABLE_EVENT)
            .map(|index| simulator((index as u64).to_be_bytes().to_vec(), vec![127, 0, 0, 1], 13_000))
            .collect();
        assert_eq!(decode_enable_simulators(&event(entries)), Err("SimulatorInfo has an invalid number of entries"));
    }

    #[test]
    fn agent_communication() {
        let agent = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
        let body = Value::Map(Map::from([
            ("agent-id".to_owned(), Value::UUID(agent)),
            ("sim-ip-and-port".to_owned(), Value::String("192.0.2.7:13000".to_owned())),
            ("seed-capability".to_owned(), Value::String("https://sim.example.invalid/seed".to_owned())),
        ]));
        let (endpoint, seed) = decode_agent_communication(&body, agent).expect("valid agent communication event");
        assert_eq!(endpoint, "192.0.2.7:13000".parse().expect("simulator endpoint"));
        assert_eq!(seed.as_str(), "https://sim.example.invalid/seed");
        assert_eq!(decode_agent_communication(&body, UUID::default()), Err("agent-id does not match the logged-in agent"));
    }

    #[test]
    fn seed_is_redacted() {
        let body = Value::Map(Map::from([("seed-capability".to_owned(), Value::String("https://sim.invalid/private".to_owned()))]));
        assert_eq!(
            redact_seed_capability(&body),
            Value::Map(Map::from([("seed-capability".to_owned(), Value::String("[redacted]".to_owned()))]))
        );
    }

    #[test]
    fn neighbor_overload_reports_failure() {
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let region_handle = 0x1000_0000_0000_1000;
        let endpoint = "127.0.0.1:9000".parse().expect("parse endpoint");

        assert!(matches!(
            neighbor_completion_event(
                simulator_id,
                region_handle,
                endpoint,
                &NeighborTaskOutcome::Finished(Err(EVENT_BACKLOG_DISCONNECT_REASON.into())),
            ),
            Event::SimulatorFailed {
                simulator_id: failed_id,
                region_handle: Some(failed_region),
                endpoint: failed_endpoint,
                reason,
            } if failed_id == simulator_id
                && failed_region == region_handle
                && failed_endpoint == endpoint
                && reason == EVENT_BACKLOG_DISCONNECT_REASON
        ));
    }

    #[tokio::test]
    async fn event_queue_panic_reaches_simulator() {
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let primary_id = SimulatorId::allocate().expect("allocate primary ID");
        let source = "127.0.0.1:9000".parse::<SocketAddr>().expect("parse endpoint");
        let mut neighbors = JoinSet::<()>::new();
        let abort = neighbors.spawn(pending::<()>());
        let (commands, _) = mpsc::channel::<NeighborCommand>(1);
        let connections = HashMap::from([(1, NeighborConnectionState { simulator_id, endpoint: source, abort, commands })]);

        let mut event_queues = JoinSet::<(SimulatorId, SocketAddr, EventQueueExit)>::new();
        let task = event_queues.spawn(async { panic!("injected EventQueue worker panic") });
        let mut owners = HashMap::from([(task.id(), (simulator_id, source))]);
        let Some(Err(error)) = event_queues.join_next().await else {
            panic!("injected worker should fail");
        };
        assert!(error.is_panic());

        assert!(matches!(
            neighbor_event_queue_task_failure(&error, &mut owners, primary_id, &connections),
            Some(Event::CapabilityError { simulator_id: failed_id, source: failed_source, .. })
                if failed_id == simulator_id && failed_source == source
        ));
        assert!(owners.is_empty());
        neighbors.abort_all();
    }

    #[tokio::test]
    async fn stale_event_queue_completion_is_ignored() {
        let stale_id = SimulatorId::allocate().expect("allocate stale simulator ID");
        let current_id = SimulatorId::allocate().expect("allocate replacement simulator ID");
        let primary_id = SimulatorId::allocate().expect("allocate primary ID");
        let source = "127.0.0.1:9001".parse::<SocketAddr>().expect("parse endpoint");
        let mut neighbors = JoinSet::<()>::new();
        let (commands, _) = mpsc::channel::<NeighborCommand>(1);
        let mut connections = HashMap::from([(
            1,
            NeighborConnectionState { simulator_id: stale_id, endpoint: source, abort: neighbors.spawn(pending::<()>()), commands },
        )]);

        let mut event_queues = JoinSet::<(SimulatorId, SocketAddr, EventQueueExit)>::new();
        let task = event_queues.spawn(async { panic!("injected stale EventQueue worker panic") });
        let mut owners = HashMap::from([(task.id(), (stale_id, source))]);
        let Some(Err(error)) = event_queues.join_next().await else {
            panic!("injected worker should fail");
        };
        assert!(error.is_panic());

        let (commands, _) = mpsc::channel::<NeighborCommand>(1);
        let replacement =
            NeighborConnectionState { simulator_id: current_id, endpoint: source, abort: neighbors.spawn(pending::<()>()), commands };
        if let Some(stale) = connections.insert(1, replacement) {
            stale.abort.abort();
        }

        assert!(neighbor_event_queue_task_failure(&error, &mut owners, primary_id, &connections).is_none());
        assert!(neighbor_event_queue_failure(stale_id, source, EventQueueExit::TaskFailed, primary_id, &connections).is_none());
        assert!(owners.is_empty());
        neighbors.abort_all();
    }

    #[tokio::test]
    async fn neighbor_panic_removes_only_its_generation() {
        let failed_id = SimulatorId::allocate().expect("allocate failed simulator ID");
        let other_id = SimulatorId::allocate().expect("allocate other simulator ID");
        let failed_endpoint = "127.0.0.1:9002".parse::<SocketAddr>().expect("parse failed endpoint");
        let other_endpoint = "127.0.0.1:9003".parse::<SocketAddr>().expect("parse other endpoint");
        let mut tasks = JoinSet::<()>::new();
        let failed_task = tasks.spawn(async { panic!("injected neighbor worker panic") });
        let other_task = tasks.spawn(pending::<()>());
        let (failed_commands, _) = mpsc::channel::<NeighborCommand>(1);
        let (other_commands, _) = mpsc::channel::<NeighborCommand>(1);
        let mut connections = HashMap::from([
            (
                1,
                NeighborConnectionState {
                    simulator_id: failed_id,
                    endpoint: failed_endpoint,
                    abort: failed_task,
                    commands: failed_commands,
                },
            ),
            (2, NeighborConnectionState { simulator_id: other_id, endpoint: other_endpoint, abort: other_task, commands: other_commands }),
        ]);

        let Some(Err(error)) = tasks.join_next().await else {
            panic!("injected neighbor worker should panic");
        };
        assert!(error.is_panic());
        assert_eq!(remove_failed_neighbor(&error, &mut connections), Some((1, failed_id, failed_endpoint)));
        assert!(!connections.contains_key(&1));
        assert!(connections.get(&2).is_some_and(|connection| connection.simulator_id == other_id));

        let stale_id = SimulatorId::allocate().expect("allocate stale simulator ID");
        let replacement_id = SimulatorId::allocate().expect("allocate replacement simulator ID");
        let stale_endpoint = "127.0.0.1:9004".parse::<SocketAddr>().expect("parse stale endpoint");
        let stale_task = tasks.spawn(async { panic!("injected late neighbor worker panic") });
        let (commands, _) = mpsc::channel::<NeighborCommand>(1);
        connections.insert(3, NeighborConnectionState { simulator_id: stale_id, endpoint: stale_endpoint, abort: stale_task, commands });
        let current_task = tasks.spawn(pending::<()>());
        let (commands, _) = mpsc::channel::<NeighborCommand>(1);
        connections
            .insert(3, NeighborConnectionState { simulator_id: replacement_id, endpoint: stale_endpoint, abort: current_task, commands });

        let Some(Err(error)) = tasks.join_next().await else {
            panic!("injected stale worker should panic");
        };
        assert!(error.is_panic());
        assert_eq!(remove_failed_neighbor(&error, &mut connections), None);
        assert!(connections.get(&3).is_some_and(|connection| connection.simulator_id == replacement_id));
        assert_eq!(connections.len(), 2);
        tasks.abort_all();
    }
}

#[cfg(test)]
mod session_tests {
    use std::{collections::HashMap, net::SocketAddr, time::Duration};

    use crate::{
        AgentSession, Error, Grid,
        circuit::{
            Circuit, DEFAULT_RETRY_TIMEOUT, Delivery, MAX_PENDING_PACKETS, MAX_RETRIES, PendingPacket, RESENT_FLAG, acknowledge_pending,
            retransmit_due,
        },
        client::{COMMAND_CAPACITY, Command},
        event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY},
        simulator::{MAX_PENDING_SIMULATOR_EVENTS, run_session},
        udp::UdpTransport,
    };
    use base::UUID;
    use protocol::{
        ChatType,
        lludp::{Message, Packet},
        messages::{
            ChatFromViewer, CompletePingCheck, LogoutRequest, PacketAck, PingId, chat_from_viewer, chat_from_viewer::ChatData,
            logout_request, packet_ack::Packets,
        },
    };
    use test_support::{lludp::vectors, parse_hex_fixture};
    use tokio::{
        net::UdpSocket,
        spawn,
        sync::{mpsc, watch},
        task::yield_now,
        time::{Instant, timeout},
    };
    use url::Url;

    fn complete_ping(ping_id: u8) -> Message {
        Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id } })
    }

    fn packet_ack(packet_ids: &[u32]) -> Message {
        Message::PacketAck(PacketAck { packets: packet_ids.iter().map(|id| Packets { id: *id }).collect() })
    }

    fn packet_ack_contains(message: &Message, packet_id: u32) -> bool {
        matches!(message, Message::PacketAck(ack) if ack.packets.iter().any(|packet| packet.id == packet_id))
    }

    fn receive_packet(circuit: &mut Circuit, packet: &Packet) -> bool {
        circuit.receive(packet).expect("valid circuit packet").is_new_packet
    }

    fn is_complete_ping(message: &Message, ping_id: u8) -> bool {
        matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == ping_id)
    }

    const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
    const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
    const CIRCUIT_CODE: u32 = 0x1122_3344;
    const SIMULATOR_TIMEOUT: Duration = Duration::from_secs(2);
    const SESSION_EVENT_CAPACITY: usize = 2;
    const RETRANSMISSION_TIMEOUT: Duration = Duration::from_secs(8);

    fn chat_message(message: impl Into<String>) -> Message {
        Message::from(ChatFromViewer {
            agent_data: chat_from_viewer::AgentData {
                agent_id: UUID::parse_str(AGENT_ID).expect("valid test agent ID"),
                session_id: UUID::parse_str(SESSION_ID).expect("valid test session ID"),
            },
            chat_data: ChatData { message: message.into(), chat_type: ChatType::Normal, channel: 0 },
        })
    }

    fn logout_message() -> Message {
        Message::LogoutRequest(LogoutRequest {
            agent_data: logout_request::AgentData {
                agent_id: UUID::parse_str(AGENT_ID).expect("valid test agent ID"),
                session_id: UUID::parse_str(SESSION_ID).expect("valid test session ID"),
            },
        })
    }

    #[tokio::test]
    async fn handshake_chat_logout() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));

        let mut buffer = [0; 65_535];
        assert!(timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err());
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");

        let (movement, movement_peer) = receive(&simulator).await;
        assert_eq!(movement_peer, peer);
        assert_eq!(movement, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        let mut malformed = [0x80, 0, 0, 0, 0, 0, 0x04, 0];
        set_sequence(&mut malformed, 22);
        simulator.send_to(&malformed, peer).await.expect("send malformed datagram");

        let mut region_handshake = fixture("region_handshake.hex");
        region_handshake[0] |= 0x40;
        set_sequence(&mut region_handshake, 24);
        simulator.send_to(&region_handshake, peer).await.expect("send region handshake before delayed movement completion");
        let (reply, reply_peer) = receive(&simulator).await;
        assert_eq!(reply_peer, peer);
        assert_eq!(reply, with_appended_ack(expected_zerocoded_reply(3), 24));
        simulator.send_to(&server_ack(25, &[3]), peer).await.expect("ack region handshake reply");
        let handshake_event = next_event(&mut events).await;
        let Event::SimulatorMessage { source, sequence, message, .. } = handshake_event else {
            panic!("expected the region handshake to be delivered to the application");
        };
        assert_eq!(source, simulator.local_addr().expect("simulator address"));
        assert_eq!(sequence, 24);
        assert!(matches!(message, Message::RegionHandshake(message) if message.region_info1.sim_name == "Test Region"));

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        simulator.send_to(&movement_complete, peer).await.expect("deliver delayed movement completion out of order");
        let Event::SessionReady { capabilities, .. } = next_event(&mut events).await else {
            panic!("expected the simulator session to become ready");
        };
        let movement_event = next_event(&mut events).await;
        assert!(matches!(movement_event, Event::SimulatorMessage { sequence: 23, message: Message::AgentMovementComplete(_), .. }));

        commands
            .send(Command::SendMessage { message: chat_message("ab"), delivery: Delivery::Reliable })
            .await
            .expect("session should accept a simulator message");
        let (chat, chat_peer) = receive(&simulator).await;
        assert_eq!(chat_peer, peer);
        assert_eq!(chat, with_appended_ack(expected_chat(4, "ab", 1, 0), 23));
        simulator.send_to(&server_ack(26, &[5]), peer).await.expect("ack nearby chat");

        let mut incoming_chat = fixture("chat_from_simulator.hex");
        incoming_chat[0] |= 0x40;
        set_sequence(&mut incoming_chat, 27);
        simulator.send_to(&incoming_chat, peer).await.expect("send reliable nearby chat");
        let (first_ack, first_ack_peer) = receive(&simulator).await;
        assert_eq!(first_ack_peer, peer);
        assert_eq!(first_ack, client_ack(5, 27));

        incoming_chat[0] |= RESENT_FLAG;
        simulator.send_to(&incoming_chat, peer).await.expect("resend nearby chat");
        let (duplicate_ack, duplicate_ack_peer) = receive(&simulator).await;
        assert_eq!(duplicate_ack_peer, peer);
        assert_eq!(duplicate_ack, client_ack(6, 27));

        let chat_event = next_event(&mut events).await;
        let Event::SimulatorMessage { source, sequence, message, .. } = chat_event else {
            panic!("expected one generic simulator message event");
        };
        assert_eq!(source, simulator.local_addr().expect("simulator address"));
        assert_eq!(sequence, 27);
        let Message::ChatFromSimulator(message) = message else {
            panic!("expected decoded ChatFromSimulator message");
        };
        assert_eq!(message.chat_data.from_name, "Ada");
        assert_eq!(message.chat_data.message, "Hi");
        yield_now().await;
        assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

        disconnect_signal.send_replace(true);
        let (logout, logout_peer) = receive(&simulator).await;
        assert_eq!(logout_peer, peer);
        assert_eq!(logout, expected_fixture("logout_request.hex", 7, true));
        assert!(
            timeout(Duration::from_millis(200), client)
                .await
                .expect("disconnect should not wait for a simulator reply")
                .expect("session task should not panic")
                .is_ok()
        );
        let error =
            capabilities.request_capabilities(&["TestCapability"]).await.expect_err("session teardown closes retained capabilities");
        assert!(matches!(error, Error::CapabilitySessionClosed));
    }

    #[tokio::test]
    async fn ping_reacks_recent_duplicate() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (_, peer) = receive(&simulator).await;
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, 100);
        simulator.send_to(&chat, peer).await.expect("send reliable chat");
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 100, .. }));
        receive_ack(&simulator, 100).await;

        let ping = start_ping_packet(102, 9, 101);
        simulator.send_to(&ping, peer).await.expect("send ping checkpoint");
        receive_message(&simulator, |message| is_complete_ping(message, 9)).await;

        chat[0] |= RESENT_FLAG;
        simulator.send_to(&chat, peer).await.expect("resend acknowledged chat");
        receive_ack(&simulator, 100).await;
        assert!(
            timeout(Duration::from_millis(50), events.recv()).await.is_err(),
            "recent reliable duplicate should not be dispatched twice"
        );

        disconnect_signal.send_replace(true);
        receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(Duration::from_millis(200), client).await.expect("disconnect should finish").expect("session task").is_ok());
    }

    #[tokio::test]
    async fn shutdown_preempts_chat() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        receive_primary_connected(&mut events).await;

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
        loop {
            if matches!(next_event(&mut events).await, Event::SessionReady { .. }) {
                break;
            }
        }

        commands
            .try_send(Command::SendMessage { message: chat_message("must not send"), delivery: Delivery::Reliable })
            .expect("queue chat before shutdown");
        disconnect_signal.send_replace(true);

        let logout = timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode client packet");
                assert!(!matches!(packet.message(), Message::ChatFromViewer(_)), "queued chat must not be sent after shutdown");
                if matches!(packet.message(), Message::LogoutRequest(_)) {
                    break packet;
                }
            }
        })
        .await
        .expect("shutdown should send a logout request");
        assert!(logout.is_reliable());
        assert_eq!(logout.message(), &logout_message());
        assert!(
            timeout(Duration::from_millis(200), client)
                .await
                .expect("shutdown should not wait for a simulator reply")
                .expect("session task should not panic")
                .is_ok()
        );
        let mut buffer = [0; 65_535];
        assert!(
            timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err(),
            "shutdown should close the simulator socket"
        );
    }

    #[tokio::test]
    async fn backpressure_keeps_acks() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        receive_primary_connected(&mut events).await;

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
        assert_eq!(receive(&simulator).await.0, client_ack(4, 23));

        disconnect_signal.send_replace(true);
        assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", 5, true));
        assert!(
            timeout(Duration::from_millis(200), client)
                .await
                .expect("disconnect should not wait for a simulator reply")
                .expect("session task should not panic")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn ping_survives_backpressure() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        receive_primary_connected(&mut events).await;

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
        simulator.send_to(&server_ack(26, &[3]), peer).await.expect("ack region handshake reply");
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. }));

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
        assert!(matches!(next_event(&mut events).await, Event::SessionReady { .. }));
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. }));

        for sequence in [24, 25] {
            let mut chat = fixture("chat_from_simulator.hex");
            chat[0] |= 0x40;
            set_sequence(&mut chat, sequence);
            simulator.send_to(&chat, peer).await.expect("send reliable simulator chat");
        }

        let ping = start_ping_packet(27, 7, 0);
        simulator.send_to(&ping, peer).await.expect("send ping request while viewer queue is full");
        let ping_reply = timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode simulator reply");
                if is_complete_ping(packet.message(), 7) {
                    break packet;
                }
            }
        })
        .await
        .expect("simulator should answer ping while event delivery is backed up");
        assert_eq!(ping_reply.message(), &complete_ping(7));
        assert!(matches!(events.try_recv().map(EventEnvelope::into_event), Ok(Event::SimulatorMessage { sequence: 24, .. })));

        disconnect_signal.send_replace(true);
        let logout = timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode client message");
                if matches!(packet.message(), Message::LogoutRequest(_)) {
                    break packet;
                }
            }
        })
        .await
        .expect("disconnect should be handled while events are queued");
        assert!(matches!(logout.message(), Message::LogoutRequest(_)));
        assert!(
            timeout(Duration::from_millis(200), client)
                .await
                .expect("disconnect should not wait for a simulator reply")
                .expect("session task should not panic")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn overload_keeps_control() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (_disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        receive_primary_connected(&mut events).await;

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
        simulator.send_to(&server_ack(26, &[3]), peer).await.expect("ack region handshake reply");
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. }));

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
        assert!(matches!(next_event(&mut events).await, Event::SessionReady { .. }));
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. }));

        let message_count = SESSION_EVENT_CAPACITY + MAX_PENDING_SIMULATOR_EVENTS;
        for sequence in 24..24 + message_count as u32 {
            let mut chat = fixture("chat_from_simulator.hex");
            chat[0] |= 0x40;
            set_sequence(&mut chat, sequence);
            simulator.send_to(&chat, peer).await.expect("fill the bounded event backlog");
        }

        let logout = timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode client message");
                if matches!(packet.message(), Message::LogoutRequest(_)) {
                    break packet;
                }
            }
        })
        .await
        .expect("saturated event backlog should start logout");

        let mut undeliverable_chat = fixture("chat_from_simulator.hex");
        undeliverable_chat[0] |= 0x40;
        set_sequence(&mut undeliverable_chat, 42);
        simulator.send_to(&undeliverable_chat, peer).await.expect("send chat after overload starts");
        let ping = start_ping_packet(43, 8, 0);
        simulator.send_to(&ping, peer).await.expect("send ping while logging out");

        let mut acknowledged_undeliverable_chat = false;
        let ping_reply = timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode simulator reply");
                acknowledged_undeliverable_chat |= packet.appended_acks().contains(&42) || packet_ack_contains(packet.message(), 42);
                if is_complete_ping(packet.message(), 8) {
                    break packet;
                }
            }
        })
        .await
        .expect("ping should be answered after event backlog saturation");
        assert!(is_complete_ping(ping_reply.message(), 8));
        assert!(acknowledged_undeliverable_chat, "reliable packet should be acknowledged after its event is dropped");

        simulator.send_to(&server_ack(50, &[logout.sequence()]), peer).await.expect("ack logout request");
        let mut logout_reply = fixture("logout_reply.hex");
        logout_reply[0] |= 0x40;
        set_sequence(&mut logout_reply, 51);
        simulator.send_to(&logout_reply, peer).await.expect("send logout reply");
        assert_eq!(receive(&simulator).await.0, client_ack(logout.sequence() + 3, 51));
        let failure = timeout(SIMULATOR_TIMEOUT, async {
            loop {
                if let Event::Failed(reason) = next_event(&mut events).await {
                    break reason;
                }
            }
        })
        .await
        .expect("event backlog disconnect should reach the viewer");
        assert!(failure.contains("could not keep up with incoming events"));
        assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_err());
    }

    #[tokio::test]
    async fn handshake_fits_backlog() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        receive_primary_connected(&mut events).await;

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
        simulator.send_to(&server_ack(26, &[3]), peer).await.expect("ack region handshake reply");
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. }));

        for sequence in 23..=38 {
            let mut chat = fixture("chat_from_simulator.hex");
            chat[0] |= 0x40;
            set_sequence(&mut chat, sequence);
            simulator.send_to(&chat, peer).await.expect("fill the bounded event backlog");

            loop {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode client ACK");
                let acknowledged = packet.appended_acks().contains(&sequence) || packet_ack_contains(packet.message(), sequence);
                if acknowledged {
                    break;
                }
            }
        }

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 39);
        simulator.send_to(&movement_complete, peer).await.expect("send movement completion at backlog limit");

        let mut acknowledged_movement_complete = false;
        let logout = timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode client message");
                acknowledged_movement_complete |= packet.appended_acks().contains(&39) || packet_ack_contains(packet.message(), 39);
                if matches!(packet.message(), Message::LogoutRequest(_)) {
                    break packet;
                }
            }
        })
        .await
        .expect("full backlog should start logout after both handshake events are queued");
        assert_ne!(logout.sequence(), 0);
        acknowledged_movement_complete |= logout.appended_acks().contains(&39) || packet_ack_contains(logout.message(), 39);
        assert!(acknowledged_movement_complete, "reliable movement completion must be acknowledged before disconnect");

        let mut saw_session_ready = false;
        let mut saw_movement_complete = false;
        while !(saw_session_ready && saw_movement_complete) {
            match next_event(&mut events).await {
                Event::SessionReady { .. } => saw_session_ready = true,
                Event::SimulatorMessage { sequence: 39, .. } => saw_movement_complete = true,
                _ => {}
            }
        }

        disconnect_signal.send_replace(true);
        assert!(
            timeout(Duration::from_millis(200), client)
                .await
                .expect("disconnect should not wait for a simulator reply")
                .expect("session task should not panic")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn closed_receiver_logs_out() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (_disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        receive_primary_connected(&mut events).await;

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { message: Message::RegionHandshake(_), .. }));

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        drop(events);
        simulator.send_to(&movement_complete, peer).await.expect("send packet as receiver closes");
        assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", 4, true));

        let mut buffer = [0; 65_535];
        let unexpected = timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await;
        assert!(!matches!(&unexpected, Ok(Ok(_))), "closed receiver must not ACK an undeliverable message: {unexpected:?}");

        assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
    }

    #[tokio::test]
    async fn disconnect_blocks_movement() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));

        simulator.try_send_to(&server_ack(20, &[1]), peer).expect("queue circuit ACK while disconnect is pending");
        disconnect_signal.send_replace(true);
        let (logout, _) = receive(&simulator).await;
        assert_eq!(logout, expected_fixture("logout_request.hex", 2, true));

        let mut buffer = [0; 65_535];
        assert!(
            timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err(),
            "a queued circuit ACK must not start movement after disconnect is requested"
        );

        assert!(
            timeout(SIMULATOR_TIMEOUT, client)
                .await
                .expect("session should stop after disconnect")
                .expect("session task should not panic")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn retry_resends_then_expires() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");

        let original = expected_fixture("use_circuit_code.hex", 42, true);
        let mut pending =
            HashMap::from([(42, PendingPacket { bytes: original.clone(), sent_at: Instant::now() - DEFAULT_RETRY_TIMEOUT, retries: 0 })]);

        for retry in 1..=MAX_RETRIES {
            retransmit_due(&client, &mut pending).await.expect("packet should retry before limit");
            let (resent, _) = receive(&simulator).await;
            let mut expected = original.clone();
            expected[0] |= RESENT_FLAG;
            assert_eq!(resent, expected);
            assert_eq!(pending[&42].retries, retry);
            pending.get_mut(&42).expect("pending packet").sent_at = Instant::now() - DEFAULT_RETRY_TIMEOUT;
        }

        assert_eq!(retransmit_due(&client, &mut pending).await, Err("The simulator did not acknowledge a reliable packet.".into()));
    }

    #[tokio::test]
    async fn lost_packet_retries() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (original, peer) = receive(&simulator).await;
        assert_eq!(original, expected_fixture("use_circuit_code.hex", 1, true));

        let (resent, resent_peer) = receive_with_timeout(&simulator, RETRANSMISSION_TIMEOUT).await;
        let mut expected_resent = original;
        expected_resent[0] |= RESENT_FLAG;
        assert_eq!(resent_peer, peer);
        assert_eq!(resent, expected_resent);

        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack retransmitted circuit packet");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));

        disconnect_signal.send_replace(true);
        let logout = loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode disconnect packet");
            if let Message::StartPingCheck(_) = packet.message() {
                assert_eq!(packet.sequence(), 3);
                assert!(!packet.is_reliable());
                continue;
            }
            break packet;
        };
        assert!((3..=4).contains(&logout.sequence()));
        assert!(logout.is_reliable());
        assert_eq!(logout.message(), &logout_message());
        assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
    }

    #[tokio::test]
    async fn overflow_keeps_sequence() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
        let mut circuit = Circuit::new();

        for expected_sequence in 1..=MAX_PENDING_PACKETS as u32 {
            let sequence =
                circuit.send(&client, complete_ping(3), Delivery::Reliable).await.expect("packet should fit in the reliable window");
            assert_eq!(sequence, expected_sequence);
        }
        assert!(circuit.send(&client, complete_ping(4), Delivery::Reliable).await.is_err());

        for expected_sequence in 1..=MAX_PENDING_PACKETS as u32 {
            let (bytes, _) = receive(&simulator).await;
            assert_eq!(Packet::decode(&bytes).expect("decode queued packet").sequence(), expected_sequence);
        }
        assert!(
            timeout(Duration::from_millis(30), async {
                let mut buffer = [0; 32];
                simulator.recv_from(&mut buffer).await
            })
            .await
            .is_err()
        );

        let ack = Packet::decode(&server_ack(50, &[1])).expect("decode fake simulator ACK");
        assert!(receive_packet(&mut circuit, &ack));
        assert_eq!(
            circuit.send(&client, complete_ping(5), Delivery::Reliable).await.expect("ACK should free one slot"),
            MAX_PENDING_PACKETS as u32 + 1
        );
        let (bytes, _) = receive(&simulator).await;
        assert_eq!(Packet::decode(&bytes).expect("decode packet after ACK").sequence(), MAX_PENDING_PACKETS as u32 + 1);
    }

    #[tokio::test]
    async fn packet_piggybacks_ack() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
        let mut circuit = Circuit::new();

        let mut incoming = fixture("chat_from_simulator.hex");
        incoming[0] |= 0x40;
        set_sequence(&mut incoming, 55);
        let incoming = Packet::decode(&incoming).expect("decode reliable simulator packet");
        assert!(receive_packet(&mut circuit, &incoming));

        circuit.send(&client, complete_ping(3), Delivery::Unreliable).await.expect("send next outbound packet");
        let (bytes, _) = receive(&simulator).await;
        let packet = Packet::decode(&bytes).expect("decode client packet");

        assert_eq!(packet.message(), &complete_ping(3));
        assert_eq!(packet.appended_acks(), &[55]);
    }

    #[tokio::test]
    async fn timer_acks_recover_loss() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let session = login_session(simulator.local_addr().expect("simulator address"));
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

        let (use_circuit, peer) = receive(&simulator).await;
        assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
        simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
        assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
        simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

        for sequence in [55, 56] {
            let mut incoming = fixture("chat_from_simulator.hex");
            incoming[0] |= 0x40;
            set_sequence(&mut incoming, sequence);
            simulator.send_to(&incoming, peer).await.expect("send reliable simulator chat");
        }

        let mut acknowledged = Vec::<u32>::new();
        let mut next_sequence = 3;
        while acknowledged.len() < 2 {
            let (bytes, _) = timeout(SIMULATOR_TIMEOUT, receive(&simulator)).await.expect("timer should send queued acknowledgements");
            let packet = Packet::decode(&bytes).expect("decode timer ACK packet");
            assert_eq!(packet.sequence(), next_sequence);
            let Message::PacketAck(message) = packet.message() else {
                panic!("expected timer to send a standalone ACK packet");
            };
            let packet_ids = message.packets.iter().map(|packet| packet.id).collect::<Vec<_>>();
            assert_eq!(bytes, server_ack(next_sequence, &packet_ids));
            acknowledged.extend(packet_ids);
            next_sequence += 1;
        }
        acknowledged.sort_unstable();
        assert_eq!(acknowledged, [55, 56]);

        disconnect_signal.send_replace(true);
        assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", next_sequence, true));
        assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
    }

    #[tokio::test]
    async fn encode_failure_keeps_ack() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
        let mut circuit = Circuit::new();

        let mut incoming = fixture("chat_from_simulator.hex");
        incoming[0] |= 0x40;
        set_sequence(&mut incoming, 55);
        let incoming = Packet::decode(&incoming).expect("decode reliable simulator packet");
        assert!(receive_packet(&mut circuit, &incoming));

        let error = circuit
            .send(&client, chat_message("x".repeat(65_520)), Delivery::Unreliable)
            .await
            .expect_err("message should exceed the LLUDP maximum size");
        assert!(error.contains("Could not encode a simulator message"));
        circuit.send(&client, complete_ping(3), Delivery::Unreliable).await.expect("send packet after failed encode");

        let (bytes, _) = receive(&simulator).await;
        let packet = Packet::decode(&bytes).expect("decode client packet");
        assert_eq!(packet.sequence(), 1);
        assert_eq!(packet.appended_acks(), &[55]);
    }

    #[tokio::test]
    async fn duplicate_requeues_ack() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
        let mut circuit = Circuit::new();

        let mut incoming = fixture("chat_from_simulator.hex");
        incoming[0] |= 0x40;
        set_sequence(&mut incoming, 55);
        let packet = Packet::decode(&incoming).expect("decode reliable simulator packet");
        assert!(receive_packet(&mut circuit, &packet));
        circuit.flush_pending_acks(&client).await.expect("flush first acknowledgement");
        assert_eq!(Packet::decode(&receive(&simulator).await.0).expect("decode first ACK").message(), &packet_ack(&[55]));

        incoming[0] |= RESENT_FLAG;
        let duplicate = Packet::decode(&incoming).expect("decode resent simulator packet");
        assert!(!receive_packet(&mut circuit, &duplicate));
        circuit.flush_pending_acks(&client).await.expect("flush duplicate acknowledgement");
        assert_eq!(Packet::decode(&receive(&simulator).await.0).expect("decode duplicate ACK").message(), &packet_ack(&[55]));
    }

    #[tokio::test]
    async fn full_ack_queue_defers_send() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
        let mut circuit = Circuit::new();

        for sequence in 1..=4096 {
            let packet = Packet::new(sequence, complete_ping(3)).with_reliable();
            assert!(receive_packet(&mut circuit, &packet));
        }
        let overflow = Packet::new(4097, complete_ping(3)).with_reliable();
        assert!(!receive_packet(&mut circuit, &overflow));

        circuit.send(&client, complete_ping(4), Delivery::Unreliable).await.expect("send packet to make ACK queue space");
        let (bytes, _) = receive(&simulator).await;
        let packet = Packet::decode(&bytes).expect("decode client packet");
        assert_eq!(packet.appended_acks().len(), u8::MAX as usize);
        assert!(receive_packet(&mut circuit, &overflow), "unacknowledged input should be accepted after queue space opens");
    }

    #[test]
    fn late_ack_clears_pending() {
        let acknowledged = [0x1122_3344, 0xa1b2_c3d4];
        let mut pending = pending_packets(&acknowledged);
        let packet = Packet::decode(&fixture("packet_ack.hex")).expect("decode ACK fixture");

        acknowledge_pending(&packet, &mut pending);
        assert!(pending.is_empty());

        let mut pending = pending_packets(&acknowledged);
        let packet = Packet::decode(&fixture("appended_acks.hex")).expect("decode appended ACK fixture");
        acknowledge_pending(&packet, &mut pending);
        assert!(pending.is_empty());
    }

    fn login_session(simulator_address: SocketAddr) -> AgentSession {
        AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address,
            agent_id: UUID::parse_str(AGENT_ID).expect("valid agent UUID"),
            session_id: UUID::parse_str(SESSION_ID).expect("valid session UUID"),
            circuit_code: CIRCUIT_CODE,
            seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("valid local capability URL"),
        }
    }

    async fn receive(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        receive_with_timeout(socket, SIMULATOR_TIMEOUT).await
    }

    async fn receive_with_timeout(socket: &UdpSocket, wait: Duration) -> (Vec<u8>, SocketAddr) {
        let mut buffer = [0; 65_535];
        let (size, peer) = timeout(wait, socket.recv_from(&mut buffer))
            .await
            .expect("client should send a UDP datagram")
            .expect("read fake simulator datagram");
        (buffer[..size].to_vec(), peer)
    }

    async fn receive_ack(socket: &UdpSocket, sequence: u32) -> Packet {
        timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(socket).await;
                let packet = Packet::decode(&bytes).expect("decode client packet");
                let in_message = packet_ack_contains(packet.message(), sequence);
                if in_message || packet.appended_acks().contains(&sequence) {
                    return packet;
                }
            }
        })
        .await
        .expect("client should acknowledge the reliable packet")
    }

    async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
        timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let (bytes, _) = receive(socket).await;
                let packet = Packet::decode(&bytes).expect("decode client packet");
                if expected(packet.message()) {
                    return packet;
                }
            }
        })
        .await
        .expect("client should send the expected message")
    }

    async fn next_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
        timeout(SIMULATOR_TIMEOUT, async {
            loop {
                let event = events.recv().await.expect("client event channel should remain open").into_event();
                if matches!(&event, Event::CapabilityError { capability, .. } if capability == "EventQueueGet") {
                    continue;
                }
                if matches!(&event, Event::SimulatorConnected { region_handle: None, .. }) {
                    continue;
                }
                return event;
            }
        })
        .await
        .expect("client should publish an event")
    }

    async fn receive_primary_connected(events: &mut mpsc::Receiver<EventEnvelope>) {
        timeout(SIMULATOR_TIMEOUT, async {
            loop {
                match events.recv().await.expect("client event channel should remain open").into_event() {
                    Event::SimulatorConnected { region_handle: None, .. } => return,
                    Event::CapabilityError { capability, .. } if capability == "EventQueueGet" => {}
                    _ => panic!("unexpected event before primary connection"),
                }
            }
        })
        .await
        .expect("client should publish primary connection");
    }

    fn fixture(name: &str) -> Vec<u8> {
        let source = vectors::get(name).unwrap_or_else(|| panic!("unknown fixture: {name}"));
        parse_hex_fixture(source).unwrap_or_else(|error| panic!("invalid fixture {name}: {error}"))
    }

    fn expected_fixture(name: &str, sequence: u32, reliable: bool) -> Vec<u8> {
        let mut bytes = fixture(name);
        bytes[0] = (bytes[0] & !0x40) | if reliable { 0x40 } else { 0 };
        set_sequence(&mut bytes, sequence);
        bytes
    }

    fn expected_complete_movement(sequence: u32) -> Vec<u8> {
        let mut bytes = fixture("complete_agent_movement.hex");
        bytes[0] |= 0x40;
        set_sequence(&mut bytes, sequence);
        let end = bytes.len();
        bytes[end - 4..].copy_from_slice(&CIRCUIT_CODE.to_le_bytes());
        bytes
    }

    fn expected_zerocoded_reply(sequence: u32) -> Vec<u8> {
        let mut bytes = fixture("region_handshake_reply.hex");
        bytes[0] |= 0x40;
        set_sequence(&mut bytes, sequence);
        bytes
    }

    fn with_appended_ack(mut bytes: Vec<u8>, packet_id: u32) -> Vec<u8> {
        bytes[0] |= 0x10;
        bytes.extend_from_slice(&packet_id.to_le_bytes());
        bytes.push(1);
        bytes
    }

    fn expected_chat(sequence: u32, message: &str, chat_type: u8, channel: i32) -> Vec<u8> {
        let mut bytes = vec![0x40];
        bytes.extend_from_slice(&sequence.to_be_bytes());
        bytes.extend_from_slice(&[0, 0xff, 0xff, 0x00, 0x50]);
        bytes.extend_from_slice(UUID::parse_str(AGENT_ID).expect("valid agent UUID").as_bytes());
        bytes.extend_from_slice(UUID::parse_str(SESSION_ID).expect("valid session UUID").as_bytes());
        bytes.extend_from_slice(&(message.len() as u16 + 1).to_le_bytes());
        bytes.extend_from_slice(message.as_bytes());
        bytes.push(0);
        bytes.push(chat_type);
        bytes.extend_from_slice(&channel.to_le_bytes());
        bytes
    }

    fn set_sequence(bytes: &mut [u8], sequence: u32) {
        bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
    }

    fn start_ping_packet(sequence: u32, ping_id: u8, oldest_unacked: u32) -> Vec<u8> {
        let mut bytes = fixture("start_ping_check.hex");
        set_sequence(&mut bytes, sequence);
        bytes[7] = ping_id;
        bytes[8..12].copy_from_slice(&oldest_unacked.to_le_bytes());
        bytes
    }

    fn server_ack(sequence: u32, acknowledged: &[u32]) -> Vec<u8> {
        let mut bytes = vec![0];
        bytes.extend_from_slice(&sequence.to_be_bytes());
        bytes.extend_from_slice(&[0, 0xff, 0xff, 0xff, 0xfb, acknowledged.len() as u8]);
        for id in acknowledged {
            bytes.extend_from_slice(&id.to_le_bytes());
        }
        bytes
    }

    fn client_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
        server_ack(sequence, &[acknowledged])
    }

    fn pending_packets(ids: &[u32]) -> HashMap<u32, PendingPacket> {
        ids.iter()
            .copied()
            .map(|id| (id, PendingPacket { bytes: Vec::new(), sent_at: Instant::now() - DEFAULT_RETRY_TIMEOUT, retries: 1 }))
            .collect()
    }

    #[test]
    fn chat_bytes_match_fixture() {
        assert_eq!(expected_chat(0x0102_0304, "abc", 2, 42), fixture("chat_from_viewer.hex"));
    }

    #[test]
    fn ack_bytes_match_fixture() {
        assert_eq!(server_ack(0x0102_0304, &[0x1122_3344, 0xa1b2_c3d4]), fixture("packet_ack.hex"));
    }
}

#[cfg(test)]
mod simulator_tests {
    use std::{
        collections::{HashMap, HashSet, VecDeque},
        future::pending,
        io::ErrorKind,
        net::SocketAddr,
        sync::Arc,
        time::Duration,
    };

    use crate::{
        AgentSession, Error, Grid, LoginResponseData,
        capabilities::{CapabilityBudget, Client, EventQueueExit},
        circuit::{Delivery, KEEPALIVE_TIMEOUT},
        client::{COMMAND_CAPACITY, Command},
        event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, PacketOrder::Ordered, SimulatorId},
        simulator::SimulatorConnection,
        status::NetworkStatusHandle,
    };
    use base::UUID;
    use llsd::Value;
    use protocol::{
        BlockValues, FieldValue, SchemaMessage,
        lludp::{Message, Packet},
        messages::{
            AgentMovementComplete, CacheMissType, ObjectRequest, PacketAck, PingId, RequestAgentData, RequestMultipleObjects,
            StartPingCheck,
            agent_movement_complete::{AgentData, Data, SimData},
            packet_ack::Packets,
        },
    };
    use test_support::{lludp::vectors, parse_hex_fixture};
    use tokio::{
        net::UdpSocket,
        spawn,
        sync::{Semaphore, mpsc, oneshot, watch},
        task::{JoinHandle, JoinSet},
        time::{advance, sleep, timeout},
    };
    use url::Url;

    const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
    const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
    const CIRCUIT_CODE: u32 = 0x1122_3344;
    const WAIT: Duration = Duration::from_secs(2);
    const SESSION_EVENT_CAPACITY: usize = 2;
    const TEST_EVENT_CAPACITY: usize = CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY;

    use super::{
        EVENT_BACKLOG_DISCONNECT_REASON, EVENT_BACKLOG_WARNING_THRESHOLD, HandoffTarget, LOGOUT_TIMEOUT, MAX_NEIGHBOR_CONNECTION_TASKS,
        MAX_NEIGHBOR_SIMULATORS, MAX_PENDING_SIMULATOR_EVENTS, MAX_PRECONNECT_EVENTS, MAX_QUEUED_SIMULATOR_EVENTS,
        NEIGHBOR_COMMAND_CAPACITY, NeighborConnection, NeighborConnectionState, NeighborEvent, PreconnectEvents, PreconnectOutcome,
        PrimaryEventBacklog, SimulatorCapabilitySession, primary_event_queue_size, queue_neighbor_event, queue_until_connected,
        run_session, run_session_with_status,
    };

    #[tokio::test(start_paused = true)]
    async fn primary_udp_activity_resets_keepalive_timeout() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let endpoint = simulator.local_addr().expect("read simulator address");
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
        let (_disconnect, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(login_session(endpoint), command_receiver, event_sender, disconnect_requested));

        let (_, peer) = receive(&simulator).await;
        simulator.send_to(&ack(20, 1), peer).await.expect("ack primary circuit");
        receive_message(&simulator, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
        simulator.send_to(&ack(21, 2), peer).await.expect("ack initial movement");

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        simulator.send_to(&handshake, peer).await.expect("send primary handshake");
        let reply = receive_message(&simulator, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        simulator.send_to(&ack(25, reply.sequence()), peer).await.expect("ack primary handshake reply");

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
        loop {
            if matches!(next_raw_event(&mut events).await, Event::SessionReady { .. }) {
                break;
            }
        }

        advance(KEEPALIVE_TIMEOUT - Duration::from_secs(1)).await;
        simulator.send_to(&start_ping_packet(24, 9, 0), peer).await.expect("send valid simulator traffic");
        receive_message(&simulator, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 9)).await;
        advance(KEEPALIVE_TIMEOUT - Duration::from_secs(1)).await;
        assert!(!client.is_finished(), "valid inbound LLUDP activity must renew the primary inactivity deadline");

        advance(Duration::from_secs(2)).await;
        let result = client.await.expect("primary session task");
        assert!(matches!(result, Err(reason) if reason == "The simulator stopped responding to LLUDP keepalive traffic."));
        drop(commands);
    }

    #[tokio::test(start_paused = true)]
    async fn neighbor_circuit_fails_after_udp_inactivity() {
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = neighbor.local_addr().expect("read neighbor address");
        let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
        let connection =
            NeighborConnection::connect(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000, None, NetworkStatusHandle::default())
                .await
                .expect("connect neighbor circuit");
        let (events, mut event_receiver) = mpsc::channel(4);
        let (_shutdown_signal, shutdown) = watch::channel(false);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(4);
        let (_command_sender, commands) = mpsc::channel(4);
        let worker = spawn(connection.run(events, shutdown, neighbor_requests, commands));

        let (use_circuit, peer) = receive(&neighbor).await;
        assert!(matches!(Packet::decode(&use_circuit).expect("decode UseCircuitCode").message(), Message::UseCircuitCode(_)));
        let acknowledgement = Packet::new(40, Message::PacketAck(PacketAck { packets: vec![Packets { id: 1 }] }))
            .encode()
            .expect("encode circuit acknowledgement");
        neighbor.send_to(&acknowledgement, peer).await.expect("ack neighbor circuit");
        assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { region_handle: Some(_), .. }));

        advance(KEEPALIVE_TIMEOUT).await;
        let result = worker.await.expect("neighbor task");
        assert!(matches!(result, Err(reason) if reason == "The simulator stopped responding to LLUDP keepalive traffic."));
    }

    #[test]
    fn backlog_warning_is_rate_limited() {
        let start = tokio::time::Instant::now();
        let mut backlog = PrimaryEventBacklog::default();

        assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD - 1, start).is_none());
        assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_millis(3)), Some(3));
        assert!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10)).is_none());
        assert_eq!(
            backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(10) + Duration::from_millis(3)),
            Some(10_003)
        );
    }

    #[test]
    fn backlog_timer_resets_after_drain() {
        let start = tokio::time::Instant::now();
        let mut backlog = PrimaryEventBacklog::default();

        assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start), Some(0));
        backlog.observe(0, start + Duration::from_secs(1));
        assert_eq!(backlog.observe(EVENT_BACKLOG_WARNING_THRESHOLD, start + Duration::from_secs(11)), Some(0));
    }

    #[tokio::test]
    async fn event_queue_reserves_terminal_slot() {
        let (events, _received) = mpsc::channel(4);
        let _terminal_slot = events.clone().reserve_owned().await.expect("reserve terminal event slot");
        events.try_send(EventEnvelope::new(Event::Failed("queued".into()))).expect("queue an application event");

        assert_eq!(
            primary_event_queue_size(&events, 3),
            (4, 3 + MAX_QUEUED_SIMULATOR_EVENTS),
            "occupancy excludes the terminal reservation and includes the pending deque"
        );
    }

    async fn terminal_after_full_queue(result: Result<(), String>) -> (Event, SimulatorId, SocketAddr) {
        let (events, mut received) = mpsc::channel(2);
        let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
        let endpoint = "127.0.0.1:9000".parse().expect("parse simulator endpoint");
        let terminal_slot = events.clone().try_reserve_owned().expect("reserve terminal event slot");
        events.try_send(EventEnvelope::new(Event::Failed("queued before disconnect".into()))).expect("fill the nonterminal slot");
        assert_eq!(events.capacity(), 0, "the ordinary event capacity is full");

        SimulatorConnection::publish_primary_terminal_event(terminal_slot, simulator_id, None, endpoint, &result);

        assert!(matches!(
            timeout(WAIT, received.recv()).await.expect("queued event should remain observable").map(EventEnvelope::into_event),
            Some(Event::Failed(message)) if message == "queued before disconnect"
        ));
        let terminal_event = timeout(WAIT, received.recv()).await.expect("terminal event should remain observable");
        (terminal_event.expect("terminal event remains observable").into_event(), simulator_id, endpoint)
    }

    #[tokio::test]
    async fn primary_disconnect_survives_full_queue() {
        let (event, simulator_id, endpoint) = terminal_after_full_queue(Ok(())).await;
        assert!(matches!(
            event,
            Event::SimulatorDisconnected { simulator_id: actual_id, region_handle: None, endpoint: actual_endpoint }
                if actual_id == simulator_id && actual_endpoint == endpoint
        ));
    }

    #[tokio::test]
    async fn primary_failure_survives_full_queue() {
        let (event, simulator_id, endpoint) = terminal_after_full_queue(Err("simulator failed".into())).await;
        assert!(matches!(
            event,
            Event::SimulatorFailed { simulator_id: actual_id, region_handle: None, endpoint: actual_endpoint, reason }
                if actual_id == simulator_id && actual_endpoint == endpoint && reason == "simulator failed"
        ));
    }

    #[tokio::test]
    async fn full_queue_keeps_control_responsive() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let endpoint = simulator.local_addr().expect("read simulator address");
        let connection = SimulatorConnection::connect_with_status(
            login_session(endpoint),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect simulator circuit");
        let (_, peer) = receive(&simulator).await;

        let (events, mut received) = mpsc::channel(2);
        let terminal_event_permit = events.clone().try_reserve_owned().expect("reserve terminal event slot");
        events.try_send(EventEnvelope::new(Event::Failed("fill event queue".into()))).expect("fill event queue");
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (disconnect, disconnect_requested) = watch::channel(false);
        let run = spawn(connection.run(command_receiver, events, disconnect_requested, terminal_event_permit));

        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, 1);
        simulator.send_to(&chat, peer).await.expect("send reliable data while event queue is full");
        simulator.send_to(&start_ping_packet(2, 8, 0), peer).await.expect("send ping while event queue is full");
        let mut chat_acknowledged = false;
        let mut ping_answered = false;
        timeout(WAIT, async {
            while !(chat_acknowledged && ping_answered) {
                let (bytes, _) = receive(&simulator).await;
                let packet = Packet::decode(&bytes).expect("decode primary control reply");
                chat_acknowledged |= packet.appended_acks().contains(&1)
                    || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|item| item.id == 1));
                ping_answered |= matches!(packet.message(), Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 8);
            }
        })
        .await
        .expect("primary must ACK reliable data and answer pings while application delivery is blocked");
        assert!(!run.is_finished(), "simulator should stay active until shutdown");
        disconnect.send_replace(true);
        receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(WAIT, run).await.expect("shutdown should finish with a full event queue").expect("simulator task").is_ok());
        assert!(
            matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(message)) if message == "fill event queue")
        );
        assert!(matches!(
            received.recv().await.map(EventEnvelope::into_event),
            Some(Event::SimulatorDisconnected { region_handle: None, endpoint: actual_endpoint, .. }) if actual_endpoint == endpoint
        ));
        assert!(timeout(WAIT, received.recv()).await.expect("event stream should close").is_none());
    }

    #[tokio::test]
    async fn capability_events_wait_for_simulator_connection() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let endpoint = simulator.local_addr().expect("read simulator address");
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(endpoint),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect simulator circuit");
        let simulator_id = connection.simulator_id;
        let (events, mut received) = mpsc::channel(4);
        let mut pending_events = VecDeque::new();
        let mut preconnect_events = PreconnectEvents::default();
        let mut connected_simulators = HashSet::new();
        let mut neighbors = HashMap::new();
        let mut capabilities = HashMap::new();
        let mut drop_application_messages = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;
        let queued_bytes = Arc::new(Semaphore::new(10));
        let mut first_event_permit = Some(queued_bytes.clone().acquire_many_owned(7).await.expect("reserve capability event bytes"));

        for (index, message) in ["First", "Second"].into_iter().enumerate() {
            let event = Event::CapabilityEvent { simulator_id, source: endpoint, message: message.to_owned(), body: Value::Undefined };
            let envelope = if index == 0 {
                EventEnvelope::with_capability_bytes(event, first_event_permit.take().expect("first event permit"))
            } else {
                EventEnvelope::new(event)
            };
            connection
                .publish_simulator_event(
                    simulator_id,
                    envelope,
                    &events,
                    &mut pending_events,
                    &mut preconnect_events,
                    &mut connected_simulators,
                    &mut neighbors,
                    &mut capabilities,
                    &mut drop_application_messages,
                    &mut logout_deadline,
                    &mut disconnect_reason,
                )
                .await
                .expect("buffer pre-connect capability event");
        }
        assert_eq!(preconnect_events.len, 2);
        assert_eq!(queued_bytes.available_permits(), 3, "the preconnect buffer must retain capability bytes");
        assert!(received.try_recv().is_err(), "capability events must wait for SimulatorConnected");

        connection
            .publish_simulator_connected(
                simulator_id,
                Event::SimulatorConnected { simulator_id, region_handle: None, endpoint },
                &events,
                &mut pending_events,
                &mut preconnect_events,
                &mut connected_simulators,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("publish connection before buffered events");
        assert_eq!(queued_bytes.available_permits(), 3, "publishing must transfer the permit with its event");

        assert!(
            matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::SimulatorConnected { simulator_id: id, .. }) if id == simulator_id)
        );
        assert_eq!(queued_bytes.available_permits(), 3);
        for expected in ["First", "Second"] {
            assert!(matches!(
                received.recv().await.map(EventEnvelope::into_event),
                Some(Event::CapabilityEvent { simulator_id: id, message, .. }) if id == simulator_id && message == expected
            ));
            if expected == "First" {
                assert_eq!(queued_bytes.available_permits(), 10, "recv_event handoff must release retained bytes");
            }
        }
        assert_eq!(preconnect_events.len, 0);
    }

    #[tokio::test]
    async fn primary_intro_precedes_early_udp_data() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
        let (disconnect, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(
            login_session(simulator.local_addr().expect("primary simulator address")),
            command_receiver,
            event_sender,
            disconnect_requested,
        ));

        let (_, peer) = receive(&simulator).await;
        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, 19);
        simulator.send_to(&chat, peer).await.expect("send data before circuit acknowledgement");
        sleep(Duration::from_millis(50)).await;
        assert!(events.try_recv().is_err(), "primary application data must wait for the simulator introduction");

        simulator.send_to(&ack(20, 1), peer).await.expect("ack primary circuit");
        receive_message(&simulator, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
        let first = next_raw_event(&mut events).await;
        let simulator_id = match first {
            Event::SimulatorConnected { simulator_id, region_handle: None, .. } => simulator_id,
            event => panic!("expected primary introduction before other events, got {event:?}"),
        };
        assert!(matches!(
            next_raw_event(&mut events).await,
            Event::SimulatorMessage { simulator_id: id, sequence: 19, message: Message::ChatFromSimulator(_), .. }
                if id == simulator_id
        ));

        disconnect.send_replace(true);
        receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(Duration::from_millis(200), client).await.expect("disconnect should finish").expect("client task").is_ok());
        drop(commands);
    }

    #[test]
    fn preconnect_buffer_is_bounded() {
        let simulator_id = SimulatorId::new(51);
        let mut pending = PreconnectEvents::default();
        for _ in 0..MAX_PRECONNECT_EVENTS {
            assert!(matches!(
                queue_until_connected(&HashSet::new(), &mut pending, simulator_id, EventEnvelope::new(Event::Failed("pending".into()))),
                PreconnectOutcome::Buffered
            ));
        }
        let overflow =
            queue_until_connected(&HashSet::new(), &mut pending, simulator_id, EventEnvelope::new(Event::Failed("overflow".into())));
        assert!(matches!(overflow, PreconnectOutcome::Full));
        assert_eq!(pending.len, MAX_PRECONNECT_EVENTS);
    }

    #[tokio::test]
    async fn simulator_removal_releases_preconnect_bytes() {
        let simulator_id = SimulatorId::new(52);
        let budget = Arc::new(Semaphore::new(10));
        let permit = budget.clone().acquire_many_owned(7).await.expect("reserve event bytes");
        let mut pending = PreconnectEvents::default();
        assert!(pending.push(
            simulator_id,
            EventEnvelope::with_capability_bytes(
                Event::CapabilityEvent {
                    simulator_id,
                    source: "127.0.0.1:13001".parse().expect("simulator address"),
                    message: "Pending".into(),
                    body: Value::Undefined,
                },
                permit,
            ),
        ));
        assert_eq!(budget.available_permits(), 3);

        pending.remove(simulator_id);
        assert_eq!(pending.len, 0);
        assert_eq!(budget.available_permits(), 10);
    }

    #[tokio::test]
    async fn replacing_neighbor_releases_queued_state() {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let old_neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind old neighbor");
        let new_neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement neighbor");
        let old_endpoint = old_neighbor.local_addr().expect("read old neighbor address");
        let new_endpoint = new_neighbor.local_addr().expect("read replacement address");
        let region_handle = 0x1000_0000_0000_1000;
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(primary.local_addr().expect("primary simulator address")),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect primary circuit");
        let _ = receive(&primary).await;
        let (application_events, _application_event_receiver) = mpsc::channel(8);
        let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<NeighborEvent>(8);
        let (_shutdown_signal, shutdown) = watch::channel(false);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(8);
        let mut neighbors = JoinSet::new();
        let mut connections = HashMap::new();
        let mut retiring_simulators = HashSet::new();
        let mut neighbor_task_owners = HashMap::new();
        let mut preconnect_events = PreconnectEvents::default();
        let mut simulator_capabilities = HashMap::new();
        let mut pending_events = VecDeque::new();
        let mut drop_application_messages = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;

        connection
            .start_neighbor(
                connection.simulator_id,
                region_handle,
                old_endpoint,
                None,
                &mut neighbors,
                &mut connections,
                &mut retiring_simulators,
                &mut neighbor_task_owners,
                &mut preconnect_events,
                &mut simulator_capabilities,
                neighbor_requests.clone(),
                application_events.clone(),
                neighbor_events.clone(),
                shutdown.clone(),
                &mut pending_events,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("start initial neighbor");
        let _ = receive(&old_neighbor).await;
        let old_id = connections.get(&region_handle).expect("old neighbor is tracked").simulator_id;

        let queued_bytes = Arc::new(Semaphore::new(10));
        let retained = queued_bytes.clone().acquire_many_owned(7).await.expect("reserve queued capability bytes");
        assert!(preconnect_events.push(
            old_id,
            EventEnvelope::with_capability_bytes(
                Event::CapabilityEvent {
                    simulator_id: old_id,
                    source: old_endpoint,
                    message: "EventQueueGet".into(),
                    body: Value::Undefined
                },
                retained,
            ),
        ));
        let seed_capability = Url::parse("http://127.0.0.1:1/seed").expect("seed URL");
        simulator_capabilities.insert(
            old_endpoint,
            SimulatorCapabilitySession {
                simulator_id: old_id,
                seed_capability: seed_capability.clone(),
                client: Client::new_with_status_and_budget(seed_capability, None, NetworkStatusHandle::default(), CapabilityBudget::new())
                    .expect("create old capability client"),
            },
        );
        assert_eq!(queued_bytes.available_permits(), 3);

        connection
            .start_neighbor(
                connection.simulator_id,
                region_handle,
                new_endpoint,
                None,
                &mut neighbors,
                &mut connections,
                &mut retiring_simulators,
                &mut neighbor_task_owners,
                &mut preconnect_events,
                &mut simulator_capabilities,
                neighbor_requests,
                application_events,
                neighbor_events,
                shutdown,
                &mut pending_events,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("replace unannounced neighbor");
        let _ = receive(&new_neighbor).await;

        assert_ne!(connections.get(&region_handle).expect("replacement remains tracked").simulator_id, old_id);
        assert!(retiring_simulators.contains(&old_id));
        assert_eq!(preconnect_events.len, 0, "replacement drops old pre-connect events immediately");
        assert_eq!(queued_bytes.available_permits(), 10, "replacement releases retained event bytes immediately");
        assert!(!simulator_capabilities.contains_key(&old_endpoint), "replacement stops the old capability session immediately");

        neighbors.abort_all();
        while neighbors.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn preconnect_overflow_is_neighbor_scoped() {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let primary_endpoint = primary.local_addr().expect("read primary address");
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(primary_endpoint),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect primary circuit");
        let simulator_id = SimulatorId::new(53);
        let neighbor_endpoint = "127.0.0.1:13003".parse().expect("parse neighbor endpoint");
        let region_handle = 0x1000_0000_0000_1003;
        let neighbor_task = spawn(pending::<()>());
        let (neighbor_commands, _neighbor_command_receiver) = mpsc::channel(1);
        let mut neighbors = HashMap::from([(
            region_handle,
            NeighborConnectionState {
                simulator_id,
                endpoint: neighbor_endpoint,
                abort: neighbor_task.abort_handle(),
                commands: neighbor_commands,
            },
        )]);
        let mut preconnect_events = PreconnectEvents::default();
        for _ in 0..MAX_PRECONNECT_EVENTS {
            assert!(preconnect_events.push(simulator_id, EventEnvelope::new(Event::Failed("buffered".into()))));
        }
        let (events, mut received) = mpsc::channel(1);
        let mut pending_events = VecDeque::new();
        let mut connected_simulators = HashSet::new();
        let mut capabilities = HashMap::new();
        let mut drop_application_messages = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;

        connection
            .fail_preconnect_overflow(
                simulator_id,
                &events,
                &mut pending_events,
                &mut preconnect_events,
                &mut connected_simulators,
                &mut neighbors,
                &mut capabilities,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("neighbor overflow should not log out the primary simulator");

        assert!(neighbors.is_empty(), "overflow must remove the affected neighbor");
        assert!(preconnect_events.by_simulator.is_empty());
        assert_eq!(preconnect_events.len, 0);
        assert!(!connected_simulators.contains(&simulator_id));
        assert!(neighbor_task.await.expect_err("overflow should cancel the neighbor worker").is_cancelled());
        assert!(matches!(
            received.recv().await.map(EventEnvelope::into_event),
            Some(Event::SimulatorFailed { simulator_id: failed_id, region_handle: Some(handle), endpoint, .. })
                if failed_id == simulator_id && handle == region_handle && endpoint == neighbor_endpoint
        ));
    }

    #[tokio::test]
    async fn neighbor_circuits_are_independent() {
        let mut harness = Box::pin(connected_neighbor()).await;
        let neighbor_peer = harness.neighbor_peer;
        let neighbor_id = harness.neighbor_id;
        assert_ne!(harness.primary_id, neighbor_id);

        let mut neighbor_handshake = fixture("region_handshake.hex");
        neighbor_handshake[0] |= 0x40;
        set_sequence(&mut neighbor_handshake, 41);
        harness.neighbor.send_to(&neighbor_handshake, neighbor_peer).await.expect("send neighbor handshake");
        let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        assert!(reply.appended_acks().contains(&41));
        assert!(matches!(
            next_event(&mut harness.events).await,
            Event::SimulatorMessage { simulator_id, source, sequence: 41, message: Message::RegionHandshake(_), .. }
                if simulator_id == neighbor_id && source == harness.neighbor.local_addr().expect("neighbor address")
        ));

        let mut disable = fixture("disable_simulator.hex");
        set_sequence(&mut disable, 42);
        harness.neighbor.send_to(&disable, neighbor_peer).await.expect("disable neighbor circuit");
        loop {
            if matches!(next_event(&mut harness.events).await, Event::SimulatorDisconnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), .. } if simulator_id == neighbor_id)
            {
                break;
            }
        }

        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, 43);
        harness.neighbor.send_to(&chat, neighbor_peer).await.expect("send after neighbor disabled");
        let mut buffer = [0; 65_535];
        match timeout(Duration::from_millis(50), harness.neighbor.recv_from(&mut buffer)).await {
            Err(_) | Ok(Err(_)) => {}
            Ok(Ok((size, _))) => {
                panic!("disabled circuit sent another packet: {:?}", Packet::decode(&buffer[..size]).expect("decode packet").message())
            }
        }

        harness.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn neighbor_delivers_accepted_data_before_terminal() {
        let mut harness = Box::pin(connected_neighbor()).await;

        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, 41);
        harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send final neighbor message");

        let mut disable = fixture("disable_simulator.hex");
        set_sequence(&mut disable, 42);
        harness.neighbor.send_to(&disable, harness.neighbor_peer).await.expect("close neighbor circuit");

        assert!(matches!(
            next_event(&mut harness.events).await,
            Event::SimulatorMessage { simulator_id, sequence: 41, message: Message::ChatFromSimulator(_), .. }
                if simulator_id == harness.neighbor_id
        ));
        assert!(matches!(
            next_event(&mut harness.events).await,
            Event::SimulatorDisconnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), .. }
                if simulator_id == harness.neighbor_id
        ));

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn object_local_ids_are_scoped_to_simulators() {
        let mut harness = Box::pin(connected_neighbor()).await;

        let mut primary_update = fixture("object_update.hex");
        set_sequence(&mut primary_update, 44);
        harness.primary.send_to(&primary_update, harness.primary_peer).await.expect("send primary object update");

        let mut neighbor_update = fixture("object_update.hex");
        set_sequence(&mut neighbor_update, 45);
        harness.neighbor.send_to(&neighbor_update, harness.neighbor_peer).await.expect("send neighbor object update");

        let mut received_simulators = Vec::with_capacity(2);
        timeout(WAIT, async {
            while received_simulators.len() < 2 {
                if let Event::SimulatorMessage { simulator_id, message: Message::ObjectUpdate(update), .. } =
                    next_event(&mut harness.events).await
                {
                    assert_eq!(update.objects[0].local_id, 0x1020_3040);
                    received_simulators.push(simulator_id);
                }
            }
        })
        .await
        .expect("receive the object update from both circuits");

        assert!(received_simulators.contains(&harness.primary_id));
        assert!(received_simulators.contains(&harness.neighbor_id));
        assert_ne!(harness.primary_id, harness.neighbor_id);
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn neighbor_targets_and_replacements() {
        let mut harness = Box::pin(connected_neighbor()).await;
        receive_ack(&harness.primary, 23).await;
        send_targeted(&harness.commands, harness.neighbor_id, object_request())
            .await
            .expect("route an object request to the selected simulator");
        let (_, sent_peer) =
            receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert_eq!(sent_peer, harness.neighbor_peer);
        let mut buffer = [0; 65_535];
        assert!(timeout(Duration::from_millis(50), harness.primary.recv_from(&mut buffer)).await.is_err());

        let mut disable = fixture("disable_simulator.hex");
        set_sequence(&mut disable, 42);
        harness.neighbor.send_to(&disable, harness.neighbor_peer).await.expect("disable neighbor circuit");
        loop {
            if matches!(next_event(&mut harness.events).await, Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == harness.neighbor_id)
            {
                break;
            }
        }
        assert!(matches!(send_targeted(&harness.commands, harness.neighbor_id, object_request()).await, Err(Error::SimulatorUnavailable)));

        let neighbor_endpoint = harness.neighbor.local_addr().expect("neighbor address");
        announce_neighbor(&harness.primary, harness.primary_peer, neighbor_endpoint, 0x1000_0000_0000_1000, 50).await;
        let (bytes, replacement_peer) = receive(&harness.neighbor).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode replacement circuit packet").message(), Message::UseCircuitCode(_)));
        harness.neighbor.send_to(&ack(60, 1), replacement_peer).await.expect("ack replacement circuit");
        let replacement_id = loop {
            match next_event(&mut harness.events).await {
                Event::SimulatorConnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), endpoint }
                    if endpoint == neighbor_endpoint =>
                {
                    break simulator_id;
                }
                _ => {}
            }
        };
        assert_ne!(replacement_id, harness.neighbor_id);
        assert!(matches!(send_targeted(&harness.commands, harness.neighbor_id, object_request()).await, Err(Error::SimulatorUnavailable)));
        send_targeted(&harness.commands, replacement_id, object_request()).await.expect("send to replacement");
        let (_, sent_peer) =
            receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert_eq!(sent_peer, replacement_peer);
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn old_neighbor_completion_keeps_replacement_route() {
        let mut harness = Box::pin(connected_neighbor()).await;
        let replacement = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement neighbor");
        let replacement_endpoint = replacement.local_addr().expect("replacement address");
        let region_handle = 0x1000_0000_0000_1000;

        announce_neighbor(&harness.primary, harness.primary_peer, replacement_endpoint, region_handle, 50).await;
        let (bytes, replacement_peer) = receive(&replacement).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode replacement circuit packet").message(), Message::UseCircuitCode(_)));
        replacement.send_to(&ack(60, 1), replacement_peer).await.expect("ack replacement circuit");

        let mut replacement_id = None;
        let mut old_disconnected = false;
        timeout(WAIT, async {
            while replacement_id.is_none() || !old_disconnected {
                match next_event(&mut harness.events).await {
                    Event::SimulatorConnected { simulator_id, region_handle: Some(handle), endpoint }
                        if handle == region_handle && endpoint == replacement_endpoint =>
                    {
                        replacement_id = Some(simulator_id);
                    }
                    Event::SimulatorDisconnected { simulator_id, region_handle: Some(handle), .. }
                        if simulator_id == harness.neighbor_id && handle == region_handle =>
                    {
                        old_disconnected = true;
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("observe old completion and replacement introduction");

        let replacement_id = replacement_id.expect("replacement connection ID");
        assert_ne!(replacement_id, harness.neighbor_id);
        send_targeted(&harness.commands, replacement_id, object_request())
            .await
            .expect("old completion must preserve the replacement route");
        let (_, sent_peer) = receive_message_with_peer(&replacement, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert_eq!(sent_peer, replacement_peer);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn neighbor_endpoint_validation() {
        let mut harness = Box::pin(connected_neighbor()).await;
        let primary_endpoint = harness.primary.local_addr().expect("primary simulator address");
        let neighbor_endpoint = harness.neighbor.local_addr().expect("neighbor simulator address");
        let candidates = [
            (0x2000_0000_0000_0000, primary_endpoint),
            (0x3000_0000_0000_0000, neighbor_endpoint),
            (0x4000_0000_0000_0000, SocketAddr::from(([127, 0, 0, 1], 0))),
            (0x5000_0000_0000_0000, SocketAddr::from(([0, 0, 0, 0], 13000))),
            (0x6000_0000_0000_0000, SocketAddr::from(([224, 0, 0, 1], 13000))),
            (0x7000_0000_0000_0000, SocketAddr::from(([255, 255, 255, 255], 13000))),
        ];

        for (index, (region_handle, endpoint)) in candidates.into_iter().enumerate() {
            announce_neighbor(&harness.primary, harness.primary_peer, endpoint, region_handle, 50 + index as u32).await;
            let failed_endpoint = timeout(WAIT, async {
                loop {
                    if let Event::SimulatorConnectFailed { region_handle: failed_region, endpoint, .. } =
                        next_event(&mut harness.events).await
                        && failed_region == region_handle
                    {
                        break endpoint;
                    }
                }
            })
            .await
            .expect("invalid or duplicate endpoint should fail before starting a connection");
            assert_eq!(failed_endpoint, endpoint);
        }

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn duplicate_replacement_keeps_old_neighbor() {
        let mut harness = Box::pin(connected_neighbor()).await;
        let other = UdpSocket::bind("127.0.0.1:0").await.expect("bind other neighbor");
        let other_endpoint = other.local_addr().expect("read other neighbor address");
        let other_region = 0x2000_0000_0000_2000;
        announce_neighbor(&harness.primary, harness.primary_peer, other_endpoint, other_region, 50).await;
        let (bytes, other_peer) = receive(&other).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode other neighbor bootstrap").message(), Message::UseCircuitCode(_)));
        other.send_to(&ack(60, 1), other_peer).await.expect("ack other neighbor bootstrap");
        loop {
            if matches!(
                next_event(&mut harness.events).await,
                Event::SimulatorConnected { region_handle: Some(handle), endpoint, .. }
                    if handle == other_region && endpoint == other_endpoint
            ) {
                break;
            }
        }

        let old_id = harness.neighbor_id;
        let old_region = 0x1000_0000_0000_1000;
        announce_neighbor(&harness.primary, harness.primary_peer, other_endpoint, old_region, 51).await;
        timeout(WAIT, async {
            loop {
                if matches!(
                    next_event(&mut harness.events).await,
                    Event::SimulatorConnectFailed { region_handle, endpoint, .. }
                        if region_handle == old_region && endpoint == other_endpoint
                ) {
                    break;
                }
            }
        })
        .await
        .expect("duplicate endpoint replacement should be rejected");

        send_targeted(&harness.commands, old_id, object_request()).await.expect("the existing route should remain active");
        let (_, sent_peer) =
            receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert_eq!(sent_peer, harness.neighbor_peer);
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn enable_overflow_keeps_primary_responsive() {
        let mut harness = Box::pin(connected_neighbor()).await;
        let mut sockets = Vec::with_capacity(MAX_NEIGHBOR_SIMULATORS);
        for _ in 0..MAX_NEIGHBOR_SIMULATORS {
            sockets.push(UdpSocket::bind("127.0.0.1:0").await.expect("bind announced neighbor"));
        }
        let addresses = sockets.iter().map(|socket| socket.local_addr().expect("read neighbor address")).collect::<Vec<_>>();
        let regions = (0..MAX_NEIGHBOR_SIMULATORS).map(|index| 0x3000_0000_0000_0000 + index as u64).collect::<Vec<_>>();
        let first_sequence = 60;
        let primary = &harness.primary;
        let primary_peer = harness.primary_peer;
        let primary_id = harness.primary_id;
        let events = &mut harness.events;

        let send_announcements = async {
            for index in 0..addresses.len() {
                announce_neighbor(primary, primary_peer, addresses[index], regions[index], first_sequence + index as u32).await;
            }
        };
        let observe_announcements = async {
            let mut seen = vec![false; addresses.len()];
            let mut rejected_endpoint = None;
            timeout(WAIT, async {
                while seen.iter().any(|received| !received) || rejected_endpoint.is_none() {
                    match next_event(events).await {
                        Event::SimulatorMessage { simulator_id, sequence, message: Message::EnableSimulator(_), .. }
                            if simulator_id == primary_id
                                && (first_sequence..first_sequence + addresses.len() as u32).contains(&sequence) =>
                        {
                            seen[(sequence - first_sequence) as usize] = true;
                        }
                        Event::SimulatorConnectFailed { region_handle, endpoint, .. }
                            if region_handle == *regions.last().expect("one announcement") =>
                        {
                            rejected_endpoint = Some(endpoint);
                        }
                        _ => {}
                    }
                }
            })
            .await
            .expect("primary should process all neighbor announcements");
            (seen, rejected_endpoint)
        };

        let (_, (seen, rejected_endpoint)) = tokio::join!(send_announcements, observe_announcements);
        assert!(seen.into_iter().all(|received| received));
        assert_eq!(rejected_endpoint, addresses.last().copied());

        for socket in &sockets[..MAX_NEIGHBOR_SIMULATORS - 1] {
            let (bytes, _) = receive(socket).await;
            assert!(matches!(Packet::decode(&bytes).expect("decode admitted neighbor bootstrap").message(), Message::UseCircuitCode(_)));
        }
        let rejected = sockets.last().expect("the 33rd neighbor announcement");
        let mut buffer = [0; 65_535];
        assert!(timeout(Duration::from_millis(50), rejected.recv_from(&mut buffer)).await.is_err());

        harness.primary.send_to(&start_ping_packet(150, 7, 0), harness.primary_peer).await.expect("ping primary after announcement burst");
        assert!(matches!(
            receive_message(&harness.primary, |message| matches!(message, Message::CompletePingCheck(_))).await.message(),
            Message::CompletePingCheck(message) if message.ping_id.ping_id == 7
        ));
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn neighbor_shutdown_preempts_start() {
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
        let (event_sender, _event_receiver) = mpsc::channel(1);
        let (shutdown_signal, shutdown) = watch::channel(false);
        shutdown_signal.send_replace(true);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
        let (_command_sender, commands) = mpsc::channel(1);
        let neighbor_connection =
            NeighborConnection::connect(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000, None, NetworkStatusHandle::default())
                .await
                .expect("connect neighbor circuit");

        let result =
            timeout(Duration::from_millis(100), neighbor_connection.run(event_sender, shutdown, neighbor_requests, commands)).await;
        assert!(matches!(result, Ok(Ok(()))), "a pre-signaled shutdown should stop the neighbor immediately");
    }

    #[tokio::test]
    async fn neighbor_receiver_close_preempts() {
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
        let (event_sender, mut event_receiver) = mpsc::channel(1);
        let (_shutdown_signal, shutdown) = watch::channel(false);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
        let (_command_sender, commands) = mpsc::channel(1);
        let neighbor_connection =
            NeighborConnection::connect(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000, None, NetworkStatusHandle::default())
                .await
                .expect("connect neighbor circuit");
        let task = spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, commands));

        let (use_circuit, peer) = receive(&neighbor).await;
        assert!(matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_)));
        neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
        assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { .. }));

        drop(event_receiver);
        assert!(
            timeout(Duration::from_millis(200), task)
                .await
                .expect("neighbor should stop after event receiver closes")
                .expect("neighbor task")
                .is_ok()
        );
    }

    #[tokio::test]
    async fn neighbor_ping_is_delayed() {
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
        let (event_sender, mut event_receiver) = mpsc::channel(1);
        let (shutdown_signal, shutdown) = watch::channel(false);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
        let (_command_sender, commands) = mpsc::channel(1);
        let neighbor_connection =
            NeighborConnection::connect(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000, None, NetworkStatusHandle::default())
                .await
                .expect("connect neighbor circuit");
        let task = spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, commands));

        let (use_circuit, peer) = receive(&neighbor).await;
        assert!(matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_)));
        neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
        assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { .. }));

        let mut buffer = [0; 65_535];
        assert!(
            timeout(Duration::from_millis(100), neighbor.recv_from(&mut buffer)).await.is_err(),
            "the neighbor should wait for its ping interval after connecting"
        );

        shutdown_signal.send_replace(true);
        assert!(timeout(Duration::from_millis(200), task).await.expect("neighbor should stop on shutdown").expect("neighbor task").is_ok());
    }

    #[tokio::test]
    async fn handoff_waits_for_matching_movement() {
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination simulator");
        let endpoint = neighbor.local_addr().expect("destination simulator address");
        let session = login_session(endpoint);
        let region_handle = 0x1000_0000_0000_1000;
        let simulator_id = SimulatorId::allocate().expect("allocate destination ID");
        let (event_sender, mut event_receiver) = mpsc::channel(8);
        let (shutdown_signal, shutdown) = watch::channel(false);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
        let (_command_sender, commands) = mpsc::channel(1);
        let mut connection =
            NeighborConnection::connect(simulator_id, session.clone(), region_handle, None, NetworkStatusHandle::default())
                .await
                .expect("connect destination circuit");
        connection.handoff = Some(HandoffTarget {
            agent_id: session.agent_id,
            session_id: session.session_id,
            region_handle,
            endpoint,
            seed_capability: session.seed_capability.clone(),
        });
        let task = spawn(connection.run(event_sender, shutdown, neighbor_requests, commands));

        let (use_circuit, peer) = receive(&neighbor).await;
        assert!(matches!(Packet::decode(&use_circuit).expect("decode circuit packet").message(), Message::UseCircuitCode(_)));
        neighbor.send_to(&ack(40, 1), peer).await.expect("ack destination circuit");
        assert!(matches!(
            next_neighbor_event(&mut event_receiver).await,
            Event::SimulatorConnected { simulator_id: id, region_handle: Some(handle), .. } if id == simulator_id && handle == region_handle
        ));
        let mut buffer = [0; 65_535];
        assert!(timeout(Duration::from_millis(50), neighbor.recv_from(&mut buffer)).await.is_err());

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 41);
        neighbor.send_to(&handshake, peer).await.expect("send destination region handshake");
        assert!(matches!(
            receive_message(&neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await.message(),
            Message::RegionHandshakeReply(_)
        ));
        assert!(matches!(
            receive_message(&neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await.message(),
            Message::CompleteAgentMovement(_)
        ));
        assert!(matches!(
            next_neighbor_event(&mut event_receiver).await,
            Event::SimulatorMessage { message: Message::RegionHandshake(_), .. }
        ));

        let wrong = movement_complete_packet(42, session.agent_id, UUID::from_u128(99), region_handle);
        neighbor.send_to(&wrong, peer).await.expect("send wrong-session movement completion");
        assert!(timeout(Duration::from_millis(50), event_receiver.recv()).await.is_err());

        let matching = movement_complete_packet(43, session.agent_id, session.session_id, region_handle);
        neighbor.send_to(&matching, peer).await.expect("send matching movement completion");
        assert!(matches!(
            next_neighbor_event(&mut event_receiver).await,
            Event::SimulatorMessage { simulator_id: id, message: Message::AgentMovementComplete(_), .. } if id == simulator_id
        ));

        shutdown_signal.send_replace(true);
        assert!(timeout(Duration::from_millis(200), task).await.expect("destination stops").expect("destination task").is_ok());
    }

    #[tokio::test]
    async fn teleport_finish_promotes_after_matching_movement() {
        use crate::event::Event::{Failed, SimulatorMessage, SimulatorPromoted};
        use protocol::lludp::Message::{AgentMovementComplete, ChatFromSimulator, RegionHandshake, Schema};

        let mut harness = Box::pin(connected_neighbor()).await;
        let target_region = 0x1000_0000_0000_1000;
        let target_endpoint = harness.neighbor.local_addr().expect("destination simulator address");
        let session = login_session(harness.primary.local_addr().expect("primary simulator address"));
        let teleport_finish =
            Packet::new(30, teleport_finish_to(session.agent_id, target_endpoint, target_region, session.seed_capability.as_str()))
                .with_reliable()
                .encode()
                .expect("encode TeleportFinish");
        harness.primary.send_to(&teleport_finish, harness.primary_peer).await.expect("send TeleportFinish");

        timeout(WAIT, async {
            loop {
                match next_raw_event(&mut harness.events).await {
                    SimulatorMessage { simulator_id, message: Schema(schema), .. }
                        if simulator_id == harness.primary_id && schema.schema().name == "TeleportFinish" =>
                    {
                        break;
                    }
                    Failed(reason) => panic!("session failed before destination promotion: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("manager should process TeleportFinish");

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 41);
        harness.neighbor.send_to(&handshake, harness.neighbor_peer).await.expect("send destination handshake");
        let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        let movement_request = receive_message(&harness.neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
        harness.neighbor.send_to(&ack(42, reply.sequence()), harness.neighbor_peer).await.expect("ack destination handshake reply");
        harness
            .neighbor
            .send_to(&ack(43, movement_request.sequence()), harness.neighbor_peer)
            .await
            .expect("ack destination movement request");

        timeout(WAIT, async {
            loop {
                match next_raw_event(&mut harness.events).await {
                    SimulatorMessage { simulator_id, message: RegionHandshake(_), .. } if simulator_id == harness.neighbor_id => break,
                    Failed(reason) => panic!("session failed during destination handshake: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("manager should observe destination handshake");

        let wrong_region = target_region + 1;
        harness
            .neighbor
            .send_to(&movement_complete_packet(44, session.agent_id, session.session_id, wrong_region), harness.neighbor_peer)
            .await
            .expect("send movement completion for another region");
        receive_ack(&harness.neighbor, 44).await;
        let mut barrier = fixture("chat_from_simulator.hex");
        barrier[0] |= 0x40;
        set_sequence(&mut barrier, 45);
        harness.neighbor.send_to(&barrier, harness.neighbor_peer).await.expect("send post-movement barrier");
        receive_ack(&harness.neighbor, 45).await;
        timeout(WAIT, async {
            loop {
                match next_raw_event(&mut harness.events).await {
                    SimulatorMessage { simulator_id, message: ChatFromSimulator(_), .. } if simulator_id == harness.neighbor_id => break,
                    SimulatorPromoted { .. } => panic!("a mismatched movement completion must not promote the destination"),
                    Failed(reason) => panic!("session failed before the post-movement barrier: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("manager should process the post-movement barrier");
        assert_eq!(
            harness.status.snapshot().simulator_endpoint,
            Some(harness.primary.local_addr().expect("primary simulator address")),
            "a mismatched movement completion must leave the primary simulator unchanged"
        );

        harness
            .neighbor
            .send_to(&movement_complete_packet(46, session.agent_id, session.session_id, target_region), harness.neighbor_peer)
            .await
            .expect("send matching movement completion");
        let (previous, promoted, promoted_endpoint) = timeout(WAIT, async {
            let mut matching_movement_seen = false;
            loop {
                match next_raw_event(&mut harness.events).await {
                    SimulatorMessage { simulator_id, message: AgentMovementComplete(movement), .. }
                        if simulator_id == harness.neighbor_id && movement.data.region_handle == target_region =>
                    {
                        matching_movement_seen = true;
                    }
                    SimulatorPromoted { previous_simulator_id, simulator_id, session, .. } => {
                        assert!(matching_movement_seen, "promotion must follow the matching movement event");
                        break (previous_simulator_id, simulator_id, session.simulator_address);
                    }
                    Failed(reason) => panic!("session failed before promotion: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("matching destination movement should promote the simulator");

        assert_eq!(previous, harness.primary_id);
        assert_eq!(promoted, harness.neighbor_id);
        assert_eq!(promoted_endpoint, target_endpoint);
        let status = harness.status.snapshot();
        assert_eq!(status.simulator_endpoint, Some(target_endpoint));
        assert!(status.circuit_established, "promoted circuit should remain established");

        let mut disable = fixture("disable_simulator.hex");
        set_sequence(&mut disable, 47);
        harness.primary.send_to(&disable, harness.primary_peer).await.expect("disable former primary");
        timeout(WAIT, async {
            loop {
                match next_raw_event(&mut harness.events).await {
                    Event::SimulatorDisconnected { simulator_id, region_handle: None, .. } if simulator_id == harness.primary_id => break,
                    Failed(reason) => panic!("former primary failure ended the promoted session: {reason}"),
                    _ => {}
                }
            }
        })
        .await
        .expect("manager should isolate the former primary failure");
        let status = harness.status.snapshot();
        assert_eq!(status.simulator_endpoint, Some(target_endpoint));
        assert!(status.circuit_established, "former primary failure must keep the destination circuit established");

        harness
            .commands
            .send(Command::SendMessage { message: object_request(), delivery: Delivery::Reliable })
            .await
            .expect("send through promoted primary");
        let (_, route_peer) =
            receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert_eq!(route_peer, harness.neighbor_peer);

        drop(harness.commands);
        receive_message(&harness.neighbor, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(Duration::from_millis(200), harness.client).await.expect("logout should finish").expect("manager task").is_ok());
    }

    #[tokio::test]
    async fn neighbor_intro_precedes_early_udp_data() {
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
        let (event_sender, mut event_receiver) = mpsc::channel(4);
        let (shutdown_signal, shutdown) = watch::channel(false);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
        let (_command_sender, commands) = mpsc::channel(1);
        let connection =
            NeighborConnection::connect(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000, None, NetworkStatusHandle::default())
                .await
                .expect("connect neighbor circuit");
        let task = spawn(connection.run(event_sender, shutdown, neighbor_requests, commands));

        let (use_circuit, peer) = receive(&neighbor).await;
        assert!(matches!(Packet::decode(&use_circuit).expect("decode circuit packet").message(), Message::UseCircuitCode(_)));

        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, 41);
        neighbor.send_to(&chat, peer).await.expect("send data before circuit acknowledgement");
        assert!(
            timeout(Duration::from_millis(50), event_receiver.recv()).await.is_err(),
            "application data must wait for the simulator introduction"
        );

        neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
        assert!(matches!(
            next_neighbor_event(&mut event_receiver).await,
            Event::SimulatorConnected { simulator_id: id, .. } if id == simulator_id
        ));
        assert!(matches!(
            next_neighbor_event(&mut event_receiver).await,
            Event::SimulatorMessage { simulator_id: id, sequence: 41, message: Message::ChatFromSimulator(_), .. } if id == simulator_id
        ));

        shutdown_signal.send_replace(true);
        assert!(timeout(Duration::from_millis(200), task).await.expect("neighbor stops").expect("neighbor task").is_ok());
    }

    #[tokio::test]
    async fn neighbor_task_limit_preserves_current() {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let (application_events, mut received_events) = mpsc::channel(MAX_NEIGHBOR_CONNECTION_TASKS + 1);
        let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<NeighborEvent>(MAX_NEIGHBOR_CONNECTION_TASKS + 1);
        let (_shutdown_signal, shutdown) = watch::channel(false);
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(primary.local_addr().expect("primary simulator address")),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect primary circuit");
        let _ = receive(&primary).await;

        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
        let mut neighbors = JoinSet::new();
        let mut connections = HashMap::new();
        let mut retiring_simulators = HashSet::new();
        let mut neighbor_task_owners = HashMap::new();
        let mut preconnect_events = PreconnectEvents::default();
        let mut simulator_capabilities = HashMap::new();
        let mut pending_events = VecDeque::new();
        let mut drop_application_messages = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;
        let target_region = 0x1000_0000_0000_1000;
        let mut sockets = Vec::with_capacity(MAX_NEIGHBOR_CONNECTION_TASKS + 1);

        for index in 0..MAX_NEIGHBOR_SIMULATORS {
            let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
            let region_handle = if index == 0 { target_region } else { 0x2000_0000_0000_0000 + index as u64 };
            connection
                .start_neighbor(
                    connection.simulator_id,
                    region_handle,
                    socket.local_addr().expect("neighbor address"),
                    None,
                    &mut neighbors,
                    &mut connections,
                    &mut retiring_simulators,
                    &mut neighbor_task_owners,
                    &mut preconnect_events,
                    &mut simulator_capabilities,
                    neighbor_requests.clone(),
                    application_events.clone(),
                    neighbor_events.clone(),
                    shutdown.clone(),
                    &mut pending_events,
                    &mut drop_application_messages,
                    &mut logout_deadline,
                    &mut disconnect_reason,
                )
                .await
                .expect("start a neighbor circuit");
            let (bytes, _) = receive(&socket).await;
            assert!(matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_)));
            sockets.push(socket);
        }

        assert_eq!(connections.len(), MAX_NEIGHBOR_SIMULATORS);
        assert_eq!(neighbors.len(), MAX_NEIGHBOR_SIMULATORS);
        let mut current_peer = None;
        for _ in 0..MAX_NEIGHBOR_SIMULATORS {
            let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement simulator");
            let endpoint = socket.local_addr().expect("replacement address");
            connection
                .start_neighbor(
                    connection.simulator_id,
                    target_region,
                    endpoint,
                    None,
                    &mut neighbors,
                    &mut connections,
                    &mut retiring_simulators,
                    &mut neighbor_task_owners,
                    &mut preconnect_events,
                    &mut simulator_capabilities,
                    neighbor_requests.clone(),
                    application_events.clone(),
                    neighbor_events.clone(),
                    shutdown.clone(),
                    &mut pending_events,
                    &mut drop_application_messages,
                    &mut logout_deadline,
                    &mut disconnect_reason,
                )
                .await
                .expect("replace target neighbor");
            let (bytes, peer) = receive(&socket).await;
            assert!(matches!(Packet::decode(&bytes).expect("decode replacement bootstrap").message(), Message::UseCircuitCode(_)));
            current_peer = Some(peer);
            sockets.push(socket);
        }

        assert_eq!(connections.len(), MAX_NEIGHBOR_SIMULATORS);
        assert_eq!(neighbors.len(), MAX_NEIGHBOR_CONNECTION_TASKS);
        let current_id = connections.get(&target_region).expect("current target connection").simulator_id;
        let rejected = UdpSocket::bind("127.0.0.1:0").await.expect("bind rejected simulator");
        let rejected_endpoint = rejected.local_addr().expect("rejected endpoint");
        connection
            .start_neighbor(
                connection.simulator_id,
                target_region,
                rejected_endpoint,
                None,
                &mut neighbors,
                &mut connections,
                &mut retiring_simulators,
                &mut neighbor_task_owners,
                &mut preconnect_events,
                &mut simulator_capabilities,
                neighbor_requests.clone(),
                application_events.clone(),
                neighbor_events.clone(),
                shutdown.clone(),
                &mut pending_events,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("reject a replacement at the task cap");

        assert_eq!(connections.get(&target_region).expect("current target remains").simulator_id, current_id);
        assert_eq!(neighbors.len(), MAX_NEIGHBOR_CONNECTION_TASKS);
        let failure = loop {
            if let Event::SimulatorConnectFailed { simulator_id, region_handle, endpoint } =
                received_events.recv().await.expect("event channel remains open").into_event()
                && region_handle == target_region
                && endpoint == rejected_endpoint
            {
                break simulator_id;
            }
        };
        assert_ne!(failure, current_id);

        let current_socket = sockets.last().expect("last admitted target socket");
        current_socket
            .send_to(&start_ping_packet(200, 7, 0), current_peer.expect("target socket peer"))
            .await
            .expect("ping current target connection");
        assert!(matches!(
            receive_message(current_socket, |message| matches!(message, Message::CompletePingCheck(_))).await.message(),
            Message::CompletePingCheck(message) if message.ping_id.ping_id == 7
        ));
        let mut buffer = [0; 65_535];
        assert!(timeout(Duration::from_millis(50), rejected.recv_from(&mut buffer)).await.is_err());

        for _ in 0..MAX_NEIGHBOR_SIMULATORS {
            let Some(Err(error)) = neighbors.join_next().await else {
                panic!("replaced neighbor task should be joined as canceled");
            };
            assert!(error.is_cancelled());
        }
        assert_eq!(neighbors.len(), MAX_NEIGHBOR_SIMULATORS);

        let recovered = UdpSocket::bind("127.0.0.1:0").await.expect("bind recovered simulator");
        let recovered_endpoint = recovered.local_addr().expect("recovered endpoint");
        connection
            .start_neighbor(
                connection.simulator_id,
                target_region,
                recovered_endpoint,
                None,
                &mut neighbors,
                &mut connections,
                &mut retiring_simulators,
                &mut neighbor_task_owners,
                &mut preconnect_events,
                &mut simulator_capabilities,
                neighbor_requests,
                application_events,
                neighbor_events,
                shutdown,
                &mut pending_events,
                &mut drop_application_messages,
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("task capacity should recover after canceled workers are joined");
        let (bytes, _) = receive(&recovered).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode recovered neighbor").message(), Message::UseCircuitCode(_)));
        assert_eq!(connections.len(), MAX_NEIGHBOR_SIMULATORS);
        assert_eq!(neighbors.len(), MAX_NEIGHBOR_SIMULATORS + 1);
    }

    #[tokio::test]
    async fn neighbor_simulator_limit_is_bounded() {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let (application_events, mut received_events) = mpsc::channel(MAX_NEIGHBOR_SIMULATORS + 1);
        let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<NeighborEvent>(1);
        let (_shutdown_signal, shutdown) = watch::channel(false);
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(primary.local_addr().expect("primary simulator address")),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect primary circuit");
        let _ = receive(&primary).await;

        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
        let mut neighbors = JoinSet::new();
        let mut connections = HashMap::new();
        let mut retiring_simulators = HashSet::new();
        let mut neighbor_task_owners = HashMap::new();
        let mut preconnect_events = PreconnectEvents::default();
        let mut simulator_capabilities = HashMap::new();
        let mut pending_events = VecDeque::new();
        let mut drop_application_messages = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;
        let mut sockets = Vec::with_capacity(MAX_NEIGHBOR_SIMULATORS + 1);

        for index in 0..=MAX_NEIGHBOR_SIMULATORS {
            let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
            let endpoint = socket.local_addr().expect("neighbor address");
            let region_handle = 0x1000_0000_0000_0000 + index as u64;
            connection
                .start_neighbor(
                    connection.simulator_id,
                    region_handle,
                    endpoint,
                    None,
                    &mut neighbors,
                    &mut connections,
                    &mut retiring_simulators,
                    &mut neighbor_task_owners,
                    &mut preconnect_events,
                    &mut simulator_capabilities,
                    neighbor_requests.clone(),
                    application_events.clone(),
                    neighbor_events.clone(),
                    shutdown.clone(),
                    &mut pending_events,
                    &mut drop_application_messages,
                    &mut logout_deadline,
                    &mut disconnect_reason,
                )
                .await
                .expect("process neighbor announcement");

            if index < MAX_NEIGHBOR_SIMULATORS {
                let (bytes, _) = receive(&socket).await;
                assert!(matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_)));
                sockets.push(socket);
            } else {
                assert_eq!(connections.len(), MAX_NEIGHBOR_SIMULATORS);
                assert_eq!(neighbors.len(), MAX_NEIGHBOR_SIMULATORS);
                let rejected_endpoint = endpoint;
                let failure = timeout(WAIT, async {
                    loop {
                        if let Event::SimulatorConnectFailed { simulator_id, region_handle: failed_region, endpoint } =
                            received_events.recv().await.expect("event channel remains open").into_event()
                            && failed_region == region_handle
                        {
                            break (simulator_id, endpoint);
                        }
                    }
                })
                .await
                .expect("the 33rd unique simulator should be rejected");
                assert_eq!(failure.1, rejected_endpoint);
                assert!(!connections.contains_key(&region_handle));
                assert_eq!(connections.len(), MAX_NEIGHBOR_SIMULATORS);
                assert_eq!(neighbors.len(), MAX_NEIGHBOR_SIMULATORS);
                sockets.push(socket);
            }
        }

        neighbors.abort_all();
        while neighbors.join_next().await.is_some() {}
    }

    #[tokio::test]
    async fn foreign_ids_are_unavailable() {
        let first = Box::pin(connected_neighbor()).await;
        let second = Box::pin(connected_neighbor()).await;

        assert_ne!(first.primary_id, second.primary_id);
        assert!(matches!(send_targeted(&first.commands, second.primary_id, object_request()).await, Err(Error::SimulatorUnavailable)));

        first.shutdown().await;
        second.shutdown().await;
    }

    #[tokio::test]
    async fn neighbor_backpressure_controls() {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let primary_endpoint = primary.local_addr().expect("primary simulator address");
        let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let session = login_session(primary_endpoint);
        let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
        let (disconnect_signal, disconnect_requested) = watch::channel(false);
        let client = spawn(run_session(session, command_receiver, event_sender.clone(), disconnect_requested));

        let (_, primary_peer) = receive(&primary).await;
        primary.send_to(&ack(20, 1), primary_peer).await.expect("ack primary circuit");
        receive_message(&primary, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
        primary.send_to(&ack(21, 2), primary_peer).await.expect("ack initial movement");

        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        primary.send_to(&handshake, primary_peer).await.expect("send primary handshake");
        receive_message(&primary, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        let Event::SimulatorMessage { sequence, packet_order, .. } = next_event(&mut events).await else {
            panic!("expected region handshake event");
        };
        assert_eq!(sequence, 22);
        assert_eq!(packet_order, Ordered(22));

        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        primary.send_to(&movement_complete, primary_peer).await.expect("send primary movement completion");
        assert!(matches!(next_event(&mut events).await, Event::SessionReady { .. }));
        assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. }));

        event_sender
            .try_send(EventEnvelope::new(Event::Failed("fill application event channel".into())))
            .expect("hold the only ordinary event slot");
        let (neighbor_ip, neighbor_port) = match neighbor_endpoint {
            SocketAddr::V4(address) => (address.ip().octets(), address.port().to_be_bytes()),
            SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
        };
        let mut enable = fixture("enable_simulator.hex");
        set_sequence(&mut enable, 30);
        enable[10..18].copy_from_slice(&0x1000_0000_0000_1000u64.to_le_bytes());
        enable[18..22].copy_from_slice(&neighbor_ip);
        enable[22..24].copy_from_slice(&neighbor_port);
        primary.send_to(&enable, primary_peer).await.expect("announce neighbor simulator");

        let (use_circuit, neighbor_peer) = receive(&neighbor).await;
        let use_circuit = Packet::decode(&use_circuit).expect("decode neighbor UseCircuitCode");
        assert_eq!(use_circuit.sequence(), 1);
        assert!(matches!(use_circuit.message(), Message::UseCircuitCode(_)));
        neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");

        let mut reliable_chat = fixture("chat_from_simulator.hex");
        reliable_chat[0] |= 0x40;
        set_sequence(&mut reliable_chat, 41);
        neighbor.send_to(&reliable_chat, neighbor_peer).await.expect("send reliable neighbor chat");
        neighbor.send_to(&start_ping_packet(42, 7, 0), neighbor_peer).await.expect("send neighbor ping check");

        let replies = timeout(WAIT, async {
            let mut acknowledged_chat = false;
            let mut answered_ping = false;
            let mut buffer = [0; 65_535];
            while !(acknowledged_chat && answered_ping) {
                let (size, _) = neighbor.recv_from(&mut buffer).await.expect("read neighbor reply");
                let packet = Packet::decode(&buffer[..size]).expect("decode neighbor reply");
                acknowledged_chat |= packet.appended_acks().contains(&41)
                    || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|packet| packet.id == 41));
                answered_ping |= matches!(packet.message(), Message::CompletePingCheck(message) if message.ping_id.ping_id == 7);
            }
        })
        .await;
        assert!(replies.is_ok(), "neighbor must ACK reliable traffic and answer ping checks while event delivery is blocked");

        disconnect_signal.send_replace(true);
        receive_message(&primary, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(Duration::from_millis(200), client).await.expect("disconnect should finish").expect("client task").is_ok());
    }

    #[tokio::test]
    async fn neighbor_overload_keeps_control() {
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
        let (event_sender, mut event_receiver) = mpsc::channel(1);
        event_sender
            .try_send(NeighborEvent::Simulator(Event::Failed("hold application events".into())))
            .expect("fill neighbor event channel");
        let (_shutdown_signal, shutdown) = watch::channel(false);
        let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
        let (_commands, command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
        let neighbor_connection = NeighborConnection::connect(
            simulator_id,
            login_session(neighbor_endpoint),
            0x1000_0000_0000_1000,
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect neighbor circuit");
        let mut task = spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, command_receiver));

        let (use_circuit, neighbor_peer) = receive(&neighbor).await;
        assert!(matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_)));
        neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");
        let first_sequence = 41;
        let burst_len = MAX_QUEUED_SIMULATOR_EVENTS * 4;
        let last_sequence = first_sequence + u32::try_from(burst_len).expect("small simulator burst") - 1;
        for sequence in first_sequence..=last_sequence {
            let mut chat = fixture("chat_from_simulator.hex");
            chat[0] |= 0x40;
            set_sequence(&mut chat, sequence);
            neighbor.send_to(&chat, neighbor_peer).await.expect("send reliable burst packet");
        }

        neighbor.send_to(&start_ping_packet(last_sequence + 1, 7, 0), neighbor_peer).await.expect("send ping after the consumer pause");

        let mut acknowledged_sequences = HashSet::with_capacity(burst_len);
        let mut answered_ping = false;
        timeout(WAIT, async {
            while !(acknowledged_sequences.len() == burst_len && answered_ping) {
                let (bytes, _) = receive(&neighbor).await;
                let packet = Packet::decode(&bytes).expect("decode neighbor control reply");
                acknowledged_sequences.extend(
                    packet.appended_acks().iter().copied().filter(|sequence| (*sequence >= first_sequence) && (*sequence <= last_sequence)),
                );
                if let Message::PacketAck(ack) = packet.message() {
                    acknowledged_sequences.extend(
                        ack.packets
                            .iter()
                            .map(|item| item.id)
                            .filter(|sequence| (*sequence >= first_sequence) && (*sequence <= last_sequence)),
                    );
                }
                answered_ping |= matches!(packet.message(), Message::CompletePingCheck(message) if message.ping_id.ping_id == 7);
            }
        })
        .await
        .expect("neighbor ACK and ping handling should continue through a paused-consumer burst");
        assert_eq!(acknowledged_sequences.len(), burst_len, "every reliable burst packet must be acknowledged");

        let mut duplicate_chat = fixture("chat_from_simulator.hex");
        duplicate_chat[0] |= 0x40;
        set_sequence(&mut duplicate_chat, first_sequence);
        neighbor.send_to(&duplicate_chat, neighbor_peer).await.expect("retransmit a reliable packet after overload begins");
        receive_ack(&neighbor, first_sequence).await;

        let mut disable = fixture("disable_simulator.hex");
        disable[0] |= 0x40;
        set_sequence(&mut disable, last_sequence + 2);
        neighbor.send_to(&disable, neighbor_peer).await.expect("disable the overloaded neighbor");
        receive_ack(&neighbor, last_sequence + 2).await;

        assert!(!task.is_finished(), "the neighbor should remain alive while queued events cannot drain");
        assert!(
            timeout(LOGOUT_TIMEOUT + Duration::from_secs(1), &mut task)
                .await
                .expect("overload teardown should finish within its deadline")
                .expect("neighbor task should not panic")
                .expect_err("backpressure must end with an explicit overload failure")
                .contains(EVENT_BACKLOG_DISCONNECT_REASON)
        );
        assert!(
            matches!(event_receiver.try_recv(), Ok(NeighborEvent::Simulator(Event::Failed(message))) if message == "hold application events")
        );
        assert!(
            matches!(event_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)),
            "overflow must not deliver queued application updates after failure"
        );
    }

    #[test]
    fn full_neighbor_queue_keeps_teardown_open() {
        let (events, receiver) = mpsc::channel(1);
        events.try_send(NeighborEvent::Simulator(Event::Failed("hold event".into()))).expect("fill application event channel");
        let mut pending = VecDeque::with_capacity(MAX_QUEUED_SIMULATOR_EVENTS);
        for _ in 0..MAX_QUEUED_SIMULATOR_EVENTS {
            pending.push_back(Event::Failed("pending event".into()));
        }
        let mut closing = false;
        let mut teardown_deadline = None;
        let mut close_reason = None;

        assert!(queue_neighbor_event(
            &events,
            &mut pending,
            Event::Failed("overflow".into()),
            &mut closing,
            &mut teardown_deadline,
            &mut close_reason,
        ));
        assert!(closing);
        assert!(teardown_deadline.is_some());
        assert_eq!(close_reason, Some(EVENT_BACKLOG_DISCONNECT_REASON));
        assert_eq!(pending.len(), MAX_QUEUED_SIMULATOR_EVENTS);

        drop(receiver);
        assert!(!queue_neighbor_event(
            &events,
            &mut pending,
            Event::Failed("receiver closed".into()),
            &mut closing,
            &mut teardown_deadline,
            &mut close_reason,
        ));
    }

    #[tokio::test]
    async fn retry_limit_disconnects_primary() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let endpoint = simulator.local_addr().expect("read simulator address");
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(endpoint),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect simulator circuit");
        let (_, peer) = receive(&simulator).await;

        connection.handle_primary_event_queue_exit(EventQueueExit::Stopped, false).await.expect("ordinary poll stop keeps session");
        let mut buffer = [0; 65_535];
        assert_eq!(simulator.try_recv_from(&mut buffer).expect_err("ordinary poll stop sends no logout").kind(), ErrorKind::WouldBlock);

        let error = connection
            .handle_primary_event_queue_exit(EventQueueExit::RetryLimitReached, false)
            .await
            .expect_err("repeated primary poll errors fail the session");
        assert!(error.contains("EventQueueGet"));
        let (bytes, logout_peer) = receive(&simulator).await;
        let logout = Packet::decode(&bytes).expect("decode logout");
        assert!(matches!(logout.message(), Message::LogoutRequest(_)));
        assert_eq!(logout_peer, peer);
    }

    #[tokio::test]
    async fn former_primary_failure_is_isolated() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind former primary");
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(simulator.local_addr().expect("read simulator address")),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect former primary");
        let _ = receive(&simulator).await;
        let former_primary_id = connection.simulator_id;
        connection.active_primary_id = SimulatorId::allocate().expect("allocate promoted primary ID");

        let (events, mut received_events) = mpsc::channel(4);
        let mut root_connection_active = true;
        let mut pending_events = VecDeque::new();
        let mut drop_application_messages = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;
        assert!(
            connection
                .isolate_former_primary_failure(
                    &mut root_connection_active,
                    "retry limit reached".into(),
                    &events,
                    &mut pending_events,
                    &mut drop_application_messages,
                    &mut logout_deadline,
                    &mut disconnect_reason,
                )
                .await
                .expect("isolate former primary")
        );

        assert!(!root_connection_active);
        assert!(connection.root_connection_failed);
        assert!(matches!(
            next_raw_event(&mut received_events).await,
            Event::SimulatorFailed { simulator_id, reason, .. }
                if simulator_id == former_primary_id && reason.contains("retry limit reached")
        ));
        assert!(
            connection
                .isolate_former_primary_failure(
                    &mut root_connection_active,
                    "duplicate failure".into(),
                    &events,
                    &mut pending_events,
                    &mut drop_application_messages,
                    &mut logout_deadline,
                    &mut disconnect_reason,
                )
                .await
                .expect("ignore repeated former-primary failure")
        );
        assert!(received_events.try_recv().is_err(), "emit only one failure event for the former primary");
    }

    #[tokio::test]
    async fn canceled_primary_event_queue_keeps_circuit_open() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(simulator.local_addr().expect("read simulator address")),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect simulator circuit");
        let _ = receive(&simulator).await;

        connection
            .handle_primary_event_queue_exit(EventQueueExit::PollCanceled, false)
            .await
            .expect("a canceled long poll is not a failed session");

        let mut buffer = [0; 65_535];
        assert_eq!(simulator.try_recv_from(&mut buffer).expect_err("poll cancellation sends no logout").kind(), ErrorKind::WouldBlock);
        drop(connection);
    }

    #[tokio::test]
    async fn retry_avoids_double_logout() {
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let endpoint = simulator.local_addr().expect("read simulator address");
        let mut connection = SimulatorConnection::connect_with_status(
            login_session(endpoint),
            LoginResponseData::default(),
            None,
            NetworkStatusHandle::default(),
        )
        .await
        .expect("connect simulator circuit");
        let (_, peer) = receive(&simulator).await;

        let (events, _event_receiver) = mpsc::channel(1);
        let mut pending_events = VecDeque::with_capacity(MAX_PENDING_SIMULATOR_EVENTS);
        for _ in 1..MAX_PENDING_SIMULATOR_EVENTS {
            pending_events.push_back(EventEnvelope::new(Event::Failed("pending event".into())));
        }
        let mut drop_application_messages = false;
        let mut logout_deadline = None;
        let mut disconnect_reason = None;
        connection
            .publish_event(
                &events,
                &mut pending_events,
                &mut drop_application_messages,
                Event::Failed("backlog limit".into()),
                &mut logout_deadline,
                &mut disconnect_reason,
            )
            .await
            .expect("backlog sends first logout");
        assert!(logout_deadline.is_some());

        let (bytes, logout_peer) = receive(&simulator).await;
        let logout = Packet::decode(&bytes).expect("decode backlog logout");
        assert!(matches!(logout.message(), Message::LogoutRequest(_)));
        assert_eq!(logout_peer, peer);

        let error = connection
            .handle_primary_event_queue_exit(EventQueueExit::RetryLimitReached, logout_deadline.is_some())
            .await
            .expect_err("repeated primary poll errors fail the session");
        assert!(error.contains("EventQueueGet"));

        let mut buffer = [0; 65_535];
        assert!(timeout(Duration::from_millis(50), simulator.recv_from(&mut buffer)).await.is_err());
    }

    fn login_session(simulator_address: SocketAddr) -> AgentSession {
        AgentSession {
            first_name: "Test".into(),
            last_name: "Resident".into(),
            grid: Grid::Agni,
            message: "Welcome".into(),
            simulator_address,
            agent_id: UUID::parse_str(AGENT_ID).expect("agent UUID"),
            session_id: UUID::parse_str(SESSION_ID).expect("session UUID"),
            circuit_code: CIRCUIT_CODE,
            seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("local seed capability URL"),
        }
    }

    struct NeighborHarness {
        primary: UdpSocket,
        neighbor: UdpSocket,
        primary_peer: SocketAddr,
        neighbor_peer: SocketAddr,
        primary_id: SimulatorId,
        neighbor_id: SimulatorId,
        commands: mpsc::Sender<Command>,
        events: mpsc::Receiver<EventEnvelope>,
        disconnect: watch::Sender<bool>,
        status: NetworkStatusHandle,
        client: JoinHandle<Result<(), String>>,
    }

    impl NeighborHarness {
        async fn shutdown(self) {
            self.disconnect.send_replace(true);
            receive_message(&self.primary, |message| matches!(message, Message::LogoutRequest(_))).await;
            assert!(
                timeout(Duration::from_millis(200), self.client).await.expect("disconnect should finish").expect("client task").is_ok()
            );
        }
    }

    async fn connected_neighbor() -> NeighborHarness {
        let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
        let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let primary_endpoint = primary.local_addr().expect("primary simulator address");
        let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
        let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
        let (disconnect, disconnect_requested) = watch::channel(false);
        let status = NetworkStatusHandle::default();
        let client = spawn(run_session_with_status(
            login_session(primary_endpoint),
            command_receiver,
            event_sender,
            disconnect_requested,
            status.clone(),
        ));

        let (_, primary_peer) = receive(&primary).await;
        primary.send_to(&ack(20, 1), primary_peer).await.expect("ack primary circuit");
        receive_message(&primary, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
        primary.send_to(&ack(21, 2), primary_peer).await.expect("ack initial movement");
        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, 22);
        primary.send_to(&handshake, primary_peer).await.expect("send primary handshake");
        receive_message(&primary, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        let mut movement_complete = fixture("agent_movement_complete.hex");
        movement_complete[0] |= 0x40;
        set_sequence(&mut movement_complete, 23);
        primary.send_to(&movement_complete, primary_peer).await.expect("complete primary movement");

        let mut primary_id = None;
        let mut ready_id = None;
        let mut movement_seen = false;
        while primary_id.is_none() || ready_id.is_none() || !movement_seen {
            match next_raw_event(&mut events).await {
                Event::SimulatorConnected { simulator_id, region_handle: None, .. } => primary_id = Some(simulator_id),
                Event::SessionReady { simulator_id, .. } => ready_id = Some(simulator_id),
                Event::SimulatorMessage { sequence: 23, .. } => movement_seen = true,
                _ => {}
            }
        }
        let primary_id = primary_id.expect("primary connection ID");
        assert_eq!(ready_id, Some(primary_id));

        announce_neighbor(&primary, primary_peer, neighbor_endpoint, 0x1000_0000_0000_1000, 30).await;
        let (bytes, neighbor_peer) = receive(&neighbor).await;
        let use_circuit = Packet::decode(&bytes).expect("decode neighbor UseCircuitCode");
        assert_eq!(use_circuit.sequence(), 1);
        assert!(use_circuit.is_reliable());
        assert!(matches!(use_circuit.message(), Message::UseCircuitCode(_)));
        assert_ne!(neighbor_peer, primary_peer, "each simulator must have a separate LLUDP circuit");
        neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");
        let neighbor_id = loop {
            match next_event(&mut events).await {
                Event::SimulatorConnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), endpoint }
                    if endpoint == neighbor_endpoint =>
                {
                    break simulator_id;
                }
                _ => {}
            }
        };
        NeighborHarness {
            primary,
            neighbor,
            primary_peer,
            neighbor_peer,
            primary_id,
            neighbor_id,
            commands,
            events,
            disconnect,
            status,
            client,
        }
    }

    fn movement_complete_packet(sequence: u32, agent_id: UUID, session_id: UUID, region_handle: u64) -> Vec<u8> {
        Packet::new(
            sequence,
            Message::AgentMovementComplete(AgentMovementComplete {
                agent_data: AgentData { agent_id, session_id },
                data: Data { region_handle, ..Default::default() },
                sim_data: SimData::default(),
            }),
        )
        .with_reliable()
        .encode()
        .expect("encode AgentMovementComplete")
    }

    fn teleport_finish_to(agent_id: UUID, endpoint: SocketAddr, region_handle: u64, seed: &str) -> Message {
        let SocketAddr::V4(endpoint) = endpoint else { panic!("simulator endpoint must use IPv4") };
        let schema = SchemaMessage::schema_by_name("TeleportFinish").expect("TeleportFinish schema");
        Message::Schema(
            SchemaMessage::new(
                schema,
                vec![BlockValues::single(vec![
                    FieldValue::UUID(agent_id),
                    FieldValue::U32(0),
                    FieldValue::IpAddr(endpoint.ip().octets()),
                    FieldValue::IpPort(endpoint.port().to_be_bytes()),
                    FieldValue::U64(region_handle),
                    FieldValue::Bytes(seed.as_bytes().to_vec()),
                    FieldValue::U8(0),
                    FieldValue::U32(0),
                ])],
            )
            .expect("valid TeleportFinish"),
        )
    }

    fn ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
        let mut packet = fixture("packet_ack.hex");
        set_sequence(&mut packet, sequence);
        packet[10] = 1;
        packet[11..15].copy_from_slice(&acknowledged.to_le_bytes());
        packet.truncate(15);
        packet
    }

    fn start_ping_packet(sequence: u32, ping_id: u8, oldest_unacked: u32) -> Vec<u8> {
        Packet::new(sequence, Message::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id }, oldest_unacked }))
            .encode()
            .expect("encode ping check")
    }

    async fn receive(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        let mut buffer = [0; 65_535];
        let (size, endpoint) =
            timeout(WAIT, socket.recv_from(&mut buffer)).await.expect("receive simulator datagram").expect("read datagram");
        (buffer[..size].to_vec(), endpoint)
    }

    async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
        timeout(WAIT, async {
            loop {
                let (bytes, _) = receive(socket).await;
                let packet = Packet::decode(&bytes).expect("decode simulator packet");
                if expected(packet.message()) {
                    return packet;
                }
            }
        })
        .await
        .expect("receive expected simulator message")
    }

    async fn receive_message_with_peer(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> (Packet, SocketAddr) {
        timeout(WAIT, async {
            loop {
                let (bytes, peer) = receive(socket).await;
                let packet = Packet::decode(&bytes).expect("decode simulator packet");
                if expected(packet.message()) {
                    return (packet, peer);
                }
            }
        })
        .await
        .expect("receive expected simulator message")
    }

    async fn receive_ack(socket: &UdpSocket, sequence: u32) {
        timeout(WAIT, async {
            loop {
                let (bytes, _) = receive(socket).await;
                let packet = Packet::decode(&bytes).expect("decode simulator acknowledgement");
                if packet.appended_acks().contains(&sequence)
                    || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|item| item.id == sequence))
                {
                    return;
                }
            }
        })
        .await
        .expect("receive reliable packet acknowledgement");
    }

    async fn announce_neighbor(primary: &UdpSocket, peer: SocketAddr, endpoint: SocketAddr, region_handle: u64, sequence: u32) {
        let (ip, port) = match endpoint {
            SocketAddr::V4(address) => (address.ip().octets(), address.port().to_be_bytes()),
            SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
        };
        let mut enable = fixture("enable_simulator.hex");
        set_sequence(&mut enable, sequence);
        enable[10..18].copy_from_slice(&region_handle.to_le_bytes());
        enable[18..22].copy_from_slice(&ip);
        enable[22..24].copy_from_slice(&port);
        primary.send_to(&enable, peer).await.expect("announce neighbor simulator");
    }

    async fn send_targeted(commands: &mpsc::Sender<Command>, simulator_id: SimulatorId, message: Message) -> Result<(), Error> {
        let (completed, response) = oneshot::channel();
        commands
            .send(Command::SendTo { simulator_id, message, delivery: Delivery::Reliable, completed })
            .await
            .map_err(|_| Error::SessionClosed)?;
        response.await.map_err(|_| Error::SessionClosed)?
    }

    fn object_request() -> Message {
        Message::RequestMultipleObjects(RequestMultipleObjects {
            agent_data: RequestAgentData {
                agent_id: UUID::parse_str(AGENT_ID).expect("agent UUID"),
                session_id: UUID::parse_str(SESSION_ID).expect("session UUID"),
            },
            objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 77 }],
        })
    }

    async fn next_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
        loop {
            let event = next_raw_event(events).await;
            if matches!(&event, Event::SimulatorConnected { region_handle: None, .. }) {
                continue;
            }
            return event;
        }
    }

    async fn next_neighbor_event(events: &mut mpsc::Receiver<NeighborEvent>) -> Event {
        timeout(WAIT, async {
            match events.recv().await.expect("neighbor event channel remains open") {
                NeighborEvent::Simulator(event) => event,
                NeighborEvent::Finished { .. } => panic!("a standalone neighbor worker must not emit its manager completion"),
            }
        })
        .await
        .expect("receive neighbor event")
    }

    async fn next_raw_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
        timeout(WAIT, async {
            loop {
                let event = events.recv().await.expect("event channel remains open").into_event();
                if matches!(&event, Event::CapabilityError { capability, .. } if capability == "EventQueueGet") {
                    continue;
                }
                return event;
            }
        })
        .await
        .expect("receive network event")
    }

    fn fixture(name: &str) -> Vec<u8> {
        let source = vectors::get(name).unwrap_or_else(|| panic!("unknown fixture {name}"));
        parse_hex_fixture(source).unwrap_or_else(|error| panic!("invalid fixture {name}: {error}"))
    }

    fn set_sequence(bytes: &mut [u8], sequence: u32) {
        bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
    }
}
