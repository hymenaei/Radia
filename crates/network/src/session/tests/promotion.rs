use std::mem::replace;

use protocol::login::Value as LoginValue;

use super::login::*;
use super::*;
use crate::circuit::lludp::advance_sequence;

struct PromotedSession {
    session: Session,
    handle: SessionHandle,
    worker: JoinHandle<Result<(), SessionError>>,
    primary: UdpSocket,
    destination: UdpSocket,
    destination_peer: SocketAddr,
    destination_id: SimulatorId,
    http_task: JoinHandle<Vec<Vec<u8>>>,
}

async fn promoted_session() -> PromotedSession {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let primary_address = primary.local_addr().expect("read primary address");
    let destination = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination simulator");
    let destination_address = destination.local_addr().expect("read destination address");
    let agent_id = Uuid::parse_str(AGENT_ID).expect("agent UUID");
    let session_id = Uuid::parse_str(SESSION_ID).expect("session UUID");
    let (login_url, poll_started, http_task) = fake_http(primary_address.port()).await;
    let (mut session, handle, worker) = spawn_test_session(login_url);

    std::assert_matches!(login_next_event(&mut session.events).await, Event::LoginAccepted { .. });
    let (use_circuit, primary_peer) = receive_udp(&primary).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    primary.send_to(&server_ack(20, 1), primary_peer).await.expect("ack primary UseCircuitCode");
    let (_, movement_request) =
        receive_client_message(&primary, primary_peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    primary.send_to(&server_ack(21, movement_request.sequence()), primary_peer).await.expect("ack primary CompleteAgentMovement");
    primary.send_to(&reliable_fixture("region_handshake.hex", 22), primary_peer).await.expect("send primary RegionHandshake");
    let (_, region_reply) =
        receive_client_message(&primary, primary_peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    primary.send_to(&server_ack(23, region_reply.sequence()), primary_peer).await.expect("ack primary RegionHandshakeReply");
    primary
        .send_to(&movement_complete_packet(24, agent_id, session_id, 0), primary_peer)
        .await
        .expect("send primary AgentMovementComplete");
    let (_, primary_throttle) =
        receive_client_message(&primary, primary_peer, |message| matches!(message, Message::AgentThrottle(_))).await;
    assert!(primary_throttle.is_reliable());
    primary.send_to(&server_ack(25, primary_throttle.sequence()), primary_peer).await.expect("ack primary AgentThrottle");

    let root_id = loop {
        match login_next_event(&mut session.events).await {
            Event::SessionReady { simulator_id, .. } => break simulator_id,
            Event::Failed(error) => panic!("session failed before readiness: {error}"),
            _ => {}
        }
    };
    let event_queue_request = timeout(CLIENT_TIMEOUT, poll_started)
        .await
        .expect("primary EventQueueGet should start")
        .expect("fake HTTP server should observe EventQueueGet");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));

    primary
        .send_to(&teleport_finish_packet(agent_id, destination_address, 0x1000_0000_0000_1000, "http://127.0.0.1:1/seed"), primary_peer)
        .await
        .expect("send TeleportFinish");
    let (use_circuit, destination_peer) = receive_udp(&destination).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    destination.send_to(&server_ack(30, 1), destination_peer).await.expect("ack destination UseCircuitCode");
    destination.send_to(&reliable_fixture("region_handshake.hex", 31), destination_peer).await.expect("send destination RegionHandshake");
    let (_, destination_reply) =
        receive_client_message(&destination, destination_peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let (_, destination_movement) =
        receive_client_message(&destination, destination_peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    destination
        .send_to(&server_ack(32, destination_reply.sequence()), destination_peer)
        .await
        .expect("ack destination RegionHandshakeReply");
    destination
        .send_to(&server_ack(33, destination_movement.sequence()), destination_peer)
        .await
        .expect("ack destination CompleteAgentMovement");
    destination
        .send_to(&movement_complete_packet(34, agent_id, session_id, 0x1000_0000_0000_1000), destination_peer)
        .await
        .expect("send destination AgentMovementComplete");

    let destination_id = timeout(CLIENT_TIMEOUT, async {
        loop {
            match login_next_event(&mut session.events).await {
                Event::SimulatorPromoted { previous_simulator_id, simulator_id, .. } => {
                    assert_eq!(previous_simulator_id, root_id);
                    break simulator_id;
                }
                Event::Failed(error) | Event::SimulatorFailed { reason: error, .. } => {
                    panic!("session failed before destination promotion: {error}");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("destination movement should promote the simulator");
    let (_, destination_throttle) =
        receive_client_message(&destination, destination_peer, |message| matches!(message, Message::AgentThrottle(_))).await;
    assert!(destination_throttle.is_reliable());
    destination.send_to(&server_ack(35, destination_throttle.sequence()), destination_peer).await.expect("ack destination AgentThrottle");

    PromotedSession { session, handle, worker, primary, destination, destination_peer, destination_id, http_task }
}

async fn receive_logout(destination: &UdpSocket, destination_peer: SocketAddr) {
    let (logout_bytes, logout) =
        receive_client_message(destination, destination_peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    std::assert_matches!(logout.message(), Message::LogoutRequest(_));
    let mut expected_logout = expected_fixture("logout_request.hex", logout.sequence());
    if logout.appended_acks() == [34] {
        expected_logout = with_appended_ack(expected_logout, 34);
    }
    assert_eq!(logout_bytes, expected_logout);
}

async fn assert_destination_disconnected(events: &mut mpsc::Receiver<EventEnvelope>, destination_id: SimulatorId) {
    let mut saw_destination_disconnect = false;
    while let Some(envelope) = timeout(CLIENT_TIMEOUT, events.recv()).await.expect("session event channel should close after disconnect") {
        match envelope.into_event() {
            Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == destination_id => {
                saw_destination_disconnect = true;
            }
            Event::Failed(error) | Event::SimulatorFailed { reason: error, .. } => {
                panic!("graceful disconnect emitted a failure: {error}");
            }
            _ => {}
        }
    }
    assert!(saw_destination_disconnect, "promoted simulator should end with a disconnect event");
}

#[tokio::test]
async fn disconnect_cancels_login() {
    let (disconnect, mut disconnect_requested) = watch::channel(false);
    let login = wait_for_login(pending::<()>(), &mut disconnect_requested);
    disconnect.send_replace(true);

    assert!(timeout(Duration::from_secs(1), login).await.expect("login cancellation should return promptly").is_none());
}

#[tokio::test]
async fn disconnect_skips_login() {
    let (_disconnect, mut disconnect_requested) = watch::channel(true);

    assert!(wait_for_login(pending::<()>(), &mut disconnect_requested).await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn session_drop_cancels_bootstrap() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, retained_handle, worker) = spawn_test_session(login_url);

    std::assert_matches!(login_next_event(&mut session.events).await, Event::LoginAccepted { .. });
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");

    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");

    let event_queue_request = timeout(Duration::from_secs(3), poll_started)
        .await
        .expect("bootstrap should start EventQueueGet")
        .expect("fake HTTP server should observe EventQueueGet");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));
    drop(session);

    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(timeout(Duration::from_secs(2), worker).await.expect("bootstrap worker should stop").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());

    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("server task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_handoff() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let credentials = LoginCredentials {
        first_name: "Input".into(),
        last_name: "Name".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: "saved-mfa".into(),
    };
    let (mut network_session, retained_handle, client) = spawn_session_client(credentials, login_url);

    std::assert_matches!(login_next_event(&mut network_session.events).await, Event::LoginAccepted { mfa_hash: Some(hash) } if hash == "returned-mfa"
    );
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));

    let event_queue_request = timeout(Duration::from_secs(3), poll_started)
        .await
        .expect("simulator startup should request EventQueueGet")
        .expect("fake HTTP server should observe EventQueueGet");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));

    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");

    let region = reliable_fixture("region_handshake.hex", 22);
    simulator.send_to(&region, peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");

    let movement_complete = reliable_fixture("agent_movement_complete.hex", 24);
    simulator.send_to(&movement_complete, peer).await.expect("send AgentMovementComplete");
    let (agent_session, login_data) = loop {
        match login_next_event(&mut network_session.events).await {
            Event::SessionReady { session, login_data, .. } => break (session, login_data),
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    };

    assert_eq!(agent_session.first_name, "Session");
    assert_eq!(agent_session.last_name, "Avatar");
    assert_eq!(agent_session.grid, Grid::Aditi);
    assert_eq!(agent_session.simulator_address, simulator_address);
    assert_eq!(agent_session.agent_id.to_string(), AGENT_ID);
    assert_eq!(agent_session.session_id.to_string(), SESSION_ID);
    assert_eq!(agent_session.circuit_code, CIRCUIT_CODE);
    let LoginValue::Array(folders) = login_data.get("inventory-skeleton").expect("login metadata survives handoff") else {
        panic!("expected inventory skeleton array");
    };
    let [LoginValue::Struct(folder)] = folders.as_slice() else {
        panic!("expected one inventory skeleton folder");
    };
    assert_eq!(folder.get("folder_id"), Some(&LoginValue::String(FOLDER_ID.into())));
    assert_eq!(folder.get("name"), Some(&LoginValue::String("Favorites".into())));

    drop(network_session);
    let mut next_sequence = advance_sequence(reply_packet.sequence());
    let mut movement_acknowledged = false;
    loop {
        let (bytes, shutdown_peer) = receive_udp(&simulator).await;
        assert_eq!(shutdown_peer, peer);
        let packet = Packet::decode(&bytes).expect("decode simulator shutdown packet");
        assert_eq!(packet.sequence(), next_sequence);
        next_sequence = advance_sequence(next_sequence);

        let mut acknowledgements = packet.appended_acks().to_vec();
        if let Message::PacketAck(message) = packet.message() {
            acknowledgements.extend(message.packets.iter().map(|packet| packet.id));
        }
        for acknowledged in acknowledgements {
            assert_eq!(acknowledged, 24, "shutdown should acknowledge AgentMovementComplete");
            assert!(!movement_acknowledged, "AgentMovementComplete must only be acknowledged once");
            movement_acknowledged = true;
        }

        match packet.message() {
            Message::PacketAck(_) => {
                assert!(packet.appended_acks().is_empty());
                assert_eq!(bytes, expected_packet_ack(packet.sequence(), 24));
            }
            Message::StartPingCheck(_) => assert!(!packet.is_reliable()),
            Message::AgentThrottle(_) => {
                assert!(packet.is_reliable());
                simulator.send_to(&server_ack(25, packet.sequence()), peer).await.expect("ack AgentThrottle during shutdown");
            }
            Message::LogoutRequest(_) => {
                let mut expected = expected_fixture("logout_request.hex", packet.sequence());
                if packet.appended_acks() == [24] {
                    expected = with_appended_ack(expected, 24);
                }
                assert_eq!(bytes, expected);
                assert!(movement_acknowledged, "logout must acknowledge AgentMovementComplete");
                break;
            }
            message => panic!("unexpected simulator shutdown message: {message:?}"),
        }
    }
    assert!(timeout(Duration::from_secs(2), client).await.expect("disconnect should return promptly").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());

    let requests = timeout(Duration::from_secs(2), http_task).await.expect("local HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /login.cgi HTTP/1.1"));
    assert!(request_body(&requests[0]).contains("login_to_simulator"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_body(&requests[1]).contains("EventQueueGet"));
    assert!(request_line(&requests[2]).starts_with("POST /events HTTP/1.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_after_promotion() {
    let PromotedSession { mut session, handle, worker, primary: _primary, destination, destination_peer, destination_id, http_task } =
        promoted_session().await;
    let mut events = replace(&mut session.events, mpsc::channel(1).1);
    handle.request_disconnect();
    let disconnect = spawn(async move {
        session.disconnect().await;
        session.disconnect().await;
    });

    receive_logout(&destination, destination_peer).await;
    timeout(CLIENT_TIMEOUT, disconnect).await.expect("disconnect should wait for teardown").expect("disconnect task");
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should finish").expect("client task").is_ok());
    assert_destination_disconnected(&mut events, destination_id).await;

    let requests = timeout(CLIENT_TIMEOUT, http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_after_promotion_with_full_reliable_window() {
    let PromotedSession { mut session, worker, primary: _primary, destination, destination_peer, destination_id, http_task, .. } =
        promoted_session().await;

    for local_id in 0..MAX_PENDING_PACKETS {
        let request = RequestMultipleObjects {
            agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
            objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: local_id as u32 }],
        };
        timeout(CLIENT_TIMEOUT, session.send_to(destination_id, request, Delivery::Reliable))
            .await
            .expect("reliable send should finish while filling the window")
            .expect("send_to should not fail before the reliable window is full");
    }

    for local_id in 0..MAX_PENDING_PACKETS {
        let (_, packet) =
            receive_client_message(&destination, destination_peer, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert!(packet.is_reliable());
        let Message::RequestMultipleObjects(request) = packet.message() else {
            unreachable!("message filter only accepts object requests")
        };
        assert_eq!(request.objects[0].local_id, local_id as u32);
    }

    let mut events = replace(&mut session.events, mpsc::channel(1).1);
    let disconnect = spawn(async move { session.disconnect().await });

    receive_logout(&destination, destination_peer).await;
    timeout(CLIENT_TIMEOUT, disconnect).await.expect("disconnect should finish with a full reliable window").expect("disconnect task");
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should finish").expect("client task").is_ok());
    assert_destination_disconnected(&mut events, destination_id).await;

    let requests = timeout(CLIENT_TIMEOUT, http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_after_promotion() {
    let PromotedSession { mut session, worker, primary: _primary, destination, destination_peer, destination_id, http_task, .. } =
        promoted_session().await;
    let handle = session.handle();
    let mut events = replace(&mut session.events, mpsc::channel(1).1);
    drop(session);

    receive_logout(&destination, destination_peer).await;
    handle.disconnect().await;
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should finish").expect("client task").is_ok());
    assert_destination_disconnected(&mut events, destination_id).await;

    let requests = timeout(CLIENT_TIMEOUT, http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn mfa_continuation() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http_with_mfa(simulator_address.port(), true).await;
    let credentials = LoginCredentials {
        first_name: "Input".into(),
        last_name: "Name".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: "saved-mfa".into(),
    };
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = events_sender.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let (disconnect, disconnect_requested) = watch::channel(false);
    let (_handoff_cancellation_sender, handoff_cancellations) = mpsc::channel(1);
    let client = spawn(run_client(
        credentials,
        Channels {
            commands: command_receiver,
            command_sender: commands.clone(),
            handoff_cancellations,
            events: events_sender,
            terminal_event_permit,
            disconnect_requested,
            coverage_losses: CoverageLossLedger::new(),
        },
        login_url,
        SessionOptions::default(),
    ));

    let challenge = match login_next_event(&mut events).await {
        Event::MfaRequired { challenge } => challenge,
        other => panic!("expected MFA challenge, got {other:?}"),
    };
    challenge.complete(" 123456 ").expect("complete login challenge");
    std::assert_matches!(login_next_event(&mut events).await, Event::LoginAccepted { mfa_hash: Some(hash) } if hash == "returned-mfa");
    let event_request = timeout(Duration::from_secs(2), poll_started).await.expect("EventQueueGet should start").expect("event request");
    assert!(request_line(&event_request).starts_with("POST /events HTTP/1.1"));

    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    let session = loop {
        match login_next_event(&mut events).await {
            Event::SessionReady { session, .. } => break session,
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    };
    assert_eq!(session.agent_id.to_string(), AGENT_ID);

    let (_, throttle) = receive_client_message(&simulator, peer, |message| matches!(message, Message::AgentThrottle(_))).await;
    assert!(throttle.is_reliable());
    assert_eq!(throttle.appended_acks(), &[24]);
    simulator.send_to(&server_ack(25, throttle.sequence()), peer).await.expect("ack AgentThrottle");

    disconnect.send_replace(true);
    let (logout_bytes, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(logout.appended_acks().is_empty());
    assert_eq!(logout_bytes, expected_fixture("logout_request.hex", logout.sequence()));
    assert!(timeout(Duration::from_secs(2), client).await.expect("client should disconnect").expect("client task").is_ok());

    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 4);
    let initial_login = request_body(&requests[0]);
    let continued_login = request_body(&requests[1]);
    assert!(initial_login.contains("saved-mfa"));
    assert!(!initial_login.contains("123456"));
    assert!(continued_login.contains("123456"));
    assert!(!continued_login.contains("saved-mfa"));
}
