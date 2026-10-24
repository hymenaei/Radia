#[cfg(test)]
pub(crate) mod test_values {
    use super::{Circuit, DUPLICATE_ACK_HEADROOM, MAX_PENDING_ACKS};

    pub(crate) const NEW_RELIABLE_ACK_CAPACITY: usize = MAX_PENDING_ACKS - DUPLICATE_ACK_HEADROOM;

    pub(crate) fn with_sequence(next_sequence: u32) -> Circuit {
        let mut circuit = Circuit::new();
        circuit.next_sequence = next_sequence;
        circuit
    }
}

use std::{
    collections::{HashMap, HashSet, VecDeque},
    error::Error as StdError,
    fmt,
    time::Duration,
};

use protocol::{
    lludp::{LLUdpError, Message, Packet, PacketFlags},
    messages::{PacketAck, PingId, StartPingCheck, circuit::packet_ack::Packets},
};
use tokio::time::Instant;

use crate::event::{
    PacketOrder,
    PacketOrder::{Ambiguous, Degraded, Ordered},
};
pub(crate) const PING_INTERVAL: Duration = Duration::from_secs(5);
pub(crate) const MAX_RETRIES: u8 = 3;
pub(crate) const RESENT_FLAG: u8 = 0x20;
pub(crate) const KEEPALIVE_TIMEOUT: Duration = Duration::from_secs(30);
const PING_ID_REUSE_DELAY: Duration = KEEPALIVE_TIMEOUT;
const MAX_PENDING_ACKS: usize = 4096;
const DUPLICATE_ACK_HEADROOM: usize = 256;
pub(crate) const MAX_PENDING_PACKETS: usize = 256;
const MAX_ACKS_PER_PACKET: usize = u8::MAX as usize;
const MAX_NORMAL_DATAGRAM_BYTES: usize = 1_200;
pub(crate) const MAX_RECEIVED_RELIABLE_PACKETS: usize = 65_536;
const DUPLICATE_SUPPRESSION_TIMEOUT: Duration = Duration::from_secs(60);
const SEQUENCE_24_MASK: u32 = 0x00ff_ffff;
const SEQUENCE_24_HALF_RANGE: u32 = 1 << 23;
const INITIAL_PING_DELAY: Duration = Duration::from_secs(1);
const MINIMUM_PING_DELAY: Duration = Duration::from_millis(100);
const MAXIMUM_PING_DELAY: Duration = Duration::from_secs(2);
const MINIMUM_RELIABLE_TIMEOUT: Duration = Duration::from_secs(1);
const RELIABLE_TIMEOUT_FACTOR: u32 = 5;
const SEQUENCE_32_HALF_RANGE: u32 = 1 << 31;
pub(crate) const MAX_ORDERED_SEQUENCE_DISTANCE: u32 = 1 << 30;

#[derive(Clone, Debug)]
pub(crate) enum CircuitError {
    InboundOnly,
    UnknownDirection,
    ReliableWindowFull,
    EncodeMessage(LLUdpError),
    EncodeAcknowledgements(LLUdpError),
    RetryLimit,
    MessageStateChanged,
    AcknowledgementStateChanged,
}

impl fmt::Display for CircuitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InboundOnly => formatter.write_str("Cannot send a simulator-to-viewer message to the simulator."),
            Self::UnknownDirection => formatter.write_str("Cannot send a message without a known viewer-to-simulator direction."),
            Self::ReliableWindowFull => formatter.write_str("Too many reliable simulator packets are awaiting acknowledgements."),
            Self::EncodeMessage(error) => write!(formatter, "Could not encode a simulator message: {error}"),
            Self::EncodeAcknowledgements(error) => write!(formatter, "Could not encode simulator acknowledgements: {error}"),
            Self::RetryLimit => formatter.write_str("The simulator did not acknowledge a reliable packet."),
            Self::MessageStateChanged => formatter.write_str("LLUDP send state changed while a packet was waiting to be sent."),
            Self::AcknowledgementStateChanged => {
                formatter.write_str("LLUDP send state changed while acknowledgements were waiting to be sent.")
            }
        }
    }
}

