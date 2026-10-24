use std::{collections::HashMap, io::ErrorKind, net::SocketAddr, time::Duration};

use math::Vec3;
use protocol::{
    Audible, ChatSourceType, ChatType,
    lludp::{Message, MessageId, Packet},
    messages::{
        CachedObjectReference, ChatFromSimulator, ChatFromViewer, LogoutRequest, ObjectUpdateCached, RegionData, RequestMultipleObjects,
        agent::logout_request,
        chat::chat_from_simulator::ChatData,
        chat::chat_from_viewer,
        object::request_multiple_objects::{CacheMissType, ObjectRequest, RequestAgentData},
    },
};
use test_support::{lludp::vectors, parse_hex_fixture};
use tokio::{net::UdpSocket, time::Instant};
use uuid::Uuid;

use crate::{
    circuit::{
        UdpTransport,
        lludp::{
            CircuitError, DUPLICATE_ACK_HEADROOM, Delivery, KEEPALIVE_TIMEOUT, MAX_NORMAL_DATAGRAM_BYTES, MAX_ORDERED_SEQUENCE_DISTANCE,
            MAX_PENDING_ACKS, MAX_PENDING_PACKETS, MAX_RECEIVED_RELIABLE_PACKETS, PendingPacket, ReceiveAdmission, ReceiveError,
            advance_sequence, remove_sent_acks, sequence_age,
        },
    },
    event::PacketOrder::{Ambiguous, Degraded, Ordered},
    fixtures::circuit::{
        complete_ping, connected_simulator, receive_at, receive_packet, receive_packet_at, reliable_packet, resent_reliable_packet,
        send_ping_at, start_ping, with_sequence,
    },
};

use super::{Circuit, available_ping_id};

fn assert_no_datagram(socket: &UdpSocket) {
    let mut buffer = [0; 65_535];
    std::assert_matches!(socket.try_recv_from(&mut buffer), Err(error) if error.kind() == ErrorKind::WouldBlock);
}

async fn receive_ping_id(simulator: &UdpSocket, buffer: &mut [u8]) -> u8 {
    let (size, _) = simulator.recv_from(buffer).await.expect("receive ping datagram");
    let packet = Packet::decode(&buffer[..size]).expect("decode ping");
    let Message::StartPingCheck(message) = packet.message() else {
        panic!("expected circuit ping");
    };
    message.ping_id.ping_id
}

#[test]
fn ping_id_selection() {
    let now = Instant::now();
    let pending = HashMap::from([(u8::MAX, now), (0, now), (1, now)]);
    assert_eq!(available_ping_id(u8::MAX, &pending, &HashMap::new()), Some(2));

    let all_pending = (0..=u8::MAX).map(|id| (id, now)).collect::<HashMap<_, _>>();
    assert_eq!(available_ping_id(0, &all_pending, &HashMap::new()), None);
}

#[tokio::test]
async fn skips_ping_when_ids_full() {
    let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let mut circuit = Circuit::new();
    let now = Instant::now();
    circuit.pending_pings.extend((0..=u8::MAX).map(|ping_id| (ping_id, now)));
    let next_sequence = circuit.next_sequence;

    assert!(circuit.prepare_ping(now).expect("prepare ping").is_none());

    assert_eq!(circuit.pending_pings.len(), 256);
    assert_eq!(circuit.next_sequence, next_sequence);
    assert_no_datagram(&simulator);
}

#[tokio::test]
async fn prepared_send_commits_after_local_send() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let sent_at = Instant::now();
    let prepared = circuit.prepare_message(complete_ping(8), Delivery::Reliable).expect("prepare reliable packet");

    assert_eq!(prepared.sequence(), 1);
    assert_eq!(circuit.next_sequence, 1);
    assert!(circuit.pending.is_empty());

    crate::fixtures::circuit::send_prepared_at(&mut circuit, &transport, prepared, sent_at).await.expect("commit successful local send");
    let mut bytes = [0; 1024];
    let (_, packet) = receive_packet(&simulator, &mut bytes).await;
    assert_eq!(packet.sequence(), 1);
    assert!(packet.is_reliable());
    assert_eq!(packet.message(), &complete_ping(8));

    assert_eq!(circuit.next_sequence, 2);
    assert_eq!(circuit.pending.get(&1).expect("track sent packet").sent_at, sent_at);
}

#[tokio::test]
async fn prepared_send_removes_only_carried_acknowledgements() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let now = Instant::now();
    receive_at(&mut circuit, &reliable_packet(41), now);
    let prepared = circuit.prepare_message(complete_ping(8), Delivery::Unreliable).expect("prepare packet");
    let packet = Packet::decode(prepared.bytes()).expect("decode prepared packet");
    assert_eq!(packet.appended_acks(), &[41]);

    receive_at(&mut circuit, &reliable_packet(42), now + Duration::from_millis(1));
    crate::fixtures::circuit::send_prepared_at(&mut circuit, &transport, prepared, now + Duration::from_millis(2))
        .await
        .expect("commit successful local send");
    let mut bytes = [0; 1024];
    simulator.recv_from(&mut bytes).await.expect("receive sent packet");

    assert_eq!(circuit.pending_acks.iter().copied().collect::<Vec<_>>(), [42]);
    assert_eq!(circuit.pending_ack_sequences, [42].into_iter().collect());
}

