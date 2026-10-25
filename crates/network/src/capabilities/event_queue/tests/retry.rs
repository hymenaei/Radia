use super::*;

#[tokio::test]
async fn retry_limit_is_terminal() {
    let mut error_count = MAX_EVENT_QUEUE_ERRORS;
    let (events, received_events) = mpsc::channel(1);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let (_session_closed, session_closed) = watch::channel(false);
    let simulator_id = test_simulator_id(1);
    let source = test_source();
    let stop = Stop::new(shutdown_requested, session_closed);
    let mut pending_loss = pending_event_queue_loss(&events, simulator_id, source, &stop);

    let outcome = retry_event_queue_with_hint(
        simulator_id,
        &mut error_count,
        source,
        &events,
        "EventQueueGet returned HTTP 500.".into(),
        None,
        &stop,
        &mut pending_loss,
    )
    .await;
    let Some(Event::CapabilityError { capability, reason, .. }) = received_events.recv().await else {
        panic!("retry exhaustion must report a capability error");
    };

    assert_eq!(outcome, RetryOutcome::LimitReached);
    assert_eq!(capability, "EventQueueGet");
    assert!(reason.contains("repeated errors"));
}

#[tokio::test(flavor = "multi_thread")]
async fn event_queue_retry_limit_records_coverage_before_reporting_error() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        let mut responses =
            vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url)))];
        for _ in 0..=MAX_EVENT_QUEUE_ERRORS {
            responses.push(EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", Some("0"), "try again")));
        }
        responses
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(1);
    event_sender.try_send(EventEnvelope::new(Event::Failed("occupy event channel".into()))).expect("fill event channel");
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let simulator_id = test_simulator_id(1);
    let coverage_losses = CoverageLossLedger::new();
    let mut loss_changes = coverage_losses.subscribe();
    let task = client
        .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
        .expect("start event queue");

    assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
    for _ in 0..=MAX_EVENT_QUEUE_ERRORS {
        let poll = server.next_request();
        assert!(request_line(&poll).starts_with("POST /event-queue HTTP/1.1"));
        assert_eq!(map_field(&decode_request_body(&poll), "ack"), Some(&Value::Undefined));
    }

    let loss = timeout(TEST_TIMEOUT, async {
        loop {
            let snapshot = coverage_losses.snapshot();
            if let [crate::coverage::CoverageLoss::Source(loss)] = snapshot.losses.as_slice()
                && loss.watermark.generation == 1
            {
                break *loss;
            }
            loss_changes.changed().await.expect("coverage ledger remains open");
        }
    })
    .await
    .expect("retry exhaustion records coverage while the event channel is full");
    assert_eq!(loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
    assert_eq!(loss.watermark.simulator_id, simulator_id);

    assert!(matches!(received_events.recv().await, Some(Event::Failed(_))));
    let Some(Event::CapabilityError { capability, reason, .. }) =
        timeout(TEST_TIMEOUT, received_events.recv()).await.expect("retry error is reported")
    else {
        panic!("retry exhaustion must report a capability error");
    };
    assert_eq!(capability, EVENT_QUEUE_NAME);
    assert!(reason.contains("repeated errors"));
    assert!(matches!(
        timeout(TEST_TIMEOUT, received_events.recv()).await.expect("coverage loss is reported"),
        Some(Event::EventQueueCoverageLost { simulator_id: id, source, .. })
            if id == simulator_id && source == test_source()
    ));

    assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("retry limit stops polling"), EventQueueExit::RetryLimitReached);
    assert!(matches!(
        coverage_losses.snapshot().losses.as_slice(),
        [crate::coverage::CoverageLoss::Source(record)]
            if record.watermark.generation == 1
                && record.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                && record.retired
    ));
    assert!(server.finish().is_empty(), "all expected requests were observed and no extra retry should be sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn seed_lookup_retries() {
    let mut server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", None, "try again")),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
        .expect("start event queue");

    assert!(request_line(&server.next_request_async().await).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&server.next_request_async().await).starts_with("POST /seed HTTP/1.1"));
    let poll = server.next_request_async().await;
    assert!(request_line(&poll).starts_with("POST /event-queue HTTP/1.1"), "{}", request_line(&poll));
    task.wait().await;
    assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn rate_limit_retries() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(429, "Too Many Requests", "text/plain", Some("0"), "try again")),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
        .expect("start event queue");

    assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
    task.wait().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn event_queue_retry_after_keeps_ack() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(499, "Client Closed Request", "text/plain", Some("0"), "busy")),
            EventServerResponse::Response(response(429, "Too Many Requests", "text/plain", Some("0"), "busy")),
            EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", Some("0"), "busy")),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "stopped")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let coverage_losses = CoverageLossLedger::new();
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
        .expect("start event queue");

    let seed = server.next_request();
    let first_poll = server.next_request();
    assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
    assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));

    let retry_499_at = Instant::now();
    let after_499_poll = server.next_request();
    assert!(retry_499_at.elapsed() < Duration::from_millis(750), "499 Retry-After: 0 should bypass the default backoff");
    assert_eq!(map_field(&decode_request_body(&after_499_poll), "ack"), Some(&Value::Undefined), "499 retry keeps the previous ACK");
    let retry_429_at = Instant::now();
    let after_429_poll = server.next_request();
    assert!(retry_429_at.elapsed() < Duration::from_millis(750), "Retry-After: 0 should bypass the default one-second backoff");
    assert_eq!(map_field(&decode_request_body(&after_429_poll), "ack"), Some(&Value::Undefined), "429 retry keeps the previous ACK");
    let retry_503_at = Instant::now();
    let after_503_poll = server.next_request();
    assert!(retry_503_at.elapsed() < Duration::from_millis(750), "503 Retry-After: 0 should bypass the default backoff");
    assert_eq!(map_field(&decode_request_body(&after_503_poll), "ack"), Some(&Value::Undefined), "503 retry keeps the previous ACK");

    let batch = timeout(TEST_TIMEOUT, received_events.recv()).await.expect("admitted EventQueue batch").expect("event channel stays open");
    assert!(matches!(batch, Event::CapabilityEventBatch { events, .. } if events.len() == 2));
    let next_poll = server.next_request();
    assert_eq!(
        map_field(&decode_request_body(&next_poll), "ack"),
        Some(&Value::Integer(5)),
        "ACK advances only after full batch admission"
    );

    assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
    assert!(!coverage_losses.snapshot().losses.is_empty(), "a possibly lost poll response must remain durable");
    assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
    assert!(server.finish().is_empty(), "all expected requests were observed and no extra retry should be sent");
}

