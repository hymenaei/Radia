use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    time::Duration,
};

use protocol::{
    lludp::{Message, Packet},
    messages::{PacketAck, PingId, StartPingCheck, packet_ack::Packets},
};
use tokio::time::Instant;

use crate::{
    event::{
        PacketOrder,
        PacketOrder::{Ambiguous, Degraded, Ordered},
    },
    status::NetworkStatusHandle,
    udp::UdpTransport,
};

#[cfg(test)]
pub(crate) const DEFAULT_RETRY_TIMEOUT: Duration = Duration::from_secs(5);
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const MAX_RETRIES: u8 = 3;
pub(crate) const RESENT_FLAG: u8 = 0x20;
pub(crate) const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);
const PING_ID_REUSE_DELAY: Duration = KEEPALIVE_TIMEOUT;
const MAX_PENDING_ACKS: usize = 4096;
pub(crate) const MAX_PENDING_PACKETS: usize = 256;
const MAX_ACKS_PER_PACKET: usize = u8::MAX as usize;
pub(crate) const MAX_RECEIVED_RELIABLE_PACKETS: usize = 65_536;
const DUPLICATE_SUPPRESSION_TIMEOUT: Duration = Duration::from_secs(60);
const INITIAL_PING_DELAY: Duration = Duration::from_secs(1);
const MINIMUM_PING_DELAY: Duration = Duration::from_millis(100);
const MAXIMUM_PING_DELAY: Duration = Duration::from_secs(2);
const MINIMUM_RELIABLE_TIMEOUT: Duration = Duration::from_secs(1);
const RELIABLE_TIMEOUT_FACTOR: u32 = 5;
const SEQUENCE_32_HALF_RANGE: u32 = 1 << 31;
// This conservative policy bound is not a measured LLUDP gap or reordering limit. Raw arrivals
// also cannot distinguish a full unseen u32 wrap from a small forward delta.
pub(crate) const MAX_ORDERED_SEQUENCE_DISTANCE: u32 = 1 << 30;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SequenceOrdering {
    Older(u32),
    Equal,
    Newer(u32),
    Ambiguous,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrderingAuthority {
    Trusted,
    Degraded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SequenceAnchor {
    sequence: u32,
    order: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ReceiveOutcome {
    pub(crate) is_new_packet: bool,
    pub(crate) packet_order: PacketOrder,
}

pub(crate) struct Circuit {
    next_sequence: u32,
    pending: HashMap<u32, PendingPacket>,
    pending_acks: VecDeque<u32>,
    pending_ack_sequences: HashSet<u32>,
    received_sequences: HashMap<u32, Instant>,
    highest_received_sequence: Option<u32>,
    ordering_anchor: Option<SequenceAnchor>,
    ordering_authority: OrderingAuthority,
    duplicate_history_full: bool,
    ping_delay_average: Duration,
    next_ping_id: u8,
    pending_pings: HashMap<u8, Instant>,
    quarantined_ping_ids: HashMap<u8, Instant>,
    status: NetworkStatusHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReceiveError {
    DuplicateHistoryFull,
}

impl fmt::Display for ReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateHistoryFull => formatter.write_str("Reliable packet duplicate history is full; closing the simulator circuit."),
        }
    }
}

impl Drop for Circuit {
    fn drop(&mut self) {
        self.status.set_reliable_packets_pending(0);
    }
}

/// Reliable delivery and local retransmission policy for an outgoing packet.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    /// Do not set the reliable flag or track this packet for retries.
    Unreliable,
    /// Set the reliable flag, track the packet until ACKed, and retry on timeout.
    Reliable,
    /// Set the reliable flag without tracking or retrying the packet locally.
    /// The simulator may still ACK it.
    ReliableNoRetry,
}

