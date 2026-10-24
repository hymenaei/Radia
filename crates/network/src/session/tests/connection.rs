use std::net::SocketAddrV4;

use llsd::{Map, Value, xml};
use reqwest::Url;
use uuid::Uuid;

use super::*;
use crate::fixtures::{
    event_queue::EventReceiver,
    http::{EventQueueTestServer, EventServerResponse, hold_response, request_line, response, seed_response},
    simulator::{acknowledgement, receive_message, receive_udp},
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

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
    std::assert_matches!(use_circuit.message(), Message::UseCircuitCode(_));
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
    let mut chat = test_fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 41);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send neighbor payload under pressure");
    let mut second_chat = test_fixture("chat_from_simulator.hex");
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
    assert_eq!(packet_memory.decode_slots_available(), 8);
}

async fn prepare_crossed_region_destination(
    neighbor: &UdpSocket,
    neighbor_endpoint: SocketAddr,
    region_handle: u64,
    session: &AgentSession,
    received_events: &mut EventReceiver,
) -> (SimulatorId, SocketAddr) {
    let (neighbor_packet, neighbor_peer) = receive_udp(neighbor).await;
    let neighbor_packet = Packet::decode(&neighbor_packet).expect("decode destination circuit");
    std::assert_matches!(neighbor_packet.message(), Message::UseCircuitCode(_));
    neighbor.send_to(&acknowledgement(40, neighbor_packet.sequence()), neighbor_peer).await.expect("ack destination circuit");
    let neighbor_id = loop {
        match timeout(TEST_TIMEOUT, received_events.recv()).await.expect("receive destination events").expect("event channel remains open")
        {
            Event::SimulatorConnected { simulator_id, endpoint, region_handle: Some(handle) }
                if endpoint == neighbor_endpoint && handle == region_handle =>
            {
                break simulator_id;
            }
            _ => {}
        }
    };
    let mut neighbor_handshake = test_fixture("region_handshake.hex");
    neighbor_handshake[0] |= 0x40;
    set_sequence(&mut neighbor_handshake, 41);
    neighbor.send_to(&neighbor_handshake, neighbor_peer).await.expect("send destination region handshake");
    let reply = receive_message(neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let movement = receive_message(neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    neighbor.send_to(&acknowledgement(42, reply.sequence()), neighbor_peer).await.expect("ack destination handshake reply");
    neighbor.send_to(&acknowledgement(43, movement.sequence()), neighbor_peer).await.expect("ack destination movement request");
    let wrong_movement = Packet::new(
        44,
        Message::AgentMovementComplete(AgentMovementComplete {
            agent_data: AgentData { agent_id: session.agent_id, session_id: session.session_id },
            data: Data { region_handle: region_handle + 1, ..Default::default() },
            sim_data: SimData::default(),
        }),
    )
    .with_reliable()
    .encode()
    .expect("encode mismatched movement completion");
    neighbor.send_to(&wrong_movement, neighbor_peer).await.expect("send mismatched movement completion");
    let mut barrier = test_fixture("chat_from_simulator.hex");
    barrier[0] |= 0x40;
    set_sequence(&mut barrier, 45);
    neighbor.send_to(&barrier, neighbor_peer).await.expect("send post-movement barrier");
    loop {
        match timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("receive before destination promotion")
            .expect("event channel remains open")
        {
            Event::SimulatorMessage { simulator_id, message: Message::ChatFromSimulator(_), .. } if simulator_id == neighbor_id => break,
            Event::SimulatorPromoted { .. } => panic!("mismatched movement completion must not promote the destination"),
            _ => {}
        }
    }
    (neighbor_id, neighbor_peer)
}

async fn promote_crossed_region(
    neighbor: &UdpSocket,
    neighbor_peer: SocketAddr,
    region_handle: u64,
    session: &AgentSession,
    primary_id: SimulatorId,
    neighbor_id: SimulatorId,
    received_events: &mut EventReceiver,
) {
    let matching_movement = Packet::new(
        46,
        Message::AgentMovementComplete(AgentMovementComplete {
            agent_data: AgentData { agent_id: session.agent_id, session_id: session.session_id },
            data: Data { region_handle, ..Default::default() },
            sim_data: SimData::default(),
        }),
    )
    .with_reliable()
    .encode()
    .expect("encode matching movement completion");
    neighbor.send_to(&matching_movement, neighbor_peer).await.expect("send matching movement completion");
    let (previous, promoted, promoted_endpoint) = loop {
        if let Event::SimulatorPromoted { previous_simulator_id, simulator_id, session, .. } =
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("receive destination promotion").expect("event channel remains open")
        {
            break (previous_simulator_id, simulator_id, session.simulator_address);
        }
    };
    assert_eq!(previous, primary_id);
    assert_eq!(promoted, neighbor_id);
    assert_eq!(promoted_endpoint, neighbor.local_addr().expect("destination simulator address"));
}

fn simulator_events(ip: [u8; 4], port: u16, agent_id: Uuid, seed: &Url) -> String {
    let simulator = Value::Map(Map::from([
        ("Handle".to_owned(), Value::Binary(0x1000_0000_0000_1000_u64.to_be_bytes().to_vec().into())),
        ("IP".to_owned(), Value::Binary(ip.to_vec().into())),
        ("Port".to_owned(), Value::Integer(i32::from(port))),
    ]));
    let enable = Value::Map(Map::from([
        ("message".to_owned(), Value::String("EnableSimulator".to_owned())),
        ("body".to_owned(), Value::Map(Map::from([("SimulatorInfo".to_owned(), Value::Array(vec![simulator]))]))),
    ]));
    let establish = Value::Map(Map::from([
        ("message".to_owned(), Value::String("EstablishAgentCommunication".to_owned())),
        (
            "body".to_owned(),
            Value::Map(Map::from([
                ("agent-id".to_owned(), Value::Uuid(agent_id)),
                (
                    "sim-ip-and-port".to_owned(),
                    Value::String(format!("{}:{}", ip.iter().map(u8::to_string).collect::<Vec<_>>().join("."), port)),
                ),
                ("seed-capability".to_owned(), Value::String(seed.to_string())),
            ])),
        ),
    ]));
    let batch = Value::Map(Map::from([("id".to_owned(), Value::Integer(1)), ("events".to_owned(), Value::Array(vec![enable, establish]))]));
    xml::to_string(&batch).expect("encode local LLSD event queue response")
}

fn crossed_region_events(agent_id: Uuid, session_id: Uuid, region_handle: u64, endpoint: SocketAddrV4, seed: &Url) -> String {
    let crossed_region = Value::Map(Map::from([
        ("message".to_owned(), Value::String("CrossedRegion".to_owned())),
        (
            "body".to_owned(),
            Value::Map(Map::from([
                (
                    "AgentData".to_owned(),
                    Value::Array(vec![Value::Map(Map::from([
                        ("AgentID".to_owned(), Value::Uuid(agent_id)),
                        ("SessionID".to_owned(), Value::Uuid(session_id)),
                    ]))]),
                ),
                (
                    "RegionData".to_owned(),
                    Value::Array(vec![Value::Map(Map::from([
                        ("RegionHandle".to_owned(), Value::Binary(region_handle.to_be_bytes().to_vec().into())),
                        ("SimIP".to_owned(), Value::Binary(endpoint.ip().octets().to_vec().into())),
                        ("SimPort".to_owned(), Value::Integer(i32::from(endpoint.port()))),
                        ("SeedCapability".to_owned(), Value::String(seed.to_string())),
                    ]))]),
                ),
            ])),
        ),
    ]));
    let batch = Value::Map(Map::from([("id".to_owned(), Value::Integer(1)), ("events".to_owned(), Value::Array(vec![crossed_region]))]));
    xml::to_string(&batch).expect("encode local CrossedRegion event")
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
        connect_test_neighbor(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000).await.expect("connect neighbor circuit");
    let task = spawn(connection.run(event_sender, shutdown, neighbor_requests, commands, mpsc::channel(1).1, SimulatorMemoryBudget::new()));

    let (use_circuit, peer) = receive(&neighbor).await;
    std::assert_matches!(Packet::decode(&use_circuit).expect("decode circuit packet").message(), Message::UseCircuitCode(_));

    let mut chat = test_fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 41);
    neighbor.send_to(&chat, peer).await.expect("send data before circuit acknowledgement");
    assert!(
        timeout(Duration::from_millis(50), event_receiver.recv()).await.is_err(),
        "application data must wait for the simulator introduction"
    );

    neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
    std::assert_matches!(
        next_neighbor_event(&mut event_receiver).await,
        Event::SimulatorConnected { simulator_id: id, .. } if id == simulator_id
    );
    std::assert_matches!(
        next_neighbor_event(&mut event_receiver).await,
        Event::SimulatorMessage { simulator_id: id, sequence: 41, message: Message::ChatFromSimulator(_), .. } if id == simulator_id
    );

    shutdown_signal.send_replace(true);
    assert!(timeout(Duration::from_millis(200), task).await.expect("neighbor stops").expect("neighbor task").is_ok());
}

#[tokio::test]
async fn neighbor_task_limit_preserves_current() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let (application_events, mut received_events) = mpsc::channel(MAX_NEIGHBOR_CONNECTION_TASKS + 1);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<SimulatorEvent>(MAX_NEIGHBOR_CONNECTION_TASKS + 1);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let mut connection = connect_runtime(login_session(primary.local_addr().expect("primary simulator address")), ResponseData::default())
        .await
        .expect("connect primary circuit");
    let _ = receive(&primary).await;

    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let mut run_state = RunState::new();
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
    let root_simulator_id = connection.simulator_id;

    for index in 0..MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
        let region_handle = if index == 0 { target_region } else { 0x2000_0000_0000_0000 + index as u64 };
        connection
            .start_neighbor(root_simulator_id, region_handle, socket.local_addr().expect("neighbor address"), None, &mut setup)
            .await
            .expect("start a neighbor circuit");
        let (bytes, _) = receive(&socket).await;
        std::assert_matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_));
        sockets.push(socket);
    }

    assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_SIMULATORS);
    let mut current_peer = None;
    for _ in 0..MAX_NEIGHBOR_SIMULATORS {
        let socket = UdpSocket::bind("127.0.0.1:0").await.expect("bind replacement simulator");
        let endpoint = socket.local_addr().expect("replacement address");
        connection.start_neighbor(root_simulator_id, target_region, endpoint, None, &mut setup).await.expect("replace target neighbor");
        let (bytes, peer) = receive(&socket).await;
        std::assert_matches!(Packet::decode(&bytes).expect("decode replacement bootstrap").message(), Message::UseCircuitCode(_));
        current_peer = Some(peer);
        sockets.push(socket);
    }

    assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_CONNECTION_TASKS);
    let current_id = setup.state.neighbor_connections.get(&target_region).expect("current target connection").simulator_id;
    let rejected = UdpSocket::bind("127.0.0.1:0").await.expect("bind rejected simulator");
    let rejected_endpoint = rejected.local_addr().expect("rejected endpoint");
    connection
        .start_neighbor(root_simulator_id, target_region, rejected_endpoint, None, &mut setup)
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
    std::assert_matches!(
        receive_message(current_socket, |message| matches!(message, Message::CompletePingCheck(_))).await.message(),
        Message::CompletePingCheck(message) if message.ping_id.ping_id == 7
    );
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
        .start_neighbor(root_simulator_id, target_region, recovered_endpoint, None, &mut setup)
        .await
        .expect("task capacity should recover after canceled workers are joined");
    let (bytes, _) = receive(&recovered).await;
    std::assert_matches!(Packet::decode(&bytes).expect("decode recovered neighbor").message(), Message::UseCircuitCode(_));
    assert_eq!(setup.state.neighbor_connections.len(), MAX_NEIGHBOR_SIMULATORS);
    assert_eq!(setup.state.neighbors.len(), MAX_NEIGHBOR_SIMULATORS + 1);
}

