use crate::{Error, Grid, LoginCredentials, LoginValue, StartLocation, event::SimulatorId};
use protocol::udp::{Message, Packet};
use protocol::{
    ChatType,
    messages::{
        CacheMissType, ChatFromViewer, ObjectRequest, RequestAgentData, RequestMultipleObjects,
        chat_from_viewer::{AgentData as ChatAgentData, ChatData},
    },
};
use std::{
    fs,
    future::pending,
    io::Read,
    net::SocketAddr,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
};
use zip::ZipArchive;

use super::{
    COMMAND_CAPACITY, CaptureCommand, CaptureOperationResult, Channels, Command, MfaChallenge, Session, SessionHandle, run_client,
    run_with_capture, stop_capture, wait_for_login,
};
use crate::status::{NetworkLifecycle, NetworkStatusHandle};
use crate::{
    capture::{CaptureConfig, CaptureManifest, CapturePublication, CaptureRecorder, CaptureSession, CaptureState},
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, EventReceiver, EventSendOutcome},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::timeout,
};

fn archive_entry(path: &std::path::Path, name: &str) -> Vec<u8> {
    let file = fs::File::open(path).expect("open capture archive");
    let mut archive = ZipArchive::new(file).expect("decode capture archive");
    let mut entry = archive.by_name(name).expect("find capture entry");
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).expect("read capture entry");
    bytes
}

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
const CIRCUIT_CODE: u32 = 0x1122_3344;
const FOLDER_ID: &str = "12345678-1234-5678-1234-567812345678";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

async fn fake_http(simulator_port: u16) -> (String, oneshot::Receiver<Vec<u8>>, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
    fake_http_with_mfa(simulator_port, false).await
}

async fn fake_http_with_mfa(
    simulator_port: u16,
    require_mfa: bool,
) -> (String, oneshot::Receiver<Vec<u8>>, tokio::task::JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake HTTP server");
    let address = listener.local_addr().expect("read fake HTTP address");
    let login_body = login_xml(simulator_port, &format!("http://{address}/seed"));
    let challenge_body = mfa_challenge_xml();
    let seed_body = format!("<llsd><map><key>EventQueueGet</key><string>http://{address}/events</string></map></llsd>");
    let (poll_started, poll_observed) = oneshot::channel();
    let worker = tokio::spawn(async move {
        let mut requests = Vec::with_capacity(if require_mfa { 4 } else { 3 });
        let login_responses: Vec<&str> = if require_mfa { vec![challenge_body, login_body.as_str()] } else { vec![login_body.as_str()] };
        for body in login_responses {
            let (mut stream, _) = listener.accept().await.expect("accept fake HTTP request");
            let request = read_http_request(&mut stream).await;
            assert!(request_line(&request).starts_with("POST /login.cgi HTTP/1.1"));
            stream.write_all(&http_response(body, "text/xml")).await.expect("write fake HTTP response");
            requests.push(request);
        }

        let (mut stream, _) = listener.accept().await.expect("accept seed capability request");
        let request = read_http_request(&mut stream).await;
        assert!(request_line(&request).starts_with("POST /seed HTTP/1.1"));
        stream.write_all(&http_response(&seed_body, "application/llsd+xml")).await.expect("write seed response");
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

async fn fake_mfa_challenge_server() -> (String, tokio::task::JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake MFA server");
    let address = listener.local_addr().expect("read fake MFA address");
    let worker = tokio::spawn(async move {
        let (mut stream, _) = timeout(CLIENT_TIMEOUT, listener.accept()).await.expect("accept login request").expect("login request");
        let request = read_http_request(&mut stream).await;
        assert!(request_line(&request).starts_with("POST /login.cgi HTTP/1.1"));
        stream.write_all(&http_response(mfa_challenge_xml(), "text/xml")).await.expect("write MFA challenge");

        match timeout(Duration::from_millis(150), listener.accept()).await {
            Err(_) => false,
            Ok(Ok((_stream, _))) => true,
            Ok(Err(error)) => panic!("accept unexpected login request: {error}"),
        }
    });
    (format!("http://{address}/login.cgi"), worker)
}

async fn fake_pending_login_server() -> (String, oneshot::Receiver<()>, tokio::task::JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake login server");
    let address = listener.local_addr().expect("read fake login address");
    let (request_started, request_observed) = oneshot::channel();
    let worker = tokio::spawn(async move {
        let (mut stream, _) = timeout(CLIENT_TIMEOUT, listener.accept()).await.expect("accept login request").expect("login request");
        let _request = read_http_request(&mut stream).await;
        let _ = request_started.send(());
        let mut byte = [0];
        matches!(timeout(CLIENT_TIMEOUT, stream.read(&mut byte)).await, Ok(Ok(0)))
    });
    (format!("http://{address}/login.cgi"), request_observed, worker)
}

fn test_credentials() -> LoginCredentials {
    LoginCredentials {
        first_name: "Input".into(),
        last_name: "Name".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: String::new(),
    }
}

fn spawn_test_session(login_url: String) -> (Session, SessionHandle, tokio::task::JoinHandle<Result<(), String>>) {
    spawn_session_client(test_credentials(), login_url)
}

fn spawn_test_session_with_event_sender(
    login_url: String,
) -> (Session, SessionHandle, tokio::task::JoinHandle<Result<(), String>>, mpsc::Sender<EventEnvelope>) {
    spawn_session_client_with_event_sender(test_credentials(), login_url)
}

fn spawn_session_client(
    credentials: LoginCredentials,
    login_url: String,
) -> (Session, SessionHandle, tokio::task::JoinHandle<Result<(), String>>) {
    let (session, handle, worker, _) = spawn_session_client_with_event_sender(credentials, login_url);
    (session, handle, worker)
}

fn spawn_session_client_with_event_sender(
    credentials: LoginCredentials,
    login_url: String,
) -> (Session, SessionHandle, tokio::task::JoinHandle<Result<(), String>>, mpsc::Sender<EventEnvelope>) {
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + crate::event::PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let retained_events = events.clone();
    let (disconnect_requested, disconnect_signal) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let (capture_commands, _capture_command_receiver) = mpsc::channel(8);
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands: commands.clone(),
        disconnect_requested,
        worker_finished,
        capture_commands,
        capture: CaptureRecorder::new(),
        status: status.clone(),
    };
    let session = Session { handle: handle.clone(), events: event_receiver };
    let worker = tokio::spawn(async move {
        let result = run_client(
            credentials,
            Channels {
                commands: command_receiver,
                command_sender: commands,
                events,
                terminal_event_permit,
                disconnect_requested: disconnect_signal,
            },
            None,
            status,
            login_url,
        )
        .await;
        worker_finished_sender.send_replace(true);
        result
    });
    (session, handle, worker, retained_events)
}

fn spawn_network_thread_session(
    credentials: LoginCredentials,
    login_url: String,
) -> (Session, SessionHandle, tokio::task::JoinHandle<Result<(), String>>) {
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + crate::event::PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let (disconnect_requested, disconnect_signal) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let (capture_commands, capture_command_receiver) = mpsc::channel(8);
    let capture = CaptureRecorder::new();
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands: commands.clone(),
        disconnect_requested,
        worker_finished,
        capture_commands,
        capture: capture.clone(),
        status: status.clone(),
    };
    let session = Session { handle: handle.clone(), events: event_receiver };
    let network_thread = thread::Builder::new()
        .name("network".into())
        .stack_size(super::NETWORK_THREAD_STACK_SIZE)
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("start network runtime");
            let channels = Channels {
                commands: command_receiver,
                command_sender: commands,
                events: events.clone(),
                terminal_event_permit,
                disconnect_requested: disconnect_signal.clone(),
            };
            let run = run_client(credentials, channels, Some(capture.clone()), status, login_url);
            let result = runtime.block_on(run_with_capture(run, events, disconnect_signal, capture, None, capture_command_receiver));
            runtime.shutdown_background();
            worker_finished_sender.send_replace(true);
            result
        })
        .expect("start network thread");
    let worker = tokio::task::spawn_blocking(move || network_thread.join().expect("network thread should not panic"));
    (session, handle, worker)
}

