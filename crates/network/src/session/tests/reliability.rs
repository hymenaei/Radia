use super::disconnect::{
    RETRANSMISSION_TIMEOUT, SIMULATOR_TIMEOUT, assert_no_datagram, complete_ping, expected_complete_movement, expected_fixture,
    expected_zerocoded_reply, is_complete_ping, logout_message, packet_ack, receive_packet, receive_primary_connected,
    receive_with_timeout, server_ack_many, start_ping_packet, with_appended_ack,
};
use super::*;
use tokio::time::Instant;
async fn wait_for_acknowledgement(simulator: &UdpSocket, sequence: u32, duration: Duration) {
    let deadline = Instant::now() + duration;
    let mut buffer = [0; 65_535];
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "simulator should acknowledge packet {sequence}");
        let (size, _) = timeout(remaining, simulator.recv_from(&mut buffer))
            .await
            .expect("simulator should acknowledge packet before its deadline")
            .expect("read simulator acknowledgement traffic");
        let packet = Packet::decode(&buffer[..size]).expect("decode simulator acknowledgement traffic");
        let acknowledged = packet.appended_acks().contains(&sequence)
            || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|ack| ack.id == sequence));
        if acknowledged {
            return;
        }
    }
}

#[tokio::test]
async fn ping_reacks_recent_duplicate() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, commands.clone(), command_receiver, event_sender, disconnect_requested));

    let (_, peer) = receive(&simulator).await;
    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack_many(21, &[2]), peer).await.expect("ack complete movement");

    let mut chat = test_fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 100);
    simulator.send_to(&chat, peer).await.expect("send reliable chat");
    std::assert_matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 100, .. });
    receive_ack(&simulator, 100).await;

    let ping = start_ping_packet(102, 9, 101);
    simulator.send_to(&ping, peer).await.expect("send ping checkpoint");
    receive_message(&simulator, |message| is_complete_ping(message, 9)).await;

    chat[0] |= RESENT_FLAG;
    simulator.send_to(&chat, peer).await.expect("resend acknowledged chat");
    receive_ack(&simulator, 100).await;
    assert_eq!(events.try_recv().err(), Some(mpsc::error::TryRecvError::Empty));

    disconnect_signal.send_replace(true);
    receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("disconnect should finish").expect("session task").is_ok());
}

