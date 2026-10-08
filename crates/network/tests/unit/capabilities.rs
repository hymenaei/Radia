use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    str,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc as std_mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use super::http::{LocalServer, response};
use crate::{
    AgentSession, Error as NetworkError, Event, Grid, NetworkStatusHandle,
    capabilities::{CapabilityBudget, CapabilityUrl, Client},
    capabilities::{
        EventQueueExit, MAX_CAPABILITY_REQUEST_BODY_BYTES, MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS,
        MAX_CONCURRENT_EVENT_QUEUES, MAX_EVENT_QUEUE_ERRORS, MAX_EVENT_QUEUE_RESPONSE_BYTES, MAX_LLSD_XML_ELEMENTS,
        MAX_QUEUED_CAPABILITY_EVENT_BYTES, MAX_SEED_CAPABILITY_RESPONSE_BYTES, RetryOutcome, decode_event_batch, event_queue_retry_delay,
        expected_empty_poll, post_event_queue, retry_event_queue, seed_capability_retry_delay, seed_retry_delay,
    },
    event::{EventEnvelope, EventReceiver, SimulatorId},
    http::{Client as HttpClient, Error},
};
use base::UUID;
use llsd::{Map, Value};
use protocol::{
    messages::packet_ack::{PacketAck, Packets},
    udp::{Message, Packet},
};
use serde::{Deserialize, Serialize};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, watch},
    task::JoinSet,
    time::timeout,
};
use url::Url;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn debug_redacts_seed() {
    let seed = Url::parse("https://cap.example/secret-token").expect("valid seed capability URL");
    let client = Client::new(seed).expect("active Tokio runtime");

    let debug = format!("{client:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("secret-token"));
    let event = Event::SimulatorCapabilities { simulator_id: SimulatorId::new(1), source: test_source(), capabilities: client };
    let debug = format!("{event:?}");
    assert!(!debug.contains("secret-token"));
}

#[tokio::test]
async fn errors_redact_urls() {
    let seed = Url::parse("https://user:password@caps.example/secret-token").expect("valid URL with rejected userinfo");
    let error = Client::new(seed).expect_err("embedded credentials should be rejected");
    let debug = format!("{error:?}");

    assert!(matches!(error, NetworkError::Capability(_)));
    assert!(!debug.contains("user"));
    assert!(!debug.contains("password"));
    assert!(!debug.contains("secret-token"));
}

#[tokio::test]
async fn discovered_urls_are_opaque() {
    let body = "<llsd><map><key>TestCapability</key><string>https://caps.example/opaque-token</string></map></llsd>";
    let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let capabilities = client.request_capabilities(&["TestCapability"]).await.expect("capability discovery");
    let capability = capabilities.get("TestCapability").expect("discovered capability");
    let debug = format!("{capability:?}");
    let with_query = capability.with_query_pair("asset_id", "private-value");

    assert!(debug.contains("REDACTED"));
    assert!(!debug.contains("opaque-token"));
    assert!(!format!("{with_query:?}").contains("private-value"));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn typed_get_and_post() {
    #[derive(Deserialize, Debug, Eq, PartialEq)]
    struct Reply {
        count: i32,
    }

    #[derive(Serialize)]
    struct Request {
        count: i32,
    }

    let body = "<?xml version=\"1.0\"?><llsd><map><key>count</key><integer>7</integer></map></llsd>";
    let server = LocalServer::start(vec![
        response(200, "OK", "application/llsd+xml", None, body),
        response(200, "OK", "application/llsd+xml", None, body),
    ]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");

    let posted: Reply = client.post(&url, &Request { count: 7 }).await.expect("typed POST response");
    let fetched: Reply = client.get(&url).await.expect("typed GET response");
    let requests = server.finish();

    assert_eq!(posted, Reply { count: 7 });
    assert_eq!(fetched, Reply { count: 7 });
    let post = str::from_utf8(&requests[0]).expect("POST is UTF-8");
    assert!(post.starts_with("POST /capability HTTP/1.1"));
    let (_, post_body) = post.split_once("\r\n\r\n").expect("POST has a body");
    let payload: Value = llsd::xml::from_str(post_body).expect("POST body is valid LLSD XML");
    assert_eq!(payload.as_map().and_then(|map| map.get("count")), Some(&Value::Integer(7)));
    assert!(str::from_utf8(&requests[1]).expect("GET is UTF-8").starts_with("GET /capability HTTP/1.1"));
}

#[tokio::test]
async fn partial_grants() {
    let body = concat!("<llsd><map><key>GrantedCapability</key>", "<uri>http://caps.example/granted</uri></map></llsd>");
    let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
    let seed = server.url("/seed");

    let client = Client::new(seed.clone()).expect("active Tokio runtime");
    let capabilities = client.request_capabilities(&["GrantedCapability", "MissingCapability"]).await.expect("partial grant response");

    assert_eq!(capabilities.len(), 1);
    assert!(capabilities.contains_key("GrantedCapability"));
    let request = server.finish().remove(0);
    let request = str::from_utf8(&request).expect("request is UTF-8");
    let (headers, body) = request.split_once("\r\n\r\n").expect("request has a body");

    assert!(headers.starts_with("POST /seed HTTP/1.1"));
    assert!(headers.to_ascii_lowercase().contains("application/llsd+xml"));
    assert!(body.contains("GrantedCapability"));
    assert!(body.contains("MissingCapability"));
}

#[tokio::test]
async fn empty_grants() {
    let server =
        LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, "<?xml version=\"1.0\"?><llsd><map></map></llsd>")]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let capabilities = client.request_capabilities(&["MissingCapability"]).await.expect("empty grants are a valid response");

    assert!(capabilities.is_empty());
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn seed_limit_is_enforced() {
    let server = LocalServer::start(vec![content_length_only_response(MAX_SEED_CAPABILITY_RESPONSE_BYTES + 1)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let error = client.request_capabilities(&["TestCapability"]).await.expect_err("oversized seed response should fail");
    assert!(matches!(&error, NetworkError::Capability(_)));

    assert!(error.to_string().contains(&format!("{}-byte limit", MAX_SEED_CAPABILITY_RESPONSE_BYTES)));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn response_limit_is_enforced() {
    let server = LocalServer::start(vec![content_length_only_response(MAX_CAPABILITY_RESPONSE_BYTES + 1)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
    let error = client.post::<_, Value>(&url, &Value::Undefined).await.expect_err("oversized LLSD response should fail");
    assert!(matches!(&error, NetworkError::Capability(_)));

    assert!(error.to_string().contains(&format!("{}-byte limit", MAX_CAPABILITY_RESPONSE_BYTES)));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn request_body_is_bounded() {
    let server = LocalServer::start(vec![]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
    let payload = Value::String("N".repeat(MAX_CAPABILITY_REQUEST_BODY_BYTES + 1));

    let error = client.post::<_, Value>(&url, &payload).await.expect_err("an oversized request body should fail");

    assert!(matches!(error, NetworkError::Capability(_)));
    assert!(server.finish().is_empty(), "an oversized request must not be sent");
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
    let client = HttpClient::new(Some(Duration::from_secs(2)), false, None).expect("build HTTP client");

    let error = match post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 5).await {
        Err(error) => error,
        Ok(_) => panic!("chunked response exceeds limit"),
    };

    assert!(error.to_string().contains("5-byte limit"));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn bad_cap_urls_are_rejected() {
    let body = concat!("<llsd><map><key>TestCapability</key><string>", "ftp://caps.example.net/capability", "</string></map></llsd>");
    let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let error = client.request_capabilities(&["TestCapability"]).await.expect_err("FTP capability URLs are not supported");
    assert!(matches!(&error, NetworkError::Capability(_)));
    assert!(error.to_string().contains("invalid TestCapability capability"));
    server.finish();
}

#[tokio::test]
async fn capability_names_are_bounded() {
    let server = LocalServer::start(vec![
        response(200, "OK", "application/llsd+xml", None, "<llsd><map/></llsd>"),
        response(200, "OK", "application/llsd+xml", None, "<llsd><map/></llsd>"),
    ]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let maximum_count = vec!["N"; 256];
    assert!(
        client.request_capabilities(&maximum_count).await.expect("256 names at the count limit are accepted").is_empty(),
        "an empty seed response grants no capabilities"
    );

    let too_many = vec!["N"; 257];
    let error = client.request_capabilities(&too_many).await.expect_err("too many names should be rejected");
    assert!(error.to_string().contains("at most 256 names"));

    let long_name = "N".repeat(129);
    let error = client.request_capabilities(&[&long_name]).await.expect_err("a long name should be rejected");
    assert!(error.to_string().contains("128-byte limit"));

    let total_names = vec!["N".repeat(128); 129];
    let total_name_refs = total_names.iter().map(String::as_str).collect::<Vec<_>>();
    let error = client.request_capabilities(&total_name_refs).await.expect_err("the total name bytes should be rejected");
    assert!(error.to_string().contains("16384-byte total limit"));

    let maximum_name = "N".repeat(128);
    assert!(
        client.request_capabilities(&[&maximum_name]).await.expect("a name at the byte limit is accepted").is_empty(),
        "an empty seed response grants no capabilities"
    );
    assert_eq!(server.finish().len(), 2, "invalid names must not send a request");
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_limit_concurrency() {
    let request_count = MAX_CONCURRENT_CAPABILITY_REQUESTS * 2;
    let server = ConcurrentCapabilityTestServer::start(request_count, Duration::from_millis(75));
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let mut requests = JoinSet::new();

    for _ in 0..request_count {
        let client = client.clone();
        requests.spawn(async move { client.request_capabilities(&["TestCapability"]).await });
    }

    timeout(TEST_TIMEOUT, async {
        while let Some(request) = requests.join_next().await {
            request.expect("capability request task should finish").expect("local seed request should succeed");
        }
    })
    .await
    .expect("bounded capability requests should finish");

    let maximum_active = server.finish();
    assert!(maximum_active > 1, "capability requests should remain concurrent");
    assert!(maximum_active <= MAX_CONCURRENT_CAPABILITY_REQUESTS, "too many HTTP requests ran at once");
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_budget_bounds_requests() {
    let request_count = MAX_CONCURRENT_CAPABILITY_REQUESTS * 2;
    let server = ConcurrentCapabilityTestServer::start(request_count, Duration::from_millis(150));
    let budget = CapabilityBudget::new();
    let clients = (0..2)
        .map(|_| {
            Client::new_with_status_and_budget(server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime")
        })
        .collect::<Vec<_>>();
    let mut requests = JoinSet::new();

    for index in 0..request_count {
        let client = clients[index % clients.len()].clone();
        requests.spawn(async move { client.request_capabilities(&["TestCapability"]).await });
    }

    timeout(TEST_TIMEOUT, async {
        while let Some(request) = requests.join_next().await {
            request.expect("capability request task should finish").expect("local seed request should succeed");
        }
    })
    .await
    .expect("shared budget requests should finish");

    let maximum_active = server.finish();
    assert!(maximum_active > 1, "the shared budget should allow concurrent requests");
    assert!(maximum_active <= MAX_CONCURRENT_CAPABILITY_REQUESTS, "shared clients exceeded their request budget");
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_budget_bounds_event_polls() {
    let task_count = MAX_CONCURRENT_EVENT_QUEUES + 1;
    let server = EventQueueBudgetTestServer::start();
    let budget = CapabilityBudget::new();
    let clients = (0..2)
        .map(|_| {
            Client::new_with_status_and_budget(server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime")
        })
        .collect::<Vec<_>>();
    let (events, received_events) = mpsc::channel(1);
    let mut received_events = EventReceiver::new(received_events);
    events.try_send(EventEnvelope::new(Event::SendFailed("hold event forwarding".into()))).expect("fill the application event channel");
    let (shutdown, shutdown_requested) = watch::channel(false);
    let mut tasks = Vec::with_capacity(task_count);

    for index in 0..task_count {
        let client = clients[index % clients.len()].clone();
        tasks.push(
            client
                .start_event_queue(SimulatorId::new(index as u64 + 1), test_source(), events.clone(), shutdown_requested.clone())
                .expect("start event queue"),
        );
    }

    let mut polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
    for _ in 0..task_count {
        server.next_seed();
    }
    let unexpected_poll = server.polls.recv_timeout(Duration::from_millis(150)).ok();
    let admitted = unexpected_poll.is_none();
    let mut forwarded_batch = false;

    if admitted {
        write_response(polls.pop().expect("one event poll to release"), response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH));
        assert!(matches!(
            timeout(TEST_TIMEOUT, received_events.recv()).await.expect("held event channel item").expect("event channel remains open"),
            Event::SendFailed(message) if message == "hold event forwarding"
        ));
        timeout(TEST_TIMEOUT, async {
            while received_events.is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("first event should fill the application channel");
        let no_poll_while_forwarding = server.polls.recv_timeout(Duration::from_millis(150)).is_err();
        let first_event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("first decoded event should be forwarded")
            .expect("event channel remains open");
        assert!(matches!(first_event, Event::CapabilityEvent { message, .. } if message == "First"));
        polls.push(server.next_poll());
        let second_event = timeout(TEST_TIMEOUT, received_events.recv())
            .await
            .expect("second decoded event should be forwarded")
            .expect("event channel remains open");
        forwarded_batch = no_poll_while_forwarding && matches!(second_event, Event::CapabilityEvent { message, .. } if message == "Second");
    } else if let Some(poll) = unexpected_poll {
        polls.push(poll);
    }

    shutdown.send_replace(true);
    drop(polls);

    timeout(TEST_TIMEOUT, async {
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
async fn event_poll_wait_cancels_on_close() {
    let server = EventQueueBudgetTestServer::start();
    let budget = CapabilityBudget::new();
    let clients = (0..2)
        .map(|_| {
            Client::new_with_status_and_budget(server.url("/seed"), None, NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime")
        })
        .collect::<Vec<_>>();
    let (events, _received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let mut tasks = Vec::with_capacity(MAX_CONCURRENT_EVENT_QUEUES);

    for index in 0..MAX_CONCURRENT_EVENT_QUEUES {
        tasks.push(
            clients[0]
                .start_event_queue(SimulatorId::new(index as u64 + 1), test_source(), events.clone(), shutdown_requested.clone())
                .expect("start admitted event queue"),
        );
    }
    let polls = (0..MAX_CONCURRENT_EVENT_QUEUES).map(|_| server.next_poll()).collect::<Vec<_>>();
    for _ in 0..MAX_CONCURRENT_EVENT_QUEUES {
        server.next_seed();
    }

    let waiting_client = clients[1].clone();
    let waiting_task = waiting_client
        .start_event_queue(SimulatorId::new(MAX_CONCURRENT_EVENT_QUEUES as u64 + 1), test_source(), events, shutdown_requested.clone())
        .expect("start waiting event queue");
    server.next_seed();
    assert!(server.polls.recv_timeout(Duration::from_millis(150)).is_err(), "the waiting queue must not start a poll");

    waiting_client.close_session();
    let waiting_exit = timeout(Duration::from_millis(500), waiting_task.wait()).await;
    shutdown.send_replace(true);
    drop(polls);

    timeout(TEST_TIMEOUT, async {
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
async fn cancel_frees_slot() {
    let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS + 1, Duration::from_secs(1));
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let mut requests = (0..MAX_CONCURRENT_CAPABILITY_REQUESTS)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.request_capabilities(&["TestCapability"]).await })
        })
        .collect::<Vec<_>>();

    for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
        server.next_request().await;
    }

    let client = client.clone();
    requests.push(tokio::spawn(async move { client.request_capabilities(&["TestCapability"]).await }));
    let cancelled = requests.remove(0);
    cancelled.abort();
    let _ = cancelled.await;

    timeout(Duration::from_millis(750), server.next_request()).await.expect("the canceled request should release its concurrency slot");

    for request in requests {
        request.abort();
        let _ = request.await;
    }
    server.finish();
}

#[tokio::test(flavor = "multi_thread")]
async fn close_stops_client() {
    let server = EventQueueTestServer::start(|_| vec![hold_response()]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let retained_client = client.clone();
    let request = tokio::spawn(async move { retained_client.request_capabilities(&["TestCapability"]).await });
    let _ = server.next_request();

    client.close_session();
    let error = timeout(TEST_TIMEOUT, request)
        .await
        .expect("closing the session should cancel the HTTP request")
        .expect("capability request task should finish")
        .expect_err("closed session cannot return capabilities");
    assert!(matches!(error, NetworkError::CapabilitySessionClosed));

    let error = client.request_capabilities(&["TestCapability"]).await.expect_err("retained client must reject new requests");
    assert!(matches!(error, NetworkError::CapabilitySessionClosed));
    assert!(server.finish().is_empty(), "no additional request should reach the server after session close");
}

#[tokio::test(flavor = "multi_thread")]
async fn close_cancels_queue() {
    let mut server = ConcurrentCapabilityTestServer::start(MAX_CONCURRENT_CAPABILITY_REQUESTS, Duration::from_secs(1));
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let mut requests = (0..=MAX_CONCURRENT_CAPABILITY_REQUESTS)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.request_capabilities(&["TestCapability"]).await })
        })
        .collect::<Vec<_>>();

    for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
        server.next_request().await;
    }
    tokio::task::yield_now().await;
    client.close_session();

    for request in requests.drain(..) {
        let error = timeout(TEST_TIMEOUT, request)
            .await
            .expect("closing the session should end queued and active requests")
            .expect("capability request task should finish")
            .expect_err("closed session cannot return capabilities");
        assert!(matches!(error, NetworkError::CapabilitySessionClosed));
    }
    assert_eq!(server.finish(), MAX_CONCURRENT_CAPABILITY_REQUESTS, "only requests already in flight should reach the server");
}

#[tokio::test(flavor = "multi_thread")]
async fn event_delivery_acks_batch() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/opaque/event/queue-token");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, EVENT_RESPONSE)),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("event queue should deliver the batch")
        .expect("event channel should remain open");
    let Event::CapabilityEvent { source, message, body, .. } = event else {
        panic!("expected a generic capability event");
    };
    assert_eq!(source, test_source());
    assert_eq!(message, "ViewerSpecificMessage");
    assert_eq!(body, Value::Map(Map::from([("value".to_owned(), Value::Array(vec![Value::Integer(12), Value::Boolean(true)]),)])));

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_body(&requests[0]).contains("EventQueueGet"));
    assert!(request_line(&requests[1]).starts_with("POST /opaque/event/queue-token HTTP/1.1"));
    let first_poll = decode_request_body(&requests[1]);
    assert_eq!(map_field(&first_poll, "ack"), Some(&Value::Undefined));
    assert_eq!(map_field(&first_poll, "done"), Some(&Value::Boolean(false)));

    shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("event queue should stop after shutdown");
}

#[tokio::test(flavor = "multi_thread")]
async fn event_queue_starts_neighbor() {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let neighbor = UdpSocket::bind("127.0.0.1:0").await.expect("bind neighbor simulator");
    let primary_endpoint = primary.local_addr().expect("primary simulator address");
    let neighbor_endpoint = neighbor.local_addr().expect("neighbor simulator address");
    let (ip, port) = match neighbor_endpoint {
        SocketAddr::V4(endpoint) => (endpoint.ip().octets(), endpoint.port()),
        SocketAddr::V6(_) => panic!("fake neighbor should use IPv4"),
    };
    let agent_id = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
    let neighbor_seed_server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/neighbor-event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "stopped")),
        ]
    });
    let neighbor_seed = neighbor_seed_server.url("/neighbor-seed");
    let server = EventQueueTestServer::start(move |base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        let seed = seed_response(&event_url);
        let event = simulator_events(ip, port, agent_id, &neighbor_seed);
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed)),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &event)),
            hold_response(),
        ]
    });

    let session = AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address: primary_endpoint,
        agent_id,
        session_id: UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID"),
        circuit_code: 0x1122_3344,
        seed_capability: server.url("/seed"),
    };
    let (_commands, command_receiver) = mpsc::channel(crate::client::COMMAND_CAPACITY);
    let (event_sender, received_events) =
        mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY + crate::event::PRIMARY_TERMINAL_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (disconnect, shutdown) = watch::channel(false);
    let worker = tokio::spawn(crate::simulator::run_session(session, command_receiver, event_sender, shutdown));

    let (primary_packet, primary_peer) = receive_udp(&primary).await;
    assert!(matches!(Packet::decode(&primary_packet).expect("decode initial circuit").message(), Message::UseCircuitCode(_)));
    primary.send_to(&acknowledgement(40, 1), primary_peer).await.expect("ack primary circuit");
    let (movement_packet, movement_peer) = receive_udp(&primary).await;
    let movement_packet = Packet::decode(&movement_packet).expect("decode initial movement");
    assert!(matches!(movement_packet.message(), Message::CompleteAgentMovement(_)));
    primary.send_to(&acknowledgement(41, movement_packet.sequence()), movement_peer).await.expect("ack initial movement");
    let (neighbor_packet, neighbor_peer) = receive_udp(&neighbor).await;
    let neighbor_packet = Packet::decode(&neighbor_packet).expect("decode neighbor circuit");
    assert_eq!(neighbor_packet.sequence(), 1);
    assert!(neighbor_packet.is_reliable());
    assert!(matches!(neighbor_packet.message(), Message::UseCircuitCode(_)));
    neighbor.send_to(&acknowledgement(40, 1), neighbor_peer).await.expect("ack neighbor circuit");

    let mut saw_enable = false;
    let mut saw_establish = false;
    let mut saw_capabilities = false;
    while !(saw_enable && saw_establish && saw_capabilities) {
        let event = timeout(TEST_TIMEOUT, received_events.recv()).await.expect("forward capability events").expect("network event channel");
        match event {
            Event::CapabilityEvent { message, .. } if message == "EnableSimulator" => saw_enable = true,
            Event::CapabilityEvent { message, body, .. } if message == "EstablishAgentCommunication" => {
                saw_establish = true;
                assert!(matches!(
                    body.as_map().and_then(|fields| fields.get("seed-capability")),
                    Some(Value::String(seed)) if seed == "[redacted]"
                ));
            }
            Event::SimulatorCapabilities { source, .. } if source == neighbor_endpoint => saw_capabilities = true,
            _ => {}
        }
    }
    assert!(request_line(&neighbor_seed_server.next_request()).starts_with("POST /neighbor-seed HTTP/1.1"));

    disconnect.send_replace(true);
    let (logout, _) = receive_udp(&primary).await;
    assert!(matches!(Packet::decode(&logout).expect("decode logout").message(), Message::LogoutRequest(_)));
    timeout(TEST_TIMEOUT, worker).await.expect("network worker shutdown").expect("network task").expect("clean shutdown");
    server.stop();
    let requests = server.finish();
    assert!(requests.len() >= 2, "fake received seed lookup and event poll");
    neighbor_seed_server.stop();
    let _ = neighbor_seed_server.finish();
    assert_ne!(primary_peer, neighbor_peer);
}

