use super::disconnect::{
    SIMULATOR_TIMEOUT, assert_no_datagram, client_ack, expected_chat, expected_complete_movement, expected_fixture,
    expected_zerocoded_reply, packet_ack_contains, receive_primary_connected, server_ack_many, with_appended_ack,
};
use super::*;
use tokio::task::yield_now;

#[tokio::test]
async fn handshake_chat_logout() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));

    assert_no_datagram(&simulator);
    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack circuit establishment");

    let (movement, movement_peer) = receive(&simulator).await;
    assert_eq!(movement_peer, peer);
    assert_eq!(movement, expected_complete_movement(2));
    simulator.send_to(&server_ack_many(21, &[2]), peer).await.expect("ack complete movement");

    let mut malformed = [0x80, 0, 0, 0, 0, 0, 0x04, 0];
    set_sequence(&mut malformed, 22);
    simulator.send_to(&malformed, peer).await.expect("send malformed datagram");

    let mut region_handshake = test_fixture("region_handshake.hex");
    region_handshake[0] |= 0x40;
    set_sequence(&mut region_handshake, 24);
    simulator.send_to(&region_handshake, peer).await.expect("send region handshake before delayed movement completion");
    let (reply, reply_peer) = receive(&simulator).await;
    assert_eq!(reply_peer, peer);
    assert_eq!(reply, with_appended_ack(expected_zerocoded_reply(3), 24));
    simulator.send_to(&server_ack_many(25, &[3]), peer).await.expect("ack region handshake reply");
    let handshake_event = next_event(&mut events).await;
    let Event::SimulatorMessage { source, sequence, message, .. } = handshake_event else {
        panic!("expected the region handshake to be delivered to the application");
    };
    assert_eq!(source, simulator.local_addr().expect("simulator address"));
    assert_eq!(sequence, 24);
    std::assert_matches!(message, Message::RegionHandshake(message) if message.region_info1.sim_name == "Test Region");

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("deliver delayed movement completion out of order");
    let Event::SessionReady { capabilities, .. } = next_event(&mut events).await else {
        panic!("expected the simulator session to become ready");
    };
    let movement_event = next_event(&mut events).await;
    std::assert_matches!(movement_event, Event::SimulatorMessage { sequence: 23, message: Message::AgentMovementComplete(_), .. });

    let throttle = Packet::decode(&receive(&simulator).await.0).expect("decode AgentThrottle");
    std::assert_matches!(throttle.message(), Message::AgentThrottle(_));
    assert!(throttle.is_reliable());
    assert!(throttle.appended_acks().contains(&23), "AgentThrottle carries the pending movement ACK");
    simulator.send_to(&server_ack_many(26, &[throttle.sequence()]), peer).await.expect("ack AgentThrottle");

    commands
        .send(Command::SendMessage { message: chat_message("ab"), delivery: Delivery::Reliable })
        .await
        .expect("session should accept a simulator message");
    let (chat, chat_peer) = receive(&simulator).await;
    assert_eq!(chat_peer, peer);
    assert_eq!(chat, expected_chat(5, "ab", 1, 0));
    simulator.send_to(&server_ack_many(27, &[5]), peer).await.expect("ack nearby chat");

    let mut incoming_chat = test_fixture("chat_from_simulator.hex");
    incoming_chat[0] |= 0x40;
    set_sequence(&mut incoming_chat, 28);
    simulator.send_to(&incoming_chat, peer).await.expect("send reliable nearby chat");
    let (first_ack, first_ack_peer) = receive(&simulator).await;
    assert_eq!(first_ack_peer, peer);
    assert_eq!(first_ack, client_ack(6, 28));

    incoming_chat[0] |= RESENT_FLAG;
    simulator.send_to(&incoming_chat, peer).await.expect("resend nearby chat");
    let (duplicate_ack, duplicate_ack_peer) = receive(&simulator).await;
    assert_eq!(duplicate_ack_peer, peer);
    assert_eq!(duplicate_ack, client_ack(7, 28));

    let chat_event = next_event(&mut events).await;
    let Event::SimulatorMessage { source, sequence, message, .. } = chat_event else {
        panic!("expected one generic simulator message event");
    };
    assert_eq!(source, simulator.local_addr().expect("simulator address"));
    assert_eq!(sequence, 28);
    let Message::ChatFromSimulator(message) = message else {
        panic!("expected decoded ChatFromSimulator message");
    };
    assert_eq!(message.chat_data.from_name, "Ada");
    assert_eq!(message.chat_data.message, "Hi");
    yield_now().await;
    assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    disconnect_signal.send_replace(true);
    let (logout, logout_peer) = receive(&simulator).await;
    assert_eq!(logout_peer, peer);
    assert_eq!(logout, expected_fixture("logout_request.hex", 8, true));
    assert!(
        timeout(Duration::from_millis(200), client)
            .await
            .expect("disconnect should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
    let error = capabilities.request_capabilities(&["TestCapability"]).await.expect_err("session teardown closes retained capabilities");
    std::assert_matches!(error, Error::CapabilitySessionClosed);
}

#[tokio::test]
async fn region_handshake_reply_is_explicit_and_preserves_application_flags() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let connection = connect_runtime(session, ResponseData::default()).await.expect("connect simulator runtime");
    let terminal = event_sender.clone().try_reserve_owned().expect("reserve terminal event slot");
    let client = spawn(connection.run(command_receiver, event_sender, disconnect_requested, terminal));

    let (_, peer) = receive(&simulator).await;
    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack_many(21, &[2]), peer).await.expect("ack movement request");

    for (sequence, flags, server_ack) in [(22, 7, 25), (23, 5, 26), (24, 7, 27)] {
        let mut handshake = test_fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, sequence);
        simulator.send_to(&handshake, peer).await.expect("send region handshake");
        receive_ack(&simulator, sequence).await;
        let simulator_id = loop {
            if let Event::SimulatorMessage { simulator_id, message: Message::RegionHandshake(_), .. } = next_event(&mut events).await {
                break simulator_id;
            }
        };
        assert_no_datagram(&simulator);

        let (completed, response) = oneshot::channel();
        commands.send(Command::ReplyRegionHandshake { simulator_id, flags, completed }).await.expect("queue explicit handshake reply");
        response.await.expect("network completion remains connected").expect("handshake reply is transmitted");
        let reply = receive_message(&simulator, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        std::assert_matches!(reply.message(), Message::RegionHandshakeReply(reply) if reply.region_info.flags == flags);
        simulator.send_to(&server_ack_many(server_ack, &[reply.sequence()]), peer).await.expect("ack explicit handshake reply");
    }

    disconnect_signal.send_replace(true);
    receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    timeout(SIMULATOR_TIMEOUT, client).await.expect("session task stops").expect("session task does not panic").expect("clean shutdown");
}

