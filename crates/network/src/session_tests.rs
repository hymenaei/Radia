use std::{
    collections::HashMap,
    future::{Future, pending},
    mem::replace,
    net::SocketAddr,
    pin::pin,
    str::from_utf8,
    sync::{Arc, Mutex},
    task::{Context, Waker},
    thread,
    time::Duration,
};

use llsd::Value;
use protocol::lludp::{BlockValues, FieldValue, Message, Packet, SchemaMessage};
use protocol::{
    ChatType, login,
    messages::{
        CacheMissType, ChatFromViewer, ObjectRequest, RequestAgentData, RequestMultipleObjects,
        agent::agent_movement_complete::{AgentData, AgentMovementComplete, Data, SimData},
        chat::chat_from_viewer,
    },
};
use test_support::lludp;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream, UdpSocket},
    runtime::Builder,
    spawn,
    sync::Semaphore,
    task::{JoinHandle, spawn_blocking, yield_now},
};
use tokio::{
    sync::{mpsc, oneshot, watch},
    time::timeout,
};
use uuid::Uuid;

use super::{
    COMMAND_CAPACITY, Channels, Command, MAX_OUTBOUND_CHAT_MESSAGE_BYTES, MAX_OUTBOUND_OBJECT_REQUESTS, MfaChallenge,
    NETWORK_THREAD_STACK_SIZE, Session, SessionHandle, SessionOptions, run_client, send_failure_event, wait_for_login,
};
use crate::coverage::{CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageRecord, CoverageSource};
use crate::event::{
    CLIENT_EVENT_CAPACITY, CapabilityEvent, Event, EventEnvelope, EventQueueBatchOrder, EventSendOutcome, PRIMARY_TERMINAL_EVENT_CAPACITY,
    send_client_event,
};
use crate::status::{NetworkLifecycle, NetworkStatusHandle};
use crate::test_support::http::response;
use crate::{
    Delivery, Error, Grid, LoginCredentials, StartLocation,
    event::SimulatorId,
    transport::lludp::{MAX_PENDING_PACKETS, advance_sequence},
};

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
const CIRCUIT_CODE: u32 = 0x1122_3344;
const FOLDER_ID: &str = "12345678-1234-5678-1234-567812345678";
const CLIENT_TIMEOUT: Duration = Duration::from_secs(3);

async fn fake_http(simulator_port: u16) -> (String, oneshot::Receiver<Vec<u8>>, JoinHandle<Vec<Vec<u8>>>) {
    fake_http_with_mfa(simulator_port, false).await
}

async fn fake_http_with_event_queue_disconnect(
    simulator_port: u16,
) -> (String, oneshot::Receiver<Vec<u8>>, oneshot::Sender<()>, JoinHandle<Vec<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake HTTP server");
    let address = listener.local_addr().expect("read fake HTTP address");
    let login_body = login_xml(simulator_port, &format!("http://{address}/seed"));
    let seed_body = format!("<llsd><map><key>EventQueueGet</key><string>http://{address}/events</string></map></llsd>");
    let (poll_started, poll_observed) = oneshot::channel();
    let (disconnect_poll, poll_disconnect) = oneshot::channel();
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
        let _ = poll_disconnect.await;
        drop(stream);
        requests
    });

    (format!("http://{address}/login.cgi"), poll_observed, disconnect_poll, worker)
}

async fn fake_http_with_mfa(simulator_port: u16, require_mfa: bool) -> (String, oneshot::Receiver<Vec<u8>>, JoinHandle<Vec<Vec<u8>>>) {
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

async fn fake_mfa_challenge_server() -> (String, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake MFA server");
    let address = listener.local_addr().expect("read fake MFA address");
    let worker = spawn(async move {
        let (mut stream, _) = timeout(CLIENT_TIMEOUT, listener.accept()).await.expect("accept login request").expect("login request");
        let request = read_http_request(&mut stream).await;
        assert!(request_line(&request).starts_with("POST /login.cgi HTTP/1.1"));
        stream.write_all(&response(200, "OK", "text/xml", None, mfa_challenge_xml())).await.expect("write MFA challenge");

        match timeout(Duration::from_millis(150), listener.accept()).await {
            Err(_) => false,
            Ok(Ok((_stream, _))) => true,
            Ok(Err(error)) => panic!("accept unexpected login request: {error}"),
        }
    });
    (format!("http://{address}/login.cgi"), worker)
}

async fn fake_pending_login_server() -> (String, oneshot::Receiver<()>, JoinHandle<bool>) {
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

fn spawn_test_session(login_url: String) -> (Session, SessionHandle, JoinHandle<Result<(), String>>) {
    spawn_session_client(test_credentials(), login_url)
}

fn spawn_test_session_with_event_sender(
    login_url: String,
) -> (Session, SessionHandle, JoinHandle<Result<(), String>>, mpsc::Sender<EventEnvelope>) {
    spawn_session_client_with_event_sender(test_credentials(), login_url)
}

fn spawn_session_client(credentials: LoginCredentials, login_url: String) -> (Session, SessionHandle, JoinHandle<Result<(), String>>) {
    let (session, handle, worker, _) = spawn_session_client_with_event_sender(credentials, login_url);
    (session, handle, worker)
}

fn spawn_session_client_with_event_sender(
    credentials: LoginCredentials,
    login_url: String,
) -> (Session, SessionHandle, JoinHandle<Result<(), String>>, mpsc::Sender<EventEnvelope>) {
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let retained_events = events.clone();
    let coverage_losses = CoverageLossLedger::new();
    let coverage_changed = coverage_losses.subscribe();
    let (disconnect_requested, disconnect_signal) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands: commands.clone(),
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
        status: status.clone(),
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
                events,
                terminal_event_permit,
                disconnect_requested: disconnect_signal,
                coverage_losses,
            },
            status,
            login_url,
            SessionOptions::default(),
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
) -> (Session, SessionHandle, JoinHandle<Result<(), String>>) {
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (events, event_receiver) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = events.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let coverage_losses = CoverageLossLedger::new();
    let coverage_changed = coverage_losses.subscribe();
    let (disconnect_requested, disconnect_signal) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands: commands.clone(),
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
        status: status.clone(),
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
                events: events.clone(),
                terminal_event_permit,
                disconnect_requested: disconnect_signal.clone(),
                coverage_losses,
            };
            let result = runtime.block_on(run_client(credentials, channels, status, login_url, SessionOptions::default()));
            runtime.shutdown_background();
            worker_finished_sender.send_replace(true);
            result
        })
        .expect("start network thread");
    let worker = spawn_blocking(move || network_thread.join().expect("network thread should not panic"));
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

