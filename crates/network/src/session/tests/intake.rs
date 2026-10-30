use std::{
    pin::pin,
    task::{Context, Waker},
};

use super::login::*;
use super::send::{chat_message_with_text, targeted_send_handle};
use super::*;
use crate::event::send_client_event;
use tokio::task::yield_now;

#[tokio::test]
async fn session_drop_cancels_login() {
    let (login_url, request_started, server) = fake_pending_login_server().await;
    let (session, retained_handle, worker) = spawn_test_session(login_url);

    request_started.await.expect("login request should start");
    drop(session);

    assert!(timeout(Duration::from_secs(1), worker).await.expect("login worker should stop").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());
    assert!(timeout(Duration::from_secs(1), server).await.expect("fake login server should stop").expect("server task"));
}

#[tokio::test]
async fn session_drop_cancels_mfa() {
    let (login_url, stop_server, server) = fake_mfa_challenge_server().await;
    let (mut session, retained_handle, worker) = spawn_test_session(login_url);

    let challenge = match next_event(&mut session.events).await {
        Event::MfaRequired { challenge } => challenge,
        other => panic!("expected MFA challenge, got {other:?}"),
    };
    drop(session);

    assert!(timeout(Duration::from_secs(1), worker).await.expect("MFA worker should stop").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());
    stop_server.send(()).expect("fake MFA server should wait for test shutdown");
    assert!(!timeout(Duration::from_secs(1), server).await.expect("fake server should finish").expect("server task"));
    drop(challenge);
}

#[tokio::test]
async fn session_drop_cancels_full_event() {
    let (commands, _command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    for _ in 0..CLIENT_EVENT_CAPACITY {
        events.try_send(EventEnvelope::new(Event::Failed("fill event queue".into()))).expect("fill the event queue");
    }
    let (disconnect, mut disconnect_requested) = watch::channel(false);
    let session = test_session(targeted_send_handle(commands, disconnect), event_receiver);
    let mut publish = pin!(send_client_event(&events, Event::Failed("pending event".into()), &mut disconnect_requested));
    assert!(publish.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(), "full event queue blocks publication");
    let retained_handle = session.handle();
    drop(session);

    assert!(matches!(
        timeout(Duration::from_secs(1), publish).await.expect("session drop should release event publication"),
        EventSendOutcome::Shutdown
    ));
    assert!(*retained_handle.disconnect_requested.borrow());
}

#[tokio::test]
async fn receive_releases_capability_bytes() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let budget = Arc::new(Semaphore::new(32));
    let permit = budget.clone().acquire_many_owned(17).await.expect("reserve capability bytes");
    let (events, event_receiver) = mpsc::channel(1);
    events
        .send(event_with_retained_bytes(
            Event::CapabilityEvent {
                simulator_id: test_simulator_id(77),
                source: "127.0.0.1:13000".parse().expect("simulator endpoint"),
                message: "TestEvent".into(),
                body: Value::Undefined,
            },
            permit,
        ))
        .await
        .expect("queue capability event");
    let mut session = test_session(targeted_send_handle(commands, disconnect), event_receiver);

    assert_eq!(budget.available_permits(), 15);
    std::assert_matches!(session.recv_event().await, Some(Event::CapabilityEvent { message, .. }) if message == "TestEvent");
    assert_eq!(budget.available_permits(), 32);
}

#[tokio::test]
async fn recv_batch_keeps_order() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let (events, event_receiver) = mpsc::channel(1);
    let order = EventQueueBatchOrder { incarnation: 71, ordinal: 9 };
    events
        .send(EventEnvelope::new(Event::CapabilityEventBatch {
            simulator_id: test_simulator_id(77),
            source: "127.0.0.1:13000".parse().expect("simulator endpoint"),
            order,
            events: vec![CapabilityEvent { message: "TestEvent".into(), body: Value::Undefined, retained_bytes: 0 }],
        }))
        .await
        .expect("queue EventQueue batch");
    let mut session = test_session(targeted_send_handle(commands, disconnect), event_receiver);

    let Some(Event::CapabilityEventBatch { order: received_order, events, .. }) = session.recv_event().await else {
        panic!("Session returns the queued EventQueue batch")
    };
    assert_eq!(received_order, order);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "TestEvent");
}

