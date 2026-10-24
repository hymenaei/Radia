use super::login::*;
use super::*;

fn object_request() -> RequestMultipleObjects {
    RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    }
}

pub(super) fn targeted_send_handle(commands: mpsc::Sender<Command>, disconnect_requested: watch::Sender<bool>) -> SessionHandle {
    let (_, worker_finished) = watch::channel(false);
    SessionHandle { commands, disconnect_requested, command_admission_closed: Arc::new(Mutex::new(false)), worker_finished }
}

fn chat_message_with_size(size: usize) -> ChatFromViewer {
    chat_message_with_text("x".repeat(size))
}

pub(super) fn chat_message_with_text(message: String) -> ChatFromViewer {
    ChatFromViewer {
        agent_data: chat_from_viewer::AgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        chat_data: chat_from_viewer::ChatData { message, chat_type: ChatType::Normal, channel: 0 },
    }
}

async fn run_stalled_send_is_isolated() {
    let mut harness = connected_neighbor().await;
    harness.pause_sends.send_replace(true);
    let (completed, mut response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: Message::RequestMultipleObjects(object_request()),
            delivery: Delivery::Reliable,
            completed,
        })
        .await
        .expect("dispatch primary send");
    timeout(WAIT, harness.send_entered.notified()).await.expect("primary enters local send");
    std::assert_matches!(response.try_recv(), Err(oneshot::error::TryRecvError::Empty));
    advance(Duration::from_secs(5)).await;
    std::assert_matches!(response.await.expect("stalled primary send should report its age limit"), Err(Error::SimulatorSend(_)));

    let neighbor_send = spawn({
        let commands = harness.commands.clone();
        let id = harness.neighbor_id;
        async move { send_targeted(&commands, id, Message::RequestMultipleObjects(object_request())).await }
    });
    let sent = receive_message(&harness.neighbor, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    assert!(neighbor_send.await.expect("send task").is_ok());
    harness.neighbor.send_to(&ack(41, sent.sequence()), harness.neighbor_peer).await.expect("ack neighbor request");

    let mut chat = test_fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 42);
    harness.neighbor.send_to(&chat, harness.neighbor_peer).await.expect("send unrelated reliable data");
    loop {
        if matches!(next_raw_event(&mut harness.events).await, Event::SimulatorMessage { simulator_id, sequence: 42, .. } if simulator_id == harness.neighbor_id)
        {
            break;
        }
    }
    receive_ack(&harness.neighbor, 42).await;
    harness.neighbor.send_to(&start_ping_packet(43, 9, 0), harness.neighbor_peer).await.expect("ping unrelated connection");
    receive_message(&harness.neighbor, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 9)).await;
    let (abandoned, abandoned_receiver) = oneshot::channel();
    drop(abandoned_receiver);
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: Message::RequestMultipleObjects(object_request()),
            delivery: Delivery::Reliable,
            completed: abandoned,
        })
        .await
        .expect("admit abandoned send");
    harness.neighbor.send_to(&start_ping_packet(44, 10, 0), harness.neighbor_peer).await.expect("ping after abandoned send");
    receive_message(&harness.neighbor, |message| matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == 10)).await;

    let started = tokio::time::Instant::now();
    harness.disconnect.send_replace(true);
    assert!(harness.client.await.expect("runtime task").is_ok());
    assert!(started.elapsed() <= LOGOUT_TIMEOUT + WAIT, "bounded shutdown despite stalled send");
    assert_eq!(harness.packet_memory.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
}

async fn run_stalled_logout_is_unavailable() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let (send_gate, pause_sends, send_entered) = new_send_gate();
    let mut runtime =
        connect_with_send_gate(login_session(simulator.local_addr().expect("simulator address")), ResponseData::default(), send_gate)
            .await
            .expect("connect root simulator circuit");
    receive(&simulator).await;
    pause_sends.send_replace(true);
    let (first_logout, first_response) = oneshot::channel();
    runtime.primary.logout.send(first_logout).await.expect("start a stalled logout send");
    timeout(WAIT, send_entered.notified()).await.expect("logout should enter the stalled local send");
    advance(Duration::from_secs(1)).await;
    runtime.logout_attempted = true;
    runtime.pending_primary_logout =
        Some(PendingLogout { completion: first_response, deadline: tokio::time::Instant::now() + LOGOUT_TIMEOUT });
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, _event_receiver) = mpsc::channel(TEST_EVENT_CAPACITY);
    let terminal_event = events.clone().reserve_owned().await.expect("reserve terminal event");
    let (_disconnect, disconnect_requested) = watch::channel(true);

    let result = runtime.run(command_receiver, events, disconnect_requested, terminal_event).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn disconnect_waits() {
    let (commands, _command_receiver) = mpsc::channel(1);
    let (disconnect_requested, mut disconnect_state) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let handle = SessionHandle { commands, disconnect_requested, command_admission_closed: Arc::new(Mutex::new(false)), worker_finished };
    let first_handle = handle.clone();
    let second_handle = handle.clone();
    let mut first_disconnect = spawn(async move { first_handle.disconnect().await });

    assert!(disconnect_state.changed().await.is_ok());
    let mut second_disconnect = spawn(async move { second_handle.disconnect().await });
    assert!(timeout(Duration::from_millis(10), &mut first_disconnect).await.is_err());
    assert!(timeout(Duration::from_millis(10), &mut second_disconnect).await.is_err());

    worker_finished_sender.send_replace(true);
    timeout(Duration::from_secs(1), first_disconnect)
        .await
        .expect("worker completion should release the first disconnect")
        .expect("first disconnect task");
    timeout(Duration::from_secs(1), second_disconnect)
        .await
        .expect("worker completion should release the repeated disconnect")
        .expect("second disconnect task");
}