#[tokio::test(flavor = "multi_thread")]
async fn event_queue_coverage_loss_keeps_viewer_session_connected() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, disconnect_poll, http_task) = fake_http_with_event_queue_disconnect(simulator_address.port()).await;
    let (mut session, retained_handle, worker) = spawn_network_thread_session(test_credentials(), login_url);

    assert!(matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. }));
    let (use_circuit, peer) = receive_udp(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");

    let (movement, movement_packet) =
        receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    assert_eq!(movement, expected_complete_movement(movement_packet.sequence()));
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");
    let event_queue_request = timeout(CLIENT_TIMEOUT, poll_started).await.expect("EventQueueGet should start").expect("event request");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));

    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (_, reply_packet) = receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    simulator.send_to(&server_ack(23, reply_packet.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");
    loop {
        match next_event(&mut session.events).await {
            Event::SessionReady { .. } => break,
            Event::Failed(error) => panic!("session failed before readiness: {error}"),
            _ => {}
        }
    }

    disconnect_poll.send(()).expect("release the failed EventQueueGet request");
    let mut saw_coverage_loss = false;
    let mut saw_queue_failure = false;
    while !saw_coverage_loss || !saw_queue_failure {
        match timeout(CLIENT_TIMEOUT, session.recv_event()).await.expect("session should keep publishing after EventQueueGet failure") {
            Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) => {
                assert!(matches!(record.watermark.source, CoverageSource::EventQueue { .. }));
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
        let mut logout_reply = lludp::fixture("logout_reply.hex");
        logout_reply[0] |= 0x40;
        set_sequence(&mut logout_reply, 26);
        simulator.send_to(&logout_reply, peer).await.expect("reply to unexpected session logout");
        let result =
            timeout(Duration::from_secs(2), worker).await.expect("failed session worker should stop").expect("network thread should join");
        assert!(result.is_err(), "the pre-fix runtime ends with an EventQueueGet failure");
        panic!("EventQueueGet failure sent LogoutRequest instead of preserving the viewer session");
    }
    send_result.expect("session should still accept simulator commands");
    assert_eq!(retained_handle.network_status().lifecycle, NetworkLifecycle::Connected);
    assert!(!*retained_handle.worker_finished.borrow(), "EventQueueGet failure must not finish the network worker");
    assert!(matches!(chat_packet.message(), Message::ChatFromViewer(message) if message.chat_data.message == "still connected"));
    simulator.send_to(&server_ack(25, chat_packet.sequence()), peer).await.expect("ack post-failure chat");

    drop(session);
    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    let mut logout_reply = lludp::fixture("logout_reply.hex");
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

fn request_line(request: &[u8]) -> &str {
    from_utf8(request).expect("HTTP request is UTF-8").split("\r\n").next().expect("HTTP request line")
}

fn request_body(request: &[u8]) -> &str {
    let body_start = request.windows(4).position(|window| window == b"\r\n\r\n").expect("HTTP request has a body") + 4;
    from_utf8(&request[body_start..]).expect("HTTP request body is UTF-8")
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
        let packet = Packet::decode(&bytes).expect("decode client LLUDP packet");
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
        if matches!(packet.message(), Message::AgentThrottle(_)) {
            assert!(packet.is_reliable());
            socket.send_to(&server_ack(u32::MAX, packet.sequence()), expected_peer).await.expect("ack AgentThrottle");
            continue;
        }
        panic!("unexpected client LLUDP message: {:?}", packet.message());
    }
}

async fn next_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
    timeout(CLIENT_TIMEOUT, events.recv())
        .await
        .expect("client should publish an event")
        .expect("client event channel should remain open")
        .into_event()
}

fn expected_fixture(name: &str, sequence: u32) -> Vec<u8> {
    let mut bytes = lludp::fixture(name);
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
    bytes.extend_from_slice(&packet_id.to_be_bytes());
    bytes.push(1);
    bytes
}

fn movement_complete_packet(sequence: u32, agent_id: Uuid, session_id: Uuid, region_handle: u64) -> Vec<u8> {
    Packet::new(
        sequence,
        Message::AgentMovementComplete(AgentMovementComplete {
            agent_data: AgentData { agent_id, session_id },
            data: Data { region_handle, ..Default::default() },
            sim_data: SimData::default(),
        }),
    )
    .with_reliable()
    .encode()
    .expect("encode AgentMovementComplete")
}

fn teleport_finish_packet(agent_id: Uuid, endpoint: SocketAddr, region_handle: u64, seed: &str) -> Vec<u8> {
    let SocketAddr::V4(endpoint) = endpoint else { panic!("simulator endpoint must use IPv4") };
    let schema = SchemaMessage::schema_by_name("TeleportFinish").expect("TeleportFinish schema");
    let message = SchemaMessage::new(
        schema,
        vec![BlockValues::single(vec![
            FieldValue::Uuid(agent_id),
            FieldValue::U32(0),
            FieldValue::IpAddr(endpoint.ip().octets()),
            FieldValue::IpPort(endpoint.port().to_be_bytes()),
            FieldValue::U64(region_handle),
            FieldValue::Bytes(seed.as_bytes().to_vec()),
            FieldValue::U8(0),
            FieldValue::U32(0),
        ])],
    )
    .expect("valid TeleportFinish");
    Packet::new(25, Message::Schema(message)).with_reliable().encode().expect("encode TeleportFinish")
}

fn server_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&[0, 0xff, 0xff, 0xff, 0xfb, 1]);
    bytes.extend_from_slice(&acknowledged.to_le_bytes());
    bytes
}

