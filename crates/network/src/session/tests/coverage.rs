use super::*;
use crate::fixtures::event_with_coverage_watermark;
use tokio::time;

#[tokio::test]
async fn retry_limit_keeps_primary_connected() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let connection = connect_runtime(login_session(endpoint), ResponseData::default()).await.expect("connect simulator circuit");
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
    let event = neighbor_completion_event(
        simulator_id,
        None,
        endpoint,
        &SimulatorTaskOutcome::Finished(Err(ConnectionError::ShutdownReason("retry limit reached"))),
    );
    std::assert_matches!(
        event,
        Event::SimulatorFailed { simulator_id: failed_id, region_handle: None, endpoint: failed_endpoint, reason }
            if failed_id == simulator_id && failed_endpoint == endpoint && reason == "retry limit reached"
    );
}

#[tokio::test]
async fn primary_event_queue_coverage_loss_keeps_session_active() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let connection = connect_runtime(login_session(simulator.local_addr().expect("read simulator address")), ResponseData::default())
        .await
        .expect("connect simulator circuit");
    let _ = receive(&simulator).await;

    connection.note_primary_event_queue_exit(EventQueueExit::StoppedWithCoverageLoss);
    let mut buffer = [0; 65_535];
    assert_eq!(simulator.try_recv_from(&mut buffer).expect_err("coverage loss must not send logout").kind(), ErrorKind::WouldBlock);
}

#[tokio::test]
async fn primary_event_backlog_reports_coverage_without_logout() {
    use crate::coverage::{CoverageLoss, CoverageLossReason, CoverageSource};

    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let coverage_losses = CoverageLossLedger::new();
    let mut connection = connect_with_coverage(login_session(endpoint), ResponseData::default(), coverage_losses.clone())
        .await
        .expect("connect simulator circuit");
    receive(&simulator).await;

    let (events, mut received_events) = mpsc::channel(2);
    let mut run_state = RunState::new();
    for _ in 0..MAX_QUEUED_SIMULATOR_EVENTS {
        run_state.pending_events.push_back(EventEnvelope::new(Event::Failed("pending event".into())));
    }
    let simulator_id = connection.primary.simulator_id;
    let source = CoverageSource::EventQueue { incarnation: 42 };
    let watermark = coverage_losses.watermark(simulator_id, source);

    connection
        .publish_event(
            &events,
            &mut run_state,
            event_with_coverage_watermark(
                Event::CapabilityEvent { simulator_id, source: endpoint, message: "CrossedRegion".into(), body: llsd::Value::Undefined },
                watermark,
            ),
        )
        .await
        .expect("a full event queue should report coverage loss without ending the session");

    assert!(received_events.try_recv().is_err(), "the full queue must not admit an extra event");
    assert!(run_state.logout_deadline.is_none(), "queue pressure must not start simulator logout");
    std::assert_matches!(
        coverage_losses.snapshot().losses.as_slice(),
        [CoverageLoss::Source(record)]
            if record.watermark.simulator_id == simulator_id
                && record.watermark.source == source
                && record.reason == CoverageLossReason::EventAdmission
    );

    run_state.pending_events.clear();
    connection
        .publish_event(&events, &mut run_state, Event::SimulatorDisconnected { simulator_id, region_handle: None, endpoint })
        .await
        .expect("events should flow again after the consumer drains the backlog");
    std::assert_matches!(received_events.try_recv().map(|event| event.event), Ok(Event::SimulatorDisconnected { .. }));
    assert!(run_state.logout_deadline.is_none());
}

#[tokio::test]
async fn terminal_logout_skips_capability_events_that_start_neighbor_work() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let endpoint = simulator.local_addr().expect("read simulator address");
    let mut connection = connect_runtime(login_session(endpoint), ResponseData::default()).await.expect("connect primary simulator");
    receive(&simulator).await;

    let mut run_state = RunState::new();
    let (neighbor_requests, _neighbor_request_rx) = mpsc::channel(1);
    let (application_events, _application_event_rx) = mpsc::channel(1);
    let (neighbor_events, _neighbor_event_rx) = mpsc::channel(1);
    let (_shutdown, shutdown) = watch::channel(false);
    run_state.logout_deadline = Some(time::Instant::now() + LOGOUT_TIMEOUT);
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
    let simulator_id = connection.primary.simulator_id;

    connection
        .process_capability_event(simulator_id, "EnableSimulator", &body, None, &mut setup, &mut handoff_state)
        .await
        .expect("terminal capability events are discarded without starting work");

    drop(setup);
    assert!(run_state.neighbors.is_empty());
    assert!(run_state.neighbor_connections.is_empty());
    assert!(run_state.simulator_capabilities.is_empty());
    drop(connection);
}
