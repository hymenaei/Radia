use super::*;

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
        NeighborConnection::connect(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000, NetworkStatusHandle::default())
            .await
            .expect("connect neighbor circuit");
    let task = spawn(connection.run(event_sender, shutdown, neighbor_requests, commands, mpsc::channel(1).1, SimulatorMemoryBudget::new()));

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
    let mut connection = SimulatorCoordinator::connect_with_status(
        login_session(primary.local_addr().expect("primary simulator address")),
        ResponseData::default(),
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect primary circuit");
    let _ = receive(&primary).await;

    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let mut run_state = CoordinatorRunState::new();
    let mut setup = PromotionSetup {
        state: &mut run_state,
        neighbor_requests,
        application_events: application_events.clone(),
        neighbor_events: neighbor_events.clone(),
        shutdown: shutdown.clone(),
        capability_events: mpsc::channel(1).0,
    };
    let target_region = 0x1000_0000_0000_1000;
    let mut sockets = Vec::with_capacity(MAX_NEIGHBOR_CONNECTION_TASKS + 1);

    for index in 0..MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let region_handle = if index == 0 { target_region } else { 0x2000_0000_0000_0000 + index as u64 };
        connection
            .start_neighbor(connection.simulator_id, region_handle, socket.local_addr().expect("neighbor address"), None, &mut setup)
            .await
            .expect("start a neighbor circuit");
        let (bytes, _) = receive(&socket).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_)));
        sockets.push(socket);
    }

    assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_SIMULATORS);
    let mut current_peer = None;
    for _ in 0..MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement simulator");
        let endpoint = socket.local_addr().expect("replacement address");
        connection
            .start_neighbor(connection.simulator_id, target_region, endpoint, None, &mut setup)
            .await
            .expect("replace target neighbor");
        let (bytes, peer) = receive(&socket).await;
        assert!(matches!(Packet::decode(&bytes).expect("decode replacement bootstrap").message(), Message::UseCircuitCode(_)));
        current_peer = Some(peer);
        sockets.push(socket);
    }

    assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_CONNECTION_TASKS);
    let current_id = setup.state.neighbor_connections.get(&target_region).expect("current target connection").simulator_id;
    let rejected = UdpSocket::bind("127.0.0.1:0").await.expect("bind rejected simulator");
    let rejected_endpoint = rejected.local_addr().expect("rejected endpoint");
    connection
        .start_neighbor(connection.simulator_id, target_region, rejected_endpoint, None, &mut setup)
        .await
        .expect("reject a replacement at the task cap");

    assert_eq!(setup.state.neighbor_connections.get(&target_region).expect("current target remains").simulator_id, current_id);
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_CONNECTION_TASKS);
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
        let Some(Err(error)) = setup.state.neighbors.join_next().await else {
            panic!("replaced neighbor task should be joined as canceled");
        };
        assert!(error.is_cancelled());
    }
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_SIMULATORS);

    let recovered = UdpSocket::bind("127.0.0.1:0").await.expect("bind recovered simulator");
    let recovered_endpoint = recovered.local_addr().expect("recovered endpoint");
    connection
        .start_neighbor(connection.simulator_id, target_region, recovered_endpoint, None, &mut setup)
        .await
        .expect("task capacity should recover after canceled workers are joined");
    let (bytes, _) = receive(&recovered).await;
    assert!(matches!(Packet::decode(&bytes).expect("decode recovered neighbor").message(), Message::UseCircuitCode(_)));
    assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_SIMULATORS + 1);
}