fn expected_packet_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    let mut bytes = lludp::fixture("packet_ack.hex");
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
    let login::Value::Array(folders) = login_data.get("inventory-skeleton").expect("login metadata survives handoff") else {
        panic!("expected inventory skeleton array");
    };
    let [login::Value::Struct(folder)] = folders.as_slice() else {
        panic!("expected one inventory skeleton folder");
    };
    assert_eq!(folder.get("folder_id"), Some(&login::Value::String(FOLDER_ID.into())));
    assert_eq!(folder.get("name"), Some(&login::Value::String("Favorites".into())));

    drop(network_session);
    let mut next_sequence = advance_sequence(reply_packet.sequence());
    let mut movement_acknowledged = false;
    loop {
        let (bytes, shutdown_peer) = receive_udp(&simulator).await;
        assert_eq!(shutdown_peer, peer);
        let packet = Packet::decode(&bytes).expect("decode simulator shutdown packet");
        assert_eq!(packet.sequence(), next_sequence);
        next_sequence = advance_sequence(next_sequence);

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
            Message::AgentThrottle(_) => {
                assert!(packet.is_reliable());
                simulator.send_to(&server_ack(25, packet.sequence()), peer).await.expect("ack AgentThrottle during shutdown");
            }
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

struct PromotedSession {
    session: Session,
    handle: SessionHandle,
    worker: JoinHandle<Result<(), String>>,
    primary: UdpSocket,
    destination: UdpSocket,
    destination_peer: SocketAddr,
    destination_id: SimulatorId,
    http_task: JoinHandle<Vec<Vec<u8>>>,
}

async fn promoted_session() -> PromotedSession {
    let primary = UdpSocket::bind("127.0.0.1:0").await.expect("bind primary simulator");
    let primary_address = primary.local_addr().expect("read primary address");
    let destination = UdpSocket::bind("127.0.0.1:0").await.expect("bind destination simulator");
    let destination_address = destination.local_addr().expect("read destination address");
    let agent_id = Uuid::parse_str(AGENT_ID).expect("agent UUID");
    let session_id = Uuid::parse_str(SESSION_ID).expect("session UUID");
    let (login_url, poll_started, http_task) = fake_http(primary_address.port()).await;
    let (mut session, handle, worker) = spawn_test_session(login_url);

    assert!(matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. }));
    let (use_circuit, primary_peer) = receive_udp(&primary).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    primary.send_to(&server_ack(20, 1), primary_peer).await.expect("ack primary UseCircuitCode");
    let (_, movement_request) =
        receive_client_message(&primary, primary_peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    primary.send_to(&server_ack(21, movement_request.sequence()), primary_peer).await.expect("ack primary CompleteAgentMovement");
    primary.send_to(&reliable_fixture("region_handshake.hex", 22), primary_peer).await.expect("send primary RegionHandshake");
    let (_, region_reply) =
        receive_client_message(&primary, primary_peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    primary.send_to(&server_ack(23, region_reply.sequence()), primary_peer).await.expect("ack primary RegionHandshakeReply");
    primary
        .send_to(&movement_complete_packet(24, agent_id, session_id, 0), primary_peer)
        .await
        .expect("send primary AgentMovementComplete");
    let (_, primary_throttle) =
        receive_client_message(&primary, primary_peer, |message| matches!(message, Message::AgentThrottle(_))).await;
    assert!(primary_throttle.is_reliable());
    primary.send_to(&server_ack(25, primary_throttle.sequence()), primary_peer).await.expect("ack primary AgentThrottle");

    let root_id = loop {
        match next_event(&mut session.events).await {
            Event::SessionReady { simulator_id, .. } => break simulator_id,
            Event::Failed(error) => panic!("session failed before readiness: {error}"),
            _ => {}
        }
    };
    let event_queue_request = timeout(CLIENT_TIMEOUT, poll_started)
        .await
        .expect("primary EventQueueGet should start")
        .expect("fake HTTP server should observe EventQueueGet");
    assert!(request_line(&event_queue_request).starts_with("POST /events HTTP/1.1"));

    primary
        .send_to(&teleport_finish_packet(agent_id, destination_address, 0x1000_0000_0000_1000, "http://127.0.0.1:1/seed"), primary_peer)
        .await
        .expect("send TeleportFinish");
    let (use_circuit, destination_peer) = receive_udp(&destination).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1));
    destination.send_to(&server_ack(30, 1), destination_peer).await.expect("ack destination UseCircuitCode");
    destination.send_to(&reliable_fixture("region_handshake.hex", 31), destination_peer).await.expect("send destination RegionHandshake");
    let (_, destination_reply) =
        receive_client_message(&destination, destination_peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    let (_, destination_movement) =
        receive_client_message(&destination, destination_peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    destination
        .send_to(&server_ack(32, destination_reply.sequence()), destination_peer)
        .await
        .expect("ack destination RegionHandshakeReply");
    destination
        .send_to(&server_ack(33, destination_movement.sequence()), destination_peer)
        .await
        .expect("ack destination CompleteAgentMovement");
    destination
        .send_to(&movement_complete_packet(34, agent_id, session_id, 0x1000_0000_0000_1000), destination_peer)
        .await
        .expect("send destination AgentMovementComplete");

    let destination_id = timeout(CLIENT_TIMEOUT, async {
        loop {
            match next_event(&mut session.events).await {
                Event::SimulatorPromoted { previous_simulator_id, simulator_id, .. } => {
                    assert_eq!(previous_simulator_id, root_id);
                    break simulator_id;
                }
                Event::Failed(error) | Event::SimulatorFailed { reason: error, .. } => {
                    panic!("session failed before destination promotion: {error}");
                }
                _ => {}
            }
        }
    })
    .await
    .expect("destination movement should promote the simulator");
    assert_eq!(session.current_simulator(), Some(destination_address));
    let (_, destination_throttle) =
        receive_client_message(&destination, destination_peer, |message| matches!(message, Message::AgentThrottle(_))).await;
    assert!(destination_throttle.is_reliable());
    destination.send_to(&server_ack(35, destination_throttle.sequence()), destination_peer).await.expect("ack destination AgentThrottle");

    PromotedSession { session, handle, worker, primary, destination, destination_peer, destination_id, http_task }
}

async fn receive_logout(destination: &UdpSocket, destination_peer: SocketAddr) {
    let (logout_bytes, logout) =
        receive_client_message(destination, destination_peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(matches!(logout.message(), Message::LogoutRequest(_)));
    let mut expected_logout = expected_fixture("logout_request.hex", logout.sequence());
    if logout.appended_acks() == [34] {
        expected_logout = with_appended_ack(expected_logout, 34);
    }
    assert_eq!(logout_bytes, expected_logout);
}

async fn assert_destination_disconnected(events: &mut mpsc::Receiver<EventEnvelope>, destination_id: SimulatorId) {
    let mut saw_destination_disconnect = false;
    while let Some(envelope) = timeout(CLIENT_TIMEOUT, events.recv()).await.expect("session event channel should close after disconnect") {
        match envelope.into_event() {
            Event::SimulatorDisconnected { simulator_id, .. } if simulator_id == destination_id => {
                saw_destination_disconnect = true;
            }
            Event::Failed(error) | Event::SimulatorFailed { reason: error, .. } => {
                panic!("graceful disconnect emitted a failure: {error}");
            }
            _ => {}
        }
    }
    assert!(saw_destination_disconnect, "promoted simulator should end with a disconnect event");
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_after_promotion() {
    let PromotedSession { mut session, handle, worker, primary: _primary, destination, destination_peer, destination_id, http_task } =
        promoted_session().await;
    let mut events = replace(&mut session.events, mpsc::channel(1).1);
    handle.request_disconnect();
    let disconnect = spawn(async move {
        session.disconnect().await;
        session.disconnect().await;
    });

    receive_logout(&destination, destination_peer).await;
    timeout(CLIENT_TIMEOUT, disconnect).await.expect("disconnect should wait for teardown").expect("disconnect task");
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should finish").expect("client task").is_ok());
    assert_destination_disconnected(&mut events, destination_id).await;
    assert_eq!(handle.network_status().lifecycle, NetworkLifecycle::Disconnected);

    let requests = timeout(CLIENT_TIMEOUT, http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_after_promotion_with_full_reliable_window() {
    let PromotedSession { mut session, handle, worker, primary: _primary, destination, destination_peer, destination_id, http_task } =
        promoted_session().await;

    for local_id in 0..MAX_PENDING_PACKETS {
        let request = RequestMultipleObjects {
            agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
            objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: local_id as u32 }],
        };
        timeout(CLIENT_TIMEOUT, session.send_to(destination_id, request, Delivery::Reliable))
            .await
            .expect("reliable send should finish while filling the window")
            .expect("send_to should not fail before the reliable window is full");
    }
    assert!(handle.network_status().reliable_packets_pending >= MAX_PENDING_PACKETS);

    for local_id in 0..MAX_PENDING_PACKETS {
        let (_, packet) =
            receive_client_message(&destination, destination_peer, |message| matches!(message, Message::RequestMultipleObjects(_))).await;
        assert!(packet.is_reliable());
        let Message::RequestMultipleObjects(request) = packet.message() else {
            unreachable!("message filter only accepts object requests")
        };
        assert_eq!(request.objects[0].local_id, local_id as u32);
    }

    let mut events = replace(&mut session.events, mpsc::channel(1).1);
    let disconnect = spawn(async move { session.disconnect().await });

    receive_logout(&destination, destination_peer).await;
    timeout(CLIENT_TIMEOUT, disconnect).await.expect("disconnect should finish with a full reliable window").expect("disconnect task");
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should finish").expect("client task").is_ok());
    assert_destination_disconnected(&mut events, destination_id).await;
    assert_eq!(handle.network_status().lifecycle, NetworkLifecycle::Disconnected);

    let requests = timeout(CLIENT_TIMEOUT, http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_after_promotion() {
    let PromotedSession { mut session, worker, primary: _primary, destination, destination_peer, destination_id, http_task, .. } =
        promoted_session().await;
    let handle = session.handle();
    let mut events = replace(&mut session.events, mpsc::channel(1).1);
    drop(session);

    receive_logout(&destination, destination_peer).await;
    handle.disconnect().await;
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should finish").expect("client task").is_ok());
    assert_destination_disconnected(&mut events, destination_id).await;
    assert_eq!(handle.network_status().lifecycle, NetworkLifecycle::Disconnected);

    let requests = timeout(CLIENT_TIMEOUT, http_task).await.expect("fake HTTP server should stop").expect("HTTP task");
    assert_eq!(requests.len(), 3);
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
    let (events_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let terminal_event_permit = events_sender.clone().try_reserve_owned().expect("test event channel reserves terminal event slot");
    let (disconnect, disconnect_requested) = watch::channel(false);
    let client = spawn(run_client(
        credentials,
        Channels {
            commands: command_receiver,
            command_sender: commands.clone(),
            events: events_sender,
            terminal_event_permit,
            disconnect_requested,
            coverage_losses: CoverageLossLedger::new(),
        },
        NetworkStatusHandle::default(),
        login_url,
        SessionOptions::default(),
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

    let (_, throttle) = receive_client_message(&simulator, peer, |message| matches!(message, Message::AgentThrottle(_))).await;
    assert!(throttle.is_reliable());
    assert_eq!(throttle.appended_acks(), &[24]);
    simulator.send_to(&server_ack(25, throttle.sequence()), peer).await.expect("ack AgentThrottle");

    disconnect.send_replace(true);
    let (logout_bytes, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(logout.appended_acks().is_empty());
    assert_eq!(logout_bytes, expected_fixture("logout_request.hex", logout.sequence()));
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
    let session = Session::new_for_test(targeted_send_handle(commands, disconnect), event_receiver);
    let mut publish = pin!(send_client_event(&events, Event::Failed("pending event".into()), &mut disconnect_requested));
    assert!(publish.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(), "full event queue blocks publication");
    let retained_handle = session.handle();
    drop(session);

    assert!(matches!(
        timeout(Duration::from_secs(1), publish).await.expect("session drop should release event publication"),
        EventSendOutcome::Shutdown
    ));
    assert!(*retained_handle.disconnect_requested.borrow());
}

#[tokio::test]
async fn receive_releases_capability_bytes() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let budget = Arc::new(Semaphore::new(32));
    let permit = budget.clone().acquire_many_owned(17).await.expect("reserve capability bytes");
    let (events, event_receiver) = mpsc::channel(1);
    events
        .send(EventEnvelope::with_retained_bytes(
            Event::CapabilityEvent {
                simulator_id: SimulatorId::new(77),
                source: "127.0.0.1:13000".parse().expect("simulator endpoint"),
                message: "TestEvent".into(),
                body: Value::Undefined,
            },
            permit,
        ))
        .await
        .expect("queue capability event");
    let mut session = Session::new_for_test(targeted_send_handle(commands, disconnect), event_receiver);

    assert_eq!(budget.available_permits(), 15);
    assert!(matches!(session.recv_event().await, Some(Event::CapabilityEvent { message, .. }) if message == "TestEvent"));
    assert_eq!(budget.available_permits(), 32);
}

#[tokio::test]
async fn recv_batch_keeps_order() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let (events, event_receiver) = mpsc::channel(1);
    let order = EventQueueBatchOrder { incarnation: 71, ordinal: 9 };
    events
        .send(EventEnvelope::new(Event::CapabilityEventBatch {
            simulator_id: SimulatorId::new(77),
            source: "127.0.0.1:13000".parse().expect("simulator endpoint"),
            order,
            events: vec![CapabilityEvent { message: "TestEvent".into(), body: Value::Undefined, retained_bytes: 0 }],
        }))
        .await
        .expect("queue EventQueue batch");
    let mut session = Session::new_for_test(targeted_send_handle(commands, disconnect), event_receiver);

    let Some(Event::CapabilityEventBatch { order: received_order, events, .. }) = session.recv_event().await else {
        panic!("Session returns the queued EventQueue batch")
    };
    assert_eq!(received_order, order);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].message, "TestEvent");
}

#[tokio::test]
async fn final_loss_wakes_empty_intake() {
    use std::{
        sync::atomic::{AtomicBool, Ordering},
        task::Wake,
    };

    struct WakeFlag(AtomicBool);
    impl Wake for WakeFlag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = SimulatorId::new(77);
    let source = CoverageSource::Lludp;
    let (events, event_receiver) = mpsc::channel(1);
    let mut session = Session::new_for_test_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger.clone());
    let mut receive = pin!(session.recv_event());
    let wake = Arc::new(WakeFlag(AtomicBool::new(false)));
    let waker = Waker::from(Arc::clone(&wake));
    assert!(receive.as_mut().poll(&mut Context::from_waker(&waker)).is_pending());
    let CoverageRecord::Recorded(watermark) = ledger.record(simulator_id, source, CoverageLossReason::EventAdmission) else {
        panic!("new source has ledger capacity")
    };

    assert!(wake.0.load(Ordering::SeqCst), "recording loss wakes an already waiting intake");
    assert!(matches!(
        timeout(Duration::from_secs(1), receive).await.expect("source loss wakes empty intake"),
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) if record.watermark == watermark
    ));
    drop(events);
}

#[tokio::test]
async fn coverage_loss_precedes_event_and_awaits_ack() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = SimulatorId::new(78);
    let source = CoverageSource::Lludp;
    let CoverageRecord::Recorded(watermark) = ledger.record(simulator_id, source, CoverageLossReason::DecodeAdmission) else {
        panic!("new source has ledger capacity")
    };
    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(EventEnvelope::with_coverage_watermark(Event::Failed("covered event".into()), watermark))
        .expect("fill bounded event queue with covered event");
    let mut session = Session::new_for_test_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger.clone());
    session.acknowledge_coverage_loss(watermark);
    assert!(matches!(
        session.recv_event().await,
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) if record.watermark == watermark
    ));
    {
        let mut receive = pin!(session.recv_event());
        assert!(
            receive.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(),
            "covered event waits for loss acknowledgement"
        );
    }
    session.acknowledge_coverage_loss(watermark);
    assert!(matches!(
        session.recv_event().await,
        Some(Event::Failed(message)) if message == "covered event"
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn preflight_loss_gates_later_event() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let simulator_address = simulator.local_addr().expect("read simulator address");
    let (login_url, poll_started, http_task) = fake_http(simulator_address.port()).await;
    let (mut session, _handle, worker) = spawn_test_session(login_url);

    assert!(matches!(next_event(&mut session.events).await, Event::LoginAccepted { .. }));
    let (_, peer) = receive_udp(&simulator).await;
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (_, movement) = receive_client_message(&simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    simulator.send_to(&server_ack(21, movement.sequence()), peer).await.expect("ack CompleteAgentMovement");
    simulator.send_to(&reliable_fixture("region_handshake.hex", 22), peer).await.expect("send RegionHandshake");
    let (_, reply) = receive_client_message(&simulator, peer, |message| matches!(message, Message::RegionHandshakeReply(_))).await;
    simulator.send_to(&server_ack(23, reply.sequence()), peer).await.expect("ack RegionHandshakeReply");
    simulator.send_to(&reliable_fixture("agent_movement_complete.hex", 24), peer).await.expect("send AgentMovementComplete");

    let mut session_ready = false;
    let mut movement_complete_received = false;
    while !session_ready || !movement_complete_received {
        match next_event(&mut session.events).await {
            Event::SessionReady { .. } => session_ready = true,
            Event::SimulatorMessage { sequence: 24, message: Message::AgentMovementComplete(_), .. } => {
                movement_complete_received = true;
            }
            Event::Failed(error) => panic!("client failed before initial movement completed: {error}"),
            _ => {}
        }
    }
    timeout(CLIENT_TIMEOUT, poll_started).await.expect("EventQueueGet should start").expect("EventQueueGet request");

    let mut rejected = lludp::fixture("improved_terse_object_update.hex");
    rejected[0] |= 0x40;
    set_sequence(&mut rejected, 25);
    rejected[18] = u8::MAX;
    assert!(Packet::preflight(&rejected).is_err(), "an object field exceeding its packet frame must fail LLUDP preflight");
    simulator.send_to(&rejected, peer).await.expect("send preflight-rejected reliable update");

    let watermark = match timeout(CLIENT_TIMEOUT, session.recv_event()).await.expect("preflight loss should wake Session") {
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) }) => {
            assert_eq!(record.reason, CoverageLossReason::PreflightRejected);
            record.watermark
        }
        other => panic!("expected LLUDP coverage loss, got {other:?}"),
    };

    simulator.send_to(&reliable_fixture("chat_from_simulator.hex", 26), peer).await.expect("send later reliable chat");
    timeout(CLIENT_TIMEOUT, async {
        loop {
            let (bytes, _) = receive_udp(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode chat acknowledgement");
            if packet.appended_acks().contains(&26)
                || matches!(packet.message(), Message::PacketAck(message) if message.packets.iter().any(|item| item.id == 26))
            {
                break;
            }
        }
    })
    .await
    .expect("later reliable event should be admitted and acknowledged");

    timeout(CLIENT_TIMEOUT, async {
        while session.events.is_empty() {
            yield_now().await;
        }
    })
    .await
    .expect("later covered event should reach Session intake");
    {
        let mut receive = pin!(session.recv_event());
        assert!(
            receive.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(),
            "Session holds an event covered by an unacknowledged LLUDP loss"
        );
    }

    session.acknowledge_coverage_loss(watermark);
    assert!(matches!(
        timeout(CLIENT_TIMEOUT, session.recv_event()).await.expect("acknowledgement should release the event"),
        Some(Event::SimulatorMessage { sequence: 26, message: Message::ChatFromSimulator(_), .. })
    ));

    drop(session);
    let (_, logout) = receive_client_message(&simulator, peer, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(logout.is_reliable());
    assert!(timeout(CLIENT_TIMEOUT, worker).await.expect("network worker should stop").expect("client task").is_ok());
    assert_eq!(http_task.await.expect("fake HTTP server should stop").len(), 3);
}

#[tokio::test]
async fn coalesced_source_loss_is_capped_to_the_event_watermark() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = SimulatorId::new(80);
    let source = CoverageSource::Lludp;
    assert!(matches!(ledger.record(simulator_id, source, CoverageLossReason::EventAdmission), CoverageRecord::Recorded(_)));
    let event_watermark = ledger.watermark(simulator_id, source);

    for id in 81..=335 {
        assert!(matches!(ledger.record(SimulatorId::new(id), source, CoverageLossReason::EventAdmission), CoverageRecord::Recorded(_)));
    }
    assert!(matches!(
        ledger.record(SimulatorId::new(336), source, CoverageLossReason::EventAdmission),
        CoverageRecord::Overflowed { generation: 1 }
    ));
    assert!(matches!(ledger.record(simulator_id, source, CoverageLossReason::DecodeAdmission), CoverageRecord::Recorded(_)));

    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(EventEnvelope::with_coverage_watermark(Event::Failed("covered before later loss".into()), event_watermark))
        .expect("queue event with the earlier source watermark");
    let mut session = Session::new_for_test_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger);

    assert!(matches!(
        session.recv_event().await,
        Some(Event::CoverageLost { loss: CoverageLoss::Source(record) })
            if record.watermark.generation == event_watermark.generation
                && record.watermark.overflow_generation == event_watermark.overflow_generation
    ));
    session.acknowledge_coverage_loss(event_watermark);
    assert!(matches!(
        session.recv_event().await,
        Some(Event::Failed(message)) if message == "covered before later loss"
    ));
    assert!(matches!(session.recv_event().await, Some(Event::CoverageLost { loss: CoverageLoss::SessionOverflow { generation: 1 } })));
}

