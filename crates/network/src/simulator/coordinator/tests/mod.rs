mod coordination;
mod coverage;
mod handoff;
mod neighbors;
mod session;
mod startup;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::{Future, pending},
    io::ErrorKind,
    net::SocketAddr,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use ::test_support::lludp::fixture;
use llsd::{Map, Value};
use protocol::{
    ChatType,
    lludp::{Message, Packet},
    login::ResponseData,
    messages::{
        AgentMovementComplete, CacheMissType, ChatFromViewer, CompletePingCheck, ObjectRequest, PacketAck, PingId, RequestAgentData,
        RequestMultipleObjects, StartPingCheck, TeleportCancel, TeleportFinish,
        agent::agent_movement_complete::{AgentData, Data, SimData},
        chat::chat_from_viewer,
        chat::chat_from_viewer::ChatData,
        circuit::packet_ack::Packets,
    },
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, UdpSocket},
    runtime, spawn,
    sync::{Notify, Semaphore, mpsc, oneshot, watch},
    task,
    task::{Id, JoinError, JoinHandle},
    time::{advance, timeout},
};
use url::Url;
use uuid::Uuid;

use crate::event::send_failure_event;
use crate::{
    AgentSession, Error, Grid,
    capabilities::{CapabilityBudget, Client, EventQueueExit},
    coverage::{CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageSource},
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, PacketOrder::Ordered, SimulatorId},
    session::{COMMAND_CAPACITY, Command, DEFAULT_UPSTREAM_BYTES_PER_SECOND, NETWORK_THREAD_STACK_SIZE},
    simulator::coordinator::{
        CoordinatorRunState, EventQueueRoleHandle, PromotionSetup, SessionStartup, SimulatorCommand, SimulatorMemoryBudget,
        SimulatorTaskOutcome, neighbor_completion_event,
    },
    simulator::{
        EventRetention, MAX_NEIGHBOR_CONNECTION_TASKS, MAX_NEIGHBOR_SIMULATORS, SimulatorCoordinator,
        admission::{
            MAX_CONCURRENT_PACKET_DECODES, MAX_NEIGHBOR_CONNECTION_EVENT_BYTES, MAX_QUEUED_NEIGHBOR_EVENT_BYTES,
            MAX_QUEUED_SIMULATOR_EVENT_BYTES,
        },
    },
    status::{NetworkLifecycle, NetworkStatusHandle},
    transport::{
        SendGate, UdpTransport,
        lludp::{Delivery, KEEPALIVE_TIMEOUT, MAX_PENDING_PACKETS},
        pacing::OutboundBudgetHandle,
    },
};

use super::logout::{LogoutAttemptError, LogoutOutcome, LogoutUnavailable, send_logout_with_deadline};
use crate::simulator::coordinator::event_routing::NeighborEventQueueOutcome;
use crate::simulator::coordinator::handoff::{
    FinishOrder, HandoffDecision, HandoffEvidence, PendingPromotion, arbitrate_handoff, decode_teleport_finish, handoff_cancellation,
    same_handoff_destination,
};

use super::*;
use super::{
    EVENT_BACKLOG_DISCONNECT_REASON, HandoffState, HandoffTarget, LOGOUT_TIMEOUT, MAX_PENDING_SIMULATOR_EVENTS, MAX_PRECONNECT_EVENTS,
    MAX_QUEUED_SIMULATOR_EVENTS, NEIGHBOR_COMMAND_CAPACITY, PreconnectEvents, PreconnectOutcome, SimulatorCapabilitySession,
    SimulatorConnection, SimulatorConnectionState, SimulatorEvent, queue_neighbor_event, queue_until_connected,
};

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
const CIRCUIT_CODE: u32 = 0x1122_3344;
const WAIT: Duration = Duration::from_secs(2);
const SESSION_EVENT_CAPACITY: usize = 2;
const TEST_EVENT_CAPACITY: usize = CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY;

pub(crate) async fn run_session(
    session: AgentSession,
    commands: mpsc::Receiver<Command>,
    events: mpsc::Sender<EventEnvelope>,
    disconnect_requested: watch::Receiver<bool>,
) -> Result<(), String> {
    run_session_with_status(session, commands, events, disconnect_requested, NetworkStatusHandle::default(), None).await
}

pub(super) fn test_options(status: NetworkStatusHandle) -> SimulatorCoordinatorOptions {
    SimulatorCoordinatorOptions {
        status,
        coverage_losses: CoverageLossLedger::new(),
        session: SessionOptions::default(),
        startup_disconnect: None,
        root_transport: None,
    }
}

