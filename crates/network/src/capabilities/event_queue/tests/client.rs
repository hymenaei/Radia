use super::*;
use crate::fixtures::event_queue::{CapabilityHarness, EventQueueHarness};
use crate::fixtures::http::llsd_response;

#[tokio::test(flavor = "multi_thread")]
async fn close_stops_client() {
    let harness = CapabilityHarness::new(|_| vec![hold_response()]);
    let client = harness.client;
    let retained_client = client.clone();
    let request = spawn(async move { retained_client.request_capabilities(&["TestCapability"]).await });
    let _ = harness.server.next_request();

    client.close_session_and_wait().await;
    let error = timeout(TEST_TIMEOUT, request)
        .await
        .expect("closing the session should cancel the HTTP request")
        .expect("capability request task should finish")
        .expect_err("closed session cannot return capabilities");
    assert!(matches!(error, CapabilitySessionClosed));

    let error = client.request_capabilities(&["TestCapability"]).await.expect_err("retained client must reject new requests");
    assert!(matches!(error, CapabilitySessionClosed));
    assert!(harness.server.finish().is_empty(), "no additional request should reach the server after session close");
}

#[tokio::test(flavor = "multi_thread")]
async fn close_cancels_queue() {
    let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let mut requests = (0..=MAX_CONCURRENT_CAPABILITY_REQUESTS)
        .map(|_| {
            let client = client.clone();
            spawn(async move { client.request_capabilities(&["TestCapability"]).await })
        })
        .collect::<Vec<_>>();

    for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
        server.next_request().await;
    }
    yield_now().await;
    client.close_session();

    for request in requests.drain(..) {
        let error = timeout(TEST_TIMEOUT, request)
            .await
            .expect("closing the session should end queued and active requests")
            .expect("capability request task should finish")
            .expect_err("closed session cannot return capabilities");
        assert!(matches!(error, CapabilitySessionClosed));
    }
    assert_eq!(server.finish(), MAX_CONCURRENT_CAPABILITY_REQUESTS, "only requests already in flight should reach the server");
}