#[tokio::test]
async fn session_overflow_requires_ack_before_covered_event() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    for id in 1..=256 {
        assert!(matches!(
            ledger.record(SimulatorId::new(id), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
            CoverageRecord::Recorded(_)
        ));
    }
    assert!(matches!(
        ledger.record(SimulatorId::new(257), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
        CoverageRecord::Overflowed { generation: 1 }
    ));
    let event_watermark = ledger.watermark(SimulatorId::new(258), CoverageSource::EventQueue { incarnation: 1 });
    assert_eq!(event_watermark.overflow_generation, 1);

    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(EventEnvelope::with_coverage_watermark(Event::Failed("global-overflow-covered event".into()), event_watermark))
        .expect("queue event covered by session overflow");
    let mut session = Session::new_for_test_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger);
    session.acknowledge_coverage_overflow(1);
    assert_eq!(session.coverage_losses().overflow_generation, Some(1), "an unsurfaced overflow cannot be acknowledged");

    let (loss_sender, loss_receiver) = oneshot::channel();
    let (ack_sender, ack_receiver) = oneshot::channel();
    let (event_sender, mut event_receiver) = oneshot::channel();
    let receive = spawn(async move {
        let loss = session.recv_event().await;
        let _ = loss_sender.send(loss);
        let _ = ack_receiver.await;
        session.acknowledge_coverage_overflow(1);
        let event = session.recv_event().await;
        let _ = event_sender.send(event);
    });

    assert!(matches!(
        timeout(Duration::from_secs(1), loss_receiver).await.expect("overflow wakes independently").expect("loss receiver"),
        Some(Event::CoverageLost { loss: CoverageLoss::SessionOverflow { generation: 1 } })
    ));
    assert!(
        timeout(Duration::from_millis(10), &mut event_receiver).await.is_err(),
        "overflow-covered event waits for explicit acknowledgement"
    );
    ack_sender.send(()).expect("ask Session to acknowledge surfaced overflow");
    assert!(matches!(
        timeout(Duration::from_secs(1), &mut event_receiver).await.expect("acknowledgement releases overflow-covered event").expect("event receiver"),
        Some(Event::Failed(message)) if message == "global-overflow-covered event"
    ));
    receive.await.expect("receive task completes after overflow acknowledgement");
}

