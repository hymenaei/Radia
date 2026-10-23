use std::time::Instant;

use super::*;

#[tokio::test]
async fn handoff_waits_for_matching_movement() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination simulator");
    let endpoint = neighbor.local_addr().expect("destination simulator address");
    let session = login_session(endpoint);
    let region_handle = 0x1000_0000_0000_1000;
    let simulator_id = SimulatorId::allocate().expect("allocate destination ID");
    let (event_sender, mut event_receiver) = mpsc::channel(8);
    let (shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let mut connection = connect_test_neighbor(simulator_id, session.clone(), region_handle, NetworkStatusHandle::default())
        .await
        .expect("connect destination circuit");
    connection.handoff = Some(HandoffTarget {
        agent_id: session.agent_id,
        session_id: session.session_id,
        region_handle,
        endpoint,
        seed_capability: session.seed_capability.clone(),
    });
    let task = spawn(connection.run(event_sender, shutdown, neighbor_requests, commands, mpsc::channel(1).1, SimulatorMemoryBudget::new()));

    let (use_circuit, peer) = receive(&neighbor).await;
    assert!(matches!(Packet::decode(&use_circuit).expect("decode circuit packet").message(), Message::UseCircuitCode(_)));
    neighbor.send_to(&ack(40, 1), peer).await.expect("ack destination circuit");
    assert!(matches!(
        next_neighbor_event(&mut event_receiver).await,
        Event::SimulatorConnected { simulator_id: id, region_handle: Some(handle), .. } if id == simulator_id && handle == region_handle
    ));
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), neighbor.recv_from(&mut buffer)).await.is_err());

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 41);
    neighbor.send_to(&handshake, peer).await.expect("send destination region handshake");
    assert!(matches!(
        receive_message(&neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await.message(),
        Message::RegionHandshakeReply(_)
    ));
    assert!(matches!(
        receive_message(&neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await.message(),
        Message::CompleteAgentMovement(_)
    ));
    assert!(matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorMessage { message: Message::RegionHandshake(_), .. }));

    let wrong = movement_complete_packet(42, session.agent_id, Uuid::from_u128(99), region_handle);
    neighbor.send_to(&wrong, peer).await.expect("send wrong-session movement completion");
    let matching = movement_complete_packet(43, session.agent_id, session.session_id, region_handle);
    neighbor.send_to(&matching, peer).await.expect("send matching movement completion");
    let Event::SimulatorMessage { simulator_id: emitted_simulator, sequence, message: Message::AgentMovementComplete(movement), .. } =
        next_neighbor_event(&mut event_receiver).await
    else {
        panic!("the matching movement completion should be the next simulator event");
    };
    assert_eq!(emitted_simulator, simulator_id);
    assert_eq!(sequence, 43, "the wrong-session packet must not be forwarded first");
    assert_eq!(movement.agent_data.agent_id, session.agent_id);
    assert_eq!(movement.agent_data.session_id, session.session_id);
    assert_eq!(movement.data.region_handle, region_handle);

    shutdown_signal.send_replace(true);
    assert!(timeout(Duration::from_millis(200), task).await.expect("destination stops").expect("destination task").is_ok());
}

#[test]
fn teleport_finish_promotes_after_matching_movement() {
    run_large_stack_test(run_teleport_finish_test);
}

#[test]
#[ignore = "manual loaded loopback handoff latency measurement; run with --release --ignored --nocapture"]
fn handoff_send_latency() {
    run_large_stack_test(run_handoff_send_latency_test);
}

async fn run_teleport_finish_test() {
    let _ = run_teleport_finish_scenario(1).await;
}

#[test]
fn overload_logout_keeps_a_pending_promotion_on_the_original_primary() {
    run_large_stack_test(run_overload_logout_promotion_test);
}