#[tokio::test(flavor = "multi_thread")]
async fn network_thread_login() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, retained_handle, worker) = spawn_network_thread_session(test_credentials(), login_url);

    assert!(matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. }));
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");

    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");

    let event_request = timeout(CLIENT_TIMEOUT, poll_started).await.expect("EventQueueGet should start").expect("event request");
    assert!(request_line(&event_request).starts_with("POST /events HTTP/1.1"));

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    let agent_session = loop {
        match next_event(&mut session.events).await {
            Event::SessionReady { session, .. } => break session,
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    };
    assert_eq!(agent_session.agent_id.to_string(), AGENT_ID);

    drop(session);

    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(timeout(Duration::from_secs(2), worker).await.expect("network thread should stop").expect("thread should join").is_ok());
    assert!(*retained_handle.worker_finished.borrow());
    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}

fn mfa_challenge_xml() -> &'static str {
    concat!(
        "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>",
        "<member><name>login</name><value><boolean>0</boolean></value></member>",
        "<member><name>reason</name><value><string>mfa_challenge</string></value></member>",
        "</struct></value></param></params></methodResponse>"
    )
}

fn login_xml(simulator_port: u16, seed_url: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>",
            "<member><name>login</name><value><boolean>1</boolean></value></member>",
            "<member><name>first_name</name><value><string>Session</string></value></member>",
            "<member><name>last_name</name><value><string>Avatar</string></value></member>",
            "<member><name>message</name><value><string>Welcome</string></value></member>",
            "<member><name>sim_ip</name><value><string>127.0.0.1</string></value></member>",
            "<member><name>sim_port</name><value><int>{simulator_port}</int></value></member>",
            "<member><name>circuit_code</name><value><int>{circuit_code}</int></value></member>",
            "<member><name>agent_id</name><value><string>{agent_id}</string></value></member>",
            "<member><name>session_id</name><value><string>{session_id}</string></value></member>",
            "<member><name>seed_capability</name><value><string>{seed_url}</string></value></member>",
            "<member><name>mfa_hash</name><value><string>returned-mfa</string></value></member>",
            "<member><name>inventory-skeleton</name><value><array><data><value><struct>",
            "<member><name>folder_id</name><value><string>{folder_id}</string></value></member>",
            "<member><name>name</name><value><string>Favorites</string></value></member>",
            "</struct></value></data></array></value></member>",
            "</struct></value></param></params></methodResponse>"
        ),
        simulator_port = simulator_port,
        seed_url = seed_url,
        circuit_code = CIRCUIT_CODE,
        agent_id = AGENT_ID,
        session_id = SESSION_ID,
        folder_id = FOLDER_ID,
    )
}

async fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
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
            .filter_map(|line| std::str::from_utf8(line).ok())
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

fn http_response(body: &str, content_type: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
        .into_bytes()
}

fn request_line(request: &[u8]) -> &str {
    std::str::from_utf8(request).expect("HTTP request is UTF-8").split("\r\n").next().expect("HTTP request line")
}

fn request_body(request: &[u8]) -> &str {
    let body_start = request.windows(4).position(|window| window == b"\r\n\r\n").expect("HTTP request has a body") + 4;
    std::str::from_utf8(&request[body_start..]).expect("HTTP request body is UTF-8")
}

