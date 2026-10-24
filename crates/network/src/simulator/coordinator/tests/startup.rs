use super::*;

#[test]
fn startup_accepts_movement_before_handshake() {
    let agent_id = Uuid::from_u128(1);
    let session_id = Uuid::from_u128(2);
    let movement = Packet::decode(&movement_complete_packet(1, agent_id, session_id, 0x1000))
        .expect("decode matching movement completion")
        .into_message();
    let handshake = Packet::decode(&fixture("region_handshake.hex")).expect("decode region handshake").into_message();
    let mut startup = SessionStartup::default();

    assert!(!startup.observe_primary_message(&movement, agent_id, session_id));
    assert!(!startup.is_ready());
    assert!(startup.observe_primary_message(&handshake, agent_id, session_id));
    assert!(startup.is_ready());
}

#[test]
fn startup_ignores_movement_for_another_session() {
    let agent_id = Uuid::from_u128(1);
    let session_id = Uuid::from_u128(2);
    let wrong_movement = Packet::decode(&movement_complete_packet(1, agent_id, Uuid::from_u128(3), 0x1000))
        .expect("decode mismatched movement completion")
        .into_message();
    let handshake = Packet::decode(&fixture("region_handshake.hex")).expect("decode region handshake").into_message();
    let mut startup = SessionStartup::default();

    assert!(!startup.observe_primary_message(&wrong_movement, agent_id, session_id));
    assert!(!startup.observe_primary_message(&handshake, agent_id, session_id));
    assert!(!startup.is_ready());
}

