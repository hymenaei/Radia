use std::{net::SocketAddr, time::Duration};

use super::{complete_ping, start_ping};
use crate::{
    circuit::{
        Circuit, Delivery, MAX_PENDING_PACKETS, MAX_RECEIVED_RELIABLE_PACKETS, ReceiveError, advance_sequence, sequence_age,
        sequence_is_newer,
    },
    status::NetworkStatusHandle,
    udp::UdpTransport,
};

fn receive_at(circuit: &mut Circuit, packet: &Packet, received_at: Instant) -> bool {
    circuit.receive_at(packet, received_at).expect("valid circuit packet")
}
use base::UUID;
use protocol::{
    Audible, ChatSourceType, ChatType, Vector3,
    messages::{
        ChatFromViewer, LogoutRequest,
        chat_from_simulator::ChatData,
        chat_from_viewer::{AgentData as ViewerAgentData, ChatData as ViewerChatData},
        logout_request::AgentData as LogoutAgentData,
    },
    udp::{Message, MessageId, Packet},
};
use test_support::{parse_hex_fixture, udp::vectors};
use tokio::{
    net::UdpSocket,
    time::{Instant, timeout},
};

#[tokio::test]
async fn refuses_inbound_message() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let address: SocketAddr = server.local_addr().expect("fake simulator address");
    let transport = UdpTransport::connect(address).await.expect("connect fake simulator");
    let message = Message::ChatFromSimulator(protocol::messages::ChatFromSimulator {
        chat_data: ChatData {
            from_name: "Resident".into(),
            source_id: UUID::nil(),
            owner_id: UUID::nil(),
            source_type: ChatSourceType::Agent,
            chat_type: ChatType::Normal,
            audible: Audible::Fully,
            position: Vector3::default(),
            message: "hello".into(),
        },
    });

    let error = Circuit::new().send(&transport, message, Delivery::Unreliable).await.expect_err("inbound-only message must be rejected");
    assert!(error.contains("simulator-to-viewer"));
    let mut buffer = [0; 64];
    assert!(timeout(Duration::from_millis(20), server.recv_from(&mut buffer)).await.is_err());
}

#[tokio::test]
async fn refuses_unknown_direction() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let datagram = parse_hex_fixture(vectors::COARSE_LOCATION_UPDATE).expect("valid independent fixture");
    let packet = Packet::decode(&datagram).expect("decode schema fallback fixture");

    assert!(matches!(packet.message(), Message::Schema(_)));
    let unknown = Message::Unknown { id: MessageId::high(0xfe), payload: Vec::new() };
    let mut circuit = Circuit::new();
    for message in [packet.message().clone(), unknown] {
        let error = circuit.send(&transport, message, Delivery::Unreliable).await.expect_err("message direction is unknown");
        assert!(error.contains("known viewer-to-simulator direction"));
    }
    let mut buffer = [0; 64];
    assert!(timeout(Duration::from_millis(20), server.recv_from(&mut buffer)).await.is_err());
}

#[tokio::test]
async fn failed_send_keeps_sequence() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let oversized = Message::from(ChatFromViewer {
        agent_data: ViewerAgentData { agent_id: UUID::nil(), session_id: UUID::nil() },
        chat_data: ViewerChatData { message: "x".repeat(9_000), chat_type: ChatType::Normal, channel: 0 },
    });

    assert!(circuit.send(&transport, oversized, Delivery::Unreliable).await.is_err());
    circuit.send(&transport, complete_ping(1), Delivery::Unreliable).await.expect("send valid packet");

    let mut buffer = [0; 64];
    let (size, _) =
        timeout(Duration::from_secs(1), server.recv_from(&mut buffer)).await.expect("read simulator datagram").expect("read datagram");
    let packet = Packet::decode(&buffer[..size]).expect("decode simulator packet");
    assert_eq!(packet.sequence(), 1);
}