fn neighbor_event_queue_task_failure(
    error: &JoinError,
    owners: &mut HashMap<Id, (SimulatorId, SocketAddr)>,
    primary_id: SimulatorId,
    connections: &HashMap<u64, SimulatorConnectionState>,
) -> Option<Event> {
    let (simulator_id, source) = owners.remove(&error.id())?;
    neighbor_event_queue_failure(simulator_id, source, EventQueueExit::TaskFailed, primary_id, connections)
}

fn remove_failed_neighbor(
    error: &JoinError,
    connections: &mut HashMap<u64, SimulatorConnectionState>,
) -> Option<(u64, SimulatorId, SocketAddr)> {
    let region_handle =
        connections.iter().find(|(_, connection)| connection.abort.id() == error.id()).map(|(region_handle, _)| *region_handle)?;
    let connection = connections.remove(&region_handle)?;
    Some((region_handle, connection.simulator_id, connection.endpoint))
}

fn coordinator_options(status: NetworkStatusHandle) -> SimulatorCoordinatorOptions {
    test_options(status)
}

async fn gated_transport(session: &AgentSession, status: &NetworkStatusHandle, send_gate: SendGate) -> Result<UdpTransport, String> {
    let mut transport = UdpTransport::connect_with_status(session.simulator_address, status.clone()).await?;
    transport.set_send_gate(send_gate);
    Ok(transport)
}

async fn connect_with_status(
    session: AgentSession,
    login_data: ResponseData,
    status: NetworkStatusHandle,
) -> Result<SimulatorCoordinator, String> {
    SimulatorCoordinator::connect(session, login_data, coordinator_options(status))
        .await?
        .ok_or_else(|| "The primary simulator startup was canceled unexpectedly.".to_string())
}