async fn receive_udp(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, peer) = timeout(CLIENT_TIMEOUT, socket.recv_from(&mut buffer))
        .await
        .expect("fake simulator should receive a datagram")
        .expect("read simulator datagram");
    (buffer[..size].to_vec(), peer)
}

async fn receive_client_message(
    socket: &UdpSocket,
    expected_peer: SocketAddr,
    is_expected: impl Fn(&Message) -> bool,
) -> (Vec<u8>, Packet) {
    loop {
        let (bytes, peer) = receive_udp(socket).await;
        assert_eq!(peer, expected_peer);
        let packet = Packet::decode(&bytes).expect("decode client UDP packet");
        if is_expected(packet.message()) {
            return (bytes, packet);
        }
        if matches!(packet.message(), Message::StartPingCheck(_)) {
            assert!(!packet.is_reliable());
            continue;
        }
        if matches!(packet.message(), Message::PacketAck(_)) {
            assert!(!packet.is_reliable());
            continue;
        }
        panic!("unexpected client UDP message: {:?}", packet.message());
    }
}

async fn next_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
    timeout(CLIENT_TIMEOUT, events.recv())
        .await
        .expect("client should publish an event")
        .expect("client event channel should remain open")
        .into_event()
}

fn fixture(name: &str) -> Vec<u8> {
    let source = test_support::udp::vectors::get(name).unwrap_or_else(|| panic!("unknown fixture: {name}"));
    test_support::parse_hex_fixture(source).unwrap_or_else(|error| panic!("invalid fixture {name}: {error}"))
}

fn expected_fixture(name: &str, sequence: u32) -> Vec<u8> {
    let mut bytes = fixture(name);
    bytes[0] |= 0x40;
    set_sequence(&mut bytes, sequence);
    bytes
}

fn expected_complete_movement(sequence: u32) -> Vec<u8> {
    let mut bytes = expected_fixture("complete_agent_movement.hex", sequence);
    let end = bytes.len();
    bytes[end - 4..].copy_from_slice(&CIRCUIT_CODE.to_le_bytes());
    bytes
}

fn reliable_fixture(name: &str, sequence: u32) -> Vec<u8> {
    expected_fixture(name, sequence)
}

fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}

fn with_appended_ack(mut bytes: Vec<u8>, packet_id: u32) -> Vec<u8> {
    bytes[0] |= 0x10;
    bytes.extend_from_slice(&packet_id.to_le_bytes());
    bytes.push(1);
    bytes
}

fn server_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&[0, 0xff, 0xff, 0xff, 0xfb, 1]);
    bytes.extend_from_slice(&acknowledged.to_le_bytes());
    bytes
}

fn expected_packet_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    let mut bytes = fixture("packet_ack.hex");
    set_sequence(&mut bytes, sequence);
    bytes[10] = 1;
    bytes[11..15].copy_from_slice(&acknowledged.to_le_bytes());
    bytes.truncate(15);
    bytes
}

#[tokio::test]
async fn disconnect_cancels_login() {
    let (disconnect, mut disconnect_requested) = watch::channel(false);
    let login = wait_for_login(pending::<()>(), &mut disconnect_requested);
    disconnect.send_replace(true);

    assert!(timeout(Duration::from_secs(1), login).await.expect("login cancellation should return promptly").is_none());
}