#[tokio::test(flavor = "multi_thread")]
async fn backpressure_stops_poll() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, TWO_EVENT_BATCH)),
            hold_response(),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(1);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let seed_request = server.next_request();
    let poll_request = server.next_request();
    assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&poll_request).starts_with("POST /event-queue HTTP/1.1"));
    let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
    while tokio::time::Instant::now() < deadline {
        assert!(matches!(server.requests.try_recv(), Err(std_mpsc::TryRecvError::Empty)), "poller requested another batch while blocked");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel a blocked event send");
    assert!(matches!(received_events.try_recv(), Ok(Event::CapabilityEvent { message, .. }) if message == "First"));
    server.stop();
    assert!(server.finish().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn fast_502_retries() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(502, "Upstream error", "text/plain", None, "Upstream error: ")),
            hold_response(),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let seed_request = server.next_request();
    let first_poll = server.next_request();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(matches!(server.requests.try_recv(), Err(std_mpsc::TryRecvError::Empty)), "a fast 502 should use retry backoff");

    shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("retry backoff should stop on shutdown");
    server.stop();
    assert!(server.finish().is_empty());
    assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&first_poll).starts_with("POST /event-queue HTTP/1.1"));
    assert_eq!(map_field(&decode_request_body(&first_poll), "ack"), Some(&Value::Undefined));
    assert!(received_events.try_recv().is_err(), "empty batches should not emit events");
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_batch_not_dispatched() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/events");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, MALFORMED_BATCH)),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert!(received_events.try_recv().is_err(), "a malformed batch must not be dispatched");

    shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("retry delay should be cancelable");
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
    let error = decode_event_batch(xml.as_bytes()).err().expect("oversized event body should be rejected");
    assert!(error.contains("event exceeded"));

    let one_event = "<map><key>message</key><string>x</string></map>";
    let events = one_event.repeat(1025);
    let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
    let error = decode_event_batch(xml.as_bytes()).err().expect("oversized event batch should be rejected");
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
    let error = decode_event_batch(&exact_response).err().expect("oversized response should be rejected");
    assert!(error.contains("byte limit"));
}

