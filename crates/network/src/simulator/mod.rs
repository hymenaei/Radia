//! Internal simulator coordination and connection ownership.

mod admission;
mod coordinator;

pub(super) const MAX_NEIGHBOR_SIMULATORS: usize = 32;
pub(super) const MAX_NEIGHBOR_CONNECTION_TASKS: usize = MAX_NEIGHBOR_SIMULATORS * 2;
/// One primary plus every neighbor connection task can decode a control ACK concurrently.
pub(super) const MAX_SIMULATOR_CONNECTIONS: usize = MAX_NEIGHBOR_CONNECTION_TASKS + 1;

#[derive(Clone, Copy, Eq, PartialEq)]
enum ConnectionRole {
    Primary,
    Neighbor,
}

pub(crate) use admission::EventRetention;
pub(crate) use coordinator::SimulatorCoordinator;

#[cfg(test)]
pub(crate) use coordinator::run_session;