#[tokio::test]
async fn neighbor_simulator_limit_is_bounded() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let (application_events, mut received_events) = mpsc::channel(MAX_NEIGHBOR_SIMULATORS + 1);
    let (neighbor_events, _neighbor_event_receiver) = mpsc::channel::<SimulatorEvent>(1);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let mut connection = connect_runtime(login_session(primary.local_addr().expect("primary simulator address")), ResponseData::default())
        .await
        .expect("connect primary circuit");
    let _ = receive(&primary).await;
    let root_simulator_id = connection.simulator_id;

    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let mut run_state = RunState::new();
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
            .start_neighbor(root_simulator_id, region_handle, endpoint, None, &mut setup)
            .await
            .expect("process neighbor announcement");

        if index < MAX_NEIGHBOR_SIMULATORS {
            let (bytes, _) = receive(&socket).await;
            std::assert_matches!(Packet::decode(&bytes).expect("decode neighbor bootstrap").message(), Message::UseCircuitCode(_));
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
    std::assert_matches!(send_targeted(&first.commands, second.primary_id, object_request()).await, Err(Error::SimulatorUnavailable));

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

    let mut handshake = test_fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    primary.send_to(&handshake, primary_peer).await.expect("send primary handshake");
    receive_message(&primary, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let Event::SimulatorMessage { sequence, packet_order, .. } = next_event(&mut events).await else {
        panic!("expected region handshake event");
    };
    assert_eq!(sequence, 22);
    assert_eq!(packet_order, Ordered(22));

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    primary.send_to(&movement_complete, primary_peer).await.expect("send primary movement completion");
    std::assert_matches!(next_event(&mut events).await, Event::SessionReady { .. });
    std::assert_matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. });

    event_sender
        .try_send(EventEnvelope::new(Event::Failed("fill application event channel".into())))
        .expect("hold the only ordinary event slot");
    let (neighbor_ip, neighbor_port) = match neighbor_endpoint {
        SocketAddr::V4(address) => (address.ip().octets(), address.port().to_be_bytes()),
        SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
    };
    let mut enable = test_fixture("enable_simulator.hex");
    set_sequence(&mut enable, 30);
    enable[10..18].copy_from_slice(&0x1000_0000_0000_1000u64.to_le_bytes());
    enable[18..22].copy_from_slice(&neighbor_ip);
    enable[22..24].copy_from_slice(&neighbor_port);
    primary.send_to(&enable, primary_peer).await.expect("announce neighbor simulator");

    let (use_circuit, neighbor_peer) = receive(&neighbor).await;
    let use_circuit = Packet::decode(&use_circuit).expect("decode neighbor UseCircuitCode");
    assert_eq!(use_circuit.sequence(), 1);
    std::assert_matches!(use_circuit.message(), Message::UseCircuitCode(_));
    neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");

    let mut reliable_chat = test_fixture("chat_from_simulator.hex");
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