#[test]
fn event_data_limits() {
    let nested = format!("{}<undef/>{}", "<array>".repeat(65), "</array>".repeat(65));
    let xml = format!("<llsd>{nested}</llsd>");
    let error = decode_event_batch(xml.as_bytes()).err().expect("excessive LLSD nesting should be rejected before parsing");
    assert!(error.contains("nesting exceeded"));

    let body = "x".repeat(200_000);
    let one_event = format!("<map><key>message</key><string>Large</string><key>body</key><string>{body}</string></map>");
    let events = one_event.repeat(42);
    let xml = format!("<llsd><map><key>id</key><integer>5</integer><key>events</key><array>{events}</array></map></llsd>");
    let error = decode_event_batch(xml.as_bytes()).err().expect("aggregate event data should be bounded");
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
        panic!("excessive LLSD values should be rejected during preflight");
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
    let linden_internal_error = reqwest::StatusCode::from_u16(499).expect("499 is a valid HTTP status");
    for status in [
        linden_internal_error,
        reqwest::StatusCode::BAD_GATEWAY,
        reqwest::StatusCode::SERVICE_UNAVAILABLE,
        reqwest::StatusCode::GATEWAY_TIMEOUT,
    ] {
        assert!(!expected_empty_poll(status, Duration::from_secs(9)));
        assert!(expected_empty_poll(status, Duration::from_secs(10)));
    }
    for status in [reqwest::StatusCode::INTERNAL_SERVER_ERROR, reqwest::StatusCode::NOT_FOUND, reqwest::StatusCode::TOO_MANY_REQUESTS] {
        assert!(!expected_empty_poll(status, Duration::from_secs(30)));
    }
    let fast_timeout = Error::Timeout { elapsed: Duration::from_secs(9), message: String::new() };
    let long_timeout = Error::Timeout { elapsed: Duration::from_secs(10), message: String::new() };
    assert!(!fast_timeout.is_timeout_after(Duration::from_secs(10)));
    assert!(long_timeout.is_timeout_after(Duration::from_secs(10)));
}

