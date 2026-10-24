use super::*;
use crate::fixtures::event_queue::EventQueueBudgetHarness;

#[tokio::test(flavor = "multi_thread")]
async fn shared_budget_bounds_event_polls() {
    let task_count = MAX_CONCURRENT_EVENT_QUEUES + 1;
    let server = EventQueueBudgetTestServer::start();
    let harness = EventQueueBudgetHarness::new(1);
    let clients = (0..2).map(|_| harness.client(server.url("/seed"))).collect::<Vec<_>>();
    let events = harness.events.clone();
    events.try_send(EventEnvelope::new(Event::SendFailed("hold event forwarding".into()))).expect("fill the application event channel");
    let shutdown = harness.shutdown.clone();
    let mut tasks = Vec::with_capacity(task_count);

    for index in 0..task_count {
        let client = clients[index % clients.len()].clone();
        let task = if index == 0 {
            harness.start_with_role(&client, test_simulator_id(index as u64 + 1), test_source(), CoverageLossLedger::new(), true)
        } else {
            harness.start(&client, test_simulator_id(index as u64 + 1), test_source(), CoverageLossLedger::new())
        };
        tasks.push(task);
    }
    let mut received_events = EventReceiver::new(harness.received_events);

    let mut polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
    for _ in 0..task_count {
        server.next_seed();
    }
    let unexpected_poll = server.polls.recv_timeout(Duration::from_millis(150)).ok();
    let admitted = unexpected_poll.is_none();
    let mut forwarded_batch = false;

    if admitted {
        let wait_started = Instant::now();
        write_response(polls.pop().expect("one event poll to release"), event_batch_response(5, &["First", "Second"]));
        std::assert_matches!(
            timeout(CLIENT_TEST_TIMEOUT, received_events.recv()).await.expect("held event channel item").expect("event channel remains open"),
            Event::SendFailed(message) if message == "hold event forwarding"
        );
        timeout(CLIENT_TEST_TIMEOUT, async {
            while received_events.is_empty() {
                yield_now().await;
            }
        })
        .await
        .expect("first event should fill the application channel");
        let no_poll_while_forwarding = server.polls.recv_timeout(Duration::from_millis(150)).is_err();
        let batch_event = timeout(CLIENT_TEST_TIMEOUT, received_events.recv())
            .await
            .expect("complete decoded batch should be forwarded")
            .expect("event channel remains open");
        let next_poll = server.next_poll();
        let observed_wait = wait_started.elapsed();
        eprintln!(
            "observed EventQueue wait from confirmed contention to next server poll (includes slot release and HTTP handling): {observed_wait:?}"
        );
        polls.push(next_poll);
        forwarded_batch = no_poll_while_forwarding
            && matches!(batch_event, Event::CapabilityEventBatch { events, .. }
                        if events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]);
    } else if let Some(poll) = unexpected_poll {
        polls.push(poll);
    }

    shutdown.send_replace(true);
    drop(polls);

    timeout(CLIENT_TEST_TIMEOUT, async {
        for task in tasks {
            assert_eq!(task.wait().await, EventQueueExit::Stopped);
        }
    })
    .await
    .expect("all event queue tasks should stop");
    server.stop();
    server.finish();

    assert!(admitted, "a shared EventQueueGet permit must hold until the batch is decoded and forwarded");
    assert!(forwarded_batch, "the permit must remain held until every decoded event is forwarded");
}