#[tokio::test]
async fn shutdown_preempts_chat() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, commands.clone(), command_receiver, event_sender, disconnect_requested));

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

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    loop {
        if matches!(next_event(&mut events).await, Event::SessionReady { .. }) {
            break;
        }
    }

    commands
        .try_send(Command::SendMessage { message: chat_message("must not send"), delivery: Delivery::Reliable })
        .expect("queue chat before shutdown");
    disconnect_signal.send_replace(true);

    let logout = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client packet");
            assert!(!matches!(packet.message(), Message::ChatFromViewer(_)), "queued chat must not be sent after shutdown");
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break packet;
            }
        }
    })
    .await
    .expect("shutdown should send a logout request");
    assert!(logout.is_reliable());
    assert_eq!(logout.message(), &logout_message());
    assert!(
        timeout(SIMULATOR_TIMEOUT, client)
            .await
            .expect("shutdown should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
    assert_no_datagram(&simulator);
}

#[tokio::test]
async fn backpressure_keeps_acks() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, commands.clone(), command_receiver, event_sender, disconnect_requested));

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

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    let throttle = Packet::decode(&receive(&simulator).await.0).expect("decode AgentThrottle");
    std::assert_matches!(throttle.message(), Message::AgentThrottle(_));
    assert!(throttle.is_reliable());
    assert!(throttle.appended_acks().contains(&23), "AgentThrottle carries the pending movement ACK");
    simulator.send_to(&server_ack_many(24, &[throttle.sequence()]), peer).await.expect("ack AgentThrottle");

    disconnect_signal.send_replace(true);
    assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", 5, true));
    assert!(
        timeout(SIMULATOR_TIMEOUT, client)
            .await
            .expect("disconnect should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
}

#[tokio::test]
async fn ping_survives_backpressure() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, commands.clone(), command_receiver, event_sender, disconnect_requested));

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

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    std::assert_matches!(next_event(&mut events).await, Event::SessionReady { .. });
    std::assert_matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. });

    for sequence in [24, 25] {
        let mut chat = test_fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        simulator.send_to(&chat, peer).await.expect("send reliable simulator chat");
    }

    let ping = start_ping_packet(27, 7, 0);
    simulator.send_to(&ping, peer).await.expect("send ping request while viewer queue is full");
    let ping_reply = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode simulator reply");
            if is_complete_ping(packet.message(), 7) {
                break packet;
            }
        }
    })
    .await
    .expect("simulator should answer ping while event delivery is backed up");
    assert_eq!(ping_reply.message(), &complete_ping(7));
    std::assert_matches!(events.try_recv().map(EventEnvelope::into_event), Ok(Event::SimulatorMessage { sequence: 24, .. }));

    disconnect_signal.send_replace(true);
    let logout = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client message");
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break packet;
            }
        }
    })
    .await
    .expect("disconnect should be handled while events are queued");
    std::assert_matches!(logout.message(), Message::LogoutRequest(_));
    assert!(
        timeout(SIMULATOR_TIMEOUT, client)
            .await
            .expect("disconnect should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
}

#[tokio::test]
async fn region_arrival_object_burst_survives_slow_consumer() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let coverage_losses = CoverageLossLedger::new();
    let mut options = test_options();
    options.coverage_losses = coverage_losses.clone();
    let client = spawn(run_session_with_options(session, commands.clone(), command_receiver, event_sender, disconnect_requested, options));

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

    let mut movement_complete = test_fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");

    let mut session_ready = false;
    let mut movement_delivered = false;
    while !session_ready || !movement_delivered {
        match next_event(&mut events).await {
            Event::SessionReady { .. } => session_ready = true,
            Event::SimulatorMessage { sequence: 23, .. } => movement_delivered = true,
            Event::Failed(reason) => panic!("session failed during region arrival: {reason}"),
            _ => {}
        }
    }

    const OBJECT_COUNT: usize =
        crate::circuit::admission::SESSION_APPLICATION_EVENT_CAPACITY + SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY;
    let first_sequence = 24;
    let last_sequence = first_sequence + OBJECT_COUNT as u32 - 1;
    let mut chunk_start = first_sequence;
    while chunk_start <= last_sequence {
        let chunk_end = (chunk_start + 127).min(last_sequence);
        for sequence in chunk_start..=chunk_end {
            let mut update = test_fixture("object_update.hex");
            update[0] |= 0x40;
            set_sequence(&mut update, sequence);
            simulator.send_to(&update, peer).await.expect("send region-arrival object update");
        }
        wait_for_acknowledgement(&simulator, chunk_end, SIMULATOR_TIMEOUT).await;
        chunk_start = chunk_end + 1;
    }

    let rejected_sequence = last_sequence + 1;
    let mut rejected_update = test_fixture("object_update.hex");
    rejected_update[0] |= 0x40;
    set_sequence(&mut rejected_update, rejected_sequence);
    simulator.send_to(&rejected_update, peer).await.expect("send update beyond the application-event bound");
    simulator.send_to(&start_ping_packet(rejected_sequence + 1, 10, 0), peer).await.expect("fence rejected object admission");
    let (_, rejected_acks) = crate::fixtures::simulator::receive_message_with_acknowledgements(
        &simulator,
        |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 10),
    )
    .await;
    assert!(!rejected_acks.contains(&rejected_sequence), "a reliable update rejected for lack of credit must remain unacknowledged");
    assert!(
        coverage_losses
            .snapshot()
            .losses
            .iter()
            .any(|loss| matches!(loss, CoverageLoss::Source(record) if record.reason == CoverageLossReason::EventAdmission))
    );

    let first = next_event(&mut events).await;
    std::assert_matches!(first, Event::SimulatorMessage { sequence, message: Message::ObjectUpdate(_), .. } if sequence == first_sequence);
    let mut retry = rejected_update;
    retry[0] |= RESENT_FLAG;
    simulator.send_to(&retry, peer).await.expect("retry the reliable update after a credit is released");
    wait_for_acknowledgement(&simulator, rejected_sequence, SIMULATOR_TIMEOUT).await;

    let mut received = HashSet::with_capacity(OBJECT_COUNT + 1);
    received.insert(first_sequence);
    timeout(SIMULATOR_TIMEOUT, async {
        while received.len() < OBJECT_COUNT + 1 {
            match next_event(&mut events).await {
                Event::SimulatorMessage { sequence, message: Message::ObjectUpdate(_), .. }
                    if (first_sequence..=rejected_sequence).contains(&sequence) =>
                {
                    assert!(received.insert(sequence), "each accepted object update should arrive once");
                }
                Event::Failed(reason) => panic!("slow consumer ended the session: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("the admitted burst and retried packet should be delivered");
    assert_eq!(received, (first_sequence..=rejected_sequence).collect());

    let last_pending_sequence = rejected_sequence + CLIENT_EVENT_CAPACITY as u32 + 4;
    for sequence in rejected_sequence + 1..=last_pending_sequence {
        let mut update = test_fixture("object_update.hex");
        update[0] |= 0x40;
        set_sequence(&mut update, sequence);
        simulator.send_to(&update, peer).await.expect("fill the viewer event channel before disconnect");
    }
    wait_for_acknowledgement(&simulator, last_pending_sequence, SIMULATOR_TIMEOUT).await;

    disconnect_signal.send_replace(true);
    receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
}

#[tokio::test]
async fn sustained_application_pressure_disconnects() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let mut options = test_options();
    options.application_event_capacity = 0;
    let client = spawn(run_session_with_options(session, commands.clone(), command_receiver, event_sender, disconnect_requested, options));

    let (_, peer) = receive(&simulator).await;
    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack_many(21, &[2]), peer).await.expect("ack complete movement");
    receive_primary_connected(&mut events).await;

    let mut rejected = test_fixture("chat_from_simulator.hex");
    rejected[0] |= 0x40;
    let rejected_sequence = 22;
    set_sequence(&mut rejected, rejected_sequence);
    simulator.send_to(&rejected, peer).await.expect("send application event beyond test capacity");
    let mut acknowledged = HashSet::new();
    let logout = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode simulator disconnect traffic");
            acknowledged.extend(packet.appended_acks().iter().copied());
            if let Message::PacketAck(message) = packet.message() {
                acknowledged.extend(message.packets.iter().map(|item| item.id));
            }
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break packet;
            }
        }
    })
    .await
    .expect("sustained pressure should send logout after the grace period");
    std::assert_matches!(logout.message(), Message::LogoutRequest(_));
    assert!(!acknowledged.contains(&rejected_sequence), "the rejected reliable packet must stay unacknowledged");
    std::assert_matches!(
        timeout(SIMULATOR_TIMEOUT, client).await.expect("pressure disconnect should finish").expect("session task should not panic"),
        Err(crate::session::SessionError::SustainedApplicationEventPressure)
    );
    disconnect_signal.send_replace(true);
}