#[tokio::test]
async fn poll_timeout_is_typed() {
    let server = EventQueueTestServer::start(|_| vec![hold_response()]);
    let client = HttpClient::new(Some(Duration::from_millis(25)), false, None).expect("build short-timeout HTTP client");

    let error =
        post_event_queue(&client, &server.url("/event-queue"), &Value::Undefined, 1024).await.expect_err("held response should time out");
    assert!(error.is_timeout());
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
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/held-long-poll");
        vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))), hold_response()]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, _received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let seed = server.next_request();
    assert!(request_line(&seed).starts_with("POST /seed HTTP/1.1"));
    let poll = server.next_request();
    assert!(request_line(&poll).starts_with("POST /held-long-poll HTTP/1.1"));
    shutdown.send_replace(true);
    timeout(TEST_TIMEOUT, task.wait()).await.expect("shutdown should cancel the held request");
    assert!(server.finish().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn unavailable_queue_errors() {
    let server = EventQueueTestServer::start(|_| {
        vec![EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, "<llsd><map></map></llsd>"))]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

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
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("404 should stop event queue polling")
        .expect("event channel should remain open");
    let Event::CapabilityError { capability, reason, .. } = event else {
        panic!("expected a capability error");
    };
    assert_eq!(capability, "EventQueueGet");
    assert!(reason.contains("HTTP 404"));
    assert_eq!(server.finish().len(), 2, "404 must not trigger another poll");
    assert_eq!(task.wait().await, EventQueueExit::PollCanceled);
}

#[tokio::test(flavor = "multi_thread")]
async fn primary_event_queue_404_keeps_session_connected() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
        ]
    });
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let agent_id = UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("agent UUID");
    let session = AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address: simulator.local_addr().expect("simulator address"),
        agent_id,
        session_id: UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("session UUID"),
        circuit_code: 0x1122_3344,
        seed_capability: server.url("/seed"),
    };
    let (_commands, command_receiver) = mpsc::channel(crate::client::COMMAND_CAPACITY);
    let (event_sender, event_receiver) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY + crate::event::PRIMARY_TERMINAL_EVENT_CAPACITY);
    let mut events = EventReceiver::new(event_receiver);
    let (disconnect, disconnect_requested) = watch::channel(false);
    let mut worker = tokio::spawn(crate::simulator::run_session(session, command_receiver, event_sender, disconnect_requested));

    let seed_request = server.next_request();
    let poll_request = server.next_request();
    assert!(request_line(&seed_request).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&poll_request).starts_with("POST /event-queue HTTP/1.1"));

    let (_, peer) = receive_udp(&simulator).await;
    simulator.send_to(&acknowledgement(20, 1), peer).await.expect("ack circuit setup");
    let (movement, _) = receive_udp(&simulator).await;
    assert!(matches!(Packet::decode(&movement).expect("decode movement").message(), Message::CompleteAgentMovement(_)));
    simulator.send_to(&acknowledgement(21, 2), peer).await.expect("ack initial movement");

    let mut handshake = test_fixture(test_support::udp::vectors::REGION_HANDSHAKE);
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    let (reply, _) = receive_udp(&simulator).await;
    let reply = Packet::decode(&reply).expect("decode handshake reply");
    assert!(matches!(reply.message(), Message::RegionHandshakeReply(_)));
    simulator.send_to(&acknowledgement(23, reply.sequence()), peer).await.expect("ack handshake reply");

    let mut movement_complete = test_fixture(test_support::udp::vectors::AGENT_MOVEMENT_COMPLETE);
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 24);
    simulator.send_to(&movement_complete, peer).await.expect("complete agent movement");

    let mut session_ready = false;
    let mut poll_canceled = false;
    timeout(TEST_TIMEOUT, async {
        while !session_ready || !poll_canceled {
            match events.recv().await.expect("session event stream remains open") {
                Event::SessionReady { .. } => session_ready = true,
                Event::CapabilityError { reason, .. } if reason.contains("HTTP 404") && reason.contains("polling stopped") => {
                    poll_canceled = true;
                }
                Event::SimulatorFailed { reason, .. } | Event::Failed(reason) => {
                    panic!("primary session failed after EventQueue 404: {reason}")
                }
                _ => {}
            }
        }
    })
    .await
    .expect("session startup and EventQueue cancellation");

    assert!(timeout(Duration::from_millis(100), &mut worker).await.is_err(), "HTTP 404 must not stop the primary session");
    let mut datagram = [0; 65_535];
    while let Ok((size, _)) = simulator.try_recv_from(&mut datagram) {
        assert!(
            !matches!(Packet::decode(&datagram[..size]).expect("decode simulator packet").message(), Message::LogoutRequest(_)),
            "HTTP 404 must not send logout"
        );
    }

    disconnect.send_replace(true);
    let (logout, _) = receive_udp(&simulator).await;
    assert!(matches!(Packet::decode(&logout).expect("decode explicit logout").message(), Message::LogoutRequest(_)));
    timeout(TEST_TIMEOUT, worker).await.expect("explicit disconnect stops the session").expect("session task").expect("clean disconnect");
    assert!(server.finish().is_empty(), "404 must end polling without another request");
}

