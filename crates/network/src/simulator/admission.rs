use std::{
    mem::size_of,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use protocol::lludp::{MAX_PACKET_RETAINED_CAPACITY_BYTES, Packet, PacketPreflight};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::event::EventEnvelope;

use super::{ConnectionRole, MAX_SIMULATOR_CONNECTIONS};

pub(super) const MAX_CONCURRENT_PACKET_DECODES: usize = 8;
pub(super) const MAX_QUEUED_SIMULATOR_EVENT_BYTES: usize = 32 * 1024 * 1024;
pub(super) const MAX_QUEUED_NEIGHBOR_EVENT_BYTES: usize = 24 * 1024 * 1024;
pub(super) const MAX_NEIGHBOR_CONNECTION_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PACKET_RETENTION_PERMITS: usize = 3;

pub(super) const MAX_PACKET_ACK_DECODE_BYTES: usize = 32 * 1024;

pub(crate) struct EventRetention {
    _charge: EventRetentionCharge,
}

enum EventRetentionCharge {
    Permits { _permits: Vec<OwnedSemaphorePermit> },
    Packet { _reservation: PacketMemoryReservation },
}

impl EventRetention {
    pub(crate) fn from_permits(permits: Vec<OwnedSemaphorePermit>) -> Self {
        Self { _charge: EventRetentionCharge::Permits { _permits: permits } }
    }

    fn from_packet_reservation(reservation: PacketMemoryReservation) -> Self {
        Self { _charge: EventRetentionCharge::Packet { _reservation: reservation } }
    }
}

#[derive(Clone, Copy)]
struct MemoryLimits {
    global: usize,
    neighbors: usize,
    primary_connection: usize,
    neighbor_connection: usize,
}

impl MemoryLimits {
    const DEFAULT: Self = Self {
        global: MAX_QUEUED_SIMULATOR_EVENT_BYTES,
        neighbors: MAX_QUEUED_NEIGHBOR_EVENT_BYTES,
        primary_connection: MAX_QUEUED_SIMULATOR_EVENT_BYTES,
        neighbor_connection: MAX_NEIGHBOR_CONNECTION_EVENT_BYTES,
    };
}

#[derive(Default)]
struct SharedUsage {
    global: usize,
    neighbors: usize,
}

struct SharedMemoryBudget {
    limits: MemoryLimits,
    usage: Mutex<SharedUsage>,
}

struct ConnectionUsage {
    role: ConnectionRole,
    bytes: usize,
}

#[derive(Clone)]
pub(super) struct SimulatorMemoryBudget {
    pub(super) decode_slots: Arc<Semaphore>,
    ack_decode_slots: Arc<Semaphore>,
    shared: Arc<SharedMemoryBudget>,
    connection: Arc<Mutex<ConnectionUsage>>,
}

impl SimulatorMemoryBudget {
    pub(super) fn new() -> Self {
        Self::with_limits(MemoryLimits::DEFAULT)
    }

    fn with_limits(limits: MemoryLimits) -> Self {
        Self {
            decode_slots: Arc::new(Semaphore::new(MAX_CONCURRENT_PACKET_DECODES)),
            ack_decode_slots: Arc::new(Semaphore::new(MAX_SIMULATOR_CONNECTIONS)),
            shared: Arc::new(SharedMemoryBudget { limits, usage: Mutex::new(SharedUsage::default()) }),
            connection: Arc::new(Mutex::new(ConnectionUsage { role: ConnectionRole::Primary, bytes: 0 })),
        }
    }

    #[cfg(test)]
    pub(super) fn with_test_limits(global: usize, neighbors: usize, neighbor_connection: usize) -> Self {
        Self::with_limits(MemoryLimits { global, neighbors, primary_connection: global, neighbor_connection })
    }

    pub(super) fn for_neighbor(&self) -> Self {
        Self {
            decode_slots: Arc::clone(&self.decode_slots),
            ack_decode_slots: Arc::clone(&self.ack_decode_slots),
            shared: Arc::clone(&self.shared),
            connection: Arc::new(Mutex::new(ConnectionUsage { role: ConnectionRole::Neighbor, bytes: 0 })),
        }
    }

    pub(super) fn promote_to(&self, promoted: &Self) {
        if Arc::ptr_eq(&self.connection, &promoted.connection) {
            return;
        }
        assert!(Arc::ptr_eq(&self.shared, &promoted.shared), "simulator memory budgets must share session accounting");

        let mut shared = lock_unpoisoned(&self.shared.usage);
        let mut former = lock_unpoisoned(&self.connection);
        let mut promoted = lock_unpoisoned(&promoted.connection);

        if former.role == ConnectionRole::Neighbor && promoted.role == ConnectionRole::Primary {
            return;
        }
        assert!(former.role == ConnectionRole::Primary && promoted.role == ConnectionRole::Neighbor);

        subtract_bytes(&mut shared.neighbors, promoted.bytes);
        shared.neighbors = shared.neighbors.checked_add(former.bytes).expect("neighbor retained byte count fits usize");
        former.role = ConnectionRole::Neighbor;
        promoted.role = ConnectionRole::Primary;
    }

    pub(super) fn try_acquire_decode_slot(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.decode_slots).try_acquire_owned().ok()
    }

    pub(super) fn try_acquire_ack_decode_slot(&self, preflight: PacketPreflight) -> Option<OwnedSemaphorePermit> {
        if preflight.peak_decode_allocation_bytes() > MAX_PACKET_ACK_DECODE_BYTES {
            return None;
        }
        Arc::clone(&self.ack_decode_slots).try_acquire_owned().ok()
    }

    pub(super) fn reserve_packet(&self, preflight: PacketPreflight) -> Result<PacketMemoryReservation, PacketMemoryError> {
        let retained_capacity_upper_bound = preflight
            .retained_capacity_upper_bound()
            .checked_add(packet_event_overhead_bytes())
            .ok_or(PacketMemoryError::CounterOverflow)?;
        let bytes = preflight
            .peak_decode_allocation_bytes()
            .checked_add(packet_event_overhead_bytes())
            .ok_or(PacketMemoryError::CounterOverflow)?;
        if preflight.retained_capacity_upper_bound() > MAX_PACKET_RETAINED_CAPACITY_BYTES {
            return Err(PacketMemoryError::PacketTooLarge);
        }
        self.reserve_bytes(bytes, retained_capacity_upper_bound)
    }

    pub(super) fn trim_packet_reservation(
        &self,
        reservation: &mut PacketMemoryReservation,
        packet: &Packet,
    ) -> Result<(), PacketMemoryError> {
        let retained_bytes =
            packet.retained_capacity_bytes().checked_add(packet_event_overhead_bytes()).ok_or(PacketMemoryError::CounterOverflow)?;
        if retained_bytes > reservation.retained_capacity_upper_bound {
            return Err(PacketMemoryError::BoundExceeded);
        }
        if retained_bytes > reservation.bytes {
            return Err(PacketMemoryError::BoundExceeded);
        }
        reservation.reduce_to(retained_bytes);
        Ok(())
    }

    fn reserve_bytes(&self, bytes: usize, retained_capacity_upper_bound: usize) -> Result<PacketMemoryReservation, PacketMemoryError> {
        let mut shared = lock_unpoisoned(&self.shared.usage);
        let mut connection = lock_unpoisoned(&self.connection);

        let global = shared.global.checked_add(bytes).ok_or(PacketMemoryError::CounterOverflow)?;
        if global > self.shared.limits.global {
            return Err(PacketMemoryError::GlobalBudget);
        }
        let neighbors = if connection.role == ConnectionRole::Neighbor {
            let neighbors = shared.neighbors.checked_add(bytes).ok_or(PacketMemoryError::CounterOverflow)?;
            if neighbors > self.shared.limits.neighbors {
                return Err(PacketMemoryError::NeighborBudget);
            }
            Some(neighbors)
        } else {
            None
        };
        let connection_limit = match connection.role {
            ConnectionRole::Primary => self.shared.limits.primary_connection,
            ConnectionRole::Neighbor => self.shared.limits.neighbor_connection,
        };
        let connection_bytes = connection.bytes.checked_add(bytes).ok_or(PacketMemoryError::CounterOverflow)?;
        if connection_bytes > connection_limit {
            return Err(PacketMemoryError::ConnectionBudget);
        }

        shared.global = global;
        if let Some(neighbors) = neighbors {
            shared.neighbors = neighbors;
        }
        connection.bytes = connection_bytes;
        drop(connection);
        drop(shared);

        Ok(PacketMemoryReservation {
            shared: Arc::clone(&self.shared),
            connection: Arc::clone(&self.connection),
            bytes,
            retained_capacity_upper_bound,
        })
    }

    #[cfg(test)]
    pub(super) fn reserve_bytes_for_test(&self, bytes: usize) -> Result<PacketMemoryReservation, PacketMemoryError> {
        self.reserve_bytes(bytes, bytes)
    }

    #[cfg(test)]
    pub(super) fn global_available_bytes(&self) -> usize {
        let shared = lock_unpoisoned(&self.shared.usage);
        self.shared.limits.global.saturating_sub(shared.global)
    }

    #[cfg(test)]
    pub(super) fn neighbor_available_bytes(&self) -> usize {
        let shared = lock_unpoisoned(&self.shared.usage);
        self.shared.limits.neighbors.saturating_sub(shared.neighbors)
    }

    #[cfg(test)]
    pub(super) fn connection_available_bytes(&self) -> usize {
        let _shared = lock_unpoisoned(&self.shared.usage);
        let connection = lock_unpoisoned(&self.connection);
        let limit = match connection.role {
            ConnectionRole::Primary => self.shared.limits.primary_connection,
            ConnectionRole::Neighbor => self.shared.limits.neighbor_connection,
        };
        limit.saturating_sub(connection.bytes)
    }
}

