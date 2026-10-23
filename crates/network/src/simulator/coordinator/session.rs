use std::{collections::HashMap, future::pending, net::SocketAddr, time::Duration};

use crate::{
    AgentSession, Error, Grid,
    event::{CLIENT_EVENT_CAPACITY, Event, EventEnvelope, PRIMARY_TERMINAL_EVENT_CAPACITY},
    session::{COMMAND_CAPACITY, Command},
    simulator::coordinator::{
        LOGOUT_TIMEOUT, LogoutAttemptError, LogoutOutcome, LogoutUnavailable, MAX_PENDING_SIMULATOR_EVENTS, send_logout_with_deadline,
    },
    simulator::{SimulatorCoordinator, run_session},
    status::{NetworkLifecycle, NetworkStatusHandle},
    transport::UdpTransport,
    transport::lludp::{
        Circuit, DEFAULT_RETRY_TIMEOUT, Delivery, MAX_PENDING_PACKETS, MAX_RETRIES, PendingPacket, RESENT_FLAG, ReceiveError,
        acknowledge_pending, retransmit_due,
    },
};
use ::test_support::lludp::fixture;
use protocol::{
    ChatType,
    lludp::{Message, Packet},
    login::ResponseData,
    messages::{
        ChatFromViewer, CompletePingCheck, LogoutRequest, PacketAck, PingId, chat_from_viewer, chat_from_viewer::ChatData, logout_request,
        packet_ack::Packets,
    },
};
use tokio::{
    net::UdpSocket,
    spawn,
    sync::{mpsc, oneshot, watch},
    task::yield_now,
    time::{self, Instant, timeout},
};
use url::Url;
use uuid::Uuid;

fn complete_ping(ping_id: u8) -> Message {
    Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id } })
}

fn packet_ack(packet_ids: &[u32]) -> Message {
    Message::PacketAck(PacketAck { packets: packet_ids.iter().map(|id| Packets { id: *id }).collect() })
}

fn packet_ack_contains(message: &Message, packet_id: u32) -> bool {
    matches!(message, Message::PacketAck(ack) if ack.packets.iter().any(|packet| packet.id == packet_id))
}

fn receive_packet(circuit: &mut Circuit, packet: &Packet) -> bool {
    circuit.receive(packet).expect("valid circuit packet").is_new_packet
}

fn is_complete_ping(message: &Message, ping_id: u8) -> bool {
    matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == ping_id)
}

const AGENT_ID: &str = "ffeeddcc-bbaa-9988-7766-554433221100";
const SESSION_ID: &str = "00112233-4455-6677-8899-aabbccddeeff";
const CIRCUIT_CODE: u32 = 0x1122_3344;
const SIMULATOR_TIMEOUT: Duration = Duration::from_secs(2);
const SESSION_EVENT_CAPACITY: usize = 2;
const RETRANSMISSION_TIMEOUT: Duration = Duration::from_secs(8);

fn chat_message(message: impl Into<String>) -> Message {
    Message::from(ChatFromViewer {
        agent_data: chat_from_viewer::AgentData {
            agent_id: Uuid::parse_str(AGENT_ID).expect("valid test agent ID"),
            session_id: Uuid::parse_str(SESSION_ID).expect("valid test session ID"),
        },
        chat_data: ChatData { message: message.into(), chat_type: ChatType::Normal, channel: 0 },
    })
}

fn logout_message() -> Message {
    Message::LogoutRequest(LogoutRequest {
        agent_data: logout_request::AgentData {
            agent_id: Uuid::parse_str(AGENT_ID).expect("valid test agent ID"),
            session_id: Uuid::parse_str(SESSION_ID).expect("valid test session ID"),
        },
    })
}

