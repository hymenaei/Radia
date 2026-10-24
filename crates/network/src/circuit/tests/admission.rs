mod memory {
    use super::*;

    use crate::circuit::admission::{
        MAX_NEIGHBOR_CONNECTION_EVENT_BYTES, MAX_PACKET_ACK_DECODE_BYTES, MAX_QUEUED_NEIGHBOR_EVENT_BYTES,
        MAX_QUEUED_SIMULATOR_EVENT_BYTES, PacketMemoryError, SimulatorMemoryBudget, packet_event_overhead_bytes,
    };
    use crate::event::{Event, EventEnvelope, PacketOrder::Ordered, SimulatorId};
    use crate::fixtures::circuit::SimulatorMemoryBudgetTestExt;
    use protocol::{
        lludp::{Message, MessageId, Packet},
        messages::{PacketAck, circuit::packet_ack::Packets},
    };
    use std::net::SocketAddr;
    use test_support::{lludp::vectors, parse_hex_fixture};

    fn fixture() -> Vec<u8> {
        parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("valid simulator chat fixture")
    }

    #[test]
    fn ack_decode_slots_are_bounded() {
        let budget = SimulatorMemoryBudget::new();
        let mut ack =
            Packet::new(1, Message::PacketAck(PacketAck { packets: (0..u8::MAX).map(|id| Packets { id: u32::from(id) }).collect() }))
                .with_extra_header(vec![0x42; usize::from(u8::MAX)])
                .with_appended_acks(vec![7; usize::from(u8::MAX)])
                .with_zero_coding(false)
                .encode()
                .expect("encode maximum-sized ACK control packet");
        let appended_ack_bytes = usize::from(u8::MAX) * size_of::<u32>() + 1;
        let ack_section_start = ack.len() - appended_ack_bytes;
        let padding_len = 8 * 1024 - ack.len();
        ack.splice(ack_section_start..ack_section_start, vec![0x55; padding_len]);
        assert_eq!(ack.len(), 8 * 1024, "the packet fills the protocol datagram limit");
        let preflight = Packet::preflight(&ack).expect("preflight maximum-sized ACK control packet");
        assert!(preflight.peak_decode_allocation_bytes() <= MAX_PACKET_ACK_DECODE_BYTES);

        let mut permits = (0..crate::circuit::MAX_SIMULATOR_CONNECTIONS)
            .map(|_| budget.try_acquire_ack_decode_slot(preflight).expect("each active simulator has one control decode slot"))
            .collect::<Vec<_>>();
        assert!(budget.try_acquire_ack_decode_slot(preflight).is_none(), "the control decode budget has no extra slot");
        drop(permits.pop());
        assert!(budget.try_acquire_ack_decode_slot(preflight).is_some(), "dropping a control decode permit restores capacity");
    }

    fn simulator_address() -> SocketAddr {
        "127.0.0.1:13000".parse().expect("simulator address")
    }

    #[tokio::test]
    async fn retained_packet_memory_lives_until_dequeue() {
        let budget = SimulatorMemoryBudget::new();
        let wire = fixture();
        let preflight = Packet::preflight(&wire).expect("preflight simulator message");
        let mut reservation = budget.reserve_packet(preflight).expect("reserve in-flight memory");
        let packet = Packet::decode(&wire).expect("decode simulator message");
        budget.trim_packet_reservation(&mut reservation, &packet).expect("trim reservation to retained capacity");
        let retained_bytes = packet.retained_capacity_bytes() + packet_event_overhead_bytes();
        let event = event_with_retention(
            Event::SimulatorMessage {
                simulator_id: SimulatorId::allocate().expect("allocate simulator ID"),
                source: simulator_address(),
                sequence: packet.sequence(),
                packet_order: Ordered(1),
                message: packet.into_message(),
            },
            reservation.into_retention(),
        );
        let (events, mut received) = mpsc::channel(1);

        events.send(event).await.expect("queue simulator event");
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES - retained_bytes);

        std::assert_matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::SimulatorMessage { .. }));
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    }

    #[test]
    fn rejected_admission_releases_bytes() {
        let budget = SimulatorMemoryBudget::new();
        let preflight = Packet::preflight(&fixture()).expect("preflight simulator message");
        let full_budget = budget.reserve_bytes_for_test(MAX_QUEUED_SIMULATOR_EVENT_BYTES).expect("reserve full event budget");

        assert!(matches!(budget.reserve_packet(preflight), Err(PacketMemoryError::GlobalBudget)));
        assert_eq!(budget.global_available_bytes(), 0);
        assert_eq!(budget.connection_available_bytes(), 0);

        drop(full_budget);
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    }

    #[test]
    fn neighbors_preserve_primary_headroom() {
        let preflight = Packet::preflight(&fixture()).expect("preflight simulator message");
        let packet_bytes = preflight.peak_decode_allocation_bytes() + packet_event_overhead_bytes();
        let primary = SimulatorMemoryBudget::with_test_limits(packet_bytes * 10, packet_bytes * 4, packet_bytes * 2);
        let neighbors = [primary.for_neighbor(), primary.for_neighbor()];
        let mut reservations = Vec::new();
        for neighbor in &neighbors {
            for _ in 0..2 {
                reservations.push(neighbor.reserve_packet(preflight).expect("reserve neighbor packet up to per-connection limit"));
            }
        }
        let remaining = primary.neighbor_available_bytes();
        assert_eq!(remaining, 0, "neighbors should use the aggregate limit exactly");
        assert_eq!(primary.global_available_bytes(), packet_bytes * 6);
        assert!(matches!(primary.for_neighbor().reserve_packet(preflight), Err(PacketMemoryError::NeighborBudget)));
        drop(primary.reserve_packet(preflight).expect("primary can use protected headroom"));
        drop(reservations);
        assert_eq!(primary.global_available_bytes(), packet_bytes * 10);
        assert_eq!(primary.neighbor_available_bytes(), packet_bytes * 4);
        for neighbor in neighbors {
            assert_eq!(neighbor.connection_available_bytes(), packet_bytes * 2);
        }
    }

    #[test]
    fn promotion_reclassifies_retained_memory() {
        let root = SimulatorMemoryBudget::with_test_limits(1_000, 500, 200);
        let former_primary = root.clone();
        let promoted = root.for_neighbor();
        let other_neighbor = root.for_neighbor();
        let former_backlog = former_primary.reserve_bytes_for_test(520).expect("primary may retain a larger backlog");
        let promoted_backlog = promoted.reserve_bytes_for_test(80).expect("neighbor may retain its backlog");
        let other_backlog = other_neighbor.reserve_bytes_for_test(200).expect("another neighbor fills its connection allowance");

        assert_eq!(root.global_available_bytes(), 200);
        assert_eq!(root.neighbor_available_bytes(), 220);

        former_primary.promote_to(&promoted);

        assert_eq!(root.neighbor_available_bytes(), 0, "over-limit retained usage stays charged after demotion");
        assert_eq!(former_primary.connection_available_bytes(), 0, "a demoted connection cannot keep its former primary allowance");
        assert_eq!(promoted.connection_available_bytes(), 920, "promoted retained bytes move to the primary connection allowance");
        assert!(matches!(
            former_primary.reserve_bytes_for_test(1),
            Err(PacketMemoryError::NeighborBudget | PacketMemoryError::ConnectionBudget)
        ));
        assert!(matches!(other_neighbor.reserve_bytes_for_test(1), Err(PacketMemoryError::NeighborBudget)));

        let new_primary = promoted.reserve_bytes_for_test(100).expect("promoted primary can use remaining global headroom");
        assert_eq!(root.global_available_bytes(), 100);
        drop(former_backlog);
        assert_eq!(root.global_available_bytes(), 620);
        assert_eq!(root.neighbor_available_bytes(), 300, "dropping former-primary retention releases its new neighbor charge");
        drop(promoted_backlog);
        drop(new_primary);
        assert_eq!(root.neighbor_available_bytes(), 300, "primary retention does not remain charged to the neighbor pool");
        drop(other_backlog);
        assert_eq!(root.global_available_bytes(), 1_000);
        assert_eq!(root.neighbor_available_bytes(), 500);
    }

    #[test]
    fn demoted_retention_releases_after_handle_drop() {
        let root = SimulatorMemoryBudget::with_test_limits(1_000, 500, 200);
        let former = root.clone();
        let promoted = root.for_neighbor();
        let retained = former.reserve_bytes_for_test(200).expect("primary may retain its packet");

        former.promote_to(&promoted);
        drop(former);
        drop(root);

        assert_eq!(promoted.neighbor_available_bytes(), 300);
        drop(retained);
        assert_eq!(promoted.neighbor_available_bytes(), 500);
    }

    #[test]
    fn repeated_promotions_reclassify_retained_memory() {
        let root = SimulatorMemoryBudget::with_test_limits(2_000, 600, 400);
        let first = root.clone();
        let second = root.for_neighbor();
        let third = root.for_neighbor();
        let first_backlog = first.reserve_bytes_for_test(500).expect("primary may retain its backlog");
        let second_backlog = second.reserve_bytes_for_test(100).expect("neighbor may retain its backlog");
        let third_backlog = third.reserve_bytes_for_test(200).expect("neighbor may retain its backlog");

        first.promote_to(&second);
        assert_eq!(root.neighbor_available_bytes(), 0, "demoted primary bytes remain charged to neighbors");
        second.promote_to(&third);

        assert_eq!(root.neighbor_available_bytes(), 0, "each promotion reclassifies both connections' retained bytes");
        assert_eq!(first.connection_available_bytes(), 0, "the oldest demoted connection loses primary headroom");
        assert_eq!(second.connection_available_bytes(), 300, "the former promoted connection gets neighbor headroom");
        assert_eq!(third.connection_available_bytes(), 1_800, "the latest primary keeps its own retained bytes");
        assert!(matches!(second.reserve_bytes_for_test(1), Err(PacketMemoryError::NeighborBudget)));

        drop(first_backlog);
        assert_eq!(root.neighbor_available_bytes(), 500, "releasing old-primary retention restores aggregate neighbor capacity");
        let second_extra = second.reserve_bytes_for_test(300).expect("demoted connection can use its neighbor allowance");
        assert_eq!(root.neighbor_available_bytes(), 200);

        drop(second_backlog);
        drop(second_extra);
        drop(third_backlog);
        assert_eq!(root.global_available_bytes(), 2_000);
        assert_eq!(root.neighbor_available_bytes(), 600);
    }

    #[test]
    fn in_flight_decode_trims_after_promotion() {
        let root = SimulatorMemoryBudget::with_test_limits(1_000_000, 500_000, 250_000);
        let former_primary = root.clone();
        let promoted = root.for_neighbor();
        let wire = fixture();
        let preflight = Packet::preflight(&wire).expect("preflight simulator message");
        let peak_bytes = preflight.peak_decode_allocation_bytes() + packet_event_overhead_bytes();
        let mut reservation = former_primary.reserve_packet(preflight).expect("reserve in-flight decode");
        let packet = Packet::decode(&wire).expect("decode simulator message");
        let retained_bytes = packet.retained_capacity_bytes() + packet_event_overhead_bytes();
        assert!(retained_bytes < peak_bytes, "fixture must release temporary decode headroom");

        former_primary.promote_to(&promoted);

        assert_eq!(root.neighbor_available_bytes(), 500_000 - peak_bytes);
        assert_eq!(former_primary.connection_available_bytes(), 250_000 - peak_bytes);
        assert_eq!(promoted.connection_available_bytes(), 1_000_000);

        former_primary
            .trim_packet_reservation(&mut reservation, &packet)
            .expect("trim reservation using the demoted connection's new role");
        assert_eq!(root.global_available_bytes(), 1_000_000 - retained_bytes);
        assert_eq!(root.neighbor_available_bytes(), 500_000 - retained_bytes);
        assert_eq!(former_primary.connection_available_bytes(), 250_000 - retained_bytes);
        assert_eq!(promoted.connection_available_bytes(), 1_000_000);

        drop(reservation);
        assert_eq!(root.global_available_bytes(), 1_000_000);
        assert_eq!(root.neighbor_available_bytes(), 500_000);
        assert_eq!(former_primary.connection_available_bytes(), 250_000);
    }

    #[test]
    fn neighbor_limits_are_independent() {
        let primary = SimulatorMemoryBudget::with_test_limits(500, 400, 200);
        let saturated = primary.for_neighbor();
        let available = primary.for_neighbor();
        let full_connection = saturated.reserve_bytes_for_test(200).expect("occupy one neighbor connection budget");
        assert!(matches!(saturated.reserve_bytes_for_test(1), Err(PacketMemoryError::ConnectionBudget)));
        assert_eq!(primary.global_available_bytes(), 300);
        assert_eq!(saturated.neighbor_available_bytes(), 200);
        drop(available.reserve_bytes_for_test(200).expect("other neighbor has independent capacity"));

        drop(full_connection);
        assert_eq!(saturated.connection_available_bytes(), 200);
        assert_eq!(primary.neighbor_available_bytes(), 400);
    }

    #[test]
    fn decode_failure_releases_packet_budgets() {
        let budget = SimulatorMemoryBudget::new().for_neighbor();
        let packet = Packet::new(1, Message::PacketAck(PacketAck { packets: vec![Packets { id: 1 }] }))
            .encode()
            .expect("encode acknowledgement packet");
        let mut truncated = packet;
        truncated.truncate(truncated.len() - 4);
        let preflight = Packet::preflight(&truncated).expect("preflight count before truncated ACK body");
        let reservation = budget.reserve_packet(preflight).expect("reserve malformed but bounded decode");
        assert!(Packet::decode(&truncated).is_err(), "typed decoder rejects missing ACK body");
        drop(reservation);
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

        drop(budget.reserve_packet(preflight).expect("reserve next packet"));
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    }

    #[test]
    fn underestimated_packet_releases_memory() {
        let budget = SimulatorMemoryBudget::new().for_neighbor();
        use protocol::messages::{CompletePingCheck, PingId};

        let small = Packet::new(1, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }))
            .encode()
            .expect("encode small packet");
        let bound = Packet::preflight(&small).expect("preflight small packet");
        let large = Packet::decode(&fixture()).expect("decode larger packet");
        let mut reservation = budget.reserve_packet(bound).expect("reserve smaller estimate");
        assert_eq!(budget.trim_packet_reservation(&mut reservation, &large), Err(PacketMemoryError::BoundExceeded));
        drop(large);
        drop(reservation);
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    }

    #[test]
    fn temporary_decode_headroom_does_not_hide_a_retained_bound_mismatch() {
        let small = Packet::new(1, Message::Unknown { id: MessageId::low(0xfff9), payload: vec![0; 4] })
            .with_zero_coding(true)
            .encode()
            .expect("encode zero-coded packet with smaller retained payload");
        let preflight = Packet::preflight(&small).expect("preflight packet");
        let larger = Packet::new(2, Message::Unknown { id: MessageId::low(0xfff9), payload: vec![0x55; 6] })
            .encode()
            .expect("encode packet with a slightly larger retained payload");
        let packet = Packet::decode(&larger).expect("decode small retained payload");
        let retained = packet.retained_capacity_bytes();

        assert!(
            retained > preflight.retained_capacity_upper_bound(),
            "retained={retained}, bound={}",
            preflight.retained_capacity_upper_bound()
        );
        assert!(
            retained <= preflight.peak_decode_allocation_bytes(),
            "retained={retained}, peak={}",
            preflight.peak_decode_allocation_bytes()
        );

        let budget = SimulatorMemoryBudget::new();
        let mut reservation = budget.reserve_packet(preflight).expect("reserve the conservative peak bound");
        assert_eq!(budget.trim_packet_reservation(&mut reservation, &packet), Err(PacketMemoryError::BoundExceeded));
    }
}

