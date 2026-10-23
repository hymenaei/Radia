use std::{
    future::Future,
    io::{self, ErrorKind},
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
    time::{Duration, Instant},
};

use llsd::{Value, xml};
use reqwest::StatusCode;
use serde::Serialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net, runtime,
    sync::{Semaphore, oneshot},
    task::{JoinSet, spawn, yield_now},
    time::timeout,
};
use url::Url;

use super::{DecodeGate, RequestDeadline, admit_capability_decode_memory_with_response_limit, decode_admitted_response};
use crate::capabilities::budget::{
    LLSD_SCAN_WORKING_BYTES, MAX_CAPABILITY_REQUEST_BODY_BYTES, MAX_CAPABILITY_RESPONSE_BYTES, MAX_CONCURRENT_CAPABILITY_REQUESTS,
    MAX_LLSD_XML_ELEMENTS, MAX_SEED_CAPABILITY_RESPONSE_BYTES,
};
use crate::test_support::{
    http::{ConcurrentCapabilityTestServer, LocalServer, response},
    simulator::test_source,
};
use crate::{
    Error,
    Error::{Capability, CapabilityHttp},
    NetworkStatusHandle,
    capabilities::request::AdmittedResponse,
    capabilities::{CapabilityBudget, CapabilityUrl, Client},
    event::{Event, SimulatorId},
    http,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

fn admit_capability_decode_memory(body: &[u8], http: &http::Client) -> Result<http::ResponseMemoryPermit, Error> {
    admit_capability_decode_memory_with_response_limit(body, http, MAX_CAPABILITY_RESPONSE_BYTES)
}

#[tokio::test]
async fn cancelled_worker_wait_can_be_resumed() {
    use std::pin::pin;

    use super::RequestWorker;
    use tokio::sync::Mutex;

    let (release, wait) = oneshot::channel::<()>();
    let worker = RequestWorker {
        task: Mutex::new(Some(spawn(async move {
            let _ = wait.await;
        }))),
    };
    {
        let mut wait = pin!(worker.wait());
        assert!(wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    }
    let mut wait = pin!(worker.wait());
    assert!(wait.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(), "the worker is still running");
    release.send(()).expect("worker is waiting for release");
    wait.await.expect("resumed join completes");
    worker.wait().await.expect("repeated join is harmless");
}

async fn read_request_headers(stream: &mut net::TcpStream) -> io::Result<()> {
    let mut request = Vec::new();
    let mut chunk = [0; 1024];
    loop {
        let size = stream.read(&mut chunk).await?;
        if size == 0 {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        request.extend_from_slice(&chunk[..size]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(());
        }
    }
}

async fn write_empty_response(stream: &mut net::TcpStream) -> io::Result<()> {
    stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await
}

#[test]
fn requests_run_outside_tokio() {
    let server = LocalServer::start(vec![response(200, "OK", "application/octet-stream", None, "body")]);
    let runtime = runtime::Builder::new_multi_thread().enable_all().build().expect("Tokio runtime");
    let client = runtime.block_on(async { Client::new(server.url("/seed")) }).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/resource").as_str()).expect("valid capability URL");

    let body = block_on(client.get_bytes(&url)).expect("capability request from the external executor");

    assert_eq!(body, b"body");
    drop(client);
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn capability_clients_share_the_session_http_connection_pool() {
    let listener = net::TcpListener::bind("127.0.0.1:0").await.expect("bind local HTTP listener");
    let address = listener.local_addr().expect("read listener address");
    let server = spawn(async move {
        let (mut first, first_peer) = listener.accept().await.expect("accept first capability request");
        read_request_headers(&mut first).await.expect("read first request");
        write_empty_response(&mut first).await.expect("write first response");

        tokio::select! {
            result = read_request_headers(&mut first) => {
                result.expect("read second request on reused connection");
                write_empty_response(&mut first).await.expect("write second response");
                (first_peer, first_peer)
            }
            accepted = listener.accept() => {
                let (mut second, second_peer) = accepted.expect("accept second capability request");
                read_request_headers(&mut second).await.expect("read second request on another connection");
                write_empty_response(&mut second).await.expect("write second response");
                (first_peer, second_peer)
            }
        }
    });

    let endpoint = format!("http://{address}");
    let seed = Url::parse(&format!("{endpoint}/seed")).expect("valid seed URL");
    let resource = CapabilityUrl::parse(&format!("{endpoint}/resource")).expect("valid capability URL");
    let budget = CapabilityBudget::new();
    let first = Client::new_with_status_and_budget(seed.clone(), NetworkStatusHandle::default(), budget.clone())
        .expect("create first capability client");
    let second = Client::new_with_status_and_budget(seed, NetworkStatusHandle::default(), budget).expect("create second capability client");

    assert!(first.get_bytes(&resource).await.expect("first capability request").is_empty());
    assert!(second.get_bytes(&resource).await.expect("second capability request").is_empty());
    let (first_peer, second_peer) = timeout(TEST_TIMEOUT, server).await.expect("server should receive both requests").expect("server task");

    assert_eq!(first_peer, second_peer, "clients in one capability session should reuse an HTTP connection");
}

fn block_on<F: Future>(future: F) -> F::Output {
    struct Unparker(thread::Thread);

    impl Wake for Unparker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(Unparker(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = Box::pin(future);
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                assert!(!remaining.is_zero(), "external-executor request timed out");
                thread::park_timeout(remaining);
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn response_memory_stays_admitted_through_value_decode() {
    let body = b"<llsd><integer>42</integer></llsd>".to_vec();
    let body_len = body.len();
    let allocation_bound =
        xml::value_allocation_bound(&body, crate::capabilities::request::capability_llsd_limits()).expect("bound valid LLSD response");
    let memory_limit = body_len + LLSD_SCAN_WORKING_BYTES + allocation_bound;
    let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0);
    let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_len).expect("reserve raw response memory");
    let (gate, entered_rx) = DecodeGate::new();
    let requests = Arc::new(Semaphore::new(1));
    let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
    let response = AdmittedResponse { body, max_response_bytes: MAX_CAPABILITY_RESPONSE_BYTES, _memory: Some(memory), _request: request };
    let http = http::Client::new(Some(TEST_TIMEOUT), false, None)
        .expect("build HTTP client")
        .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);
    let decode_gate = Arc::clone(&gate);
    let task = spawn(decode_admitted_response(response, http, RequestDeadline::new(TEST_TIMEOUT), move || decode_gate.wait()));

    let entered_decode = entered_rx.recv_timeout(TEST_TIMEOUT).is_ok();
    let raw_memory_held = budget.try_reserve(http::ResponseMemoryClass::Ordinary, 1).is_err();
    let request_held = requests.try_acquire().is_err();
    gate.release();
    let result = timeout(TEST_TIMEOUT, task).await.expect("decoder should finish").expect("decoder task");

    assert!(entered_decode, "LLSD decode should reach its admitted blocking point");
    assert!(raw_memory_held, "raw response and decode admission should remain held while decoding");
    assert!(request_held, "request concurrency should remain admitted while decoding");
    assert_eq!(result.expect("decode admitted response"), Value::Integer(42));
    let permit = budget
        .try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit)
        .expect("successful decode should release body and decoder admissions");
    drop(permit);
    drop(requests.try_acquire().expect("successful decode should release its request slot"));

    let malformed = b"<llsd><array></map></llsd>".to_vec();
    let malformed_len = malformed.len();
    let bound = xml::value_allocation_bound(&malformed, crate::capabilities::request::capability_llsd_limits());
    assert!(matches!(bound, Err(xml::BoundError::Malformed)), "malformed input must fail preflight");
    let memory_limit = malformed_len + LLSD_SCAN_WORKING_BYTES;
    let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0);
    let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, malformed_len).expect("reserve malformed response memory");
    let requests = Arc::new(Semaphore::new(1));
    let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
    let response =
        AdmittedResponse { body: malformed, max_response_bytes: MAX_CAPABILITY_RESPONSE_BYTES, _memory: Some(memory), _request: request };
    let http = http::Client::new(Some(TEST_TIMEOUT), false, None)
        .expect("build HTTP client")
        .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);
    assert!(matches!(decode_admitted_response(response, http, RequestDeadline::new(TEST_TIMEOUT), || {}).await, Err(Capability(_))));
    let permit = budget
        .try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit)
        .expect("failed decode should release body and decoder admissions");
    drop(permit);
    drop(requests.try_acquire().expect("failed decode should release its request slot"));
}

