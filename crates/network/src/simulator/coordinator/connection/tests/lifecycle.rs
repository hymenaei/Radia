use super::*;
use crate::simulator::coordinator::tests::{connect_test_connection, connect_test_primary_with_state};

#[tokio::test]
async fn primary_sends_agent_throttle_after_matching_movement_complete() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let mut agent_session = session(simulator_address);
    agent_session.agent_id = Uuid::from_bytes([1; 16]);
    agent_session.session_id = Uuid::from_bytes([2; 16]);
    agent_session.circuit_code = 0x1234_5678;
    let budget = OutboundBudget::new(crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
    let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::Connecting);
    let connection = connect_test_primary_with_state(
        simulator_id,
        agent_session.clone(),
        CoverageLossLedger::new(),
        primary_state_receiver,
        budget.handle(),
        None,
    )
    .await
    .expect("connect primary");
    let (events, mut received_events) = mpsc::channel(8);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (requests, _requests) = mpsc::channel(1);
    let (_commands, command_receiver) = mpsc::channel(1);
    let (_logout, logout_receiver) = mpsc::channel(1);
    let worker =
        spawn(connection.run(events, shutdown_receiver, requests, command_receiver, logout_receiver, SimulatorMemoryBudget::new()));

    let (use_circuit, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit.sequence()).await;
    receive_connected(&mut received_events).await;
    let (movement_request, _) = receive_packet(&simulator).await;
    assert!(matches!(movement_request.message(), Message::CompleteAgentMovement(_)));
    send_ack(&simulator, peer, movement_request.sequence()).await;

    let movement_sequence = 0x0102_0304;
    let movement = AgentMovementComplete {
        agent_data: agent_movement_complete::AgentData { agent_id: agent_session.agent_id, session_id: agent_session.session_id },
        data: agent_movement_complete::Data { region_handle: 42, ..Default::default() },
        sim_data: agent_movement_complete::SimData::default(),
    };
    let bytes = Packet::new(movement_sequence, Message::AgentMovementComplete(movement))
        .with_reliable()
        .encode()
        .expect("encode movement completion");
    simulator.send_to(&bytes, peer).await.expect("send movement completion");
    assert!(matches!(
        receive_message_event(&mut received_events, movement_sequence).await,
        Event::SimulatorMessage { message: Message::AgentMovementComplete(_), .. }
    ));

    let (throttle, _) = receive_packet(&simulator).await;
    assert!(throttle.is_reliable(), "AgentThrottle must use LLUDP reliable delivery");
    let Message::AgentThrottle(throttle_message) = throttle.message() else { panic!("expected AgentThrottle") };
    assert_eq!(throttle_message.agent_data.agent_id, agent_session.agent_id);
    assert_eq!(throttle_message.agent_data.session_id, agent_session.session_id);
    assert_eq!(throttle_message.agent_data.circuit_code, agent_session.circuit_code);
    assert_eq!(throttle_message.throttle.gen_counter, 0);
    assert_eq!(throttle_message.throttle.throttles, DEFAULT_AGENT_THROTTLE_RATES);

    shutdown.send_replace(true);
    assert!(timeout(WAIT, worker).await.expect("join connection worker").expect("connection worker").is_ok());
    drop(primary_state);
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn demoted_primary_times_out() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let simulator_id = SimulatorId::allocate().expect("simulator ID");
    let budget = OutboundBudget::new(crate::session::DEFAULT_UPSTREAM_BYTES_PER_SECOND.get());
    let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::Connecting);
    let connection = connect_test_primary_with_state(
        simulator_id,
        session(simulator.local_addr().expect("simulator address")),
        CoverageLossLedger::new(),
        primary_state_receiver,
        budget.handle(),
        None,
    )
    .await
    .expect("connect primary");
    let (events, mut received) = mpsc::channel(4);
    let (_shutdown, shutdown) = watch::channel(false);
    let (requests, _requests) = mpsc::channel(1);
    let (commands, command_receiver) = mpsc::channel(2);
    commands.try_send(SimulatorCommand::CancelHandoff).expect("fill first command slot");
    commands.try_send(SimulatorCommand::CancelHandoff).expect("fill second command slot");
    let (_logout, logout) = mpsc::channel(1);
    primary_state.send_replace(PrimaryState::Neighbor { generation: 1 });
    drop(primary_state);
    let worker = spawn(connection.run(events, shutdown, requests, command_receiver, logout, SimulatorMemoryBudget::new()));

    let (circuit, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, circuit.sequence()).await;
    receive_connected(&mut received).await;
    let (movement, _) = receive_packet(&simulator).await;
    assert!(matches!(movement.message(), Message::CompleteAgentMovement(_)));
    send_ack(&simulator, peer, movement.sequence()).await;

    let (completed, result) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 9 } }),
            delivery: Delivery::Unreliable,
            completed: Some(completed),
            cancel_target: None,
            report_failure: false,
        })
        .await
        .expect("queue command after demotion");
    assert!(result.await.expect("demotion barrier").is_ok());
    let _ = receive_packet(&simulator).await;
    let ping = parse_hex_fixture(vectors::START_PING_CHECK).expect("ping fixture");
    simulator.send_to(&ping, peer).await.expect("send activity barrier");
    let (reply, _) = receive_packet(&simulator).await;
    assert!(matches!(reply.message(), Message::CompletePingCheck(_)));

    advance(KEEPALIVE_TIMEOUT).await;
    let started = tokio::time::Instant::now();
    let result = worker.await.expect("connection worker");
    assert!(started.elapsed() <= WAIT, "demoted connection expires");
    std::assert_matches!(result, Err(ConnectionError::KeepaliveTimeout));
}