use super::*;
use crate::circuit::tests::*;
use crate::event::{Event, SimulatorId};
use crate::fixtures::{circuit::SimulatorMemoryBudgetTestExt, event_with_retention};
use protocol::{
    lludp::{Message, MessageId, Packet},
    messages::{PacketAck, circuit::packet_ack::Packets},
};
use std::time::Duration;
use test_support::{lludp::vectors, parse_hex_fixture};
use tokio::sync::mpsc;

const WAIT: Duration = Duration::from_secs(2);

async fn wait_for_coverage_loss(
    changed: &mut watch::Receiver<u64>,
    coverage: &CoverageLossLedger,
    simulator_id: SimulatorId,
    reason: CoverageLossReason,
) {
    loop {
        if coverage.snapshot().losses.iter().any(|loss| {
            matches!(
                loss,
                CoverageLoss::Source(record)
                    if record.watermark.simulator_id == simulator_id
                        && record.watermark.source == CoverageSource::Lludp
                        && record.reason == reason
            )
        }) {
            break;
        }
        changed.changed().await.expect("coverage ledger remains available");
    }
}

#[test]
fn preflight_resource_classification() {
    for error in [LLUdpError::DecodedPacketTooLarge, LLUdpError::AllocationBoundOverflow] {
        assert_eq!(crate::circuit::preflight_loss_reason(error, protocol::schema::ids::OBJECT_UPDATE), CoverageLossReason::DecodeLimit);
    }
    for error in [LLUdpError::TruncatedPacket, LLUdpError::InvalidMessage, LLUdpError::InvalidZeroCode] {
        assert_eq!(crate::circuit::preflight_loss_reason(error, MessageId::high(0x71)), CoverageLossReason::PreflightRejected);
    }
    assert_eq!(
        crate::circuit::preflight_loss_reason(LLUdpError::InvalidMessageId, protocol::schema::ids::OBJECT_UPDATE),
        CoverageLossReason::AllocationEstimateMismatch,
        "an unsupported specialized route is an internal preflight defect"
    );
    assert_eq!(
        crate::circuit::preflight_loss_reason(LLUdpError::InvalidMessageId, MessageId::high(0x71)),
        CoverageLossReason::PreflightRejected
    );
}