async fn run_overload_logout_promotion_test() {
    use crate::event::Event::{Failed, SimulatorDisconnected, SimulatorMessage, SimulatorPromoted};
    use protocol::lludp::Message::RegionHandshake;

    let mut harness = connected_neighbor().await;
    let target_region = 0x1000_0000_0000_1000;
    let session = login_session(harness.primary.local_addr().expect("primary simulator address"));
    let finish = Packet::new(
        30,
        teleport_finish_to(
            session.agent_id,
            harness.neighbor.local_addr().expect("destination address"),
            target_region,
            session.seed_capability.as_str(),
        ),
    )
    .with_reliable()
    .encode()
    .expect("encode TeleportFinish");
    harness.primary.send_to(&finish, harness.primary_peer).await.expect("send TeleportFinish");
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, message: Message::TeleportFinish(_), .. } if simulator_id == harness.primary_id => {
                    break;
                }
                Failed(reason) => panic!("session failed before handoff setup: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("manager should process TeleportFinish");

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 41);
    harness.neighbor.send_to(&handshake, harness.neighbor_peer).await.expect("send destination handshake");
    let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let movement_request = receive_message(&harness.neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    harness.neighbor.send_to(&ack(42, reply.sequence()), harness.neighbor_peer).await.expect("ack destination handshake reply");
    harness.neighbor.send_to(&ack(43, movement_request.sequence()), harness.neighbor_peer).await.expect("ack destination movement request");
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, message: RegionHandshake(_), .. } if simulator_id == harness.neighbor_id => break,
                Failed(reason) => panic!("session failed during destination handshake: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("manager should process destination handshake");

    let mut observed_logout = None;
    for sequence in 60..60 + (TEST_EVENT_CAPACITY + MAX_PENDING_SIMULATOR_EVENTS) as u32 {
        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        harness.primary.send_to(&chat, harness.primary_peer).await.expect("fill application event backlog");
        let response = receive_ack(&harness.primary, sequence).await;
        if matches!(response.message(), Message::LogoutRequest(_)) {
            observed_logout = Some(response);
            break;
        }
    }
    let logout = match observed_logout {
        Some(logout) => logout,
        None => timeout(WAIT, async {
            loop {
                let (bytes, _) = receive(&harness.primary).await;
                let packet = Packet::decode(&bytes).expect("decode primary output");
                if matches!(packet.message(), Message::LogoutRequest(_)) {
                    break packet;
                }
            }
        })
        .await
        .expect("backlog should start logout on the current primary"),
    };

    harness
        .neighbor
        .send_to(&movement_complete_packet(44, session.agent_id, session.session_id, target_region), harness.neighbor_peer)
        .await
        .expect("send destination movement completion while logout is pending");
    receive_ack(&harness.neighbor, 44).await;
    let mut barrier = fixture("chat_from_simulator.hex");
    barrier[0] |= 0x40;
    set_sequence(&mut barrier, 45);
    harness.neighbor.send_to(&barrier, harness.neighbor_peer).await.expect("send post-movement barrier");
    receive_ack(&harness.neighbor, 45).await;

    let mut disable = fixture("disable_simulator.hex");
    set_sequence(&mut disable, 46);
    harness.neighbor.send_to(&disable, harness.neighbor_peer).await.expect("close destination after movement barrier");
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorDisconnected { simulator_id, region_handle: Some(region_handle), .. }
                    if simulator_id == harness.neighbor_id && region_handle == target_region =>
                {
                    break;
                }
                SimulatorPromoted { .. } => panic!("logout must freeze pending promotion before destination disconnect"),
                Failed(reason) => panic!("destination disconnect failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("destination disconnect should drain accepted movement events");

    assert_eq!(
        harness.status.snapshot().simulator_endpoint,
        Some(harness.primary.local_addr().expect("primary endpoint")),
        "a promotion received after logout starts must not replace its logout target"
    );

    harness.primary.send_to(&ack(120, logout.sequence()), harness.primary_peer).await.expect("ack primary logout");
    let mut logout_reply = fixture("logout_reply.hex");
    logout_reply[0] |= 0x40;
    set_sequence(&mut logout_reply, 121);
    harness.primary.send_to(&logout_reply, harness.primary_peer).await.expect("reply to primary logout");
    let result = timeout(LOGOUT_TIMEOUT + WAIT, harness.client).await.expect("overload teardown completes").expect("coordinator task");
    assert!(result.is_err(), "application overload should remain a visible session failure");
}

async fn run_handoff_send_latency_test() {
    const REPETITIONS: usize = 8;
    let mut completion_latencies = Vec::with_capacity(REPETITIONS * (NEIGHBOR_COMMAND_CAPACITY - 1));
    for _ in 0..REPETITIONS {
        let completion = run_teleport_finish_scenario(NEIGHBOR_COMMAND_CAPACITY - 1).await;
        completion_latencies.extend(completion);
    }
    eprintln!(
        "repeated loaded handoff latency: repetitions={REPETITIONS} exact-send-samples={} enqueue-to-local-send-completion p50/p95/p99={:?}",
        completion_latencies.len(),
        duration_percentiles(&completion_latencies),
    );
}

async fn run_teleport_finish_scenario(send_count: usize) -> Vec<Duration> {
    use crate::event::Event::{Failed, SimulatorMessage, SimulatorPromoted};
    use protocol::lludp::Message::{AgentMovementComplete, ChatFromSimulator, RegionHandshake};

    let (promotion_entered, promotion_entered_rx) = oneshot::channel();
    let (promotion_release, promotion_release_rx) = oneshot::channel();
    let gate = CoordinatorGate { entered: promotion_entered, release: promotion_release_rx };
    let mut harness = Box::pin(connected_neighbor_with_promotion_gate(gate)).await;
    let target_region = 0x1000_0000_0000_1000;
    let target_endpoint = harness.neighbor.local_addr().expect("destination simulator address");
    let session = login_session(harness.primary.local_addr().expect("primary simulator address"));
    let global_available = harness.packet_memory.global_available_bytes();
    let neighbor_available = harness.packet_memory.neighbor_available_bytes();
    let mut primary_chat = fixture("chat_from_simulator.hex");
    primary_chat[0] |= 0x40;
    set_sequence(&mut primary_chat, 24);
    harness.primary.send_to(&primary_chat, harness.primary_peer).await.expect("send retained primary payload");
    receive_ack(&harness.primary, 24).await;
    let held_primary_event = timeout(WAIT, async {
        loop {
            let event = harness.events.recv().await.expect("event channel remains open");
            if matches!(
                &event.event,
                SimulatorMessage { simulator_id, sequence: 24, message: ChatFromSimulator(_), .. }
                    if *simulator_id == harness.primary_id
            ) {
                break event;
            }
        }
    })
    .await
    .expect("primary payload should be retained by the event consumer");
    let retained_primary_bytes = global_available - harness.packet_memory.global_available_bytes();
    assert!(retained_primary_bytes > 0, "decoded primary payload should hold an admission charge");
    assert_eq!(harness.packet_memory.neighbor_available_bytes(), neighbor_available);

    let teleport_finish =
        Packet::new(30, teleport_finish_to(session.agent_id, target_endpoint, target_region, session.seed_capability.as_str()))
            .with_reliable()
            .encode()
            .expect("encode TeleportFinish");
    harness.primary.send_to(&teleport_finish, harness.primary_peer).await.expect("send TeleportFinish");

    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, message: Message::TeleportFinish(_), .. } if simulator_id == harness.primary_id => {
                    break;
                }
                Failed(reason) => panic!("session failed before destination promotion: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("manager should process TeleportFinish");

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 41);
    harness.neighbor.send_to(&handshake, harness.neighbor_peer).await.expect("send destination handshake");
    let reply = receive_message(&harness.neighbor, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let movement_request = receive_message(&harness.neighbor, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    harness.neighbor.send_to(&ack(42, reply.sequence()), harness.neighbor_peer).await.expect("ack destination handshake reply");
    harness.neighbor.send_to(&ack(43, movement_request.sequence()), harness.neighbor_peer).await.expect("ack destination movement request");

    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, message: RegionHandshake(_), .. } if simulator_id == harness.neighbor_id => break,
                Failed(reason) => panic!("session failed during destination handshake: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("manager should observe destination handshake");

    let wrong_region = target_region + 1;
    harness
        .neighbor
        .send_to(&movement_complete_packet(44, session.agent_id, session.session_id, wrong_region), harness.neighbor_peer)
        .await
        .expect("send movement completion for another region");
    receive_ack(&harness.neighbor, 44).await;
    let mut barrier = fixture("chat_from_simulator.hex");
    barrier[0] |= 0x40;
    set_sequence(&mut barrier, 45);
    harness.neighbor.send_to(&barrier, harness.neighbor_peer).await.expect("send post-movement barrier");
    receive_ack(&harness.neighbor, 45).await;
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, message: ChatFromSimulator(_), .. } if simulator_id == harness.neighbor_id => break,
                SimulatorPromoted { .. } => panic!("a mismatched movement completion must not promote the destination"),
                Failed(reason) => panic!("session failed before the post-movement barrier: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("manager should process the post-movement barrier");
    assert_eq!(
        harness.status.snapshot().simulator_endpoint,
        Some(harness.primary.local_addr().expect("primary simulator address")),
        "a mismatched movement completion must leave the primary simulator unchanged"
    );

    harness.pause_sends.send_replace(true);
    let (completed, mut held_send) = oneshot::channel();
    let held_send_queued_at = Instant::now();
    harness
        .commands
        .send(Command::SendTo { simulator_id: harness.primary_id, message: object_request(), delivery: Delivery::Reliable, completed })
        .await
        .expect("dispatch explicit send before promotion");
    timeout(WAIT, harness.send_entered.notified()).await.expect("hold former primary local send");

    harness
        .neighbor
        .send_to(&movement_complete_packet(46, session.agent_id, session.session_id, target_region), harness.neighbor_peer)
        .await
        .expect("send matching movement completion");
    timeout(WAIT, promotion_entered_rx).await.expect("coordinator should reach the promotion boundary").expect("promotion gate");
    harness
        .commands
        .send(Command::SendMessage { message: object_request(), delivery: Delivery::Reliable })
        .await
        .expect("queue default send before the route changes");
    let control_send_count = if send_count > 1 { 2 } else { 0 };
    let mut handoff_sends = Vec::with_capacity(send_count);
    for index in 0..send_count {
        let (completed, completion) = oneshot::channel();
        let queued_at = Instant::now();
        let message = if index < control_send_count {
            Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: u8::try_from(index).expect("ping ID fits u8") } })
        } else {
            object_request()
        };
        harness
            .commands
            .send(Command::SendTo { simulator_id: harness.neighbor_id, message, delivery: Delivery::Unreliable, completed })
            .await
            .expect("queue exact send during promotion");
        handoff_sends.push((queued_at, completion));
    }
    let queued_commands = handoff_sends.len() + 1;
    let available_command_slots = harness.commands.capacity();
    if send_count > 1 {
        assert_eq!(queued_commands, NEIGHBOR_COMMAND_CAPACITY, "the loaded handoff should queue one bounded neighbor backlog");
        assert_eq!(
            available_command_slots,
            COMMAND_CAPACITY - queued_commands,
            "only the handoff workload should occupy the coordinator command channel"
        );
    }
    promotion_release.send(()).expect("resume coordinator promotion");

    let (previous, promoted, promoted_endpoint) = timeout(WAIT, async {
        let mut matching_movement_seen = false;
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, message: AgentMovementComplete(movement), .. }
                    if simulator_id == harness.neighbor_id && movement.data.region_handle == target_region =>
                {
                    matching_movement_seen = true;
                }
                SimulatorPromoted { previous_simulator_id, simulator_id, session, .. } => {
                    assert!(matching_movement_seen, "promotion must follow the matching movement event");
                    break (previous_simulator_id, simulator_id, session.simulator_address);
                }
                Failed(reason) => panic!("session failed before promotion: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("matching destination movement should promote the simulator");

    assert_eq!(previous, harness.primary_id);
    assert_eq!(promoted, harness.neighbor_id);
    assert_eq!(promoted_endpoint, target_endpoint);
    let mut completion_latencies = Vec::with_capacity(send_count);
    for (queued_at, completion) in handoff_sends {
        timeout(WAIT, completion).await.expect("handoff send reaches local socket").expect("send completion").expect("local send");
        completion_latencies.push(Instant::now().duration_since(queued_at));
    }
    let expected_fair_sends = queued_commands - control_send_count;
    let mut fair_sends_received = 0;
    let mut control_sends_received = 0;
    let mut throttle_sends_received = 0;
    let mut buffer = [0; 65_535];
    for _ in 0..queued_commands + 1 {
        let (size, _) = timeout(WAIT, harness.neighbor.recv_from(&mut buffer))
            .await
            .expect("every queued handoff send reaches the local simulator")
            .expect("receive queued handoff send");
        let packet = Packet::decode(&buffer[..size]).expect("decode queued handoff send");
        match packet.message() {
            Message::RequestMultipleObjects(_) => {
                fair_sends_received += 1;
                if packet.is_reliable() {
                    harness.neighbor.send_to(&ack(49, packet.sequence()), harness.neighbor_peer).await.expect("ack promoted default send");
                }
            }
            Message::CompletePingCheck(_) => control_sends_received += 1,
            Message::AgentThrottle(_) => {
                throttle_sends_received += 1;
                assert!(packet.is_reliable());
                harness.neighbor.send_to(&ack(50, packet.sequence()), harness.neighbor_peer).await.expect("ack destination AgentThrottle");
            }
            message => panic!("unexpected message in loaded handoff burst: {message:?}"),
        }
    }
    assert_eq!(fair_sends_received, expected_fair_sends, "all queued fair sends should arrive at the destination");
    assert_eq!(control_sends_received, control_send_count, "all queued control sends should arrive at the destination");
    assert_eq!(throttle_sends_received, 1, "the promoted connection refreshes its throttle once");
    if send_count > 1 {
        assert_eq!(completion_latencies.len(), send_count, "every queued handoff send should complete locally");
        eprintln!(
            "loaded handoff command burst: exact-sends={}, default-sends=1, total-queued-commands={queued_commands}, neighbor-command-capacity={}, coordinator-command-slots-before-release={available_command_slots}, delayed-former-primary-send=true",
            send_count, NEIGHBOR_COMMAND_CAPACITY,
        );
    }
    timeout(WAIT, async {
        while harness.packet_memory.neighbor_available_bytes() != neighbor_available - retained_primary_bytes {
            task::yield_now().await;
        }
    })
    .await
    .expect("former primary's retained bytes should move into the neighbor budget");
    assert_eq!(harness.packet_memory.global_available_bytes(), global_available - retained_primary_bytes);
    let status = harness.status.snapshot();
    assert_eq!(status.simulator_endpoint, Some(target_endpoint));
    assert!(status.circuit_established, "promoted circuit should remain established");
    assert!(matches!(held_send.try_recv(), Err(oneshot::error::TryRecvError::Empty)), "promotion must preserve the pending explicit send");
    harness.pause_sends.send_replace(false);
    let sent = receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert!(timeout(WAIT, held_send).await.expect("former primary send completes").expect("send reply").is_ok());
    if send_count > 1 {
        eprintln!(
            "exact send held on former primary across promotion: enqueue-to-local-send-completion={:?}",
            held_send_queued_at.elapsed()
        );
    }
    harness.primary.send_to(&ack(31, sent.sequence()), harness.primary_peer).await.expect("ack explicit former-primary send");
    assert!(
        timeout(
            Duration::from_millis(50),
            receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_)))
        )
        .await
        .is_err(),
        "the queued default send must not also reach the former primary"
    );

    let mut demoted_movement = movement_complete_packet(50, session.agent_id, session.session_id, wrong_region);
    demoted_movement[0] |= 0x40;
    harness.primary.send_to(&demoted_movement, harness.primary_peer).await.expect("send movement completion to demoted primary");
    receive_ack(&harness.primary, 50).await;
    let mut demoted_chat = fixture("chat_from_simulator.hex");
    demoted_chat[0] |= 0x40;
    set_sequence(&mut demoted_chat, 51);
    harness.primary.send_to(&demoted_chat, harness.primary_peer).await.expect("send barrier after demoted movement");
    receive_ack(&harness.primary, 51).await;
    let mut demoted_movement_was_published = false;
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, sequence: 50, message: AgentMovementComplete(_), .. }
                    if simulator_id == harness.primary_id =>
                {
                    demoted_movement_was_published = true
                }
                SimulatorMessage { simulator_id, sequence: 51, message: ChatFromSimulator(_), .. }
                    if simulator_id == harness.primary_id =>
                {
                    break;
                }
                Failed(reason) => panic!("demoted primary event routing failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("demoted primary chat should cross the event barrier");
    assert!(!demoted_movement_was_published, "demoted primary must filter movement completion outside its handoff");

    let mut disable = fixture("disable_simulator.hex");
    set_sequence(&mut disable, 47);
    harness.primary.send_to(&disable, harness.primary_peer).await.expect("disable former primary");
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                Event::SimulatorDisconnected { simulator_id, region_handle: None, .. } if simulator_id == harness.primary_id => break,
                Failed(reason) => panic!("former primary failure ended the promoted session: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("manager should isolate the former primary failure");
    assert_eq!(harness.packet_memory.global_available_bytes(), global_available - retained_primary_bytes);
    assert_eq!(harness.packet_memory.neighbor_available_bytes(), neighbor_available - retained_primary_bytes);
    let resource_return_started_at = Instant::now();
    drop(held_primary_event);
    assert_eq!(harness.packet_memory.global_available_bytes(), global_available);
    assert_eq!(harness.packet_memory.neighbor_available_bytes(), neighbor_available);
    eprintln!(
        "handoff retained-resource return: samples=1, returned-bytes={retained_primary_bytes}, global-and-neighbor-budgets-restored=true, observed-return-time={:?}",
        resource_return_started_at.elapsed()
    );

    let neighbor_budget = fill_neighbor_budget(&harness.packet_memory);
    let mut promoted_chat = fixture("chat_from_simulator.hex");
    promoted_chat[0] |= 0x40;
    set_sequence(&mut promoted_chat, 48);
    harness.neighbor.send_to(&promoted_chat, harness.neighbor_peer).await.expect("send promoted simulator payload under pressure");
    receive_ack(&harness.neighbor, 48).await;
    let promoted_event = timeout(WAIT, async {
        loop {
            let event = harness.events.recv().await.expect("event channel remains open");
            if matches!(
                &event.event,
                Event::SimulatorMessage { simulator_id, sequence: 48, .. } if *simulator_id == harness.neighbor_id
            ) {
                break event;
            }
        }
    })
    .await
    .expect("promoted simulator should publish its payload while neighbor memory is saturated");
    assert_eq!(harness.packet_memory.neighbor_available_bytes(), 0);
    assert!(harness.packet_memory.global_available_bytes() < MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    drop(promoted_event);
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES - MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
    drop(neighbor_budget);
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(harness.packet_memory.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);

    let status = harness.status.snapshot();
    assert_eq!(status.simulator_endpoint, Some(target_endpoint));
    assert!(status.circuit_established, "former primary failure must keep the destination circuit established");

    harness
        .neighbor
        .send_to(&movement_complete_packet(50, session.agent_id, session.session_id, wrong_region), harness.neighbor_peer)
        .await
        .expect("send unrelated movement completion to promoted primary");
    timeout(WAIT, async {
        loop {
            match next_raw_event(&mut harness.events).await {
                SimulatorMessage { simulator_id, sequence: 50, message: AgentMovementComplete(_), .. }
                    if simulator_id == harness.neighbor_id =>
                {
                    break;
                }
                Failed(reason) => panic!("promoted primary movement completion failed: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("promoted primary should publish movement completion outside its original handoff");

    harness
        .commands
        .send(Command::SendMessage { message: object_request(), delivery: Delivery::Reliable })
        .await
        .expect("send through promoted primary");
    let (_, route_peer) =
        receive_message_with_peer(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert_eq!(route_peer, harness.neighbor_peer);

    harness.disconnect.send_replace(true);
    let logout = receive_message(&harness.neighbor, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(matches!(logout.message(), Message::LogoutRequest(_)));
    assert!(timeout(WAIT, harness.client).await.expect("logout should finish").expect("manager task").is_ok());
    completion_latencies
}

#[test]
fn repeated_promotions() {
    run_large_stack_test(run_repeated_promotions);
}

async fn run_repeated_promotions() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let third = UdpSocket::bind("127.0.0.1:0").await.expect("bind third simulator");
    let session = login_session(harness.primary.local_addr().expect("primary address"));
    let sockets = [&harness.primary, &harness.neighbor, &third];
    let mut peers = vec![harness.primary_peer, harness.neighbor_peer];
    let mut ids = vec![harness.primary_id, harness.neighbor_id];

    for step in 0..2 {
        let target = step + 1;
        let endpoint = sockets[target].local_addr().expect("target address");
        let region = 0x1000_0000_0000_1000 + step as u64;
        let sequence = 100 + step as u32 * 20;
        let finish = Packet::new(sequence, teleport_finish_to(session.agent_id, endpoint, region, session.seed_capability.as_str()))
            .with_reliable()
            .encode()
            .expect("encode handoff");
        sockets[step].send_to(&finish, peers[step]).await.expect("announce next primary");
        if step == 1 {
            let (circuit, peer) = receive_message_with_peer(sockets[target], |message| matches!(message, Message::UseCircuitCode(_))).await;
            peers.push(peer);
            sockets[target].send_to(&ack(sequence + 1, circuit.sequence()), peer).await.expect("ack new circuit");
            loop {
                if let Event::SimulatorConnected { simulator_id, endpoint: actual, .. } = next_raw_event(&mut harness.events).await
                    && actual == endpoint
                {
                    ids.push(simulator_id);
                    break;
                }
            }
        }
        let mut handshake = fixture("region_handshake.hex");
        handshake[0] |= 0x40;
        set_sequence(&mut handshake, sequence + 2);
        sockets[target].send_to(&handshake, peers[target]).await.expect("handshake destination");
        let reply = receive_message(sockets[target], |message| matches!(message, Message::RegionHandshakeReply(_))).await;
        let movement = receive_message(sockets[target], |message| matches!(message, Message::CompleteAgentMovement(_))).await;
        sockets[target].send_to(&ack(sequence + 3, reply.sequence()), peers[target]).await.expect("ack handshake");
        sockets[target].send_to(&ack(sequence + 4, movement.sequence()), peers[target]).await.expect("ack movement");
        sockets[target]
            .send_to(&movement_complete_packet(sequence + 5, session.agent_id, session.session_id, region), peers[target])
            .await
            .expect("complete movement");
        loop {
            match next_raw_event(&mut harness.events).await {
                Event::SimulatorPromoted { previous_simulator_id, simulator_id, session, .. } => {
                    assert_eq!(previous_simulator_id, ids[step]);
                    assert_eq!(simulator_id, ids[target]);
                    assert_eq!(session.simulator_address, endpoint);
                    break;
                }
                Event::Failed(reason) => panic!("handoff failed: {reason}"),
                _ => {}
            }
        }
        assert_eq!(harness.status.snapshot().simulator_endpoint, Some(endpoint));
        harness
            .commands
            .send(Command::SendMessage { message: object_request(), delivery: Delivery::Unreliable })
            .await
            .expect("default send");
        receive_message(sockets[target], |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        let (completed, result) = oneshot::channel();
        harness
            .commands
            .send(Command::SendTo { simulator_id: ids[step], message: object_request(), delivery: Delivery::Unreliable, completed })
            .await
            .expect("send to former primary");
        receive_message(sockets[step], |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert!(timeout(WAIT, result).await.expect("explicit send finishes").expect("send reply").is_ok());
        if step == 1 {
            let stale_sequence = sequence + 6;
            let stale_region = 0x1000_0000_0000_1000;
            let stale_movement = movement_complete_packet(stale_sequence, session.agent_id, session.session_id, stale_region);
            sockets[step].send_to(&stale_movement, peers[step]).await.expect("send old-target movement completion from demoted primary");
            receive_ack(sockets[step], stale_sequence).await;

            let barrier_sequence = stale_sequence + 1;
            let mut barrier = fixture("chat_from_simulator.hex");
            barrier[0] |= 0x40;
            set_sequence(&mut barrier, barrier_sequence);
            sockets[step].send_to(&barrier, peers[step]).await.expect("send barrier after stale movement");
            receive_ack(sockets[step], barrier_sequence).await;

            let mut stale_movement_published = false;
            timeout(WAIT, async {
                loop {
                    match next_raw_event(&mut harness.events).await {
                        Event::SimulatorMessage { simulator_id, sequence, message, .. } if simulator_id == ids[step] => {
                            if sequence == stale_sequence && matches!(message, Message::AgentMovementComplete(_)) {
                                stale_movement_published = true;
                            }
                            if sequence == barrier_sequence && matches!(message, Message::ChatFromSimulator(_)) {
                                break;
                            }
                        }
                        Event::Failed(reason) => panic!("stale movement filter failed: {reason}"),
                        _ => {}
                    }
                }
            })
            .await
            .expect("barrier from demoted primary should be published");
            assert!(!stale_movement_published, "demoted primary must not reuse handoff authorization from its promotion");
        }
        if step == 0 {
            let mut disable = fixture("disable_simulator.hex");
            set_sequence(&mut disable, sequence + 6);
            sockets[step].send_to(&disable, peers[step]).await.expect("close the failed former primary");
            timeout(WAIT, async {
                    loop {
                        if matches!(next_raw_event(&mut harness.events).await, Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == ids[step]) {
                            break;
                        }
                    }
                })
                .await
                .expect("observe former-primary disconnect");
            assert!(matches!(send_targeted(&harness.commands, ids[step], object_request()).await, Err(Error::SimulatorUnavailable)));
            harness
                .commands
                .send(Command::SendMessage { message: object_request(), delivery: Delivery::Unreliable })
                .await
                .expect("current primary remains usable after former-primary failure");
            receive_message(sockets[target], |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        }
    }
    harness.disconnect.send_replace(true);
    receive_message(&third, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(WAIT, harness.client).await.expect("shutdown finishes").expect("coordinator task").is_ok());
    drop(harness.events);
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    assert_eq!(harness.packet_memory.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
}