#[tokio::test]
async fn final_loss_wakes_empty_intake() {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        task::Wake,
    };

    struct WakeFlag(AtomicBool);
    impl Wake for WakeFlag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = test_simulator_id(77);
    let source = CoverageSource::Lludp;
    let (events, event_receiver) = mpsc::channel(1);
    let mut session = test_session_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger.clone());
    let mut receive = pin!(session.recv_event());
    let wake = Arc::new(WakeFlag(AtomicBool::new(false)));
    let waker = Waker::from(Arc::clone(&wake));
    assert!(receive.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    let CoverageRecord::Recorded(watermark) = ledger.record(simulator_id, source, CoverageLossReason::EventAdmission) else {
        panic!("new source has ledger capacity")
    };

    assert!(wake.0.load(Ordering::SeqCst), "recording loss wakes an already waiting intake");
    std::assert_matches!(
        timeout(Duration::from_secs(1), receive).await.expect("source loss wakes empty intake"),
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) if record.watermark == watermark
    );
    drop(events);
}

#[tokio::test]
async fn coverage_loss_precedes_event_and_awaits_ack() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = test_simulator_id(78);
    let source = CoverageSource::Lludp;
    let CoverageRecord::Recorded(watermark) = ledger.record(simulator_id, source, CoverageLossReason::DecodeAdmission) else {
        panic!("new source has ledger capacity")
    };
    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(event_with_coverage_watermark(Event::Failed("covered event".into()), watermark))
        .expect("fill bounded event queue with covered event");
    let mut session = test_session_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger.clone());
    session.acknowledge_coverage_loss(watermark);
    std::assert_matches!(
        session.recv_event().await,
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) if record.watermark == watermark
    );
    {
        let mut receive = pin!(session.recv_event());
        assert!(
            receive.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(),
            "covered event waits for loss acknowledgement"
        );
    }
    session.acknowledge_coverage_loss(watermark);
    std::assert_matches!(
        session.recv_event().await,
        Some(Event::Failed(message)) if message == "covered event"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn preflight_loss_gates_later_event() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, _handle, worker) = spawn_test_session(login_url);

    std::assert_matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. });
    let (_, peer) = receive_udp(&simulator).await;
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (_, movement) = receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    simulator.send_to(&server_ack(21, movement.sequence()), peer).await.expect("ack CompleteAgentMovement");
    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (_, reply) = receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    simulator.send_to(&server_ack(23, reply.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");

    let mut session_ready = false;
    let mut movement_complete_received = false;
    while !session_ready || !movement_complete_received {
        match next_event(&mut session.events).await {
            Event::SessionReady { .. } => session_ready = true,
            Event::SimulatorMessage { sequence: 24, message: Message::AgentMovementComplete(_), .. } => {
                movement_complete_received = true;
            }
            Event::Failed(error) => panic!("client failed before initial movement completed: {error}"),
            _ => {}
        }
    }
    timeout(CLIENT_TIMEOUT, poll_started).await.expect("EventQueueGet should start").expect("EventQueueGet request");

    let mut rejected = test_fixture("improved_terse_object_update.hex");
    rejected[0] |= 0x40;
    set_sequence(&mut rejected, 25);
    rejected[18] = u8::MAX;
    assert!(Packet::preflight(&rejected).is_err(), "an object field exceeding its packet frame must fail LLUDP preflight");
    simulator.send_to(&rejected, peer).await.expect("send preflight-rejected reliable update");

    let watermark = match timeout(CLIENT_TIMEOUT, session.recv_event()).await.expect("preflight loss should wake Session") {
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) => {
            assert_eq!(record.reason, CoverageLossReason::PreflightRejected);
            record.watermark
        }
        other => panic!("expected LLUDP coverage loss, got {other:?}"),
    };

    simulator.send_to(&reliable_fixture("chat_from_simulator.hex", 26), peer).await.expect("send later reliable chat");
    timeout(CLIENT_TIMEOUT, async {
        loop {
            let (bytes, _) = receive_udp(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode chat acknowledgement");
            if packet.appended_acks().contains(&26)
                || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|item| item.id == 26))
            {
                break;
            }
        }
    })
    .await
    .expect("later reliable event should be admitted and acknowledged");

    timeout(CLIENT_TIMEOUT, async {
        while session.events.is_empty() {
            yield_now().await;
        }
    })
    .await
    .expect("later covered event should reach Session intake");
    {
        let mut receive = pin!(session.recv_event());
        assert!(
            receive.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(),
            "Session holds an event covered by an unacknowledged LLUDP loss"
        );
    }

    session.acknowledge_coverage_loss(watermark);
    std::assert_matches!(
        timeout(CLIENT_TIMEOUT, session.recv_event()).await.expect("acknowledgement should release the event"),
        Some(Event::SimulatorMessage { sequence: 26, message: Message::ChatFromSimulator(_), .. })
    );

    drop(session);
    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should stop").expect("client task").is_ok());
    assert_eq!(http_task.await.expect("fake HTTP server should stop").len(), 3);
}

