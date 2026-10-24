use std::{
    mem::size_of,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use protocol::lludp::{MAX_PACKET_RETAINED_CAPACITY_BYTES, Packet, PacketPreflight};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::{
    circuit::MAX_SIMULATOR_CONNECTIONS,
    event::{EventEnvelope, EventRetention},
};

pub(crate) const MAX_CONCURRENT_PACKET_DECODES: usize = 8;
pub(crate) const MAX_QUEUED_SIMULATOR_EVENT_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_QUEUED_NEIGHBOR_EVENT_BYTES: usize = 24 * 1024 * 1024;
pub(crate) const MAX_NEIGHBOR_CONNECTION_EVENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_PACKET_RETENTION_PERMITS: usize = 3;
pub(crate) const MAX_PACKET_ACK_DECODE_BYTES: usize = 32 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ConnectionRole {
    Primary,
    Neighbor,
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
pub(crate) struct SimulatorMemoryBudget {
    pub(crate) decode_slots: Arc<Semaphore>,
    ack_decode_slots: Arc<Semaphore>,
    shared: Arc<SharedMemoryBudget>,
    connection: Arc<Mutex<ConnectionUsage>>,
}

impl SimulatorMemoryBudget {
    pub(crate) fn new() -> Self {
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

    pub(crate) fn for_neighbor(&self) -> Self {
        Self {
            decode_slots: Arc::clone(&self.decode_slots),
            ack_decode_slots: Arc::clone(&self.ack_decode_slots),
            shared: Arc::clone(&self.shared),
            connection: Arc::new(Mutex::new(ConnectionUsage { role: ConnectionRole::Neighbor, bytes: 0 })),
        }
    }

    pub(crate) fn promote_to(&self, promoted: &Self) {
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

    pub(crate) fn try_acquire_ack_decode_slot(&self, preflight: PacketPreflight) -> Option<OwnedSemaphorePermit> {
        if preflight.peak_decode_allocation_bytes() > MAX_PACKET_ACK_DECODE_BYTES {
            return None;
        }
        Arc::clone(&self.ack_decode_slots).try_acquire_owned().ok()
    }

    pub(crate) fn reserve_packet(&self, preflight: PacketPreflight) -> Result<PacketMemoryReservation, PacketMemoryError> {
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

    pub(crate) fn trim_packet_reservation(
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
}

pub(crate) struct PacketMemoryReservation {
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

    pub(crate) fn into_retention(self) -> EventRetention {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PacketMemoryError {
    PacketTooLarge,
    GlobalBudget,
    NeighborBudget,
    ConnectionBudget,
    CounterOverflow,
    BoundExceeded,
}

pub(super) fn packet_event_overhead_bytes() -> usize {
    size_of::<EventEnvelope>() + MAX_PACKET_RETENTION_PERMITS * size_of::<OwnedSemaphorePermit>()
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn subtract_bytes(counter: &mut usize, bytes: usize) {
    debug_assert!(*counter >= bytes, "admission byte accounting underflow");
    *counter = counter.saturating_sub(bytes);
}

#[cfg(test)]
pub(crate) mod test_values {
    use super::{ConnectionRole, MemoryLimits, PacketMemoryError, PacketMemoryReservation, SimulatorMemoryBudget, lock_unpoisoned};

    pub(crate) fn with_test_limits(global: usize, neighbors: usize, neighbor_connection: usize) -> SimulatorMemoryBudget {
        SimulatorMemoryBudget::with_limits(MemoryLimits { global, neighbors, primary_connection: global, neighbor_connection })
    }

    pub(crate) fn reserve_bytes_for_test(
        budget: &SimulatorMemoryBudget,
        bytes: usize,
    ) -> Result<PacketMemoryReservation, PacketMemoryError> {
        budget.reserve_bytes(bytes, bytes)
    }

    pub(crate) fn global_available_bytes(budget: &SimulatorMemoryBudget) -> usize {
        let shared = lock_unpoisoned(&budget.shared.usage);
        budget.shared.limits.global.saturating_sub(shared.global)
    }

    pub(crate) fn neighbor_available_bytes(budget: &SimulatorMemoryBudget) -> usize {
        let shared = lock_unpoisoned(&budget.shared.usage);
        budget.shared.limits.neighbors.saturating_sub(shared.neighbors)
    }

    pub(crate) fn connection_available_bytes(budget: &SimulatorMemoryBudget) -> usize {
        let _shared = lock_unpoisoned(&budget.shared.usage);
        let connection = lock_unpoisoned(&budget.connection);
        let limit = match connection.role {
            ConnectionRole::Primary => budget.shared.limits.primary_connection,
            ConnectionRole::Neighbor => budget.shared.limits.neighbor_connection,
        };
        limit.saturating_sub(connection.bytes)
    }
}

#[cfg(test)]
#[path = "tests/admission.rs"]
mod tests;
