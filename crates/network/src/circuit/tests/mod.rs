mod lifecycle;
mod outbound;
mod receive;

pub(in crate::circuit) use std::{
    collections::HashSet,
    future, io,
    iter::once,
    net::SocketAddr,
    num::NonZeroU64,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

pub(in crate::circuit) use protocol::{
    ChatType,
    lludp::{Message, Packet, PacketFlags},
    messages::{
        AgentMovementComplete, ChatFromViewer, CompletePingCheck, PacketAck, PingId, agent::agent_movement_complete,
        chat::chat_from_viewer, circuit::packet_ack::Packets,
    },
};
pub(in crate::circuit) use test_support::{lludp::vectors, parse_hex_fixture};
pub(in crate::circuit) use tokio::{
    net::UdpSocket,
    spawn,
    sync::{mpsc, oneshot, watch},
    task::{JoinHandle, yield_now},
    time::{advance, timeout},
};
pub(in crate::circuit) use url::Url;
pub(in crate::circuit) use uuid::Uuid;

pub(in crate::circuit) use crate::{
    AgentSession, Grid,
    circuit::{
        UdpTransport,
        lludp::{KEEPALIVE_TIMEOUT, MAX_PENDING_PACKETS, ReceiveAdmission},
        pacing::OutboundBudget,
    },
    coverage::{CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageSource},
    error::Error,
    event::{Event, SimulatorId},
};

pub(in crate::circuit) use crate::circuit::admission::SimulatorMemoryBudget;
pub(in crate::circuit) use crate::circuit::admission::{
    MAX_CONCURRENT_PACKET_DECODES, MAX_NEIGHBOR_CONNECTION_EVENT_BYTES, MAX_QUEUED_NEIGHBOR_EVENT_BYTES, MAX_QUEUED_SIMULATOR_EVENT_BYTES,
};
pub(in crate::circuit) use crate::circuit::{HandoffCancellation, HandoffGeneration, HandoffTarget};
pub(in crate::circuit) use crate::fixtures::{
    circuit::{NEW_RELIABLE_ACK_CAPACITY, SimulatorMemoryBudgetTestExt, pacer_rate, with_sequence},
    session::{HandoffEvidence, HandoffState, PendingPromotion, handoff_cancellation},
};

pub(in crate::circuit) use super::outbound::{
    MovementSend, OutboundQueue, PendingSend, PendingSendExpiration, PendingSendFuture, PreparedMeta, QueuedMessage, begin_retry_send,
    expire_deferred_retries, expire_pending_send, preempt_deferred_send, prepare_next_send, prepare_shutdown_send,
};
pub(in crate::circuit) use super::*;

const WAIT: Duration = Duration::from_secs(2);

pub(in crate::circuit) fn session(simulator_address: SocketAddr) -> AgentSession {
    AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address,
        agent_id: Uuid::nil(),
        session_id: Uuid::nil(),
        circuit_code: 1,
        seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("valid seed URL"),
    }
}

fn handoff_cancellation_for_test(source: SimulatorId, endpoint: SocketAddr) -> (PendingPromotion, HandoffCancellation) {
    let handoff = HandoffTarget {
        agent_id: Uuid::nil(),
        session_id: Uuid::nil(),
        region_handle: 42,
        endpoint,
        seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("valid seed URL"),
    };
    let pending = PendingPromotion {
        source,
        target: SimulatorId::allocate().expect("allocate target simulator ID"),
        generation: HandoffGeneration::new(NonZeroU64::new(7).expect("nonzero generation")),
        handoff,
        evidence: HandoffEvidence::default(),
    };
    let cancellation = handoff_cancellation(&pending);
    (pending, cancellation)
}

fn queued_for_test(class: TrafficClass) -> QueuedMessage {
    queued_with_delivery_for_test(class, Delivery::Unreliable)
}

fn queued_with_delivery_for_test(class: TrafficClass, delivery: Delivery) -> QueuedMessage {
    QueuedMessage {
        message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }),
        delivery,
        class,
        queued_at: Instant::now(),
        completion: None,
        cancel_target: None,
        report_failure: false,
        movement: MovementSend::None,
    }
}

fn pending_for_test(circuit: &Circuit, queued: QueuedMessage, class: TrafficClass) -> PendingSend {
    let prepared = circuit.prepare_message(queued.message.clone(), queued.delivery).expect("prepare test datagram");
    let meta = PreparedMeta::from(&prepared);
    let result: PendingSendFuture = Box::pin(future::pending());
    PendingSend { meta, result, class, queued_message: Some(queued), retry_deadline: None }
}

pub(in crate::circuit) async fn receive_packet(socket: &UdpSocket) -> (Packet, SocketAddr) {
    let mut buffer = [0; 16 * 1024];
    let (size, peer) = socket.recv_from(&mut buffer).await.expect("read datagram");
    (Packet::decode(&buffer[..size]).expect("decode simulator packet"), peer)
}

pub(in crate::circuit) async fn send_ack(socket: &UdpSocket, peer: SocketAddr, sequence: u32) {
    let bytes = Packet::new(500, Message::PacketAck(PacketAck { packets: vec![Packets { id: sequence }] }))
        .encode()
        .expect("encode simulator acknowledgement");
    socket.send_to(&bytes, peer).await.expect("send simulator acknowledgement");
}

pub(in crate::circuit) async fn receive_ack(socket: &UdpSocket, sequence: u32) {
    loop {
        let (packet, _) = receive_packet(socket).await;
        if packet.appended_acks().contains(&sequence)
            || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|ack| ack.id == sequence))
        {
            break;
        }
    }
}