#[tokio::test(flavor = "multi_thread")]
async fn promoted_logout_bypasses_full_queue() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let simulator_address = simulator.local_addr().expect("simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let agent_session = session(simulator_address);
    let agent_id = agent_session.agent_id;
    let session_id = agent_session.session_id;
    let budget = OutboundBudget::new(1);
    let (primary_state, primary_state_receiver) = watch::channel(PrimaryState::Neighbor { generation: 0 });
    let promote_connection = primary_state.clone();
    let mut connection = connect_test_connection(
        simulator_id,
        agent_session,
        Some(42),
        CoverageLossLedger::new(),
        Some(primary_state_receiver),
        Some(budget.handle()),
        None,
        None,
    )
    .await
    .expect("connect primary");
    let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
    Arc::get_mut(&mut connection.socket).expect("test connection owns its socket").set_send_gate(send_gate);
    let (events, mut received_events) = mpsc::channel(16);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (requests, _requests) = mpsc::channel(1);
    let (commands, command_receiver) = mpsc::channel(16);
    let (logout_commands, logout_receiver) = mpsc::channel(1);
    let worker =
        spawn(connection.run(events, shutdown_receiver, requests, command_receiver, logout_receiver, SimulatorMemoryBudget::new()));

    let (use_circuit, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit.sequence()).await;
    receive_connected(&mut received_events).await;
    promote_connection.send_replace(PrimaryState::Ready { generation: 0 });

    let (ready_completed, ready_response) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 8 } }),
            delivery: Delivery::Unreliable,
            completed: Some(ready_completed),
            cancel_target: None,
            report_failure: false,
        })
        .await
        .expect("queue a send after promotion");
    assert!(ready_response.await.expect("promoted send completion").is_ok());
    let (promoted_send, _) = receive_packet(&simulator).await;
    assert!(matches!(promoted_send.message(), Message::CompletePingCheck(_)));

    let make_chat = |message: String| {
        Message::ChatFromViewer(ChatFromViewer {
            agent_data: chat_from_viewer::AgentData { agent_id, session_id },
            chat_data: chat_from_viewer::ChatData { message, chat_type: ChatType::Normal, channel: 0 },
        })
    };
    let first = make_chat("x".repeat(7_900));
    let wire_bytes = Packet::new(100, first.clone()).encode().expect("encode burst-filling chat");
    assert!(wire_bytes.len() < 8 * 1024 && wire_bytes.len() > 7_500, "fixture should consume most of the initial pacing burst");
    commands
        .send(SimulatorCommand::Send {
            message: first,
            delivery: Delivery::Unreliable,
            completed: None,
            cancel_target: None,
            report_failure: false,
        })
        .await
        .expect("queue first chat");
    let (first_sent, _) = receive_packet(&simulator).await;
    assert!(matches!(first_sent.message(), Message::ChatFromViewer(_)));

    let (pending_completed, pending_response) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: make_chat("y".repeat(7_900)),
            delivery: Delivery::Unreliable,
            completed: Some(pending_completed),
            cancel_target: None,
            report_failure: false,
        })
        .await
        .expect("queue rate-limited chat");
    for _ in 0..MAX_PENDING_SENDS {
        commands
            .send(SimulatorCommand::Send {
                message: make_chat("queued".into()),
                delivery: Delivery::Unreliable,
                completed: None,
                cancel_target: None,
                report_failure: false,
            })
            .await
            .expect("queue ordinary chat");
    }
    let (full_completed, full_response) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: make_chat("full".into()),
            delivery: Delivery::Unreliable,
            completed: Some(full_completed),
            cancel_target: None,
            report_failure: false,
        })
        .await
        .expect("send command to full outbound queue");
    assert!(matches!(
        timeout(WAIT, full_response).await.expect("full queue response").expect("completion sender"),
        Err(Error::SimulatorCommandQueueFull)
    ));

    pause_sends.send_replace(true);
    let (logout_completed, logout_response) = oneshot::channel();
    logout_commands.send(logout_completed).await.expect("request priority logout");
    timeout(WAIT, send_entered.notified()).await.expect("logout should reach the paused local send");

    let (repeated_logout_completed, mut repeated_logout_response) = oneshot::channel();
    logout_commands.send(repeated_logout_completed).await.expect("repeat priority logout");
    assert!(timeout(Duration::from_millis(25), &mut repeated_logout_response).await.is_err());

    pause_sends.send_replace(false);
    let (logout_packet, _) = receive_packet(&simulator).await;
    assert!(matches!(logout_packet.message(), Message::LogoutRequest(_)));
    assert!(logout_response.await.expect("logout completion").is_ok(), "logout local send completes despite a full ordinary queue");
    assert!(repeated_logout_response.await.expect("repeated logout completion").is_ok());
    assert!(matches!(pending_response.await, Ok(Err(Error::SimulatorConnectionClosed))));

    shutdown.send_replace(true);
    assert!(timeout(WAIT, worker).await.expect("stop connection worker").expect("join connection worker").is_ok());
    budget.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn saturated_neighbor_events_preserve_ack_and_generation_completion() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let simulator_address = simulator.local_addr().expect("simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, CoverageLossLedger::new())
        .await
        .expect("connect neighbor simulator");
    let (events, mut received_events) = mpsc::channel(1);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (commands, command_receiver) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel(1);
    let task = spawn(connection.run(
        events.clone(),
        shutdown_receiver,
        neighbor_requests,
        command_receiver,
        logout,
        SimulatorMemoryBudget::new().for_neighbor(),
    ));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit_code.sequence()).await;
    receive_connected(&mut received_events).await;

    let (completed, response) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 12 } }),
            delivery: Delivery::Unreliable,
            completed: Some(completed),
            cancel_target: None,
            report_failure: true,
        })
        .await
        .expect("queue send that reports failures only");
    let (sent, _) = receive_packet(&simulator).await;
    assert!(matches!(sent.message(), Message::CompletePingCheck(_)));
    assert!(response.await.expect("local send result").is_ok());
    assert!(
        matches!(received_events.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "a successful send without a generation target must not emit a completion event"
    );

    events
        .try_send(SimulatorEvent::SendCompleted { simulator_id, cancel_target: None, error: None, report_failure: false })
        .expect("fill the neighbor event channel");
    let (promotion, cancel_target) = handoff_cancellation_for_test(simulator_id, simulator_address);
    let handoff_state = HandoffState::Preparing(promotion);
    let (completed, response) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 13 } }),
            delivery: Delivery::Unreliable,
            completed: Some(completed),
            cancel_target: Some(cancel_target),
            report_failure: true,
        })
        .await
        .expect("queue reportable handoff control message");
    let (sent, _) = receive_packet(&simulator).await;
    assert!(matches!(sent.message(), Message::CompletePingCheck(_)));
    assert!(response.await.expect("local send result").is_ok());

    let reliable_sequence = 40_001_u32;
    let mut reliable_chat = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse chat fixture");
    reliable_chat[0] |= 0x40;
    reliable_chat[1..5].copy_from_slice(&reliable_sequence.to_be_bytes());
    simulator.send_to(&reliable_chat, peer).await.expect("send reliable simulator event");
    for _ in 0..16 {
        yield_now().await;
    }
    advance(ACK_INTERVAL).await;
    receive_ack(&simulator, reliable_sequence).await;

    assert!(matches!(received_events.try_recv(), Ok(SimulatorEvent::SendCompleted { .. })), "remove the channel filler");
    loop {
        if let Some(SimulatorEvent::SendCompleted { simulator_id: completed_id, cancel_target: Some(actual), error, report_failure }) =
            received_events.recv().await
        {
            assert_eq!(completed_id, simulator_id);
            assert!(error.is_none(), "successful local send has no error");
            assert!(!report_failure, "successful send completion does not report a failure");
            assert!(crate::simulator::coordinator::handoff::matches_handoff_cancellation(&handoff_state, actual));
            break;
        }
    }

    shutdown.send_replace(true);
    assert!(task.await.expect("connection task").is_ok());
}