#[tokio::test]
async fn disconnect_skips_login() {
    let (_disconnect, mut disconnect_requested) = watch::channel(true);

    assert!(wait_for_login(pending::<()>(), &mut disconnect_requested).await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn session_drop_cancels_bootstrap() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, retained_handle, worker) = spawn_test_session(login_url);

    assert!(matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. }));
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");

    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");

    let event_queue_request = timeout(Duration::from_secs(3), poll_started)
        .await
        .expect("bootstrap should start EventQueueGet")
        .expect("fake HTTP server should observe EventQueueGet");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));
    drop(session);

    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(timeout(Duration::from_secs(2), worker).await.expect("bootstrap worker should stop").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());

    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("server task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_handoff() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let credentials = LoginCredentials {
        first_name: "Input".into(),
        last_name: "Name".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: "saved-mfa".into(),
    };
    let (mut network_session, retained_handle, client) = spawn_session_client(credentials, login_url);

    assert!(
        matches!(next_event(&mut network_session.events).await, Event::LoginAccepted { mfa_hash: Some(hash) } if hash == "returned-mfa")
    );
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));

    let event_queue_request = timeout(Duration::from_secs(3), poll_started)
        .await
        .expect("simulator startup should request EventQueueGet")
        .expect("fake HTTP server should observe EventQueueGet");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));

    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");

    let region = reliable_fixture("region_handshake.hex", 22);
    simulator.send_to(&region, peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");

    let movement_complete = reliable_fixture("agent_movement_complete.hex", 24);
    simulator.send_to(&movement_complete, peer).await.expect("send AgentMovementComplete");
    let (agent_session, login_data) = loop {
        match next_event(&mut network_session.events).await {
            Event::SessionReady { session, login_data, .. } => break (session, login_data),
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    };

    assert_eq!(agent_session.first_name, "Session");
    assert_eq!(agent_session.last_name, "Avatar");
    assert_eq!(agent_session.grid, Grid::Aditi);
    assert_eq!(agent_session.simulator_address, simulator_address);
    assert_eq!(agent_session.agent_id.to_string(), AGENT_ID);
    assert_eq!(agent_session.session_id.to_string(), SESSION_ID);
    assert_eq!(agent_session.circuit_code, CIRCUIT_CODE);
    let LoginValue::Array(folders) = login_data.get("inventory-skeleton").expect("login metadata survives handoff") else {
        panic!("expected inventory skeleton array");
    };
    let [LoginValue::Struct(folder)] = folders.as_slice() else {
        panic!("expected one inventory skeleton folder");
    };
    assert_eq!(folder.get("folder_id"), Some(&LoginValue::String(FOLDER_ID.into())));
    assert_eq!(folder.get("name"), Some(&LoginValue::String("Favorites".into())));

    drop(network_session);
    let mut next_sequence = crate::circuit::advance_sequence(reply_packet.sequence());
    let mut movement_acknowledged = false;
    loop {
        let (bytes, shutdown_peer) = receive_udp(&simulator).await;
        assert_eq!(shutdown_peer, peer);
        let packet = Packet::decode(&bytes).expect("decode simulator shutdown packet");
        assert_eq!(packet.sequence(), next_sequence);
        next_sequence = crate::circuit::advance_sequence(next_sequence);

        let mut acknowledgements = packet.appended_acks().to_vec();
        if let Message::PacketAck(message) = packet.message() {
            acknowledgements.extend(message.packets.iter().map(|packet| packet.id));
        }
        for acknowledged in acknowledgements {
            assert_eq!(acknowledged, 24, "shutdown should acknowledge AgentMovementComplete");
            assert!(!movement_acknowledged, "AgentMovementComplete must only be acknowledged once");
            movement_acknowledged = true;
        }

        match packet.message() {
            Message::PacketAck(_) => {
                assert!(packet.appended_acks().is_empty());
                assert_eq!(bytes, expected_packet_ack(packet.sequence(), 24));
            }
            Message::StartPingCheck(_) => assert!(!packet.is_reliable()),
            Message::LogoutRequest(_) => {
                let mut expected = expected_fixture("logout_request.hex", packet.sequence());
                if packet.appended_acks() == [24] {
                    expected = with_appended_ack(expected, 24);
                }
                assert_eq!(bytes, expected);
                assert!(movement_acknowledged, "logout must acknowledge AgentMovementComplete");
                break;
            }
            message => panic!("unexpected simulator shutdown message: {message:?}"),
        }
    }
    assert!(timeout(Duration::from_secs(2), client).await.expect("disconnect should return promptly").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());

    let requests = timeout(Duration::from_secs(2), http_task).await.expect("local HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
    assert!(request_line(&requests[0]).starts_with("POST /login.cgi HTTP/1.1"));
    assert!(request_body(&requests[0]).contains("login_to_simulator"));
    assert!(request_line(&requests[1]).starts_with("POST /seed HTTP/1.1"));
    assert!(request_body(&requests[1]).contains("EventQueueGet"));
    assert!(request_line(&requests[2]).starts_with("POST /events HTTP/1.1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mfa_continuation() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http_with_mfa(simulator_address.port(), true).await;
    let credentials = LoginCredentials {
        first_name: "Input".into(),
        last_name: "Name".into(),
        password: "fixture-password".into(),
        grid: Grid::Aditi,
        start_location: StartLocation::Last,
        mfa_hash: "saved-mfa".into(),
    };
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + crate::event::PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = events_sender.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let (disconnect, disconnect_requested) = watch::channel(false);
    let client = tokio::spawn(run_client(
        credentials,
        Channels {
            commands: command_receiver,
            command_sender: commands.clone(),
            events: events_sender,
            terminal_event_permit,
            disconnect_requested,
        },
        None,
        NetworkStatusHandle::default(),
        login_url,
    ));

    let challenge = match next_event(&mut events).await {
        Event::MfaRequired { challenge } => challenge,
        other => panic!("expected MFA challenge, got {other:?}"),
    };
    challenge.complete(" 123456 ").expect("complete login challenge");
    assert!(matches!(next_event(&mut events).await, Event::LoginAccepted { mfa_hash: Some(hash) } if hash == "returned-mfa"));
    let event_request = timeout(Duration::from_secs(2), poll_started).await.expect("EventQueueGet should start").expect("event request");
    assert!(request_line(&event_request).starts_with("POST /events HTTP/1.1"));

    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    let session = loop {
        match next_event(&mut events).await {
            Event::SessionReady { session, .. } => break session,
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    };
    assert_eq!(session.agent_id.to_string(), AGENT_ID);

    disconnect.send_replace(true);
    let (logout_bytes, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert_eq!(logout.appended_acks(), &[24]);
    assert_eq!(logout_bytes, with_appended_ack(expected_fixture("logout_request.hex", logout.sequence()), 24));
    assert!(timeout(Duration::from_secs(2), client).await.expect("client should disconnect").expect("client task").is_ok());

    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 4);
    let initial_login = request_body(&requests[0]);
    let continued_login = request_body(&requests[1]);
    assert!(initial_login.contains("saved-mfa"));
    assert!(!initial_login.contains("123456"));
    assert!(continued_login.contains("123456"));
    assert!(!continued_login.contains("saved-mfa"));
}

#[tokio::test]
async fn session_drop_cancels_login() {
    let (login_url, request_started, server) = fake_pending_login_server().await;
    let (session, retained_handle, worker) = spawn_test_session(login_url);

    request_started.await.expect("login request should start");
    drop(session);

    assert!(timeout(Duration::from_secs(1), worker).await.expect("login worker should stop").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());
    assert!(timeout(Duration::from_secs(1), server).await.expect("fake login server should stop").expect("server task"));
}

#[tokio::test]
async fn session_drop_cancels_mfa() {
    let (login_url, server) = fake_mfa_challenge_server().await;
    let (mut session, retained_handle, worker) = spawn_test_session(login_url);

    let challenge = match next_event(&mut session.events).await {
        Event::MfaRequired { challenge } => challenge,
        other => panic!("expected MFA challenge, got {other:?}"),
    };
    drop(session);

    assert!(timeout(Duration::from_secs(1), worker).await.expect("MFA worker should stop").expect("client task").is_ok());
    assert!(*retained_handle.worker_finished.borrow());
    assert!(!timeout(Duration::from_secs(1), server).await.expect("fake server should finish").expect("server task"));
    drop(challenge);
}

#[tokio::test]
async fn session_drop_cancels_full_event() {
    let (commands, _command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY);
    for _ in 0..CLIENT_EVENT_CAPACITY {
        events.try_send(EventEnvelope::new(Event::Failed("fill event queue".into()))).expect("fill the event queue");
    }
    let (disconnect, mut disconnect_requested) = watch::channel(false);
    let session = Session { handle: targeted_send_handle(commands, disconnect), events: event_receiver };
    let events_sender = events.clone();
    let mut publish = tokio::spawn(async move {
        crate::event::send_client_event(&events_sender, Event::Failed("pending event".into()), &mut disconnect_requested).await
    });

    assert!(timeout(Duration::from_millis(10), &mut publish).await.is_err(), "full event queue should block publication");
    let retained_handle = session.handle();
    drop(session);

    assert!(matches!(
        timeout(Duration::from_secs(1), publish).await.expect("session drop should release event publication").expect("publisher task"),
        EventSendOutcome::Shutdown
    ));
    assert!(*retained_handle.disconnect_requested.borrow());
}

#[tokio::test]
async fn receive_releases_capability_bytes() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let budget = std::sync::Arc::new(tokio::sync::Semaphore::new(32));
    let permit = budget.clone().acquire_many_owned(17).await.expect("reserve capability bytes");
    let (events, event_receiver) = mpsc::channel(1);
    events
        .send(EventEnvelope::with_capability_bytes(
            Event::CapabilityEvent {
                simulator_id: SimulatorId::new(77),
                source: "127.0.0.1:13000".parse().expect("simulator endpoint"),
                message: "TestEvent".into(),
                body: llsd::Value::Undefined,
            },
            permit,
        ))
        .await
        .expect("queue capability event");
    let mut session = Session { handle: targeted_send_handle(commands, disconnect), events: event_receiver };

    assert_eq!(budget.available_permits(), 15);
    assert!(matches!(session.recv_event().await, Some(Event::CapabilityEvent { message, .. }) if message == "TestEvent"));
    assert_eq!(budget.available_permits(), 32);
}

#[tokio::test(flavor = "multi_thread")]
async fn session_drop_cancels_active_saturated_session() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, retained_handle, worker, event_sender) = spawn_test_session_with_event_sender(login_url);

    assert!(matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. }));
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (reply, reply_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    assert_eq!(reply, with_appended_ack(expected_fixture("region_handshake_reply.hex", reply_packet.sequence()), 22));
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    loop {
        match next_event(&mut session.events).await {
            Event::SessionReady { session: agent_session, .. } => {
                assert_eq!(agent_session.agent_id.to_string(), AGENT_ID);
                break;
            }
            Event::Failed(error) => panic!("client failed before session readiness: {error}"),
            _ => {}
        }
    }
    while session.events.try_recv().is_ok() {}
    timeout(CLIENT_TIMEOUT, poll_started).await.expect("EventQueueGet should start").expect("EventQueueGet request");

    loop {
        match event_sender.try_send(EventEnvelope::new(Event::Failed("hold saturated session events".into()))) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => break,
            Err(mpsc::error::TrySendError::Closed(_)) => panic!("active session event receiver should remain open"),
        }
    }
    assert_eq!(event_sender.capacity(), 0);

    simulator.send_to(&reliable_fixture("chat_from_simulator.hex", 25), peer).await.expect("send while application delivery is saturated");
    timeout(CLIENT_TIMEOUT, async {
        loop {
            let (bytes, _) = receive_udp(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode chat acknowledgement");
            if packet.appended_acks().contains(&25)
                || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|item| item.id == 25))
            {
                break;
            }
        }
    })
    .await
    .expect("the client should process reliable control while events are saturated");
    assert!(!*retained_handle.worker_finished.borrow());

    let unexpected_logout = timeout(Duration::from_millis(50), async {
        receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await
    })
    .await;
    assert!(unexpected_logout.is_err(), "one queued application event should not close a healthy session");

    drop(session);
    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(
        timeout(Duration::from_secs(2), worker).await.expect("active worker should stop after Session drop").expect("client task").is_ok()
    );
    assert!(*retained_handle.worker_finished.borrow());
    let requests = timeout(Duration::from_secs(2), http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test]
