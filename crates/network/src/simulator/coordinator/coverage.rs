use super::*;

#[tokio::test]
async fn retry_limit_keeps_primary_connected() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let connection =
        SimulatorCoordinator::connect_with_status(login_session(endpoint), ResponseData::default(), None, NetworkStatusHandle::default())
            .await
            .expect("connect simulator circuit");
    let _ = receive(&simulator).await;

    connection.note_primary_event_queue_exit(EventQueueExit::Stopped);
    let mut buffer = [0; 65_535];
    assert_eq!(simulator.try_recv_from(&mut buffer).expect_err("ordinary poll stop sends no logout").kind(), ErrorKind::WouldBlock);

    connection.note_primary_event_queue_exit(EventQueueExit::RetryLimitReached);
    assert_eq!(
        simulator.try_recv_from(&mut buffer).expect_err("EventQueue failure must not log out the simulator").kind(),
        ErrorKind::WouldBlock
    );
}

#[test]
fn former_primary_failure_completion_keeps_its_identity() {
    let simulator_id = SimulatorId::allocate().expect("allocate former primary ID");
    let endpoint = "127.0.0.1:9000".parse().expect("parse endpoint");
    let event = neighbor_completion_event(simulator_id, None, endpoint, &NeighborTaskOutcome::Finished(Err("retry limit reached".into())));
    assert!(matches!(
        event,
        Event::SimulatorFailed { simulator_id: failed_id, region_handle: None, endpoint: failed_endpoint, reason }
            if failed_id == simulator_id && failed_endpoint == endpoint && reason == "retry limit reached"
    ));
}

#[tokio::test]
async fn primary_event_queue_coverage_loss_keeps_session_active() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let connection = SimulatorCoordinator::connect_with_status(
        login_session(simulator.local_addr().expect("read simulator address")),
        ResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect simulator circuit");
    let _ = receive(&simulator).await;

    connection.note_primary_event_queue_exit(EventQueueExit::StoppedWithCoverageLoss);
    let mut buffer = [0; 65_535];
    assert_eq!(simulator.try_recv_from(&mut buffer).expect_err("coverage loss must not send logout").kind(), ErrorKind::WouldBlock);
}

#[tokio::test]
async fn retry_avoids_double_logout() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let mut connection =
        SimulatorCoordinator::connect_with_status(login_session(endpoint), ResponseData::default(), None, NetworkStatusHandle::default())
            .await
            .expect("connect simulator circuit");
    let (_, peer) = receive(&simulator).await;

    let (events, _event_receiver) = mpsc::channel(1);
    let mut pending_events = VecDeque::with_capacity(MAX_PENDING_SIMULATOR_EVENTS);
    for _ in 1..MAX_PENDING_SIMULATOR_EVENTS {
        pending_events.push_back(EventEnvelope::new(Event::Failed("pending event".into())));
    }
    let mut logout_deadline = None;
    let mut disconnect_reason = None;
    connection
        .publish_event(&events, &mut pending_events, Event::Failed("backlog limit".into()), &mut logout_deadline, &mut disconnect_reason)
        .await
        .expect("backlog sends first logout");
    assert!(logout_deadline.is_some());

    let (bytes, logout_peer) = receive(&simulator).await;
    let logout = Packet::decode(&bytes).expect("decode backlog logout");
    assert!(matches!(logout.message(), Message::LogoutRequest(_)));
    assert_eq!(logout_peer, peer);

    connection.note_primary_event_queue_exit(EventQueueExit::RetryLimitReached);

    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), simulator.recv_from(&mut buffer)).await.is_err());
}

#[tokio::test]
async fn failed_backlog_logout_is_not_reported_as_sent() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let status = NetworkStatusHandle::default();
    let mut connection = SimulatorCoordinator::connect_with_status(login_session(endpoint), ResponseData::default(), None, status.clone())
        .await
        .expect("connect simulator circuit");
    receive(&simulator).await;

    let (logout, logout_receiver) = mpsc::channel(1);
    drop(logout_receiver);
    connection.primary.logout = logout;

    let (events, _event_receiver) = mpsc::channel(1);
    let mut pending_events = VecDeque::with_capacity(MAX_PENDING_SIMULATOR_EVENTS);
    for _ in 1..MAX_PENDING_SIMULATOR_EVENTS {
        pending_events.push_back(EventEnvelope::new(Event::Failed("pending event".into())));
    }
    let mut logout_deadline = None;
    let mut disconnect_reason = None;
    connection
        .publish_event(&events, &mut pending_events, Event::Failed("backlog limit".into()), &mut logout_deadline, &mut disconnect_reason)
        .await
        .expect("backlog handling records the unavailable logout");

    assert!(logout_deadline.is_some());
    assert!(!connection.logout_request_sent);
    assert_eq!(
        connection.disconnect_once(connection.logout_request_sent).await.expect("disconnect reports unavailable logout"),
        LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionClosed)
    );
    assert!(status.snapshot().last_error.as_deref().is_some_and(|error| error.contains("was not sent")));

    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(50), simulator.recv_from(&mut buffer)).await.is_err());
}