#[tokio::test]
async fn handshake_survives_application_backlog() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack_many(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = test_fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
    simulator.send_to(&server_ack_many(26, &[3]), peer).await.expect("ack region handshake reply");
    std::assert_matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. });

    for sequence in 23..=38 {
        let mut chat = test_fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        simulator.send_to(&chat, peer).await.expect("fill the bounded event backlog");

        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client ACK");
            let acknowledged = packet.appended_acks().contains(&sequence) || packet_ack_contains(packet.message(), sequence);
            if acknowledged {
                break;
            }
        }
    }

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 39);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion at backlog limit");

    let mut received = HashSet::new();
    let mut saw_session_ready = false;
    while !(saw_session_ready && received.len() == 17) {
        match next_event(&mut events).await {
            Event::SessionReady { .. } => saw_session_ready = true,
            Event::SimulatorMessage { sequence, .. } if (23..=39).contains(&sequence) => {
                assert!(received.insert(sequence), "each queued simulator message should be delivered once");
            }
            Event::Failed(reason) => panic!("queued startup events ended the session: {reason}"),
            _ => {}
        }
    }
    assert_eq!(received, (23..=39).collect());

    disconnect_signal.send_replace(true);
    assert!(
        timeout(Duration::from_millis(200), client)
            .await
            .expect("disconnect should finish")
            .expect("session task should not panic")
            .is_ok()
    );
}

#[tokio::test]
async fn closed_receiver_logs_out() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (_disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack_many(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = test_fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
    std::assert_matches!(next_event(&mut events).await, Event::SimulatorMessage { message: Message::RegionHandshake(_), .. });

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    drop(events);
    simulator.send_to(&movement_complete, peer).await.expect("send packet as receiver closes");
    assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", 4, true));

    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
    assert_no_datagram(&simulator);
}

#[tokio::test]
async fn disconnect_blocks_movement() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));

    simulator.try_send_to(&server_ack_many(20, &[1]), peer).expect("queue circuit ACK while disconnect is pending");
    disconnect_signal.send_replace(true);
    let (logout, _) = receive(&simulator).await;
    assert_eq!(logout, expected_fixture("logout_request.hex", 2, true));

    assert!(
        timeout(SIMULATOR_TIMEOUT, client)
            .await
            .expect("session should stop after disconnect")
            .expect("session task should not panic")
            .is_ok()
    );
    assert_no_datagram(&simulator);
}

#[test]
fn chat_bytes_match_fixture() {
    assert_eq!(expected_chat(0x0102_0304, "abc", 2, 42), test_fixture("chat_from_viewer.hex"));
}

#[test]
fn ack_bytes_match_fixture() {
    assert_eq!(server_ack_many(0x0102_0304, &[0x1122_3344, 0xa1b2_c3d4]), test_fixture("packet_ack.hex"));
}