impl StdError for CircuitError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::EncodeMessage(error) | Self::EncodeAcknowledgements(error) => Some(error),
            _ => None,
        }
    }
}

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

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ReceiveAdmission {
    Duplicate(ReceiveOutcome),
    New(ReceiveReservation),
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ReceiveReservation {
    sequence: u32,
    reliable: bool,
    received_at: Instant,
}

pub(crate) struct Circuit {
    next_sequence: u32,
    pending: HashMap<u32, PendingPacket>,
    pending_acks: VecDeque<u32>,
    pending_ack_sequences: HashSet<u32>,
    received_sequences: HashMap<u32, Instant>,
    highest_received_sequence: Option<u32>,
    wide_sequence_seen: bool,
    ordering_anchor: Option<SequenceAnchor>,
    ordering_authority: OrderingAuthority,
    duplicate_history_full: bool,
    ping_delay_average: Duration,
    next_ping_id: u8,
    pending_pings: HashMap<u8, Instant>,
    quarantined_ping_ids: HashMap<u8, Instant>,
}

impl Circuit {
    pub(crate) fn new() -> Self {
        Self {
            next_sequence: 1,
            pending: HashMap::new(),
            pending_acks: VecDeque::new(),
            pending_ack_sequences: HashSet::new(),
            received_sequences: HashMap::new(),
            highest_received_sequence: None,
            wide_sequence_seen: false,
            ordering_anchor: None,
            ordering_authority: OrderingAuthority::Trusted,
            duplicate_history_full: false,
            ping_delay_average: INITIAL_PING_DELAY,
            next_ping_id: 0,
            pending_pings: HashMap::new(),
            quarantined_ping_ids: HashMap::new(),
        }
    }

    pub(crate) fn retry_timeout(&self) -> Duration {
        MINIMUM_RELIABLE_TIMEOUT.max(self.ping_delay_average.saturating_mul(RELIABLE_TIMEOUT_FACTOR))
    }

    fn retry_timeout_for(&self, retries: u8) -> Duration {
        let multiplier = 1u32.checked_shl(u32::from(retries)).unwrap_or(u32::MAX);
        self.retry_timeout().saturating_mul(multiplier).min(KEEPALIVE_TIMEOUT)
    }

    pub(crate) fn prepare_ping(&mut self, sent_at: Instant) -> Result<Option<PreparedDatagram>, CircuitError> {
        self.expire_pings(sent_at);
        let Some(ping_id) = available_ping_id(self.next_ping_id, &self.pending_pings, &self.quarantined_ping_ids) else {
            return Ok(None);
        };
        let oldest_unacked = oldest_unacked_sequence(self.next_sequence, &self.pending);
        self.prepare_message_with_ping(
            Message::StartPingCheck(StartPingCheck { ping_id: PingId { ping_id }, oldest_unacked }),
            Delivery::Unreliable,
            Some(ping_id),
        )
        .map(Some)
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

    pub(crate) fn has_pending_acks(&self) -> bool {
        !self.pending_acks.is_empty()
    }

    pub(crate) fn has_reliable_capacity(&self) -> bool {
        self.pending.len() < MAX_PENDING_PACKETS
    }

    pub(crate) fn prepare_message(&self, message: Message, delivery: Delivery) -> Result<PreparedDatagram, CircuitError> {
        self.prepare_message_with_ping(message, delivery, None)
    }

    fn prepare_message_with_ping(&self, message: Message, delivery: Delivery, ping: Option<u8>) -> Result<PreparedDatagram, CircuitError> {
        if message.is_inbound_only() {
            return Err(CircuitError::InboundOnly);
        }
        if matches!(&message, Message::Schema(_) | Message::Unknown { .. }) {
            return Err(CircuitError::UnknownDirection);
        }
        if delivery.tracks_reliable() && !self.has_reliable_capacity() {
            return Err(CircuitError::ReliableWindowFull);
        }

        let sequence = self.next_sequence;
        let mut packet = Packet::new(sequence, message);
        if delivery.is_reliable() {
            packet = packet.with_reliable();
        }
        let mut bytes = packet.encode().map_err(CircuitError::EncodeMessage)?;
        let available_ack_bytes = MAX_NORMAL_DATAGRAM_BYTES.saturating_sub(bytes.len().saturating_add(1));
        let ack_count = self.pending_acks.len().min(MAX_ACKS_PER_PACKET).min(available_ack_bytes / size_of::<u32>());
        let appended_acks = self.pending_acks.iter().take(ack_count).copied().collect::<Vec<_>>();
        if !appended_acks.is_empty() {
            bytes[0] |= PacketFlags::APPENDED_ACKS.bits();
            for ack in &appended_acks {
                bytes.extend_from_slice(&ack.to_be_bytes());
            }
            bytes.push(u8::try_from(appended_acks.len()).expect("ACK batch is capped at the wire maximum"));
        }
        Ok(PreparedDatagram { bytes, commit: DatagramCommit::Message { sequence, delivery, appended_acks, ping } })
    }

    pub(crate) fn prepare_acknowledgements(&self) -> Result<Option<PreparedDatagram>, CircuitError> {
        if self.pending_acks.is_empty() {
            return Ok(None);
        }
        let acknowledgements = self.pending_acks.iter().take(MAX_ACKS_PER_PACKET).copied().collect::<Vec<_>>();
        let packets = acknowledgements.iter().map(|id| Packets { id: *id }).collect();
        let sequence = self.next_sequence;
        let bytes =
            Packet::new(sequence, Message::PacketAck(PacketAck { packets })).encode().map_err(CircuitError::EncodeAcknowledgements)?;
        Ok(Some(PreparedDatagram { bytes, commit: DatagramCommit::Acknowledgements { sequence, acknowledgements } }))
    }

    pub(crate) fn prepare_retransmission(&self, now: Instant) -> Result<Option<PreparedDatagram>, CircuitError> {
        let due = self
            .pending
            .iter()
            .filter(|(_, packet)| now.saturating_duration_since(packet.sent_at) >= self.retry_timeout_for(packet.retries))
            .min_by_key(|(_, packet)| packet.sent_at);
        let Some((&sequence, packet)) = due else {
            return Ok(None);
        };
        if packet.retries >= MAX_RETRIES {
            return Err(CircuitError::RetryLimit);
        }
        let mut bytes = packet.bytes.clone();
        bytes[0] |= RESENT_FLAG;
        Ok(Some(PreparedDatagram {
            bytes,
            commit: DatagramCommit::Retransmission { sequence, sent_at: packet.sent_at, retries: packet.retries },
        }))
    }

    pub(crate) fn commit_sent(&mut self, prepared: PreparedDatagram, sent_at: Instant) -> Result<(), CircuitError> {
        match prepared.commit {
            DatagramCommit::Message { sequence, delivery, appended_acks, ping } => {
                if sequence != self.next_sequence {
                    return Err(CircuitError::MessageStateChanged);
                }
                self.next_sequence = advance_sequence(sequence);
                if delivery.tracks_reliable() {
                    self.pending.insert(sequence, PendingPacket { bytes: prepared.bytes, sent_at, retries: 0 });
                }
                remove_sent_acks(&mut self.pending_acks, &mut self.pending_ack_sequences, &appended_acks);
                if let Some(ping_id) = ping {
                    self.pending_pings.insert(ping_id, sent_at);
                    self.next_ping_id = ping_id.wrapping_add(1);
                }
            }
            DatagramCommit::Acknowledgements { sequence, acknowledgements } => {
                if sequence != self.next_sequence {
                    return Err(CircuitError::AcknowledgementStateChanged);
                }
                self.next_sequence = advance_sequence(sequence);
                remove_sent_acks(&mut self.pending_acks, &mut self.pending_ack_sequences, &acknowledgements);
            }
            DatagramCommit::Retransmission { sequence, sent_at: prepared_at, retries } => {
                if let Some(packet) = self.pending.get_mut(&sequence)
                    && packet.sent_at == prepared_at
                    && packet.retries == retries
                {
                    packet.bytes = prepared.bytes;
                    packet.sent_at = sent_at;
                    packet.retries += 1;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn prepare_receive(
        &mut self,
        sequence: u32,
        reliable: bool,
        appended_acks: impl IntoIterator<Item = u32>,
        received_at: Instant,
    ) -> Result<ReceiveAdmission, ReceiveError> {
        let packet_order = self.preview_sequence(sequence);
        self.acknowledge_outgoing_ids(appended_acks);
        if !reliable {
            return Ok(ReceiveAdmission::New(ReceiveReservation { sequence, reliable, received_at }));
        }

        if self.received_sequences.contains_key(&sequence) {
            if !self.queue_ack(sequence) {
                return Err(ReceiveError::AcknowledgementCapacityFull);
            }
            return Ok(ReceiveAdmission::Duplicate(ReceiveOutcome { is_new_packet: false, packet_order }));
        }
        if self.received_sequences.len() >= MAX_RECEIVED_RELIABLE_PACKETS {
            if !self.duplicate_history_full {
                tracing::warn!("Reliable packet duplicate history is full; new reliable packets are not acknowledged");
                self.duplicate_history_full = true;
            }
            return Err(ReceiveError::DuplicateHistoryFull);
        }
        if self.pending_acks.len() >= MAX_PENDING_ACKS - DUPLICATE_ACK_HEADROOM
            || self.pending_acks.try_reserve(1).is_err()
            || self.pending_ack_sequences.try_reserve(1).is_err()
            || self.received_sequences.try_reserve(1).is_err()
        {
            return Err(ReceiveError::AcknowledgementCapacityFull);
        }
        Ok(ReceiveAdmission::New(ReceiveReservation { sequence, reliable, received_at }))
    }

    pub(crate) fn commit_receive(&mut self, admission: ReceiveAdmission) -> ReceiveOutcome {
        match admission {
            ReceiveAdmission::Duplicate(outcome) => outcome,
            ReceiveAdmission::New(reservation) => {
                let packet_order = self.observe_sequence(reservation.sequence);
                if reservation.reliable {
                    let queued = self.queue_ack(reservation.sequence);
                    debug_assert!(queued, "reliable receive commit follows queue-capacity reservation");
                    self.received_sequences.entry(reservation.sequence).or_insert(reservation.received_at);
                }
                ReceiveOutcome { is_new_packet: true, packet_order }
            }
        }
    }

    pub(crate) fn commit_received_packet(&mut self, admission: ReceiveAdmission, packet: &Packet) -> ReceiveOutcome {
        self.acknowledge_message(packet.message());
        self.commit_receive(admission)
    }

    pub(crate) fn acknowledge_message(&mut self, message: &Message) {
        if let Message::PacketAck(message) = message {
            self.acknowledge_outgoing_ids(message.packets.iter().map(|packet| packet.id));
        }
    }

    pub(crate) fn clear_duplicate_list(&mut self, oldest_unacked: u32, now: Instant) {
        let Some(highest_received) = self.highest_received_sequence else {
            return;
        };

        if highest_received == oldest_unacked || self.received_sequence_is_newer(highest_received, oldest_unacked) {
            let pending_ack_sequences = &self.pending_ack_sequences;
            let wide_sequence_seen = self.wide_sequence_seen;
            self.received_sequences.retain(|sequence, received_at| {
                pending_ack_sequences.contains(sequence)
                    || matches!(
                        compare_received_sequence(
                            *sequence,
                            oldest_unacked,
                            wide_sequence_seen || *sequence > SEQUENCE_24_MASK || oldest_unacked > SEQUENCE_24_MASK,
                        ),
                        SequenceOrdering::Equal | SequenceOrdering::Newer(_)
                    )
                    || now.saturating_duration_since(*received_at) <= DUPLICATE_SUPPRESSION_TIMEOUT
            });
        }
        if self.received_sequences.len() < MAX_RECEIVED_RELIABLE_PACKETS {
            self.duplicate_history_full = false;
        }
    }

    pub(crate) fn observe_sequence(&mut self, sequence: u32) -> PacketOrder {
        self.wide_sequence_seen |= sequence > SEQUENCE_24_MASK;
        self.advance_received_sequence(sequence);

        if self.ordering_authority == OrderingAuthority::Degraded {
            return Degraded;
        }

        let Some(anchor) = self.ordering_anchor else {
            let order = u64::from(sequence);
            self.ordering_anchor = Some(SequenceAnchor { sequence, order });
            return Ordered(order);
        };

        match compare_received_sequence(sequence, anchor.sequence, self.wide_sequence_seen) {
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

    fn preview_sequence(&self, sequence: u32) -> PacketOrder {
        if self.ordering_authority == OrderingAuthority::Degraded {
            return Degraded;
        }

        let Some(anchor) = self.ordering_anchor else {
            return Ordered(u64::from(sequence));
        };

        match compare_received_sequence(sequence, anchor.sequence, self.wide_sequence_seen || sequence > SEQUENCE_24_MASK) {
            SequenceOrdering::Equal => Ordered(anchor.order),
            SequenceOrdering::Newer(distance) if distance <= MAX_ORDERED_SEQUENCE_DISTANCE => {
                anchor.order.checked_add(u64::from(distance)).map_or(Degraded, Ordered)
            }
            SequenceOrdering::Newer(_) => Degraded,
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

        if matches!(compare_received_sequence(sequence, highest_received, self.wide_sequence_seen), SequenceOrdering::Newer(_)) {
            self.highest_received_sequence = Some(sequence);
        }
    }

    fn received_sequence_is_newer(&self, sequence: u32, reference: u32) -> bool {
        matches!(
            compare_received_sequence(
                sequence,
                reference,
                self.wide_sequence_seen || sequence > SEQUENCE_24_MASK || reference > SEQUENCE_24_MASK
            ),
            SequenceOrdering::Newer(_)
        )
    }

    fn acknowledge_outgoing_ids(&mut self, acknowledgements: impl IntoIterator<Item = u32>) {
        for acknowledged in acknowledgements {
            self.pending.remove(&acknowledged);
        }
    }

    fn queue_ack(&mut self, sequence: u32) -> bool {
        if self.pending_ack_sequences.contains(&sequence) {
            return true;
        }
        if self.pending_acks.len() == MAX_PENDING_ACKS
            || self.pending_acks.try_reserve(1).is_err()
            || self.pending_ack_sequences.try_reserve(1).is_err()
        {
            return false;
        }
        self.pending_ack_sequences.insert(sequence);
        self.pending_acks.push_back(sequence);
        true
    }
}

#[cfg_attr(test, derive(Clone))]
pub(crate) struct PreparedDatagram {
    bytes: Vec<u8>,
    commit: DatagramCommit,
}

impl PreparedDatagram {
    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub(crate) fn sequence(&self) -> u32 {
        match &self.commit {
            DatagramCommit::Message { sequence, .. }
            | DatagramCommit::Acknowledgements { sequence, .. }
            | DatagramCommit::Retransmission { sequence, .. } => *sequence,
        }
    }

    pub(crate) fn is_acknowledgement(&self) -> bool {
        matches!(self.commit, DatagramCommit::Acknowledgements { .. })
    }

    pub(crate) fn is_retransmission(&self) -> bool {
        matches!(self.commit, DatagramCommit::Retransmission { .. })
    }
}

#[cfg_attr(test, derive(Clone))]
enum DatagramCommit {
    Message { sequence: u32, delivery: Delivery, appended_acks: Vec<u32>, ping: Option<u8> },
    Acknowledgements { sequence: u32, acknowledgements: Vec<u32> },
    Retransmission { sequence: u32, sent_at: Instant, retries: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReceiveError {
    DuplicateHistoryFull,
    AcknowledgementCapacityFull,
}

impl fmt::Display for ReceiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateHistoryFull => formatter.write_str("Reliable packet duplicate history is full."),
            Self::AcknowledgementCapacityFull => formatter.write_str("Reliable packet acknowledgement capacity is full."),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Delivery {
    Unreliable,
    Reliable,
    ReliableNoRetry,
}

impl Delivery {
    fn is_reliable(self) -> bool {
        !matches!(self, Self::Unreliable)
    }

    pub(crate) fn tracks_reliable(self) -> bool {
        matches!(self, Self::Reliable)
    }
}

pub(crate) struct PendingPacket {
    pub(crate) bytes: Vec<u8>,
    pub(crate) sent_at: Instant,
    pub(crate) retries: u8,
}

fn available_ping_id(next_ping_id: u8, pending_pings: &HashMap<u8, Instant>, quarantined_ping_ids: &HashMap<u8, Instant>) -> Option<u8> {
    (0..=u8::MAX)
        .map(|offset| next_ping_id.wrapping_add(offset))
        .find(|ping_id| !pending_pings.contains_key(ping_id) && !quarantined_ping_ids.contains_key(ping_id))
}

pub(crate) fn advance_sequence(sequence: u32) -> u32 {
    sequence.wrapping_add(1)
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

fn oldest_unacked_sequence(next_sequence: u32, pending: &HashMap<u32, PendingPacket>) -> u32 {
    pending.keys().copied().max_by_key(|sequence| sequence_age(next_sequence, *sequence)).unwrap_or_else(|| next_sequence.wrapping_sub(1))
}

pub(crate) fn sequence_age(next_sequence: u32, sequence: u32) -> u32 {
    next_sequence.wrapping_sub(sequence)
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

fn compare_received_sequence(sequence: u32, reference: u32, wide_sequence_seen: bool) -> SequenceOrdering {
    if wide_sequence_seen || sequence > SEQUENCE_24_MASK || reference > SEQUENCE_24_MASK {
        return compare_sequence(sequence, reference);
    }

    let forward = sequence.wrapping_sub(reference) & SEQUENCE_24_MASK;
    let backward = reference.wrapping_sub(sequence) & SEQUENCE_24_MASK;
    match forward {
        0 => SequenceOrdering::Equal,
        distance if distance == SEQUENCE_24_HALF_RANGE => SequenceOrdering::Ambiguous,
        distance if distance < SEQUENCE_24_HALF_RANGE => SequenceOrdering::Newer(distance),
        _ => SequenceOrdering::Older(backward),
    }
}

#[cfg(test)]
#[path = "tests/lludp.rs"]
mod tests;