fn test_fixture(source: &str) -> Vec<u8> {
    test_support::parse_hex_fixture(source).expect("valid UDP fixture")
}

fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}

#[tokio::test]
async fn retry_limit_is_terminal() {
    let mut error_count = MAX_EVENT_QUEUE_ERRORS;
    let (events, received_events) = mpsc::channel(1);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, mut shutdown_requested) = watch::channel(false);
    let (_session_closed, session_closed) = watch::channel(false);

    let outcome = retry_event_queue(
        SimulatorId::new(1),
        &mut error_count,
        test_source(),
        &events,
        "EventQueueGet returned HTTP 500.".into(),
        &mut shutdown_requested,
        &session_closed,
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
async fn seed_lookup_retries() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::Response(response(503, "Service Unavailable", "text/plain", None, "try again")),
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("seed lookup should retry after a server error")
        .expect("event channel should remain open");
    assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 404")));
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
    task.wait().await;
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
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("seed lookup should retry after a rate limit")
        .expect("event channel should remain open");
    assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 404")));
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
    task.wait().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn seed_transport_retries() {
    let server = EventQueueTestServer::start(|base| {
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        vec![
            EventServerResponse::DropConnection,
            EventServerResponse::Response(response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url))),
            EventServerResponse::Response(response(404, "Not Found", "text/plain", None, "not found")),
        ]
    });
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("seed lookup should retry after a transport failure")
        .expect("event channel should remain open");
    assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 404")));
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_line(&requests[2]).starts_with("POST /event-queue HTTP/1.1"));
    task.wait().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn permanent_seed_error_stops() {
    let server =
        EventQueueTestServer::start(|_| vec![EventServerResponse::Response(response(401, "Unauthorized", "text/plain", None, "denied"))]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let (event_sender, received_events) = mpsc::channel(crate::event::CLIENT_EVENT_CAPACITY);
    let mut received_events = EventReceiver::new(received_events);
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let task = client.start_event_queue(SimulatorId::new(1), test_source(), event_sender, shutdown_requested).expect("start event queue");

    let event = timeout(TEST_TIMEOUT, received_events.recv())
        .await
        .expect("permanent seed failure should be reported")
        .expect("event channel should remain open");
    assert!(matches!(event, Event::CapabilityError { reason, .. } if reason.contains("HTTP 401")));
    assert_eq!(server.finish().len(), 1, "permanent errors must not be retried");
    task.wait().await;
}

const EVENT_RESPONSE: &str = "<llsd><map><key>id</key><integer>4</integer><key>events</key><array><map><key>message</key><string>ViewerSpecificMessage</string><key>body</key><map><key>value</key><array><integer>12</integer><boolean>true</boolean></array></map></map></array></map></llsd>";
const TWO_EVENT_BATCH: &str = "<llsd><map><key>id</key><integer>5</integer><key>events</key><array><map><key>message</key><string>First</string></map><map><key>message</key><string>Second</string></map></array></map></llsd>";
const MALFORMED_BATCH: &str = "<llsd><map>";

fn content_length_only_response(content_length: usize) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

struct ConcurrentCapabilityTestServer {
    base: Url,
    requests: mpsc::UnboundedReceiver<()>,
    worker: Option<thread::JoinHandle<usize>>,
}

impl ConcurrentCapabilityTestServer {
    fn start(request_count: usize, response_delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind capability concurrency server");
        listener.set_nonblocking(true).expect("set concurrency listener nonblocking");
        let address = listener.local_addr().expect("read concurrency server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid local capability URL");
        let mut capability_url = base.clone();
        capability_url.set_path("/capability");
        let body = format!("<llsd><map><key>TestCapability</key><string>{capability_url}</string></map></llsd>");
        let response = response(200, "OK", "application/llsd+xml", None, &body);
        let (request_sender, requests) = mpsc::unbounded_channel();

        let worker = thread::spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            let maximum_active = Arc::new(AtomicUsize::new(0));
            let mut handlers = Vec::with_capacity(request_count);

            for _ in 0..request_count {
                let (mut stream, _) = accept_with_timeout(&listener);
                stream.set_nonblocking(false).expect("set capability test stream blocking");
                stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set concurrency request timeout");
                let response = response.clone();
                let active = Arc::clone(&active);
                let maximum_active = Arc::clone(&maximum_active);
                let request_sender = request_sender.clone();
                handlers.push(thread::spawn(move || {
                    let _request = read_request(&mut stream);
                    let _ = request_sender.send(());
                    let active_count = active.fetch_add(1, Ordering::Relaxed) + 1;
                    maximum_active.fetch_max(active_count, Ordering::Relaxed);
                    thread::sleep(response_delay);
                    let _ = stream.write_all(&response);
                    active.fetch_sub(1, Ordering::Relaxed);
                }));
            }
            drop(request_sender);

            for handler in handlers {
                handler.join().expect("capability request handler should finish");
            }
            maximum_active.load(Ordering::Relaxed)
        });

        Self { base, requests, worker: Some(worker) }
    }

    fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    async fn next_request(&mut self) {
        timeout(TEST_TIMEOUT, self.requests.recv())
            .await
            .expect("HTTP request should reach capability server")
            .expect("capability server request stream should remain open");
    }

    fn finish(mut self) -> usize {
        self.worker.take().expect("concurrency server worker").join().expect("concurrency server should stop")
    }
}

