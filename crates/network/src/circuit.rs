use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt,
    time::Duration,
};

use protocol::{
    messages::{PacketAck, StartPingCheck, packet_ack::Packets, start_ping_check::PingID},
    udp::{Message, Packet},
};
use tokio::time::Instant;

use crate::{status::NetworkStatusHandle, udp::UdpTransport};

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
const SEQUENCE_24_MAX: u32 = 0x00ff_ffff;
const SEQUENCE_24_HALF_RANGE: u32 = 1 << 23;
const SEQUENCE_32_HALF_RANGE: u32 = 1 << 31;
pub(crate) const MAX_RECEIVED_RELIABLE_PACKETS: usize = 65_536;
const DUPLICATE_SUPPRESSION_TIMEOUT: Duration = Duration::from_secs(60);
const INITIAL_PING_DELAY: Duration = Duration::from_secs(1);
const MINIMUM_PING_DELAY: Duration = Duration::from_millis(100);
const MAXIMUM_PING_DELAY: Duration = Duration::from_secs(2);
const MINIMUM_RELIABLE_TIMEOUT: Duration = Duration::from_secs(1);
const RELIABLE_TIMEOUT_FACTOR: u32 = 5;

pub(crate) struct Circuit {
    next_sequence: u32,
    pending: HashMap<u32, PendingPacket>,
    pending_acks: VecDeque<u32>,
    pending_ack_sequences: HashSet<u32>,
    received_sequences: HashMap<u32, Instant>,
    highest_received_sequence: Option<u32>,
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
        self.send(socket, Message::StartPingCheck(StartPingCheck { ping_id: PingID { ping_id, oldest_unacked } }), Delivery::Unreliable)
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

    pub(crate) fn receive(&mut self, packet: &Packet) -> Result<bool, ReceiveError> {
        self.receive_at(packet, Instant::now())
    }

    pub(crate) fn receive_at(&mut self, packet: &Packet, received_at: Instant) -> Result<bool, ReceiveError> {
        self.observe_sequence(packet.sequence());
        self.acknowledge_outgoing(packet);
        if packet.is_reliable() {
            let sequence = packet.sequence();
            let already_received = self.received_sequences.contains_key(&sequence);
            if already_received {
                let _ = self.queue_ack(sequence);
                return Ok(false);
            }
            if !already_received && self.received_sequences.len() >= MAX_RECEIVED_RELIABLE_PACKETS {
                if !self.duplicate_history_full {
                    self.status.record_error("Reliable packet duplicate history is full; the packet was not acknowledged.");
                    self.duplicate_history_full = true;
                }
                return Err(ReceiveError::DuplicateHistoryFull);
            }
            if !self.queue_ack(sequence) {
                return Ok(false);
            }
            self.received_sequences.entry(sequence).or_insert(received_at);
        }
        Ok(true)
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

    pub(crate) fn observe_sequence(&mut self, sequence: u32) {
        let Some(highest_received) = self.highest_received_sequence else {
            self.highest_received_sequence = Some(sequence);
            return;
        };

        if sequence_is_newer(sequence, highest_received) {
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
    name = "network.udp.send", level = "debug", skip_all,
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
    let (distance, half_range) = if sequence <= SEQUENCE_24_MAX && reference <= SEQUENCE_24_MAX {
        (sequence.wrapping_sub(reference) & SEQUENCE_24_MAX, SEQUENCE_24_HALF_RANGE)
    } else {
        (sequence.wrapping_sub(reference), SEQUENCE_32_HALF_RANGE)
    };
    distance != 0 && distance < half_range
}

fn sequence_is_at_or_after(sequence: u32, reference: u32) -> bool {
    sequence == reference || sequence_is_newer(sequence, reference)
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
