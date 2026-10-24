use std::{net::SocketAddr, time::Duration};

use protocol::lludp::{Message, Packet};
use tokio::{net::UdpSocket, time::timeout};

pub(crate) use crate::session::{
    HandoffEvidence, HandoffState, PendingPromotion, connect_test_connection, connect_test_primary_with_state, handoff_cancellation,
    matches_handoff_cancellation, run_test_session as run_session,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(3);

pub(crate) fn login_xml(simulator_port: u16, seed_url: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\"?><methodResponse><params><param><value><struct>",
            "<member><name>login</name><value><boolean>1</boolean></value></member>",
            "<member><name>first_name</name><value><string>Session</string></value></member>",
            "<member><name>last_name</name><value><string>Avatar</string></value></member>",
            "<member><name>message</name><value><string>Welcome</string></value></member>",
            "<member><name>sim_ip</name><value><string>127.0.0.1</string></value></member>",
            "<member><name>sim_port</name><value><int>{simulator_port}</int></value></member>",
            "<member><name>circuit_code</name><value><int>287454020</int></value></member>",
            "<member><name>agent_id</name><value><string>ffeeddcc-bbaa-9988-7766-554433221100</string></value></member>",
            "<member><name>session_id</name><value><string>00112233-4455-6677-8899-aabbccddeeff</string></value></member>",
            "<member><name>seed_capability</name><value><string>{seed_url}</string></value></member>",
            "<member><name>mfa_hash</name><value><string>returned-mfa</string></value></member>",
            "<member><name>inventory-skeleton</name><value><array><data><value><struct>",
            "<member><name>folder_id</name><value><string>12345678-1234-5678-1234-567812345678</string></value></member>",
            "<member><name>name</name><value><string>Favorites</string></value></member>",
            "</struct></value></data></array></value></member>",
            "</struct></value></param></params></methodResponse>"
        ),
        simulator_port = simulator_port,
        seed_url = seed_url,
    )
}

pub(crate) async fn receive_udp(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buffer = [0; 65_535];
    let (size, peer) = timeout(TEST_TIMEOUT, socket.recv_from(&mut buffer))
        .await
        .expect("fake simulator should receive a datagram")
        .expect("read simulator datagram");
    (buffer[..size].to_vec(), peer)
}

pub(crate) async fn receive_client_message(
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
        if matches!(packet.message(), Message::AgentThrottle(_) | Message::RegionHandshakeReply(_)) {
            assert!(packet.is_reliable());
            socket.send_to(&server_ack(u32::MAX, packet.sequence()), expected_peer).await.expect("ack client control message");
            continue;
        }
        panic!("unexpected client LLUDP message: {:?}", packet.message());
    }
}

pub(crate) async fn acknowledge_initial_movement(simulator: &UdpSocket) -> (Vec<u8>, SocketAddr, Vec<u8>, Packet) {
    let (use_circuit, peer) = receive_udp(simulator).await;
    simulator.send_to(&server_ack(20, 1), peer).await.expect("ack UseCircuitCode");
    let (movement, movement_packet) =
        receive_client_message(simulator, peer, |message| matches!(message, Message::CompleteAgentMovement(_))).await;
    simulator.send_to(&server_ack(21, movement_packet.sequence()), peer).await.expect("ack CompleteAgentMovement");
    (use_circuit, peer, movement, movement_packet)
}

pub(crate) fn server_ack(sequence: u32, acknowledged: u32) -> Vec<u8> {
    let mut bytes = vec![0];
    bytes.extend_from_slice(&sequence.to_be_bytes());
    bytes.extend_from_slice(&[0, 0xff, 0xff, 0xff, 0xfb, 1]);
    bytes.extend_from_slice(&acknowledged.to_le_bytes());
    bytes
}

pub(crate) fn test_fixture(source: &str) -> Vec<u8> {
    test_support::lludp::fixture(source)
}

pub(crate) fn set_sequence(bytes: &mut [u8], sequence: u32) {
    bytes[1..5].copy_from_slice(&sequence.to_be_bytes());
}
