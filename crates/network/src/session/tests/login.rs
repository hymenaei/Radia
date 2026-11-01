use std::{
    collections::HashMap,
    net::SocketAddr,
    str::from_utf8,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use protocol::lludp::{Message, Packet};
use protocol::messages::TeleportFinish;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    runtime::Builder,
    spawn,
    task::{JoinHandle, spawn_blocking},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::timeout,
};
use uuid::Uuid;

use super::SessionError;
use super::send::chat_message_with_text;
use super::{COMMAND_CAPACITY, Channels, NETWORK_THREAD_STACK_SIZE, Session, SessionHandle, SessionOptions, run_client};
use crate::coverage::{CoverageLoss, CoverageLossLedger, CoverageSource};
use crate::event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY};
use crate::fixtures::{
    http::response,
    session::{acknowledge_initial_movement, login_xml, receive_client_message, server_ack, set_sequence, test_fixture},
};
use crate::{Delivery, Grid, LoginCredentials, StartLocation};

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const CIRCUIT_CODE: u32 = 0x1122_3344;
pub(super) const FOLDER_ID: &str = "12345678-1234-5678-1234-567812345678";
pub(super) const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

pub(super) struct EventProbe {
    pub events: mpsc::Sender<EventEnvelope>,
    pub observed: watch::Receiver<Option<u32>>,
}

pub(super) fn test_session(handle: SessionHandle, events: mpsc::Receiver<EventEnvelope>) -> Session {
    test_session_with_coverage(handle, events, CoverageLossLedger::new())
}

pub(super) fn test_session_with_coverage(
    handle: SessionHandle,
    events: mpsc::Receiver<EventEnvelope>,
    coverage_losses: CoverageLossLedger,
) -> Session {
    let coverage_changed = coverage_losses.subscribe();
    Session {
        handle,
        events,
        coverage_losses,
        coverage_changed,
        pending_event: None,
        surfaced_coverage: HashMap::new(),
        surfaced_overflow_generation: 0,
    }
}

pub(super) async fn fake_http(simulator_port: u16) -> (String, oneshot::Receiver<Vec<u8>>, JoinHandle<Vec<Vec<u8>>>) {
    fake_http_with_mfa(simulator_port, false).await
}

pub(super) async fn fake_http_with_event_queue_failure(
    simulator_port: u16,
) -> (String, oneshot::Receiver<Vec<u8>>, oneshot::Sender<()>, JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake HTTP server");
    let address = listener.local_addr().expect("read fake HTTP address");
    let login_body = login_xml(simulator_port, &format!("http://{address}/seed"));
    let seed_body = format!("<llsd><map><key>EventQueueGet</key><string>http://{address}/events</string></map></llsd>");
    let (poll_started, poll_observed) = oneshot::channel();
    let (fail_poll, poll_failure) = oneshot::channel();
    let worker = spawn(async move {
        let mut requests = Vec::with_capacity(3);
        let (mut stream, _) = listener.accept().await.expect("accept fake login request");
        let request = read_http_request(&mut stream).await;
        assert!(request_line(&request).starts_with("POST /login.cgi HTTP/1.1"));
        stream.write_all(&response(200, "OK", "text/xml", None, &login_body)).await.expect("write fake login response");
        requests.push(request);

        let (mut stream, _) = listener.accept().await.expect("accept seed capability request");
        let request = read_http_request(&mut stream).await;
        assert!(request_line(&request).starts_with("POST /seed HTTP/1.1"));
        stream.write_all(&response(200, "OK", "application/llsd+xml", None, &seed_body)).await.expect("write seed response");
        requests.push(request);

        let (mut stream, _) = listener.accept().await.expect("accept EventQueueGet request");
        let request = read_http_request(&mut stream).await;
        let _ = poll_started.send(request.clone());
        requests.push(request);
        let _ = poll_failure.await;
        stream
            .write_all(&response(401, "Unauthorized", "text/plain", None, "denied"))
            .await
            .expect("write permanent EventQueueGet failure");
        requests
    });

    (format!("http://{address}/login.cgi"), poll_observed, fail_poll, worker)
}