pub(super) async fn run_session_with_status(
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
        let connection = SimulatorCoordinator::connect(session, ResponseData::default(), coordinator_options(status))
            .await?
            .ok_or_else(|| "The primary simulator startup was canceled unexpectedly.".to_string())?;
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

async fn connect_with_coverage(
    session: AgentSession,
    login_data: ResponseData,
    status: NetworkStatusHandle,
    coverage_losses: CoverageLossLedger,
) -> Result<SimulatorCoordinator, String> {
    let mut options = coordinator_options(status);
    options.coverage_losses = coverage_losses;
    SimulatorCoordinator::connect(session, login_data, options)
        .await?
        .ok_or_else(|| "The primary simulator startup was canceled unexpectedly.".to_string())
}

async fn connect_with_status_and_send_gate(
    session: AgentSession,
    login_data: ResponseData,
    status: NetworkStatusHandle,
    send_gate: SendGate,
) -> Result<SimulatorCoordinator, String> {
    let mut options = coordinator_options(status);
    options.root_transport = Some(gated_transport(&session, &options.status, send_gate).await?);
    SimulatorCoordinator::connect(session, login_data, options)
        .await?
        .ok_or_else(|| "The primary simulator startup was canceled unexpectedly.".to_string())
}

async fn connect_with_gate_and_disconnect(
    session: AgentSession,
    login_data: ResponseData,
    status: NetworkStatusHandle,
    send_gate: SendGate,
    disconnect_requested: watch::Receiver<bool>,
) -> Result<Option<SimulatorCoordinator>, String> {
    let mut options = coordinator_options(status);
    options.root_transport = Some(gated_transport(&session, &options.status, send_gate).await?);
    options.startup_disconnect = Some(disconnect_requested);
    SimulatorCoordinator::connect(session, login_data, options).await
}

pub(super) async fn connect_test_connection(
    simulator_id: SimulatorId,
    session: AgentSession,
    region_handle: Option<u64>,
    status: NetworkStatusHandle,
    coverage_losses: CoverageLossLedger,
    primary_state: Option<watch::Receiver<PrimaryState>>,
    budget: Option<OutboundBudgetHandle>,
    owned_budget: Option<OutboundBudget>,
    send_gate: Option<SendGate>,
) -> Result<SimulatorConnection, String> {
    let transport = match send_gate {
        Some(send_gate) => Some(gated_transport(&session, &status, send_gate).await?),
        None => None,
    };
    let (budget, owned_budget) = match (budget, owned_budget) {
        (Some(budget), owned_budget) => (budget, owned_budget),
        (None, Some(owned_budget)) => (owned_budget.handle(), Some(owned_budget)),
        (None, None) => {
            let owned_budget = OutboundBudget::new(DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
            (owned_budget.handle(), Some(owned_budget))
        }
    };
    SimulatorConnection::connect(
        session,
        SimulatorConnectionOptions { simulator_id, region_handle, status, coverage_losses, primary_state, budget, owned_budget, transport },
    )
    .await
}

async fn connect_test_neighbor(
    simulator_id: SimulatorId,
    session: AgentSession,
    region_handle: u64,
    status: NetworkStatusHandle,
) -> Result<SimulatorConnection, String> {
    connect_test_connection(simulator_id, session, Some(region_handle), status, CoverageLossLedger::new(), None, None, None, None).await
}

pub(super) async fn connect_test_primary_with_state(
    simulator_id: SimulatorId,
    session: AgentSession,
    status: NetworkStatusHandle,
    coverage_losses: CoverageLossLedger,
    primary_state: watch::Receiver<PrimaryState>,
    budget: OutboundBudgetHandle,
    send_gate: Option<SendGate>,
) -> Result<SimulatorConnection, String> {
    connect_test_connection(simulator_id, session, None, status, coverage_losses, Some(primary_state), Some(budget), None, send_gate).await
}

struct NeighborHarness {
    primary: UdpSocket,
    neighbor: UdpSocket,
    primary_peer: SocketAddr,
    neighbor_peer: SocketAddr,
    primary_id: SimulatorId,
    primary_region_handle: u64,
    neighbor_id: SimulatorId,
    commands: mpsc::Sender<Command>,
    events: mpsc::Receiver<EventEnvelope>,
    disconnect: watch::Sender<bool>,
    status: NetworkStatusHandle,
    packet_memory: SimulatorMemoryBudget,
    client: JoinHandle<Result<(), String>>,
    pause_sends: watch::Sender<bool>,
    send_entered: Arc<Notify>,
}

impl NeighborHarness {
    async fn shutdown(self) {
        self.disconnect.send_replace(true);
        receive_message(&self.primary, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(Duration::from_millis(200), self.client).await.expect("disconnect should finish").expect("client task").is_ok());
    }
}

pub(super) fn login_session(simulator_address: SocketAddr) -> AgentSession {
    AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address,
        agent_id: Uuid::parse_str(AGENT_ID).expect("agent UUID"),
        session_id: Uuid::parse_str(SESSION_ID).expect("session UUID"),
        circuit_code: CIRCUIT_CODE,
        seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("local seed capability URL"),
    }
}

fn retained_packet_event(budget: &SimulatorMemoryBudget, simulator_id: SimulatorId, source: SocketAddr, wire: &[u8]) -> EventEnvelope {
    let preflight = Packet::preflight(wire).expect("preflight retained simulator packet");
    let mut reservation = budget.reserve_packet(preflight).expect("reserve retained simulator packet");
    let packet = Packet::decode(wire).expect("decode retained simulator packet");
    budget.trim_packet_reservation(&mut reservation, &packet).expect("trim to retained simulator packet capacity");
    let sequence = packet.sequence();
    EventEnvelope::with_retention(
        Event::SimulatorMessage {
            simulator_id,
            source,
            sequence,
            packet_order: Ordered(u64::from(sequence)),
            message: packet.into_message(),
        },
        reservation.into_retention(),
    )
}

fn fill_neighbor_budget(budget: &SimulatorMemoryBudget) -> Vec<EventRetention> {
    let mut remaining = MAX_QUEUED_NEIGHBOR_EVENT_BYTES;
    let mut retained = Vec::with_capacity(MAX_QUEUED_NEIGHBOR_EVENT_BYTES / MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    while remaining > 0 {
        let bytes = remaining.min(MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
        let reservation = budget.for_neighbor().reserve_bytes_for_test(bytes).expect("fill a neighbor connection's admission allowance");
        retained.push(reservation.into_retention());
        remaining -= bytes;
    }
    assert_eq!(budget.neighbor_available_bytes(), 0);
    retained
}

fn run_large_stack_test<F: Future<Output = ()> + 'static>(test: fn() -> F) {
    thread::Builder::new()
        .name("network-handoff-test".into())
        .stack_size(NETWORK_THREAD_STACK_SIZE)
        .spawn(move || {
            let runtime = runtime::Builder::new_current_thread().enable_all().build().expect("handoff runtime");
            runtime.block_on(test());
        })
        .expect("handoff test thread")
        .join()
        .expect("handoff test completes");
}

fn run_large_paused_stack_test<F: Future<Output = ()> + 'static>(test: fn() -> F) {
    thread::Builder::new()
        .name("network-coordinator-test".into())
        .stack_size(NETWORK_THREAD_STACK_SIZE)
        .spawn(move || {
            let mut builder = runtime::Builder::new_current_thread();
            builder.enable_all();
            builder.start_paused(true);
            builder.build().expect("coordinator runtime").block_on(test());
        })
        .expect("coordinator test thread")
        .join()
        .expect("coordinator test completes");
}

fn duration_percentiles(samples: &[Duration]) -> Option<(Duration, Duration, Duration)> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let percentile = |percent: usize| sorted[(sorted.len() - 1) * percent / 100];
    Some((percentile(50), percentile(95), percentile(99)))
}