struct EventQueueBudgetTestServer {
    base: Url,
    seeds: std_mpsc::Receiver<()>,
    polls: std_mpsc::Receiver<TcpStream>,
    stop: std_mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl EventQueueBudgetTestServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue budget server");
        listener.set_nonblocking(true).expect("set budget listener nonblocking");
        let address = listener.local_addr().expect("read budget server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid event queue test URL");
        let mut event_url = base.clone();
        event_url.set_path("/event-queue");
        let seed_response = response(200, "OK", "application/llsd+xml", None, &seed_response(&event_url));
        let (seed_sender, seeds) = std_mpsc::channel();
        let (poll_sender, polls) = std_mpsc::channel();
        let (stop, stop_receiver) = std_mpsc::channel();
        let worker = thread::spawn(move || {
            while let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) {
                stream.set_nonblocking(false).expect("set budget stream blocking");
                stream.set_read_timeout(Some(TEST_TIMEOUT)).expect("set budget request timeout");
                let request = read_request(&mut stream);
                match request_line(&request).split_whitespace().nth(1) {
                    Some("/seed") => {
                        stream.write_all(&seed_response).expect("write seed capability response");
                        seed_sender.send(()).expect("test should observe every seed lookup");
                    }
                    Some("/event-queue") => poll_sender.send(stream).expect("test should receive each event poll"),
                    path => panic!("unexpected capability request path: {path:?}"),
                }
            }
        });
        Self { base, seeds, polls, stop, worker: Some(worker) }
    }

    fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    fn next_poll(&self) -> TcpStream {
        self.polls.recv_timeout(TEST_TIMEOUT).expect("event queue request should reach budget server")
    }

    fn next_seed(&self) {
        self.seeds.recv_timeout(TEST_TIMEOUT).expect("seed lookup should reach budget server");
    }

    fn stop(&self) {
        let _ = self.stop.send(());
    }

    fn finish(mut self) {
        self.worker.take().expect("budget server worker").join().expect("event queue budget server should stop");
    }
}