#[tokio::test(start_paused = true)]
async fn saturated_event_channel_delivers_expired_send_failure() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let simulator_address = simulator.local_addr().expect("simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let mut connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, CoverageLossLedger::new())
        .await
        .expect("connect neighbor simulator");
    let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
    Arc::get_mut(&mut connection.socket).expect("connection owns socket").set_send_gate(send_gate);
    let (events, mut received_events) = mpsc::channel(1);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (commands, command_receiver) = mpsc::channel(1);
    let (_logout, logout) = mpsc::channel(1);
    let task = spawn(connection.run(
        events.clone(),
        shutdown_receiver,
        neighbor_requests,
        command_receiver,
        logout,
        SimulatorMemoryBudget::new().for_neighbor(),
    ));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit_code.sequence()).await;
    receive_connected(&mut received_events).await;
    events
        .try_send(SimulatorEvent::SendCompleted { simulator_id, cancel_target: None, error: None, report_failure: false })
        .expect("fill the neighbor event channel");

    let (promotion, cancel_target) = handoff_cancellation_for_test(simulator_id, simulator_address);
    let handoff_state = HandoffState::Preparing(promotion);
    pause_sends.send_replace(true);
    let (completed, response) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 14 } }),
            delivery: Delivery::Unreliable,
            completed: Some(completed),
            cancel_target: Some(cancel_target),
            report_failure: true,
        })
        .await
        .expect("queue reportable send");
    timeout(WAIT, send_entered.notified()).await.expect("send should enter the gate");
    advance(MAX_SEND_AGE).await;
    assert!(matches!(response.await.expect("expired local send result"), Err(Error::SimulatorSend(_))));

    pause_sends.send_replace(false);
    let reliable_sequence = 40_002_u32;
    let mut reliable_chat = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse chat fixture");
    reliable_chat[0] |= 0x40;
    reliable_chat[1..5].copy_from_slice(&reliable_sequence.to_be_bytes());
    simulator.send_to(&reliable_chat, peer).await.expect("send reliable simulator event while notification is buffered");
    for _ in 0..16 {
        yield_now().await;
    }
    advance(ACK_INTERVAL).await;
    receive_ack(&simulator, reliable_sequence).await;

    assert!(matches!(received_events.try_recv(), Ok(SimulatorEvent::SendCompleted { .. })), "remove the channel filler");
    loop {
        if let Some(SimulatorEvent::SendCompleted { simulator_id: completed_id, cancel_target: Some(actual), error, report_failure }) =
            received_events.recv().await
        {
            assert_eq!(completed_id, simulator_id);
            assert!(error.is_some(), "expired send reports its failure");
            assert!(report_failure);
            assert!(crate::simulator::coordinator::handoff::matches_handoff_cancellation(&handoff_state, actual));
            break;
        }
    }

    shutdown.send_replace(true);
    assert!(task.await.expect("connection task").is_ok());
}