#[tokio::test]
async fn coalesced_source_loss_is_capped_to_the_event_watermark() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = test_simulator_id(80);
    let source = CoverageSource::Lludp;
    std::assert_matches!(ledger.record(simulator_id, source, CoverageLossReason::EventAdmission), CoverageRecord::Recorded(_));
    let event_watermark = ledger.watermark(simulator_id, source);

    for id in 81..=335 {
        std::assert_matches!(ledger.record(test_simulator_id(id), source, CoverageLossReason::EventAdmission), CoverageRecord::Recorded(_));
    }
    std::assert_matches!(
        ledger.record(test_simulator_id(336), source, CoverageLossReason::EventAdmission),
        CoverageRecord::Overflowed { generation: 1 }
    );
    std::assert_matches!(ledger.record(simulator_id, source, CoverageLossReason::DecodeAdmission), CoverageRecord::Recorded(_));

    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(event_with_coverage_watermark(Event::Failed("covered before later loss".into()), event_watermark))
        .expect("queue event with the earlier source watermark");
    let mut session = test_session_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger);

    std::assert_matches!(
        session.recv_event().await,
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) })
            if record.watermark.generation == event_watermark.generation
                && record.watermark.overflow_generation == event_watermark.overflow_generation
    );
    session.acknowledge_coverage_loss(event_watermark);
    std::assert_matches!(
        session.recv_event().await,
        Some(Event::Failed(message)) if message == "covered before later loss"
    );
    std::assert_matches!(session.recv_event().await, Some(Event::CoverageLost { loss: CoverageLoss::SessionOverflow { generation: 1 } }));
}

#[tokio::test]
async fn session_overflow_requires_ack_before_covered_event() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    for id in 1..=256 {
        std::assert_matches!(
            ledger.record(test_simulator_id(id), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
            CoverageRecord::Recorded(_)
        );
    }
    std::assert_matches!(
        ledger.record(test_simulator_id(257), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
        CoverageRecord::Overflowed { generation: 1 }
    );
    let event_watermark = ledger.watermark(test_simulator_id(258), CoverageSource::EventQueue { incarnation: 1 });
    assert_eq!(event_watermark.overflow_generation, 1);

    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(event_with_coverage_watermark(Event::Failed("global-overflow-covered event".into()), event_watermark))
        .expect("queue event covered by session overflow");
    let mut session = test_session_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger);
    session.acknowledge_coverage_overflow(1);
    assert_eq!(session.coverage_losses().overflow_generation, Some(1), "an unsurfaced overflow cannot be acknowledged");

    let (loss_sender, loss_receiver) = oneshot::channel();
    let (ack_sender, ack_receiver) = oneshot::channel();
    let (event_sender, mut event_receiver) = oneshot::channel();
    let receive = spawn(async move {
        let loss = session.recv_event().await;
        let _ = loss_sender.send(loss);
        let _ = ack_receiver.await;
        session.acknowledge_coverage_overflow(1);
        let event = session.recv_event().await;
        let _ = event_sender.send(event);
    });

    std::assert_matches!(
        timeout(Duration::from_secs(1), loss_receiver).await.expect("overflow wakes independently").expect("loss receiver"),
        Some(Event::CoverageLost { loss: CoverageLoss::SessionOverflow { generation: 1 } })
    );
    assert_eq!(event_receiver.try_recv().err(), Some(oneshot::error::TryRecvError::Empty));
    ack_sender.send(()).expect("ask Session to acknowledge surfaced overflow");
    std::assert_matches!(
        timeout(Duration::from_secs(1), &mut event_receiver).await.expect("acknowledgement releases overflow-covered event").expect("event receiver"),
        Some(Event::Failed(message)) if message == "global-overflow-covered event"
    );
    receive.await.expect("receive task completes after overflow acknowledgement");
}

