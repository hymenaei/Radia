use super::http::{LocalServer, response};
use std::{
    fs,
    io::{BufRead, BufReader, Cursor, Read, Write},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Duration,
};
use zip::ZipArchive;

use protocol::udp::{Message, Packet};
use test_support::{parse_hex_fixture, udp::vectors};

use crate::{
    capabilities::{CapabilityUrl, Client as CapabilityClient},
    capture::{
        CaptureConfig, CaptureEvent, CaptureEventKind, CaptureManifest, CapturePublication, CaptureRecorder, CaptureSession, CaptureState,
        MAX_CAPTURE_EVENT_LINE_BYTES,
    },
    http::Client,
};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let path =
            std::env::temp_dir().join(format!("radia-capture-test-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).expect("create capture test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn start_capture(directory: &TempDir, recorder: &CaptureRecorder) -> CaptureSession {
    CaptureSession::start(CaptureConfig::new(directory.path().join("captures")), recorder.clone()).expect("start capture")
}

fn finish_capture(session: CaptureSession) -> PathBuf {
    let summary = session.finish_recording(&CapturePublication::default());
    assert_eq!(summary.state, CaptureState::Complete, "capture failed: {:?}", summary.error);
    summary.capture_path.unwrap_or_else(|| panic!("finished capture path: {:?}", summary.error))
}

fn archive_entry(path: &Path, name: &str) -> Vec<u8> {
    let file = fs::File::open(path).expect("open capture archive");
    let mut archive = ZipArchive::new(file).expect("decode ZIP container");
    let mut entry = archive.by_name(name).expect("find capture entry");
    let mut contents = Vec::new();
    entry.read_to_end(&mut contents).expect("read capture entry");
    contents
}

fn archive_entries(path: &Path) -> Vec<String> {
    let file = fs::File::open(path).expect("open capture archive");
    let mut archive = ZipArchive::new(file).expect("decode ZIP container");
    (0..archive.len()).map(|index| archive.by_index(index).expect("read capture entry").name().to_owned()).collect()
}

fn capture_events(path: &Path) -> Vec<CaptureEvent> {
    BufReader::new(Cursor::new(archive_entry(path, "events.jsonl")))
        .lines()
        .map(|line| serde_json::from_str(&line.expect("read capture event")).expect("parse capture event"))
        .collect()
}

fn payload<'a>(payloads: &'a [u8], event: &CaptureEvent) -> &'a [u8] {
    let reference = event.payloads.first().expect("capture event payload reference");
    let start = usize::try_from(reference.offset).expect("payload offset");
    let end = start + usize::try_from(reference.length).expect("payload length");
    &payloads[start..end]
}

fn check_capture(path: &Path) -> (CaptureManifest, Vec<CaptureEvent>, Vec<u8>, usize) {
    assert_eq!(archive_entries(path), ["manifest.json", "events.jsonl", "payloads.bin"]);
    let manifest: CaptureManifest = serde_json::from_slice(&archive_entry(path, "manifest.json")).expect("parse capture manifest");
    let events = capture_events(path);
    let payloads = archive_entry(path, "payloads.bin");

    assert_eq!(manifest.schema_version, 1);
    assert!(!manifest.viewer_version.is_empty());
    assert_ne!(manifest.state, CaptureState::Recording);
    assert_eq!(manifest.event_count, events.len() as u64);
    assert_eq!(manifest.payload_bytes, payloads.len() as u64);

    let mut elapsed_ns = 0;
    let mut decoded_packets = 0;
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event.index, index as u64);
        assert!(event.elapsed_ns >= elapsed_ns);
        elapsed_ns = event.elapsed_ns;

        for reference in &event.payloads {
            let end = reference.offset.checked_add(reference.length).expect("payload range does not overflow");
            assert!(end <= manifest.payload_bytes, "payload range exceeds payloads.bin");
        }

        match &event.kind {
            CaptureEventKind::UdpSend { sequence, reliable, resent, zerocoded, appended_acks, packet_acks, .. }
            | CaptureEventKind::UdpReceive { sequence, reliable, resent, zerocoded, appended_acks, packet_acks, .. } => {
                assert_eq!(event.payloads.len(), 1);
                assert_eq!(event.payloads[0].role, "datagram");
                match Packet::decode(payload(&payloads, event)) {
                    Ok(packet) => {
                        decoded_packets += 1;
                        assert_eq!(*sequence, Some(packet.sequence()));
                        assert_eq!(*reliable, Some(packet.is_reliable()));
                        assert_eq!(*resent, Some(packet.is_resent()));
                        assert_eq!(*zerocoded, Some(packet.is_zerocoded().expect("read packet zero-code flag")));
                        assert_eq!(appended_acks.as_slice(), packet.appended_acks());
                        let actual_packet_acks = match packet.message() {
                            Message::PacketAck(message) => message.packets.iter().map(|ack| ack.id).collect::<Vec<_>>(),
                            _ => Vec::new(),
                        };
                        assert_eq!(*packet_acks, actual_packet_acks);
                    }
                    Err(_) => {
                        assert!(sequence.is_none() && reliable.is_none() && resent.is_none() && zerocoded.is_none());
                        assert!(appended_acks.is_empty() && packet_acks.is_empty());
                    }
                }
            }
            CaptureEventKind::HttpRequest { .. } | CaptureEventKind::HttpResponse { .. } => {
                assert!(event.payloads.len() <= 1);
                assert!(event.payloads.iter().all(|reference| reference.role == "body"));
            }
            _ => assert!(event.payloads.is_empty()),
        }
    }

    (manifest, events, payloads, decoded_packets)
}