#[tokio::test]
async fn retry_resends_then_expires() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();
    let prepared = circuit.prepare_message(complete_ping(1), Delivery::Reliable).expect("prepare original reliable packet");
    let sent_at = Instant::now() - Duration::from_secs(60);
    crate::fixtures::circuit::send_prepared_at(&mut circuit, &client, prepared, sent_at).await.expect("send original reliable packet");
    let original = receive(&simulator).await.0;
    let mut now = sent_at + Duration::from_secs(30);

    for _ in 1..=MAX_RETRIES {
        let prepared = circuit.prepare_retransmission(now).expect("prepare retry").expect("packet should retry before limit");
        crate::fixtures::circuit::send_prepared_at(&mut circuit, &client, prepared, now).await.expect("send retry");
        let (resent, _) = receive(&simulator).await;
        let mut expected = original.clone();
        expected[0] |= RESENT_FLAG;
        assert_eq!(resent, expected);
        now += Duration::from_secs(30);
    }

    std::assert_matches!(circuit.prepare_retransmission(now).map(|_| ()), Err(CircuitError::RetryLimit));
}

#[tokio::test]
async fn lost_packet_retries() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, commands.clone(), command_receiver, event_sender, disconnect_requested));

    let (original, peer) = receive(&simulator).await;
    assert_eq!(original, expected_fixture("use_circuit_code.hex", 1, true));

    let (resent, resent_peer) = receive_with_timeout(&simulator, RETRANSMISSION_TIMEOUT).await;
    let mut expected_resent = original;
    expected_resent[0] |= RESENT_FLAG;
    assert_eq!(resent_peer, peer);
    assert_eq!(resent, expected_resent);

    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack retransmitted circuit packet");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));

    disconnect_signal.send_replace(true);
    let logout = loop {
        let (bytes, _) = receive(&simulator).await;
        let packet = Packet::decode(&bytes).expect("decode disconnect packet");
        if let Message::StartPingCheck(_) = packet.message() {
            assert_eq!(packet.sequence(), 3);
            assert!(!packet.is_reliable());
            continue;
        }
        break packet;
    };
    assert!((3..=4).contains(&logout.sequence()));
    assert!(logout.is_reliable());
    assert_eq!(logout.message(), &logout_message());
    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
}