#[tokio::test]
async fn logout_bypasses_window() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let status = NetworkStatusHandle::new();
    let mut circuit = Circuit::with_status(status.clone());
    let mut buffer = [0; 1024];

    for sequence in 1..=MAX_PENDING_PACKETS {
        circuit.send(&transport, complete_ping(1), Delivery::Reliable).await.expect("fill reliable window");
        let (size, _) =
            timeout(Duration::from_secs(1), server.recv_from(&mut buffer)).await.expect("read simulator datagram").expect("read datagram");
        let packet = Packet::decode(&buffer[..size]).expect("decode simulator packet");
        assert_eq!(packet.sequence(), sequence as u32);
    }

    let logout_sequence = circuit
        .send(
            &transport,
            Message::LogoutRequest(LogoutRequest { agent_data: LogoutAgentData { agent_id: UUID::nil(), session_id: UUID::nil() } }),
            Delivery::ReliableNoRetry,
        )
        .await
        .expect("send one final logout packet");
    let (size, _) =
        timeout(Duration::from_secs(1), server.recv_from(&mut buffer)).await.expect("read logout request").expect("read datagram");
    let logout = Packet::decode(&buffer[..size]).expect("decode logout request");

    assert_eq!(logout.sequence(), logout_sequence);
    assert!(logout.is_reliable());
    assert!(matches!(logout.message(), Message::LogoutRequest(_)));
    assert!(!circuit.has_pending(logout_sequence));
    assert_eq!(status.snapshot().reliable_packets_pending, MAX_PENDING_PACKETS);
}

#[test]
fn ping_prunes_old_duplicates() {
    let now = Instant::now();
    let mut circuit = Circuit::new();

    assert!(receive_at(&mut circuit, &reliable_packet(100), now));
    assert!(receive_at(&mut circuit, &reliable_packet(101), now));
    assert!(receive_at(&mut circuit, &Packet::new(102, complete_ping(1)), now));

    circuit.clear_duplicate_list(101, now);

    assert!(!receive_at(&mut circuit, &resent_reliable_packet(100), now));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(101), now));

    circuit.clear_duplicate_list(101, now + Duration::from_secs(61));
    assert!(receive_at(&mut circuit, &resent_reliable_packet(100), now + Duration::from_secs(61)));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(101), now + Duration::from_secs(61)));
}

#[test]
fn wrapped_duplicates_expire() {
    let start = Instant::now();
    let mut circuit = Circuit::new();
    let wrapped_sequence = u32::MAX;

    assert!(receive_at(&mut circuit, &reliable_packet(wrapped_sequence), start));
    assert!(receive_at(&mut circuit, &Packet::new(0, complete_ping(1)), start + Duration::from_secs(1)));
    circuit.clear_duplicate_list(0, start + Duration::from_secs(60));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(wrapped_sequence), start + Duration::from_secs(60)));

    circuit.clear_duplicate_list(0, start + Duration::from_secs(61));
    assert!(receive_at(&mut circuit, &resent_reliable_packet(wrapped_sequence), start + Duration::from_secs(61)));
}

#[test]
fn stale_wrapped_id_expires() {
    let start = Instant::now();
    let mut circuit = Circuit::new();
    let previous_cycle_id = u32::MAX;

    assert!(receive_at(&mut circuit, &reliable_packet(10), start));
    assert!(receive_at(&mut circuit, &reliable_packet(previous_cycle_id), start + Duration::from_secs(1)));
    circuit.clear_duplicate_list(1, start + Duration::from_secs(62));

    assert!(receive_at(&mut circuit, &resent_reliable_packet(previous_cycle_id), start + Duration::from_secs(62)));
}

#[test]
fn wrap24_prunes_old_ids() {
    let start = Instant::now();
    let mut circuit = Circuit::new();

    for (sequence, second) in [(0x00ff_fffe, 0), (0x00ff_ffff, 1), (0, 2), (1, 3), (2, 4), (3, 5)] {
        assert!(receive_at(&mut circuit, &reliable_packet(sequence), start + Duration::from_secs(second)));
    }

    circuit.clear_duplicate_list(3, start + Duration::from_secs(61));

    assert!(receive_at(&mut circuit, &resent_reliable_packet(0x00ff_fffe), start + Duration::from_secs(61)));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(3), start + Duration::from_secs(61)));
}

