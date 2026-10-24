use std::{future::pending, io::ErrorKind, net::SocketAddr, time::Duration};

use protocol::{
    lludp::{Message, Packet},
    login::ResponseData,
    messages::{CompletePingCheck, LogoutRequest, PacketAck, PingId, agent::logout_request, circuit::packet_ack::Packets},
};
use tokio::{
    net::UdpSocket,
    spawn,
    sync::{mpsc, oneshot},
    task::yield_now,
    time::{self, Instant, timeout},
};
use uuid::Uuid;

use super::test_fixture;
use super::{
    AGENT_ID, LOGOUT_TIMEOUT, LogoutAttemptError, LogoutOutcome, PendingLogout, SESSION_ID, connect_runtime, login_session,
    send_logout_with_deadline, set_sequence,
};
use crate::session::logout::LogoutUnavailable;
use crate::{
    Error,
    circuit::lludp::Circuit,
    event::{Event, EventEnvelope},
};

const CIRCUIT_CODE: u32 = 0x1122_3344;
pub(super) const SIMULATOR_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const RETRANSMISSION_TIMEOUT: Duration = Duration::from_secs(8);

pub(super) fn assert_no_datagram(socket: &UdpSocket) {
    let mut buffer = [0; 65_535];
    std::assert_matches!(socket.try_recv_from(&mut buffer), Err(error) if error.kind() == ErrorKind::WouldBlock);
}

pub(super) fn complete_ping(ping_id: u8) -> Message {
    Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id } })
}

pub(super) fn packet_ack(packet_ids: &[u32]) -> Message {
    Message::PacketAck(PacketAck { packets: packet_ids.iter().map(|id| Packets { id: *id }).collect() })
}

pub(super) fn packet_ack_contains(message: &Message, packet_id: u32) -> bool {
    matches!(message, Message::PacketAck(ack) if ack.packets.iter().any(|packet| packet.id == packet_id))
}

pub(super) fn receive_packet(circuit: &mut Circuit, packet: &Packet) -> bool {
    crate::fixtures::circuit::receive_packet_at(circuit, packet, Instant::now()).expect("valid circuit packet").is_new_packet
}

pub(super) fn is_complete_ping(message: &Message, ping_id: u8) -> bool {
    matches!(message, Message::CompletePingCheck(ping) if ping.ping_id.ping_id == ping_id)
}

pub(super) fn logout_message() -> Message {
    Message::LogoutRequest(LogoutRequest {
        agent_data: logout_request::AgentData {
            agent_id: Uuid::parse_str(AGENT_ID).expect("valid test agent ID"),
            session_id: Uuid::parse_str(SESSION_ID).expect("valid test session ID"),
        },
    })
}

async fn receive(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, peer) = socket.recv_from(&mut buffer).await.expect("read fake simulator datagram");
    (buffer[..size].to_vec(), peer)
}

pub(super) async fn receive_with_timeout(socket: &UdpSocket, wait: Duration) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, peer) = timeout(wait, socket.recv_from(&mut buffer))
        .await
        .expect("client should send a UDP datagram")
        .expect("read fake simulator datagram");
    (buffer[..size].to_vec(), peer)
}