#[tokio::test]
async fn earlier_watermark_does_not_wait_for_later_source_loss() {
    let (commands, _) = mpsc::channel(COMMAND_CAPACITY);
    let (disconnect, _) = watch::channel(false);
    let ledger = CoverageLossLedger::new();
    let simulator_id = SimulatorId::new(79);
    let source = CoverageSource::Lludp;
    let earlier = ledger.watermark(simulator_id, source);
    assert!(matches!(ledger.record(simulator_id, source, CoverageLossReason::EventAdmission), CoverageRecord::Recorded(_)));
    let (events, event_receiver) = mpsc::channel(1);
    events
        .try_send(EventEnvelope::with_coverage_watermark(Event::Failed("earlier event".into()), earlier))
        .expect("queue event predating source loss");
    let mut session = Session::new_for_test_with_coverage(targeted_send_handle(commands, disconnect), event_receiver, ledger);

    assert!(matches!(session.recv_event().await, Some(Event::Failed(message)) if message == "earlier event"));
    assert!(matches!(session.recv_event().await, Some(Event::CoverageLost { loss: CoverageLoss::Source(_) })));
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
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands,
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
        status: status.clone(),
    };
    let first_handle = handle.clone();
    let second_handle = handle.clone();
    let mut first_disconnect = spawn(async move { first_handle.disconnect().await });

    assert!(disconnect_state.changed().await.is_ok());
    assert_eq!(status.snapshot().lifecycle, NetworkLifecycle::Disconnecting);
    let mut second_disconnect = spawn(async move { second_handle.disconnect().await });
    assert!(timeout(Duration::from_millis(10), &mut first_disconnect).await.is_err());
    assert!(timeout(Duration::from_millis(10), &mut second_disconnect).await.is_err());

    worker_finished_sender.send_replace(true);
    timeout(Duration::from_secs(1), first_disconnect)
        .await
        .expect("worker completion should release the first disconnect")
        .expect("first disconnect task");
    timeout(Duration::from_secs(1), second_disconnect)
        .await
        .expect("worker completion should release the repeated disconnect")
        .expect("second disconnect task");
}