#[tokio::test]
async fn root_disconnect_skips_remote_logout_wait() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut connection = SimulatorCoordinator::connect_with_status(
        login_session(simulator.local_addr().expect("simulator address")),
        ResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect root simulator circuit");

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    let outcome = timeout(Duration::from_millis(200), connection.disconnect_once(false))
        .await
        .expect("root logout send should be bounded")
        .expect("local logout send should succeed");
    assert_eq!(outcome, LogoutOutcome::Sent);

    let (logout, logout_peer) = receive(&simulator).await;
    assert_eq!(logout_peer, peer);
    assert_eq!(logout, expected_fixture("logout_request.hex", 2, true));
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn logout_deadline_reports_unsent() {
    let send = spawn(send_logout_with_deadline(pending::<Result<(), LogoutAttemptError>>(), LogoutUnavailable::Deadline));
    yield_now().await;
    time::advance(LOGOUT_TIMEOUT).await;

    assert_eq!(
        send.await.expect("logout deadline task should not panic").expect_err("pending local send should time out"),
        LogoutAttemptError::Unavailable(LogoutUnavailable::Deadline)
    );
}

#[tokio::test]
async fn logout_failure_preserves_unexpected_error() {
    let send = async { Err(LogoutAttemptError::Unexpected("socket send failed".into())) };

    assert_eq!(
        send_logout_with_deadline(send, LogoutUnavailable::Deadline).await,
        Err(LogoutAttemptError::Unexpected("socket send failed".into()))
    );
}

#[tokio::test]
async fn disconnect_reports_closed_primary() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let status = NetworkStatusHandle::default();
    let mut connection = SimulatorCoordinator::connect_with_status(
        login_session(simulator.local_addr().expect("simulator address")),
        ResponseData::default(),
        None,
        status.clone(),
    )
    .await
    .expect("connect root simulator circuit");
    receive(&simulator).await;
    let (logout, receiver) = mpsc::channel(1);
    drop(receiver);
    connection.primary.logout = logout;

    assert_eq!(
        connection.disconnect_once(false).await.expect("closed route is an unavailable logout"),
        LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionClosed)
    );
    let snapshot = status.snapshot();
    assert_eq!(snapshot.lifecycle, NetworkLifecycle::Disconnecting);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("Simulator logout request was not sent before disconnect completed: the active simulator connection is closed")
    );
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn disconnect_logout_respects_deadline() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let status = NetworkStatusHandle::default();
    let mut connection = SimulatorCoordinator::connect_with_status(
        login_session(simulator.local_addr().expect("simulator address")),
        ResponseData::default(),
        None,
        status.clone(),
    )
    .await
    .expect("connect root simulator circuit");
    receive(&simulator).await;
    let (logout, _logout_receiver) = mpsc::channel(1);
    let (queued_logout, _queued_response) = oneshot::channel();
    logout.try_send(queued_logout).expect("fill the destination logout queue");
    connection.primary.logout = logout;
    let disconnect = spawn(async move { connection.disconnect_once(false).await });

    yield_now().await;
    time::advance(LOGOUT_TIMEOUT).await;

    assert_eq!(
        disconnect.await.expect("disconnect task should not panic").expect("deadline is an unavailable logout"),
        LogoutOutcome::Unavailable(LogoutUnavailable::Deadline)
    );
    let snapshot = status.snapshot();
    assert_eq!(snapshot.lifecycle, NetworkLifecycle::Disconnecting);
    assert_eq!(
        snapshot.last_error.as_deref(),
        Some("Simulator logout request was not sent before disconnect completed: the bounded logout send deadline expired")
    );
}

#[tokio::test]
async fn failed_root_disconnect_reports_no_send() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut connection = SimulatorCoordinator::connect_with_status(
        login_session(simulator.local_addr().expect("simulator address")),
        ResponseData::default(),
        None,
        NetworkStatusHandle::default(),
    )
    .await
    .expect("connect root simulator circuit");
    receive(&simulator).await;
    connection.root_connection_failed = true;

    assert_eq!(
        connection.disconnect_once(false).await.expect("failed root connection is an unavailable logout"),
        LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionClosed)
    );
    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err());
}