#[tokio::test(flavor = "multi_thread")]
async fn event_queue_ack_advances_after_batch_admission() {
    const TWO_EVENT_RESPONSE: &str = "<llsd><map><key>id</key><integer>4</integer><key>events</key><array><map><key>message</key><string>FirstEvent</string></map><map><key>message</key><string>SecondEvent</string></map></array></map></llsd>";
    let mut harness = EventQueueHarness::new(
        |base| {
            let mut event_url = base.clone();
            event_url.set_path("/opaque/event/queue-token");
            vec![
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
                EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_RESPONSE)),
                hold_response(),
            ]
        },
        1,
    );
    let queued_event_capacity = harness.client.budget.queued_event_bytes.available_permits();
    let batch_reservation_floor = queued_event_capacity
        .checked_sub(crate::capabilities::budget::MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES)
        .expect("session queue budget includes one EventQueue batch reservation");
    harness.event_sender.try_send(EventEnvelope::new(Event::Failed("hold first event slot".into()))).expect("fill event channel");
    let task = harness.start_queue_for(test_simulator_id(1), test_source());

    let seed_request = harness.server.next_request();
    let first_poll = harness.server.next_request();
    timeout(TEST_TIMEOUT, async {
        while harness.client.budget.queued_event_bytes.available_permits() <= batch_reservation_floor {
            yield_now().await;
        }
    })
    .await
    .expect("the returned batch should reach admission while the first channel slot is full");
    assert!(matches!(harness.server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)), "ack must wait for batch admission");

    assert!(matches!(harness.received_events.recv().await, Some(Event::Failed(_))));
    let batch = timeout(TEST_TIMEOUT, harness.received_events.recv())
        .await
        .expect("event queue should deliver one complete batch")
        .expect("event channel should remain open");
    let Event::CapabilityEventBatch { source, events, .. } = batch else {
        panic!("EventQueue response must be admitted as one complete batch");
    };
    assert_eq!(source, test_source());
    assert_eq!(events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>(), ["FirstEvent", "SecondEvent"]);
    drop(events);

    let second_poll = harness.server.next_request();
    assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
    assert!(request_body(&seed_request).contains("EventQueueGet"));
    assert!(request_line(&first_poll).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
    let first_poll = decode_request_body(&first_poll);
    assert_eq!(map_field(&first_poll, "ack"), Some(&Value::Undefined));
    assert!(request_line(&second_poll).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
    let second_poll = decode_request_body(&second_poll);
    assert_eq!(map_field(&second_poll, "ack"), Some(&Value::Integer(4)));
    assert_eq!(map_field(&second_poll, "done"), Some(&Value::Boolean(false)));

    harness.shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("event queue should stop after shutdown");
    assert!(harness.server.finish().is_empty(), "shutdown must stop further event polling");
}

#[tokio::test(flavor = "multi_thread")]
async fn zero_delay_502_retries() {
    let mut harness = EventQueueHarness::seeded(
        "/event-queue",
        CLIENT_EVENT_CAPACITY,
        [EventServerResponse::Response(response(502, "Upstream error", "text/plain", Some("0"), "Upstream error: ")), hold_response()],
    );
    let task = harness.start_queue();

    harness.expect_request("/seed");
    let first_poll = harness.expect_request("/event-queue");
    let retry_poll = harness.expect_request("/event-queue");

    harness.shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("event queue should stop after shutdown");
    harness.server.stop();
    assert!(harness.server.finish().is_empty());
    assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
    assert_eq!(map_field(&decode_request_body(&retry_poll), "ack"), Some(&Value::Undefined));
    assert!(harness.received_events.try_recv().is_err(), "empty batches should not emit events");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_batch_not_dispatched() {
    let mut harness = EventQueueHarness::seeded("/events", 1, [EventServerResponse::Response(llsd_response(MALFORMED_BATCH))]);
    harness
        .event_sender
        .try_send(EventEnvelope::new(Event::Failed("hold the only event slot".into())))
        .expect("fill the event channel before the malformed response");
    let mut loss_changes = harness.coverage_losses.subscribe();
    let simulator_id = test_simulator_id(1);
    let source = test_source();
    let task = harness.start_queue_for(simulator_id, source);

    harness.expect_request("/seed");
    harness.expect_request("/events");
    timeout(TEST_TIMEOUT, loss_changes.changed())
        .await
        .expect("durable coverage loss should wake subscribers")
        .expect("coverage ledger remains open");
    assert!(matches!(
        harness.received_events.try_recv(),
        Ok(Event::Failed(reason)) if reason == "hold the only event slot"
    ));
    let notification = timeout(TEST_TIMEOUT, harness.received_events.recv())
        .await
        .expect("loss notification should follow after a slot opens")
        .expect("event channel should remain open");
    assert!(matches!(
        notification,
        Event::EventQueueCoverageLost { simulator_id, source, reason }
            if simulator_id == test_simulator_id(1) && source == test_source() && reason.contains("response")
    ));
    assert!(harness.received_events.try_recv().is_err(), "one queue incarnation emits one loss notification");
    assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss, "an irrecoverable malformed batch stops this queue incarnation");
    assert!(matches!(
        harness.coverage_losses.snapshot().losses.as_slice(),
        [crate::coverage::CoverageLoss::Source(loss)]
            if loss.watermark.simulator_id == test_simulator_id(1)
                && matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. })
                && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                && loss.retired
    ));

    assert!(harness.server.finish().is_empty(), "a malformed batch must stop polling");
}