fn object_request() -> RequestMultipleObjects {
    RequestMultipleObjects {
        agent_data: RequestAgentData {
            agent_id: Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    }
}

#[tokio::test]
async fn dropping_send_future_keeps_the_admitted_command() {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Waker},
    };

    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let simulator_id = SimulatorId::new(42);
    {
        let mut send = pin!(handle.send_to(simulator_id, object_request(), Delivery::Reliable));
        assert!(send.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    }

    let Command::SendTo { completed, .. } = command_receiver.try_recv().expect("admitted command survives cancellation") else {
        panic!("expected targeted send");
    };
    assert!(completed.is_closed(), "only the result receiver is cancelled");
}

#[tokio::test]
async fn targeted_send_keeps_id() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let simulator_id = SimulatorId::new(42);
    let request = object_request();
    let send = spawn(async move { handle.send_to(simulator_id, request, Delivery::Reliable).await });

    let Some(Command::SendTo { simulator_id: routed_id, message, delivery, completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };
    assert_eq!(routed_id, simulator_id);
    assert_eq!(delivery, Delivery::Reliable);
    assert!(matches!(message, Message::RequestMultipleObjects(_)));
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
            agent_id: Uuid::parse_str("00112233-4455-6677-8899-aabbccddeeff").expect("agent UUID"),
            session_id: Uuid::parse_str("ffeeddcc-bbaa-9988-7766-554433221100").expect("session UUID"),
        },
        objects: Vec::new(),
    };

    assert!(matches!(handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await, Err(Error::SimulatorCommandQueueFull)));
}