impl Circuit {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_status(NetworkStatusHandle::default())
    }

    #[cfg(test)]
    pub(crate) fn with_sequence(next_sequence: u32) -> Self {
        let mut circuit = Self::new();
        circuit.next_sequence = next_sequence;
        circuit
    }

    pub(crate) fn with_status(status: NetworkStatusHandle) -> Self {
        Self {
            next_sequence: 1,
            pending: HashMap::new(),
            pending_acks: VecDeque::new(),
            pending_ack_sequences: HashSet::new(),
            received_sequences: HashMap::new(),
            highest_received_sequence: None,
            ordering_anchor: None,
            ordering_authority: OrderingAuthority::Trusted,
            duplicate_history_full: false,
            ping_delay_average: INITIAL_PING_DELAY,
            next_ping_id: 0,
            pending_pings: HashMap::new(),
            quarantined_ping_ids: HashMap::new(),
            status,
        }
    }

    pub(crate) fn retry_timeout(&self) -> Duration {
        MINIMUM_RELIABLE_TIMEOUT.max(self.ping_delay_average.saturating_mul(RELIABLE_TIMEOUT_FACTOR))
    }

    pub(crate) async fn send_ping(&mut self, socket: &UdpTransport, sent_at: Instant) -> Result<(), String> {
        self.expire_pings(sent_at);
        let Some(ping_id) = available_ping_id(self.next_ping_id, &self.pending_pings, &self.quarantined_ping_ids) else {
            return Ok(());
        };
        let oldest_unacked = oldest_unacked_sequence(self.next_sequence, &self.pending);
        self.send(socket, Message::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id }, oldest_unacked }), Delivery::Unreliable)
            .await?;
        self.pending_pings.insert(ping_id, sent_at);
        self.next_ping_id = ping_id.wrapping_add(1);
        Ok(())
    }

    pub(crate) fn record_ping_response(&mut self, ping_id: u8, received_at: Instant) -> bool {
        self.expire_pings(received_at);
        let Some(sent_at) = self.pending_pings.get(&ping_id).copied() else {
            return false;
        };
        let Some(sample) = received_at.checked_duration_since(sent_at) else {
            return false;
        };
        self.pending_pings.remove(&ping_id);
        self.quarantined_ping_ids.insert(ping_id, received_at);
        self.update_ping_delay(sample);
        true
    }

    fn expire_pings(&mut self, now: Instant) {
        self.quarantined_ping_ids.retain(|_, quarantined_at| now.saturating_duration_since(*quarantined_at) < PING_ID_REUSE_DELAY);

        for ping_id in 0..=u8::MAX {
            let expired =
                self.pending_pings.get(&ping_id).is_some_and(|sent_at| now.saturating_duration_since(*sent_at) >= KEEPALIVE_TIMEOUT);
            if expired {
                self.pending_pings.remove(&ping_id);
                self.quarantined_ping_ids.insert(ping_id, now);
            }
        }
    }

    fn update_ping_delay(&mut self, sample: Duration) {
        let previous_or_sample = self.ping_delay_average.max(sample);
        let average_nanos = (previous_or_sample.as_nanos() * 4 + sample.as_nanos()) / 5;
        self.ping_delay_average = Duration::from_nanos(average_nanos.min(MAXIMUM_PING_DELAY.as_nanos()) as u64).max(MINIMUM_PING_DELAY);
    }

    pub(crate) fn has_pending(&self, sequence: u32) -> bool {
        self.pending.contains_key(&sequence)
    }

    pub(crate) async fn send(&mut self, socket: &UdpTransport, message: Message, delivery: Delivery) -> Result<u32, String> {
        self.send_with_tracking(socket, message, delivery).await
    }

    async fn send_with_tracking(&mut self, socket: &UdpTransport, message: Message, delivery: Delivery) -> Result<u32, String> {
        let result = send_message(
            socket,
            &mut self.next_sequence,
            message,
            delivery,
            &mut self.pending,
            &mut self.pending_acks,
            &mut self.pending_ack_sequences,
        )
        .await;
        let sequence = match result {
            Ok(sequence) => sequence,
            Err(error) => {
                self.status.record_error(&error);
                return Err(error);
            }
        };
        self.status.set_reliable_packets_pending(self.pending.len());
        Ok(sequence)
    }

    pub(crate) async fn flush_pending_acks(&mut self, socket: &UdpTransport) -> Result<(), String> {
        while !self.pending_acks.is_empty() {
            let packet_ids = self.pending_acks.iter().take(MAX_ACKS_PER_PACKET).copied().collect::<Vec<_>>();
            let sequence = self.next_sequence;

            let packets = packet_ids.iter().map(|id| Packets { id: *id }).collect();
            let bytes = Packet::new(sequence, Message::PacketAck(PacketAck { packets }))
                .encode()
                .map_err(|error| format!("Could not encode simulator acknowledgements: {error}"))?;
            socket.send(&bytes).await.map_err(|error| format!("Could not send simulator acknowledgements: {error}"))?;
            self.next_sequence = advance_sequence(self.next_sequence);
            remove_sent_acks(&mut self.pending_acks, &mut self.pending_ack_sequences, &packet_ids);
        }
        Ok(())
    }

    pub(crate) fn receive(&mut self, packet: &Packet) -> Result<ReceiveOutcome, ReceiveError> {
        self.receive_at(packet, Instant::now())
    }

    pub(crate) fn receive_at(&mut self, packet: &Packet, received_at: Instant) -> Result<ReceiveOutcome, ReceiveError> {
        let packet_order = self.observe_sequence(packet.sequence());
        self.acknowledge_outgoing(packet);
        if packet.is_reliable() {
            let sequence = packet.sequence();
            let already_received = self.received_sequences.contains_key(&sequence);
            if already_received {
                let _ = self.queue_ack(sequence);
                return Ok(ReceiveOutcome { is_new_packet: false, packet_order });
            }
            if !already_received && self.received_sequences.len() >= MAX_RECEIVED_RELIABLE_PACKETS {
                if !self.duplicate_history_full {
                    self.status.record_error("Reliable packet duplicate history is full; the packet was not acknowledged.");
                    self.duplicate_history_full = true;
                }
                return Err(ReceiveError::DuplicateHistoryFull);
            }
            if !self.queue_ack(sequence) {
                return Ok(ReceiveOutcome { is_new_packet: false, packet_order });
            }
            self.received_sequences.entry(sequence).or_insert(received_at);
        }
        Ok(ReceiveOutcome { is_new_packet: true, packet_order })
    }

    pub(crate) fn clear_duplicate_list(&mut self, oldest_unacked: u32, now: Instant) {
        let Some(highest_received) = self.highest_received_sequence else {
            return;
        };

        if highest_received == oldest_unacked || sequence_is_newer(highest_received, oldest_unacked) {
            self.received_sequences.retain(|sequence, received_at| {
                sequence_is_at_or_after(*sequence, oldest_unacked)
                    || now.saturating_duration_since(*received_at) <= DUPLICATE_SUPPRESSION_TIMEOUT
            });
        }
        if self.received_sequences.len() < MAX_RECEIVED_RELIABLE_PACKETS {
            self.duplicate_history_full = false;
        }
    }

    pub(crate) fn observe_sequence(&mut self, sequence: u32) -> PacketOrder {
        self.advance_received_sequence(sequence);

        if self.ordering_authority == OrderingAuthority::Degraded {
            return Degraded;
        }

        let Some(anchor) = self.ordering_anchor else {
            let order = u64::from(sequence);
            self.ordering_anchor = Some(SequenceAnchor { sequence, order });
            return Ordered(order);
        };

        match compare_sequence(sequence, anchor.sequence) {
            SequenceOrdering::Equal => Ordered(anchor.order),
            SequenceOrdering::Newer(distance) if distance <= MAX_ORDERED_SEQUENCE_DISTANCE => {
                let Some(order) = anchor.order.checked_add(u64::from(distance)) else {
                    self.ordering_authority = OrderingAuthority::Degraded;
                    return Degraded;
                };
                self.ordering_anchor = Some(SequenceAnchor { sequence, order });
                Ordered(order)
            }
            SequenceOrdering::Newer(_) => {
                self.ordering_authority = OrderingAuthority::Degraded;
                Degraded
            }
            SequenceOrdering::Older(distance) if distance <= MAX_ORDERED_SEQUENCE_DISTANCE => {
                anchor.order.checked_sub(u64::from(distance)).map_or(Ambiguous, Ordered)
            }
            SequenceOrdering::Older(_) | SequenceOrdering::Ambiguous => Ambiguous,
        }
    }

    fn advance_received_sequence(&mut self, sequence: u32) {
        let Some(highest_received) = self.highest_received_sequence else {
            self.highest_received_sequence = Some(sequence);
            return;
        };

        if matches!(compare_sequence(sequence, highest_received), SequenceOrdering::Newer(_)) {
            self.highest_received_sequence = Some(sequence);
        }
    }

    pub(crate) fn acknowledge_outgoing(&mut self, packet: &Packet) {
        acknowledge_pending(packet, &mut self.pending);
        self.status.set_reliable_packets_pending(self.pending.len());
    }

    pub(crate) async fn retransmit_due(&mut self, socket: &UdpTransport) -> Result<(), String> {
        let retry_timeout = self.retry_timeout();
        retransmit_due_tracked(socket, &mut self.pending, &self.status, retry_timeout).await
    }

    fn queue_ack(&mut self, sequence: u32) -> bool {
        if self.pending_ack_sequences.contains(&sequence) {
            return true;
        }
        if self.pending_acks.len() == MAX_PENDING_ACKS {
            return false;
        }
        self.pending_ack_sequences.insert(sequence);
        self.pending_acks.push_back(sequence);
        true
    }
}

