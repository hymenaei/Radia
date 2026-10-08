use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use base::UUID;
use llsd::Value;
use protocol::{
    messages::{
        CompleteAgentMovement, CompletePingCheck, LogoutRequest, RegionHandshakeReply, UseCircuitCode,
        complete_agent_movement::AgentData as MovementAgentData,
        complete_ping_check::PingID as CompletePingID,
        logout_request::AgentData as LogoutAgentData,
        region_handshake_reply::{AgentData as ReplyAgentData, RegionInfo as ReplyRegionInfo},
        use_circuit_code::CircuitCode,
    },
    udp::{Message, Packet},
};
use tokio::{
    sync::{mpsc, watch},
    task::{AbortHandle, JoinSet},
    time::{self, Instant},
};
use url::Url;

#[cfg(test)]
use crate::event::send_failure_event;
use crate::{
    AgentSession, LoginResponseData,
    capabilities::{CapabilityBudget, Client, EventQueueExit},
    capture::CaptureRecorder,
    circuit::{Circuit, Delivery, KEEPALIVE_TIMEOUT, PING_INTERVAL},
    client::Command,
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, SimulatorId},
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
    Send { message: Message, delivery: Delivery, completed: tokio::sync::oneshot::Sender<Result<(), crate::Error>> },
}