#[tokio::test]
async fn handshake_chat_logout() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));

    let mut buffer = [0; 65_535];
    assert!(timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err());
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");

    let (movement, movement_peer) = receive(&simulator).await;
    assert_eq!(movement_peer, peer);
    assert_eq!(movement, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    let mut malformed = [0x80, 0, 0, 0, 0, 0, 0x04, 0];
    set_sequence(&mut malformed, 22);
    simulator.send_to(&malformed, peer).await.expect("send malformed datagram");

    let mut region_handshake = fixture("region_handshake.hex");
    region_handshake[0] |= 0x40;
    set_sequence(&mut region_handshake, 24);
    simulator.send_to(&region_handshake, peer).await.expect("send region handshake before delayed movement completion");
    let (reply, reply_peer) = receive(&simulator).await;
    assert_eq!(reply_peer, peer);
    assert_eq!(reply, with_appended_ack(expected_zerocoded_reply(3), 24));
    simulator.send_to(&server_ack(25, &[3]), peer).await.expect("ack region handshake reply");
    let handshake_event = next_event(&mut events).await;
    let Event::SimulatorMessage { source, sequence, message, .. } = handshake_event else {
        panic!("expected the region handshake to be delivered to the application");
    };
    assert_eq!(source, simulator.local_addr().expect("simulator address"));
    assert_eq!(sequence, 24);
    assert!(matches!(message, Message::RegionHandshake(message) if message.region_info1.sim_name == "Test Region"));

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("deliver delayed movement completion out of order");
    let Event::SessionReady { capabilities, .. } = next_event(&mut events).await else {
        panic!("expected the simulator session to become ready");
    };
    let movement_event = next_event(&mut events).await;
    assert!(matches!(movement_event, Event::SimulatorMessage { sequence: 23, message: Message::AgentMovementComplete(_), .. }));

    let throttle = Packet::decode(&receive(&simulator).await.0).expect("decode AgentThrottle");
    assert!(matches!(throttle.message(), Message::AgentThrottle(_)));
    assert!(throttle.is_reliable());
    assert!(throttle.appended_acks().contains(&23), "AgentThrottle carries the pending movement ACK");
    simulator.send_to(&server_ack(26, &[throttle.sequence()]), peer).await.expect("ack AgentThrottle");

    commands
        .send(Command::SendMessage { message: chat_message("ab"), delivery: Delivery::Reliable })
        .await
        .expect("session should accept a simulator message");
    let (chat, chat_peer) = receive(&simulator).await;
    assert_eq!(chat_peer, peer);
    assert_eq!(chat, expected_chat(5, "ab", 1, 0));
    simulator.send_to(&server_ack(27, &[5]), peer).await.expect("ack nearby chat");

    let mut incoming_chat = fixture("chat_from_simulator.hex");
    incoming_chat[0] |= 0x40;
    set_sequence(&mut incoming_chat, 28);
    simulator.send_to(&incoming_chat, peer).await.expect("send reliable nearby chat");
    let (first_ack, first_ack_peer) = receive(&simulator).await;
    assert_eq!(first_ack_peer, peer);
    assert_eq!(first_ack, client_ack(6, 28));

    incoming_chat[0] |= RESENT_FLAG;
    simulator.send_to(&incoming_chat, peer).await.expect("resend nearby chat");
    let (duplicate_ack, duplicate_ack_peer) = receive(&simulator).await;
    assert_eq!(duplicate_ack_peer, peer);
    assert_eq!(duplicate_ack, client_ack(7, 28));

    let chat_event = next_event(&mut events).await;
    let Event::SimulatorMessage { source, sequence, message, .. } = chat_event else {
        panic!("expected one generic simulator message event");
    };
    assert_eq!(source, simulator.local_addr().expect("simulator address"));
    assert_eq!(sequence, 28);
    let Message::ChatFromSimulator(message) = message else {
        panic!("expected decoded ChatFromSimulator message");
    };
    assert_eq!(message.chat_data.from_name, "Ada");
    assert_eq!(message.chat_data.message, "Hi");
    yield_now().await;
    assert!(matches!(events.try_recv(), Err(mpsc::error::TryRecvError::Empty)));

    disconnect_signal.send_replace(true);
    let (logout, logout_peer) = receive(&simulator).await;
    assert_eq!(logout_peer, peer);
    assert_eq!(logout, expected_fixture("logout_request.hex", 8, true));
    assert!(
        timeout(Duration::from_millis(200), client)
            .await
            .expect("disconnect should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
    let error = capabilities.request_capabilities(&["TestCapability"]).await.expect_err("session teardown closes retained capabilities");
    assert!(matches!(error, Error::CapabilitySessionClosed));
}

#[tokio::test]
async fn ping_reacks_recent_duplicate() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (_, peer) = receive(&simulator).await;
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    let mut chat = fixture("chat_from_simulator.hex");
    chat[0] |= 0x40;
    set_sequence(&mut chat, 100);
    simulator.send_to(&chat, peer).await.expect("send reliable chat");
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 100, .. }));
    receive_ack(&simulator, 100).await;

    let ping = start_ping_packet(102, 9, 101);
    simulator.send_to(&ping, peer).await.expect("send ping checkpoint");
    receive_message(&simulator, |message| is_complete_ping(message, 9)).await;

    chat[0] |= RESENT_FLAG;
    simulator.send_to(&chat, peer).await.expect("resend acknowledged chat");
    receive_ack(&simulator, 100).await;
    assert!(timeout(Duration::from_millis(50), events.recv()).await.is_err(), "recent reliable duplicate should not be dispatched twice");

    disconnect_signal.send_replace(true);
    receive_message(&simulator, |message| matches!(message, Message::LogoutRequest(_))).await;
    assert!(timeout(Duration::from_millis(200), client).await.expect("disconnect should finish").expect("session task").is_ok());
}

