use super::*;

async fn wait_for_coverage_loss(
    changed: &mut watch::Receiver<u64>,
    coverage: &CoverageLossLedger,
    simulator_id: SimulatorId,
    reason: CoverageLossReason,
) {
    timeout(WAIT, async {
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
    })
    .await
    .expect("expected coverage loss to be recorded");
}

#[test]
fn preflight_resource_classification() {
    for error in [LLUdpError::DecodedPacketTooLarge, LLUdpError::AllocationBoundOverflow] {
        assert_eq!(super::preflight_loss_reason(error, protocol::schema::ids::OBJECT_UPDATE), CoverageLossReason::DecodeLimit);
    }
    for error in [LLUdpError::TruncatedPacket, LLUdpError::InvalidMessage, LLUdpError::InvalidZeroCode] {
        assert_eq!(super::preflight_loss_reason(error, MessageId::high(0x71)), CoverageLossReason::PreflightRejected);
    }
    assert_eq!(
        super::preflight_loss_reason(LLUdpError::InvalidMessageId, protocol::schema::ids::OBJECT_UPDATE),
        CoverageLossReason::AllocationEstimateMismatch,
        "an unsupported specialized route is an internal preflight defect"
    );
    assert_eq!(super::preflight_loss_reason(LLUdpError::InvalidMessageId, MessageId::high(0x71)), CoverageLossReason::PreflightRejected);
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
    let connection =
        connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, NetworkStatusHandle::default(), coverage.clone())
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
    assert!(
        matches!(simulator.try_recv_from(&mut buffer), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
        "preflight-rejected packet must not be acknowledged"
    );
    assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
    assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

    let mut retry = parse_hex_fixture(vectors::IMPROVED_TERSE_OBJECT_UPDATE).expect("parse terse update fixture");
    retry[0] |= 0x40;
    retry[1..5].copy_from_slice(&sequence.to_be_bytes());
    simulator.send_to(&retry, peer).await.expect("retry reliable update");
    receive_ack(&simulator, sequence).await;
    assert!(matches!(receive_message_event(&mut received_events, sequence).await, Event::SimulatorMessage { .. }));

    shutdown.send_replace(true);
    assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
    assert_eq!(budget_view.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
    assert_eq!(budget_view.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget_view.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
}

#[tokio::test]
async fn typed_decode_retries() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let coverage = CoverageLossLedger::new();
    let connection =
        connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, NetworkStatusHandle::default(), coverage.clone())
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
    assert!(
        matches!(simulator.try_recv_from(&mut buffer), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
        "packet rejected by typed decode must not be acknowledged or answered"
    );
    assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
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
    assert_eq!(budget_view.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
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
    let connection =
        connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, NetworkStatusHandle::default(), coverage.clone())
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
    let duplicate_retention = timeout(WAIT, async {
        loop {
            let Some(SimulatorEvent::Simulator(envelope)) = received_events.recv().await else { continue };
            let (event, retention, _) = envelope.into_parts();
            if matches!(event, Event::SimulatorMessage { sequence, .. } if sequence == duplicate_sequence) {
                break retention.expect("received chat retains its packet memory");
            }
        }
    })
    .await
    .expect("receive the retained chat");

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
    assert!(
        matches!(simulator.try_recv_from(&mut buffer), Err(error) if error.kind() == io::ErrorKind::WouldBlock),
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
    assert!(matches!(receive_message_event(&mut received_events, rejected_sequence).await, Event::SimulatorMessage { .. }));

    shutdown.send_replace(true);
    assert!(timeout(WAIT, task).await.expect("join simulator task").expect("join task").is_ok());
}

#[tokio::test]
async fn ack_control_survives_memory_pressure() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let coverage = CoverageLossLedger::new();
    let status = NetworkStatusHandle::default();
    let connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, status, coverage.clone())
        .await
        .expect("connect neighbor simulator");

    let shared_budget = SimulatorMemoryBudget::new();
    let packet_memory = shared_budget.for_neighbor();
    let (events, mut received_events) = mpsc::channel(4);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (_commands, commands) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel::<oneshot::Sender<Result<(), Error>>>(1);
    let task: JoinHandle<Result<(), String>> =
        spawn(connection.run(events, shutdown_receiver, neighbor_requests, commands, logout, packet_memory.clone()));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    let _decode_slots = packet_memory
        .decode_slots
        .clone()
        .acquire_many_owned(MAX_CONCURRENT_PACKET_DECODES.try_into().expect("decode slot count fits u32"))
        .await
        .expect("occupy every decoder slot");
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