pub(crate) struct SimulatorConnection {
    simulator_id: SimulatorId,
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
        self,
        commands: mpsc::Receiver<Command>,
        events: mpsc::Sender<EventEnvelope>,
        disconnect_requested: watch::Receiver<bool>,
        terminal_event_permit: mpsc::OwnedPermit<EventEnvelope>,
    ) -> Result<(), String> {
        let status = self.status.clone();
        let simulator_id = self.simulator_id;
        let endpoint = self.session.simulator_address;
        let result = self.run_inner(commands, events, disconnect_requested).await;
        status.clear_simulator();
        match &result {
            Ok(()) => status.set_lifecycle(NetworkLifecycle::Disconnected),
            Err(error) => {
                status.record_error(error);
                status.set_lifecycle(NetworkLifecycle::Failed);
            }
        }
        Self::publish_primary_terminal_event(terminal_event_permit, simulator_id, endpoint, &result);
        result
    }

    fn publish_primary_terminal_event(
        permit: mpsc::OwnedPermit<EventEnvelope>,
        simulator_id: SimulatorId,
        endpoint: SocketAddr,
        result: &Result<(), String>,
    ) {
        let event = match result {
            Ok(()) => Event::SimulatorDisconnected { simulator_id, region_handle: None, endpoint },
            Err(reason) => Event::SimulatorFailed { simulator_id, region_handle: None, endpoint, reason: reason.clone() },
        };
        permit.send(EventEnvelope::new(event));
    }

    #[tracing::instrument(name = "simulator.session", level = "info", skip_all)]
    async fn run_inner(
        mut self,
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
        let mut neighbor_event_queue_owners = HashMap::<tokio::task::Id, (SimulatorId, SocketAddr)>::new();
        let mut neighbor_connections: HashMap<u64, NeighborConnectionState> = HashMap::new();
        let mut retiring_simulators = HashSet::new();
        let mut neighbor_task_owners = HashMap::<tokio::task::Id, (SimulatorId, u64, SocketAddr)>::new();
        let mut pending_neighbor_finishes = VecDeque::new();
        let mut simulator_capabilities: HashMap<SocketAddr, SimulatorCapabilitySession> = HashMap::new();
        let mut connected_simulators = HashSet::new();
        let mut preconnect_events = PreconnectEvents::default();
        let mut retry_timer = time::interval(ACK_INTERVAL);
        retry_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut ping_timer = time::interval_at(Instant::now() + PING_INTERVAL, PING_INTERVAL);
        ping_timer.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        let mut last_udp_received = Instant::now();
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
                _ = time::sleep_until(last_udp_received + KEEPALIVE_TIMEOUT), if session_ready && logout_deadline.is_none() && !*disconnect_requested.borrow() => {
                    return Err("The simulator stopped responding to UDP keepalive traffic.".into());
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
                                            seed,
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
                                        body: if message == "EstablishAgentCommunication" {
                                            redact_seed_capability(&body)
                                        } else {
                                            body
                                        },
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
                                        || retiring_simulators.contains(simulator_id)
                                }
                                _ => false,
                            };
                            if is_current {
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
                    self.handle_primary_event_queue_exit(exit, logout_deadline.is_some()).await?;
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

                            if let Err(error) = self.circuit.send(
                                &self.socket,
                                message,
                                delivery,
                            ).await {
                                self.publish_event(
                                    &events,
                                    &mut pending_events,
                                    &mut drop_application_messages,
                                    Event::SendFailed(error),
                                    &mut logout_deadline,
                                    &mut disconnect_reason,
                                ).await?;
                            }
                        }
                        Some(Command::SendTo { simulator_id, message, delivery, completed }) => {
                            if simulator_id == self.simulator_id {
                                let result = if session_ready {
                                    self.circuit
                                        .send(&self.socket, message, delivery)
                                        .await
                                        .map(|_| ())
                                        .map_err(crate::Error::SimulatorSend)
                                } else {
                                    Err(crate::Error::SimulatorNotReady)
                                };
                                let _ = completed.send(result);
                            } else if let Some(connection) = neighbor_connections
                                .values()
                                .find(|connection| connection.simulator_id == simulator_id)
                            {
                                match connection.commands.try_send(NeighborCommand::Send { message, delivery, completed }) {
                                    Ok(()) => {}
                                    Err(mpsc::error::TrySendError::Full(NeighborCommand::Send { completed, .. })) => {
                                        let _ = completed.send(Err(crate::Error::SimulatorCommandQueueFull));
                                    }
                                    Err(mpsc::error::TrySendError::Closed(NeighborCommand::Send { completed, .. })) => {
                                        let _ = completed.send(Err(crate::Error::SimulatorConnectionClosed));
                                    }
                                }
                            } else {
                                let _ = completed.send(Err(crate::Error::SimulatorUnavailable));
                            }
                        }
                        None => {
                            self.disconnect_once(logout_deadline.is_some()).await?;
                            return Ok(());
                        }
                    }
                }
                received = self.socket.recv(&mut receive_buffer), if drop_application_messages || pending_events.len() < MAX_PENDING_SIMULATOR_EVENTS => {
                    let size = received
                        .map_err(|error| format!("Simulator connection failed: {error}"))?;
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
                            self.disconnect_once(logout_deadline.is_some()).await?;
                            return Err(error.to_string());
                        }
                        if events.capacity() == 0 {
                            self.circuit.flush_pending_acks(&self.socket).await?;
                        }
                        continue;
                    }

                    let is_new_packet = match self.circuit.receive(&packet) {
                        Ok(is_new_packet) => is_new_packet,
                        Err(error) => {
                            self.disconnect_once(logout_deadline.is_some()).await?;
                            return Err(error.to_string());
                        }
                    };
                    if events.capacity() == 0 {
                        self.circuit.flush_pending_acks(&self.socket).await?;
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
                                agent_data: MovementAgentData {
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
                    let mut dispatch_message = true;
                    match message {
                        Message::EnableSimulator(message) => {
                            let info = &message.simulator_info;
                            let endpoint = SocketAddr::from((Ipv4Addr::from(info.ip), u16::from_be_bytes(info.port)));
                            Box::pin(self.start_neighbor(
                                self.simulator_id,
                                info.handle,
                                endpoint,
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
                            tracing::info!("Simulator disconnected the session");
                            self.circuit.flush_pending_acks(&self.socket).await?;
                            if let Some(capture) = &self.capture {
                                capture.circuit_lifecycle("disconnected", self.session.circuit_code);
                            }
                            return finish_logout(disconnect_reason);
                        }
                        Message::RegionHandshake(_) => {
                            saw_region_handshake = true;
                            self.circuit.send(
                                &self.socket,
                                Message::RegionHandshakeReply(RegionHandshakeReply {
                                    agent_data: ReplyAgentData {
                                        agent_id: self.session.agent_id,
                                        session_id: self.session.session_id,
                                    },
                                    region_info: ReplyRegionInfo { flags: 0 },
                                }),
                                Delivery::Reliable,
                            ).await?;

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
                            self.circuit.clear_duplicate_list(message.ping_id.oldest_unacked, Instant::now());
                            self.circuit.send(
                                &self.socket,
                                Message::CompletePingCheck(CompletePingCheck {
                                    ping_id: CompletePingID { ping_id: message.ping_id.ping_id },
                                }),
                                Delivery::Unreliable,
                            ).await?;
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
                            self.circuit.flush_pending_acks(&self.socket).await?;
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
                _ = retry_timer.tick() => {
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
                    ack_flush?;
                }
                _ = ping_timer.tick(), if agent_movement_sent && logout_deadline.is_none() && !*disconnect_requested.borrow() => {
                    self.circuit.send_ping(&self.socket, Instant::now()).await?;
                }
            }
        }
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
        neighbors: &mut JoinSet<(SimulatorId, u64, SocketAddr, NeighborTaskOutcome)>,
        connections: &mut HashMap<u64, NeighborConnectionState>,
        retiring_simulators: &mut HashSet<SimulatorId>,
        neighbor_task_owners: &mut HashMap<tokio::task::Id, (SimulatorId, u64, SocketAddr)>,
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
        if !is_current_simulator(origin, self.simulator_id, connections) {
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
            return Ok(());
        }
        if endpoint == self.session.simulator_address
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
        let capture = self.capture.clone();
        let status = self.status.clone();
        let (command_sender, command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
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

    async fn disconnect_once(&mut self, logout_already_sent: bool) -> Result<(), String> {
        tracing::info!(logout_already_sent, "Simulator disconnect started");
        self.status.set_lifecycle(NetworkLifecycle::Disconnecting);
        if let Some(capture) = &self.capture {
            capture.circuit_lifecycle("disconnect_started", self.session.circuit_code);
        }
        if !logout_already_sent {
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
    Value::Map(fields)
}

struct SimulatorCapabilitySession {
    simulator_id: SimulatorId,
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
        capability: crate::capabilities::EVENT_QUEUE_NAME.to_owned(),
        reason: "The neighbor simulator EventQueueGet task ended unexpectedly.".into(),
    })
}

fn neighbor_event_queue_task_failure(
    error: &tokio::task::JoinError,
    owners: &mut HashMap<tokio::task::Id, (SimulatorId, SocketAddr)>,
    primary_id: SimulatorId,
    connections: &HashMap<u64, NeighborConnectionState>,
) -> Option<Event> {
    let (simulator_id, source) = owners.remove(&error.id())?;
    neighbor_event_queue_failure(simulator_id, source, EventQueueExit::TaskFailed, primary_id, connections)
}

#[cfg(test)]
fn remove_failed_neighbor(
    error: &tokio::task::JoinError,
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
        Ok(Self { simulator_id, session, region_handle, socket, circuit, use_circuit_sequence })
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
                    return Err("The simulator stopped responding to UDP keepalive traffic.".into());
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
                            let _ = completed.send(Err(crate::Error::SimulatorConnectionClosed));
                        }
                        Some(NeighborCommand::Send { completed, .. }) if !established => {
                            let _ = completed.send(Err(crate::Error::SimulatorNotReady));
                        }
                        Some(NeighborCommand::Send { message, delivery, completed }) => {
                            let result = self.circuit
                                .send(&self.socket, message, delivery)
                                .await
                                .map(|_| ())
                                .map_err(crate::Error::SimulatorSend);
                            let failed = result.as_ref().err().map(ToString::to_string);
                            let _ = completed.send(result);
                            if let Some(error) = failed {
                                return Err(error);
                            }
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
                    let is_new_packet = self.circuit.receive(&packet).map_err(|error| error.to_string())?;
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
                    }
                    if !is_new_packet {
                        continue;
                    }

                    let message = packet.message();
                    let mut dispatch_message = true;
                    match message {
                        Message::EnableSimulator(enabled) => {
                            let info = &enabled.simulator_info;
                            let endpoint = SocketAddr::from((Ipv4Addr::from(info.ip), u16::from_be_bytes(info.port)));
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
                            self.circuit.send(
                                &self.socket,
                                Message::RegionHandshakeReply(RegionHandshakeReply {
                                    agent_data: ReplyAgentData {
                                        agent_id: self.session.agent_id,
                                        session_id: self.session.session_id,
                                    },
                                    region_info: ReplyRegionInfo { flags: 0 },
                                }),
                                Delivery::Reliable,
                            ).await?;
                        }
                        Message::StartPingCheck(message) => {
                            dispatch_message = false;
                            self.circuit.clear_duplicate_list(message.ping_id.oldest_unacked, Instant::now());
                            self.circuit.send(
                                &self.socket,
                                Message::CompletePingCheck(CompletePingCheck {
                                    ping_id: CompletePingID { ping_id: message.ping_id.ping_id },
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
    let failure_events = events.clone();
    let mut failure_shutdown = disconnect_requested.clone();
    let terminal_event_permit = events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let result = async {
        SimulatorConnection::connect_with_status(session, LoginResponseData::default(), None, NetworkStatusHandle::default())
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
                agent_data: LogoutAgentData { agent_id: session.agent_id, session_id: session.session_id },
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
                agent_data: LogoutAgentData { agent_id: session.agent_id, session_id: session.session_id },
            }),
            Delivery::ReliableNoRetry,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        EVENT_BACKLOG_DISCONNECT_REASON, MAX_SIMULATORS_PER_ENABLE_EVENT, NeighborCommand, NeighborConnectionState, NeighborTaskOutcome,
        decode_agent_communication, decode_enable_simulators, neighbor_completion_event, neighbor_event_queue_failure,
        neighbor_event_queue_task_failure, redact_seed_capability, remove_failed_neighbor,
    };
    use crate::{
        capabilities::EventQueueExit,
        event::{Event, SimulatorId},
    };
    use base::UUID;
    use llsd::{Map, Value};
    use std::{collections::HashMap, future::pending, net::SocketAddr};
    use tokio::{sync::mpsc, task::JoinSet};

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
    async fn event_queue_worker_panic_reaches_current_simulator() {
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
    async fn late_event_queue_completion_is_ignored_after_replacement() {
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
#[path = "../tests/unit/session.rs"]
mod session_tests;

#[cfg(test)]
#[path = "../tests/unit/simulator.rs"]
mod simulator_tests;