#[tokio::test]
async fn cancelled_queues_release_packet_memory() {
    let budget = SimulatorMemoryBudget::new();
    let neighbors = [budget.for_neighbor(), budget.for_neighbor()];
    let wire = fixture("chat_from_simulator.hex");
    let endpoint = "127.0.0.1:13000".parse().expect("simulator endpoint");
    let primary_id = SimulatorId::allocate().expect("primary ID");
    let (events, _receiver) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupy intake".into()))).expect("fill intake");
    let mut primary_queue = VecDeque::new();
    assert!(matches!(
        crate::simulator::coordinator::queue_simulator_event(
            &events,
            &mut primary_queue,
            retained_packet_event(&budget, primary_id, endpoint, &wire)
        ),
        crate::simulator::coordinator::EventQueueOutcome::Queued
    ));
    let mut neighbor_queues = Vec::new();
    let mut receivers = Vec::new();
    for neighbor in &neighbors {
        let id = SimulatorId::allocate().expect("neighbor ID");
        let (events, receiver) = mpsc::channel(1);
        events
            .try_send(SimulatorEvent::Simulator(EventEnvelope::new(Event::Failed("occupy intake".into()))))
            .expect("fill neighbor intake");
        let mut queue = VecDeque::new();
        assert_eq!(
            queue_neighbor_event(&events, &mut queue, retained_packet_event(neighbor, id, endpoint, &wire)),
            NeighborEventQueueOutcome::Queued
        );
        neighbor_queues.push(queue);
        receivers.push(receiver);
    }
    assert_eq!(primary_queue.len(), 1);
    assert!(neighbor_queues.iter().all(|queue| queue.len() == 1));
    assert!(budget.global_available_bytes() < MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert!(budget.neighbor_available_bytes() < MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    let (entered, ready) = oneshot::channel();
    let task = spawn(async move {
        let queues = (primary_queue, neighbor_queues);
        entered.send(()).expect("observe suspended queue owner");
        pending::<()>().await;
        drop(queues);
    });
    ready.await.expect("queues moved to suspended owner");
    task.abort();
    assert!(task.await.expect_err("owner cancelled").is_cancelled());
    assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget.connection_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    let bound = Packet::preflight(&wire).expect("valid packet bound");
    for neighbor in &neighbors {
        assert_eq!(neighbor.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
        drop(neighbor.reserve_packet(bound).expect("admission recovers after cancellation"));
    }
    drop(budget.reserve_packet(bound).expect("primary admission recovers"));
}

#[tokio::test]
async fn retired_capability_requests_are_waited_for() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let endpoint = simulator.local_addr().expect("simulator endpoint");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind held capability");
    let seed = Url::parse(&format!("http://{}/held", listener.local_addr().expect("HTTP endpoint"))).expect("held URL");
    let mut connection = connect_coordinator(login_session(endpoint), ResponseData::default()).await.expect("connect coordinator");
    let client = connection.new_capability_client(seed.clone()).expect("track capability requests");
    let id = SimulatorId::allocate().expect("neighbor ID");
    let mut capabilities = HashMap::from([(
        id,
        SimulatorCapabilitySession {
            endpoint,
            seed_capability: seed,
            client: client.clone(),
            event_queue_role: EventQueueRoleHandle::channel(false).0,
            event_queue_task: None,
            event_queue_running: false,
        },
    )]);
    let request = spawn({
        let client = client.clone();
        async move { client.request_capabilities(&["TestCapability"]).await }
    });
    let (mut socket, _) = timeout(WAIT, listener.accept()).await.expect("request reaches server").expect("accept request");
    let mut bytes = [0; 1024];
    assert!(timeout(WAIT, socket.read(&mut bytes)).await.expect("read held request").expect("HTTP request bytes") > 0);
    assert_eq!(connection.capability_request_waiters.len(), 2, "root and neighbor capability requests stay tracked");
    crate::simulator::coordinator::remove_simulator_capability(&mut capabilities, id);
    assert!(capabilities.is_empty());
    timeout(WAIT, connection.finish_capability_request_waiters()).await.expect("wait for retired and root capability requests");
    assert!(connection.capability_request_waiters.is_empty());
    let result = timeout(WAIT, request).await.expect("closed request resolves").expect("request task");
    assert!(matches!(result, Err(Error::CapabilitySessionClosed)));
    assert!(matches!(client.request_capabilities(&["TestCapability"]).await, Err(Error::CapabilitySessionClosed)));
}

#[tokio::test]
async fn capability_client_count_is_bounded() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let mut connection = connect_coordinator(login_session(simulator.local_addr().expect("simulator endpoint")), ResponseData::default())
        .await
        .expect("connect coordinator");
    let seed = connection.session.seed_capability.clone();
    let mut clients = Vec::new();
    for _ in 1..crate::simulator::coordinator::MAX_TRACKED_CAPABILITY_CLIENTS {
        clients.push(connection.new_capability_client(seed.clone()).expect("admit client within limit"));
    }
    assert!(connection.new_capability_client(seed).is_err());
    assert_eq!(connection.capability_request_waiters.len(), crate::simulator::coordinator::MAX_TRACKED_CAPABILITY_CLIENTS);
    for client in &clients {
        client.close_session();
    }
    timeout(WAIT, connection.finish_capability_request_waiters()).await.expect("wait for bounded capability requests");
    assert!(connection.capability_request_waiters.is_empty());
}

#[tokio::test(start_paused = true)]
async fn full_decode_slots_do_not_block_primary_retry_timers() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let coverage_losses = CoverageLossLedger::new();
    let mut coverage_changes = coverage_losses.subscribe();
    let connection = connect_with_coverage(
        login_session(simulator.local_addr().expect("read simulator address")),
        ResponseData::default(),
        coverage_losses.clone(),
    )
    .await
    .expect("connect simulator circuit");
    let simulator_id = connection.simulator_id;
    let held_slots = connection
        .packet_memory
        .decode_slots
        .clone()
        .acquire_many_owned(MAX_CONCURRENT_PACKET_DECODES.try_into().expect("decode slot count fits u32"))
        .await
        .expect("occupy every decode slot");
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, _received) = mpsc::channel(TEST_EVENT_CAPACITY);
    let terminal_event_permit = events.clone().reserve_owned().await.expect("reserve terminal event slot");
    let (_disconnect, disconnect_requested) = watch::channel(false);
    let client = spawn(connection.run(command_receiver, events, disconnect_requested, terminal_event_permit));

    let (initial, peer) = receive(&simulator).await;
    assert!(matches!(Packet::decode(&initial).expect("decode initial circuit packet").message(), Message::UseCircuitCode(_)));
    let dropped_packet = Packet::new(40, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }))
        .encode()
        .expect("encode packet rejected by unavailable decoder");
    simulator.send_to(&dropped_packet, peer).await.expect("send packet while decode slots are full");

    let started = tokio::time::Instant::now();
    loop {
        if coverage_losses.snapshot().losses.iter().any(|loss| {
            matches!(
                loss,
                CoverageLoss::Source(record)
                    if record.watermark.simulator_id == simulator_id
                        && record.watermark.source == CoverageSource::Lludp
                        && record.reason == CoverageLossReason::DecodeAdmission
            )
        }) {
            break;
        }
        coverage_changes.changed().await.expect("coverage ledger remains open");
    }
    assert!(started.elapsed() < WAIT, "inbound packet must be rejected by decode-slot admission before advancing retry time");

    advance(Duration::from_secs(6)).await;
    let retry = receive(&simulator).await.0;
    let retry = Packet::decode(&retry).expect("decode circuit retry");
    assert_eq!(retry.sequence(), 1);
    assert!(retry.is_resent(), "retry timer continues while packet decode is denied");

    drop(held_slots);
    client.abort();
    let _ = client.await;
    drop(commands);
}

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
    std::assert_matches!(
        result,
        Err(crate::simulator::coordinator::CoordinatorError::Connection(
            crate::simulator::coordinator::connection::ConnectionError::KeepaliveTimeout
        ))
    );
    drop(commands);
}