#[tokio::test]
async fn shutdown_preempts_chat() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    loop {
        if matches!(next_event(&mut events).await, Event::SessionReady { .. }) {
            break;
        }
    }

    commands
        .try_send(Command::SendMessage { message: chat_message("must not send"), delivery: Delivery::Reliable })
        .expect("queue chat before shutdown");
    disconnect_signal.send_replace(true);

    let logout = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client packet");
            assert!(!matches!(packet.message(), Message::ChatFromViewer(_)), "queued chat must not be sent after shutdown");
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break packet;
            }
        }
    })
    .await
    .expect("shutdown should send a logout request");
    assert!(logout.is_reliable());
    assert_eq!(logout.message(), &logout_message());
    assert!(
        timeout(Duration::from_millis(200), client)
            .await
            .expect("shutdown should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
    let mut buffer = [0; 65_535];
    assert!(
        timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err(),
        "shutdown should close the simulator socket"
    );
}

#[tokio::test]
async fn backpressure_keeps_acks() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    let throttle = Packet::decode(&receive(&simulator).await.0).expect("decode AgentThrottle");
    assert!(matches!(throttle.message(), Message::AgentThrottle(_)));
    assert!(throttle.is_reliable());
    assert!(throttle.appended_acks().contains(&23), "AgentThrottle carries the pending movement ACK");
    simulator.send_to(&server_ack(24, &[throttle.sequence()]), peer).await.expect("ack AgentThrottle");

    disconnect_signal.send_replace(true);
    assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", 5, true));
    assert!(
        timeout(Duration::from_millis(200), client)
            .await
            .expect("disconnect should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
}

#[tokio::test]
async fn ping_survives_backpressure() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
    simulator.send_to(&server_ack(26, &[3]), peer).await.expect("ack region handshake reply");
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. }));

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    assert!(matches!(next_event(&mut events).await, Event::SessionReady { .. }));
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. }));

    for sequence in [24, 25] {
        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        simulator.send_to(&chat, peer).await.expect("send reliable simulator chat");
    }

    let ping = start_ping_packet(27, 7, 0);
    simulator.send_to(&ping, peer).await.expect("send ping request while viewer queue is full");
    let ping_reply = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode simulator reply");
            if is_complete_ping(packet.message(), 7) {
                break packet;
            }
        }
    })
    .await
    .expect("simulator should answer ping while event delivery is backed up");
    assert_eq!(ping_reply.message(), &complete_ping(7));
    assert!(matches!(events.try_recv().map(EventEnvelope::into_event), Ok(Event::SimulatorMessage { sequence: 24, .. })));

    disconnect_signal.send_replace(true);
    let logout = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client message");
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break packet;
            }
        }
    })
    .await
    .expect("disconnect should be handled while events are queued");
    assert!(matches!(logout.message(), Message::LogoutRequest(_)));
    assert!(
        timeout(Duration::from_millis(200), client)
            .await
            .expect("disconnect should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
}

#[tokio::test]
async fn overload_keeps_control() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (_disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
    simulator.send_to(&server_ack(26, &[3]), peer).await.expect("ack region handshake reply");
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. }));

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion");
    assert!(matches!(next_event(&mut events).await, Event::SessionReady { .. }));
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 23, .. }));

    let message_count = SESSION_EVENT_CAPACITY + MAX_PENDING_SIMULATOR_EVENTS;
    for sequence in 24..24 + message_count as u32 {
        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        simulator.send_to(&chat, peer).await.expect("fill the bounded event backlog");
    }

    let logout = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client message");
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break packet;
            }
        }
    })
    .await
    .expect("saturated event backlog should start logout");

    let mut undeliverable_chat = fixture("chat_from_simulator.hex");
    undeliverable_chat[0] |= 0x40;
    set_sequence(&mut undeliverable_chat, 42);
    simulator.send_to(&undeliverable_chat, peer).await.expect("send chat after overload starts");
    let ping = start_ping_packet(43, 8, 0);
    simulator.send_to(&ping, peer).await.expect("send ping while logging out");

    let mut acknowledged_undeliverable_chat = false;
    let ping_reply = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode simulator reply");
            acknowledged_undeliverable_chat |= packet.appended_acks().contains(&42) || packet_ack_contains(packet.message(), 42);
            if is_complete_ping(packet.message(), 8) {
                break packet;
            }
        }
    })
    .await
    .expect("ping should be answered after event backlog saturation");
    assert!(is_complete_ping(ping_reply.message(), 8));
    assert!(!acknowledged_undeliverable_chat, "a reliable payload rejected by event admission must not be acknowledged");

    simulator.send_to(&server_ack(50, &[logout.sequence()]), peer).await.expect("ack logout request");
    let mut logout_reply = fixture("logout_reply.hex");
    logout_reply[0] |= 0x40;
    set_sequence(&mut logout_reply, 51);
    simulator.send_to(&logout_reply, peer).await.expect("send logout reply");
    assert_eq!(receive(&simulator).await.0, client_ack(logout.sequence() + 2, 51));
    let failure = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            if let Event::Failed(reason) = next_event(&mut events).await {
                break reason;
            }
        }
    })
    .await
    .expect("event backlog disconnect should reach the viewer");
    assert!(failure.contains("could not keep up with incoming events"));
    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_err());
}

