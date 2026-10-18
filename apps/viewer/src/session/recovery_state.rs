//! Session-local recovery routing state.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use network::SimulatorId;
use world::LocalAddress;

pub(super) const MAX_SESSION_RECOVERY_SCOPES: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RecoveryHealth {
    Pending,
    InFlight,
    Degraded,
}

#[derive(Clone)]
pub(super) struct RecoveryState {
    state: Arc<Mutex<RecoveryEntries>>,
}

#[derive(Default)]
struct RecoveryEntries {
    entries: HashMap<LocalAddress, RecoveryEntry>,
}

#[derive(Clone, Copy)]
struct RecoveryEntry {
    simulator_id: Option<SimulatorId>,
    token: u64,
    health: RecoveryHealth,
}

impl RecoveryEntry {
    fn new(simulator_id: Option<SimulatorId>, token: u64, health: RecoveryHealth) -> Self {
        Self { simulator_id, token, health }
    }
}

impl Default for RecoveryState {
    fn default() -> Self {
        Self::new()
    }
}

impl RecoveryState {
    pub(super) fn new() -> Self {
        Self { state: Arc::new(Mutex::new(RecoveryEntries::default())) }
    }

    pub(super) fn can_schedule(&self, address: LocalAddress) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.entries.get(&address) {
            Some(current) => current.health != RecoveryHealth::Degraded,
            None => state.entries.len() < MAX_SESSION_RECOVERY_SCOPES,
        }
    }

    pub(super) fn is_degraded(&self, address: LocalAddress) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.entries.get(&address).is_some_and(|current| current.health == RecoveryHealth::Degraded)
    }

    pub(super) fn schedule(&self, address: LocalAddress, simulator_id: SimulatorId, token: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(current) = state.entries.get(&address) {
            if current.token == token && current.simulator_id == Some(simulator_id) {
                return;
            }
            state.entries.insert(address, RecoveryEntry::new(Some(simulator_id), token, RecoveryHealth::Pending));
            return;
        }
        if state.entries.len() < MAX_SESSION_RECOVERY_SCOPES {
            state.entries.insert(address, RecoveryEntry::new(Some(simulator_id), token, RecoveryHealth::Pending));
        }
    }

    pub(super) fn start(&self, recoveries: impl IntoIterator<Item = (LocalAddress, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        for (address, token) in recoveries {
            if let Some(current) = state.entries.get_mut(&address)
                && current.token == token
                && current.health == RecoveryHealth::Pending
            {
                current.health = RecoveryHealth::InFlight;
            }
        }
    }

    pub(super) fn retry(&self, recoveries: impl IntoIterator<Item = (LocalAddress, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        for (address, token) in recoveries {
            if let Some(current) = state.entries.get_mut(&address)
                && current.token == token
                && current.health != RecoveryHealth::Degraded
            {
                current.health = RecoveryHealth::Pending;
            }
        }
    }

    pub(super) fn degrade(&self, recoveries: impl IntoIterator<Item = (LocalAddress, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        for (address, token) in recoveries {
            if let Some(current) = state.entries.get_mut(&address)
                && current.token == token
                && current.health != RecoveryHealth::Degraded
            {
                current.health = RecoveryHealth::Degraded;
            }
        }
    }

    pub(super) fn mark_degraded(&self, address: LocalAddress, simulator_id: Option<SimulatorId>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(current) = state.entries.get_mut(&address) {
            current.simulator_id = simulator_id;
            current.health = RecoveryHealth::Degraded;
        } else if state.entries.len() < MAX_SESSION_RECOVERY_SCOPES {
            state.entries.insert(address, RecoveryEntry::new(simulator_id, 0, RecoveryHealth::Degraded));
        }
    }

    pub(super) fn cancel(&self, address: LocalAddress) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.entries.remove(&address);
    }

    pub(super) fn cancel_simulator(&self, simulator_id: SimulatorId) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.entries.retain(|_, entry| entry.simulator_id != Some(simulator_id));
    }
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, sync::PoisonError};

    use network::testing::simulator_id;
    use world::{LocalAddress, Region};

    use super::{RecoveryHealth, RecoveryState};

    fn address(region: u64, local_id: u32) -> LocalAddress {
        LocalAddress::new(Region::new(region), NonZeroU32::new(local_id).expect("nonzero local ID"))
    }

    fn health(state: &RecoveryState, address: LocalAddress) -> RecoveryHealth {
        state.state.lock().unwrap_or_else(PoisonError::into_inner).entries.get(&address).expect("recovery entry").health
    }

    #[test]
    fn recovery_transitions_from_pending_to_degraded() {
        let state = RecoveryState::new();
        let simulator_id = simulator_id();
        let address = address(7, 3);
        state.schedule(address, simulator_id, 1);
        assert_eq!(health(&state, address), RecoveryHealth::Pending);

        state.start([(address, 1)]);
        assert_eq!(health(&state, address), RecoveryHealth::InFlight);

        state.retry([(address, 1)]);
        assert_eq!(health(&state, address), RecoveryHealth::Pending);

        state.start([(address, 1)]);
        state.degrade([(address, 1)]);
        assert_eq!(health(&state, address), RecoveryHealth::Degraded);
        assert!(!state.can_schedule(address));
        assert!(state.is_degraded(address));
    }

    #[test]
    fn stale_recovery_completion_does_not_change_replacement() {
        let state = RecoveryState::new();
        let simulator_id = simulator_id();
        let address = address(7, 6);
        state.schedule(address, simulator_id, 1);
        state.start([(address, 1)]);
        state.cancel(address);
        state.schedule(address, simulator_id, 2);

        state.retry([(address, 1)]);
        state.degrade([(address, 1)]);

        assert_eq!(health(&state, address), RecoveryHealth::Pending);
        assert!(!state.is_degraded(address));
    }

    #[test]
    fn simulator_retirement_cancels_only_its_current_routes() {
        let state = RecoveryState::new();
        let source = simulator_id();
        let destination = simulator_id();
        let address = address(source.get(), 7);
        state.schedule(address, source, 1);
        state.schedule(address, destination, 2);

        state.cancel_simulator(source);
        assert_eq!(health(&state, address), RecoveryHealth::Pending);

        state.cancel_simulator(destination);
        assert!(state.can_schedule(address));
    }

    #[test]
    fn dropped_recovery_is_not_rescheduled_until_object_changes() {
        let state = RecoveryState::new();
        let address = address(7, 8);
        state.mark_degraded(address, Some(simulator_id()));

        assert!(state.is_degraded(address));
        assert!(!state.can_schedule(address));

        state.cancel(address);
        assert!(state.can_schedule(address));
    }
}