fn available_ping_id(next_ping_id: u8, pending_pings: &HashMap<u8, Instant>, quarantined_ping_ids: &HashMap<u8, Instant>) -> Option<u8> {
    (0..=u8::MAX)
        .map(|offset| next_ping_id.wrapping_add(offset))
        .find(|ping_id| !pending_pings.contains_key(ping_id) && !quarantined_ping_ids.contains_key(ping_id))
}

pub(crate) struct PendingPacket {
    pub(crate) bytes: Vec<u8>,
    pub(crate) sent_at: Instant,
    pub(crate) retries: u8,
}

#[tracing::instrument(
    name = "network.lludp.send", level = "debug", skip_all,
    fields(sequence = *next_sequence, delivery = ?delivery, datagram_bytes = tracing::field::Empty)
)]
async fn send_message(
    socket: &UdpTransport,
    next_sequence: &mut u32,
    message: Message,
    delivery: Delivery,
    pending: &mut HashMap<u32, PendingPacket>,
    pending_acks: &mut VecDeque<u32>,
    pending_ack_sequences: &mut HashSet<u32>,
) -> Result<u32, String> {
    if message.is_inbound_only() {
        return Err("Cannot send a simulator-to-viewer message to the simulator.".into());
    }
    if matches!(&message, Message::Schema(_) | Message::Unknown { .. }) {
        return Err("Cannot send a message without a known viewer-to-simulator direction.".into());
    }
    if delivery.tracks_reliable() && pending.len() >= MAX_PENDING_PACKETS {
        return Err("Too many reliable simulator packets are awaiting acknowledgements.".into());
    }

    let sequence = *next_sequence;

    let packet_ids = pending_acks.iter().take(MAX_ACKS_PER_PACKET).copied().collect::<Vec<_>>();
    let mut packet = Packet::new(sequence, message).with_appended_acks(packet_ids.clone());
    if delivery.is_reliable() {
        packet = packet.with_reliable();
    }
    let bytes = packet.encode().map_err(|error| format!("Could not encode a simulator message: {error}"))?;
    socket.send(&bytes).await.map_err(|error| format!("Could not send a simulator message: {error}"))?;
    tracing::Span::current().record("datagram_bytes", bytes.len());
    *next_sequence = advance_sequence(*next_sequence);

    if delivery.tracks_reliable() {
        pending.insert(sequence, PendingPacket { bytes, sent_at: Instant::now(), retries: 0 });
    }
    remove_sent_acks(pending_acks, pending_ack_sequences, &packet_ids);
    Ok(sequence)
}