pub(super) async fn fake_http_with_mfa(
    simulator_port: u16,
    require_mfa: bool,
) -> (String, oneshot::Receiver<Vec<u8>>, JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake HTTP server");
    let address = listener.local_addr().expect("read fake HTTP address");
    let login_body = login_xml(simulator_port, &format!("http://{address}/seed"));
    let challenge_body = mfa_challenge_xml();
    let seed_body = format!("<llsd><map><key>EventQueueGet</key><string>http://{address}/events</string></map></llsd>");
    let (poll_started, poll_observed) = oneshot::channel();
    let worker = spawn(async move {
        let mut requests = Vec::with_capacity(if require_mfa { 4 } else { 3 });
        let login_responses: Vec<&str> = if require_mfa { vec![challenge_body, login_body.as_str()] } else { vec![login_body.as_str()] };
        for body in login_responses {
            let (mut stream, _) = listener.accept().await.expect("accept fake HTTP request");
            let request = read_http_request(&mut stream).await;
            assert!(request_line(&request).starts_with("POST /login.cgi HTTP/1.1"));
            stream.write_all(&response(200, "OK", "text/xml", None, body)).await.expect("write fake HTTP response");
            requests.push(request);
        }

        let (mut stream, _) = listener.accept().await.expect("accept seed capability request");
        let request = read_http_request(&mut stream).await;
        assert!(request_line(&request).starts_with("POST /seed HTTP/1.1"));
        stream.write_all(&response(200, "OK", "application/llsd+xml", None, &seed_body)).await.expect("write seed response");
        requests.push(request);

        let (mut stream, _) = listener.accept().await.expect("accept EventQueueGet request");
        let request = read_http_request(&mut stream).await;
        let _ = poll_started.send(request.clone());
        requests.push(request);
        let mut byte = [0];
        let _ = stream.read(&mut byte).await;
        requests
    });

    (format!("http://{address}/login.cgi"), poll_observed, worker)
}

pub(super) async fn fake_pending_login_server() -> (String, oneshot::Receiver<()>, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake login server");
    let address = listener.local_addr().expect("read fake login address");
    let (request_started, request_observed) = oneshot::channel();
    let worker = spawn(async move {
        let (mut stream, _) = timeout(CLIENT_TIMEOUT, listener.accept()).await.expect("accept login request").expect("login request");
        let _request = read_http_request(&mut stream).await;
        let _ = request_started.send(());
        let mut byte = [0];
        matches!(timeout(CLIENT_TIMEOUT, stream.read(&mut byte)).await, Ok(Ok(0)))
    });
    (format!("http://{address}/login.cgi"), request_observed, worker)
}

pub(super) async fn fake_mfa_challenge_server() -> (String, oneshot::Sender<()>, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake MFA server");
    let address = listener.local_addr().expect("read fake MFA server address");
    let (stop, stop_requested) = oneshot::channel();
    let worker = spawn(async move {
        let (mut stream, _) =
            timeout(CLIENT_TIMEOUT, listener.accept()).await.expect("accept MFA login request").expect("MFA login request");
        let request = read_http_request(&mut stream).await;
        assert!(request_line(&request).starts_with("POST /login.cgi HTTP/1.1"));
        stream.write_all(&response(200, "OK", "text/xml", None, mfa_challenge_xml())).await.expect("write MFA challenge");
        stop_requested.await.expect("test should stop fake MFA server");
        let listener = listener.into_std().expect("take stopped MFA listener");
        listener.set_nonblocking(true).expect("inspect pending MFA connections without blocking");
        match listener.accept() {
            Ok(_) => true,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(error) => panic!("check for an unexpected MFA response request: {error}"),
        }
    });
    (format!("http://{address}/login.cgi"), stop, worker)
}