#[tokio::test]
async fn overflow_keeps_sequence() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    for expected_sequence in 1..=MAX_PENDING_PACKETS as u32 {
        let sequence = crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(3), Delivery::Reliable)
            .await
            .expect("packet should fit in the reliable window");
        assert_eq!(sequence, expected_sequence);
    }
    assert!(crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(4), Delivery::Reliable).await.is_err());

    for expected_sequence in 1..=MAX_PENDING_PACKETS as u32 {
        let (bytes, _) = receive(&simulator).await;
        assert_eq!(Packet::decode(&bytes).expect("decode queued packet").sequence(), expected_sequence);
    }
    assert_no_datagram(&simulator);

    let ack = Packet::decode(&server_ack_many(50, &[1])).expect("decode fake simulator ACK");
    assert!(receive_packet(&mut circuit, &ack));
    assert_eq!(
        crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(5), Delivery::Reliable)
            .await
            .expect("ACK should free one slot"),
        MAX_PENDING_PACKETS as u32 + 1
    );
    let (bytes, _) = receive(&simulator).await;
    assert_eq!(Packet::decode(&bytes).expect("decode packet after ACK").sequence(), MAX_PENDING_PACKETS as u32 + 1);
}

#[tokio::test]
async fn packet_piggybacks_ack() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut incoming = test_fixture("chat_from_simulator.hex");
    incoming[0] |= 0x40;
    set_sequence(&mut incoming, 55);
    let incoming = Packet::decode(&incoming).expect("decode reliable simulator packet");
    assert!(receive_packet(&mut circuit, &incoming));

    crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(3), Delivery::Unreliable)
        .await
        .expect("send next outbound packet");
    let (bytes, _) = receive(&simulator).await;
    let packet = Packet::decode(&bytes).expect("decode client packet");

    assert_eq!(packet.message(), &complete_ping(3));
    assert_eq!(packet.appended_acks(), &[55]);
}

#[tokio::test]
async fn timer_acks_recover_loss() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, commands.clone(), command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack_many(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack_many(21, &[2]), peer).await.expect("ack complete movement");

    for sequence in [55, 56] {
        let mut incoming = test_fixture("chat_from_simulator.hex");
        incoming[0] |= 0x40;
        set_sequence(&mut incoming, sequence);
        simulator.send_to(&incoming, peer).await.expect("send reliable simulator chat");
    }

    let mut acknowledged = Vec::<u32>::new();
    let mut next_sequence = 3;
    while acknowledged.len() < 2 {
        let (bytes, _) = timeout(SIMULATOR_TIMEOUT, receive(&simulator)).await.expect("timer should send queued acknowledgements");
        let packet = Packet::decode(&bytes).expect("decode timer ACK packet");
        assert_eq!(packet.sequence(), next_sequence);
        let Message::PacketAck(message) = packet.message() else {
            panic!("expected timer to send a standalone ACK packet");
        };
        let packet_ids = message.packets.iter().map(|packet| packet.id).collect::<Vec<_>>();
        assert_eq!(bytes, server_ack_many(next_sequence, &packet_ids));
        acknowledged.extend(packet_ids);
        next_sequence += 1;
    }
    acknowledged.sort_unstable();
    assert_eq!(acknowledged, [55, 56]);

    disconnect_signal.send_replace(true);
    assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", next_sequence, true));
    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
}