#[test]
fn duplicate_sequence_suppressed() {
    let now = Instant::now();
    let mut circuit = Circuit::new();

    assert!(receive_at(&mut circuit, &reliable_packet(42), now));
    assert!(!receive_at(&mut circuit, &reliable_packet(42), now + Duration::from_secs(1)));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(42), now + Duration::from_secs(2)));
    assert!(receive_at(&mut circuit, &reliable_packet(43), now + Duration::from_secs(3)));
}

#[tokio::test]
async fn bounds_duplicate_history() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let status = NetworkStatusHandle::new();
    let mut circuit = Circuit::with_status(status.clone());
    let received_at = Instant::now();
    let mut buffer = [0; 2048];

    for sequence in 1..=MAX_RECEIVED_RELIABLE_PACKETS as u32 {
        assert!(receive_at(&mut circuit, &reliable_packet(sequence), received_at));
        if sequence % 200 == 0 {
            circuit.flush_pending_acks(&transport).await.expect("flush fake simulator acknowledgements");
            let (size, _) = timeout(Duration::from_secs(1), server.recv_from(&mut buffer))
                .await
                .expect("read fake simulator ack")
                .expect("receive fake simulator ack");
            let ack = Packet::decode(&buffer[..size]).expect("decode fake simulator ack");
            let Message::PacketAck(message) = ack.message() else {
                panic!("expected a packet acknowledgement");
            };
            assert_eq!(message.packets.len(), 200);
        }
    }
    let remaining_acks = MAX_RECEIVED_RELIABLE_PACKETS % 200;
    if remaining_acks != 0 {
        circuit.flush_pending_acks(&transport).await.expect("flush remaining fake simulator acknowledgements");
        let (size, _) = timeout(Duration::from_secs(1), server.recv_from(&mut buffer))
            .await
            .expect("read final fake simulator ack")
            .expect("receive final fake simulator ack");
        let ack = Packet::decode(&buffer[..size]).expect("decode final fake simulator ack");
        assert!(matches!(ack.message(), Message::PacketAck(message) if message.packets.len() == remaining_acks));
    }

    assert_eq!(
        circuit.receive_at(&reliable_packet(MAX_RECEIVED_RELIABLE_PACKETS as u32 + 1), received_at),
        Err(ReceiveError::DuplicateHistoryFull)
    );
    assert!(status.snapshot().last_error.as_deref().is_some_and(|error| error.contains("duplicate history is full")));
    circuit.flush_pending_acks(&transport).await.expect("flush no ack for untracked packet");
    assert!(timeout(Duration::from_millis(20), server.recv_from(&mut buffer)).await.is_err());

    assert!(!receive_at(&mut circuit, &resent_reliable_packet(1), received_at + Duration::from_secs(1)));
    circuit.flush_pending_acks(&transport).await.expect("re-ack known duplicate");
    let (size, _) = timeout(Duration::from_secs(1), server.recv_from(&mut buffer)).await.expect("read re-ack").expect("receive re-ack");
    let ack = Packet::decode(&buffer[..size]).expect("decode re-ack");
    assert!(matches!(ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 1));

    let resumed_at = received_at + Duration::from_secs(61);
    circuit.clear_duplicate_list(MAX_RECEIVED_RELIABLE_PACKETS as u32 + 1, resumed_at);
    assert!(receive_at(&mut circuit, &reliable_packet(MAX_RECEIVED_RELIABLE_PACKETS as u32 + 1), resumed_at));
}