pub(super) fn test_credentials() -> LoginCredentials {
    LoginCredentials {
        first_name: "Input".into(),
        last_name: "Name".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: String::new(),
    }
}

pub(super) fn spawn_test_session(login_url: String) -> (Session, SessionHandle, JoinHandle<Result<(), SessionError>>) {
    spawn_session_client(test_credentials(), login_url)
}

pub(super) fn spawn_test_session_with_event_probe(
    login_url: String,
) -> (Session, SessionHandle, JoinHandle<Result<(), SessionError>>, EventProbe) {
    spawn_session_client_with_event_probe(test_credentials(), login_url)
}

pub(super) fn spawn_session_client(
    credentials: LoginCredentials,
    login_url: String,
) -> (Session, SessionHandle, JoinHandle<Result<(), SessionError>>) {
    let (session, handle, worker, _) = spawn_session_client_with_event_probe(credentials, login_url);
    (session, handle, worker)
}

pub(super) fn spawn_session_client_with_event_probe(
    credentials: LoginCredentials,
    login_url: String,
) -> (Session, SessionHandle, JoinHandle<Result<(), SessionError>>, EventProbe) {
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let retained_events = events.clone();
    let (runtime_events, relay, observed) =
        super::relay_handshake_events(events, commands.clone(), CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = runtime_events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let coverage_losses = CoverageLossLedger::new();
    let coverage_changed = coverage_losses.subscribe();
    let (disconnect_requested, disconnect_signal) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let handle = SessionHandle {
        commands: commands.clone(),
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
    };
    let session = Session {
        handle: handle.clone(),
        events: event_receiver,
        coverage_losses: coverage_losses.clone(),
        coverage_changed,
        pending_event: None,
        surfaced_coverage: HashMap::new(),
        surfaced_overflow_generation: 0,
    };
    let worker = spawn(async move {
        let result = run_client(
            credentials,
            Channels {
                commands: command_receiver,
                command_sender: commands,
                events: runtime_events,
                terminal_event_permit,
                disconnect_requested: disconnect_signal,
                coverage_losses,
            },
            login_url,
            SessionOptions::default(),
        )
        .await;
        drop(relay);
        worker_finished_sender.send_replace(true);
        result
    });
    (session, handle, worker, EventProbe { events: retained_events, observed })
}

pub(super) fn spawn_network_thread_session(
    credentials: LoginCredentials,
    login_url: String,
) -> (Session, SessionHandle, JoinHandle<Result<(), SessionError>>) {
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (runtime_events, relay, _) =
        super::relay_handshake_events(events, commands.clone(), CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = runtime_events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let coverage_losses = CoverageLossLedger::new();
    let coverage_changed = coverage_losses.subscribe();
    let (disconnect_requested, disconnect_signal) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let handle = SessionHandle {
        commands: commands.clone(),
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
    };
    let session = Session {
        handle: handle.clone(),
        events: event_receiver,
        coverage_losses: coverage_losses.clone(),
        coverage_changed,
        pending_event: None,
        surfaced_coverage: HashMap::new(),
        surfaced_overflow_generation: 0,
    };
    let network_thread = thread::Builder::new()
        .name("network".into())
        .stack_size(NETWORK_THREAD_STACK_SIZE)
        .spawn(move || {
            let runtime = Builder::new_multi_thread().enable_all().build().expect("start network runtime");
            let channels = Channels {
                commands: command_receiver,
                command_sender: commands,
                events: runtime_events,
                terminal_event_permit,
                disconnect_requested: disconnect_signal.clone(),
                coverage_losses,
            };
            let result = runtime.block_on(run_client(credentials, channels, login_url, SessionOptions::default()));
            runtime.shutdown_background();
            drop(relay);
            worker_finished_sender.send_replace(true);
            result
        })
        .expect("start network thread");
    let worker = spawn_blocking(move || network_thread.join().expect("network thread should not panic"));
    (session, handle, worker)
}

pub(super) fn mfa_challenge_xml() -> &'static str {
    concat!(
        "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>",
        "<member><name>login</name><value><boolean>0</boolean></value></member>",
        "<member><name>reason</name><value><string>mfa_challenge</string></value></member>",
        "</struct></value></param></params></methodResponse>"
    )
}

pub(super) async fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0; 2048];
    loop {
        let count = timeout(CLIENT_TIMEOUT, stream.read(&mut chunk))
            .await
            .expect("fake HTTP request should arrive")
            .expect("read fake HTTP request");
        assert_ne!(count, 0, "fake HTTP client closed before sending its request");
        request.extend_from_slice(&chunk[..count]);
        let Some(body_start) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let content_length = request[..body_start]
            .split(|byte| *byte == b'\n')
            .filter_map(|line| from_utf8(line).ok())
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

pub(super) fn request_line(request: &[u8]) -> &str {
    from_utf8(request).expect("HTTP request is UTF-8").split("\r\n").next().expect("HTTP request line")
}

pub(super) fn request_body(request: &[u8]) -> &str {
    let body_start = request.windows(4).position(|window| window == b"\r\n\r\n").expect("HTTP request has a body") + 4;
    from_utf8(&request[body_start..]).expect("HTTP request body is UTF-8")
}

pub(super) async fn login_next_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
    timeout(CLIENT_TIMEOUT, events.recv())
        .await
        .expect("client should publish an event")
        .expect("client event channel should remain open")
        .into_event()
}