async fn disconnect_waits() {
    let (commands, _command_receiver) = mpsc::channel(1);
    let (disconnect_requested, mut disconnect_state) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let (capture_commands, _capture_command_receiver) = mpsc::channel(1);
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands,
        disconnect_requested,
        worker_finished,
        capture_commands,
        capture: CaptureRecorder::new(),
        status: status.clone(),
    };
    let mut disconnect = tokio::spawn(async move { handle.disconnect().await });

    assert!(disconnect_state.changed().await.is_ok());
    assert_eq!(status.snapshot().lifecycle, NetworkLifecycle::Disconnecting);
    assert!(timeout(Duration::from_millis(10), &mut disconnect).await.is_err());

    worker_finished_sender.send_replace(true);
    timeout(Duration::from_secs(1), disconnect).await.expect("worker completion should release disconnect").expect("disconnect task");
}

#[tokio::test]
async fn targeted_send_keeps_id() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let simulator_id = SimulatorId::new(42);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: base::UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: base::UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    };
    let send = tokio::spawn(async move { handle.send_to(simulator_id, request, crate::Delivery::Reliable).await });

    let Some(Command::SendTo { simulator_id: routed_id, message, delivery, completed }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };
    assert_eq!(routed_id, simulator_id);
    assert_eq!(delivery, crate::Delivery::Reliable);
    assert!(matches!(message, protocol::udp::Message::RequestMultipleObjects(_)));
    completed.send(Ok(())).expect("return queue admission result");
    assert!(send.await.expect("send task").is_ok());
}

