use super::*;
use crate::capabilities::EventQueueRoleHandle;
use crate::fixtures::{event_with_retained_bytes, test_simulator_id};

async fn run_queued_target_does_not_follow_neighbor_replacement() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let neighbor_id = harness.neighbor_id;
    let replacement = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement simulator");
    let endpoint = replacement.local_addr().expect("replacement endpoint");
    let region_handle = 0x1000_0000_0000_1000;

    announce_neighbor(&harness.primary, harness.primary_peer, endpoint, region_handle, 50).await;
    let (bytes, replacement_peer) = receive(&replacement).await;
    let circuit = Packet::decode(&bytes).expect("decode replacement circuit packet");
    std::assert_matches!(circuit.message(), Message::UseCircuitCode(_));
    replacement.send_to(&ack(60, circuit.sequence()), replacement_peer).await.expect("ack replacement circuit");

    let replacement_id = timeout(WAIT, async {
        loop {
            if let Event::SimulatorConnected { simulator_id, region_handle: Some(handle), endpoint: connected_endpoint } =
                next_event(&mut harness.events).await
                && handle == region_handle
                && connected_endpoint == endpoint
            {
                break simulator_id;
            }
        }
    })
    .await
    .expect("replacement connection should be announced");
    assert_ne!(replacement_id, neighbor_id);

    let (completed, result) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo { simulator_id: neighbor_id, message: object_request(), delivery: Delivery::Reliable, completed })
        .await
        .expect("queue an exact send while the replacement route is installed");
    std::assert_matches!(
        timeout(WAIT, result).await.expect("stale target send should complete").expect("send reply"),
        Err(Error::SimulatorUnavailable)
    );

    let mut buffer = [0; 65_535];
    let deadline = Instant::now() + Duration::from_millis(50);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match timeout(remaining, replacement.recv_from(&mut buffer)).await {
            Ok(Ok((size, _))) => {
                let packet = Packet::decode(&buffer[..size]).expect("decode replacement traffic");
                assert!(
                    !matches!(packet.message(), Message::RequestMultipleObjects(_)),
                    "a queued send for the retired simulator ID must not reach its replacement"
                );
            }
            Err(_) | Ok(Err(_)) => break,
        }
    }

    harness.shutdown().await;
}

async fn run_neighbor_targets_and_replacements() {
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

    let mut disable = test_fixture("disable_simulator.hex");
    set_sequence(&mut disable, 42);
    harness.neighbor.send_to(&disable, harness.neighbor_peer).await.expect("disable neighbor circuit");
    loop {
        if matches!(next_event(&mut harness.events).await, Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == harness.neighbor_id)
        {
            break;
        }
    }
    std::assert_matches!(send_targeted(&harness.commands, harness.neighbor_id, object_request()).await, Err(Error::SimulatorUnavailable));

    let neighbor_endpoint = harness.neighbor.local_addr().expect("neighbor address");
    announce_neighbor(&harness.primary, harness.primary_peer, neighbor_endpoint, 0x1000_0000_0000_1000, 50).await;
    let (bytes, replacement_peer) = receive(&harness.neighbor).await;
    std::assert_matches!(Packet::decode(&bytes).expect("decode replacement circuit packet").message(), Message::UseCircuitCode(_));
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
    std::assert_matches!(send_targeted(&harness.commands, harness.neighbor_id, object_request()).await, Err(Error::SimulatorUnavailable));
    send_targeted(&harness.commands, replacement_id, object_request()).await.expect("send to replacement");
    let (_, sent_peer) =
        receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert_eq!(sent_peer, replacement_peer);
    harness.shutdown().await;
}