#[tokio::test(flavor = "multi_thread")]
async fn cancellation_keeps_capability_decode_admission_until_worker_finishes() {
    let body = b"<llsd><integer>42</integer></llsd>".to_vec();
    let allocation_bound =
        xml::value_allocation_bound(&body, crate::capabilities::request::capability_llsd_limits()).expect("bound valid LLSD response");
    let memory_limit = body.capacity() + LLSD_SCAN_WORKING_BYTES + allocation_bound;
    let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0);
    let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body.capacity()).expect("reserve raw response memory");
    let (gate, entered_rx) = DecodeGate::new();
    let requests = Arc::new(Semaphore::new(1));
    let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
    let response = AdmittedResponse { body, max_response_bytes: MAX_CAPABILITY_RESPONSE_BYTES, _memory: Some(memory), _request: request };
    let http = http::Client::new(Some(TEST_TIMEOUT), false, None)
        .expect("build HTTP client")
        .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);
    let decode_gate = Arc::clone(&gate);
    let task = spawn(decode_admitted_response(response, http, RequestDeadline::new(TEST_TIMEOUT), move || decode_gate.wait()));

    entered_rx.recv_timeout(TEST_TIMEOUT).expect("LLSD decoder reached its admitted blocking point");
    task.abort();
    assert!(task.await.expect_err("async decoder waiter should be canceled").is_cancelled());
    assert!(budget.try_reserve(http::ResponseMemoryClass::Ordinary, 1).is_err());
    assert_eq!(requests.available_permits(), 0, "the request slot remains held by the blocking decoder");

    gate.release();
    timeout(TEST_TIMEOUT, async {
        loop {
            let memory_released = match budget.try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit) {
                Ok(permit) => {
                    drop(permit);
                    true
                }
                Err(_) => false,
            };
            let request_released = match Arc::clone(&requests).try_acquire_owned() {
                Ok(permit) => {
                    drop(permit);
                    true
                }
                Err(_) => false,
            };
            if memory_released && request_released {
                break;
            }
            yield_now().await;
        }
    })
    .await
    .expect("decoder completion should release body, decode memory, and request slot");
}