pub(super) async fn receive_primary_connected(events: &mut mpsc::Receiver<EventEnvelope>) {
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

pub(super) fn expected_fixture(name: &str, sequence: u32, reliable: bool) -> Vec<u8> {
    let mut bytes = test_fixture(name);
    bytes[0] = (bytes[0] & !0x40) | if reliable { 0x40 } else { 0 };
    set_sequence(&mut bytes, sequence);
    bytes
}

pub(super) fn expected_complete_movement(sequence: u32) -> Vec<u8> {
    let mut bytes = test_fixture("complete_agent_movement.hex");
    bytes[0] |= 0x40;
    set_sequence(&mut bytes, sequence);
    let end = bytes.len();
    bytes[end - 4..].copy_from_slice(&CIRCUIT_CODE.to_le_bytes());
    bytes
}

pub(super) fn expected_zerocoded_reply(sequence: u32) -> Vec<u8> {
    let mut bytes = test_fixture("region_handshake_reply.hex");
    bytes[0] |= 0x40;
    set_sequence(&mut bytes, sequence);
    bytes
}

pub(super) fn with_appended_ack(mut bytes: Vec<u8>, packet_id: u32) -> Vec<u8> {
    bytes[0] |= 0x10;
    bytes.extend_from_slice(&packet_id.to_be_bytes());
    bytes.push(1);
    bytes
}

pub(super) fn expected_chat(sequence: u32, message: &str, chat_type: u8, channel: i32) -> Vec<u8> {
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

pub(super) fn start_ping_packet(sequence: u32, ping_id: u8, oldest_unacked: u32) -> Vec<u8> {
    let mut bytes = test_fixture("start_ping_check.hex");
    set_sequence(&mut bytes, sequence);
    bytes[7] = ping_id;
    bytes[8..12].copy_from_slice(&oldest_unacked.to_le_bytes());
    bytes
}

pub(super) fn server_ack_many(sequence: u32, acknowledged: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&[0, 0xff, 0xff, 0xff, 0xfb, acknowledged.len() as u8]);
    for id in acknowledged {
        bytes.extend_from_slice(&id.to_le_bytes());
    }
    bytes
}

pub(super) fn client_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    server_ack_many(sequence, &[acknowledged])
}

#[tokio::test]
async fn root_disconnect_skips_remote_logout_wait() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut connection = connect_runtime(login_session(simulator.local_addr().expect("simulator address")), ResponseData::default())
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
    assert_no_datagram(&simulator);
}

#[tokio::test(start_paused = true)]
async fn logout_deadline_reports_unsent() {
    let send = spawn(send_logout_with_deadline(pending::<Result<(), LogoutAttemptError>>(), LogoutUnavailable::Deadline));
    yield_now().await;
    time::advance(LOGOUT_TIMEOUT).await;

    std::assert_matches!(
        send.await.expect("logout deadline task should not panic").expect_err("pending local send should time out"),
        LogoutAttemptError::Unavailable(LogoutUnavailable::Deadline)
    );
}

#[tokio::test]
async fn logout_failure_preserves_unexpected_error() {
    let send = async { Err(LogoutAttemptError::Unexpected(Error::SimulatorSend("socket send failed".into()))) };

    std::assert_matches!(
        send_logout_with_deadline(send, LogoutUnavailable::Deadline).await,
        Err(LogoutAttemptError::Unexpected(Error::SimulatorSend(message))) if message == "socket send failed"
    );
}

#[tokio::test]
async fn logout_transport_failure_is_unexpected() {
    let (completion, response) = oneshot::channel();
    completion.send(Err(Error::SimulatorSend("socket send failed".into()))).expect("deliver transport failure");
    let mut pending = PendingLogout { completion: response, deadline: Instant::now() + LOGOUT_TIMEOUT };

    std::assert_matches!(
        pending.wait().await,
        Err(LogoutAttemptError::Unexpected(Error::SimulatorSend(message))) if message == "socket send failed"
    );
}

#[tokio::test]
async fn disconnect_reports_closed_primary() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut connection = connect_runtime(login_session(simulator.local_addr().expect("simulator address")), ResponseData::default())
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
    assert_no_datagram(&simulator);
}

#[tokio::test(start_paused = true)]
async fn disconnect_logout_respects_deadline() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut connection = connect_runtime(login_session(simulator.local_addr().expect("simulator address")), ResponseData::default())
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
}

#[tokio::test]
async fn failed_root_disconnect_reports_no_send() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut connection = connect_runtime(login_session(simulator.local_addr().expect("simulator address")), ResponseData::default())
        .await
        .expect("connect root simulator circuit");
    receive(&simulator).await;
    connection.root_connection_failed = true;

    assert_eq!(
        connection.disconnect_once(false).await.expect("failed root connection is an unavailable logout"),
        LogoutOutcome::Unavailable(LogoutUnavailable::ConnectionClosed)
    );
    assert_no_datagram(&simulator);
}