#[tokio::test(flavor = "multi_thread")]
async fn ordinary_requests_run_during_event_queue_polls() {
    let event_server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(seed_grant(&event_url)),
            EventServerResponse::Response(event_batch_response(5, &["First", "Second"])),
            hold_response(),
        ]
    });
    let ordinary_body = "ordinary response";
    let ordinary_server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, ordinary_body)]);
    let budget = CapabilityBudget::new();
    let ordinary_request_capacity = budget.ordinary_requests.available_permits();
    let event_queue_capacity = available_event_queue_permits(&budget);
    let queued_event_capacity = budget.queued_event_bytes.available_permits();
    let client = Client::new_with_budget(event_server.url("/seed"), budget.clone()).expect("active Tokio runtime");
    let (event_sender, mut received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
        .expect("start EventQueue");

    let seed_request = event_server.next_request();
    let first_poll = event_server.next_request();
    assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&first_poll).starts_with("POST /event-queue HTTP/1.1"));
    assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));

    let batch = timeout(CLIENT_TEST_TIMEOUT, received_events.recv())
        .await
        .expect("EventQueue response should be decoded")
        .expect("event channel remains open");
    std::assert_matches!(&batch.event, Event::CapabilityEventBatch { events, .. } if events.len() == 2);
    assert!(budget.queued_event_bytes.available_permits() < queued_event_capacity, "decoded batch must retain its queue reservation");
    let second_poll = event_server.next_request();
    assert_eq!(map_field(&decode_request_body(&second_poll), "ack"), Some(&Value::Integer(5)));
    assert_eq!(available_event_queue_permits(&budget), event_queue_capacity - 1, "long poll holds its concurrency slot");

    let ordinary_url = CapabilityUrl::parse(ordinary_server.url("/ordinary").as_str()).expect("valid ordinary capability URL");
    let ordinary_client = client.clone();
    let ordinary_task =
        spawn(async move { ordinary_client.get_bytes_with_response_limit(&ordinary_url, MAX_CAPABILITY_RESPONSE_BYTES).await });
    assert_eq!(
        timeout(CLIENT_TEST_TIMEOUT, ordinary_task)
            .await
            .expect("ordinary decode should finish")
            .expect("ordinary request task")
            .expect("ordinary response should complete"),
        ordinary_body.as_bytes()
    );
    assert_eq!(available_event_queue_permits(&budget), event_queue_capacity - 1);
    assert_eq!(ordinary_server.finish().len(), 1);

    shutdown.send_replace(true);
    assert_eq!(timeout(CLIENT_TEST_TIMEOUT, task.wait()).await.expect("EventQueue should stop"), EventQueueExit::Stopped);
    assert!(
        budget.queued_event_bytes.available_permits() < queued_event_capacity,
        "the queued batch keeps its retained-event reservation until the envelope is dropped"
    );
    drop(batch);
    assert_eq!(event_server.finish().len(), 0, "the seed and both polls were already observed");
    assert_eq!(budget.ordinary_requests.available_permits(), ordinary_request_capacity);
    assert_eq!(available_event_queue_permits(&budget), event_queue_capacity);
    assert_eq!(budget.queued_event_bytes.available_permits(), queued_event_capacity);

    drop(client);
}