#[tokio::test(flavor = "multi_thread")]
async fn redelivered_batch_is_suppressed() {
    let mut harness = EventQueueHarness::seeded(
        "/events",
        CLIENT_EVENT_CAPACITY,
        [
            EventServerResponse::Response(llsd_response(TWO_EVENT_BATCH)),
            EventServerResponse::Response(llsd_response(TWO_EVENT_BATCH)),
            hold_response(),
        ],
    );
    let task = harness.start_queue();

    harness.expect_request("/seed");
    let first_poll = harness.expect_request("/events");
    assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
    let first_batch = timeout(TEST_TIMEOUT, harness.received_events.recv())
        .await
        .expect("first response batch should be admitted")
        .expect("event channel should remain open");
    assert!(matches!(
        first_batch,
        Event::CapabilityEventBatch { events, .. }
            if events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]
    ));

    let redelivery_poll = harness.expect_request("/events");
    assert_eq!(map_field(&decode_request_body(&redelivery_poll), "ack"), Some(&Value::Integer(5)));
    let next_poll = harness.expect_request("/events");
    assert_eq!(map_field(&decode_request_body(&next_poll), "ack"), Some(&Value::Integer(5)));
    assert!(harness.received_events.try_recv().is_err(), "the same response id in this queue incarnation must not republish its events");

    harness.shutdown.send_replace(true);
    assert_eq!(timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should stop the held poll"), EventQueueExit::Stopped);
    harness.server.stop();
    assert!(harness.server.finish().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn lost_batch_is_redelivered_on_new_queue_incarnation() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/events");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
            hold_response(),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let simulator_id = test_simulator_id(1);
    let source = test_source();
    let coverage_losses = CoverageLossLedger::new();
    let queued_event_capacity = client.budget.queued_event_bytes.available_permits();
    let batch_reservation_floor = queued_event_capacity
        .checked_sub(crate::capabilities::budget::MAX_EVENT_QUEUE_BATCH_RETAINED_BYTES)
        .expect("session queue budget includes one EventQueue batch reservation");

    let (first_sender, first_receiver) = mpsc::channel(1);
    first_sender
        .try_send(EventEnvelope::new(Event::Failed("hold the first queue slot".into())))
        .expect("fill the first EventQueue channel");
    let mut first_receiver = EventReceiver::new(first_receiver);
    let (first_shutdown, first_shutdown_requested) = watch::channel(false);
    let first_task = client
        .start_event_queue(simulator_id, source, first_sender.clone(), first_shutdown_requested, coverage_losses.clone())
        .expect("start first EventQueue");

    assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
    let first_poll = server.next_request();
    assert!(request_line(&first_poll).starts_with("POST /events HTTP/1.1"));
    assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
    timeout(TEST_TIMEOUT, async {
        while client.budget.queued_event_bytes.available_permits() <= batch_reservation_floor {
            yield_now().await;
        }
    })
    .await
    .expect("the returned batch should reach queue admission while the first channel is full");
    assert_eq!(first_sender.capacity(), 0, "the first queue slot remains occupied");

    first_shutdown.send_replace(true);
    assert_eq!(
        timeout(TEST_TIMEOUT, first_task.wait()).await.expect("the first EventQueue should stop after shutdown"),
        EventQueueExit::Stopped
    );
    assert!(
        matches!(server.requests.try_recv(), Err(sync::mpsc::TryRecvError::Empty)),
        "the unadmitted batch must not advance the ACK or start another poll"
    );
    assert!(matches!(
        first_receiver.try_recv(),
        Ok(Event::Failed(message)) if message == "hold the first queue slot"
    ));
    assert!(first_receiver.try_recv().is_err(), "the first queue must not publish a batch prefix or loss wake-up after shutdown");

    let first_loss = match coverage_losses.snapshot().losses.as_slice() {
        [crate::coverage::CoverageLoss::Source(loss)] => *loss,
        losses => panic!("expected one durable EventQueue coverage loss, got {losses:?}"),
    };
    assert_eq!(first_loss.watermark.simulator_id, simulator_id);
    assert!(matches!(first_loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. }));
    assert_eq!(first_loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
    assert!(first_loss.retired, "stopping the first queue retires its loss record");

    let (second_sender, second_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut second_receiver = EventReceiver::new(second_receiver);
    let (second_shutdown, second_shutdown_requested) = watch::channel(false);
    let second_task = client
        .start_event_queue(simulator_id, source, second_sender, second_shutdown_requested, coverage_losses.clone())
        .expect("start a fresh EventQueue incarnation");

    assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
    let redelivery_poll = server.next_request();
    assert!(request_line(&redelivery_poll).starts_with("POST /events HTTP/1.1"));
    assert_eq!(map_field(&decode_request_body(&redelivery_poll), "ack"), Some(&Value::Undefined));
    let redelivered_batch = timeout(TEST_TIMEOUT, second_receiver.recv())
        .await
        .expect("the fresh queue should admit the redelivered batch")
        .expect("the second EventQueue channel should remain open");
    assert!(matches!(
        redelivered_batch,
        Event::CapabilityEventBatch { simulator_id: id, events, .. }
            if id == simulator_id
                && events.iter().map(|event| event.message.as_str()).collect::<Vec<_>>() == ["First", "Second"]
    ));
    assert!(second_receiver.try_recv().is_err(), "the complete redelivery is published exactly once");

    let held_poll = server.next_request();
    assert_eq!(map_field(&decode_request_body(&held_poll), "ack"), Some(&Value::Integer(5)));
    second_shutdown.send_replace(true);
    assert_eq!(
        timeout(TEST_TIMEOUT, second_task.wait()).await.expect("the second EventQueue should stop cleanly"),
        EventQueueExit::Stopped
    );
    server.stop();
    assert!(server.finish().is_empty(), "all expected seed and poll requests were observed");
    assert!(
        matches!(
            coverage_losses.snapshot().losses.as_slice(),
            [crate::coverage::CoverageLoss::Source(current)]
                if current.watermark == first_loss.watermark && current.retired
        ),
        "the first incarnation's loss remains pending for reconciliation"
    );
}

#[test]
fn bad_event_fields() {
    for body in [
        "<llsd><map><key>events</key><array></array></map></llsd>",
        "<llsd><map><key>id</key><integer>5</integer><key>events</key><string>not an array</string></map></llsd>",
        "<llsd><map><key>id</key><string>five</string><key>events</key><array></array></map></llsd>",
        "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><string>not an event map</string></array></map></llsd>",
        "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>body</key><string>missing message name</string></map></array></map></llsd>",
        "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><integer>5</integer></map></array></map></llsd>",
    ] {
        assert!(decode_event_batch(body.as_bytes()).is_err(), "malformed batch should be rejected");
    }
}

#[test]
fn event_batch_limits() {
    let large_body = "x".repeat(MAX_QUEUED_CAPABILITY_EVENT_BYTES + 1);
    let xml = format!(
        "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>Large</string><key>body</key><string>{large_body}</string></map></array></map></llsd>"
    );
    let error = decode_event_batch(xml.as_bytes()).expect_err("oversized event body should be rejected");
    assert!(error.contains("event exceeded"));

    let one_event = "<map><key>message</key><string>x</string></map>";
    let events = one_event.repeat(1025);
    let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
    let error = decode_event_batch(xml.as_bytes()).expect_err("oversized event batch should be rejected");
    assert!(error.contains("1024-event limit"));

    let events = one_event.repeat(1024);
    let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
    let (_, decoded) = decode_event_batch(xml.as_bytes()).expect("the maximum event count is accepted");
    assert_eq!(decoded.len(), 1024);

    let mut exact_response = b"<llsd><map><key>id</key><integer>5</integer><key>events</key><array/></map></llsd>".to_vec();
    exact_response.resize(MAX_EVENT_QUEUE_RESPONSE_BYTES, b' ');
    let (_, decoded) = decode_event_batch(&exact_response).expect("a response at the byte limit is accepted");
    assert!(decoded.is_empty());

    exact_response.push(b' ');
    let error = decode_event_batch(&exact_response).expect_err("oversized response should be rejected");
    assert!(error.contains("byte limit"));
}

#[test]
fn event_data_limits() {
    let nested = format!("{}<undef/>{}", "<array>".repeat(65), "</array>".repeat(65));
    let xml = format!("<llsd>{nested}</llsd>");
    let error = decode_event_batch(xml.as_bytes()).expect_err("excessive LLSD nesting should be rejected before parsing");
    assert!(error.contains("nesting exceeded"));

    let body = "x".repeat(200_000);
    let one_event = format!("<map><key>message</key><string>Large</string><key>body</key><string>{body}</string></map>");
    let events = one_event.repeat(42);
    let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
    let error = decode_event_batch(xml.as_bytes()).expect_err("aggregate event data should be bounded");
    assert!(error.contains("retained-data limit"));
}

#[test]
fn map_budget() {
    let fields = (0..2_000).map(|index| format!("<key>field{index:04}</key><integer>1</integer>")).collect::<String>();
    let xml = format!(
        "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>CompactMap</string><key>body</key><map>{fields}</map></map></array></map></llsd>"
    );
    let Err(error) = decode_event_batch(xml.as_bytes()) else {
        panic!("decoded map storage should exceed the event memory limit");
    };
    assert!(error.contains("event exceeded"), "{error}");
}

#[test]
fn xml_element_limit() {
    let values = "<undef/>".repeat(MAX_LLSD_XML_ELEMENTS);
    let xml = format!("<llsd><array>{values}</array></llsd>");
    let Err(error) = decode_event_batch(xml.as_bytes()) else {
        panic!("excessive LLSD values should be rejected by decoder limits");
    };
    assert!(error.contains("element limit"));
}

#[test]
fn viewer_retry_backoff() {
    assert_eq!(event_queue_retry_delay(0), Duration::from_secs(1));
    assert_eq!(event_queue_retry_delay(1), Duration::from_secs(4));
    assert_eq!(event_queue_retry_delay(2), Duration::from_secs(7));
    assert_eq!(event_queue_retry_delay(14), Duration::from_secs(43));
}

#[test]
fn long_poll_error_classification() {
    let linden_error = StatusCode::from_u16(499).expect("499 is a valid HTTP status");
    for status in [linden_error, StatusCode::BAD_GATEWAY, StatusCode::SERVICE_UNAVAILABLE, StatusCode::GATEWAY_TIMEOUT] {
        assert!(!expected_empty_poll(status, Duration::from_secs(10) - Duration::from_nanos(1)));
        assert!(expected_empty_poll(status, Duration::from_secs(10)));
    }
    for status in [StatusCode::INTERNAL_SERVER_ERROR, StatusCode::NOT_FOUND, StatusCode::TOO_MANY_REQUESTS] {
        assert!(!expected_empty_poll(status, Duration::from_secs(10)));
        assert!(!expected_empty_poll(status, Duration::from_secs(30)));
    }
    for status in [
        StatusCode::TOO_MANY_REQUESTS,
        linden_error,
        StatusCode::INTERNAL_SERVER_ERROR,
        StatusCode::BAD_GATEWAY,
        StatusCode::SERVICE_UNAVAILABLE,
        StatusCode::GATEWAY_TIMEOUT,
    ] {
        assert!(retryable_event_queue_error(status), "HTTP {status} should retry as an ordinary error");
    }
    for status in [StatusCode::BAD_REQUEST, StatusCode::UNAUTHORIZED, StatusCode::NOT_FOUND] {
        assert!(!retryable_event_queue_error(status), "HTTP {status} should stop this poll incarnation");
    }
    let fast_timeout = http::Error::timeout(Duration::from_secs(9), "fast timeout");
    let long_timeout = http::Error::timeout(Duration::from_secs(10), "long timeout");
    assert!(!fast_timeout.is_timeout_after(Duration::from_secs(10)));
    assert!(long_timeout.is_timeout_after(Duration::from_secs(10)));
}

#[tokio::test]
async fn poll_timeout_is_typed() {
    let server = EventQueueTestServer::start(|_| vec![hold_response()]);
    let client = http::Client::new(Some(Duration::from_millis(25)), false, None).expect("build short-timeout HTTP client");

    let error =
        post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 1024).await.expect_err("held response should time out");
    assert_eq!(error.kind, http::ErrorKind::Timeout);
    assert_eq!(server.finish().len(), 1);
}

#[test]
fn seed_backoff_caps() {
    assert_eq!(seed_capability_retry_delay(1), Duration::from_secs(1));
    assert_eq!(seed_capability_retry_delay(2), Duration::from_secs(2));
    assert_eq!(seed_capability_retry_delay(3), Duration::from_secs(4));
    assert_eq!(seed_capability_retry_delay(4), Duration::from_secs(8));
    assert_eq!(seed_capability_retry_delay(5), Duration::from_secs(8));
    assert_eq!(seed_retry_delay(2, None), Duration::from_secs(2));
    assert_eq!(seed_retry_delay(1, Some(Duration::ZERO)), Duration::ZERO);
    assert_eq!(seed_retry_delay(3, Some(Duration::from_secs(15))), Duration::from_secs(15));
    assert_eq!(seed_retry_delay(1, Some(Duration::from_secs(90))), Duration::from_secs(30));
}

#[tokio::test(flavor = "multi_thread")]
async fn shutdown_cancels_long_poll() {
    let harness = EventQueueHarness::seeded("/held-long-poll", CLIENT_EVENT_CAPACITY, [hold_response()]);
    let task = harness.start_queue();

    let seed = harness.expect_request("/seed");
    assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
    let poll = harness.expect_request("/held-long-poll");
    assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
    harness.shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel the held request");
    assert!(harness.coverage_losses.snapshot().losses.is_empty(), "an intentional shutdown is not a coverage loss");
    assert!(harness.server.finish().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn aborting_poll_records_coverage_loss() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/held-long-poll");
        vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))), hold_response()]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let simulator_id = test_simulator_id(1);
    let coverage_losses = CoverageLossLedger::new();
    let mut loss_changes = coverage_losses.subscribe();
    let task = client
        .start_event_queue(simulator_id, test_source(), event_sender, shutdown_requested, coverage_losses.clone())
        .expect("start event queue");

    let seed = server.next_request();
    assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
    let poll = server.next_request();
    assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
    drop(task);

    let loss = timeout(TEST_TIMEOUT, async {
        loop {
            if let [crate::coverage::CoverageLoss::Source(loss)] = coverage_losses.snapshot().losses.as_slice()
                && loss.retired
            {
                break *loss;
            }
            loss_changes.changed().await.expect("coverage ledger remains open");
        }
    })
    .await
    .expect("aborting an in-flight poll should record and retire its source loss");
    assert_eq!(loss.watermark.simulator_id, simulator_id);
    assert!(matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. }));
    assert_eq!(loss.reason, crate::coverage::CoverageLossReason::EventQueueAdmission);
    let notification = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("aborting a poll should still wake the event receiver")
        .expect("event channel should remain open");
    assert!(matches!(
        notification,
        Event::EventQueueCoverageLost { simulator_id: id, source, reason }
            if id == simulator_id && source == test_source() && reason.contains("canceled")
    ));
    server.stop();
    assert!(server.finish().is_empty(), "the two observed requests were already consumed");
}