#[tokio::test]
async fn handshake_fits_backlog() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
    simulator.send_to(&server_ack(26, &[3]), peer).await.expect("ack region handshake reply");
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { sequence: 22, .. }));

    for sequence in 23..=38 {
        let mut chat = fixture("chat_from_simulator.hex");
        chat[0] |= 0x40;
        set_sequence(&mut chat, sequence);
        simulator.send_to(&chat, peer).await.expect("fill the bounded event backlog");

        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client ACK");
            let acknowledged = packet.appended_acks().contains(&sequence) || packet_ack_contains(packet.message(), sequence);
            if acknowledged {
                break;
            }
        }
    }

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 39);
    simulator.send_to(&movement_complete, peer).await.expect("send movement completion at backlog limit");

    let mut acknowledged_movement_complete = false;
    let logout = timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(&simulator).await;
            let packet = Packet::decode(&bytes).expect("decode client message");
            acknowledged_movement_complete |= packet.appended_acks().contains(&39) || packet_ack_contains(packet.message(), 39);
            if matches!(packet.message(), Message::LogoutRequest(_)) {
                break packet;
            }
        }
    })
    .await
    .expect("full backlog should start logout after both handshake events are queued");
    assert_ne!(logout.sequence(), 0);
    acknowledged_movement_complete |= logout.appended_acks().contains(&39) || packet_ack_contains(logout.message(), 39);
    assert!(acknowledged_movement_complete, "reliable movement completion must be acknowledged before disconnect");

    let mut saw_session_ready = false;
    let mut saw_movement_complete = false;
    while !(saw_session_ready && saw_movement_complete) {
        match next_event(&mut events).await {
            Event::SessionReady { .. } => saw_session_ready = true,
            Event::SimulatorMessage { sequence: 39, .. } => saw_movement_complete = true,
            _ => {}
        }
    }

    disconnect_signal.send_replace(true);
    assert!(
        timeout(Duration::from_millis(200), client)
            .await
            .expect("disconnect should not wait for a simulator reply")
            .expect("session task should not panic")
            .is_ok()
    );
}

#[tokio::test]
async fn closed_receiver_logs_out() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, mut events) = mpsc::channel(SESSION_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (_disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    receive_primary_connected(&mut events).await;

    let mut handshake = fixture("region_handshake.hex");
    handshake[0] |= 0x40;
    set_sequence(&mut handshake, 22);
    simulator.send_to(&handshake, peer).await.expect("send region handshake");
    assert_eq!(receive(&simulator).await.0, with_appended_ack(expected_zerocoded_reply(3), 22));
    assert!(matches!(next_event(&mut events).await, Event::SimulatorMessage { message: Message::RegionHandshake(_), .. }));

    let mut movement_complete = fixture("agent_movement_complete.hex");
    movement_complete[0] |= 0x40;
    set_sequence(&mut movement_complete, 23);
    drop(events);
    simulator.send_to(&movement_complete, peer).await.expect("send packet as receiver closes");
    assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", 4, true));

    let mut buffer = [0; 65_535];
    let unexpected = timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await;
    assert!(!matches!(&unexpected, Ok(Ok(_))), "closed receiver must not ACK an undeliverable message: {unexpected:?}");

    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
}

#[tokio::test]
async fn disconnect_blocks_movement() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));

    simulator.try_send_to(&server_ack(20, &[1]), peer).expect("queue circuit ACK while disconnect is pending");
    disconnect_signal.send_replace(true);
    let (logout, _) = receive(&simulator).await;
    assert_eq!(logout, expected_fixture("logout_request.hex", 2, true));

    let mut buffer = [0; 65_535];
    assert!(
        timeout(Duration::from_millis(30), simulator.recv_from(&mut buffer)).await.is_err(),
        "a queued circuit ACK must not start movement after disconnect is requested"
    );

    assert!(
        timeout(SIMULATOR_TIMEOUT, client)
            .await
            .expect("session should stop after disconnect")
            .expect("session task should not panic")
            .is_ok()
    );
}

#[tokio::test]
async fn retry_resends_then_expires() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");

    let original = expected_fixture("use_circuit_code.hex", 42, true);
    let mut pending =
        HashMap::from([(42, PendingPacket { bytes: original.clone(), sent_at: Instant::now() - DEFAULT_RETRY_TIMEOUT, retries: 0 })]);

    for retry in 1..=MAX_RETRIES {
        retransmit_due(&client, &mut pending).await.expect("packet should retry before limit");
        let (resent, _) = receive(&simulator).await;
        let mut expected = original.clone();
        expected[0] |= RESENT_FLAG;
        assert_eq!(resent, expected);
        assert_eq!(pending[&42].retries, retry);
        pending.get_mut(&42).expect("pending packet").sent_at = Instant::now() - DEFAULT_RETRY_TIMEOUT;
    }

    assert_eq!(retransmit_due(&client, &mut pending).await, Err("The simulator did not acknowledge a reliable packet.".into()));
}

#[tokio::test]
async fn lost_packet_retries() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (original, peer) = receive(&simulator).await;
    assert_eq!(original, expected_fixture("use_circuit_code.hex", 1, true));

    let (resent, resent_peer) = receive_with_timeout(&simulator, RETRANSMISSION_TIMEOUT).await;
    let mut expected_resent = original;
    expected_resent[0] |= RESENT_FLAG;
    assert_eq!(resent_peer, peer);
    assert_eq!(resent, expected_resent);

    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack retransmitted circuit packet");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));

    disconnect_signal.send_replace(true);
    let logout = loop {
        let (bytes, _) = receive(&simulator).await;
        let packet = Packet::decode(&bytes).expect("decode disconnect packet");
        if let Message::StartPingCheck(_) = packet.message() {
            assert_eq!(packet.sequence(), 3);
            assert!(!packet.is_reliable());
            continue;
        }
        break packet;
    };
    assert!((3..=4).contains(&logout.sequence()));
    assert!(logout.is_reliable());
    assert_eq!(logout.message(), &logout_message());
    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
}