#[test]
fn neighbor_disconnect_removes_preconnect_packet_memory() {
    let simulator_id = test_simulator_id(54);
    let source = "127.0.0.1:13004".parse().expect("parse neighbor endpoint");
    let budget = SimulatorMemoryBudget::new().for_neighbor();
    let wire = test_fixture("chat_from_simulator.hex");
    let mut preconnect_events = PreconnectEvents::default();

    assert!(preconnect_events.push(simulator_id, retained_packet_event(&budget, simulator_id, source, &wire)));
    assert!(budget.global_available_bytes() < MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert!(budget.neighbor_available_bytes() < MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert!(budget.connection_available_bytes() < MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

    drop(preconnect_events.remove(simulator_id));

    assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
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
    let mut chat = test_fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 19);
    simulator.send_to(&chat, peer).await.expect("send data before circuit acknowledgement");
    receive_message(&simulator, |message| matches!(message, Message::PacketAck(acks) if acks.packets.iter().any(|packet| packet.id == 19)))
        .await;
    assert!(events.try_recv().is_err(), "primary application data must wait for the simulator introduction");

    simulator.send_to(&ack(20, 1), peer).await.expect("ack primary circuit");
    receive_message(&simulator, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    let first = next_raw_event(&mut events).await;
    let simulator_id = match first {
        Event::SimulatorConnected { simulator_id, region_handle: None, .. } => simulator_id,
        event => panic!("expected primary introduction before other events, got {event:?}"),
    };
    std::assert_matches!(
        next_raw_event(&mut events).await,
        Event::SimulatorMessage { simulator_id: id, sequence: 19, message: Message::ChatFromSimulator(_), .. }
            if id == simulator_id
    );

    disconnect.send_replace(true);
    receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(Duration::from_millis(200), client).await.expect("disconnect should finish").expect("client task").is_ok());
    drop(commands);
}

#[test]
fn preconnect_buffer_is_bounded() {
    let simulator_id = test_simulator_id(51);
    let mut pending = PreconnectEvents::default();
    for _ in 0..MAX_PRECONNECT_EVENTS {
        assert!(matches!(
            queue_until_connected(&HashSet::new(), &mut pending, simulator_id, EventEnvelope::new(Event::Failed("pending".into()))),
            PreconnectOutcome::Buffered
        ));
    }
    let overflow = queue_until_connected(&HashSet::new(), &mut pending, simulator_id, EventEnvelope::new(Event::Failed("overflow".into())));
    assert!(matches!(overflow, PreconnectOutcome::Full));
    assert_eq!(pending.len(), MAX_PRECONNECT_EVENTS);
}

#[tokio::test]
async fn simulator_removal_releases_preconnect_bytes() {
    let simulator_id = test_simulator_id(52);
    let budget = Arc::new(Semaphore::new(10));
    let permit = budget.clone().acquire_many_owned(7).await.expect("reserve event bytes");
    let mut pending = PreconnectEvents::default();
    assert!(pending.push(
        simulator_id,
        event_with_retained_bytes(
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
    assert_eq!(pending.len(), 0);
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
    let mut connection = connect_runtime(login_session(primary.local_addr().expect("primary simulator address")), ResponseData::default())
        .await
        .expect("connect primary circuit");
    let _ = receive(&primary).await;
    let (application_events, _application_event_receiver) = mpsc::channel(8);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<SimulatorEvent>(8);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(8);
    let mut run_state = RunState::new();
    let (capability_events, _capability_event_receiver) = mpsc::channel(8);
    let mut setup =
        PromotionSetup { state: &mut run_state, neighbor_requests, application_events, neighbor_events, shutdown, capability_events };
    let root_simulator_id = connection.simulator_id;

    connection.start_neighbor(root_simulator_id, region_handle, old_endpoint, None, &mut setup).await.expect("start initial neighbor");
    let _ = receive(&old_neighbor).await;
    let old_id = setup.state.neighbor_connections.get(&region_handle).expect("old neighbor is tracked").simulator_id;

    let packet_memory = SimulatorMemoryBudget::new().for_neighbor();
    let packet = test_fixture("chat_from_simulator.hex");
    assert!(setup.state.preconnect_events.push(old_id, retained_packet_event(&packet_memory, old_id, old_endpoint, &packet),));
    let seed_capability = Url::parse("http://127.0.0.1:1/seed").expect("seed URL");
    setup.state.simulator_capabilities.insert(
        old_id,
        SimulatorCapabilitySession {
            endpoint: old_endpoint,
            seed_capability: seed_capability.clone(),
            client: Client::new_with_budget(seed_capability, CapabilityBudget::new()).expect("create old capability client"),
            event_queue_role: EventQueueRoleHandle::channel(false).0,
            event_queue_task: None,
            event_queue_running: false,
        },
    );
    assert!(packet_memory.global_available_bytes() < MAX_QUEUED_SIMULATOR_EVENT_BYTES);

    connection
        .start_neighbor(root_simulator_id, region_handle, new_endpoint, None, &mut setup)
        .await
        .expect("replace unannounced neighbor");
    let _ = receive(&new_neighbor).await;

    assert_ne!(setup.state.neighbor_connections.get(&region_handle).expect("replacement remains tracked").simulator_id, old_id);
    assert!(setup.state.retiring_simulators.contains(&old_id));
    assert_eq!(setup.state.preconnect_events.len(), 0, "replacement drops old pre-connect events immediately");
    assert_eq!(packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(packet_memory.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(packet_memory.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    assert!(!setup.state.simulator_capabilities.contains_key(&old_id), "replacement stops the old capability session immediately");

    setup.state.neighbors.abort_all();
    while setup.state.neighbors.join_next().await.is_some() {}
}

#[tokio::test]
async fn preconnect_overflow_is_neighbor_scoped() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let primary_endpoint = primary.local_addr().expect("read primary address");
    let mut connection = connect_runtime(login_session(primary_endpoint), ResponseData::default()).await.expect("connect primary circuit");
    let simulator_id = test_simulator_id(53);
    let neighbor_endpoint = "127.0.0.1:13003".parse().expect("parse neighbor endpoint");
    let region_handle = 0x1000_0000_0000_1003;
    let neighbor_task = spawn(pending::<()>());
    let (neighbor_commands, _neighbor_command_receiver) = mpsc::channel(1);
    let mut run_state = RunState::new();
    run_state.neighbor_connections.insert(
        region_handle,
        SimulatorConnectionState {
            simulator_id,
            endpoint: neighbor_endpoint,
            abort: neighbor_task.abort_handle(),
            commands: neighbor_commands,
            logout: mpsc::channel(1).0,
            pending_handoff_command: None,
            primary_state: None,
            packet_memory: SimulatorMemoryBudget::new().for_neighbor(),
        },
    );
    for _ in 0..MAX_PRECONNECT_EVENTS {
        assert!(run_state.preconnect_events.push(simulator_id, EventEnvelope::new(Event::Failed("buffered".into()))));
    }
    let (events, mut received) = mpsc::channel(1);

    connection
        .fail_preconnect_overflow(simulator_id, &events, &mut run_state)
        .await
        .expect("neighbor overflow should not log out the primary simulator");

    assert!(run_state.neighbor_connections.is_empty(), "overflow must remove the affected neighbor");
    assert_eq!(run_state.preconnect_events.len(), 0);
    assert!(!run_state.connected_simulators.contains(&simulator_id));
    assert!(neighbor_task.await.expect_err("overflow should cancel the neighbor worker").is_cancelled());
    std::assert_matches!(
        received.recv().await.map(EventEnvelope::into_event),
        Some(Event::SimulatorFailed { simulator_id: failed_id, region_handle: Some(handle), endpoint, .. })
            if failed_id == simulator_id && handle == region_handle && endpoint == neighbor_endpoint
    );
}

#[tokio::test]
async fn neighbor_circuits_are_independent() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let neighbor_peer = harness.neighbor_peer;
    let neighbor_id = harness.neighbor_id;
    assert_ne!(harness.primary_id, neighbor_id);

    let mut neighbor_handshake = test_fixture("region_handshake.hex");
    neighbor_handshake[0] |= 0x40;
    set_sequence(&mut neighbor_handshake, 41);
    harness.neighbor.send_to(&neighbor_handshake, neighbor_peer).await.expect("send neighbor handshake");
    let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert!(reply.appended_acks().contains(&41));
    std::assert_matches!(
        next_event(&mut harness.events).await,
        Event::SimulatorMessage { simulator_id, source, sequence: 41, message: Message::RegionHandshake(_), .. }
            if simulator_id == neighbor_id && source == harness.neighbor.local_addr().expect("neighbor address")
    );

    let mut disable = test_fixture("disable_simulator.hex");
    set_sequence(&mut disable, 42);
    harness.neighbor.send_to(&disable, neighbor_peer).await.expect("disable neighbor circuit");
    loop {
        if matches!(next_event(&mut harness.events).await, Event::SimulatorDisconnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), .. } if simulator_id == neighbor_id)
        {
            break;
        }
    }

    let mut chat = test_fixture("chat_from_simulator.hex");
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

    let mut chat = test_fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 41);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send final neighbor message");

    let mut disable = test_fixture("disable_simulator.hex");
    set_sequence(&mut disable, 42);
    harness.neighbor.send_to(&disable, harness.neighbor_peer).await.expect("close neighbor circuit");

    std::assert_matches!(
        next_event(&mut harness.events).await,
        Event::SimulatorMessage { simulator_id, sequence: 41, message: Message::ChatFromSimulator(_), .. }
            if simulator_id == harness.neighbor_id
    );
    std::assert_matches!(
        next_event(&mut harness.events).await,
        Event::SimulatorDisconnected { simulator_id, region_handle: Some(0x1000_0000_0000_1000), .. }
            if simulator_id == harness.neighbor_id
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn object_local_ids_are_scoped_to_simulators() {
    let mut harness = Box::pin(connected_neighbor()).await;

    let mut primary_update = test_fixture("object_update.hex");
    set_sequence(&mut primary_update, 44);
    harness.primary.send_to(&primary_update, harness.primary_peer).await.expect("send primary object update");

    let mut neighbor_update = test_fixture("object_update.hex");
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

#[test]
fn neighbor_targets_and_replacements() {
    run_large_stack_test(run_neighbor_targets_and_replacements);
}

#[test]
fn queued_target_does_not_follow_neighbor_replacement() {
    run_large_stack_test(run_queued_target_does_not_follow_neighbor_replacement);
}

#[tokio::test]
async fn old_neighbor_completion_keeps_replacement_route() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let replacement = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement neighbor");
    let replacement_endpoint = replacement.local_addr().expect("replacement address");
    let region_handle = 0x1000_0000_0000_1000;

    announce_neighbor(&harness.primary, harness.primary_peer, replacement_endpoint, region_handle, 50).await;
    let (bytes, replacement_peer) = receive(&replacement).await;
    std::assert_matches!(Packet::decode(&bytes).expect("decode replacement circuit packet").message(), Message::UseCircuitCode(_));
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
    send_targeted(&harness.commands, replacement_id, object_request()).await.expect("old completion must preserve the replacement route");
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
                if let Event::SimulatorConnectFailed { region_handle: failed_region, endpoint, .. } = next_event(&mut harness.events).await
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
    std::assert_matches!(Packet::decode(&bytes).expect("decode other neighbor bootstrap").message(), Message::UseCircuitCode(_));
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