#[tokio::test]
async fn sends_are_rejected_after_disconnect_begins() {
    let (commands, mut command_receiver) = mpsc::channel(2);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    handle.request_disconnect();

    assert!(matches!(handle.send(chat_message_with_text("after disconnect".into()), Delivery::Unreliable), Err(Error::SessionClosed)));
    let request =
        RequestMultipleObjects { agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() }, objects: Vec::new() };
    assert!(matches!(handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await, Err(Error::SessionClosed)));
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn targeted_send_target_closed() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let request = object_request();
    let send = spawn(async move { handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    drop(completed);

    assert!(matches!(send.await.expect("send task"), Err(Error::SimulatorConnectionClosed)));
}

#[tokio::test]
async fn targeted_send_resolves_when_session_closes() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_signal, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_signal.clone());
    let request = object_request();
    let send_handle = handle.clone();
    let send = spawn(async move { send_handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    handle.request_disconnect();
    assert!(matches!(
        timeout(Duration::from_secs(1), send).await.expect("disconnect should release the targeted send waiter").expect("send task"),
        Err(Error::SessionClosed)
    ));
    assert!(completed.send(Ok(())).is_err(), "the caller should stop waiting when disconnect begins");
}

#[tokio::test]
async fn targeted_send_reports_finished_worker() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let (worker_finished_sender, worker_finished) = watch::channel(false);
    let handle = SessionHandle {
        commands,
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
        status: NetworkStatusHandle::new(),
    };
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }],
    };
    let send = spawn(async move { handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { completed, .. }) = command_receiver.recv().await else {
        panic!("expected a targeted simulator command");
    };

    worker_finished_sender.send_replace(true);
    drop(completed);

    assert!(matches!(send.await.expect("send task"), Err(Error::SessionClosed)));
}