#[test]
fn capture_stop() {
    let directory = TempDir::new();
    let recorder = CaptureRecorder::new();
    let session = start_capture(&directory, &recorder);
    let request_body = b"<login><username>private-user</username><password>private-password-931</password></login>";
    let response_body = b"<response><session>private-session-822</session></response>";
    let request_id = recorder.http_request("login", "POST", vec![("content-type".into(), "text/xml".into())], request_body);
    recorder.http_response(
        request_id,
        Some(200),
        vec![("content-type".into(), "text/xml".into())],
        Duration::from_millis(11),
        "complete",
        Some(response_body),
    );
    recorder.lifecycle("login_completed");
    let datagram = parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("parse independent chat vector");
    let local: SocketAddr = "127.0.0.1:50000".parse().expect("local endpoint");
    let remote: SocketAddr = "198.51.100.44:13000".parse().expect("simulator endpoint");
    recorder.udp(512347, "in", local, remote, &datagram);

    let capture = finish_capture(session);
    let (manifest, events, payloads, decoded_packets) = check_capture(&capture);
    assert_eq!(manifest.state, CaptureState::Complete);
    assert_eq!(decoded_packets, 1);
    assert!(payloads.windows(request_body.len()).any(|part| part == request_body));
    assert!(payloads.windows(response_body.len()).any(|part| part == response_body));
    assert!(events.iter().any(|event| matches!(event.kind, CaptureEventKind::LoginCompleted)));

    let request = events
        .iter()
        .find_map(|event| match &event.kind {
            CaptureEventKind::HttpRequest { request_id, .. } => Some(*request_id),
            _ => None,
        })
        .expect("captured HTTP request");
    assert!(events.iter().any(|event| matches!(event.kind, CaptureEventKind::HttpResponse { request_id, .. } if request_id == request)));
    let udp = events.iter().find(|event| matches!(event.kind, CaptureEventKind::UdpReceive { .. })).expect("captured UDP receive");
    assert_eq!(payload(&payloads, udp), datagram);
}

#[test]
fn oversized_event_is_omitted_and_marks_capture_incomplete() {
    let directory = TempDir::new();
    let recorder = CaptureRecorder::new();
    let session = start_capture(&directory, &recorder);
    recorder.http_request("test", "GET", vec![("x-large".into(), "x".repeat(MAX_CAPTURE_EVENT_LINE_BYTES))], &[]);

    let summary = session.finish_recording(&CapturePublication::default());
    assert_eq!(summary.state, CaptureState::Incomplete);
    assert_eq!(summary.error.as_deref(), Some("Capture event exceeds the per-event line size limit."));

    let path = summary.capture_path.expect("incomplete capture archive");
    let (manifest, events, _, _) = check_capture(&path);
    assert_eq!(manifest.state, CaptureState::Incomplete);
    assert!(events.is_empty());
}

#[test]
fn large_http_body_does_not_count_toward_event_line_limit() {
    let directory = TempDir::new();
    let recorder = CaptureRecorder::new();
    let session = start_capture(&directory, &recorder);
    let body = vec![b'x'; MAX_CAPTURE_EVENT_LINE_BYTES + 1];
    recorder.http_request("test", "POST", Vec::new(), &body);

    let capture = finish_capture(session);
    let (_, events, payloads, _) = check_capture(&capture);

    assert_eq!(events.len(), 1);
    assert_eq!(payload(&payloads, &events[0]), body);
    assert!(archive_entry(&capture, "events.jsonl").len() <= MAX_CAPTURE_EVENT_LINE_BYTES);
}