#[tokio::test(flavor = "multi_thread")]
async fn loss_notification_waits_for_event_channel_capacity() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/malformed-batch");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, "<llsd><map>")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(1);
    let mut received_events = EventReceiver::new(received_events);
    event_sender
        .send(EventEnvelope::new(Event::Failed("fixture occupying the event channel".into())))
        .await
        .expect("fill the single event slot");
    assert_eq!(event_sender.capacity(), 0);

    let (_shutdown, shutdown_requested) = watch::channel(false);
    let simulator_id = test_simulator_id(1);
    let coverage_losses = CoverageLossLedger::new();
    let mut loss_changes = coverage_losses.subscribe();
    let task = client
        .start_event_queue(simulator_id, test_source(), event_sender.clone(), shutdown_requested, coverage_losses.clone())
        .expect("start event queue");

    assert!(request_line(&server.next_request()).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&server.next_request()).starts_with("POST /malformed-batch HTTP/1.1"));
    let loss = timeout(TEST_TIMEOUT, async {
        loop {
            let snapshot = coverage_losses.snapshot();
            if let [crate::coverage::CoverageLoss::Source(loss)] = snapshot.losses.as_slice()
                && loss.watermark.simulator_id == simulator_id
                && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
            {
                break *loss;
            }
            loss_changes.changed().await.expect("coverage ledger remains open");
        }
    })
    .await
    .expect("the durable loss fact should be recorded while its notification is blocked");
    assert_eq!(event_sender.capacity(), 0, "the wake-up must wait behind the queued event");

    assert!(matches!(
        received_events.recv().await,
        Some(Event::Failed(message)) if message == "fixture occupying the event channel"
    ));
    let notification = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("the loss wake-up should be sent after a slot opens")
        .expect("event channel should remain open");
    assert!(matches!(
        notification,
        Event::EventQueueCoverageLost { simulator_id: id, source, reason }
            if id == simulator_id && source == test_source() && reason.contains("EventQueueGet")
    ));
    assert!(received_events.try_recv().is_err(), "one queue incarnation emits only one loss wake-up");
    assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
    assert!(matches!(
        coverage_losses.snapshot().losses.as_slice(),
        [crate::coverage::CoverageLoss::Source(current)]
            if current.watermark == loss.watermark && current.retired
    ));
    assert!(server.finish().is_empty(), "the observed requests were already consumed");
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_queue_errors() {
    let server = EventQueueTestServer::start(|_| {
        vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, "<llsd><map></map></llsd>"))]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client
        .start_event_queue(test_simulator_id(1), test_source(), event_sender, shutdown_requested, CoverageLossLedger::new())
        .expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("missing capability should be reported")
        .expect("event channel should remain open");
    let Event::CapabilityError { capability, reason, .. } = event else {
        panic!("expected a capability-scoped error");
    };
    assert_eq!(capability, "EventQueueGet");
    assert!(reason.contains("EventQueueGet"));
    assert_eq!(server.finish().len(), 1);
    task.wait().await;

    shutdown.send_replace(true);
}