#[tokio::test]
async fn appended_ack_batches_respect_normal_datagram_limit() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let now = Instant::now();
    for sequence in 1..=u32::from(u8::MAX) {
        let admission = circuit.prepare_receive(sequence, true, [], now).expect("reserve reliable packet");
        circuit.commit_receive(admission);
    }

    let message = Message::ChatFromViewer(ChatFromViewer {
        agent_data: chat_from_viewer::AgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        chat_data: chat_from_viewer::ChatData { message: "x".repeat(255), chat_type: ChatType::Normal, channel: 0 },
    });
    let prepared = circuit.prepare_message(message.clone(), Delivery::Unreliable).expect("prepare limited ACK batch");
    assert!(prepared.bytes().len() <= MAX_NORMAL_DATAGRAM_BYTES);
    let packet = Packet::decode(prepared.bytes()).expect("decode limited datagram");
    assert!(!packet.appended_acks().is_empty());
    assert!(packet.appended_acks().len() < usize::from(u8::MAX));
    let acknowledged = packet.appended_acks().len();
    crate::fixtures::circuit::send_prepared_at(&mut circuit, &transport, prepared, now).await.expect("commit limited ACK batch");
    let mut bytes = [0; 2048];
    let (_, sent) = receive_packet(&simulator, &mut bytes).await;
    assert!(sent.appended_acks().len() == acknowledged);
    assert_eq!(circuit.pending_acks.len(), usize::from(u8::MAX) - acknowledged);

    let next = circuit.prepare_message(message, Delivery::Unreliable).expect("prepare remaining ACKs");
    assert!(next.bytes().len() <= MAX_NORMAL_DATAGRAM_BYTES);
    let expected = usize::from(u8::MAX) - acknowledged;
    assert_eq!(Packet::decode(next.bytes()).expect("decode remaining batch").appended_acks().len(), expected);
    crate::fixtures::circuit::send_prepared_at(&mut circuit, &transport, next, now + Duration::from_millis(1))
        .await
        .expect("send remaining ACK batch");
    let (_, sent) = receive_packet(&simulator, &mut bytes).await;
    assert_eq!(sent.appended_acks().len(), expected);
    assert!(circuit.pending_acks.is_empty());
}

#[tokio::test]
async fn oversized_base_message_leaves_acks_for_standalone_batch() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let now = Instant::now();
    for sequence in 1..=u32::from(u8::MAX) {
        let admission = circuit.prepare_receive(sequence, true, [], now).expect("reserve reliable packet");
        circuit.commit_receive(admission);
    }
    let message = Message::RequestMultipleObjects(RequestMultipleObjects {
        agent_data: RequestAgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        objects: (0..u8::MAX).map(|_| ObjectRequest { cache_miss_type: CacheMissType::Full, local_id: 0x0102_0304 }).collect(),
    });
    let prepared = circuit.prepare_message(message, Delivery::Unreliable).expect("prepare compatibility-sized request");
    assert!(prepared.bytes().len() > MAX_NORMAL_DATAGRAM_BYTES);
    assert!(Packet::decode(prepared.bytes()).expect("decode oversized base message").appended_acks().is_empty());
    crate::fixtures::circuit::send_prepared_at(&mut circuit, &transport, prepared, now).await.expect("commit base message without ACKs");
    let mut bytes = [0; 65_535];
    let (size, _) = simulator.recv_from(&mut bytes).await.expect("receive oversized base message");
    assert!(bytes[..size].len() > MAX_NORMAL_DATAGRAM_BYTES);
    assert_eq!(circuit.pending_acks.len(), usize::from(u8::MAX));

    let acknowledgements = circuit.prepare_acknowledgements().expect("prepare standalone ACKs").expect("ACKs remain pending");
    assert!(acknowledgements.bytes().len() <= MAX_NORMAL_DATAGRAM_BYTES);
    assert_eq!(Packet::decode(acknowledgements.bytes()).expect("decode standalone ACKs").message().name(), "PacketAck");
    crate::fixtures::circuit::send_prepared_at(&mut circuit, &transport, acknowledgements, now + Duration::from_millis(1))
        .await
        .expect("send standalone acknowledgement batch");
    let (size, _) = simulator.recv_from(&mut bytes).await.expect("receive standalone acknowledgement batch");
    assert_eq!(Packet::decode(&bytes[..size]).expect("decode standalone acknowledgements").message().name(), "PacketAck");
    assert!(circuit.pending_acks.is_empty());
}

#[test]
fn dropped_prepared_ack_keeps_circuit_state() {
    let mut circuit = Circuit::new();
    receive_at(&mut circuit, &reliable_packet(41), Instant::now());
    let prepared = circuit.prepare_acknowledgements().expect("prepare ACK").expect("ACK is pending");
    assert_eq!(prepared.sequence(), 1);

    drop(prepared);

    assert_eq!(circuit.next_sequence, 1);
    assert_eq!(circuit.pending_acks.iter().copied().collect::<Vec<_>>(), [41]);
    assert_eq!(circuit.pending_ack_sequences, [41].into_iter().collect());
}