pub(super) fn expected_fixture(name: &str, sequence: u32) -> Vec<u8> {
    let mut bytes = test_fixture(name);
    bytes[0] |= 0x40;
    set_sequence(&mut bytes, sequence);
    bytes
}

pub(super) fn expected_complete_movement(sequence: u32) -> Vec<u8> {
    let mut bytes = expected_fixture("complete_agent_movement.hex", sequence);
    let end = bytes.len();
    bytes[end - 4..].copy_from_slice(&CIRCUIT_CODE.to_le_bytes());
    bytes
}

pub(super) fn reliable_fixture(name: &str, sequence: u32) -> Vec<u8> {
    expected_fixture(name, sequence)
}

pub(super) fn with_appended_ack(mut bytes: Vec<u8>, packet_id: u32) -> Vec<u8> {
    bytes[0] |= 0x10;
    bytes.extend_from_slice(&packet_id.to_be_bytes());
    bytes.push(1);
    bytes
}

pub(super) fn teleport_finish_packet(agent_id: Uuid, endpoint: SocketAddr, region_handle: u64, seed: &str) -> Vec<u8> {
    let SocketAddr::V4(endpoint) = endpoint else { panic!("simulator endpoint must use IPv4") };
    let message = TeleportFinish {
        agent_id,
        location_id: 0,
        sim_ip: endpoint.ip().octets(),
        sim_port: endpoint.port().to_be_bytes(),
        region_handle,
        seed_capability: seed.as_bytes().to_vec(),
        sim_access: Some(0),
        teleport_flags: Some(0),
    };
    Packet::new(25, Message::TeleportFinish(message)).with_reliable().encode().expect("encode TeleportFinish")
}

pub(super) fn expected_packet_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    let mut bytes = test_fixture("packet_ack.hex");
    set_sequence(&mut bytes, sequence);
    bytes[10] = 1;
    bytes[11..15].copy_from_slice(&acknowledged.to_le_bytes());
    bytes.truncate(15);
    bytes
}