pub(super) enum EventServerResponse {
    Response(Vec<u8>),
    Hold,
    DropConnection,
}

pub(super) struct EventQueueTestServer {
    base: Url,
    requests: std_mpsc::Receiver<Vec<u8>>,
    stop: std_mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl EventQueueTestServer {
    pub(super) fn start(responses: impl FnOnce(&Url) -> Vec<EventServerResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind event queue test server");
        listener.set_nonblocking(true).expect("set test listener nonblocking");
        let address = listener.local_addr().expect("read test server address");
        let base = Url::parse(&format!("http://{address}/")).expect("valid local test URL");
        let responses = responses(&base);
        let (request_sender, requests) = std_mpsc::channel();
        let (stop, stop_receiver) = std_mpsc::channel();
        let worker = thread::spawn(move || {
            for response in responses {
                let Some((mut stream, _)) = accept_until_stopped(&listener, &stop_receiver) else {
                    return;
                };
                stream.set_nonblocking(false).expect("set test stream blocking");
                stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set test request timeout");
                let request = read_request(&mut stream);
                request_sender.send(request).expect("test should read request");
                match response {
                    EventServerResponse::Response(response) => {
                        stream.write_all(&response).expect("write test HTTP response");
                    }
                    EventServerResponse::Hold => {
                        let mut byte = [0];
                        let _ = stream.read(&mut byte);
                    }
                    EventServerResponse::DropConnection => drop(stream),
                }
            }
        });
        Self { base, requests, stop, worker: Some(worker) }
    }

    pub(super) fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    fn next_request(&self) -> Vec<u8> {
        self.requests.recv_timeout(TEST_TIMEOUT).expect("server should receive HTTP request")
    }

    pub(super) fn stop(&self) {
        let _ = self.stop.send(());
    }

    pub(super) fn finish(mut self) -> Vec<Vec<u8>> {
        if let Some(worker) = self.worker.take() {
            worker.join().expect("event queue server should stop");
        }
        self.requests.try_iter().collect()
    }
}

fn accept_until_stopped(listener: &TcpListener, stop: &std_mpsc::Receiver<()>) -> Option<(TcpStream, SocketAddr)> {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        if stop.try_recv().is_ok() {
            return None;
        }
        match listener.accept() {
            Ok(connection) => return Some(connection),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "event queue test server did not receive HTTP request");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept event queue test connection: {error}"),
        }
    }
}

