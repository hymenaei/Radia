use super::*;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::{Future, pending},
    io::ErrorKind,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    AgentSession, Error, Grid,
    capabilities::{CapabilityBudget, Client, EventQueueExit, begin_process_measurement},
    coverage::{CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageSource},
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY, PacketOrder::Ordered, SimulatorId},
    session::{COMMAND_CAPACITY, Command, NETWORK_THREAD_STACK_SIZE},
    simulator::coordinator::{
        CoordinatorRunState, EventQueueRoleHandle, LogoutOutcome, LogoutUnavailable, NeighborCommand, NeighborTaskOutcome, PromotionSetup,
        SessionStartup, SimulatorMemoryBudget, neighbor_completion_event,
    },
    simulator::{EventRetention, SimulatorCoordinator},
    status::{NetworkLifecycle, NetworkStatusHandle},
    transport::{
        UdpTransport,
        lludp::{Circuit, Delivery, KEEPALIVE_TIMEOUT, MAX_PENDING_PACKETS},
        pacing::{ProbeHandle, TrafficClass},
    },
};
use ::testing::lludp::fixture;
use base::UUID;
use llsd::{Map, Value};
use protocol::{
    BlockValues, ChatType, FieldValue, SchemaMessage,
    lludp::{Message, Packet},
    login::ResponseData,
    messages::{
        AgentMovementComplete, CacheMissType, ChatFromViewer, CompletePingCheck, ObjectRequest, PacketAck, PingId, RequestAgentData,
        RequestMultipleObjects, StartPingCheck, TeleportCancel,
        agent_movement_complete::{AgentData, Data, SimData},
        chat_from_viewer,
        chat_from_viewer::ChatData,
        packet_ack::Packets,
    },
};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, UdpSocket},
    spawn,
    sync::{Barrier, Notify, Semaphore, mpsc, oneshot, watch},
    task::JoinHandle,
    time::{advance, sleep, timeout},
};
use url::Url;

use super::{
    CoordinatorGate, EVENT_BACKLOG_DISCONNECT_REASON, HandoffState, HandoffTarget, LOGOUT_TIMEOUT, MAX_CONCURRENT_PACKET_DECODES,
    MAX_NEIGHBOR_CONNECTION_EVENT_BYTES, MAX_NEIGHBOR_CONNECTION_TASKS, MAX_NEIGHBOR_SIMULATORS, MAX_PENDING_SIMULATOR_EVENTS,
    MAX_PRECONNECT_EVENTS, MAX_QUEUED_NEIGHBOR_EVENT_BYTES, MAX_QUEUED_SIMULATOR_EVENT_BYTES, MAX_QUEUED_SIMULATOR_EVENTS,
    NEIGHBOR_COMMAND_CAPACITY, NeighborConnection, NeighborConnectionState, NeighborEvent, PreconnectEvents, PreconnectOutcome,
    ReplacementGate, SimulatorCapabilitySession, queue_neighbor_event, queue_until_connected, run_session,
};
const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
const CIRCUIT_CODE: u32 = 0x1122_3344;
const WAIT: Duration = Duration::from_secs(2);
const SESSION_EVENT_CAPACITY: usize = 2;
const TEST_EVENT_CAPACITY: usize = CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY;

pub(super) fn login_session(simulator_address: SocketAddr) -> AgentSession {
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
    std::thread::Builder::new()
        .name("network-handoff-test".into())
        .stack_size(NETWORK_THREAD_STACK_SIZE)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("handoff runtime");
            runtime.block_on(test());
        })
        .expect("handoff test thread")
        .join()
        .expect("handoff test completes");
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
    pacer_probe: Option<ProbeHandle>,
}

impl NeighborHarness {
    async fn shutdown(self) {
        self.disconnect.send_replace(true);
        receive_message(&self.primary, |message| matches!(message, Message::LogoutRequest(_))).await;
        assert!(timeout(Duration::from_millis(200), self.client).await.expect("disconnect should finish").expect("client task").is_ok());
    }
}

async fn connected_neighbor() -> NeighborHarness {
    connected_neighbor_with_options(Url::parse("http://127.0.0.1:1/seed").expect("fixture seed"), None, None).await
}

async fn connected_neighbor_with_seed(seed: Url) -> NeighborHarness {
    connected_neighbor_with_options(seed, None, None).await
}