#[tokio::test]
async fn targeted_send_full() {
    let (commands, _command_receiver) = mpsc::channel(1);
    commands.try_send(Command::SubmitMfaCode("queued command".into())).expect("fill the session command queue");
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: base::UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: base::UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: Vec::new(),
    };

    assert!(matches!(
        handle.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await,
        Err(Error::SimulatorCommandQueueFull)
    ));
}

#[tokio::test]
async fn targeted_send_target_closed() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: base::UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: base::UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    };
    let send = tokio::spawn(async move { handle.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    drop(completed);

    assert!(matches!(send.await.expect("send task"), Err(Error::SimulatorConnectionClosed)));
}

#[tokio::test]
async fn targeted_send_session_closed() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_signal, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_signal.clone());
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: base::UUID::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: base::UUID::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    };
    let send = tokio::spawn(async move { handle.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    disconnect_signal.send_replace(true);
    drop(completed);

    assert!(matches!(send.await.expect("send task"), Err(Error::SessionClosed)));
}

#[tokio::test]
async fn targeted_send_reports_finished_worker() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let (capture_commands, _) = mpsc::channel(1);
    let handle = SessionHandle {
        commands,
        disconnect_requested,
        worker_finished,
        capture_commands,
        capture: CaptureRecorder::new(),
        status: NetworkStatusHandle::new(),
    };
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: base::UUID::nil(), session_id: base::UUID::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    };
    let send = tokio::spawn(async move { handle.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    worker_finished_sender.send_replace(true);
    drop(completed);

    assert!(matches!(send.await.expect("send task"), Err(Error::SessionClosed)));
}

fn targeted_send_handle(commands: mpsc::Sender<Command>, disconnect_requested: watch::Sender<bool>) -> SessionHandle {
    let (_, worker_finished) = watch::channel(false);
    let (capture_commands, _) = mpsc::channel(1);
    SessionHandle {
        commands,
        disconnect_requested,
        worker_finished,
        capture_commands,
        capture: CaptureRecorder::new(),
        status: NetworkStatusHandle::new(),
    }
}

fn chat_message_with_size(size: usize) -> ChatFromViewer {
    chat_message_with_text("x".repeat(size))
}

fn chat_message_with_text(message: String) -> ChatFromViewer {
    ChatFromViewer {
        agent_data: ChatAgentData { agent_id: base::UUID::nil(), session_id: base::UUID::nil() },
        chat_data: ChatData { message, chat_type: ChatType::Normal, channel: 0 },
    }
}

#[test]
fn outbound_chat_limit_precedes_queue_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);

    assert!(matches!(
        handle.send(chat_message_with_size(super::MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1), crate::Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    ));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    let mut oversized_capacity = String::with_capacity(super::MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1);
    oversized_capacity.push('x');
    assert!(matches!(
        handle.send(chat_message_with_text(oversized_capacity), crate::Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    ));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    handle.send(chat_message_with_size(super::MAX_OUTBOUND_CHAT_MESSAGE_BYTES), crate::Delivery::Unreliable).expect("exact limit fits");
    assert_eq!(commands.capacity(), 0);
    let Some(Command::SendMessage { message: Message::ChatFromViewer(message), .. }) = command_receiver.try_recv().ok() else {
        panic!("expected the exact-limit chat message");
    };
    assert_eq!(message.chat_data.message.len(), super::MAX_OUTBOUND_CHAT_MESSAGE_BYTES);
}

#[tokio::test]
async fn object_request_limit_precedes_targeted_queue_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: base::UUID::nil(), session_id: base::UUID::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }; super::MAX_OUTBOUND_OBJECT_REQUESTS + 1],
    };

    assert!(matches!(handle.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge)));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    let mut oversized_capacity = Vec::with_capacity(super::MAX_OUTBOUND_OBJECT_REQUESTS + 1);
    oversized_capacity.push(ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 });
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: base::UUID::nil(), session_id: base::UUID::nil() },
        objects: oversized_capacity,
    };
    assert!(matches!(handle.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge)));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn object_request_at_limit_is_accepted() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let mut objects = Vec::with_capacity(super::MAX_OUTBOUND_OBJECT_REQUESTS);
    objects.resize(super::MAX_OUTBOUND_OBJECT_REQUESTS, ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 });
    let request =
        RequestMultipleObjects { agent_data: RequestAgentData { agent_id: base::UUID::nil(), session_id: base::UUID::nil() }, objects };

    let send = tokio::spawn(async move { handle.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await });
    let Some(Command::SendTo { message: Message::RequestMultipleObjects(message), completed, .. }) = command_receiver.recv().await else {
        panic!("expected an accepted object request");
    };
    assert_eq!(message.objects.len(), super::MAX_OUTBOUND_OBJECT_REQUESTS);
    completed.send(Ok(())).expect("send result receiver");
    assert!(send.await.expect("send task").is_ok());
}