#[tokio::test]
async fn connection_survives_send_failures() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator");
    let simulator_address = simulator.local_addr().expect("simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate simulator ID");
    let mut connection = connect_test_neighbor_with_coverage(simulator_id, session(simulator_address), 42, CoverageLossLedger::new())
        .await
        .expect("connect neighbor simulator");
    let (send_gate, pause_sends, send_entered) = UdpTransport::new_send_gate();
    Arc::get_mut(&mut connection.socket).expect("connection owns socket").set_send_gate(send_gate);
    let (events, mut received_events) = mpsc::channel(1);
    let (shutdown, shutdown_receiver) = watch::channel(false);
    let (neighbor_requests, _neighbor_requests_receiver) = mpsc::channel(1);
    let (commands, command_receiver) = mpsc::channel(MAX_PENDING_SENDS + MAX_PENDING_SEND_NOTIFICATIONS + 4);
    let (_logout, logout) = mpsc::channel(1);
    let task = spawn(connection.run(
        events.clone(),
        shutdown_receiver,
        neighbor_requests,
        command_receiver,
        logout,
        SimulatorMemoryBudget::new().for_neighbor(),
    ));

    let (use_circuit_code, peer) = receive_packet(&simulator).await;
    send_ack(&simulator, peer, use_circuit_code.sequence()).await;
    receive_connected(&mut received_events).await;
    events
        .try_send(SimulatorEvent::SendCompleted { simulator_id, cancel_target: None, error: None, report_failure: false })
        .expect("fill the neighbor event channel");
    drop(events);

    pause_sends.send_replace(true);
    commands
        .send(SimulatorCommand::Send {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 14 } }),
            delivery: Delivery::Unreliable,
            completed: None,
            cancel_target: None,
            report_failure: true,
        })
        .await
        .expect("queue the send held at the transport");
    timeout(WAIT, send_entered.notified()).await.expect("send should enter the gate");

    let failed_sends = MAX_PENDING_SEND_NOTIFICATIONS + 1;
    let queued_sends = MAX_PENDING_SENDS - 1 + failed_sends;
    let (first_completed, first_failure) = oneshot::channel();
    let mut first_completed = Some(first_completed);
    let (completed, completion) = oneshot::channel();
    let mut final_completion = Some(completed);
    for index in 0..queued_sends {
        commands
            .send(SimulatorCommand::Send {
                message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 14 } }),
                delivery: Delivery::Unreliable,
                completed: if index == MAX_PENDING_SENDS - 1 {
                    first_completed.take()
                } else if index + 1 == queued_sends {
                    final_completion.take()
                } else {
                    None
                },
                cancel_target: None,
                report_failure: true,
            })
            .await
            .expect("queue send behind the held transport operation");
    }

    assert!(matches!(first_failure.await.expect("first failed send completion"), Err(Error::SimulatorCommandQueueFull)));
    assert!(matches!(received_events.try_recv(), Ok(SimulatorEvent::SendCompleted { .. })), "remove the channel filler");
    for _ in 0..failed_sends {
        let event = timeout(WAIT, received_events.recv()).await.expect("send failure notification is delivered");
        assert!(matches!(
            event,
            Some(SimulatorEvent::SendCompleted { simulator_id: actual, error: Some(_), report_failure: true, .. }) if actual == simulator_id
        ));
    }
    assert!(matches!(completion.await.expect("final send completion"), Err(Error::SimulatorCommandQueueFull)));

    pause_sends.send_replace(false);
    for _ in 0..MAX_PENDING_SENDS {
        let (packet, _) = receive_packet(&simulator).await;
        assert!(matches!(packet.message(), Message::CompletePingCheck(_)));
    }
    let (completed, completion) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 15 } }),
            delivery: Delivery::Unreliable,
            completed: Some(completed),
            cancel_target: None,
            report_failure: false,
        })
        .await
        .expect("queue a send after the backlog drains");
    assert!(completion.await.expect("send completion").is_ok());
    let (packet, _) = receive_packet(&simulator).await;
    assert!(matches!(packet.message(), Message::CompletePingCheck(_)));

    shutdown.send_replace(true);
    assert!(timeout(WAIT, task).await.expect("join connection task").expect("connection task").is_ok());
}