#[tokio::test]
async fn earlier_watermark_does_not_wait_for_later_source_loss() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = test_simulator_id(79);
    let source = CoverageSource::Lludp;
    let earlier = ledger.watermark(simulator_id, source);
    std::assert_matches!(ledger.record(simulator_id, source, CoverageLossReason::EventAdmission), CoverageRecord::Recorded(_));
    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(event_with_coverage_watermark(Event::Failed("earlier event".into()), earlier))
        .expect("queue event predating source loss");
    let mut session = test_session_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger);

    std::assert_matches!(session.recv_event().await, Some(Event::Failed(message)) if message == "earlier event");
    std::assert_matches!(session.recv_event().await, Some(Event::CoverageLost { loss: CoverageLoss::Source(_) }));
}

#[tokio::test(flavor = "multi_thread")]
async fn session_drop_cancels_active_saturated_session() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, retained_handle, worker, mut probe) = spawn_test_session_with_event_probe(login_url);
    let event_sender = &probe.events;

    std::assert_matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. });
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    loop {
        match next_event(&mut session.events).await {
            Event::SessionReady { session: agent_session, .. } => {
                assert_eq!(agent_session.agent_id.to_string(), AGENT_ID);
                break;
            }
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    }
    while session.events.try_recv().is_ok() {}
    timeout(CLIENT_TIMEOUT, poll_started).await.expect("EventQueueGet should start").expect("EventQueueGet request");

    loop {
        match event_sender.try_send(EventEnvelope::new(Event::Failed("hold saturated session events".into()))) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => break,
            Err(mpsc::error::TrySendError::Closed(_)) => panic!("active session event receiver should remain open"),
        }
    }
    assert_eq!(event_sender.capacity(), 0);

    simulator.send_to(&reliable_fixture("chat_from_simulator.hex", 25), peer).await.expect("send while application delivery is saturated");
    timeout(CLIENT_TIMEOUT, async {
        loop {
            let (bytes, _) = receive_udp(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode chat acknowledgement");
            if packet.appended_acks().contains(&25)
                || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|item| item.id == 25))
            {
                break;
            }
        }
    })
    .await
    .expect("the client should process reliable control while events are saturated");
    assert!(!*retained_handle.worker_finished.borrow());
    timeout(CLIENT_TIMEOUT, probe.observed.wait_for(|sequence| *sequence == Some(25)))
        .await
        .expect("session manager forwards the saturated chat")
        .expect("event relay remains active");
    retained_handle
        .send(chat_message_with_text("session remains active".into()), Delivery::Reliable)
        .expect("queue session progress fence");
    let packet = timeout(CLIENT_TIMEOUT, async {
        loop {
            let (bytes, _) = receive_udp(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode session progress fence");
            match packet.message() {
                Message::LogoutRequest(_) => panic!("a queued application event must not log out the session"),
                Message::ChatFromViewer(message) => {
                    assert_eq!(message.chat_data.message, "session remains active");
                    break packet;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("session progress fence arrives");
    simulator.send_to(&server_ack(26, packet.sequence()), peer).await.expect("ack session progress fence");

    drop(session);
    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(
        timeout(Duration::from_secs(2), worker).await.expect("active worker should stop after Session drop").expect("client task").is_ok()
    );
    assert!(*retained_handle.worker_finished.borrow());
    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}