#[tokio::test]
async fn dropping_send_future_keeps_the_admitted_command() {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Waker},
    };

    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let simulator_id = test_simulator_id(42);
    {
        let mut send = pin!(handle.send_to(simulator_id, object_request(), Delivery::Reliable));
        assert!(send.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    }

    let Command::SendTo { completed, .. } = command_receiver.try_recv().expect("admitted command survives cancellation") else {
        panic!("expected targeted send");
    };
    assert!(completed.is_closed(), "only the result receiver is cancelled");
}

#[tokio::test]
async fn targeted_send_keeps_id() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let simulator_id = test_simulator_id(42);
    let request = object_request();
    let send = spawn(async move { handle.send_to(simulator_id, request, Delivery::Reliable).await });

    let Some(Command::SendTo { simulator_id: routed_id, message, delivery, completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };
    assert_eq!(routed_id, simulator_id);
    assert_eq!(delivery, Delivery::Reliable);
    std::assert_matches!(message, Message::RequestMultipleObjects(_));
    completed.send(Ok(())).expect("return queue admission result");
    assert!(send.await.expect("send task").is_ok());
}

#[tokio::test]
async fn targeted_send_full() {
    let (commands, _command_receiver) = mpsc::channel(1);
    commands.try_send(Command::SubmitMfaCode("queued command".into())).expect("fill the session command queue");
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: Vec::new(),
    };

    std::assert_matches!(handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await, Err(Error::SimulatorCommandQueueFull));
}

#[tokio::test]
async fn sends_are_rejected_after_disconnect_begins() {
    let (commands, mut command_receiver) = mpsc::channel(2);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    handle.request_disconnect();

    std::assert_matches!(handle.send(chat_message_with_text("after disconnect".into()), Delivery::Unreliable), Err(Error::SessionClosed));
    let request =
        RequestMultipleObjects { agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() }, objects: Vec::new() };
    std::assert_matches!(handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await, Err(Error::SessionClosed));
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn targeted_send_target_closed() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let request = object_request();
    let send = spawn(async move { handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    drop(completed);

    std::assert_matches!(send.await.expect("send task"), Err(Error::SimulatorConnectionClosed));
}

#[tokio::test]
async fn targeted_send_resolves_when_session_closes() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_signal, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_signal.clone());
    let request = object_request();
    let send_handle = handle.clone();
    let send = spawn(async move { send_handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    handle.request_disconnect();
    std::assert_matches!(
        timeout(Duration::from_secs(1), send).await.expect("disconnect should release the targeted send waiter").expect("send task"),
        Err(Error::SessionClosed)
    );
    assert!(completed.send(Ok(())).is_err(), "the caller should stop waiting when disconnect begins");
}

#[tokio::test]
async fn targeted_send_reports_finished_worker() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let handle = SessionHandle { commands, disconnect_requested, command_admission_closed: Arc::new(Mutex::new(false)), worker_finished };
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    };
    let send = spawn(async move { handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    worker_finished_sender.send_replace(true);
    drop(completed);

    std::assert_matches!(send.await.expect("send task"), Err(Error::SessionClosed));
}

#[test]
fn outbound_chat_limit_precedes_queue_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);

    std::assert_matches!(
        handle.send(chat_message_with_size(MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1), Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    );
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    let mut oversized_capacity = String::with_capacity(MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1);
    oversized_capacity.push('x');
    std::assert_matches!(
        handle.send(chat_message_with_text(oversized_capacity), Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    );
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    handle.send(chat_message_with_size(MAX_OUTBOUND_CHAT_MESSAGE_BYTES), Delivery::Unreliable).expect("exact limit fits");
    assert_eq!(commands.capacity(), 0);
    let Some(Command::SendMessage { message: Message::ChatFromViewer(message), .. }) = command_receiver.try_recv().ok() else {
        panic!("expected the exact-limit chat message");
    };
    assert_eq!(message.chat_data.message.len(), MAX_OUTBOUND_CHAT_MESSAGE_BYTES);
}

#[tokio::test]
async fn object_request_limit_precedes_targeted_queue_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }; MAX_OUTBOUND_OBJECT_REQUESTS + 1],
    };

    std::assert_matches!(handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    let mut oversized_capacity = Vec::with_capacity(MAX_OUTBOUND_OBJECT_REQUESTS + 1);
    oversized_capacity.push(ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 });
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: oversized_capacity,
    };
    std::assert_matches!(handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn object_request_at_limit_is_accepted() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let mut objects = Vec::with_capacity(MAX_OUTBOUND_OBJECT_REQUESTS);
    objects.resize(MAX_OUTBOUND_OBJECT_REQUESTS, ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 });
    let request = RequestMultipleObjects { agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() }, objects };

    let send = spawn(async move { handle.send_to(test_simulator_id(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { message: Message::RequestMultipleObjects(message), completed, .. }) = command_receiver.recv().await else {
        panic!("expected an accepted object request");
    };
    assert_eq!(message.objects.len(), MAX_OUTBOUND_OBJECT_REQUESTS);
    completed.send(Ok(())).expect("send result receiver");
    assert!(send.await.expect("send task").is_ok());
}

#[tokio::test]
async fn oversized_sends_are_rejected_before_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);
    let session = test_session(handle, mpsc::channel(1).1);
    std::assert_matches!(
        session.send(chat_message_with_size(MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1), Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    );

    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }; MAX_OUTBOUND_OBJECT_REQUESTS + 1],
    };
    std::assert_matches!(session.send_to(test_simulator_id(42), request, Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge));

    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[test]