pub(crate) fn advance_sequence(sequence: u32) -> u32 {
    sequence.wrapping_add(1)
}

impl Delivery {
    fn is_reliable(self) -> bool {
        !matches!(self, Self::Unreliable)
    }

    fn tracks_reliable(self) -> bool {
        matches!(self, Self::Reliable)
    }
}

fn remove_sent_acks(pending_acks: &mut VecDeque<u32>, pending_ack_sequences: &mut HashSet<u32>, packet_ids: &[u32]) {
    for packet_id in packet_ids {
        if pending_acks.front() != Some(packet_id) {
            break;
        }
        pending_acks.pop_front();
        pending_ack_sequences.remove(packet_id);
    }
}

#[cfg(test)]
pub(crate) async fn retransmit_due(socket: &UdpTransport, pending: &mut HashMap<u32, PendingPacket>) -> Result<(), String> {
    retransmit_due_tracked(socket, pending, &NetworkStatusHandle::default(), DEFAULT_RETRY_TIMEOUT).await
}

async fn retransmit_due_tracked(
    socket: &UdpTransport,
    pending: &mut HashMap<u32, PendingPacket>,
    status: &NetworkStatusHandle,
    retry_timeout: Duration,
) -> Result<(), String> {
    let now = Instant::now();
    for packet in pending.values_mut() {
        if now.duration_since(packet.sent_at) < retry_timeout {
            continue;
        }
        if packet.retries >= MAX_RETRIES {
            return Err("The simulator did not acknowledge a reliable packet.".into());
        }

        let mut bytes = packet.bytes.clone();
        bytes[0] |= RESENT_FLAG;
        socket.send(&bytes).await.map_err(|error| format!("Could not retry a simulator message: {error}"))?;
        tracing::debug!(retry = packet.retries + 1, datagram_bytes = bytes.len(), "Reliable packet retransmitted");
        packet.bytes = bytes;
        packet.sent_at = now;
        packet.retries += 1;
        status.note_retransmit();
    }
    Ok(())
}