#[tokio::test(start_paused = true)]
async fn neighbor_circuit_fails_after_udp_inactivity() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("read neighbor address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let connection =
        connect_test_neighbor(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000).await.expect("connect neighbor circuit");
    let (events, mut event_receiver) = mpsc::channel(4);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(4);
    let (_command_sender, commands) = mpsc::channel(4);
    let worker = spawn(connection.run(events, shutdown, neighbor_requests, commands, mpsc::channel(1).1, SimulatorMemoryBudget::new()));

    let (use_circuit, peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode UseCircuitCode").message(), Message::UseCircuitCode(_)));
    let acknowledgement = Packet::new(40, Message::PacketAck(PacketAck { packets: vec![Packets { id: 1 }] }))
        .encode()
        .expect("encode circuit acknowledgement");
    neighbor.send_to(&acknowledgement, peer).await.expect("ack neighbor circuit");
    assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { region_handle: Some(_), .. }));

    advance(KEEPALIVE_TIMEOUT).await;
    let result = worker.await.expect("neighbor task");
    std::assert_matches!(result, Err(crate::simulator::coordinator::connection::ConnectionError::KeepaliveTimeout));
}

async fn terminal_after_full_queue(
    result: Result<(), crate::simulator::coordinator::CoordinatorError>,
) -> (Event, SimulatorId, SocketAddr) {
    let (events, mut received) = mpsc::channel(2);
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let endpoint = "127.0.0.1:9000".parse().expect("parse simulator endpoint");
    let terminal_slot = events.clone().try_reserve_owned().expect("reserve terminal event slot");
    events.try_send(EventEnvelope::new(Event::Failed("queued before disconnect".into()))).expect("fill the nonterminal slot");
    assert_eq!(events.capacity(), 0, "the ordinary event capacity is full");

    SimulatorCoordinator::publish_primary_terminal_event(terminal_slot, simulator_id, None, endpoint, &result);

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
    let (event, simulator_id, endpoint) =
        terminal_after_full_queue(Err(crate::simulator::coordinator::CoordinatorError::ShutdownReason("simulator failed"))).await;
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
    let connection = connect_coordinator(login_session(endpoint), ResponseData::default()).await.expect("connect simulator circuit");
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
    assert!(matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::Failed(message)) if message == "fill event queue"));
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
    let mut connection = connect_coordinator(login_session(endpoint), ResponseData::default()).await.expect("connect simulator circuit");
    let simulator_id = connection.simulator_id;
    let (events, mut received) = mpsc::channel(4);
    let mut run_state = CoordinatorRunState::new();
    let queued_bytes = Arc::new(Semaphore::new(10));
    let mut first_event_permit = Some(queued_bytes.clone().acquire_many_owned(7).await.expect("reserve capability event bytes"));

    for (index, message) in ["First", "Second"].into_iter().enumerate() {
        let event = Event::CapabilityEvent { simulator_id, source: endpoint, message: message.to_owned(), body: Value::Undefined };
        let envelope = if index == 0 {
            EventEnvelope::with_retained_bytes(event, first_event_permit.take().expect("first event permit"))
        } else {
            EventEnvelope::new(event)
        };
        connection
            .publish_simulator_event(simulator_id, envelope, &events, &mut run_state)
            .await
            .expect("buffer pre-connect capability event");
    }
    assert_eq!(run_state.preconnect_events.len(), 2);
    assert_eq!(queued_bytes.available_permits(), 3, "the preconnect buffer must retain capability bytes");
    assert!(received.try_recv().is_err(), "capability events must wait for SimulatorConnected");

    connection
        .publish_simulator_connected(
            simulator_id,
            Event::SimulatorConnected { simulator_id, region_handle: None, endpoint },
            &events,
            &mut run_state.pending_events,
            &mut run_state.preconnect_events,
            &mut run_state.connected_simulators,
            &mut run_state.logout_deadline,
            &mut run_state.disconnect_reason,
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
    assert_eq!(run_state.preconnect_events.len(), 0);
}

#[tokio::test]
async fn disconnect_cancels_primary_startup() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("primary simulator address");
    let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
    pause_sends.send_replace(true);
    let (disconnect, disconnect_requested) = watch::channel(false);
    let startup =
        spawn(connect_with_gate_and_disconnect(login_session(endpoint), ResponseData::default(), send_gate, disconnect_requested));

    timeout(WAIT, send_entered.notified()).await.expect("initial UseCircuitCode reaches the gated local send");
    let mut packet = [0; 1024];
    assert_eq!(simulator.try_recv_from(&mut packet).expect_err("gated send must not reach the simulator").kind(), ErrorKind::WouldBlock);

    disconnect.send_replace(true);
    assert!(
        timeout(WAIT, startup)
            .await
            .expect("startup cancellation is bounded")
            .expect("startup task")
            .expect("clean cancellation")
            .is_none()
    );
}