#[tokio::test(flavor = "multi_thread")]
async fn closed_poll_recovers_without_loss() {
    let (close_sender, close_receiver) = sync::mpsc::channel();
    let mut server = EventQueueTestServer::start(move |base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Controlled(close_receiver),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
            EventServerResponse::Hold,
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let coverage_losses = CoverageLossLedger::new();
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
        .expect("start event queue");

    assert!(request_line(&server.next_request_async().await).starts_with("POST /seed HTTP/1.1"));
    let closed_poll = server.next_request_async().await;
    let closed_poll_body = decode_request_body(&closed_poll);
    assert_eq!(request_line(&closed_poll), "POST /event-queue HTTP/1.1");
    assert_eq!(map_field(&closed_poll_body, "ack"), Some(&Value::Undefined), "{closed_poll_body:?}");
    close_sender.send(Vec::new()).expect("release the unanswered long poll");

    let retry_poll = server.next_request_async().await;
    assert_eq!(map_field(&decode_request_body(&retry_poll), "ack"), Some(&Value::Undefined));
    let batch =
        timeout(TEST_TIMEOUT, received_events.recv()).await.expect("events follow the successful retry").expect("event stream stays open");
    let Event::CapabilityEventBatch { events, .. } = batch else {
        panic!("the successful retry should deliver one event batch");
    };
    assert_eq!(events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>(), ["First", "Second"]);
    assert!(received_events.try_recv().is_err(), "the batch must be delivered once");

    let next_poll = server.next_request_async().await;
    assert_eq!(map_field(&decode_request_body(&next_poll), "ack"), Some(&Value::Integer(5)));
    assert!(coverage_losses.snapshot().losses.is_empty(), "an unanswered poll cannot lose events before advancing ack");
    shutdown.send_replace(true);
    assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown ends the held poll"), EventQueueExit::Stopped);
    assert!(coverage_losses.snapshot().losses.is_empty());
    assert_eq!(server.finish().len(), 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn transient_transport_retries_reach_limit() {
    let body = TWO_EVENT_BATCH;
    let partial = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {}\r\nRetry-After: 0\r\nConnection: close\r\n\r\n{}",
        body.len() + 32,
        body
    )
    .into_bytes();
    let mut server = EventQueueTestServer::start(move |base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        let mut responses =
            vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url)))];
        responses.extend((0..=MAX_EVENT_QUEUE_ERRORS).map(|_| EventServerResponse::Response(partial.clone())));
        responses
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let simulator_id = test_simulator_id(1);
    let coverage_losses = CoverageLossLedger::new();
    let task = client
        .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
        .expect("start event queue");

    assert!(request_line(&server.next_request_async().await).starts_with("POST /seed HTTP/1.1"));
    for _ in 0..=MAX_EVENT_QUEUE_ERRORS {
        let poll = server.next_request_async().await;
        assert_eq!(map_field(&decode_request_body(&poll), "ack"), Some(&Value::Undefined));
    }

    let Some(Event::CapabilityError { reason, .. }) =
        timeout(TEST_TIMEOUT, received_events.recv()).await.expect("retry failure is reported")
    else {
        panic!("retry exhaustion must report a capability error");
    };
    assert!(reason.contains("repeated errors"));
    assert!(matches!(
        timeout(TEST_TIMEOUT, received_events.recv()).await.expect("coverage loss is reported"),
        Some(Event::EventQueueCoverageLost { simulator_id: id, source, reason })
            if id == simulator_id && source == test_source() && reason.contains("request failed")
    ));
    assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("retry limit stops a failing poll"), EventQueueExit::RetryLimitReached);
    assert!(matches!(
        coverage_losses.snapshot().losses.as_slice(),
        [crate::coverage::CoverageLoss::Source(loss)]
            if loss.watermark.simulator_id == simulator_id
                && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                && loss.retired
    ));
    assert_eq!(server.finish().len(), 1 + MAX_EVENT_QUEUE_ERRORS as usize + 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn permanent_poll_error_stops() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(401, "Unauthorized", "text/plain", None, "denied")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let coverage_losses = CoverageLossLedger::new();
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, coverage_losses.clone())
        .expect("start event queue");

    assert!(matches!(
        timeout(TEST_TIMEOUT, received_events.recv()).await.expect("permanent failure reports coverage loss"),
        Some(Event::EventQueueCoverageLost { reason, .. }) if reason.contains("HTTP 401")
    ));
    assert!(received_events.try_recv().is_err(), "permanent poll failures do not emit a second retry event");
    assert_eq!(server.finish().len(), 2, "permanent poll failures do not retry");
    assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
    assert!(!coverage_losses.snapshot().losses.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn seed_transport_retries() {
    let mut server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::DropConnection,
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
        .expect("start event queue");

    assert!(request_line(&server.next_request_async().await).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&server.next_request_async().await).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&server.next_request_async().await).starts_with("POST /event-queue HTTP/1.1"));
    task.wait().await;
    assert!(receive_poll_error(&mut received_events).await.contains("HTTP 404"));
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn permanent_seed_error_stops() {
    let server =
        EventQueueTestServer::start(|_| vec![EventServerResponse::Response(response(401, "Unauthorized", "text/plain", None, "denied"))]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
        .expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("permanent seed failure should be reported")
        .expect("event channel should remain open");
    assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 401")));
    assert_eq!(server.finish().len(), 1, "permanent errors must not be retried");
    task.wait().await;
}
#[tokio::test]
async fn chunked_event_limit() {
    let response = concat!(
        "HTTP/1.1 200 OK\r\n",
        "Transfer-Encoding: chunked\r\n",
        "Connection: close\r\n\r\n",
        "3\r\nabc\r\n",
        "3\r\ndef\r\n",
        "0\r\n\r\n"
    );
    let server = LocalServer::start(vec![response.as_bytes().to_vec()]);
    let client = http::Client::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = match post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 5).await {
        Err(error) => error,
        Ok(_) => panic!("chunked response exceeds limit"),
    };

    assert!(error.to_string().contains("5-byte limit"));
    assert_eq!(server.finish().len(), 1);
}