#[tokio::test(flavor = "multi_thread")]
async fn network_thread_login() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, retained_handle, worker) = spawn_network_thread_session(test_credentials(), login_url);

    std::assert_matches!(login_next_event(&mut session.events).await, Event::LoginAccepted { .. });
    let (use_circuit, peer, movement, movement_packet) = acknowledge_initial_movement(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));

    let event_request = poll_started.await.expect("EventQueueGet request should be observed");
    assert!(request_line(&event_request).starts_with("POST /events HTTP/1.1"));

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    let agent_session = loop {
        match login_next_event(&mut session.events).await {
            Event::SessionReady { session, .. } => break session,
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    };
    assert_eq!(agent_session.agent_id.to_string(), AGENT_ID);

    drop(session);

    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(worker.await.expect("network thread should join").is_ok());
    assert!(*retained_handle.worker_finished.borrow());
    let requests = http_task.await.expect("fake HTTP server should stop");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
pub(super) async fn event_queue_coverage_loss_keeps_viewer_session_connected() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, fail_poll, http_task) = fake_http_with_event_queue_failure(simulator_address.port()).await;
    let (mut session, retained_handle, worker) = spawn_network_thread_session(test_credentials(), login_url);

    std::assert_matches!(login_next_event(&mut session.events).await, Event::LoginAccepted { .. });
    let (use_circuit, peer, movement, movement_packet) = acknowledge_initial_movement(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    let event_queue_request = timeout(CLIENT_TIMEOUT, poll_started).await.expect("EventQueueGet should start").expect("event request");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (_, reply_packet) = receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    loop {
        match login_next_event(&mut session.events).await {
            Event::SessionReady { .. } => break,
            Event::Failed(error) => panic!("session failed before readiness: {error}"),
            _ => {}
        }
    }

    fail_poll.send(()).expect("release the permanent EventQueueGet failure");
    let mut saw_coverage_loss = false;
    let mut saw_queue_failure = false;
    while !saw_coverage_loss || !saw_queue_failure {
        match timeout(CLIENT_TIMEOUT, session.recv_event()).await.expect("session should keep publishing after EventQueueGet failure") {
            Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) => {
                std::assert_matches!(record.watermark.source, CoverageSource::EventQueue { .. });
                session.acknowledge_coverage_loss(record.watermark);
                saw_coverage_loss = true;
            }
            Some(Event::EventQueueCoverageLost { .. }) => saw_queue_failure = true,
            Some(Event::Failed(error)) => panic!("EventQueueGet failure must not end the viewer session: {error}"),
            Some(_) => {}
            None => panic!("network event stream should remain open after EventQueueGet failure"),
        }
    }

    let send_result = session.send(chat_message_with_text("still connected".into()), Delivery::Reliable);
    let (_, chat_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::ChatFromViewer(_) | Message::LogoutRequest(_))).await;
    if matches!(chat_packet.message(), Message::LogoutRequest(_)) {
        let mut logout_reply = test_fixture("logout_reply.hex");
        logout_reply[0] |= 0x40;
        set_sequence(&mut logout_reply, 26);
        simulator.send_to(&logout_reply, peer).await.expect("reply to unexpected session logout");
        let result =
            timeout(Duration::from_secs(2), worker).await.expect("failed session worker should stop").expect("network thread should join");
        assert!(result.is_err(), "the pre-fix runtime ends with an EventQueueGet failure");
        panic!("EventQueueGet failure sent LogoutRequest instead of preserving the viewer session");
    }
    send_result.expect("session should still accept simulator commands");
    assert!(!*retained_handle.worker_finished.borrow(), "EventQueueGet failure must not finish the network worker");
    std::assert_matches!(chat_packet.message(), Message::ChatFromViewer(message) if message.chat_data.message == "still connected");
    simulator.send_to(&server_ack(25, chat_packet.sequence()), peer).await.expect("ack post-failure chat");

    drop(session);
    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    let mut logout_reply = test_fixture("logout_reply.hex");
    logout_reply[0] |= 0x40;
    set_sequence(&mut logout_reply, 26);
    simulator.send_to(&logout_reply, peer).await.expect("reply to session logout");
    assert!(
        timeout(Duration::from_secs(2), worker)
            .await
            .expect("network worker should stop after explicit logout")
            .expect("thread should join")
            .is_ok()
    );
    assert!(logout.is_reliable());

    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}