#[test]
fn decode_preflight_enforces_llsd_limits() {
    let max_depth = crate::capabilities::budget::MAX_LLSD_NESTING_DEPTH;
    let mut nested = String::from("<llsd>");
    for _ in 0..=max_depth {
        nested.push_str("<array>");
    }
    nested.push_str("<undef/>");
    for _ in 0..=max_depth {
        nested.push_str("</array>");
    }
    nested.push_str("</llsd>");

    let mut many_values = String::from("<llsd><array>");
    for _ in 0..MAX_LLSD_XML_ELEMENTS {
        many_values.push_str("<undef/>");
    }
    many_values.push_str("<undef/></array></llsd>");

    for body in [nested.into_bytes(), many_values.into_bytes()] {
        let memory_limit = body.capacity() + LLSD_SCAN_WORKING_BYTES;
        let budget = http::ResponseMemoryBudget::new(memory_limit, 0, 0);
        let raw = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body.capacity()).expect("admit raw response body");
        let client = http::Client::new(Some(TEST_TIMEOUT), false, None)
            .expect("build HTTP client")
            .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);

        let error = match admit_capability_decode_memory(&body, &client) {
            Ok(_) => panic!("preflight must reject a configured LLSD limit"),
            Err(error) => error,
        };
        assert!(matches!(error, CapabilityHttp(http::Error::ResourceExhausted { .. })));
        drop(raw);
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, memory_limit).expect("failed preflight releases workspace"));
    }
}