pub(super) fn packet_event_overhead_bytes() -> usize {
    size_of::<EventEnvelope>() + MAX_PACKET_RETENTION_PERMITS * size_of::<OwnedSemaphorePermit>()
}

pub(super) struct PacketMemoryReservation {
    shared: Arc<SharedMemoryBudget>,
    connection: Arc<Mutex<ConnectionUsage>>,
    bytes: usize,
    retained_capacity_upper_bound: usize,
}

impl PacketMemoryReservation {
    fn reduce_to(&mut self, bytes: usize) {
        let released = self.bytes - bytes;
        let mut shared = lock_unpoisoned(&self.shared.usage);
        let mut connection = lock_unpoisoned(&self.connection);
        subtract_bytes(&mut shared.global, released);
        if connection.role == ConnectionRole::Neighbor {
            subtract_bytes(&mut shared.neighbors, released);
        }
        subtract_bytes(&mut connection.bytes, released);
        self.bytes = bytes;
    }

    pub(super) fn into_retention(self) -> EventRetention {
        EventRetention::from_packet_reservation(self)
    }
}

impl Drop for PacketMemoryReservation {
    fn drop(&mut self) {
        let mut shared = lock_unpoisoned(&self.shared.usage);
        let mut connection = lock_unpoisoned(&self.connection);
        subtract_bytes(&mut shared.global, self.bytes);
        if connection.role == ConnectionRole::Neighbor {
            subtract_bytes(&mut shared.neighbors, self.bytes);
        }
        subtract_bytes(&mut connection.bytes, self.bytes);
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn subtract_bytes(counter: &mut usize, bytes: usize) {
    debug_assert!(*counter >= bytes, "admission byte accounting underflow");
    *counter = counter.saturating_sub(bytes);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PacketMemoryError {
    PacketTooLarge,
    GlobalBudget,
    NeighborBudget,
    ConnectionBudget,
    CounterOverflow,
    BoundExceeded,
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use crate::event::{Event, EventEnvelope, PacketOrder::Ordered, SimulatorId};
    use ::test_support::{lludp::vectors, parse_hex_fixture};
    use protocol::{
        lludp::{Message, MessageId, Packet},
        messages::{PacketAck, circuit::packet_ack::Packets},
    };
    use tokio::sync::mpsc;

    use super::{
        MAX_NEIGHBOR_CONNECTION_EVENT_BYTES, MAX_PACKET_ACK_DECODE_BYTES, MAX_QUEUED_NEIGHBOR_EVENT_BYTES,
        MAX_QUEUED_SIMULATOR_EVENT_BYTES, PacketMemoryError, SimulatorMemoryBudget, packet_event_overhead_bytes,
    };
    fn fixture() -> Vec<u8> {
        parse_hex_fixture(vectors::CHAT_FROM_SIMULATOR).expect("valid simulator chat fixture")
    }

    #[test]
    fn ack_decode_slots_are_bounded() {
        let budget = SimulatorMemoryBudget::new();
        let mut ack =
            Packet::new(1, Message::PacketAck(PacketAck { packets: (0..u8::MAX).map(|id| Packets { id: u32::from(id) }).collect() }))
                .with_extra_header(vec![0x42; usize::from(u8::MAX)])
                .with_appended_acks(vec![7; usize::from(u8::MAX)])
                .with_zero_coding(false)
                .encode()
                .expect("encode maximum-sized ACK control packet");
        let appended_ack_bytes = usize::from(u8::MAX) * size_of::<u32>() + 1;
        let ack_section_start = ack.len() - appended_ack_bytes;
        let padding_len = 8 * 1024 - ack.len();
        ack.splice(ack_section_start..ack_section_start, vec![0x55; padding_len]);
        assert_eq!(ack.len(), 8 * 1024, "the packet fills the protocol datagram limit");
        let preflight = Packet::preflight(&ack).expect("preflight maximum-sized ACK control packet");
        assert!(preflight.peak_decode_allocation_bytes() <= MAX_PACKET_ACK_DECODE_BYTES);

        let mut permits = (0..super::super::MAX_SIMULATOR_CONNECTIONS)
            .map(|_| budget.try_acquire_ack_decode_slot(preflight).expect("each active simulator has one control decode slot"))
            .collect::<Vec<_>>();
        assert!(budget.try_acquire_ack_decode_slot(preflight).is_none(), "the control decode budget has no extra slot");
        drop(permits.pop());
        assert!(budget.try_acquire_ack_decode_slot(preflight).is_some(), "dropping a control decode permit restores capacity");
    }

    fn simulator_address() -> SocketAddr {
        "127.0.0.1:13000".parse().expect("simulator address")
    }

    #[tokio::test]
    async fn retained_packet_memory_lives_until_dequeue() {
        let budget = SimulatorMemoryBudget::new();
        let wire = fixture();
        let preflight = Packet::preflight(&wire).expect("preflight simulator message");
        let mut reservation = budget.reserve_packet(preflight).expect("reserve in-flight memory");
        let packet = Packet::decode(&wire).expect("decode simulator message");
        budget.trim_packet_reservation(&mut reservation, &packet).expect("trim reservation to retained capacity");
        let retained_bytes = packet.retained_capacity_bytes() + packet_event_overhead_bytes();
        let event = EventEnvelope::with_retention(
            Event::SimulatorMessage {
                simulator_id: SimulatorId::allocate().expect("allocate simulator ID"),
                source: simulator_address(),
                sequence: packet.sequence(),
                packet_order: Ordered(1),
                message: packet.into_message(),
            },
            reservation.into_retention(),
        );
        let (events, mut received) = mpsc::channel(1);

        events.send(event).await.expect("queue simulator event");
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES - retained_bytes);

        assert!(matches!(received.recv().await.map(EventEnvelope::into_event), Some(Event::SimulatorMessage { .. })));
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    }

    #[test]
    fn rejected_admission_releases_bytes() {
        let budget = SimulatorMemoryBudget::new();
        let preflight = Packet::preflight(&fixture()).expect("preflight simulator message");
        let full_budget = budget.reserve_bytes_for_test(MAX_QUEUED_SIMULATOR_EVENT_BYTES).expect("reserve full event budget");

        assert!(matches!(budget.reserve_packet(preflight), Err(PacketMemoryError::GlobalBudget)));
        assert_eq!(budget.global_available_bytes(), 0);
        assert_eq!(budget.connection_available_bytes(), 0);

        drop(full_budget);
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
    }

    #[test]
    fn neighbors_preserve_primary_headroom() {
        let preflight = Packet::preflight(&fixture()).expect("preflight simulator message");
        let packet_bytes = preflight.peak_decode_allocation_bytes() + packet_event_overhead_bytes();
        let primary = SimulatorMemoryBudget::with_test_limits(packet_bytes * 10, packet_bytes * 4, packet_bytes * 2);
        let neighbors = [primary.for_neighbor(), primary.for_neighbor()];
        let mut reservations = Vec::new();
        for neighbor in &neighbors {
            for _ in 0..2 {
                reservations.push(neighbor.reserve_packet(preflight).expect("reserve neighbor packet up to per-connection limit"));
            }
        }
        let remaining = primary.neighbor_available_bytes();
        assert_eq!(remaining, 0, "neighbors should use the aggregate limit exactly");
        assert_eq!(primary.global_available_bytes(), packet_bytes * 6);
        assert!(matches!(primary.for_neighbor().reserve_packet(preflight), Err(PacketMemoryError::NeighborBudget)));
        drop(primary.reserve_packet(preflight).expect("primary can use protected headroom"));
        drop(reservations);
        assert_eq!(primary.global_available_bytes(), packet_bytes * 10);
        assert_eq!(primary.neighbor_available_bytes(), packet_bytes * 4);
        for neighbor in neighbors {
            assert_eq!(neighbor.connection_available_bytes(), packet_bytes * 2);
        }
    }

    #[test]
    fn promotion_reclassifies_retained_memory() {
        let root = SimulatorMemoryBudget::with_test_limits(1_000, 500, 200);
        let former_primary = root.clone();
        let promoted = root.for_neighbor();
        let other_neighbor = root.for_neighbor();
        let former_backlog = former_primary.reserve_bytes_for_test(520).expect("primary may retain a larger backlog");
        let promoted_backlog = promoted.reserve_bytes_for_test(80).expect("neighbor may retain its backlog");
        let other_backlog = other_neighbor.reserve_bytes_for_test(200).expect("another neighbor fills its connection allowance");

        assert_eq!(root.global_available_bytes(), 200);
        assert_eq!(root.neighbor_available_bytes(), 220);

        former_primary.promote_to(&promoted);

        assert_eq!(root.neighbor_available_bytes(), 0, "over-limit retained usage stays charged after demotion");
        assert_eq!(former_primary.connection_available_bytes(), 0, "a demoted connection cannot keep its former primary allowance");
        assert_eq!(promoted.connection_available_bytes(), 920, "promoted retained bytes move to the primary connection allowance");
        assert!(matches!(
            former_primary.reserve_bytes_for_test(1),
            Err(PacketMemoryError::NeighborBudget | PacketMemoryError::ConnectionBudget)
        ));
        assert!(matches!(other_neighbor.reserve_bytes_for_test(1), Err(PacketMemoryError::NeighborBudget)));

        let new_primary = promoted.reserve_bytes_for_test(100).expect("promoted primary can use remaining global headroom");
        assert_eq!(root.global_available_bytes(), 100);
        drop(former_backlog);
        assert_eq!(root.global_available_bytes(), 620);
        assert_eq!(root.neighbor_available_bytes(), 300, "dropping former-primary retention releases its new neighbor charge");
        drop(promoted_backlog);
        drop(new_primary);
        assert_eq!(root.neighbor_available_bytes(), 300, "primary retention does not remain charged to the neighbor pool");
        drop(other_backlog);
        assert_eq!(root.global_available_bytes(), 1_000);
        assert_eq!(root.neighbor_available_bytes(), 500);
    }

    #[test]
    fn demoted_retention_releases_after_handle_drop() {
        let root = SimulatorMemoryBudget::with_test_limits(1_000, 500, 200);
        let former = root.clone();
        let promoted = root.for_neighbor();
        let retained = former.reserve_bytes_for_test(200).expect("primary may retain its packet");

        former.promote_to(&promoted);
        drop(former);
        drop(root);

        assert_eq!(promoted.neighbor_available_bytes(), 300);
        drop(retained);
        assert_eq!(promoted.neighbor_available_bytes(), 500);
    }

    #[test]
    fn repeated_promotions_reclassify_retained_memory() {
        let root = SimulatorMemoryBudget::with_test_limits(2_000, 600, 400);
        let first = root.clone();
        let second = root.for_neighbor();
        let third = root.for_neighbor();
        let first_backlog = first.reserve_bytes_for_test(500).expect("primary may retain its backlog");
        let second_backlog = second.reserve_bytes_for_test(100).expect("neighbor may retain its backlog");
        let third_backlog = third.reserve_bytes_for_test(200).expect("neighbor may retain its backlog");

        first.promote_to(&second);
        assert_eq!(root.neighbor_available_bytes(), 0, "demoted primary bytes remain charged to neighbors");
        second.promote_to(&third);

        assert_eq!(root.neighbor_available_bytes(), 0, "each promotion reclassifies both connections' retained bytes");
        assert_eq!(first.connection_available_bytes(), 0, "the oldest demoted connection loses primary headroom");
        assert_eq!(second.connection_available_bytes(), 300, "the former promoted connection gets neighbor headroom");
        assert_eq!(third.connection_available_bytes(), 1_800, "the latest primary keeps its own retained bytes");
        assert!(matches!(second.reserve_bytes_for_test(1), Err(PacketMemoryError::NeighborBudget)));

        drop(first_backlog);
        assert_eq!(root.neighbor_available_bytes(), 500, "releasing old-primary retention restores aggregate neighbor capacity");
        let second_extra = second.reserve_bytes_for_test(300).expect("demoted connection can use its neighbor allowance");
        assert_eq!(root.neighbor_available_bytes(), 200);

        drop(second_backlog);
        drop(second_extra);
        drop(third_backlog);
        assert_eq!(root.global_available_bytes(), 2_000);
        assert_eq!(root.neighbor_available_bytes(), 600);
    }

    #[test]
    fn in_flight_decode_trims_after_promotion() {
        let root = SimulatorMemoryBudget::with_test_limits(1_000_000, 500_000, 250_000);
        let former_primary = root.clone();
        let promoted = root.for_neighbor();
        let wire = fixture();
        let preflight = Packet::preflight(&wire).expect("preflight simulator message");
        let peak_bytes = preflight.peak_decode_allocation_bytes() + packet_event_overhead_bytes();
        let mut reservation = former_primary.reserve_packet(preflight).expect("reserve in-flight decode");
        let packet = Packet::decode(&wire).expect("decode simulator message");
        let retained_bytes = packet.retained_capacity_bytes() + packet_event_overhead_bytes();
        assert!(retained_bytes < peak_bytes, "fixture must release temporary decode headroom");

        former_primary.promote_to(&promoted);

        assert_eq!(root.neighbor_available_bytes(), 500_000 - peak_bytes);
        assert_eq!(former_primary.connection_available_bytes(), 250_000 - peak_bytes);
        assert_eq!(promoted.connection_available_bytes(), 1_000_000);

        former_primary
            .trim_packet_reservation(&mut reservation, &packet)
            .expect("trim reservation using the demoted connection's new role");
        assert_eq!(root.global_available_bytes(), 1_000_000 - retained_bytes);
        assert_eq!(root.neighbor_available_bytes(), 500_000 - retained_bytes);
        assert_eq!(former_primary.connection_available_bytes(), 250_000 - retained_bytes);
        assert_eq!(promoted.connection_available_bytes(), 1_000_000);

        drop(reservation);
        assert_eq!(root.global_available_bytes(), 1_000_000);
        assert_eq!(root.neighbor_available_bytes(), 500_000);
        assert_eq!(former_primary.connection_available_bytes(), 250_000);
    }

    #[test]
    fn neighbor_limits_are_independent() {
        let primary = SimulatorMemoryBudget::with_test_limits(500, 400, 200);
        let saturated = primary.for_neighbor();
        let available = primary.for_neighbor();
        let full_connection = saturated.reserve_bytes_for_test(200).expect("occupy one neighbor connection budget");
        assert!(matches!(saturated.reserve_bytes_for_test(1), Err(PacketMemoryError::ConnectionBudget)));
        assert_eq!(primary.global_available_bytes(), 300);
        assert_eq!(saturated.neighbor_available_bytes(), 200);
        drop(available.reserve_bytes_for_test(200).expect("other neighbor has independent capacity"));

        drop(full_connection);
        assert_eq!(saturated.connection_available_bytes(), 200);
        assert_eq!(primary.neighbor_available_bytes(), 400);
    }

    #[test]
    fn decode_failure_releases_packet_budgets() {
        let budget = SimulatorMemoryBudget::new().for_neighbor();
        let packet = Packet::new(1, Message::PacketAck(PacketAck { packets: vec![Packets { id: 1 }] }))
            .encode()
            .expect("encode acknowledgement packet");
        let mut truncated = packet;
        truncated.truncate(truncated.len() - 4);
        let preflight = Packet::preflight(&truncated).expect("preflight count before truncated ACK body");
        let reservation = budget.reserve_packet(preflight).expect("reserve malformed but bounded decode");
        assert!(Packet::decode(&truncated).is_err(), "typed decoder rejects missing ACK body");
        drop(reservation);
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);

        drop(budget.reserve_packet(preflight).expect("reserve next packet"));
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    }

    #[test]
    fn underestimated_packet_releases_memory() {
        let budget = SimulatorMemoryBudget::new().for_neighbor();
        use protocol::messages::{CompletePingCheck, PingId};

        let small = Packet::new(1, Message::CompletePingCheck(CompletePingCheck { ping_id: PingId { ping_id: 1 } }))
            .encode()
            .expect("encode small packet");
        let bound = Packet::preflight(&small).expect("preflight small packet");
        let large = Packet::decode(&fixture()).expect("decode larger packet");
        let mut reservation = budget.reserve_packet(bound).expect("reserve smaller estimate");
        assert_eq!(budget.trim_packet_reservation(&mut reservation, &large), Err(PacketMemoryError::BoundExceeded));
        drop(large);
        drop(reservation);
        assert_eq!(budget.global_available_bytes(), MAX_QUEUED_SIMULATOR_EVENT_BYTES);
        assert_eq!(budget.neighbor_available_bytes(), MAX_QUEUED_NEIGHBOR_EVENT_BYTES);
        assert_eq!(budget.connection_available_bytes(), MAX_NEIGHBOR_CONNECTION_EVENT_BYTES);
    }

    #[test]
    fn temporary_decode_headroom_does_not_hide_a_retained_bound_mismatch() {
        let small = Packet::new(1, Message::Unknown { id: MessageId::low(0xfff9), payload: vec![0; 4] })
            .with_zero_coding(true)
            .encode()
            .expect("encode zero-coded packet with smaller retained payload");
        let preflight = Packet::preflight(&small).expect("preflight packet");
        let larger = Packet::new(2, Message::Unknown { id: MessageId::low(0xfff9), payload: vec![0x55; 6] })
            .encode()
            .expect("encode packet with a slightly larger retained payload");
        let packet = Packet::decode(&larger).expect("decode small retained payload");
        let retained = packet.retained_capacity_bytes();

        assert!(
            retained > preflight.retained_capacity_upper_bound(),
            "retained={retained}, bound={}",
            preflight.retained_capacity_upper_bound()
        );
        assert!(
            retained <= preflight.peak_decode_allocation_bytes(),
            "retained={retained}, peak={}",
            preflight.peak_decode_allocation_bytes()
        );

        let budget = SimulatorMemoryBudget::new();
        let mut reservation = budget.reserve_packet(preflight).expect("reserve the conservative peak bound");
        assert_eq!(budget.trim_packet_reservation(&mut reservation, &packet), Err(PacketMemoryError::BoundExceeded));
    }
}