#[tokio::test]
async fn encode_failure_keeps_ack() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut incoming = test_fixture("chat_from_simulator.hex");
    incoming[0] |= 0x40;
    set_sequence(&mut incoming, 55);
    let incoming = Packet::decode(&incoming).expect("decode reliable simulator packet");
    assert!(receive_packet(&mut circuit, &incoming));

    let Err(error) = circuit.prepare_message(chat_message("x".repeat(65_520)), Delivery::Unreliable) else {
        panic!("oversized message should fail during preparation");
    };
    std::assert_matches!(error, CircuitError::EncodeMessage(_));
    crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(3), Delivery::Unreliable)
        .await
        .expect("send packet after failed encode");

    let (bytes, _) = receive(&simulator).await;
    let packet = Packet::decode(&bytes).expect("decode client packet");
    assert_eq!(packet.sequence(), 1);
    assert_eq!(packet.appended_acks(), &[55]);
}

#[tokio::test]
async fn duplicate_requeues_ack() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut incoming = test_fixture("chat_from_simulator.hex");
    incoming[0] |= 0x40;
    set_sequence(&mut incoming, 55);
    let packet = Packet::decode(&incoming).expect("decode reliable simulator packet");
    assert!(receive_packet(&mut circuit, &packet));
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &client).await.expect("flush first acknowledgement");
    assert_eq!(Packet::decode(&receive(&simulator).await.0).expect("decode first ACK").message(), &packet_ack(&[55]));

    incoming[0] |= RESENT_FLAG;
    let duplicate = Packet::decode(&incoming).expect("decode resent simulator packet");
    assert!(!receive_packet(&mut circuit, &duplicate));
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &client).await.expect("flush duplicate acknowledgement");
    assert_eq!(Packet::decode(&receive(&simulator).await.0).expect("decode duplicate ACK").message(), &packet_ack(&[55]));
}

#[tokio::test]
async fn full_ack_queue_defers_send() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut sequence = 1;
    loop {
        let packet = Packet::new(sequence, complete_ping(3)).with_reliable();
        match crate::fixtures::circuit::receive_packet_at(&mut circuit, &packet, Instant::now()) {
            Ok(outcome) => assert!(outcome.is_new_packet),
            Err(ReceiveError::AcknowledgementCapacityFull) => break,
            Err(error) => panic!("unexpected receive rejection: {error}"),
        }
        sequence += 1;
    }
    let overflow = Packet::new(sequence, complete_ping(3)).with_reliable();
    assert_eq!(
        crate::fixtures::circuit::receive_packet_at(&mut circuit, &overflow, Instant::now()),
        Err(ReceiveError::AcknowledgementCapacityFull)
    );

    crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(4), Delivery::Unreliable)
        .await
        .expect("send packet to make ACK queue space");
    let (bytes, _) = receive(&simulator).await;
    let packet = Packet::decode(&bytes).expect("decode client packet");
    assert_eq!(packet.appended_acks().len(), u8::MAX as usize);
    assert!(receive_packet(&mut circuit, &overflow), "unacknowledged input should be accepted after queue space opens");
}

#[tokio::test]
async fn late_ack_clears_pending() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();
    crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(1), Delivery::Reliable)
        .await
        .expect("send first reliable packet");
    crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(2), Delivery::Reliable)
        .await
        .expect("send second reliable packet");
    let ack = Packet::new(70, packet_ack(&[1, 2]));
    crate::fixtures::circuit::receive_packet_at(&mut circuit, &ack, Instant::now()).expect("receive packet ACK");
    assert!(!circuit.has_pending(1));
    assert!(!circuit.has_pending(2));

    let mut circuit = Circuit::new();
    crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(1), Delivery::Reliable)
        .await
        .expect("send first reliable packet");
    crate::fixtures::circuit::send_message(&mut circuit, &client, complete_ping(2), Delivery::Reliable)
        .await
        .expect("send second reliable packet");
    let appended_ack = Packet::new(71, complete_ping(3)).with_appended_acks(vec![1, 2]);
    crate::fixtures::circuit::receive_packet_at(&mut circuit, &appended_ack, Instant::now()).expect("receive appended ACKs");
    assert!(!circuit.has_pending(1));
    assert!(!circuit.has_pending(2));
}
