use super::*;

async fn run_enable_overflow_keeps_primary_responsive() {
    let mut harness = Box::pin(connected_neighbor()).await;
    let mut sockets = Vec::with_capacity(MAX_NEIGHBOR_SIMULATORS);
    for _ in 0..MAX_NEIGHBOR_SIMULATORS {
        sockets.push(UdpSocket::bind("127.0.0.1:0").await.expect("bind announced neighbor"));
    }
    let addresses = sockets.iter().map(|socket| socket.local_addr().expect("read neighbor address")).collect::<Vec<_>>();
    let regions = (0..MAX_NEIGHBOR_SIMULATORS).map(|index| 0x3000_0000_0000_0000 + index as u64).collect::<Vec<_>>();
    let first_sequence = 60;
    let primary = &harness.primary;
    let primary_peer = harness.primary_peer;
    let primary_id = harness.primary_id;
    let events = &mut harness.events;

    let send_announcements = async {
        for index in 0..addresses.len() {
            announce_neighbor(primary, primary_peer, addresses[index], regions[index], first_sequence + index as u32).await;
        }
    };
    let observe_announcements = async {
        let mut seen = vec![false; addresses.len()];
        let mut rejected_endpoint = None;
        timeout(WAIT, async {
            while seen.iter().any(|received| !received) || rejected_endpoint.is_none() {
                match next_event(events).await {
                    Event::SimulatorMessage { simulator_id, sequence, message: Message::EnableSimulator(_), .. }
                        if simulator_id == primary_id && (first_sequence..first_sequence + addresses.len() as u32).contains(&sequence) =>
                    {
                        seen[(sequence - first_sequence) as usize] = true;
                    }
                    Event::SimulatorConnectFailed { region_handle, endpoint, .. }
                        if region_handle == *regions.last().expect("one announcement") =>
                    {
                        rejected_endpoint = Some(endpoint);
                    }
                    _ => {}
                }
            }
        })
        .await
        .expect("primary should process all neighbor announcements");
        (seen, rejected_endpoint)
    };

    let (_, (seen, rejected_endpoint)) = tokio::join!(send_announcements, observe_announcements);
    assert!(seen.into_iter().all(|received| received));
    assert_eq!(rejected_endpoint, addresses.last().copied());

    for socket in &sockets[..MAX_NEIGHBOR_SIMULATORS - 1] {
        let (bytes, _) = receive(socket).await;
        std::assert_matches!(Packet::decode(&bytes).expect("decode admitted neighbor bootstrap").message(), Message::UseCircuitCode(_));
    }
    let rejected = sockets.last().expect("the 33rd neighbor announcement");
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), rejected.recv_from(&mut buffer)).await.is_err());

    harness.primary.send_to(&start_ping_packet(150, 7, 0), harness.primary_peer).await.expect("ping primary after announcement burst");
    std::assert_matches!(
        receive_message(&harness.primary, |message| matches!(message, Message::CompletePingCheck(_))).await.message(),
        Message::CompletePingCheck(message) if message.ping_id.ping_id == 7
    );
    harness.shutdown().await;
}

#[test]
fn enable_overflow_keeps_primary_responsive() {
    run_large_stack_test(run_enable_overflow_keeps_primary_responsive);
}

#[tokio::test]
async fn neighbor_shutdown_preempts_start() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, _event_receiver) = mpsc::channel(1);
    let (shutdown_signal, shutdown) = watch::channel(false);
    shutdown_signal.send_replace(true);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let neighbor_connection =
        connect_test_neighbor(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000).await.expect("connect neighbor circuit");

    let result = timeout(
        Duration::from_millis(100),
        neighbor_connection.run(event_sender, shutdown, neighbor_requests, commands, mpsc::channel(1).1, SimulatorMemoryBudget::new()),
    )
    .await;
    std::assert_matches!(result, Ok(Ok(())), "a pre-signaled shutdown should stop the neighbor immediately");
}