#[tokio::test]
async fn capability_endpoints_are_captured() {
    let directory = TempDir::new();
    let recorder = CaptureRecorder::new();
    let session = start_capture(&directory, &recorder);
    let capability_server = LocalServer::start(vec![
        response(200, "OK", "application/llsd+xml", None, "<llsd><integer>9</integer></llsd>"),
        response(200, "OK", "application/octet-stream", None, "named"),
        response(200, "OK", "application/octet-stream", None, "unsafe"),
        response(200, "OK", "application/octet-stream", None, "manual"),
    ]);
    let named_url = capability_server.url("/named-secret-token");
    let unsafe_url = capability_server.url("/unsafe-secret-token");
    let seed_body =
        format!("<llsd><map><key>TestCapability</key><uri>{named_url}</uri><key>bad/name</key><uri>{unsafe_url}</uri></map></llsd>");
    let seed_server = LocalServer::start(vec![response(200, "OK", "application/llsd+xml", None, &seed_body)]);
    let client = CapabilityClient::new_with_capture(seed_server.url("/seed"), Some(recorder.clone())).expect("capability client");
    let capabilities = client.request_capabilities(&["TestCapability", "bad/name"]).await.expect("capability discovery");

    let named = capabilities.get("TestCapability").expect("named capability").with_query_pair("request", "private-value");
    let unsafe_name = capabilities.get("bad/name").expect("capability with non-label name");
    assert_eq!(client.post::<_, llsd::Value>(&named, &llsd::Value::Undefined).await.expect("named POST"), llsd::Value::Integer(9));
    assert_eq!(client.get_bytes(&named).await.expect("named request"), b"named");
    assert_eq!(client.get_bytes(unsafe_name).await.expect("unsafe-name request"), b"unsafe");
    let manual = CapabilityUrl::parse(capability_server.url("/manual-secret-token").as_str()).expect("manual URL handle");
    assert_eq!(client.get_bytes(&manual).await.expect("manual request"), b"manual");
    client.close_session();

    assert_eq!(seed_server.finish().len(), 1);
    assert_eq!(capability_server.finish().len(), 4);
    let capture = finish_capture(session);
    let (_, events, _, _) = check_capture(&capture);
    let endpoints = events
        .iter()
        .filter_map(|event| match &event.kind {
            CaptureEventKind::HttpRequest { endpoint, .. } => Some(endpoint.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(endpoints, ["seed_capability", "TestCapability", "TestCapability", "capability_get", "capability_get"]);
    assert!(endpoints.iter().all(|endpoint| !endpoint.contains("secret-token")));
    assert!(endpoints.iter().all(|endpoint| !endpoint.contains("127.0.0.1")));
}

#[test]
fn capture_interrupt() {
    let directory = TempDir::new();
    let recorder = CaptureRecorder::new();
    let session = start_capture(&directory, &recorder);
    let path = session.path().to_path_buf();
    recorder.lifecycle("login_completed");

    let summary = session.interrupt();
    assert_eq!(summary.state, CaptureState::Interrupted);
    assert_eq!(summary.capture_path.as_deref(), Some(path.as_path()));
    let (manifest, events, _, _) = check_capture(&path);
    assert_eq!(manifest.state, CaptureState::Interrupted);
    assert!(events.iter().any(|event| matches!(event.kind, CaptureEventKind::LoginCompleted)));
}

#[test]
fn http_errors() {
    let directory = TempDir::new();
    let recorder = CaptureRecorder::new();
    let session = start_capture(&directory, &recorder);
    let request_id = recorder.http_request(
        "capability_get",
        "GET",
        vec![("accept".into(), "application/llsd+xml".into()), ("accept".into(), "application/llsd+xml".into())],
        &[],
    );
    recorder.http_response(request_id, None, Vec::new(), Duration::from_millis(3), "send_error", None);

    let capture = finish_capture(session);
    let (_, events, _, _) = check_capture(&capture);
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        CaptureEventKind::HttpRequest { request_id: id, headers, .. }
            if *id == request_id && headers == &[("accept".into(), "application/llsd+xml".into()), ("accept".into(), "application/llsd+xml".into())]
    )));
    assert!(events.iter().any(|event| matches!(event.kind, CaptureEventKind::HttpError { request_id: id, .. } if id == request_id)));
    assert!(!events.iter().any(|event| matches!(event.kind, CaptureEventKind::HttpResponse { .. })));
}

#[tokio::test]
async fn pending_http() {
    let directory = TempDir::new();
    let recorder = CaptureRecorder::new();
    let session = start_capture(&directory, &recorder);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local HTTP server");
    let address = listener.local_addr().expect("read local HTTP address");
    let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept local HTTP request");
        let mut request = Vec::new();
        let mut chunk = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut chunk).expect("read local HTTP request");
            assert_ne!(count, 0, "client closed before sending HTTP headers");
            request.extend_from_slice(&chunk[..count]);
        }
        accepted_tx.send(()).expect("signal accepted request");
        release_rx.recv_timeout(Duration::from_secs(10)).expect("release held HTTP response");
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").expect("send local HTTP response");
    });
    let client = Client::new(Some(Duration::from_secs(5)), true, None).expect("build HTTP client").with_capture(Some(recorder));
    let request_url = format!("http://{address}/capability");
    let request = tokio::spawn(async move { client.get(&url::Url::parse(&request_url).expect("local HTTP URL")).await });

    tokio::task::spawn_blocking(move || accepted_rx.recv_timeout(Duration::from_secs(5)).expect("HTTP request reached local server"))
        .await
        .expect("wait for local HTTP server");
    let capture = finish_capture(session);
    release_tx.send(()).expect("release local HTTP response");
    let response = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .expect("HTTP request should finish")
        .expect("HTTP task should finish")
        .expect("receive held HTTP response");
    assert_eq!(response.body, b"ok");
    server.join().expect("local HTTP server should finish");

    let (_, events, _, _) = check_capture(&capture);
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].kind, CaptureEventKind::HttpRequest { .. }));
}