#[tokio::test]
async fn neighbor_simulator_limit_is_bounded() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let (application_events, mut received_events) = mpsc::channel(MAX_NEIGHBOR_SIMULATORS + 1);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<NeighborEvent>(1);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let mut connection = SimulatorCoordinator::connect_with_status(
        login_session(primary.local_addr().expect("primary simulator address")),
        ResponseData::default(),
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect primary circuit");
    let _ = receive(&primary).await;

    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let mut run_state = CoordinatorRunState::new();
    let mut setup = PromotionSetup {
        state: &mut run_state,
        neighbor_requests,
        application_events: application_events.clone(),
        neighbor_events: neighbor_events.clone(),
        shutdown: shutdown.clone(),
        capability_events: mpsc::channel(1).0,
    };
    let mut sockets = Vec::with_capacity(MAX_NEIGHBOR_SIMULATORS + 1);

    for index in 0..=MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let endpoint = socket.local_addr().expect("neighbor address");
        let region_handle = 0x1000_0000_0000_0000 + index as u64;
        connection
            .start_neighbor(connection.simulator_id, region_handle, endpoint, None, &mut setup)
            .await
            .expect("process neighbor announcement");

        if index < MAX_NEIGHBOR_SIMULATORS {
            let (bytes, _) = receive(&socket).await;
            assert!(matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_)));
            sockets.push(socket);
        } else {
            assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
            assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_SIMULATORS);
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
            assert!(!setup.state.neighbor_connections.contains_key(&region_handle));
            assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
            assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_SIMULATORS);
            sockets.push(socket);
        }
    }

    setup.state.neighbors.abort_all();
    while setup.state.neighbors.join_next().await.is_some() {}
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

#[test]
fn neighbor_pressure_keeps_primary_live() {
    run_large_stack_test(neighbor_pressure_keeps_primary_live_inner);
}