#[tokio::test]
async fn neighbor_logout_bypasses_full_queues() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("read neighbor address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let mut connection =
        connect_test_neighbor(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000).await.expect("connect neighbor circuit");
    let (send_gate, pause_sends, send_entered) = new_send_gate();
    crate::circuit::test_values::set_send_gate(&mut connection, send_gate);
    let (event_sender, mut event_receiver) = mpsc::channel(4);
    let (shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(4);
    let (commands, command_receiver) = mpsc::channel(1);
    let (logout_sender, logout_receiver) = mpsc::channel(1);
    let worker =
        spawn(connection.run(event_sender, shutdown, neighbor_requests, command_receiver, logout_receiver, SimulatorMemoryBudget::new()));

    let (_, peer) = receive(&neighbor).await;
    neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
    std::assert_matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { .. });

    for _ in 0..MAX_PENDING_PACKETS {
        let (completed, response) = oneshot::channel();
        commands
            .send(SimulatorCommand::Send {
                message: object_request(),
                delivery: Delivery::Reliable,
                completed: Some(completed),
                cancel_target: None,
                report_failure: false,
            })
            .await
            .expect("queue reliable neighbor message");
        assert!(timeout(WAIT, response).await.expect("reliable message should be sent").expect("send reply").is_ok());
        let (bytes, sender) = receive(&neighbor).await;
        assert_eq!(sender, peer);
        std::assert_matches!(Packet::decode(&bytes).expect("decode reliable message").message(), Message::RequestMultipleObjects(_));
    }

    pause_sends.send_replace(true);
    let (queued_completed, queued_response) = oneshot::channel();
    commands
        .try_send(SimulatorCommand::Send {
            message: chat_message("x".repeat(7_900)),
            delivery: Delivery::Unreliable,
            completed: Some(queued_completed),
            cancel_target: None,
            report_failure: false,
        })
        .expect("fill the application command queue");
    timeout(WAIT, send_entered.notified()).await.expect("ordinary send should reach the paused local socket");
    let (queued_tail_completed, queued_tail_response) = oneshot::channel();
    commands
        .send(SimulatorCommand::Send {
            message: object_request(),
            delivery: Delivery::Unreliable,
            completed: Some(queued_tail_completed),
            cancel_target: None,
            report_failure: false,
        })
        .await
        .expect("queue an ordinary message behind the paused send");
    commands.send(SimulatorCommand::CancelHandoff).await.expect("queue a command behind the application message");
    timeout(WAIT, async {
        while commands.capacity() == 0 {
            task::yield_now().await;
        }
    })
    .await
    .expect("connection should process the command after the queued application message");
    let (logout_completed, logout_response) = oneshot::channel();
    logout_sender.send(logout_completed).await.expect("queue priority logout control");

    std::assert_matches!(queued_response.await, Ok(Err(Error::SimulatorConnectionClosed)));
    std::assert_matches!(queued_tail_response.await, Ok(Err(Error::SimulatorConnectionClosed)));
    pause_sends.send_replace(false);

    let (logout, logout_peer) = timeout(WAIT, receive_message_with_peer(&neighbor, |message| matches!(message, Message::LogoutRequest(_))))
        .await
        .expect("logout should bypass the full reliable window and application queue");
    assert_eq!(logout_peer, peer);
    assert!(logout.is_reliable());
    std::assert_matches!(logout.message(), Message::LogoutRequest(_));
    assert!(timeout(WAIT, logout_response).await.expect("logout send should complete").expect("logout reply").is_ok());

    shutdown_signal.send_replace(true);
    assert!(timeout(WAIT, worker).await.expect("neighbor should stop after shutdown").expect("neighbor task").is_ok());
}

#[tokio::test]
async fn neighbor_receiver_close_preempts() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    let (_shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let neighbor_connection =
        connect_test_neighbor(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000).await.expect("connect neighbor circuit");
    let task = spawn(neighbor_connection.run(
        event_sender,
        shutdown,
        neighbor_requests,
        commands,
        mpsc::channel(1).1,
        SimulatorMemoryBudget::new(),
    ));

    let (use_circuit, peer) = receive(&neighbor).await;
    std::assert_matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_));
    neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
    std::assert_matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { .. });

    drop(event_receiver);
    assert!(
        timeout(Duration::from_millis(200), task)
            .await
            .expect("neighbor should stop after event receiver closes")
            .expect("neighbor task")
            .is_ok()
    );
}

#[tokio::test]
async fn neighbor_ping_is_delayed() {
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let simulator_id = SimulatorId::allocate().expect("allocate neighbor ID");
    let (event_sender, mut event_receiver) = mpsc::channel(1);
    let (shutdown_signal, shutdown) = watch::channel(false);
    let (neighbor_requests, _neighbor_request_receiver) = mpsc::channel(1);
    let (_command_sender, commands) = mpsc::channel(1);
    let neighbor_connection =
        connect_test_neighbor(simulator_id, login_session(endpoint), 0x1000_0000_0000_1000).await.expect("connect neighbor circuit");
    let task = spawn(neighbor_connection.run(
        event_sender,
        shutdown,
        neighbor_requests,
        commands,
        mpsc::channel(1).1,
        SimulatorMemoryBudget::new(),
    ));

    let (use_circuit, peer) = receive(&neighbor).await;
    std::assert_matches!(Packet::decode(&use_circuit).expect("decode neighbor circuit packet").message(), Message::UseCircuitCode(_));
    neighbor.send_to(&ack(40, 1), peer).await.expect("ack neighbor circuit");
    std::assert_matches!(next_neighbor_event(&mut event_receiver).await, Event::SimulatorConnected { .. });

    let mut buffer = [0; 65_535];
    assert!(
        timeout(Duration::from_millis(100), neighbor.recv_from(&mut buffer)).await.is_err(),
        "the neighbor should wait for its ping interval after connecting"
    );

    shutdown_signal.send_replace(true);
    assert!(timeout(Duration::from_millis(200), task).await.expect("neighbor should stop on shutdown").expect("neighbor task").is_ok());
}