pub(super) fn seed_response(event_url: &Url) -> String {
    format!("<llsd><map><key>EventQueueGet</key><string>{event_url}</string></map></llsd>")
}

fn test_source() -> SocketAddr {
    "127.0.0.1:13000".parse().expect("fixture simulator endpoint")
}

fn simulator_events(ip: [u8; 4], port: u16, agent_id: UUID, seed: &Url) -> String {
    let simulator = Value::Map(Map::from([
        ("Handle".to_owned(), Value::Binary(0x1000_0000_0000_1000_u64.to_be_bytes().to_vec().into())),
        ("IP".to_owned(), Value::Binary(ip.to_vec().into())),
        ("Port".to_owned(), Value::Integer(i32::from(port))),
    ]));
    let enable = Value::Map(Map::from([
        ("message".to_owned(), Value::String("EnableSimulator".to_owned())),
        ("body".to_owned(), Value::Map(Map::from([("SimulatorInfo".to_owned(), Value::Array(vec![simulator]))]))),
    ]));
    let establish = Value::Map(Map::from([
        ("message".to_owned(), Value::String("EstablishAgentCommunication".to_owned())),
        (
            "body".to_owned(),
            Value::Map(Map::from([
                ("agent-id".to_owned(), Value::UUID(agent_id)),
                (
                    "sim-ip-and-port".to_owned(),
                    Value::String(format!("{}:{}", ip.iter().map(u8::to_string).collect::<Vec<_>>().join("."), port)),
                ),
                ("seed-capability".to_owned(), Value::String(seed.to_string())),
            ])),
        ),
    ]));
    let batch = Value::Map(Map::from([("id".to_owned(), Value::Integer(1)), ("events".to_owned(), Value::Array(vec![enable, establish]))]));
    llsd::xml::to_string(&batch).expect("encode local LLSD event queue response")
}

async fn receive_udp(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, endpoint) = timeout(TEST_TIMEOUT, socket.recv_from(&mut buffer))
        .await
        .expect("receive local simulator packet")
        .expect("read local simulator packet");
    (buffer[..size].to_vec(), endpoint)
}

fn acknowledgement(sequence: u32, received_sequence: u32) -> Vec<u8> {
    Packet::new(sequence, Message::PacketAck(PacketAck { packets: vec![Packets { id: received_sequence }] }))
        .encode()
        .expect("encode simulator circuit acknowledgement")
}

fn hold_response() -> EventServerResponse {
    EventServerResponse::Hold
}

fn accept_with_timeout(listener: &TcpListener) -> (TcpStream, SocketAddr) {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match listener.accept() {
            Ok(connection) => return connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "event queue server did not receive a request");
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("accept test connection: {error}"),
        }
    }
}

fn write_response(mut stream: TcpStream, response: Vec<u8>) {
    stream.write_all(&response).expect("write controlled event queue response");
}

fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let size = stream.read(&mut chunk).expect("read test HTTP request");
        if size == 0 {
            return request;
        }
        request.extend_from_slice(&chunk[..size]);
        if let Some(body_start) = request.windows(4).position(|window| window == b"\r\n\r\n") {
            let content_length = request[..body_start]
                .split(|byte| *byte == b'\n')
                .filter_map(|line| str::from_utf8(line).ok())
                .find_map(|line| {
                    let (name, value) = line.trim().split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
                })
                .unwrap_or(0);
            if request.len() >= body_start + 4 + content_length {
                return request;
            }
        }
    }
}

fn request_line(request: &[u8]) -> &str {
    str::from_utf8(request).expect("request should be UTF-8").split("\r\n").next().expect("request line")
}

fn request_body(request: &[u8]) -> &str {
    str::from_utf8(request).expect("request should be UTF-8").split_once("\r\n\r\n").expect("request should include body").1
}

fn decode_request_body(request: &[u8]) -> Value {
    llsd::xml::from_str(request_body(request)).expect("request body should contain LLSD XML")
}

fn map_field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_map()?.get(key)
}