#[tokio::test]
async fn byte_pressure_retries_for_both_roles() {
    run_budget_retry(ConnectionRole::Primary).await;
    run_budget_retry(ConnectionRole::Neighbor).await;
}

#[tokio::test]
async fn rejected_packet_retries() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let coverage = CoverageLossLedger::new();
    let connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, coverage.clone())
        .await
        .expect("connect neighbor simulator");
    let budget = SimulatorMemoryBudget::new();
    let packet_memory = budget.for_neighbor();
    let budget_view = packet_memory.clone();
    let (events, mut received_events) = mpsc::channel(4);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (_commands, commands) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
    let task = spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit_code.sequence()).await;
    receive_connected(&mut received_events).await;
    let mut coverage_changed = coverage.subscribe();

    let sequence = 700u32;
    let mut malformed = parse_hex_fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE).expect("parse terse update fixture");
    malformed[0] |= 0x40;
    malformed[1..5].copy_from_slice(&sequence.to_be_bytes());
    malformed[18] = u8::MAX;
    assert_eq!(
        protocol::lludp::Packet::preflight(&malformed),
        Err(LLUdpError::TruncatedPacket),
        "preflight rejects an object field that exceeds its packet frame"
    );
    assert!(Packet::decode(&malformed).is_err(), "decoder must reject malformed object framing");
    simulator.send_to(&malformed, peer).await.expect("send malformed reliable update");

    wait_for_coverage_loss(&mut coverage_changed, &coverage, simulator_id, CoverageLossReason::PreflightRejected).await;
    let mut buffer = [0; 2048];
    std::assert_matches!(simulator.try_recv_from(&mut buffer), Err(error) if error.kind() == io::ErrorKind::WouldBlock,
        "preflight-rejected packet must not be acknowledged"
    );
    assert_eq!(budget_view.decode_slots_available(), MAX_CONCURRENT_PACKET_DECODES);
    assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

    let mut retry = parse_hex_fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE).expect("parse terse update fixture");
    retry[0] |= 0x40;
    retry[1..5].copy_from_slice(&sequence.to_be_bytes());
    simulator.send_to(&retry, peer).await.expect("retry reliable update");
    receive_ack(&simulator, sequence).await;
    std::assert_matches!(receive_message_event(&mut received_events, sequence).await, Event::SimulatorMessage { .. });

    shutdown.send_replace(true);
    assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
    assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget_view.decode_slots_available(), MAX_CONCURRENT_PACKET_DECODES);
    assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
}