async fn connected_neighbor() -> NeighborHarness {
    connected_neighbor_with_seed(Url::parse("http://127.0.0.1:1/seed").expect("fixture seed")).await
}

async fn connected_neighbor_with_seed(seed: Url) -> NeighborHarness {
    connected_neighbor_with_options(seed).await
}

async fn connected_neighbor_with_options(seed: Url) -> NeighborHarness {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let primary_endpoint = primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
    let (disconnect, disconnect_requested) = watch::channel(false);
    let status = NetworkStatusHandle::default();
    let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
    let connection = connect_with_status_and_send_gate(
        AgentSession { seed_capability: seed, ..login_session(primary_endpoint) },
        ResponseData::default(),
        status.clone(),
        send_gate,
    )
    .await
    .expect("connect primary simulator");
    let packet_memory = connection.packet_memory.clone();
    let terminal = event_sender.clone().reserve_owned().await.expect("reserve terminal event");
    let client = spawn(connection.run(command_receiver, event_sender, disconnect_requested, terminal));

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
    let primary_region_handle =
        match Packet::decode(&fixture("agent_movement_complete.hex")).expect("decode initial movement completion").message() {
            Message::AgentMovementComplete(movement) => movement.data.region_handle,
            message => panic!("unexpected initial movement message: {message:?}"),
        };

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
        primary_region_handle,
        neighbor_id,
        commands,
        events,
        disconnect,
        status,
        packet_memory,
        client,
        pause_sends,
        send_entered,
    }
}

fn movement_complete_packet(sequence: u32, agent_id: Uuid, session_id: Uuid, region_handle: u64) -> Vec<u8> {
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

fn teleport_finish_to(agent_id: Uuid, endpoint: SocketAddr, region_handle: u64, seed: &str) -> Message {
    let SocketAddr::V4(endpoint) = endpoint else { panic!("simulator endpoint must use IPv4") };
    Message::TeleportFinish(TeleportFinish {
        agent_id,
        location_id: 0,
        sim_ip: endpoint.ip().octets(),
        sim_port: endpoint.port().to_be_bytes(),
        region_handle,
        seed_capability: seed.as_bytes().to_vec(),
        sim_access: Some(0),
        teleport_flags: Some(0),
    })
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
    let (size, endpoint) = socket.recv_from(&mut buffer).await.expect("read datagram");
    (buffer[..size].to_vec(), endpoint)
}

async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
    loop {
        let (bytes, _) = receive(socket).await;
        let packet = Packet::decode(&bytes).expect("decode simulator packet");
        if expected(packet.message()) {
            return packet;
        }
    }
}

async fn receive_message_with_peer(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> (Packet, SocketAddr) {
    loop {
        let (bytes, peer) = receive(socket).await;
        let packet = Packet::decode(&bytes).expect("decode simulator packet");
        if expected(packet.message()) {
            return (packet, peer);
        }
    }
}

async fn receive_ack(socket: &UdpSocket, sequence: u32) -> Packet {
    loop {
        let (bytes, _) = receive(socket).await;
        let packet = Packet::decode(&bytes).expect("decode simulator acknowledgement");
        if packet.appended_acks().contains(&sequence)
            || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|item| item.id == sequence))
        {
            return packet;
        }
    }
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
            agent_id: Uuid::parse_str(AGENT_ID).expect("agent UUID"),
            session_id: Uuid::parse_str(SESSION_ID).expect("session UUID"),
        },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 77 }],
    })
}