#[test]
fn decode_preflight_enforces_memory_budget() {
    let body = format!("<llsd><string>{}</string></llsd>", "x".repeat(128 * 1024)).into_bytes();
    let bound =
        xml::value_allocation_bound(&body, crate::capabilities::request::capability_llsd_limits()).expect("bound valid LLSD response");
    let body_capacity = body.capacity();

    for limit in [body_capacity + LLSD_SCAN_WORKING_BYTES - 1, body_capacity + LLSD_SCAN_WORKING_BYTES + bound - 1] {
        let budget = http::ResponseMemoryBudget::new(limit, 0, 0);
        let raw = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_capacity).expect("admit raw response body");
        let client = http::Client::new(Some(TEST_TIMEOUT), false, None)
            .expect("build HTTP client")
            .with_memory_budget(budget.clone(), http::ResponseMemoryClass::Ordinary);

        let error = match admit_capability_decode_memory(&body, &client) {
            Ok(_) => panic!("preflight must reject insufficient memory"),
            Err(error) => error,
        };
        assert!(matches!(error, CapabilityHttp(http::Error::ResourceExhausted { .. })));
        drop(raw);
        drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, limit).expect("failed admission returns all memory"));
    }
}

#[test]
fn dropped_reply_releases_admission() {
    let body = b"body".to_vec();
    let body_len = body.len();
    let budget = http::ResponseMemoryBudget::new(body_len, 0, 0);
    let memory = budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_len).expect("reserve response memory");
    let requests = Arc::new(Semaphore::new(1));
    let request = Arc::clone(&requests).try_acquire_owned().expect("reserve request slot");
    let response = AdmittedResponse { body, max_response_bytes: MAX_CAPABILITY_RESPONSE_BYTES, _memory: Some(memory), _request: request };
    let (reply, waiting) = oneshot::channel();

    assert!(reply.send(response).is_ok(), "caller should be able to retain the admitted reply");
    assert_eq!(requests.available_permits(), 0);
    assert!(budget.try_reserve(http::ResponseMemoryClass::Ordinary, 1).is_err());

    drop(waiting);

    assert_eq!(requests.available_permits(), 1);
    drop(budget.try_reserve(http::ResponseMemoryClass::Ordinary, body_len).expect("dropping reply releases response memory"));
}

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

    assert!(matches!(error, Capability(_)));
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
async fn get_and_post_return_llsd_values_for_consumer_conversion() {
    #[derive(Debug, Eq, PartialEq)]
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

    let posted = client.post(&url, &Request { count: 7 }).await.expect("LLSD POST response");
    let fetched = client.get(&url).await.expect("LLSD GET response");
    let requests = server.finish();

    let into_reply = |value: Value| {
        let count = value.as_map().and_then(|fields| fields.get("count")).and_then(Value::as_integer).expect("integer count field");
        Reply { count }
    };
    assert_eq!(into_reply(posted), Reply { count: 7 });
    assert_eq!(into_reply(fetched), Reply { count: 7 });
    let post = str::from_utf8(&requests[0]).expect("POST is UTF-8");
    assert!(post.starts_with("POST /capability HTTP/1.1"));
    let (_, post_body) = post.split_once("\r\n\r\n").expect("POST has a body");
    let payload: Value = xml::from_str(post_body).expect("POST body is valid LLSD XML");
    assert_eq!(payload.as_map().and_then(|map| map.get("count")), Some(&Value::Integer(7)));
    assert!(str::from_utf8(&requests[1]).expect("GET is UTF-8").starts_with("GET /capability HTTP/1.1"));
}

#[tokio::test]
async fn raw_body_limits_apply_to_declared_and_unknown_lengths() {
    let server = LocalServer::start(vec![
        response(200, "OK", "application/octet-stream", None, "12345"),
        b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n3\r\n123\r\n3\r\n456\r\n0\r\n\r\n".to_vec(),
        response(200, "OK", "application/octet-stream", None, "123456"),
        response(200, "OK", "application/octet-stream", None, "12345"),
    ]);
    let client = Client::new(server.url("/seed")).unwrap();
    let url = CapabilityUrl::parse(server.url("/raw").as_str()).unwrap();
    assert_eq!(client.get_bytes_with_response_limit(&url, 5).await.unwrap(), b"12345");
    assert!(matches!(client.get_bytes_with_response_limit(&url, 5).await, Err(CapabilityHttp(http::Error::ResourceExhausted { .. }))));
    assert!(matches!(
        client.post_bytes_with_response_limit(&url, &Value::Undefined, 5).await,
        Err(CapabilityHttp(http::Error::ResourceExhausted { .. }))
    ));
    assert_eq!(client.post_bytes_with_response_limit(&url, &Value::Undefined, 5).await.unwrap(), b"12345");
    for limit in [0, MAX_CAPABILITY_RESPONSE_BYTES + 1] {
        assert!(matches!(client.get_bytes_with_response_limit(&url, limit).await, Err(Capability(_))));
        assert!(matches!(client.post_bytes_with_response_limit(&url, &Value::Undefined, limit).await, Err(Capability(_))));
    }
    let requests = server.finish();
    assert_eq!(requests.len(), 4, "invalid limits never reach the peer");
    assert!(requests[2].starts_with(b"POST /raw HTTP/1.1"));
}