#[tokio::test]
async fn reacks_duplicates() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let mut buffer = [0; 2048];
    let first_time = Instant::now();

    assert!(receive_at(&mut circuit, &reliable_packet(72), first_time));
    circuit.flush_pending_acks(&transport).await.expect("flush first acknowledgement");
    let (size, _) = timeout(Duration::from_secs(1), server.recv_from(&mut buffer))
        .await
        .expect("read first acknowledgement")
        .expect("receive first acknowledgement");
    let first_ack = Packet::decode(&buffer[..size]).expect("decode first acknowledgement");
    assert!(matches!(first_ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 72));

    assert!(!receive_at(&mut circuit, &reliable_packet(72), first_time + Duration::from_secs(1)));
    circuit.flush_pending_acks(&transport).await.expect("flush duplicate acknowledgement");
    let (size, _) = timeout(Duration::from_secs(1), server.recv_from(&mut buffer))
        .await
        .expect("read duplicate acknowledgement")
        .expect("receive duplicate acknowledgement");
    let duplicate_ack = Packet::decode(&buffer[..size]).expect("decode duplicate acknowledgement");
    assert!(matches!(duplicate_ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 72));

    assert!(!receive_at(&mut circuit, &resent_reliable_packet(72), first_time + Duration::from_secs(2)));
    circuit.flush_pending_acks(&transport).await.expect("flush resent duplicate acknowledgement");
    let (size, _) = timeout(Duration::from_secs(1), server.recv_from(&mut buffer))
        .await
        .expect("read resent duplicate acknowledgement")
        .expect("receive resent duplicate acknowledgement");
    let resent_ack = Packet::decode(&buffer[..size]).expect("decode resent duplicate acknowledgement");
    assert!(matches!(resent_ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 72));
}

#[test]
fn sequence_wraps() {
    assert_eq!(advance_sequence(7), 8);
    assert_eq!(advance_sequence(0x00ff_ffff), 0x0100_0000);
    assert_eq!(advance_sequence(u32::MAX - 1), u32::MAX);
    assert_eq!(advance_sequence(u32::MAX), 0);
    assert_eq!(sequence_age(0x0100_0000, 0x00ff_ffff), 1);
    assert!(sequence_is_newer(0x0100_0000, 0x00ff_ffff));
    assert!(!sequence_is_newer(0x00ff_ffff, 0x0100_0000));
    assert_eq!(sequence_age(0, u32::MAX), 1);
    assert!(sequence_is_newer(0, u32::MAX));
    assert!(!sequence_is_newer(u32::MAX, 0));
}

#[test]
fn accepts_wide_sequence() {
    let status = NetworkStatusHandle::new();
    let mut circuit = Circuit::with_status(status.clone());
    let now = Instant::now();

    assert!(circuit.receive_at(&reliable_packet(0x0100_0000), now).expect("valid 32-bit sequence"));
    circuit.clear_duplicate_list(0x0100_0001, now + Duration::from_secs(1));
    assert!(status.snapshot().last_error.is_none());
}

#[tokio::test]
async fn sends_wrapped_sequence() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::with_sequence(0x00ff_fffe);
    let mut buffer = [0; 1024];

    for expected in [0x00ff_fffe_u32, 0x00ff_ffff, 0x0100_0000, 0x0100_0001] {
        circuit.send(&transport, complete_ping(1), Delivery::Unreliable).await.expect("send packet");
        let (size, _) =
            timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read packet").expect("read datagram");
        assert_eq!(&buffer[1..5], &expected.to_be_bytes());
        assert_eq!(Packet::decode(&buffer[..size]).expect("decode packet").sequence(), expected);
    }

    let mut circuit = Circuit::with_sequence(u32::MAX - 1);
    for expected in [u32::MAX - 1, u32::MAX, 0, 1] {
        circuit.send(&transport, complete_ping(1), Delivery::Unreliable).await.expect("send wrapped packet");
        let (size, _) =
            timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read wrapped packet").expect("read datagram");
        assert_eq!(&buffer[1..5], &expected.to_be_bytes());
        assert_eq!(Packet::decode(&buffer[..size]).expect("decode wrapped packet").sequence(), expected);
    }
}

#[tokio::test]
async fn acks_keep_wide_sequence() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let sequence = 0x0100_0000;
    let now = Instant::now();
    let mut buffer = [0; 1024];

    assert!(receive_at(&mut circuit, &reliable_packet(sequence), now));
    circuit.flush_pending_acks(&transport).await.expect("flush acknowledgement");
    let (size, _) =
        timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read acknowledgement").expect("read datagram");
    let ack = Packet::decode(&buffer[..size]).expect("decode acknowledgement");
    assert!(matches!(ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == sequence));

    circuit.clear_duplicate_list(sequence + 1, now + Duration::from_secs(1));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(sequence), now + Duration::from_secs(2)));
}