async fn connected_neighbor_with_promotion_gate(gate: CoordinatorGate) -> NeighborHarness {
    connected_neighbor_with_options(Url::parse("http://127.0.0.1:1/seed").expect("fixture seed"), Some(gate), None).await
}

async fn connected_neighbor_with_replacement_gate(gate: ReplacementGate) -> NeighborHarness {
    connected_neighbor_with_options(Url::parse("http://127.0.0.1:1/seed").expect("fixture seed"), None, Some(gate)).await
}

async fn connected_neighbor_with_options(
    seed: Url,
    promotion_gate: Option<CoordinatorGate>,
    replacement_gate: Option<ReplacementGate>,
) -> NeighborHarness {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let primary_endpoint = primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(TEST_EVENT_CAPACITY);
    let (disconnect, disconnect_requested) = watch::channel(false);
    let status = NetworkStatusHandle::default();
    let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
    let mut connection = SimulatorCoordinator::connect_with_status_and_send_gate(
        AgentSession { seed_capability: seed, ..login_session(primary_endpoint) },
        ResponseData::default(),
        None,
        status.clone(),
        send_gate,
    )
    .await
    .expect("connect primary simulator");
    connection.promotion_gate = promotion_gate;
    connection.replacement_gate = replacement_gate;
    let packet_memory = connection.packet_memory.clone();
    let pacer_probe = connection.outbound_budget.as_ref().and_then(|budget| budget.handle().probe());
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
        pacer_probe,
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
    let (size, endpoint) = timeout(WAIT, socket.recv_from(&mut buffer)).await.expect("receive simulator datagram").expect("read datagram");
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

async fn receive_ack(socket: &UdpSocket, sequence: u32) -> Packet {
    timeout(WAIT, async {
        loop {
            let (bytes, _) = receive(socket).await;
            let packet = Packet::decode(&bytes).expect("decode simulator acknowledgement");
            if packet.appended_acks().contains(&sequence)
                || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|item| item.id == sequence))
            {
                return packet;
            }
        }
    })
    .await
    .expect("receive reliable packet acknowledgement")
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
        .send(Command::SendTo { simulator_id, message, delivery: Delivery::Reliable, completed, dispatch_probe: None })
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

fn burst_object_request() -> Message {
    const OBJECTS_PER_REQUEST: usize = u8::MAX as usize;
    let mut objects = Vec::with_capacity(OBJECTS_PER_REQUEST);
    for index in 0..OBJECTS_PER_REQUEST {
        objects.push(ObjectRequest {
            cache_miss_type: CacheMissType::Full,
            local_id: u32::try_from(index).expect("outbound object index fits u32"),
        });
    }
    Message::RequestMultipleObjects(RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: UUID::parse_str(AGENT_ID).expect("agent UUID"),
            session_id: UUID::parse_str(SESSION_ID).expect("session UUID"),
        },
        objects,
    })
}

fn chat_message(message: impl Into<String>) -> Message {
    Message::from(ChatFromViewer {
        agent_data: chat_from_viewer::AgentData {
            agent_id: UUID::parse_str(AGENT_ID).expect("valid test agent ID"),
            session_id: UUID::parse_str(SESSION_ID).expect("valid test session ID"),
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

async fn next_neighbor_event(events: &mut mpsc::Receiver<NeighborEvent>) -> Event {
    timeout(WAIT, async {
        match events.recv().await.expect("neighbor event channel remains open") {
            NeighborEvent::Simulator(event) => event.into_event(),
            NeighborEvent::SendCompleted { .. } | NeighborEvent::Finished { .. } => {
                panic!("a standalone neighbor worker must not emit its manager completion")
            }
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

fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}

#[path = "coordination.rs"]
mod coordination;
#[path = "coverage.rs"]
mod coverage;
#[path = "handoff.rs"]
mod handoff_cases;
#[path = "handoff_return.rs"]
mod handoff_return;
#[path = "load.rs"]
mod load;
#[path = "neighbor.rs"]
mod neighbor;
#[path = "neighbor_control.rs"]
mod neighbor_control;
#[path = "routing.rs"]
mod routing;
#[path = "session.rs"]
mod session;
#[path = "startup.rs"]
mod startup;