#[tokio::test]
async fn overflow_keeps_sequence() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    for expected_sequence in 1..=MAX_PENDING_PACKETS as u32 {
        let sequence = circuit.send(&client, complete_ping(3), Delivery::Reliable).await.expect("packet should fit in the reliable window");
        assert_eq!(sequence, expected_sequence);
    }
    assert!(circuit.send(&client, complete_ping(4), Delivery::Reliable).await.is_err());

    for expected_sequence in 1..=MAX_PENDING_PACKETS as u32 {
        let (bytes, _) = receive(&simulator).await;
        assert_eq!(Packet::decode(&bytes).expect("decode queued packet").sequence(), expected_sequence);
    }
    assert!(
        timeout(Duration::from_millis(30), async {
            let mut buffer = [0; 32];
            simulator.recv_from(&mut buffer).await
        })
        .await
        .is_err()
    );

    let ack = Packet::decode(&server_ack(50, &[1])).expect("decode fake simulator ACK");
    assert!(receive_packet(&mut circuit, &ack));
    assert_eq!(
        circuit.send(&client, complete_ping(5), Delivery::Reliable).await.expect("ACK should free one slot"),
        MAX_PENDING_PACKETS as u32 + 1
    );
    let (bytes, _) = receive(&simulator).await;
    assert_eq!(Packet::decode(&bytes).expect("decode packet after ACK").sequence(), MAX_PENDING_PACKETS as u32 + 1);
}

#[tokio::test]
async fn packet_piggybacks_ack() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut incoming = fixture("chat_from_simulator.hex");
    incoming[0] |= 0x40;
    set_sequence(&mut incoming, 55);
    let incoming = Packet::decode(&incoming).expect("decode reliable simulator packet");
    assert!(receive_packet(&mut circuit, &incoming));

    circuit.send(&client, complete_ping(3), Delivery::Unreliable).await.expect("send next outbound packet");
    let (bytes, _) = receive(&simulator).await;
    let packet = Packet::decode(&bytes).expect("decode client packet");

    assert_eq!(packet.message(), &complete_ping(3));
    assert_eq!(packet.appended_acks(), &[55]);
}

#[tokio::test]
async fn timer_acks_recover_loss() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let session = login_session(simulator.local_addr().expect("simulator address"));
    let (_commands, command_receiver) = mpsc::channel(COMMAND_CAPACITY);
    let (event_sender, _events) = mpsc::channel(CLIENT_EVENT_CAPACITY + PRIMARY_TERMINAL_EVENT_CAPACITY);
    let (disconnect_signal, disconnect_requested) = watch::channel(false);
    let client = spawn(run_session(session, command_receiver, event_sender, disconnect_requested));

    let (use_circuit, peer) = receive(&simulator).await;
    assert_eq!(use_circuit, expected_fixture("use_circuit_code.hex", 1, true));
    simulator.send_to(&server_ack(20, &[1]), peer).await.expect("ack circuit establishment");
    assert_eq!(receive(&simulator).await.0, expected_complete_movement(2));
    simulator.send_to(&server_ack(21, &[2]), peer).await.expect("ack complete movement");

    for sequence in [55, 56] {
        let mut incoming = fixture("chat_from_simulator.hex");
        incoming[0] |= 0x40;
        set_sequence(&mut incoming, sequence);
        simulator.send_to(&incoming, peer).await.expect("send reliable simulator chat");
    }

    let mut acknowledged = Vec::<u32>::new();
    let mut next_sequence = 3;
    while acknowledged.len() < 2 {
        let (bytes, _) = timeout(SIMULATOR_TIMEOUT, receive(&simulator)).await.expect("timer should send queued acknowledgements");
        let packet = Packet::decode(&bytes).expect("decode timer ACK packet");
        assert_eq!(packet.sequence(), next_sequence);
        let Message::PacketAck(message) = packet.message() else {
            panic!("expected timer to send a standalone ACK packet");
        };
        let packet_ids = message.packets.iter().map(|packet| packet.id).collect::<Vec<_>>();
        assert_eq!(bytes, server_ack(next_sequence, &packet_ids));
        acknowledged.extend(packet_ids);
        next_sequence += 1;
    }
    acknowledged.sort_unstable();
    assert_eq!(acknowledged, [55, 56]);

    disconnect_signal.send_replace(true);
    assert_eq!(receive(&simulator).await.0, expected_fixture("logout_request.hex", next_sequence, true));
    assert!(timeout(SIMULATOR_TIMEOUT, client).await.expect("session should stop").expect("session task should not panic").is_ok());
}