#[tokio::test]
async fn session_send_wrappers_reject_oversized_messages_before_queue_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);
    let session = Session { handle, events: mpsc::channel(1).1 };
    assert!(matches!(
        session.send(chat_message_with_size(super::MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1), crate::Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    ));

    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: base::UUID::nil(), session_id: base::UUID::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }; super::MAX_OUTBOUND_OBJECT_REQUESTS + 1],
    };
    assert!(matches!(session.send_to(SimulatorId::new(42), request, crate::Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge)));

    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[test]
fn session_drop_requests_disconnect() {
    let (commands, _command_receiver) = mpsc::channel(1);
    let (disconnect_requested, disconnect_observed) = watch::channel(false);
    let (_worker_finished_sender, worker_finished) = watch::channel(false);
    let (capture_commands, _capture_command_receiver) = mpsc::channel(1);
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands,
        disconnect_requested,
        worker_finished,
        capture_commands,
        capture: CaptureRecorder::new(),
        status: status.clone(),
    };
    let session = Session { handle, events: mpsc::channel(1).1 };

    drop(session);

    assert!(*disconnect_observed.borrow());
    assert_eq!(status.snapshot().lifecycle, NetworkLifecycle::Disconnecting);
}

#[tokio::test]
async fn shutdown_cancels_error() {
    let (events, _event_receiver) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("fill event queue".into()))).expect("fill event queue");
    let (shutdown, mut shutdown_requested) = watch::channel(false);
    let failure_events = events.clone();
    let mut send_failure =
        tokio::spawn(async move { super::send_failure_event(&failure_events, "network failed".into(), &mut shutdown_requested).await });

    assert!(timeout(Duration::from_millis(10), &mut send_failure).await.is_err(), "full queue should apply backpressure");
    shutdown.send_replace(true);
    let outcome =
        timeout(Duration::from_secs(1), send_failure).await.expect("shutdown should release the blocked send").expect("send task");

    assert!(matches!(outcome, EventSendOutcome::Shutdown));
}

#[tokio::test]
async fn late_interruption_keeps_capture_complete() {
    let directory = TempDirectory::new();
    let recorder = CaptureRecorder::new();
    let capture_root = directory.path().join("captures");
    let session = CaptureSession::start(CaptureConfig::new(&capture_root), recorder).expect("start capture");
    let capture_path = session.path().to_path_buf();
    let (events, mut received_events) = mpsc::channel(1);
    let publication = CapturePublication::default();

    let CaptureOperationResult::Stopped(Ok(summary)) = stop_capture(session, publication.clone(), events).await else {
        panic!("capture stop should publish an archive");
    };
    publication.request_interruption();

    assert_eq!(summary.state, CaptureState::Complete, "capture finalization error: {:?}", summary.error);
    assert!(
        matches!(received_events.try_recv().map(EventEnvelope::into_event), Ok(Event::CaptureFinished { path, error: None }) if path == capture_path)
    );
    let manifest = archive_entry(&capture_path, "manifest.json");
    let manifest: CaptureManifest = serde_json::from_slice(&manifest).expect("decode capture manifest");
    assert_eq!(manifest.state, CaptureState::Complete);
}

#[tokio::test]
async fn pre_publish_interruption_stays_interrupted() {
    let directory = TempDirectory::new();
    let recorder = CaptureRecorder::new();
    let capture_root = directory.path().join("captures");
    let session = CaptureSession::start(CaptureConfig::new(&capture_root), recorder).expect("start capture");
    let capture_path = session.path().to_path_buf();
    let (events, mut received_events) = mpsc::channel(1);
    let publication = CapturePublication::default();
    publication.request_interruption();

    let CaptureOperationResult::Stopped(Ok(summary)) = stop_capture(session, publication, events).await else {
        panic!("capture stop should publish an archive");
    };

    assert_eq!(summary.state, CaptureState::Interrupted, "capture finalization error: {:?}", summary.error);
    assert!(
        matches!(received_events.try_recv().map(EventEnvelope::into_event), Ok(Event::CaptureInterrupted { path, error: None }) if path == capture_path)
    );
    let manifest = archive_entry(&capture_path, "manifest.json");
    let manifest: CaptureManifest = serde_json::from_slice(&manifest).expect("decode capture manifest");
    assert_eq!(manifest.state, CaptureState::Interrupted);
}

#[tokio::test]
async fn shutdown_during_publish_interrupts_capture() {
    let directory = TempDirectory::new();
    let recorder = CaptureRecorder::new();
    let capture_root = directory.path().join("captures");
    let session = CaptureSession::start(CaptureConfig::new(&capture_root), recorder.clone()).expect("start capture");
    let capture_path = session.path().to_path_buf();
    let (publication_reached, at_publication) = oneshot::channel();
    let (continue_publication, release_publication) = std::sync::mpsc::channel();
    let publication = CapturePublication::default();
    publication.pause_before_claim(publication_reached, release_publication);

    let (events, mut received_events) = mpsc::channel(1);
    let (network_finished, wait_for_network) = oneshot::channel();
    let network = async move {
        wait_for_network.await.map_err(|_| "test ended before network shutdown".to_string())?;
        Ok(())
    };
    let (shutdown, shutdown_requested) = watch::channel(false);
    let (capture_command_sender, capture_commands) = mpsc::channel(1);
    let coordinator = tokio::spawn(run_with_capture(network, events, shutdown_requested, recorder, Some(session), capture_commands));
    let (stop_completed, stop_result) = oneshot::channel();
    capture_command_sender.send(CaptureCommand::Stop { completed: stop_completed, publication }).await.expect("queue capture stop");

    timeout(Duration::from_secs(2), at_publication).await.expect("stop should reach archive publication").expect("publication signal");
    network_finished.send(()).expect("network task should still be waiting");
    shutdown.send_replace(true);

    let stop_result =
        timeout(Duration::from_secs(2), stop_result).await.expect("shutdown should finish the stop request").expect("stop reply");
    assert!(matches!(stop_result, Err(error) if error.contains("shutting down")));
    timeout(Duration::from_secs(2), coordinator)
        .await
        .expect("capture coordinator should return before archive publication resumes")
        .expect("capture coordinator should not panic")
        .expect("network shutdown should succeed");
    assert!(!capture_path.exists(), "archive should remain unpublished while the writer is paused");

    continue_publication.send(()).expect("capture writer should be paused");
    let event = timeout(Duration::from_secs(2), received_events.recv()).await.expect("capture writer should finish publication");
    assert!(matches!(
        event.map(EventEnvelope::into_event),
        Some(Event::CaptureInterrupted { path, error: None }) if path == capture_path
    ));
    let manifest: CaptureManifest =
        serde_json::from_slice(&archive_entry(&capture_path, "manifest.json")).expect("decode capture manifest");
    assert_eq!(manifest.state, CaptureState::Interrupted);
}