pub(in crate::circuit) async fn receive_connected(events: &mut mpsc::Receiver<SimulatorEvent>) {
    let mut connected = false;
    loop {
        if let Some(SimulatorEvent::Simulator(event)) = events.recv().await {
            match event.into_event() {
                Event::SimulatorConnected { .. } => connected = true,
                Event::SimulatorMotionStatus { .. } if connected => break,
                _ => {}
            }
        }
    }
}

pub(in crate::circuit) async fn receive_message_event(events: &mut mpsc::Receiver<SimulatorEvent>, sequence: u32) -> Event {
    loop {
        if let Some(SimulatorEvent::Simulator(event)) = events.recv().await {
            let event = event.into_event();
            if matches!(event, Event::SimulatorMessage { sequence: received, .. } if received == sequence) {
                break event;
            }
        }
    }
}

async fn connect_test_primary_with_coverage(
    simulator_id: SimulatorId,
    session: AgentSession,
    coverage_losses: CoverageLossLedger,
) -> Result<SimulatorConnection, ConnectionError> {
    let budget = OutboundBudget::new(crate::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
    SimulatorConnection::connect(
        session,
        SimulatorConnectionOptions {
            simulator_id,
            region_handle: None,
            coverage_losses,
            primary_state: None,
            budget: budget.handle(),
            owned_budget: Some(budget),
            transport: None,
        },
    )
    .await
}

pub(in crate::circuit) async fn connect_test_neighbor_with_coverage(
    simulator_id: SimulatorId,
    session: AgentSession,
    region_handle: u64,
    coverage_losses: CoverageLossLedger,
) -> Result<SimulatorConnection, ConnectionError> {
    let budget = OutboundBudget::new(crate::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
    SimulatorConnection::connect(
        session,
        SimulatorConnectionOptions {
            simulator_id,
            region_handle: Some(region_handle),
            coverage_losses,
            primary_state: None,
            budget: budget.handle(),
            owned_budget: Some(budget),
            transport: None,
        },
    )
    .await
}

pub(in crate::circuit) async fn run_budget_retry(role: ConnectionRole) {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let session = session(simulator_address);
    let coverage = CoverageLossLedger::new();
    let connection = match role {
        ConnectionRole::Primary => {
            connect_test_primary_with_coverage(simulator_id, session, coverage.clone()).await.expect("connect primary simulator")
        }
        ConnectionRole::Neighbor => {
            connect_test_neighbor_with_coverage(simulator_id, session, 42, coverage.clone()).await.expect("connect neighbor simulator")
        }
    };

    let budget = SimulatorMemoryBudget::new();
    let packet_memory = match role {
        ConnectionRole::Primary => budget.clone(),
        ConnectionRole::Neighbor => budget.for_neighbor(),
    };
    let budget_view = packet_memory.clone();
    let (events, mut received_events) = mpsc::channel(4);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (_commands, commands) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
    let task: JoinHandle<Result<(), ConnectionError>> =
        spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit_code.sequence()).await;
    receive_connected(&mut received_events).await;
    if role == ConnectionRole::Primary {
        let (movement, peer) = receive_packet(&simulator).await;
        std::assert_matches!(movement.message(), Message::CompleteAgentMovement(_));
        send_ack(&simulator, peer, movement.sequence()).await;
    }

    let full_budget = budget.reserve_bytes_for_test(MAX_QUEUED_SIMULATOR_EVENT_BYTES).expect("occupy the shared byte budget");
    let mut wire = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse simulator chat fixture");
    let sequence = 700u32;
    wire[0] |= 0x40;
    wire[1..5].copy_from_slice(&sequence.to_be_bytes());
    let mut coverage_changed = coverage.subscribe();
    simulator.send_to(&wire, peer).await.expect("send packet while memory admission is full");
    timeout(WAIT, async {
        loop {
            if coverage.snapshot().losses.iter().any(|loss| {
                matches!(loss, CoverageLoss::Source(record)
                    if record.watermark.simulator_id == simulator_id
                        && record.watermark.source == CoverageSource::Lludp
                        && record.reason == CoverageLossReason::RetainedMemoryAdmission)
            }) {
                break;
            }
            coverage_changed.changed().await.expect("coverage ledger remains open");
        }
    })
    .await
    .expect("memory rejection is recorded before inspecting acknowledgements");
    drop(full_budget);
    let fence = Packet::new(sequence + 1, crate::fixtures::circuit::start_ping(77, 0)).encode().expect("encode admission fence");
    simulator.send_to(&fence, peer).await.expect("fence rejected memory admission");
    let (_, acknowledgements) = crate::fixtures::simulator::receive_message_with_acknowledgements(
        &simulator,
        |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 77),
    )
    .await;
    assert!(!acknowledgements.contains(&sequence), "memory-rejected reliable packet must not be acknowledged");
    assert!(
        coverage.snapshot().losses.iter().any(|loss| matches!(
            loss,
            CoverageLoss::Source(record)
                if record.watermark.simulator_id == simulator_id
                    && record.watermark.source == CoverageSource::Lludp
                    && record.reason == CoverageLossReason::RetainedMemoryAdmission
        )),
        "memory rejection must remain visible to Session coverage handling"
    );

    simulator.send_to(&wire, peer).await.expect("retry after byte budget recovers");
    receive_ack(&simulator, sequence).await;
    std::assert_matches!(receive_message_event(&mut received_events, sequence).await, Event::SimulatorMessage { .. });

    shutdown.send_replace(true);
    assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
    assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget_view.decode_slots_available(), MAX_CONCURRENT_PACKET_DECODES);
    if role == ConnectionRole::Neighbor {
        assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    }
}