#[tokio::test]
async fn acknowledged_packet_is_not_restored_by_late_retry_commit() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let sent_at = Instant::now() - Duration::from_secs(6);
    let prepared = circuit.prepare_message(complete_ping(8), Delivery::Reliable).expect("prepare reliable packet");
    crate::fixtures::circuit::send_prepared_at(&mut circuit, &transport, prepared, sent_at).await.expect("send original reliable packet");
    let mut bytes = [0; 1024];
    simulator.recv_from(&mut bytes).await.expect("receive original packet");

    let retry_at = sent_at + Duration::from_secs(6);
    let retry = circuit.prepare_retransmission(retry_at).expect("prepare due retry").expect("retry is due");
    transport.try_send(retry.bytes()).expect("send prepared retry");
    let (size, _) = simulator.recv_from(&mut bytes).await.expect("receive sent retry");
    assert!(Packet::decode(&bytes[..size]).expect("decode resent packet").is_resent());
    receive_packet_at(&mut circuit, &reliable_packet(2).with_appended_acks(vec![1]), retry_at)
        .expect("receive acknowledgement before local retry commit");
    circuit.commit_sent(retry, retry_at).expect("commit retry after ACK");

    assert!(!circuit.pending.contains_key(&1));
}

#[tokio::test]
async fn refuses_inbound_message() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let address: SocketAddr = server.local_addr().expect("fake simulator address");
    let transport = UdpTransport::connect(address).await.expect("connect fake simulator");
    let message = Message::ChatFromSimulator(ChatFromSimulator {
        chat_data: ChatData {
            from_name: "Resident".into(),
            source_id: Uuid::nil(),
            owner_id: Uuid::nil(),
            source_type: ChatSourceType::Agent,
            chat_type: ChatType::Normal,
            audible: Audible::Fully,
            position: Vec3::default(),
            message: "hello".into(),
        },
    });

    let error = crate::fixtures::circuit::send_message(&mut Circuit::new(), &transport, message, Delivery::Unreliable)
        .await
        .expect_err("inbound-only message must be rejected");
    std::assert_matches!(error, CircuitError::InboundOnly);
    assert_no_datagram(&server);
}

#[test]
fn rejects_every_typed_inbound_message_before_encoding() {
    for (name, fixture) in [
        ("region_handshake.hex", vectors::REGION_HANDSHAKE),
        ("agent_movement_complete.hex", vectors::AGENT_MOVEMENT_COMPLETE),
        ("logout_reply.hex", vectors::LOGOUT_REPLY),
        ("chat_from_simulator.hex", vectors::CHAT_FROM_SIMULATOR),
        ("region_id_and_handle_reply.hex", vectors::REGION_ID_AND_HANDLE_REPLY),
        ("map_block_reply.hex", vectors::MAP_BLOCK_REPLY),
        ("enable_simulator.hex", vectors::ENABLE_SIMULATOR),
        ("disable_simulator.hex", vectors::DISABLE_SIMULATOR),
        ("confirm_enable_simulator.hex", vectors::CONFIRM_ENABLE_SIMULATOR),
        ("object_update.hex", vectors::OBJECT_UPDATE),
        ("object_update_compressed_typed.hex", vectors::OBJECT_UPDATE_COMPRESSED_TYPED),
        ("improved_terse_object_update.hex", vectors::IMPROVED_TERSE_OBJECT_UPDATE),
        ("object_update_cached.hex", vectors::OBJECT_UPDATE_CACHED),
        ("kill_object.hex", vectors::KILL_OBJECT),
        ("object_extra_params.hex", vectors::OBJECT_EXTRA_PARAMS),
    ] {
        let bytes = parse_hex_fixture(fixture).unwrap_or_else(|error| panic!("{name}: {error}"));
        let packet = Packet::decode(&bytes).unwrap_or_else(|error| panic!("{name}: {error}"));
        let message = packet.into_message();

        assert!(message.is_inbound_only(), "{name} must be classified as inbound-only");
        assert!(
            Circuit::new().prepare_message(message, Delivery::Unreliable).is_err_and(|error| matches!(error, CircuitError::InboundOnly)),
            "{name} must be rejected by the Circuit send path"
        );
    }
}

#[tokio::test]
async fn refuses_unknown_direction() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let datagram = parse_hex_fixture(vectors::COARSE_LOCATION_UPDATE).expect("valid independent fixture");
    let packet = Packet::decode(&datagram).expect("decode schema fallback fixture");

    std::assert_matches!(packet.message(), Message::Schema(_));
    let unknown = Message::Unknown { id: MessageId::high(0xfe), payload: Vec::new() };
    let mut circuit = Circuit::new();
    for message in [packet.message().clone(), unknown] {
        let error = crate::fixtures::circuit::send_message(&mut circuit, &transport, message, Delivery::Unreliable)
            .await
            .expect_err("message direction is unknown");
        std::assert_matches!(error, CircuitError::UnknownDirection);
    }
    assert_no_datagram(&server);
}

#[tokio::test]
async fn failed_send_keeps_sequence() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let oversized = Message::from(ChatFromViewer {
        agent_data: chat_from_viewer::AgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() },
        chat_data: chat_from_viewer::ChatData { message: "x".repeat(9_000), chat_type: ChatType::Normal, channel: 0 },
    });

    assert!(crate::fixtures::circuit::send_message(&mut circuit, &transport, oversized, Delivery::Unreliable).await.is_err());
    crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(1), Delivery::Unreliable)
        .await
        .expect("send valid packet");

    let mut buffer = [0; 64];
    let (_, packet) = receive_packet(&server, &mut buffer).await;
    assert_eq!(packet.sequence(), 1);
}