#[tokio::test]
async fn capture_stop_completes_when_event_queue_is_full() {
    let directory = TempDirectory::new();
    let recorder = CaptureRecorder::new();
    let capture_root = directory.path().join("captures");
    let session = CaptureSession::start(CaptureConfig::new(&capture_root), recorder.clone()).expect("start capture");
    let (events, mut received_events) = mpsc::channel(1);
    events.try_send(EventEnvelope::new(Event::Failed("occupy event queue".into()))).expect("fill event queue");
    let (network_finished, wait_for_network) = oneshot::channel();
    let network = async move {
        wait_for_network.await.map_err(|_| "network test ended early".to_string())?;
        Ok(())
    };
    let (_shutdown, shutdown_requested) = watch::channel(false);
    let (capture_commands, capture_command_receiver) = mpsc::channel(1);
    let coordinator =
        tokio::spawn(run_with_capture(network, events, shutdown_requested, recorder, Some(session), capture_command_receiver));
    let (completed, response) = oneshot::channel();
    capture_commands
        .send(CaptureCommand::Stop { completed, publication: CapturePublication::default() })
        .await
        .expect("queue capture stop");

    let capture_path = timeout(Duration::from_secs(2), response)
        .await
        .expect("full event queue must not block capture stop")
        .expect("capture stop reply")
        .expect("capture archive should be complete");
    assert!(capture_path.is_file());
    assert!(
        matches!(received_events.try_recv().map(EventEnvelope::into_event), Ok(Event::Failed(message)) if message == "occupy event queue")
    );
    assert!(received_events.try_recv().is_err());

    network_finished.send(()).expect("network task should still be waiting");
    timeout(Duration::from_secs(2), coordinator)
        .await
        .expect("capture coordinator should exit")
        .expect("capture coordinator should not panic")
        .expect("network task should finish");
}

#[tokio::test]
async fn capture_stops_on_close() {
    let directory = TempDirectory::new();
    let recorder = CaptureRecorder::new();
    let capture_root = directory.path().join("captures");
    let capture_session = CaptureSession::start(CaptureConfig::new(&capture_root), recorder.clone()).expect("start capture");
    let capture_path = capture_session.path().to_path_buf();
    let (events, received_events) = mpsc::channel(2);
    let mut received_events = EventReceiver::new(received_events);
    let (shutdown, shutdown_requested) = watch::channel(false);
    let network_shutdown = shutdown_requested.clone();
    let run = async move {
        let mut network_shutdown = network_shutdown;
        while !*network_shutdown.borrow() {
            if network_shutdown.changed().await.is_err() {
                break;
            }
        }
        Ok(())
    };
    let (capture_command_sender, capture_commands) = mpsc::channel(1);
    let worker = tokio::spawn(run_with_capture(run, events, shutdown_requested, recorder, Some(capture_session), capture_commands));

    shutdown.send_replace(true);
    timeout(Duration::from_secs(1), worker)
        .await
        .expect("shutdown should interrupt capture promptly")
        .expect("capture coordinator should not panic")
        .expect("network shutdown should succeed");

    let event = received_events.try_recv();
    assert!(
        matches!(event, Ok(Event::CaptureInterrupted { ref path, error: None }) if path == &capture_path),
        "unexpected capture shutdown event: {event:?}"
    );
    let manifest = archive_entry(&capture_path, "manifest.json");
    let manifest: CaptureManifest = serde_json::from_slice(&manifest).expect("decode capture manifest");
    assert_eq!(manifest.state, CaptureState::Interrupted);
    assert_eq!(fs::read_dir(&capture_root).expect("capture directory").count(), 1);

    drop(capture_command_sender);
}

#[test]
fn full_command_queue_errors() {
    let (commands, mut receiver) = mpsc::channel(COMMAND_CAPACITY);
    let challenge = MfaChallenge::new(commands);
    assert!(matches!(challenge.complete(" \t "), Err(Error::EmptyMfaCode)));
    for _ in 0..COMMAND_CAPACITY {
        MfaChallenge::new(challenge.commands.clone()).complete("123456").expect("queue has room");
    }

    assert!(matches!(challenge.complete("123456"), Err(Error::CommandQueueFull)));
    assert!(receiver.try_recv().is_ok());
    challenge.complete("123456").expect("challenge can be retried after queue pressure");
    assert!(matches!(challenge.complete("123456"), Err(Error::MfaChallengeCompleted)));
}

struct TempDirectory(PathBuf);

impl TempDirectory {
    fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
        let path =
            std::env::temp_dir().join(format!("radia-client-capture-{}-{}", std::process::id(), NEXT_ID.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir(&path).expect("create client test directory");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