#[tokio::test]
async fn backlog_logout_drops_covered_application_events_but_keeps_lifecycle_events() {
    use crate::coverage::{CoverageLoss, CoverageLossReason, CoverageSource};

    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let coverage_losses = CoverageLossLedger::new();
    let mut connection = SimulatorCoordinator::connect_with_coverage(
        login_session(endpoint),
        ResponseData::default(),
        None,
        NetworkStatusHandle::default(),
        coverage_losses.clone(),
    )
    .await
    .expect("connect simulator circuit");
    receive(&simulator).await;

    let (events, mut received_events) = mpsc::channel(2);
    let mut pending_events = VecDeque::new();
    let mut logout_deadline = Some(time::Instant::now() + LOGOUT_TIMEOUT);
    let mut disconnect_reason = Some(EVENT_BACKLOG_DISCONNECT_REASON);
    let simulator_id = connection.primary.simulator_id;
    let source = CoverageSource::EventQueue { incarnation: 42 };
    let watermark = coverage_losses.watermark(simulator_id, source);

    connection
        .publish_event(
            &events,
            &mut pending_events,
            EventEnvelope::with_coverage_watermark(
                Event::CapabilityEvent { simulator_id, source: endpoint, message: "CrossedRegion".into(), body: llsd::Value::Undefined },
                watermark,
            ),
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("application event suppression is a normal overload outcome");

    assert!(received_events.try_recv().is_err(), "application data must not resume after overload starts logout");
    assert!(matches!(
        coverage_losses.snapshot().losses.as_slice(),
        [CoverageLoss::Source(record)]
            if record.watermark.simulator_id == simulator_id
                && record.watermark.source == source
                && record.reason == CoverageLossReason::EventAdmission
    ));

    connection
        .publish_event(
            &events,
            &mut pending_events,
            Event::SimulatorDisconnected { simulator_id, region_handle: None, endpoint },
            &mut logout_deadline,
            &mut disconnect_reason,
        )
        .await
        .expect("lifecycle events remain admissible during graceful teardown");
    assert!(matches!(received_events.try_recv().map(|event| event.event), Ok(Event::SimulatorDisconnected { .. })));
}

#[tokio::test]
async fn terminal_logout_skips_capability_events_that_start_neighbor_work() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let mut connection =
        SimulatorCoordinator::connect_with_status(login_session(endpoint), ResponseData::default(), None, NetworkStatusHandle::default())
            .await
            .expect("connect primary simulator");
    receive(&simulator).await;

    let mut run_state = CoordinatorRunState::new();
    let (neighbor_requests, _neighbor_request_rx) = mpsc::channel(1);
    let (application_events, _application_event_rx) = mpsc::channel(1);
    let (neighbor_events, _neighbor_event_rx) = mpsc::channel(1);
    let (_shutdown, shutdown) = watch::channel(false);
    run_state.logout_deadline = Some(time::Instant::now() + LOGOUT_TIMEOUT);
    run_state.disconnect_reason = Some(EVENT_BACKLOG_DISCONNECT_REASON);
    let (capability_events, _capability_event_rx) = mpsc::channel(1);
    let mut setup =
        PromotionSetup { state: &mut run_state, neighbor_requests, application_events, neighbor_events, shutdown, capability_events };
    let body = Value::Map(Map::from([(
        "SimulatorInfo".to_owned(),
        Value::Array(vec![Value::Map(Map::from([
            ("Handle".to_owned(), Value::Binary(0x1000_u64.to_be_bytes().to_vec().into())),
            ("IP".to_owned(), Value::Binary(vec![127, 0, 0, 1].into())),
            ("Port".to_owned(), Value::Integer(13_000)),
        ]))]),
    )]));
    let mut handoff_state = HandoffState::default();

    connection
        .process_capability_event(connection.primary.simulator_id, "EnableSimulator", &body, None, &mut setup, &mut handoff_state)
        .await
        .expect("terminal capability events are discarded without starting work");

    drop(setup);
    assert!(run_state.neighbors.is_empty());
    assert!(run_state.neighbor_connections.is_empty());
    assert!(run_state.simulator_capabilities.is_empty());
    drop(connection);
}