fn session_drop_requests_disconnect() {
    let (commands, _command_receiver) = mpsc::channel(1);
    let (disconnect_requested, disconnect_observed) = watch::channel(false);
    let (_worker_finished_sender, worker_finished) = watch::channel(false);
    let handle = SessionHandle { commands, disconnect_requested, command_admission_closed: Arc::new(Mutex::new(false)), worker_finished };
    let session = test_session(handle, mpsc::channel(1).1);

    drop(session);

    assert!(*disconnect_observed.borrow());
}

#[tokio::test]
async fn shutdown_cancels_error() {
    let (events, _event_receiver) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("fill event queue".into()))).expect("fill event queue");
    let (shutdown, mut shutdown_requested) = watch::channel(false);
    let failure_events = events.clone();
    let mut send_failure =
        spawn(async move { send_failure_event(&failure_events, "network failed".into(), &mut shutdown_requested).await });

    assert!(timeout(Duration::from_millis(10), &mut send_failure).await.is_err(), "full queue should apply backpressure");
    shutdown.send_replace(true);
    let outcome =
        timeout(Duration::from_secs(1), send_failure).await.expect("shutdown should release the blocked send").expect("send task");

    assert!(matches!(outcome, EventSendOutcome::Shutdown));
}

#[test]
fn full_command_queue_errors() {
    let (commands, mut receiver) = mpsc::channel(COMMAND_CAPACITY);
    let challenge = MfaChallenge::new(commands);
    std::assert_matches!(challenge.complete(" \t "), Err(Error::EmptyMfaCode));
    for _ in 0..COMMAND_CAPACITY {
        MfaChallenge::new(challenge.commands.clone()).complete("123456").expect("queue has room");
    }

    std::assert_matches!(challenge.complete("123456"), Err(Error::CommandQueueFull));
    assert!(receiver.try_recv().is_ok());
    challenge.complete("123456").expect("challenge can be retried after queue pressure");
    std::assert_matches!(challenge.complete("123456"), Err(Error::MfaChallengeCompleted));
}

#[test]
fn stalled_send_is_isolated() {
    run_large_paused_stack_test(run_stalled_send_is_isolated);
}

#[test]
fn stalled_logout_is_unavailable() {
    run_large_paused_stack_test(run_stalled_logout_is_unavailable);
}

#[tokio::test]
async fn abandoned_send_does_not_block() {
    let harness = connected_neighbor().await;
    harness.pause_sends.send_replace(true);

    let (first, first_response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: Message::RequestMultipleObjects(object_request()),
            delivery: Delivery::Reliable,
            completed: first,
        })
        .await
        .expect("dispatch first send");
    drop(first_response);
    timeout(WAIT, harness.send_entered.notified()).await.expect("first send should stall in the local socket");

    let (second, second_response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: Message::RequestMultipleObjects(object_request()),
            delivery: Delivery::Reliable,
            completed: second,
        })
        .await
        .expect("queue second send");
    drop(second_response);

    harness.pause_sends.send_replace(false);
    for ack_sequence in 24..26 {
        let sent = receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        harness.primary.send_to(&ack(ack_sequence, sent.sequence()), harness.primary_peer).await.expect("ack abandoned send");
    }

    let (completed, response) = oneshot::channel();
    harness
        .commands
        .send(Command::SendTo {
            simulator_id: harness.primary_id,
            message: Message::RequestMultipleObjects(object_request()),
            delivery: Delivery::Reliable,
            completed,
        })
        .await
        .expect("dispatch send after abandoned completions");
    let sent = receive_message(&harness.primary, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
    timeout(WAIT, response).await.expect("later send completion should arrive").expect("send reply").expect("later send succeeds");
    harness.primary.send_to(&ack(26, sent.sequence()), harness.primary_peer).await.expect("ack later send");

    harness.shutdown().await;
}