async fn neighbor_pressure_keeps_primary_live_inner() {
    use crate::event::Event::SimulatorMessage;

    let mut harness = Box::pin(connected_neighbor()).await;
    let second_neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind second neighbor simulator");
    let second_endpoint = second_neighbor.local_addr().expect("second neighbor simulator address");
    let second_region = 0x2000_0000_0000_2000;
    announce_neighbor(&harness.primary, harness.primary_peer, second_endpoint, second_region, 31).await;
    let (use_circuit, second_peer) = receive(&second_neighbor).await;
    let use_circuit = Packet::decode(&use_circuit).expect("decode second neighbor UseCircuitCode");
    assert_eq!(use_circuit.sequence(), 1);
    assert!(matches!(use_circuit.message(), Message::UseCircuitCode(_)));
    second_neighbor.send_to(&ack(40, 1), second_peer).await.expect("ack second neighbor circuit");
    let second_id = timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                Event::SimulatorConnected { simulator_id, region_handle: Some(region), endpoint }
                    if region == second_region && endpoint == second_endpoint =>
                {
                    break simulator_id;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("second neighbor should connect");

    let neighbor_budget = fill_neighbor_budget(&harness.packet_memory);
    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 41);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send neighbor payload under pressure");
    let mut second_chat = fixture("chat_from_simulator.hex");
    second_chat[0] |= 0x40;
    set_sequence(&mut second_chat, 51);
    second_neighbor.send_to(&second_chat, second_peer).await.expect("send second neighbor payload under pressure");
    harness.primary.send_to(&start_ping_packet(32, 7, 0), harness.primary_peer).await.expect("send primary ping under pressure");
    receive_message(&harness.primary, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 7)).await;
    assert!(
        timeout(Duration::from_millis(100), receive_ack(&harness.neighbor, 41)).await.is_err(),
        "neighbor payload rejected by memory admission must stay unacknowledged"
    );
    assert!(
        timeout(Duration::from_millis(100), receive_ack(&second_neighbor, 51)).await.is_err(),
        "second neighbor payload rejected by memory admission must stay unacknowledged"
    );
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES - MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(harness.status.snapshot().simulator_endpoint, Some(harness.primary.local_addr().expect("primary endpoint")));

    drop(neighbor_budget);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("retry neighbor payload after budget recovery");
    second_neighbor.send_to(&second_chat, second_peer).await.expect("retry second neighbor payload after budget recovery");
    receive_ack(&harness.neighbor, 41).await;
    receive_ack(&second_neighbor, 51).await;
    assert!(harness.packet_memory.neighbor_available_bytes() < MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    timeout(WAIT, async {
        let mut first_delivered = false;
        let mut second_delivered = false;
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, sequence: 41, .. } if simulator_id == harness.neighbor_id => first_delivered = true,
                SimulatorMessage { simulator_id, sequence: 51, .. } if simulator_id == second_id => second_delivered = true,
                _ => {}
            }
            if first_delivered && second_delivered {
                break;
            }
        }
    })
    .await
    .expect("both retries should publish their neighbor payloads");
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(harness.packet_memory.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);

    let packet_memory = harness.packet_memory.clone();
    harness.shutdown().await;
    assert_eq!(packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(packet_memory.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(packet_memory.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
}

#[tokio::test]
async fn neighbor_overload_keeps_control() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    event_sender
        .try_send(NeighborEvent::Simulator(Event::Failed("hold application events".into()).into()))
        .expect("fill neighbor event channel");
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let (_commands, command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
    let neighbor_connection =
        NeighborConnection::connect(simulator_id, login_session(neighbor_endpoint), 0x1000_0000_0000_1000, NetworkStatusHandle::default())
            .await
            .expect("connect neighbor circuit");
    let packet_memory = SimulatorMemoryBudget::new().for_neighbor();
    let budget = packet_memory.clone();
    let mut task =
        spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, command_receiver, mpsc::channel(1).1, packet_memory));

    let (use_circuit, neighbor_peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_)));
    neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");
    let first_sequence = 41;
    let burst_len = MAX_QUEUED_SIMULATOR_EVENTS * 4;
    let admitted_count = MAX_PENDING_SIMULATOR_EVENTS - 1;
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
        while !(acknowledged_sequences.len() == admitted_count && answered_ping) {
            let (bytes, _) = receive(&neighbor).await;
            let packet = Packet::decode(&bytes).expect("decode neighbor control reply");
            acknowledged_sequences.extend(
                packet.appended_acks().iter().copied().filter(|sequence| (*sequence >= first_sequence) && (*sequence <= last_sequence)),
            );
            if let Message::PacketAck(ack) = packet.message() {
                acknowledged_sequences.extend(
                    ack.packets.iter().map(|item| item.id).filter(|sequence| (*sequence >= first_sequence) && (*sequence <= last_sequence)),
                );
            }
            answered_ping |= matches!(packet.message(), Message::CompletePingCheck(message) if message.ping_id.ping_id == 7);
        }
    })
    .await
    .expect("neighbor ACK and ping handling should continue through a paused-consumer burst");
    assert_eq!(
        acknowledged_sequences,
        (first_sequence..first_sequence + admitted_count as u32).collect(),
        "only payloads retained by the event queue may be acknowledged"
    );
    let retained_bytes = MAX_QUEUED_SIMULATOR_EVENT_BYTES - budget.global_available_bytes();
    assert!(retained_bytes > 0, "accepted packets must remain charged while the consumer is paused");
    assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES - retained_bytes);
    assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES - retained_bytes);
    assert_eq!(budget.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);

    let mut duplicate_chat = fixture("chat_from_simulator.hex");
    duplicate_chat[0] |= 0x40;
    set_sequence(&mut duplicate_chat, first_sequence);
    neighbor.send_to(&duplicate_chat, neighbor_peer).await.expect("retransmit a reliable packet after overload begins");
    receive_ack(&neighbor, first_sequence).await;

    let rejected_sequence = first_sequence + admitted_count as u32;
    let mut rejected_chat = fixture("chat_from_simulator.hex");
    rejected_chat[0] |= 0x40;
    set_sequence(&mut rejected_chat, rejected_sequence);
    neighbor.send_to(&rejected_chat, neighbor_peer).await.expect("retransmit a payload rejected by event admission");
    let rejected_ack = timeout(Duration::from_millis(100), async {
        loop {
            let (bytes, _) = receive(&neighbor).await;
            let packet = Packet::decode(&bytes).expect("decode possible rejected-payload ACK");
            if packet.appended_acks().contains(&rejected_sequence)
                || matches!(packet.message(), Message::PacketAck(ack) if ack.packets.iter().any(|item| item.id == rejected_sequence))
            {
                break;
            }
        }
    })
    .await;
    assert!(rejected_ack.is_err(), "a rejected reliable payload must remain unacknowledged on retransmission");

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
    assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    assert_eq!(budget.decode_slots.available_permits(), MAX_CONCURRENT_PACKET_DECODES);
    let queued = event_receiver.try_recv().expect("the held event remains queued");
    let NeighborEvent::Simulator(queued) = queued else {
        panic!("expected queued simulator event");
    };
    assert!(matches!(queued.into_event(), Event::Failed(message) if message == "hold application events"));
    assert!(
        matches!(event_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)),
        "overflow must not deliver queued application updates after failure"
    );
}