fn chat_message(message: impl Into<String>) -> Message {
    Message::from(ChatFromViewer {
        agent_data: chat_from_viewer::AgentData {
            agent_id: Uuid::parse_str(AGENT_ID).expect("valid test agent ID"),
            session_id: Uuid::parse_str(SESSION_ID).expect("valid test session ID"),
        },
        chat_data: ChatData { message: message.into(), chat_type: ChatType::Normal, channel: 0 },
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

async fn next_neighbor_event(events: &mut mpsc::Receiver<SimulatorEvent>) -> Event {
    match events.recv().await.expect("neighbor event channel remains open") {
        SimulatorEvent::Simulator(event) => event.into_event(),
        SimulatorEvent::SendCompleted { .. } | SimulatorEvent::Finished { .. } => {
            panic!("a standalone neighbor worker must not emit its manager completion")
        }
    }
}

async fn next_raw_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
    loop {
        let event = events.recv().await.expect("event channel remains open").into_event();
        if matches!(&event, Event::CapabilityError { capability, .. } if capability == "EventQueueGet") {
            continue;
        }
        return event;
    }
}

fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}

#[test]
fn stalled_send_is_isolated() {
    run_large_paused_stack_test(run_stalled_send_is_isolated);
}

async fn run_stalled_send_is_isolated() {
    let mut harness = connected_neighbor().await;
    harness.pause_sends.send_replace(true);
    let (completed, mut response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo { simulator_id: harness.primary_id, message: object_request(), delivery: Delivery::Reliable, completed })
        .await
        .expect("dispatch primary send");
    timeout(WAIT, harness.send_entered.notified()).await.expect("primary enters local send");
    assert!(matches!(response.try_recv(), Err(oneshot::error::TryRecvError::Empty)));
    advance(Duration::from_secs(5)).await;
    assert!(matches!(response.await.expect("stalled primary send should report its age limit"), Err(Error::SimulatorSend(_))));

    let neighbor_send = spawn({
        let commands = harness.commands.clone();
        let id = harness.neighbor_id;
        async move { send_targeted(&commands, id, object_request()).await }
    });
    let sent = receive_message(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert!(neighbor_send.await.expect("send task").is_ok());
    harness.neighbor.send_to(&ack(41, sent.sequence()), harness.neighbor_peer).await.expect("ack neighbor request");

    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 42);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send unrelated reliable data");
    loop {
        if matches!(next_raw_event(&mut harness.events).await, Event::SimulatorMessage { simulator_id, sequence: 42, .. } if simulator_id == harness.neighbor_id)
        {
            break;
        }
    }
    receive_ack(&harness.neighbor, 42).await;
    harness.neighbor.send_to(&start_ping_packet(43, 9, 0), harness.neighbor_peer).await.expect("ping unrelated connection");
    receive_message(&harness.neighbor, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 9)).await;
    let (abandoned, abandoned_receiver) = oneshot::channel();
    drop(abandoned_receiver);
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed: abandoned,
        })
        .await
        .expect("admit abandoned send");
    harness.neighbor.send_to(&start_ping_packet(44, 10, 0), harness.neighbor_peer).await.expect("ping after abandoned send");
    receive_message(&harness.neighbor, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 10)).await;

    let started = tokio::time::Instant::now();
    harness.disconnect.send_replace(true);
    assert!(harness.client.await.expect("coordinator task").is_ok());
    assert!(started.elapsed() <= LOGOUT_TIMEOUT + WAIT, "bounded shutdown despite stalled send");
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
}

#[tokio::test]
async fn abandoned_send_does_not_block() {
    let harness = connected_neighbor().await;
    harness.pause_sends.send_replace(true);

    let (first, first_response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed: first,
        })
        .await
        .expect("dispatch first send");
    drop(first_response);
    timeout(WAIT, harness.send_entered.notified()).await.expect("first send should stall in the local socket");

    let (second, second_response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: object_request(),
            delivery: Delivery::Reliable,
            completed: second,
        })
        .await
        .expect("queue second send");
    drop(second_response);

    harness.pause_sends.send_replace(false);
    for ack_sequence in 24..26 {
        let sent = receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        harness.primary.send_to(&ack(ack_sequence, sent.sequence()), harness.primary_peer).await.expect("ack abandoned send");
    }

    let (completed, response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo { simulator_id: harness.primary_id, message: object_request(), delivery: Delivery::Reliable, completed })
        .await
        .expect("dispatch send after abandoned completions");
    let sent = receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    timeout(WAIT, response).await.expect("later send completion should arrive").expect("send reply").expect("later send succeeds");
    harness.primary.send_to(&ack(26, sent.sequence()), harness.primary_peer).await.expect("ack later send");

    harness.shutdown().await;
}