#[tokio::test]
async fn typed_decode_retries() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let coverage = CoverageLossLedger::new();
    let connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, coverage.clone())
        .await
        .expect("connect neighbor simulator");
    let packet_memory = SimulatorMemoryBudget::new().for_neighbor();
    let budget_view = packet_memory.clone();
    let (events, mut received_events) = mpsc::channel(4);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (_commands, commands) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
    let task = spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit_code.sequence()).await;
    receive_connected(&mut received_events).await;
    let mut coverage_changed = coverage.subscribe();

    let sequence = 701u32;
    let mut retry = parse_hex_fixture(vectors::START_PING_CHECK).expect("parse ping fixture");
    retry[0] |= PacketFlags::RELIABLE.bits();
    retry[1..5].copy_from_slice(&sequence.to_be_bytes());
    let ping_id = match Packet::decode(&retry).expect("decode ping fixture").message() {
        Message::StartPingCheck(message) => message.ping_id.ping_id,
        message => panic!("expected a ping check, got {message:?}"),
    };
    let mut malformed = retry.clone();
    malformed.pop();
    assert!(Packet::preflight(&malformed).is_ok(), "allocation preflight accepts the bounded no-allocation payload");
    assert!(Packet::decode(&malformed).is_err(), "typed decoder rejects the truncated ping field");
    simulator.send_to(&malformed, peer).await.expect("send packet rejected by typed decode");

    wait_for_coverage_loss(&mut coverage_changed, &coverage, simulator_id, CoverageLossReason::DecodeRejected).await;
    let mut buffer = [0; 2048];
    std::assert_matches!(simulator.try_recv_from(&mut buffer), Err(error) if error.kind() == io::ErrorKind::WouldBlock,
        "packet rejected by typed decode must not be acknowledged or answered"
    );
    assert_eq!(budget_view.decode_slots_available(), MAX_CONCURRENT_PACKET_DECODES);
    assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

    simulator.send_to(&retry, peer).await.expect("retry packet with the same sequence");
    let mut acknowledged = false;
    let mut answered = false;
    timeout(WAIT, async {
        while !acknowledged || !answered {
            let (packet, _) = receive_packet(&simulator).await;
            acknowledged |= packet.appended_acks().contains(&sequence)
                || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|ack| ack.id == sequence));
            answered |= matches!(packet.message(), Message::CompletePingCheck(message) if message.ping_id.ping_id == ping_id);
        }
    })
    .await
    .expect("retry must be acknowledged and answered with its ping ID");

    shutdown.send_replace(true);
    assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
    assert_eq!(budget_view.decode_slots_available(), MAX_CONCURRENT_PACKET_DECODES);
    assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
}