#[tokio::test]
async fn ping_keeps_wide_watermark() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::with_sequence(0x0100_0000);
    let mut buffer = [0; 1024];

    circuit.send(&transport, complete_ping(1), Delivery::Reliable).await.expect("send wide reliable packet");
    let (size, _) =
        timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read reliable packet").expect("read datagram");
    assert_eq!(Packet::decode(&buffer[..size]).expect("decode reliable packet").sequence(), 0x0100_0000);

    circuit.send_ping(&transport, Instant::now()).await.expect("send circuit ping");
    let (size, _) =
        timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read circuit ping").expect("read datagram");
    let packet = Packet::decode(&buffer[..size]).expect("decode circuit ping");
    assert_eq!(packet.sequence(), 0x0100_0001);
    assert_eq!(packet.message(), &start_ping(0, 0x0100_0000));
}

#[tokio::test]
async fn retry_timeout_tracks_ping() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let mut buffer = [0; 1024];

    assert_eq!(circuit.retry_timeout(), Duration::from_secs(5));
    let sent_at = Instant::now();
    circuit.send_ping(&transport, sent_at).await.expect("send circuit ping");
    let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read ping").expect("read datagram");
    let packet = Packet::decode(&buffer[..size]).expect("decode ping");
    let Message::StartPingCheck(message) = packet.message() else {
        panic!("expected circuit ping");
    };

    assert!(circuit.record_ping_response(message.ping_id.ping_id, sent_at + Duration::from_millis(300)));
    assert_eq!(circuit.retry_timeout(), Duration::from_millis(4300));
    assert!(!circuit.record_ping_response(message.ping_id.ping_id, Instant::now() + Duration::from_secs(1)));
}

#[tokio::test]
async fn lost_ping_ids_expire_and_wrap() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let start = Instant::now();
    let mut buffer = [0; 1024];
    let mut ping_ids = Vec::with_capacity(300);

    for index in 0..300 {
        let sent_at = start + Duration::from_secs(index * 5);
        circuit.send_ping(&transport, sent_at).await.expect("send ping despite lost replies");
        let (size, _) =
            timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read ping").expect("receive ping datagram");
        let packet = Packet::decode(&buffer[..size]).expect("decode ping");
        let Message::StartPingCheck(message) = packet.message() else {
            panic!("expected circuit ping");
        };
        ping_ids.push(message.ping_id.ping_id);
    }

    assert_eq!(ping_ids[0], 0);
    assert_eq!(ping_ids[256], 0, "expired IDs become reusable after the u8 range wraps");
}

async fn receive_ping_id(simulator: &UdpSocket, buffer: &mut [u8]) -> u8 {
    let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(buffer)).await.expect("read ping").expect("receive ping datagram");
    let packet = Packet::decode(&buffer[..size]).expect("decode ping");
    let Message::StartPingCheck(message) = packet.message() else {
        panic!("expected circuit ping");
    };
    message.ping_id.ping_id
}

#[tokio::test]
async fn ping_id_quarantine_expires_at_boundary() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let start = Instant::now();
    let mut buffer = [0; 1024];

    circuit.send_ping(&transport, start).await.expect("send first ping");
    let first_id = receive_ping_id(&simulator, &mut buffer).await;
    assert_eq!(first_id, 0);
    let quarantine_started = start + Duration::from_secs(1);
    assert!(circuit.record_ping_response(first_id, quarantine_started));

    for expected_id in 1..=u8::MAX {
        circuit.send_ping(&transport, quarantine_started).await.expect("send ping");
        assert_eq!(receive_ping_id(&simulator, &mut buffer).await, expected_id);
        assert!(circuit.record_ping_response(expected_id, quarantine_started));
    }

    let just_before_expiry = quarantine_started + Duration::from_secs(30) - Duration::from_nanos(1);
    circuit.send_ping(&transport, just_before_expiry).await.expect("skip while all IDs are quarantined");
    assert!(timeout(Duration::from_millis(20), simulator.recv_from(&mut buffer)).await.is_err());

    let at_expiry = quarantine_started + Duration::from_secs(30);
    circuit.send_ping(&transport, at_expiry).await.expect("send when IDs expire");
    assert_eq!(receive_ping_id(&simulator, &mut buffer).await, first_id);
}