#[tokio::test]
async fn logout_bypasses_window() {
    let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
    let transport = UdpTransport::connect(server.local_addr().expect("simulator address")).await.expect("connect fake simulator");
    let mut circuit = Circuit::new();
    let mut buffer = [0; 1024];

    for sequence in 1..=MAX_PENDING_PACKETS {
        crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(1), Delivery::Reliable)
            .await
            .expect("fill reliable window");
        let (_, packet) = receive_packet(&server, &mut buffer).await;
        assert_eq!(packet.sequence(), sequence as u32);
    }

    let logout_sequence = crate::fixtures::circuit::send_message(
        &mut circuit,
        &transport,
        Message::LogoutRequest(LogoutRequest { agent_data: logout_request::AgentData { agent_id: Uuid::nil(), session_id: Uuid::nil() } }),
        Delivery::ReliableNoRetry,
    )
    .await
    .expect("send one final logout packet");
    let (_, logout) = receive_packet(&server, &mut buffer).await;

    assert_eq!(logout.sequence(), logout_sequence);
    assert!(logout.is_reliable());
    std::assert_matches!(logout.message(), Message::LogoutRequest(_));
    assert!(!circuit.has_pending(logout_sequence));
}

#[test]
fn ping_prunes_old_duplicates() {
    let now = Instant::now();
    let mut circuit = Circuit::new();

    assert!(receive_at(&mut circuit, &reliable_packet(100), now));
    assert!(receive_at(&mut circuit, &reliable_packet(101), now));
    assert!(receive_at(&mut circuit, &Packet::new(102, complete_ping(1)), now));
    remove_sent_acks(&mut circuit.pending_acks, &mut circuit.pending_ack_sequences, &[100, 101]);

    circuit.clear_duplicate_list(101, now);

    assert!(!receive_at(&mut circuit, &resent_reliable_packet(100), now));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(101), now));
    remove_sent_acks(&mut circuit.pending_acks, &mut circuit.pending_ack_sequences, &[100, 101]);

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
    remove_sent_acks(&mut circuit.pending_acks, &mut circuit.pending_ack_sequences, &[wrapped_sequence]);
    circuit.clear_duplicate_list(0, start + Duration::from_secs(60));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(wrapped_sequence), start + Duration::from_secs(60)));
    remove_sent_acks(&mut circuit.pending_acks, &mut circuit.pending_ack_sequences, &[wrapped_sequence]);

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
    remove_sent_acks(&mut circuit.pending_acks, &mut circuit.pending_ack_sequences, &[10, previous_cycle_id]);
    circuit.clear_duplicate_list(1, start + Duration::from_secs(62));

    assert!(receive_at(&mut circuit, &resent_reliable_packet(previous_cycle_id), start + Duration::from_secs(62)));
}

#[test]
fn wrap32_prunes_old_ids() {
    let start = Instant::now();
    let mut circuit = Circuit::new();

    for (sequence, second) in [(u32::MAX - 1, 0), (u32::MAX, 1), (0, 2), (1, 3), (2, 4), (3, 5)] {
        assert!(receive_at(&mut circuit, &reliable_packet(sequence), start + Duration::from_secs(second)));
    }
    remove_sent_acks(&mut circuit.pending_acks, &mut circuit.pending_ack_sequences, &[u32::MAX - 1, u32::MAX, 0, 1, 2, 3]);

    circuit.clear_duplicate_list(3, start + Duration::from_secs(61));

    assert!(receive_at(&mut circuit, &resent_reliable_packet(u32::MAX - 1), start + Duration::from_secs(61)));
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

#[test]
fn outbound_acks_precede_payload_admission() {
    let now = Instant::now();
    let mut circuit = Circuit::new();
    circuit.pending.insert(7, PendingPacket { bytes: Vec::new(), sent_at: now, retries: 0 });

    let _admission = circuit.prepare_receive(42, true, [7], now).expect("reserve acknowledgement and duplicate-history capacity");
    assert!(!circuit.pending.contains_key(&7), "appended ACK evidence is independent of payload admission");
    assert_eq!(circuit.highest_received_sequence, None, "unadmitted payload must not advance receive ordering");
    assert!(circuit.ordering_anchor.is_none(), "unadmitted payload must not establish an ordering anchor");
    assert!(!circuit.received_sequences.contains_key(&42));
    assert!(!circuit.pending_ack_sequences.contains(&42));
    let admission = circuit.prepare_receive(42, true, [], now).expect("reserve retransmitted reliable packet");
    let outcome = circuit.commit_receive(admission);
    assert!(outcome.is_new_packet);
    assert_eq!(outcome.packet_order, Ordered(42));
    assert!(circuit.received_sequences.contains_key(&42));
    assert!(circuit.pending_ack_sequences.contains(&42));

    let duplicate = circuit.prepare_receive(42, true, [], now).expect("reserve duplicate re-ack");
    let ReceiveAdmission::Duplicate(outcome) = duplicate else { panic!("already-admitted packet is a duplicate") };
    assert!(!outcome.is_new_packet);
    assert_eq!(circuit.received_sequences.len(), 1, "duplicates do not add history or redeliver");
}

#[test]
fn pending_ack_preserves_duplicate_history() {
    let now = Instant::now();
    let mut circuit = Circuit::new();

    assert!(receive_at(&mut circuit, &reliable_packet(100), now));
    assert!(receive_at(&mut circuit, &Packet::new(101, complete_ping(1)), now));
    circuit.clear_duplicate_list(101, now + Duration::from_secs(61));

    assert!(circuit.received_sequences.contains_key(&100), "an unsent ACK must keep its accepted payload in duplicate history");
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(100), now + Duration::from_secs(61)));

    remove_sent_acks(&mut circuit.pending_acks, &mut circuit.pending_ack_sequences, &[100]);
    circuit.clear_duplicate_list(101, now + Duration::from_secs(62));

    assert!(!circuit.received_sequences.contains_key(&100), "history can expire after its ACK has been sent");
}