#[tokio::test(start_paused = true)]
async fn ack_queue_full_retries() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let coverage = CoverageLossLedger::new();
    let connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, coverage.clone())
        .await
        .expect("connect neighbor simulator");
    let shared_budget = SimulatorMemoryBudget::new();
    let packet_memory = shared_budget.for_neighbor();
    let budget_view = packet_memory.clone();
    let (events, mut received_events) = mpsc::channel(4);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (_commands, commands) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
    let task = spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit_code.sequence()).await;
    receive_connected(&mut received_events).await;

    let duplicate_sequence = 9_000u32;
    let mut duplicate = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse simulator chat fixture");
    duplicate[0] |= 0x40;
    duplicate[1..5].copy_from_slice(&duplicate_sequence.to_be_bytes());
    simulator.send_to(&duplicate, peer).await.expect("send reliable chat");
    receive_ack(&simulator, duplicate_sequence).await;
    let duplicate_retention = loop {
        let Some(SimulatorEvent::Simulator(envelope)) = received_events.recv().await else { continue };
        let (event, retention, _) = envelope.into_parts();
        if matches!(event, Event::SimulatorMessage { sequence, .. } if sequence == duplicate_sequence) {
            break retention.expect("received chat retains its packet memory");
        }
    };

    let first_sequence = 10_000;
    let rejected_sequence = first_sequence + NEW_RELIABLE_ACK_CAPACITY as u32;
    for sequence in first_sequence..rejected_sequence {
        let bytes = Packet::new(sequence, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 0 } }))
            .with_reliable()
            .encode()
            .expect("encode reliable ping response");
        simulator.send_to(&bytes, peer).await.expect("send reliable ping response");
        if sequence % 128 == 0 {
            yield_now().await;
        }
    }
    let mut rejected = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse chat fixture");
    rejected[0] |= 0x40;
    rejected[1..5].copy_from_slice(&rejected_sequence.to_be_bytes());
    let mut coverage_changed = coverage.subscribe();
    simulator.send_to(&rejected, peer).await.expect("send packet with a full ACK queue");

    wait_for_coverage_loss(&mut coverage_changed, &coverage, simulator_id, CoverageLossReason::AcknowledgementAdmission).await;

    let global_bytes =
        shared_budget.reserve_bytes_for_test(budget_view.global_available_bytes()).expect("hold the remaining global packet-memory budget");
    assert_eq!(budget_view.global_available_bytes(), 0);
    duplicate[0] |= PacketFlags::RESENT.bits();
    simulator.send_to(&duplicate, peer).await.expect("resend accepted chat while both budgets are full");

    let mut buffer = [0; 2048];
    std::assert_matches!(simulator.try_recv_from(&mut buffer), Err(error) if error.kind() == io::ErrorKind::WouldBlock,
        "no queued ACK should be sent before its timer or another outbound packet"
    );

    advance(ACK_INTERVAL).await;
    let mut acknowledged = HashSet::with_capacity(NEW_RELIABLE_ACK_CAPACITY + 1);
    while acknowledged.len() < NEW_RELIABLE_ACK_CAPACITY + 1 {
        let (packet, _) = receive_packet(&simulator).await;
        let Message::PacketAck(message) = packet.message() else { panic!("expected a standalone ACK packet") };
        for ack in &message.packets {
            assert!(acknowledged.insert(ack.id), "an accepted reliable sequence is ACKed once");
        }
    }
    for sequence in first_sequence..rejected_sequence {
        assert!(acknowledged.contains(&sequence), "accepted sequence {sequence} must be ACKed after capacity drains");
    }
    assert!(acknowledged.contains(&duplicate_sequence), "duplicate reliable packets remain re-ACKable at ACK capacity");
    assert!(!acknowledged.contains(&rejected_sequence), "the unadmitted sequence must not be ACKed");
    assert!(received_events.try_recv().is_err(), "the packet rejected by ACK admission must not be delivered");
    assert!(
        !coverage.snapshot().losses.iter().any(|loss| matches!(
            loss,
            CoverageLoss::Source(record)
                if record.watermark.simulator_id == simulator_id
                    && record.watermark.source == CoverageSource::Lludp
                    && record.reason == CoverageLossReason::RetainedMemoryAdmission
        )),
        "a duplicate should bypass payload-memory admission"
    );

    simulator.send_to(&duplicate, peer).await.expect("resend accepted chat with only byte memory exhausted");
    advance(ACK_INTERVAL).await;
    receive_ack(&simulator, duplicate_sequence).await;
    assert!(received_events.try_recv().is_err(), "re-ACKing a duplicate must not redeliver it");

    drop(global_bytes);
    drop(duplicate_retention);

    simulator.send_to(&rejected, peer).await.expect("retry after ACK capacity drains");
    advance(ACK_INTERVAL).await;
    receive_ack(&simulator, rejected_sequence).await;
    std::assert_matches!(receive_message_event(&mut received_events, rejected_sequence).await, Event::SimulatorMessage { .. });

    shutdown.send_replace(true);
    assert!(task.await.expect("join task").is_ok());
}