#[tokio::test]
async fn late_and_duplicate_ping_replies_do_not_change_rtt() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let start = Instant::now();
    let mut buffer = [0; 1024];

    circuit.send_ping(&transport, start).await.expect("send first ping");
    let (size, _) =
        timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read first ping").expect("receive ping");
    let first = Packet::decode(&buffer[..size]).expect("decode first ping");
    let Message::StartPingCheck(first) = first.message() else {
        panic!("expected first circuit ping");
    };
    let first_id = first.ping_id.ping_id;

    assert!(!circuit.record_ping_response(first_id, start + Duration::from_secs(31)));
    assert_eq!(circuit.retry_timeout(), Duration::from_secs(5));
    assert!(!circuit.record_ping_response(first_id, start + Duration::from_secs(32)), "duplicate late reply is ignored");
    assert_eq!(circuit.retry_timeout(), Duration::from_secs(5));

    let next_sent_at = start + Duration::from_secs(32);
    circuit.send_ping(&transport, next_sent_at).await.expect("send after expiry");
    let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read next ping").expect("receive ping");
    let next = Packet::decode(&buffer[..size]).expect("decode next ping");
    let Message::StartPingCheck(next) = next.message() else {
        panic!("expected next circuit ping");
    };
    let next_id = next.ping_id.ping_id;
    assert_ne!(next_id, first_id, "recently expired IDs stay quarantined");

    assert!(circuit.record_ping_response(next_id, next_sent_at + Duration::from_millis(300)));
    assert_eq!(circuit.retry_timeout(), Duration::from_millis(4300));
    assert!(!circuit.record_ping_response(next_id, next_sent_at + Duration::from_secs(1)), "duplicate reply is ignored");
    assert_eq!(circuit.retry_timeout(), Duration::from_millis(4300));
}

#[tokio::test]
async fn retry_timeout_caps() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let mut buffer = [0; 1024];

    let sent_at = Instant::now();
    circuit.send_ping(&transport, sent_at).await.expect("send circuit ping");
    let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read ping").expect("read datagram");
    let packet = Packet::decode(&buffer[..size]).expect("decode ping");
    let Message::StartPingCheck(message) = packet.message() else {
        panic!("expected circuit ping");
    };

    assert!(circuit.record_ping_response(message.ping_id.ping_id, sent_at + Duration::from_secs(20)));
    assert_eq!(circuit.retry_timeout(), Duration::from_secs(10));
}

#[tokio::test]
async fn ping_uses_oldest_unacked() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let mut buffer = [0; 1024];

    circuit.send(&transport, complete_ping(1), Delivery::Reliable).await.expect("send reliable packet");
    circuit.send(&transport, complete_ping(2), Delivery::Reliable).await.expect("send second reliable packet");
    for _ in 0..2 {
        let (size, _) =
            timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read packet").expect("read datagram");
        Packet::decode(&buffer[..size]).expect("decode reliable packet");
    }

    circuit.send_ping(&transport, Instant::now()).await.expect("send circuit ping");
    let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read ping").expect("read datagram");
    let mut expected = parse_hex_fixture(vectors::START_PING_CHECK).expect("valid ping fixture");
    expected[1..5].copy_from_slice(&3u32.to_be_bytes());
    expected[7] = 0;
    expected[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(&buffer[..size], expected);
    let ping = Packet::decode(&buffer[..size]).expect("decode circuit ping");

    assert!(!ping.is_reliable());
    assert_eq!(ping.message(), &start_ping(0, 1));
}

fn reliable_packet(sequence: u32) -> Packet {
    Packet::new(sequence, complete_ping(1)).with_reliable()
}

fn resent_reliable_packet(sequence: u32) -> Packet {
    reliable_packet(sequence).with_resent()
}