fn oldest_unacked_sequence(next_sequence: u32, pending: &HashMap<u32, PendingPacket>) -> u32 {
    pending.keys().copied().max_by_key(|sequence| sequence_age(next_sequence, *sequence)).unwrap_or_else(|| next_sequence.wrapping_sub(1))
}

pub(crate) fn sequence_age(next_sequence: u32, sequence: u32) -> u32 {
    next_sequence.wrapping_sub(sequence)
}

pub(crate) fn sequence_is_newer(sequence: u32, reference: u32) -> bool {
    matches!(compare_sequence(sequence, reference), SequenceOrdering::Newer(_))
}

fn sequence_is_at_or_after(sequence: u32, reference: u32) -> bool {
    matches!(compare_sequence(sequence, reference), SequenceOrdering::Equal | SequenceOrdering::Newer(_))
}

fn compare_sequence(sequence: u32, reference: u32) -> SequenceOrdering {
    let forward = sequence.wrapping_sub(reference);
    let backward = reference.wrapping_sub(sequence);
    match forward {
        0 => SequenceOrdering::Equal,
        distance if distance == SEQUENCE_32_HALF_RANGE => SequenceOrdering::Ambiguous,
        distance if distance < SEQUENCE_32_HALF_RANGE => SequenceOrdering::Newer(distance),
        _ => SequenceOrdering::Older(backward),
    }
}