#[tokio::test]
async fn encode_failure_keeps_ack() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut incoming = fixture("chat_from_simulator.hex");
    incoming[0] |= 0x40;
    set_sequence(&mut incoming, 55);
    let incoming = Packet::decode(&incoming).expect("decode reliable simulator packet");
    assert!(receive_packet(&mut circuit, &incoming));

    let error = circuit
        .send(&client, chat_message("x".repeat(65_520)), Delivery::Unreliable)
        .await
        .expect_err("message should exceed the LLUDP maximum size");
    assert!(error.contains("Could not encode a simulator message"));
    circuit.send(&client, complete_ping(3), Delivery::Unreliable).await.expect("send packet after failed encode");

    let (bytes, _) = receive(&simulator).await;
    let packet = Packet::decode(&bytes).expect("decode client packet");
    assert_eq!(packet.sequence(), 1);
    assert_eq!(packet.appended_acks(), &[55]);
}

#[tokio::test]
async fn duplicate_requeues_ack() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut incoming = fixture("chat_from_simulator.hex");
    incoming[0] |= 0x40;
    set_sequence(&mut incoming, 55);
    let packet = Packet::decode(&incoming).expect("decode reliable simulator packet");
    assert!(receive_packet(&mut circuit, &packet));
    circuit.flush_pending_acks(&client).await.expect("flush first acknowledgement");
    assert_eq!(Packet::decode(&receive(&simulator).await.0).expect("decode first ACK").message(), &packet_ack(&[55]));

    incoming[0] |= RESENT_FLAG;
    let duplicate = Packet::decode(&incoming).expect("decode resent simulator packet");
    assert!(!receive_packet(&mut circuit, &duplicate));
    circuit.flush_pending_acks(&client).await.expect("flush duplicate acknowledgement");
    assert_eq!(Packet::decode(&receive(&simulator).await.0).expect("decode duplicate ACK").message(), &packet_ack(&[55]));
}

#[tokio::test]
async fn full_ack_queue_defers_send() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let client = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect local UDP socket");
    let mut circuit = Circuit::new();

    let mut sequence = 1;
    loop {
        let packet = Packet::new(sequence, complete_ping(3)).with_reliable();
        match circuit.receive(&packet) {
            Ok(outcome) => assert!(outcome.is_new_packet),
            Err(ReceiveError::AcknowledgementCapacityFull) => break,
            Err(error) => panic!("unexpected receive rejection: {error}"),
        }
        sequence += 1;
    }
    let overflow = Packet::new(sequence, complete_ping(3)).with_reliable();
    assert_eq!(circuit.receive(&overflow), Err(ReceiveError::AcknowledgementCapacityFull));

    circuit.send(&client, complete_ping(4), Delivery::Unreliable).await.expect("send packet to make ACK queue space");
    let (bytes, _) = receive(&simulator).await;
    let packet = Packet::decode(&bytes).expect("decode client packet");
    assert_eq!(packet.appended_acks().len(), u8::MAX as usize);
    assert!(receive_packet(&mut circuit, &overflow), "unacknowledged input should be accepted after queue space opens");
}

#[test]
fn late_ack_clears_pending() {
    let acknowledged = [0x1122_3344, 0xa1b2_c3d4];
    let mut pending = pending_packets(&acknowledged);
    let packet = Packet::decode(&fixture("packet_ack.hex")).expect("decode ACK fixture");

    acknowledge_pending(&packet, &mut pending);
    assert!(pending.is_empty());

    let mut pending = pending_packets(&acknowledged);
    let packet = Packet::decode(&fixture("appended_acks.hex")).expect("decode appended ACK fixture");
    acknowledge_pending(&packet, &mut pending);
    assert!(pending.is_empty());
}

fn login_session(simulator_address: SocketAddr) -> AgentSession {
    AgentSession {
        first_name: "Test".into(),
        last_name: "Resident".into(),
        grid: Grid::Agni,
        message: "Welcome".into(),
        simulator_address,
        agent_id: Uuid::parse_str(AGENT_ID).expect("valid agent UUID"),
        session_id: Uuid::parse_str(SESSION_ID).expect("valid session UUID"),
        circuit_code: CIRCUIT_CODE,
        seed_capability: Url::parse("http://127.0.0.1:1/seed").expect("valid local capability URL"),
    }
}

async fn receive(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    receive_with_timeout(socket, SIMULATOR_TIMEOUT).await
}

async fn receive_with_timeout(socket: &UdpSocket, wait: Duration) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, peer) = timeout(wait, socket.recv_from(&mut buffer))
        .await
        .expect("client should send a UDP datagram")
        .expect("read fake simulator datagram");
    (buffer[..size].to_vec(), peer)
}