fn targeted_send_handle(commands: mpsc::Sender<Command>, disconnect_requested: watch::Sender<bool>) -> SessionHandle {
    let (_, worker_finished) = watch::channel(false);
    SessionHandle {
        commands,
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
        status: NetworkStatusHandle::new(),
    }
}

fn chat_message_with_size(size: usize) -> ChatFromViewer {
    chat_message_with_text("x".repeat(size))
}

fn chat_message_with_text(message: String) -> ChatFromViewer {
    ChatFromViewer {
        agent_data: chat_from_viewer::AgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        chat_data: chat_from_viewer::ChatData { message, chat_type: ChatType::Normal, channel: 0 },
    }
}

#[test]
fn outbound_chat_limit_precedes_queue_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);

    assert!(matches!(
        handle.send(chat_message_with_size(MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1), Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    ));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    let mut oversized_capacity = String::with_capacity(MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1);
    oversized_capacity.push('x');
    assert!(matches!(handle.send(chat_message_with_text(oversized_capacity), Delivery::Unreliable), Err(Error::OutboundMessageTooLarge)));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    handle.send(chat_message_with_size(MAX_OUTBOUND_CHAT_MESSAGE_BYTES), Delivery::Unreliable).expect("exact limit fits");
    assert_eq!(commands.capacity(), 0);
    let Some(Command::SendMessage { message: Message::ChatFromViewer(message), .. }) = command_receiver.try_recv().ok() else {
        panic!("expected the exact-limit chat message");
    };
    assert_eq!(message.chat_data.message.len(), MAX_OUTBOUND_CHAT_MESSAGE_BYTES);
}

#[tokio::test]
async fn object_request_limit_precedes_targeted_queue_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }; MAX_OUTBOUND_OBJECT_REQUESTS + 1],
    };

    assert!(matches!(handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge)));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    let mut oversized_capacity = Vec::with_capacity(MAX_OUTBOUND_OBJECT_REQUESTS + 1);
    oversized_capacity.push(ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 });
    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: oversized_capacity,
    };
    assert!(matches!(handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge)));
    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[tokio::test]
async fn object_request_at_limit_is_accepted() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands, disconnect_requested);
    let mut objects = Vec::with_capacity(MAX_OUTBOUND_OBJECT_REQUESTS);
    objects.resize(MAX_OUTBOUND_OBJECT_REQUESTS, ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 });
    let request = RequestMultipleObjects { agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() }, objects };

    let send = spawn(async move { handle.send_to(SimulatorId::new(42), request, Delivery::Reliable).await });
    let Some(Command::SendTo { message: Message::RequestMultipleObjects(message), completed, .. }) = command_receiver.recv().await else {
        panic!("expected an accepted object request");
    };
    assert_eq!(message.objects.len(), MAX_OUTBOUND_OBJECT_REQUESTS);
    completed.send(Ok(())).expect("send result receiver");
    assert!(send.await.expect("send task").is_ok());
}

#[tokio::test]
async fn oversized_sends_are_rejected_before_admission() {
    let (commands, mut command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let handle = targeted_send_handle(commands.clone(), disconnect_requested);
    let session = Session::new_for_test(handle, mpsc::channel(1).1);
    assert!(matches!(
        session.send(chat_message_with_size(MAX_OUTBOUND_CHAT_MESSAGE_BYTES + 1), Delivery::Unreliable),
        Err(Error::OutboundMessageTooLarge)
    ));

    let request = RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: vec![ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 7 }; MAX_OUTBOUND_OBJECT_REQUESTS + 1],
    };
    assert!(matches!(session.send_to(SimulatorId::new(42), request, Delivery::Reliable).await, Err(Error::OutboundMessageTooLarge)));

    assert_eq!(commands.capacity(), 1);
    assert!(matches!(command_receiver.try_recv(), Err(mpsc::error::TryRecvError::Empty)));
}

#[test]
fn disconnect_request_preserves_terminal_status() {
    let (commands, _command_receiver) = mpsc::channel(1);
    let (disconnect_requested, _) = watch::channel(false);
    let (_worker_finished_sender, worker_finished) = watch::channel(false);
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands,
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
        status: status.clone(),
    };

    for terminal in [NetworkLifecycle::Disconnected, NetworkLifecycle::Failed] {
        status.set_lifecycle(terminal);
        handle.request_disconnect();
        assert_eq!(status.snapshot().lifecycle, terminal);
    }
}

#[test]
fn session_drop_requests_disconnect() {
    let (commands, _command_receiver) = mpsc::channel(1);
    let (disconnect_requested, disconnect_observed) = watch::channel(false);
    let (_worker_finished_sender, worker_finished) = watch::channel(false);
    let status = NetworkStatusHandle::new();
    let handle = SessionHandle {
        commands,
        disconnect_requested,
        command_admission_closed: Arc::new(Mutex::new(false)),
        worker_finished,
        status: status.clone(),
    };
    let session = Session::new_for_test(handle, mpsc::channel(1).1);

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
        spawn(async move { send_failure_event(&failure_events, "network failed".into(), &mut shutdown_requested).await });

    assert!(timeout(Duration::from_millis(10), &mut send_failure).await.is_err(), "full queue should apply backpressure");
    shutdown.send_replace(true);
    let outcome =
        timeout(Duration::from_secs(1), send_failure).await.expect("shutdown should release the blocked send").expect("send task");

    assert!(matches!(outcome, EventSendOutcome::Shutdown));
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