#[tokio::test]
async fn ack_control_survives_memory_pressure() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let coverage = CoverageLossLedger::new();
    let connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, coverage.clone())
        .await
        .expect("connect neighbor simulator");

    let shared_budget = SimulatorMemoryBudget::new();
    let packet_memory = shared_budget.for_neighbor();
    let (events, mut received_events) = mpsc::channel(4);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (_commands, commands) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
    let task: JoinHandle<Result<(), ConnectionError>> =
        spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory.clone()));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    let _decode_slots = packet_memory.hold_all_decode_slots().await;
    let _global_bytes = shared_budget.reserve_bytes_for_test(MAX_QUEUED_SIMULATOR_EVENT_BYTES).expect("occupy global packet memory");

    let mut malformed_ack = Packet::new(700, Message::PacketAck(PacketAck { packets: vec![Packets { id: use_circuit_code.sequence() }] }))
        .encode()
        .expect("encode circuit ACK");
    malformed_ack.truncate(11);
    assert!(Packet::preflight(&malformed_ack).is_ok(), "preflight should bound the fixed-width ACK allocation");
    assert!(Packet::decode(&malformed_ack).is_err(), "the typed ACK decoder should reject its truncated entry");
    let mut coverage_changed = coverage.subscribe();
    simulator.send_to(&malformed_ack, peer).await.expect("send malformed circuit ACK under memory pressure");
    wait_for_coverage_loss(&mut coverage_changed, &coverage, simulator_id, CoverageLossReason::DecodeRejected).await;

    let packets = once(Packets { id: use_circuit_code.sequence() }).chain((0..254).map(|id| Packets { id })).collect();
    let ack = Packet::new(701, Message::PacketAck(PacketAck { packets })).encode().expect("encode maximum-sized circuit ACK");
    simulator.send_to(&ack, peer).await.expect("send circuit ACK under memory pressure");
    receive_connected(&mut received_events).await;

    shutdown.send_replace(true);
    assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
}