async fn receive_ack(socket: &UdpSocket, sequence: u32) -> Packet {
    timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(socket).await;
            let packet = Packet::decode(&bytes).expect("decode client packet");
            let in_message = packet_ack_contains(packet.message(), sequence);
            if in_message || packet.appended_acks().contains(&sequence) {
                return packet;
            }
        }
    })
    .await
    .expect("client should acknowledge the reliable packet")
}

async fn receive_message(socket: &UdpSocket, expected: impl Fn(&Message) -> bool) -> Packet {
    timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let (bytes, _) = receive(socket).await;
            let packet = Packet::decode(&bytes).expect("decode client packet");
            if expected(packet.message()) {
                return packet;
            }
        }
    })
    .await
    .expect("client should send the expected message")
}

async fn next_event(events: &mut mpsc::Receiver<EventEnvelope>) -> Event {
    timeout(SIMULATOR_TIMEOUT, async {
        loop {
            let event = events.recv().await.expect("client event channel should remain open").into_event();
            if matches!(&event, Event::CapabilityError { capability, .. } if capability == "EventQueueGet") {
                continue;
            }
            if matches!(&event, Event::SimulatorConnected { region_handle: None, .. }) {
                continue;
            }
            return event;
        }
    })
    .await
    .expect("client should publish an event")
}

async fn receive_primary_connected(events: &mut mpsc::Receiver<EventEnvelope>) {
    timeout(SIMULATOR_TIMEOUT, async {
        loop {
            match events.recv().await.expect("client event channel should remain open").into_event() {
                Event::SimulatorConnected { region_handle: None, .. } => return,
                Event::CapabilityError { capability, .. } if capability == "EventQueueGet" => {}
                _ => panic!("unexpected event before primary connection"),
            }
        }
    })
    .await
    .expect("client should publish primary connection");
}

fn expected_fixture(name: &str, sequence: u32, reliable: bool) -> Vec<u8> {
    let mut bytes = fixture(name);
    bytes[0] = (bytes[0] & !0x40) | if reliable { 0x40 } else { 0 };
    set_sequence(&mut bytes, sequence);
    bytes
}

fn expected_complete_movement(sequence: u32) -> Vec<u8> {
    let mut bytes = fixture("complete_agent_movement.hex");
    bytes[0] |= 0x40;
    set_sequence(&mut bytes, sequence);
    let end = bytes.len();
    bytes[end - 4..].copy_from_slice(&CIRCUIT_CODE.to_le_bytes());
    bytes
}

fn expected_zerocoded_reply(sequence: u32) -> Vec<u8> {
    let mut bytes = fixture("region_handshake_reply.hex");
    bytes[0] |= 0x40;
    set_sequence(&mut bytes, sequence);
    bytes
}

fn with_appended_ack(mut bytes: Vec<u8>, packet_id: u32) -> Vec<u8> {
    bytes[0] |= 0x10;
    bytes.extend_from_slice(&packet_id.to_be_bytes());
    bytes.push(1);
    bytes
}

fn expected_chat(sequence: u32, message: &str, chat_type: u8, channel: i32) -> Vec<u8> {
    let mut bytes = vec![0x40];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&[0, 0xff, 0xff, 0x00, 0x50]);
    bytes.extend_from_slice(Uuid::parse_str(AGENT_ID).expect("valid agent UUID").as_bytes());
    bytes.extend_from_slice(Uuid::parse_str(SESSION_ID).expect("valid session UUID").as_bytes());
    bytes.extend_from_slice(&(message.len() as u16 + 1).to_le_bytes());
    bytes.extend_from_slice(message.as_bytes());
    bytes.push(0);
    bytes.push(chat_type);
    bytes.extend_from_slice(&channel.to_le_bytes());
    bytes
}

fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}

fn start_ping_packet(sequence: u32, ping_id: u8, oldest_unacked: u32) -> Vec<u8> {
    let mut bytes = fixture("start_ping_check.hex");
    set_sequence(&mut bytes, sequence);
    bytes[7] = ping_id;
    bytes[8..12].copy_from_slice(&oldest_unacked.to_le_bytes());
    bytes
}

fn server_ack(sequence: u32, acknowledged: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&[0, 0xff, 0xff, 0xff, 0xfb, acknowledged.len() as u8]);
    for id in acknowledged {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    bytes
}

fn client_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    server_ack(sequence, &[acknowledged])
}

fn pending_packets(ids: &[u32]) -> HashMap<u32, PendingPacket> {
    ids.iter()
        .copied()
        .map(|id| (id, PendingPacket { bytes: Vec::new(), sent_at: Instant::now() - DEFAULT_RETRY_TIMEOUT, retries: 1 }))
        .collect()
}

#[test]
fn chat_bytes_match_fixture() {
    assert_eq!(expected_chat(0x0102_0304, "abc", 2, 42), fixture("chat_from_viewer.hex"));
}

#[test]
fn ack_bytes_match_fixture() {
    assert_eq!(server_ack(0x0102_0304, &[0x1122_3344, 0xa1b2_c3d4]), fixture("packet_ack.hex"));
}