#[test]
fn full_ack_queue_rejects_new_reliable_packet() {
    let now = Instant::now();
    let mut circuit = Circuit::new();
    let accepted = (MAX_PENDING_ACKS - DUPLICATE_ACK_HEADROOM) as u32;

    for sequence in 1..=accepted {
        let admission = circuit.prepare_receive(sequence, true, [], now).expect("reserve reliable packet");
        assert!(circuit.commit_receive(admission).is_new_packet);
    }

    let rejected = accepted + 1;
    let highest_before = circuit.highest_received_sequence;
    assert_eq!(circuit.prepare_receive(rejected, true, [], now), Err(ReceiveError::AcknowledgementCapacityFull));
    assert_eq!(circuit.highest_received_sequence, highest_before, "rejected packet must not advance receive ordering");
    assert!(!circuit.received_sequences.contains_key(&rejected), "rejected packet must not enter duplicate history");
    assert!(!circuit.pending_ack_sequences.contains(&rejected), "rejected packet must not enter the ACK queue");
}

#[test]
fn prepared_receive_does_not_commit_sequence_ordering() {
    let now = Instant::now();
    let mut circuit = Circuit::new();

    let _reservation = circuit.prepare_receive(u32::MAX, true, [], now).expect("reserve reliable packet admission");
    assert!(circuit.ordering_anchor.is_none());
    assert_eq!(circuit.highest_received_sequence, None);

    assert_eq!(circuit.observe_sequence(10), Ordered(10), "a dropped reservation must not shift later packet order");
}

#[tokio::test]
async fn bounds_duplicate_history() {
    let (server, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let received_at = Instant::now();
    let mut buffer = [0; 2048];

    for sequence in 1..=MAX_RECEIVED_RELIABLE_PACKETS as u32 {
        assert!(receive_at(&mut circuit, &reliable_packet(sequence), received_at));
        if sequence % 200 == 0 {
            crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush fake simulator acknowledgements");
            let (_, ack) = receive_packet(&server, &mut buffer).await;
            let Message::PacketAck(message) = ack.message() else {
                panic!("expected a packet acknowledgement");
            };
            assert_eq!(message.packets.len(), 200);
        }
    }
    let remaining_acks = MAX_RECEIVED_RELIABLE_PACKETS % 200;
    if remaining_acks != 0 {
        crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport)
            .await
            .expect("flush remaining fake simulator acknowledgements");
        let (_, ack) = receive_packet(&server, &mut buffer).await;
        std::assert_matches!(ack.message(), Message::PacketAck(message) if message.packets.len() == remaining_acks);
    }

    assert_eq!(
        receive_packet_at(&mut circuit, &reliable_packet(MAX_RECEIVED_RELIABLE_PACKETS as u32 + 1), received_at),
        Err(ReceiveError::DuplicateHistoryFull)
    );
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush no ack for untracked packet");
    assert_no_datagram(&server);

    assert!(!receive_at(&mut circuit, &resent_reliable_packet(1), received_at + Duration::from_secs(1)));
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("re-ack known duplicate");
    let (_, ack) = receive_packet(&server, &mut buffer).await;
    std::assert_matches!(ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 1);

    let resumed_at = received_at + Duration::from_secs(61);
    let ping_sequence = MAX_RECEIVED_RELIABLE_PACKETS as u32 + 1;
    let ping = Packet::new(ping_sequence, start_ping(1, ping_sequence));
    assert!(receive_at(&mut circuit, &ping, resumed_at));
    circuit.clear_duplicate_list(ping_sequence, resumed_at);
    assert!(receive_at(&mut circuit, &reliable_packet(ping_sequence + 1), resumed_at));
}