#[tokio::test(flavor = "multi_thread")]
async fn not_found_stops_polling() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
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

    let reason = receive_poll_error(&mut received_events).await;
    assert!(reason.contains("HTTP 404"));
    assert_eq!(server.finish().len(), 2, "404 must not trigger another poll");
    assert_eq!(task.wait().await, EventQueueExit::StoppedWithCoverageLoss);
    assert!(matches!(
        coverage_losses.snapshot().losses.as_slice(),
        [crate::coverage::CoverageLoss::Source(loss)]
            if matches!(loss.watermark.source, crate::coverage::CoverageSource::EventQueue { .. })
                && loss.reason == crate::coverage::CoverageLossReason::EventQueueAdmission
                && loss.retired
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn primary_event_queue_404_keeps_active_session_connected() {
    let (response_sender, response_receiver) = sync::mpsc::channel();
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Controlled(response_receiver),
        ]
    });
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let agent_id = Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
    let session_id = Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID");
    let session = AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address: simulator.local_addr().expect("simulator address"),
        agent_id,
        session_id,
        circuit_code: 0x1122_3344,
        seed_capability: server.url("/seed"),
    };
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let mut events = EventReceiver::new(event_receiver);
    let (disconnect, disconnect_requested) = watch::channel(false);
    let worker = spawn(run_session(session, commands.clone(), command_receiver, event_sender, disconnect_requested));

    let seed_request = server.next_request();
    let poll_request = server.next_request();
    assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&poll_request).starts_with("POST /event-queue HTTP/1.1"));

    let (_, peer) = receive_udp(&simulator).await;
    simulator.send_to(&acknowledgement(20, 1), peer).await.expect("ack circuit setup");
    let (movement, _) = receive_udp(&simulator).await;
    assert!(matches!(Packet::decode(&movement).expect("decode movement").message(), Message::CompleteAgentMovement(_)));
    simulator.send_to(&acknowledgement(21, 2), peer).await.expect("ack initial movement");

    let mut handshake = test_fixture(vectors::REGION_HANDSHAKE);
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    let (reply, _) = receive_udp(&simulator).await;
    let reply = Packet::decode(&reply).expect("decode handshake reply");
    assert!(matches!(reply.message(), Message::RegionHandshakeReply(_)));
    simulator.send_to(&acknowledgement(23, reply.sequence()), peer).await.expect("ack handshake reply");

    let mut movement_complete = test_fixture(vectors::AGENT_MOVEMENT_COMPLETE);
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 24);
    simulator.send_to(&movement_complete, peer).await.expect("complete agent movement");

    timeout(TEST_TIMEOUT, async {
        loop {
            match events.recv().await.expect("session event stream remains open") {
                Event::SessionReady { .. } => break,
                Event::Failed(reason) => panic!("primary session failed before the EventQueue response: {reason}"),
                _ => continue,
            }
        }
    })
    .await
    .expect("primary session should become ready before the held EventQueue response");

    response_sender
        .send(response(404, "Not Found", "text/plain", None, "not found"))
        .expect("EventQueue request is still waiting for its response");
    timeout(TEST_TIMEOUT, async {
        loop {
            match events.recv().await.expect("active session event stream remains open") {
                Event::EventQueueCoverageLost { .. } => break,
                Event::Failed(reason) => panic!("EventQueue 404 must not end the active session: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("EventQueue coverage loss should be reported");
    timeout(TEST_TIMEOUT, async {
        loop {
            match events.recv().await.expect("active session event stream remains open") {
                Event::CapabilityError { capability, reason, .. } if capability == EVENT_QUEUE_NAME && reason.contains("HTTP 404") => {
                    break;
                }
                Event::Failed(reason) => panic!("EventQueue 404 must not end the active session: {reason}"),
                _ => {}
            }
        }
    })
    .await
    .expect("EventQueue capability failure should be reported");

    let chat = Message::ChatFromViewer(ChatFromViewer {
        agent_data: chat_from_viewer::AgentData { agent_id, session_id },
        chat_data: chat_from_viewer::ChatData { message: "still connected".into(), chat_type: ChatType::Normal, channel: 0 },
    });
    let send_result = commands.try_send(Command::SendMessage { message: chat, delivery: Delivery::Reliable });
    let message = receive_message(&simulator, |message| matches!(message, Message::ChatFromViewer(_) | Message::LogoutRequest(_))).await;
    if matches!(message.message(), Message::LogoutRequest(_)) {
        simulator.send_to(&acknowledgement(25, message.sequence()), peer).await.expect("ack unexpected active simulator logout");
        let mut logout_reply = test_fixture(vectors::LOGOUT_REPLY);
        logout_reply[0] |= 0x40;
        set_sequence(&mut logout_reply, 26);
        simulator.send_to(&logout_reply, peer).await.expect("reply to unexpected active simulator logout");
        let result = timeout(TEST_TIMEOUT, worker).await.expect("failed session should stop").expect("session task");
        assert!(result.is_err(), "the pre-fix runtime treats EventQueue loss as terminal");
        panic!("EventQueue 404 ended the active viewer session");
    }
    send_result.expect("active session must continue accepting simulator commands");
    assert!(matches!(
        message.message(),
        Message::ChatFromViewer(chat) if chat.chat_data.message == "still connected"
    ));
    simulator.send_to(&acknowledgement(25, message.sequence()), peer).await.expect("ack post-failure chat");

    disconnect.send_replace(true);
    let logout = receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    simulator.send_to(&acknowledgement(26, logout.sequence()), peer).await.expect("ack explicit active simulator logout");
    let mut logout_reply = test_fixture(vectors::LOGOUT_REPLY);
    logout_reply[0] |= 0x40;
    set_sequence(&mut logout_reply, 27);
    simulator.send_to(&logout_reply, peer).await.expect("reply to explicit active simulator logout");
    let result = timeout(TEST_TIMEOUT, worker).await.expect("explicit logout should stop the session").expect("session task");
    assert!(result.is_ok(), "explicit logout is a normal session end");
    assert!(server.finish().is_empty(), "404 must end polling without another request");
}