#[tokio::test]
async fn neighbor_overload_keeps_control() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    event_sender
        .try_send(SimulatorEvent::Simulator(Event::Failed("hold application events".into()).into()))
        .expect("fill neighbor event channel");
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(32);
    let (_commands, command_receiver) = mpsc::channel(NEIGHBOR_COMMAND_CAPACITY);
    let neighbor_connection = connect_test_neighbor(simulator_id, login_session(neighbor_endpoint), 0x1000_0000_0000_1000)
        .await
        .expect("connect neighbor circuit");
    let packet_memory = SimulatorMemoryBudget::new().for_neighbor();
    let budget = packet_memory.clone();
    let mut task =
        spawn(neighbor_connection.run(event_sender, shutdown, neighbor_requests, command_receiver, mpsc::channel(1).1, packet_memory));

    let (use_circuit, neighbor_peer) = receive(&neighbor).await;
    std::assert_matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_));
    neighbor.send_to(&ack(40, 1), neighbor_peer).await.expect("ack neighbor circuit");
    let first_sequence = 41;
    let burst_len = MAX_QUEUED_SIMULATOR_EVENTS * 4;
    let admitted_count = MAX_PENDING_SIMULATOR_EVENTS - 1;
    let last_sequence = first_sequence + u32::try_from(burst_len).expect("small simulator burst") - 1;
    for sequence in first_sequence..=last_sequence {
        let mut chat = test_fixture("chat_from_simulator.hex");
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
    assert_eq!(budget.decode_slots_available(), 8);

    let mut duplicate_chat = test_fixture("chat_from_simulator.hex");
    duplicate_chat[0] |= 0x40;
    set_sequence(&mut duplicate_chat, first_sequence);
    neighbor.send_to(&duplicate_chat, neighbor_peer).await.expect("retransmit a reliable packet after overload begins");
    receive_ack(&neighbor, first_sequence).await;

    let rejected_sequence = first_sequence + admitted_count as u32;
    let mut rejected_chat = test_fixture("chat_from_simulator.hex");
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

    let mut disable = test_fixture("disable_simulator.hex");
    disable[0] |= 0x40;
    set_sequence(&mut disable, last_sequence + 2);
    neighbor.send_to(&disable, neighbor_peer).await.expect("disable the overloaded neighbor");
    receive_ack(&neighbor, last_sequence + 2).await;

    assert!(!task.is_finished(), "the neighbor should remain alive while queued events cannot drain");
    let error = timeout(LOGOUT_TIMEOUT + Duration::from_secs(1), &mut task)
        .await
        .expect("overload teardown should finish within its deadline")
        .expect("neighbor task should not panic")
        .expect_err("backpressure must end with an explicit overload failure");
    std::assert_matches!(error, ConnectionError::ShutdownReason(reason) if reason.contains(EVENT_BACKLOG_DISCONNECT_REASON));
    assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    assert_eq!(budget.decode_slots_available(), 8);
    let queued = event_receiver.try_recv().expect("the held event remains queued");
    let SimulatorEvent::Simulator(queued) = queued else {
        panic!("expected queued simulator event");
    };
    std::assert_matches!(queued.into_event(), Event::Failed(message) if message == "hold application events");
    assert!(
        matches!(event_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected)),
        "overflow must not deliver queued application updates after failure"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn event_queue_starts_neighbor() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let primary_endpoint = primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let (ip, port) = match neighbor_endpoint {
        SocketAddr::V4(endpoint) => (endpoint.ip().octets(), endpoint.port()),
        SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
    };
    let agent_id = Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
    let neighbor_seed_server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/neighbor-event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "stopped")),
        ]
    });
    let neighbor_seed = neighbor_seed_server.url("/neighbor-seed");
    let server = EventQueueTestServer::start(move |base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        let seed = seed_response(&event_url);
        let event = simulator_events(ip, port, agent_id, &neighbor_seed);
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed)),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &event)),
            hold_response(),
        ]
    });

    let session = AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address: primary_endpoint,
        agent_id,
        session_id: Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID"),
        circuit_code: 0x1122_3344,
        seed_capability: server.url("/seed"),
    };
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (disconnect, shutdown) = watch::channel(false);
    let worker = spawn(run_session(session, command_receiver, event_sender, shutdown));

    let (primary_packet, primary_peer) = receive_udp(&primary).await;
    std::assert_matches!(Packet::decode(&primary_packet).expect("decode initial circuit").message(), Message::UseCircuitCode(_));
    primary.send_to(&acknowledgement(40, 1), primary_peer).await.expect("ack primary circuit");
    let (movement_packet, movement_peer) = receive_udp(&primary).await;
    let movement_packet = Packet::decode(&movement_packet).expect("decode initial movement");
    std::assert_matches!(movement_packet.message(), Message::CompleteAgentMovement(_));
    primary.send_to(&acknowledgement(41, movement_packet.sequence()), movement_peer).await.expect("ack initial movement");
    let (neighbor_packet, neighbor_peer) = receive_udp(&neighbor).await;
    let neighbor_packet = Packet::decode(&neighbor_packet).expect("decode neighbor circuit");
    assert_eq!(neighbor_packet.sequence(), 1);
    assert!(neighbor_packet.is_reliable());
    std::assert_matches!(neighbor_packet.message(), Message::UseCircuitCode(_));
    neighbor.send_to(&acknowledgement(40, 1), neighbor_peer).await.expect("ack neighbor circuit");

    let mut saw_event_batch = false;
    let mut saw_capabilities = false;
    while !(saw_event_batch && saw_capabilities) {
        let event = timeout(TEST_TIMEOUT, received_events.recv()).await.expect("forward capability events").expect("network event channel");
        match event {
            Event::CapabilityEventBatch { events, .. } => {
                assert!(!saw_event_batch, "one EventQueue response is published as one application batch");
                assert_eq!(
                    events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>(),
                    ["EnableSimulator", "EstablishAgentCommunication"],
                    "batch items keep simulator wire order"
                );
                saw_event_batch = true;
                std::assert_matches!(
                    events[1].body.as_map().and_then(|fields| fields.get("seed-capability")),
                    Some(Value::String(seed)) if seed == "[redacted]"
                );
            }
            Event::SimulatorCapabilities { source, .. } if source == neighbor_endpoint => saw_capabilities = true,
            _ => {}
        }
    }
    assert!(request_line(&neighbor_seed_server.next_request()).starts_with("POST /neighbor-seed HTTP/1.1"));

    disconnect.send_replace(true);
    let (logout, _) = receive_udp(&primary).await;
    std::assert_matches!(Packet::decode(&logout).expect("decode logout").message(), Message::LogoutRequest(_));
    timeout(TEST_TIMEOUT, worker).await.expect("network worker shutdown").expect("network task").expect("clean shutdown");
    server.stop();
    let requests = server.finish();
    assert!(requests.len() >= 2, "fake received seed lookup and event poll");
    neighbor_seed_server.stop();
    let _ = neighbor_seed_server.finish();
    assert_ne!(primary_peer, neighbor_peer);
}