#[tokio::test]
async fn reacks_duplicates() {
    let (server, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let mut buffer = [0; 2048];
    let first_time = Instant::now();

    assert!(receive_at(&mut circuit, &reliable_packet(72), first_time));
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush first acknowledgement");
    let (_, first_ack) = receive_packet(&server, &mut buffer).await;
    std::assert_matches!(first_ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 72);

    assert!(!receive_at(&mut circuit, &reliable_packet(72), first_time + Duration::from_secs(1)));
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush duplicate acknowledgement");
    let (_, duplicate_ack) = receive_packet(&server, &mut buffer).await;
    std::assert_matches!(duplicate_ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 72);

    assert!(!receive_at(&mut circuit, &resent_reliable_packet(72), first_time + Duration::from_secs(2)));
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush resent duplicate acknowledgement");
    let (_, resent_ack) = receive_packet(&server, &mut buffer).await;
    std::assert_matches!(resent_ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 72);
}

#[test]
fn sequence_wraps() {
    assert_eq!(advance_sequence(7), 8);
    assert_eq!(advance_sequence(0x00ff_ffff), 0x0100_0000);
    assert_eq!(advance_sequence(u32::MAX - 1), u32::MAX);
    assert_eq!(advance_sequence(u32::MAX), 0);
    assert_eq!(sequence_age(0x0100_0000, 0x00ff_ffff), 1);
    assert_eq!(sequence_age(0, u32::MAX), 1);
}

#[test]
fn accepts_wide_sequence() {
    let mut circuit = Circuit::new();
    let now = Instant::now();

    assert!(receive_packet_at(&mut circuit, &reliable_packet(0x0100_0000), now).expect("valid 32-bit sequence").is_new_packet);
    circuit.clear_duplicate_list(0x0100_0001, now + Duration::from_secs(1));
}

#[test]
fn source_packet_order_tracks_u32_wrap_and_recent_reordering() {
    let mut circuit = Circuit::new();
    for (sequence, order) in
        [(u32::MAX - 1, u64::from(u32::MAX - 1)), (u32::MAX, u64::from(u32::MAX)), (0, 0x1_0000_0000), (1, 0x1_0000_0001)]
    {
        assert_eq!(circuit.observe_sequence(sequence), Ordered(order));
    }

    assert_eq!(circuit.observe_sequence(u32::MAX), Ordered(u64::from(u32::MAX)));
}

#[test]
fn source_packet_order_tracks_viewer_sequence_wrap() {
    let mut circuit = Circuit::new();
    for (sequence, order) in [(0x00ff_fffe, 0x00ff_fffe), (0x00ff_ffff, 0x00ff_ffff), (0, 0x0100_0000), (1, 0x0100_0001)] {
        assert_eq!(circuit.observe_sequence(sequence), Ordered(order));
    }

    assert_eq!(circuit.observe_sequence(0x00ff_ffff), Ordered(0x00ff_ffff));
}

#[test]
fn received_duplicate_history_uses_the_viewer_sequence_wrap() {
    let start = Instant::now();
    let mut circuit = Circuit::new();
    for sequence in [0x00ff_fffe, 0x00ff_ffff, 0] {
        receive_packet_at(&mut circuit, &reliable_packet(sequence), start).expect("receive reliable packet");
    }
    circuit.pending_acks.clear();
    circuit.pending_ack_sequences.clear();

    circuit.clear_duplicate_list(0, start + Duration::from_secs(61));

    assert_eq!(circuit.received_sequences.keys().copied().collect::<Vec<_>>(), [0]);
}

#[test]
fn ping_anchors_reliable_object_order_across_wrap() {
    let mut circuit = Circuit::new();
    let now = Instant::now();

    let ping = Packet::new(u32::MAX - 1, start_ping(1, 0));
    let ping = receive_packet_at(&mut circuit, &ping, now).expect("receive inbound ping");
    assert!(ping.is_new_packet);
    assert_eq!(ping.packet_order, Ordered(u64::from(u32::MAX - 1)));

    let object = Packet::new(
        0,
        Message::ObjectUpdateCached(ObjectUpdateCached {
            region_data: RegionData { region_handle: 0, time_dilation: 0 },
            objects: vec![CachedObjectReference { local_id: 1, crc: 0, update_flags: 0 }],
        }),
    )
    .with_reliable();
    let object = receive_packet_at(&mut circuit, &object, now).expect("receive reliable object update");
    assert!(object.is_new_packet);
    assert_eq!(object.packet_order, Ordered(0x1_0000_0000));
}

#[test]
fn sequence_extension_handles_order_window_boundaries() {
    let mut circuit = Circuit::new();
    let first = 50;
    let last = first + MAX_ORDERED_SEQUENCE_DISTANCE;
    assert_eq!(circuit.observe_sequence(first), Ordered(u64::from(first)));
    assert_eq!(circuit.observe_sequence(last), Ordered(u64::from(last)));
    assert_eq!(circuit.observe_sequence(first), Ordered(u64::from(first)));
    assert_eq!(circuit.observe_sequence(first - 1), Ambiguous);
    assert_eq!(circuit.observe_sequence(last + 1), Ordered(u64::from(last + 1)));
}

#[test]
fn old_epoch_sequence_does_not_move_anchor() {
    let mut circuit = Circuit::new();
    assert_eq!(circuit.observe_sequence(10), Ordered(10));
    assert_eq!(circuit.observe_sequence(u32::MAX), Ambiguous);
    assert_eq!(circuit.observe_sequence(11), Ordered(11));
}

#[test]
fn isolated_ambiguous_sequence_preserves_ordering_anchor() {
    let mut circuit = Circuit::new();
    assert_eq!(circuit.observe_sequence(10), Ordered(10));
    assert_eq!(circuit.observe_sequence(10_u32.wrapping_add(1 << 31)), Ambiguous);
    assert_eq!(circuit.observe_sequence(11), Ordered(11));
    assert_eq!(circuit.observe_sequence(9), Ordered(9));
}

#[test]
fn excessive_forward_jump_degrades_ordering_for_the_connection() {
    let mut circuit = Circuit::new();
    assert_eq!(circuit.observe_sequence(10), Ordered(10));
    assert_eq!(circuit.observe_sequence(10 + MAX_ORDERED_SEQUENCE_DISTANCE + 1), Degraded);
    assert_eq!(circuit.observe_sequence(12), Degraded);
    assert_eq!(circuit.observe_sequence(11), Degraded);
}

#[tokio::test]
async fn undeliverable_packets_still_process_acks() {
    let (simulator, transport) = connected_simulator().await;
    let mut buffer = [0; 1024];

    let mut circuit = Circuit::new();
    let outgoing = crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(1), Delivery::Reliable)
        .await
        .expect("send reliable packet");
    simulator.recv_from(&mut buffer).await.expect("receive packet");

    let now = Instant::now();
    let anchor = receive_packet_at(&mut circuit, &reliable_packet(20), now).expect("receive anchor packet");
    assert_eq!(anchor.packet_order, Ordered(20));
    let ambiguous_sequence = 20_u32.wrapping_add(1 << 31);
    let ambiguous = receive_packet_at(&mut circuit, &reliable_packet(ambiguous_sequence).with_appended_acks(vec![outgoing]), now)
        .expect("receive ambiguous reliable packet");
    assert!(ambiguous.is_new_packet);
    assert_eq!(ambiguous.packet_order, Ambiguous);
    assert!(!circuit.has_pending(outgoing), "an ambiguous packet still clears its valid outgoing ACK");
    assert_eq!(circuit.observe_sequence(21), Ordered(21), "isolated ambiguity leaves the anchor usable");
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush ACKs for ambiguous packet");
    let (_, ack) = receive_packet(&simulator, &mut buffer).await;
    std::assert_matches!(ack.message(), Message::PacketAck(message) if message.packets.iter().map(|packet| packet.id).collect::<Vec<_>>() == [20, ambiguous_sequence]);

    let mut circuit = Circuit::new();
    let outgoing = crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(2), Delivery::Reliable)
        .await
        .expect("send another reliable packet");
    simulator.recv_from(&mut buffer).await.expect("receive packet");
    let anchor = receive_packet_at(&mut circuit, &reliable_packet(30), now).expect("receive second anchor");
    assert_eq!(anchor.packet_order, Ordered(30));
    let jump_sequence = 30 + MAX_ORDERED_SEQUENCE_DISTANCE + 1;
    let degraded = receive_packet_at(&mut circuit, &reliable_packet(jump_sequence).with_appended_acks(vec![outgoing]), now)
        .expect("receive jump packet");
    assert!(degraded.is_new_packet);
    assert_eq!(degraded.packet_order, Degraded);
    assert!(!circuit.has_pending(outgoing), "a degraded packet still clears its valid outgoing ACK");
    let later = receive_packet_at(&mut circuit, &reliable_packet(jump_sequence + 1), now).expect("receive later degraded packet");
    assert!(later.is_new_packet);
    assert_eq!(later.packet_order, Degraded);
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush ACKs for degraded packet");
    let (_, ack) = receive_packet(&simulator, &mut buffer).await;
    std::assert_matches!(ack.message(), Message::PacketAck(message) if message.packets.iter().map(|packet| packet.id).collect::<Vec<_>>() == [30, jump_sequence, jump_sequence + 1]);
}