pub(crate) fn acknowledge_pending(packet: &Packet, pending: &mut HashMap<u32, PendingPacket>) {
    for ack in packet.appended_acks() {
        pending.remove(ack);
    }
    if let Message::PacketAck(message) = packet.message() {
        for packet in &message.packets {
            pending.remove(&packet.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, net::SocketAddr, time::Duration};

    use super::{Circuit, available_ping_id};
    use crate::udp::UdpTransport;
    use tokio::{
        net::UdpSocket,
        time::{Instant, timeout},
    };

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
        let address: SocketAddr = simulator.local_addr().expect("fake simulator address");
        let transport = UdpTransport::connect(address).await.expect("connect fake simulator");
        let mut circuit = Circuit::new();
        let now = Instant::now();
        circuit.pending_pings.extend((0..=u8::MAX).map(|ping_id| (ping_id, now)));
        let next_sequence = circuit.next_sequence;

        circuit.send_ping(&transport, now).await.expect("skip ping without an unused id");

        assert_eq!(circuit.pending_pings.len(), 256);
        assert_eq!(circuit.next_sequence, next_sequence);
        let mut buffer = [0; 64];
        assert!(timeout(Duration::from_millis(20), simulator.recv_from(&mut buffer)).await.is_err());
    }
}

#[cfg(test)]
mod protocol_tests {
    use crate::{
        circuit::{
            Circuit, Delivery, MAX_ORDERED_SEQUENCE_DISTANCE, MAX_PENDING_PACKETS, MAX_RECEIVED_RELIABLE_PACKETS, ReceiveError,
            advance_sequence, sequence_age, sequence_is_newer,
        },
        event::PacketOrder::{Ambiguous, Degraded, Ordered},
        status::NetworkStatusHandle,
        udp::UdpTransport,
    };
    use base::UUID;
    use math::Vec3;
    use protocol::{
        Audible, ChatSourceType, ChatType,
        lludp::{Message, MessageId, Packet},
        messages::{
            CachedObjectReference, ChatFromSimulator, ChatFromViewer, CompletePingCheck, LogoutRequest, ObjectUpdateCached, PingId,
            RegionData, StartPingCheck, chat_from_simulator::ChatData, chat_from_viewer, logout_request,
        },
    };
    use std::{net::SocketAddr, time::Duration};
    use test_support::{lludp::vectors, parse_hex_fixture};
    use tokio::{
        net::UdpSocket,
        time::{Instant, timeout},
    };

    fn complete_ping(ping_id: u8) -> Message {
        Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id } })
    }

    fn start_ping(ping_id: u8, oldest_unacked: u32) -> Message {
        Message::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id }, oldest_unacked })
    }

    fn receive_at(circuit: &mut Circuit, packet: &Packet, received_at: Instant) -> bool {
        circuit.receive_at(packet, received_at).expect("valid circuit packet").is_new_packet
    }

    #[tokio::test]
    async fn refuses_inbound_message() {
        let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let address: SocketAddr = server.local_addr().expect("fake simulator address");
        let transport = UdpTransport::connect(address).await.expect("connect fake simulator");
        let message = Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: "Resident".into(),
                source_id: UUID::nil(),
                owner_id: UUID::nil(),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Normal,
                audible: Audible::Fully,
                position: Vec3::default(),
                message: "hello".into(),
            },
        });

        let error =
            Circuit::new().send(&transport, message, Delivery::Unreliable).await.expect_err("inbound-only message must be rejected");
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
            agent_data: chat_from_viewer::AgentData { agent_id: UUID::nil(), session_id: UUID::nil() },
            chat_data: chat_from_viewer::ChatData { message: "x".repeat(9_000), chat_type: ChatType::Normal, channel: 0 },
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
            let (size, _) = timeout(Duration::from_secs(1), server.recv_from(&mut buffer))
                .await
                .expect("read simulator datagram")
                .expect("read datagram");
            let packet = Packet::decode(&buffer[..size]).expect("decode simulator packet");
            assert_eq!(packet.sequence(), sequence as u32);
        }

        let logout_sequence = circuit
            .send(
                &transport,
                Message::LogoutRequest(LogoutRequest {
                    agent_data: logout_request::AgentData { agent_id: UUID::nil(), session_id: UUID::nil() },
                }),
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
    fn wrap32_prunes_old_ids() {
        let start = Instant::now();
        let mut circuit = Circuit::new();

        for (sequence, second) in [(u32::MAX - 1, 0), (u32::MAX, 1), (0, 2), (1, 3), (2, 4), (3, 5)] {
            assert!(receive_at(&mut circuit, &reliable_packet(sequence), start + Duration::from_secs(second)));
        }

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
        assert!(
            matches!(duplicate_ack.message(), Message::PacketAck(message) if message.packets.len() == 1 && message.packets[0].id == 72)
        );

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
    fn sequence_is_newer_rejects_equal_and_ambiguous_pairs() {
        assert!(sequence_is_newer(11, 10));
        assert!(!sequence_is_newer(10, 10));
        assert!(!sequence_is_newer(1 << 31, 0));
        assert!(!sequence_is_newer(9, 10));
    }

    #[test]
    fn accepts_wide_sequence() {
        let status = NetworkStatusHandle::new();
        let mut circuit = Circuit::with_status(status.clone());
        let now = Instant::now();

        assert!(circuit.receive_at(&reliable_packet(0x0100_0000), now).expect("valid 32-bit sequence").is_new_packet);
        circuit.clear_duplicate_list(0x0100_0001, now + Duration::from_secs(1));
        assert!(status.snapshot().last_error.is_none());
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
    fn ping_anchors_reliable_object_order_across_wrap() {
        let mut circuit = Circuit::new();
        let now = Instant::now();

        let ping = Packet::new(u32::MAX - 1, start_ping(1, 0));
        let ping = circuit.receive_at(&ping, now).expect("receive inbound ping");
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
        let object = circuit.receive_at(&object, now).expect("receive reliable object update");
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
        let simulator = UdpSocket::bind("127.0.0.1:0").await.expect("bind fake simulator");
        let transport = UdpTransport::connect(simulator.local_addr().expect("simulator address")).await.expect("connect fake simulator");
        let mut buffer = [0; 1024];

        let mut circuit = Circuit::new();
        let outgoing = circuit.send(&transport, complete_ping(1), Delivery::Reliable).await.expect("send reliable packet");
        timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read outgoing packet").expect("receive packet");

        let now = Instant::now();
        let anchor = circuit.receive_at(&reliable_packet(20), now).expect("receive anchor packet");
        assert_eq!(anchor.packet_order, Ordered(20));
        let ambiguous_sequence = 20_u32.wrapping_add(1 << 31);
        let ambiguous = circuit
            .receive_at(&reliable_packet(ambiguous_sequence).with_appended_acks(vec![outgoing]), now)
            .expect("receive ambiguous reliable packet");
        assert!(ambiguous.is_new_packet);
        assert_eq!(ambiguous.packet_order, Ambiguous);
        assert!(!circuit.has_pending(outgoing), "an ambiguous packet still clears its valid outgoing ACK");
        assert_eq!(circuit.observe_sequence(21), Ordered(21), "isolated ambiguity leaves the anchor usable");
        circuit.flush_pending_acks(&transport).await.expect("flush ACKs for ambiguous packet");
        let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read ACK").expect("receive ACK");
        let ack = Packet::decode(&buffer[..size]).expect("decode ACK");
        assert!(
            matches!(ack.message(), Message::PacketAck(message) if message.packets.iter().map(|packet| packet.id).collect::<Vec<_>>() == [20, ambiguous_sequence])
        );

        let mut circuit = Circuit::new();
        let outgoing = circuit.send(&transport, complete_ping(2), Delivery::Reliable).await.expect("send another reliable packet");
        timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read outgoing packet").expect("receive packet");
        let anchor = circuit.receive_at(&reliable_packet(30), now).expect("receive second anchor");
        assert_eq!(anchor.packet_order, Ordered(30));
        let jump_sequence = 30 + MAX_ORDERED_SEQUENCE_DISTANCE + 1;
        let degraded =
            circuit.receive_at(&reliable_packet(jump_sequence).with_appended_acks(vec![outgoing]), now).expect("receive jump packet");
        assert!(degraded.is_new_packet);
        assert_eq!(degraded.packet_order, Degraded);
        assert!(!circuit.has_pending(outgoing), "a degraded packet still clears its valid outgoing ACK");
        let later = circuit.receive_at(&reliable_packet(jump_sequence + 1), now).expect("receive later degraded packet");
        assert!(later.is_new_packet);
        assert_eq!(later.packet_order, Degraded);
        circuit.flush_pending_acks(&transport).await.expect("flush ACKs for degraded packet");
        let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read ACK").expect("receive ACK");
        let ack = Packet::decode(&buffer[..size]).expect("decode ACK");
        assert!(
            matches!(ack.message(), Message::PacketAck(message) if message.packets.iter().map(|packet| packet.id).collect::<Vec<_>>() == [30, jump_sequence, jump_sequence + 1])
        );
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
            let (size, _) = timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer))
                .await
                .expect("read wrapped packet")
                .expect("read datagram");
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
        let (size, _) =
            timeout(Duration::from_secs(1), simulator.recv_from(buffer)).await.expect("read ping").expect("receive ping datagram");
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
        let (size, _) =
            timeout(Duration::from_secs(1), simulator.recv_from(&mut buffer)).await.expect("read next ping").expect("receive ping");
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
}