#[tokio::test(flavor = "multi_thread")]
async fn crossed_region_promotes_after_matching_movement() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination simulator");
    let primary_endpoint = primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = neighbor.local_addr().expect("destination simulator address");
    let SocketAddr::V4(neighbor_endpoint_v4) = neighbor_endpoint else { panic!("fake destination must use IPv4") };
    let agent_id = Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
    let session_id = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID");
    let region_handle = 0x1000_0000_0000_1000;
    let neighbor_seed_server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/neighbor-event-queue");
        vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))), hold_response()]
    });
    let neighbor_seed = neighbor_seed_server.url("/neighbor-seed");
    let primary_seed_server = EventQueueTestServer::start(move |base| {
        let mut event_url = base.clone();
        event_url.set_path("/primary-event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(
                200,
                "OK",
                "application/llsd+xml",
                None,
                &crossed_region_events(agent_id, session_id, region_handle, neighbor_endpoint_v4, &neighbor_seed),
            )),
            hold_response(),
        ]
    });
    let session = AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address: primary_endpoint,
        agent_id,
        session_id,
        circuit_code: 0x1122_3344,
        seed_capability: primary_seed_server.url("/primary-seed"),
    };
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_disconnect, shutdown) = watch::channel(false);
    let worker = spawn(run_session(session.clone(), command_receiver, event_sender, shutdown));

    let (primary_packet, primary_peer) = receive_udp(&primary).await;
    std::assert_matches!(Packet::decode(&primary_packet).expect("decode primary circuit").message(), Message::UseCircuitCode(_));
    primary.send_to(&acknowledgement(40, 1), primary_peer).await.expect("ack primary circuit");
    let (movement, movement_peer) = receive_udp(&primary).await;
    let movement = Packet::decode(&movement).expect("decode primary movement request");
    std::assert_matches!(movement.message(), Message::CompleteAgentMovement(_));
    primary.send_to(&acknowledgement(41, movement.sequence()), movement_peer).await.expect("ack primary movement request");

    let mut primary_handshake = test_fixture("region_handshake.hex");
    primary_handshake[0] |= 0x40;
    set_sequence(&mut primary_handshake, 22);
    primary.send_to(&primary_handshake, primary_peer).await.expect("send primary region handshake");
    let reply = receive_message(&primary, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    primary.send_to(&acknowledgement(42, reply.sequence()), primary_peer).await.expect("ack primary handshake reply");
    let mut primary_movement_complete = test_fixture("agent_movement_complete.hex");
    primary_movement_complete[0] |= 0x40;
    set_sequence(&mut primary_movement_complete, 23);
    primary.send_to(&primary_movement_complete, primary_peer).await.expect("complete primary movement");

    let primary_id = loop {
        match timeout(TEST_TIMEOUT, received_events.recv()).await.expect("receive primary events").expect("event channel remains open") {
            Event::SimulatorConnected { simulator_id, endpoint, region_handle: None } if endpoint == primary_endpoint => {
                break simulator_id;
            }
            _ => {}
        }
    };
    let (neighbor_id, neighbor_peer) =
        prepare_crossed_region_destination(&neighbor, neighbor_endpoint, region_handle, &session, &mut received_events).await;
    promote_crossed_region(&neighbor, neighbor_peer, region_handle, &session, primary_id, neighbor_id, &mut received_events).await;

    commands
        .send(Command::SendMessage {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 9 } }),
            delivery: Delivery::Unreliable,
        })
        .await
        .expect("send through the promoted simulator");
    let routed = receive_message(&neighbor, |message| matches!(message, Message::CompletePingCheck(_))).await;
    std::assert_matches!(routed.message(), Message::CompletePingCheck(_));

    drop(commands);
    let logout = receive_message(&neighbor, |message| matches!(message, Message::LogoutRequest(_))).await;
    std::assert_matches!(logout.message(), Message::LogoutRequest(_));
    timeout(TEST_TIMEOUT, worker).await.expect("network worker shutdown").expect("network task").expect("clean shutdown");
    primary_seed_server.stop();
    let primary_requests = primary_seed_server.finish();
    assert!(primary_requests.len() >= 2, "primary event queue should receive its seed and poll requests");
    neighbor_seed_server.stop();
    let _ = neighbor_seed_server.finish();
}