#[tokio::test]
async fn sends_wrapped_sequence() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = with_sequence(0x00ff_fffe);
    let mut buffer = [0; 1024];

    for expected in [0x00ff_fffe_u32, 0x00ff_ffff, 0x0100_0000, 0x0100_0001] {
        crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(1), Delivery::Unreliable)
            .await
            .expect("send packet");
        let (size, _) = simulator.recv_from(&mut buffer).await.expect("read datagram");
        assert_eq!(&buffer[1..5], &expected.to_be_bytes());
        assert_eq!(Packet::decode(&buffer[..size]).expect("decode packet").sequence(), expected);
    }

    let mut circuit = with_sequence(u32::MAX - 1);
    for expected in [u32::MAX - 1, u32::MAX, 0, 1] {
        crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(1), Delivery::Unreliable)
            .await
            .expect("send wrapped packet");
        let (size, _) = simulator.recv_from(&mut buffer).await.expect("read datagram");
        assert_eq!(&buffer[1..5], &expected.to_be_bytes());
        assert_eq!(Packet::decode(&buffer[..size]).expect("decode wrapped packet").sequence(), expected);
    }
}

#[tokio::test]
async fn acks_keep_wide_sequence() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let sequence = 0x0100_0000;
    let now = Instant::now();
    let mut buffer = [0; 1024];

    assert!(receive_at(&mut circuit, &reliable_packet(sequence), now));
    crate::fixtures::circuit::send_acknowledgements(&mut circuit, &transport).await.expect("flush acknowledgement");
    let (_, ack) = receive_packet(&simulator, &mut buffer).await;
    std::assert_matches!(ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == sequence);

    circuit.clear_duplicate_list(sequence + 1, now + Duration::from_secs(1));
    assert!(!receive_at(&mut circuit, &resent_reliable_packet(sequence), now + Duration::from_secs(2)));
}

#[tokio::test]
async fn ping_keeps_wide_watermark() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = with_sequence(0x0100_0000);
    let mut buffer = [0; 1024];

    crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(1), Delivery::Reliable)
        .await
        .expect("send wide reliable packet");
    let (size, _) = simulator.recv_from(&mut buffer).await.expect("read datagram");
    assert_eq!(Packet::decode(&buffer[..size]).expect("decode reliable packet").sequence(), 0x0100_0000);

    send_ping_at(&mut circuit, &transport, Instant::now()).await.expect("send circuit ping");
    let (_, packet) = receive_packet(&simulator, &mut buffer).await;
    assert_eq!(packet.sequence(), 0x0100_0001);
    assert_eq!(packet.message(), &start_ping(0, 0x0100_0000));
}

#[tokio::test]
async fn retry_timeout_tracks_ping() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let mut buffer = [0; 1024];

    assert_eq!(circuit.retry_timeout(), Duration::from_secs(5));
    let sent_at = Instant::now();
    send_ping_at(&mut circuit, &transport, sent_at).await.expect("send circuit ping");
    let (_, packet) = receive_packet(&simulator, &mut buffer).await;
    let Message::StartPingCheck(message) = packet.message() else {
        panic!("expected circuit ping");
    };

    assert!(circuit.record_ping_response(message.ping_id.ping_id, sent_at + Duration::from_millis(300)));
    assert_eq!(circuit.retry_timeout(), Duration::from_millis(4300));
    assert!(!circuit.record_ping_response(message.ping_id.ping_id, Instant::now() + Duration::from_secs(1)));
}