#[test]
fn asset_directory_urls_preserve_opaque_path_and_query() {
    let url = CapabilityUrl::parse("https://example.test/cap%2Ftoken?grant=opaque")
        .unwrap()
        .with_trailing_slash()
        .with_query_pair("texture_id", "content");
    assert_eq!(url.url.as_str(), "https://example.test/cap%2Ftoken/?grant=opaque&texture_id=content");
    assert!(!format!("{url:?}").contains("token"));
}

#[tokio::test]
async fn post_uses_response_limit_for_decode() {
    let body = "<llsd><string>media</string></llsd>";
    let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/object-media").as_str()).expect("valid capability URL");

    let value =
        client.post_with_response_limit(&url, &Value::Undefined, body.len()).await.expect("body at the selected limit should decode");

    assert_eq!(value, Value::String("media".into()));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn post_rejects_oversized_response_before_reading() {
    const RESPONSE_LIMIT: usize = 32;

    let server = LocalServer::start(vec![content_length_only_response(RESPONSE_LIMIT + 1)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/object-media").as_str()).expect("valid capability URL");

    let error = client
        .post_with_response_limit(&url, &Value::Undefined, RESPONSE_LIMIT)
        .await
        .expect_err("declared response over the selected limit should fail");

    assert!(matches!(&error, CapabilityHttp(http::Error::ResourceExhausted { status: Some(StatusCode::OK), .. })));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn post_rejects_invalid_limits_without_sending() {
    let server = LocalServer::start(vec![]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/object-media").as_str()).expect("valid capability URL");

    for limit in [0, MAX_CAPABILITY_RESPONSE_BYTES + 1] {
        assert!(matches!(client.post_with_response_limit(&url, &Value::Undefined, limit).await, Err(Capability(_))));
    }

    assert!(server.finish().is_empty());
}

#[tokio::test]
async fn large_valid_capability_value_is_decoded_with_memory_admission() {
    const PAYLOAD_BYTES: usize = 1024 * 1024;

    let payload = "x".repeat(PAYLOAD_BYTES);
    let body = format!("<llsd><string>{payload}</string></llsd>");
    let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, &body)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/large").as_str()).expect("valid capability URL");

    let value = client.get(&url).await.expect("admit and decode a 1 MiB LLSD value");

    assert_eq!(value, Value::String(payload));
    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn ordinary_post_is_not_replayed_after_a_server_error() {
    let server = LocalServer::start(vec![response(503, "Service Unavailable", "text/plain", Some("0"), "try again")]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/side-effect").as_str()).expect("valid capability URL");

    let error = client.post(&url, &Value::String("one-shot".into())).await.expect_err("ordinary POST reports the server failure");

    assert!(matches!(
        &error,
        CapabilityHttp(http::Error::HttpStatus {
            status: StatusCode::SERVICE_UNAVAILABLE,
            retry_after: Some(retry_after),
            elapsed,
            ..
        }) if *retry_after == Duration::ZERO && !elapsed.is_zero()
    ));
    assert!(!format!("{error:?}").contains("try again"));
    assert_eq!(server.finish().len(), 1, "ordinary capability POSTs are never retried automatically");
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
async fn oversized_declared_seed_body_is_rejected_before_reading() {
    let server = LocalServer::start(vec![content_length_only_response(MAX_SEED_CAPABILITY_RESPONSE_BYTES + 1)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let error = client.request_capabilities(&["TestCapability"]).await.expect_err("oversized seed response should fail");
    assert!(matches!(&error, CapabilityHttp(http::Error::ResourceExhausted { status: Some(StatusCode::OK), .. })));

    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn oversized_declared_capability_body_is_rejected_before_reading() {
    let server = LocalServer::start(vec![content_length_only_response(MAX_CAPABILITY_RESPONSE_BYTES + 1)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
    let error = client.post(&url, &Value::Undefined).await.expect_err("oversized LLSD response should fail");
    assert!(matches!(&error, CapabilityHttp(http::Error::ResourceExhausted { status: Some(StatusCode::OK), .. })));

    assert_eq!(server.finish().len(), 1);
}

#[tokio::test]
async fn request_body_is_bounded() {
    let server = LocalServer::start(vec![]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let url = CapabilityUrl::parse(server.url("/capability").as_str()).expect("valid capability URL");
    let payload = Value::String("N".repeat(MAX_CAPABILITY_REQUEST_BODY_BYTES + 1));

    let error = client.post(&url, &payload).await.expect_err("an oversized request body should fail");

    assert!(matches!(error, Capability(_)));
    assert!(server.finish().is_empty(), "an oversized request must not be sent");
}

#[tokio::test]
async fn bad_cap_urls_are_rejected() {
    let body = concat!("<llsd><map><key>TestCapability</key><string>", "ftp://caps.example.net/capability", "</string></map></llsd>");
    let server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, body)]);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");

    let error = client.request_capabilities(&["TestCapability"]).await.expect_err("FTP capability URLs are not supported");
    assert!(matches!(&error, Capability(_)));
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
    let mut server = ConcurrentCapabilityTestServer::start(request_count);
    let client = Client::new(server.url("/seed")).expect("active Tokio runtime");
    let mut requests = JoinSet::new();

    for _ in 0..request_count {
        let client = client.clone();
        requests.spawn(async move { client.request_capabilities(&["TestCapability"]).await });
    }

    for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
        server.next_request().await;
    }
    for _ in MAX_CONCURRENT_CAPABILITY_REQUESTS..request_count {
        server.release_response();
        server.next_request().await;
    }
    let maximum_active = server.finish();

    timeout(TEST_TIMEOUT, async {
        while let Some(request) = requests.join_next().await {
            request.expect("capability request task should finish").expect("local seed request should succeed");
        }
    })
    .await
    .expect("bounded capability requests should finish");

    assert!(maximum_active > 1, "capability requests should remain concurrent");
    assert!(maximum_active <= MAX_CONCURRENT_CAPABILITY_REQUESTS, "too many HTTP requests ran at once");
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_budget_bounds_requests() {
    let request_count = MAX_CONCURRENT_CAPABILITY_REQUESTS * 2;
    let mut server = ConcurrentCapabilityTestServer::start(request_count);
    let budget = CapabilityBudget::new();
    let clients = (0..2)
        .map(|_| {
            Client::new_with_status_and_budget(server.url("/seed"), NetworkStatusHandle::default(), budget.clone())
                .expect("active Tokio runtime")
        })
        .collect::<Vec<_>>();
    let mut requests = JoinSet::new();

    for index in 0..request_count {
        let client = clients[index % clients.len()].clone();
        requests.spawn(async move { client.request_capabilities(&["TestCapability"]).await });
    }

    for _ in 0..MAX_CONCURRENT_CAPABILITY_REQUESTS {
        server.next_request().await;
    }
    for _ in MAX_CONCURRENT_CAPABILITY_REQUESTS..request_count {
        server.release_response();
        server.next_request().await;
    }
    let maximum_active = server.finish();

    timeout(TEST_TIMEOUT, async {
        while let Some(request) = requests.join_next().await {
            request.expect("capability request task should finish").expect("local seed request should succeed");
        }
    })
    .await
    .expect("shared budget requests should finish");

    assert!(maximum_active > 1, "the shared budget should allow concurrent requests");
    assert!(maximum_active <= MAX_CONCURRENT_CAPABILITY_REQUESTS, "shared clients exceeded their request budget");
}

fn content_length_only_response(content_length: usize) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Type: application/llsd+xml\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n")
        .into_bytes()
}