#[tokio::test(flavor = "multi_thread")]
async fn event_poll_wait_cancels_on_close() {
    let server = EventQueueBudgetTestServer::start();
    let budget = CapabilityBudget::new();
    let clients =
        (0..2).map(|_| Client::new_with_budget(server.url("/seed"), budget.clone()).expect("active Tokio runtime")).collect::<Vec<_>>();
    let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let mut tasks = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES);

    for index in 0..MAX_CONCURRENT_EVENT_QUEUES {
        let task = clients[0]
            .start_event_queue_with_role(
                test_simulator_id(index as u64 + 1),
                test_source(),
                events.clone(),
                shutdown_requested.clone(),
                CoverageLossLedger::new(),
                index == 0,
            )
            .expect("start admitted event queue");
        tasks.push(task);
    }
    let polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
    for _ in 0..MAX_CONCURRENT_EVENT_QUEUES {
        server.next_seed();
    }

    let waiting_client = clients[1].clone();
    let waiting_task = waiting_client
        .start_primary_event_queue(
            test_simulator_id(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
            test_source(),
            events,
            shutdown_requested.clone(),
            CoverageLossLedger::new(),
        )
        .expect("start waiting event queue");
    server.next_seed();
    assert!(server.polls.recv_timeout(Duration::from_millis(150)).is_err(), "the waiting queue must not start a poll");

    waiting_client.close_session();
    let waiting_exit = timeout(Duration::from_millis(500), waiting_task.wait()).await;
    shutdown.send_replace(true);
    drop(polls);

    timeout(CLIENT_TEST_TIMEOUT, async {
        for task in tasks {
            assert_eq!(task.wait().await, EventQueueExit::Stopped);
        }
    })
    .await
    .expect("admitted event queues should stop");
    server.stop();
    server.finish();

    assert_eq!(waiting_exit.expect("session close should cancel permit acquisition"), EventQueueExit::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn waiting_event_queue_gets_slot_after_active_poll_stops() {
    let active_server = EventQueueBudgetTestServer::start();
    let waiting_server = EventQueueBudgetTestServer::start();
    let primary_server = EventQueueBudgetTestServer::start();
    let budget = CapabilityBudget::new();
    let active_client = Client::new_with_budget(active_server.url("/seed"), budget.clone()).expect("active Tokio runtime");
    let waiting_client = Client::new_with_budget(waiting_server.url("/seed"), budget.clone()).expect("active Tokio runtime");
    let primary_client = Client::new_with_budget(primary_server.url("/seed"), budget).expect("active Tokio runtime");
    let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let mut active_tasks = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES - 1);

    for index in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
        active_tasks.push(
            active_client
                .start_event_queue(
                    test_simulator_id(index as u64 + 1),
                    test_source(),
                    events.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                )
                .expect("start admitted EventQueue"),
        );
    }
    for _ in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
        active_server.next_seed();
    }
    let mut active_polls = (0..MAX_CONCURRENT_EVENT_QUEUES - 1).map(|_| active_server.next_poll()).collect::<Vec<_>>();

    let waiting_task = waiting_client
        .start_event_queue(
            test_simulator_id(MAX_CONCURRENT_EVENT_QUEUES as u64),
            test_source(),
            events.clone(),
            shutdown_requested.clone(),
            CoverageLossLedger::new(),
        )
        .expect("start waiting EventQueue");
    waiting_server.next_seed();

    let primary_task = primary_client
        .start_primary_event_queue(
            test_simulator_id(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
            test_source(),
            events,
            shutdown_requested.clone(),
            CoverageLossLedger::new(),
        )
        .expect("start primary EventQueue");
    primary_server.next_seed();
    let primary_poll =
        primary_server.polls.recv_timeout(Duration::from_secs(1)).expect("the primary EventQueue uses its reserved poll slot");
    assert!(waiting_server.polls.recv_timeout(Duration::from_millis(150)).is_err(), "neighbor polls cannot take the reserved primary slot");

    active_polls.remove(0).write_all(&response(404, "Not Found", "text/plain", None, "stopped")).expect("stop one active EventQueue poll");
    let released_at = Instant::now();
    let waiting_poll = waiting_server
        .polls
        .recv_timeout(Duration::from_secs(1))
        .expect("a queued simulator should use the released EventQueue slot promptly");
    assert!(released_at.elapsed() < Duration::from_secs(1), "a queued simulator should use the released EventQueue slot promptly");

    shutdown.send_replace(true);
    drop((active_polls, waiting_poll, primary_poll));
    for task in active_tasks {
        assert_ne!(task.wait().await, EventQueueExit::TaskFailed, "active queue should stop without panicking");
    }
    assert_eq!(waiting_task.wait().await, EventQueueExit::Stopped);
    assert_eq!(primary_task.wait().await, EventQueueExit::Stopped);
    active_server.stop();
    waiting_server.stop();
    primary_server.stop();
    active_server.finish();
    waiting_server.finish();
    primary_server.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn primary_event_queue_slot_follows_role_changes() {
    let neighbor_server = EventQueueBudgetTestServer::start();
    let old_primary_server = EventQueueBudgetTestServer::start();
    let new_primary_server = EventQueueBudgetTestServer::start();
    let budget = CapabilityBudget::new();
    let neighbor_client = Client::new_with_budget(neighbor_server.url("/seed"), budget.clone()).expect("active Tokio runtime");
    let old_primary_client = Client::new_with_budget(old_primary_server.url("/seed"), budget.clone()).expect("active Tokio runtime");
    let new_primary_client = Client::new_with_budget(new_primary_server.url("/seed"), budget).expect("active Tokio runtime");
    let (events, _received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let old_coverage_losses = CoverageLossLedger::new();
    let mut old_losses_changed = old_coverage_losses.subscribe();
    let mut neighbors = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES - 1);

    for index in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
        neighbors.push(
            neighbor_client
                .start_event_queue(
                    test_simulator_id(index as u64 + 1),
                    test_source(),
                    events.clone(),
                    shutdown_requested.clone(),
                    CoverageLossLedger::new(),
                )
                .expect("start neighbor EventQueue"),
        );
    }
    for _ in 0..MAX_CONCURRENT_EVENT_QUEUES - 1 {
        neighbor_server.next_seed();
    }
    let neighbor_polls = (0..MAX_CONCURRENT_EVENT_QUEUES - 1).map(|_| neighbor_server.next_poll()).collect::<Vec<_>>();

    let old_primary = old_primary_client
        .start_primary_event_queue(
            test_simulator_id(MAX_CONCURRENT_EVENT_QUEUES as u64),
            test_source(),
            events.clone(),
            shutdown_requested.clone(),
            old_coverage_losses.clone(),
        )
        .expect("start old primary EventQueue");
    old_primary_server.next_seed();
    let mut old_poll = old_primary_server.next_poll();

    let partial_response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{}",
        TWO_EVENT_BATCH.len() + 32,
        TWO_EVENT_BATCH
    );
    old_poll.write_all(partial_response.as_bytes()).expect("send a partial event batch from the old simulator");
    old_primary.role_handle().set_primary(false);
    timeout(Duration::from_secs(1), old_losses_changed.changed())
        .await
        .expect("canceling a live EventQueue poll must record uncertain coverage")
        .expect("coverage ledger remains available");
    assert!(old_coverage_losses.snapshot().losses.iter().any(|loss| {
        matches!(loss, crate::coverage::CoverageLoss::Source(record)
                    if record.watermark.simulator_id == test_simulator_id(MAX_CONCURRENT_EVENT_QUEUES as u64)
                        && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission)
    }));
    let new_primary = new_primary_client
        .start_event_queue(
            test_simulator_id(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1),
            test_source(),
            events,
            shutdown_requested.clone(),
            CoverageLossLedger::new(),
        )
        .expect("start promoted EventQueue");
    new_primary.role_handle().set_primary(true);
    new_primary_server.next_seed();
    let new_poll =
        new_primary_server.polls.recv_timeout(Duration::from_secs(1)).expect("the promoted EventQueue acquires the released primary slot");

    new_primary.role_handle().set_primary(false);
    old_primary.role_handle().set_primary(true);
    let returned_poll =
        old_primary_server.polls.recv_timeout(Duration::from_secs(1)).expect("a re-promoted EventQueue reacquires the primary slot");

    shutdown.send_replace(true);
    drop((neighbor_polls, old_poll, new_poll, returned_poll));
    for task in neighbors {
        assert_ne!(task.wait().await, EventQueueExit::TaskFailed, "neighbor queue should stop without panicking");
    }
    assert_eq!(old_primary.wait().await, EventQueueExit::Stopped);
    assert_eq!(new_primary.wait().await, EventQueueExit::Stopped);
    neighbor_server.stop();
    old_primary_server.stop();
    new_primary_server.stop();
    neighbor_server.finish();
    old_primary_server.finish();
    new_primary_server.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_frees_slot() {
    let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS + 1);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let mut requests = (0..MAX_CONCURRENT_CAPABILITY_REQUESTS)
        .map(|_| {
            let client = client.clone();
            spawn(async move { client.request_capabilities(&["TestCapability"]).await })
        })
        .collect::<Vec<_>>();

    for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
        server.next_request().await;
    }

    let client = client.clone();
    requests.push(spawn(async move { client.request_capabilities(&["TestCapability"]).await }));
    let cancelled = requests.remove(0);
    cancelled.abort();
    assert!(cancelled.await.expect_err("aborted request task").is_cancelled());

    server.next_request().await;

    for request in requests {
        request.abort();
        assert!(request.await.expect_err("aborted queued request task").is_cancelled());
    }
    server.finish();
}