#[test]
fn retry_timeout_backs_off() {
    let circuit = Circuit::new();
    assert_eq!(circuit.retry_timeout_for(0), Duration::from_secs(5));
    assert_eq!(circuit.retry_timeout_for(1), Duration::from_secs(10));
    assert_eq!(circuit.retry_timeout_for(2), Duration::from_secs(20));
    assert_eq!(circuit.retry_timeout_for(3), KEEPALIVE_TIMEOUT);
}

#[tokio::test]
async fn lost_ping_ids_expire_and_wrap() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let start = Instant::now();
    let mut buffer = [0; 1024];
    let mut ping_ids = Vec::with_capacity(300);

    for index in 0..300 {
        let sent_at = start + Duration::from_secs(index * 5);
        send_ping_at(&mut circuit, &transport, sent_at).await.expect("send ping despite lost replies");
        let (_, packet) = receive_packet(&simulator, &mut buffer).await;
        let Message::StartPingCheck(message) = packet.message() else {
            panic!("expected circuit ping");
        };
        ping_ids.push(message.ping_id.ping_id);
    }

    assert_eq!(ping_ids[0], 0);
    assert_eq!(ping_ids[256], 0, "expired IDs become reusable after the u8 range wraps");
}

#[tokio::test]
async fn ping_id_quarantine_expires_at_boundary() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let start = Instant::now();
    let mut buffer = [0; 1024];

    send_ping_at(&mut circuit, &transport, start).await.expect("send first ping");
    let first_id = receive_ping_id(&simulator, &mut buffer).await;
    assert_eq!(first_id, 0);
    let quarantine_started = start + Duration::from_secs(1);
    assert!(circuit.record_ping_response(first_id, quarantine_started));

    for expected_id in 1..=u8::MAX {
        send_ping_at(&mut circuit, &transport, quarantine_started).await.expect("send ping");
        assert_eq!(receive_ping_id(&simulator, &mut buffer).await, expected_id);
        assert!(circuit.record_ping_response(expected_id, quarantine_started));
    }

    let just_before_expiry = quarantine_started + Duration::from_secs(30) - Duration::from_nanos(1);
    send_ping_at(&mut circuit, &transport, just_before_expiry).await.expect("skip while all IDs are quarantined");
    assert_no_datagram(&simulator);

    let at_expiry = quarantine_started + Duration::from_secs(30);
    send_ping_at(&mut circuit, &transport, at_expiry).await.expect("send when IDs expire");
    assert_eq!(receive_ping_id(&simulator, &mut buffer).await, first_id);
}

#[tokio::test]
async fn late_and_duplicate_ping_replies_do_not_change_rtt() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let start = Instant::now();
    let mut buffer = [0; 1024];

    send_ping_at(&mut circuit, &transport, start).await.expect("send first ping");
    let (_, first) = receive_packet(&simulator, &mut buffer).await;
    let Message::StartPingCheck(first) = first.message() else {
        panic!("expected first circuit ping");
    };
    let first_id = first.ping_id.ping_id;

    assert!(!circuit.record_ping_response(first_id, start + Duration::from_secs(31)));
    assert_eq!(circuit.retry_timeout(), Duration::from_secs(5));
    assert!(!circuit.record_ping_response(first_id, start + Duration::from_secs(32)), "duplicate late reply is ignored");
    assert_eq!(circuit.retry_timeout(), Duration::from_secs(5));

    let next_sent_at = start + Duration::from_secs(32);
    send_ping_at(&mut circuit, &transport, next_sent_at).await.expect("send after expiry");
    let (_, next) = receive_packet(&simulator, &mut buffer).await;
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
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let mut buffer = [0; 1024];

    let sent_at = Instant::now();
    send_ping_at(&mut circuit, &transport, sent_at).await.expect("send circuit ping");
    let (_, packet) = receive_packet(&simulator, &mut buffer).await;
    let Message::StartPingCheck(message) = packet.message() else {
        panic!("expected circuit ping");
    };

    assert!(circuit.record_ping_response(message.ping_id.ping_id, sent_at + Duration::from_secs(20)));
    assert_eq!(circuit.retry_timeout(), Duration::from_secs(10));
}

#[tokio::test]
async fn ping_uses_oldest_unacked() {
    let (simulator, transport) = connected_simulator().await;
    let mut circuit = Circuit::new();
    let mut buffer = [0; 1024];

    crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(1), Delivery::Reliable)
        .await
        .expect("send reliable packet");
    crate::fixtures::circuit::send_message(&mut circuit, &transport, complete_ping(2), Delivery::Reliable)
        .await
        .expect("send second reliable packet");
    for _ in 0..2 {
        let (size, _) = simulator.recv_from(&mut buffer).await.expect("read datagram");
        Packet::decode(&buffer[..size]).expect("decode reliable packet");
    }

    send_ping_at(&mut circuit, &transport, Instant::now()).await.expect("send circuit ping");
    let (size, _) = simulator.recv_from(&mut buffer).await.expect("read datagram");
    let mut expected = parse_hex_fixture(vectors::START_PING_CHECK).expect("valid ping fixture");
    expected[1..5].copy_from_slice(&3u32.to_be_bytes());
    expected[7] = 0;
    expected[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert_eq!(&buffer[..size], expected);
    let ping = Packet::decode(&buffer[..size]).expect("decode circuit ping");

    assert!(!ping.is_reliable());
    assert_eq!(ping.message(), &start_ping(0, 1));
}
