//! Session coverage and semantic recovery status.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

use network::{CoverageLossReason, CoverageWatermark, SimulatorId};
use tokio::sync::watch;
use world::{LocalAddress, WorldRevision};

pub(super) const MAX_SESSION_COVERAGE_SCOPES: usize = 256;
pub(super) const MAX_SESSION_RECOVERY_SCOPES: usize = 65_536;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoverageHealth {
    /// No detected delivery loss or unresolved semantic recovery is retained.
    /// This does not assert that the simulator sent a complete scene snapshot.
    NoKnownLoss,
    /// Bounded semantic recovery requests are still outstanding.
    Uncertain,
    /// A detected loss or recovery failure has no verified repair path.
    Degraded,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RecoveryProgress {
    pub(crate) pending: usize,
    pub(crate) in_flight: usize,
    pub(crate) degraded: usize,
    pub(crate) dropped: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SourceCoverage {
    pub(crate) watermark: CoverageWatermark,
    pub(crate) reason: CoverageLossReason,
    pub(crate) retired: bool,
    pub(crate) health: CoverageHealth,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct RecoveryKey {
    pub(super) address: LocalAddress,
    pub(super) kind: RecoveryKind,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum RecoveryKind {
    Object,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RecoveryHealth {
    Pending,
    InFlight,
    Degraded,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CoverageSnapshot {
    /// The World revision observed with this coverage state.
    pub(crate) world_revision: WorldRevision,
    /// Session-local publication generation, independent of source watermarks.
    pub(crate) generation: u64,
    pub(crate) health: CoverageHealth,
    pub(crate) losses: Vec<SourceCoverage>,
    pub(crate) network_overflow_generation: Option<u64>,
    pub(crate) local_overflow_count: u64,
    pub(crate) recovery: RecoveryProgress,
}

#[derive(Clone)]
pub(super) struct SessionCoverageState {
    state: Arc<Mutex<CoverageState>>,
    changed: watch::Sender<u64>,
}

#[derive(Default)]
struct CoverageState {
    generation: u64,
    losses: Vec<SourceCoverage>,
    network_overflow_generation: Option<u64>,
    local_overflow_count: u64,
    recovery_dropped: u64,
    recoveries: HashMap<RecoveryKey, RecoveryEntry>,
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

impl Default for SessionCoverageState {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCoverageState {
    pub(super) fn new() -> Self {
        let (changed, _) = watch::channel(0);
        Self { state: Arc::new(Mutex::new(CoverageState::default())), changed }
    }

    pub(super) fn subscribe(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub(super) fn record_source_loss(&self, watermark: CoverageWatermark, reason: CoverageLossReason, retired: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = false;
        if watermark.overflow_generation > 0
            && state.network_overflow_generation.is_none_or(|current| watermark.overflow_generation > current)
        {
            state.network_overflow_generation = Some(watermark.overflow_generation);
            changed = true;
        }
        let incoming = SourceCoverage { watermark, reason, retired, health: CoverageHealth::Degraded };
        if let Some(index) = state.losses.iter().position(|current| same_scope(current.watermark, watermark)) {
            let current = &mut state.losses[index];
            if watermark.generation > current.watermark.generation {
                *current = incoming;
                changed = true;
            } else if watermark.generation == current.watermark.generation && retired && !current.retired {
                current.retired = true;
                changed = true;
            }
        } else if state.losses.len() < MAX_SESSION_COVERAGE_SCOPES {
            state.losses.push(incoming);
            changed = true;
        } else {
            state.local_overflow_count = state.local_overflow_count.saturating_add(1);
            changed = true;
        }
        if changed {
            state.changed(&self.changed);
        }
    }

    pub(super) fn record_network_overflow(&self, generation: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.network_overflow_generation.is_none_or(|current| generation > current) {
            state.network_overflow_generation = Some(generation);
            state.changed(&self.changed);
        }
    }

    pub(super) fn can_schedule_recovery(&self, key: RecoveryKey) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.recoveries.get(&key) {
            Some(current) => current.health != RecoveryHealth::Degraded,
            None => state.recoveries.len() < MAX_SESSION_RECOVERY_SCOPES,
        }
    }

    pub(super) fn recovery_is_degraded(&self, key: RecoveryKey) -> bool {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.recoveries.get(&key).is_some_and(|current| current.health == RecoveryHealth::Degraded)
    }

    pub(super) fn schedule_recovery(&self, key: RecoveryKey, simulator_id: SimulatorId, token: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(current) = state.recoveries.get(&key) {
            if current.token == token && current.simulator_id == Some(simulator_id) {
                return;
            }
            state.recoveries.insert(key, RecoveryEntry::new(Some(simulator_id), token, RecoveryHealth::Pending));
            state.changed(&self.changed);
            return;
        }
        if state.recoveries.len() >= MAX_SESSION_RECOVERY_SCOPES {
            state.recovery_dropped = state.recovery_dropped.saturating_add(1);
        } else {
            state.recoveries.insert(key, RecoveryEntry::new(Some(simulator_id), token, RecoveryHealth::Pending));
        }
        state.changed(&self.changed);
    }

    pub(super) fn start_recoveries(&self, keys: impl IntoIterator<Item = (RecoveryKey, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = false;
        for (key, token) in keys {
            if let Some(current) = state.recoveries.get_mut(&key)
                && current.token == token
                && current.health == RecoveryHealth::Pending
            {
                current.health = RecoveryHealth::InFlight;
                changed = true;
            }
        }
        if changed {
            state.changed(&self.changed);
        }
    }

    pub(super) fn retry_recoveries(&self, recoveries: impl IntoIterator<Item = (RecoveryKey, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = false;
        for (key, token) in recoveries {
            if let Some(current) = state.recoveries.get_mut(&key)
                && current.token == token
                && current.health != RecoveryHealth::Degraded
            {
                current.health = RecoveryHealth::Pending;
                changed = true;
            }
        }
        if changed {
            state.changed(&self.changed);
        }
    }

    pub(super) fn degrade_recoveries(&self, recoveries: impl IntoIterator<Item = (RecoveryKey, u64)>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut changed = false;
        for (key, token) in recoveries {
            if let Some(current) = state.recoveries.get_mut(&key)
                && current.token == token
                && current.health != RecoveryHealth::Degraded
            {
                current.health = RecoveryHealth::Degraded;
                changed = true;
            }
        }
        if changed {
            state.changed(&self.changed);
        }
    }

    pub(super) fn drop_recovery(&self, key: RecoveryKey, simulator_id: Option<SimulatorId>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.recovery_dropped = state.recovery_dropped.saturating_add(1);
        if let Some(current) = state.recoveries.get_mut(&key) {
            current.simulator_id = simulator_id;
            current.health = RecoveryHealth::Degraded;
        } else if state.recoveries.len() < MAX_SESSION_RECOVERY_SCOPES {
            state.recoveries.insert(key, RecoveryEntry::new(simulator_id, 0, RecoveryHealth::Degraded));
        } else {
            state.local_overflow_count = state.local_overflow_count.saturating_add(1);
        }
        state.changed(&self.changed);
    }

    pub(super) fn cancel_recovery(&self, key: RecoveryKey) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.recoveries.remove(&key).is_some() {
            state.changed(&self.changed);
        }
    }

    pub(super) fn cancel_simulator_recoveries(&self, simulator_id: SimulatorId) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let before = state.recoveries.len();
        state.recoveries.retain(|_, entry| entry.simulator_id != Some(simulator_id));
        if state.recoveries.len() != before {
            state.changed(&self.changed);
        }
    }

    pub(super) fn snapshot(&self, world_revision: WorldRevision) -> CoverageSnapshot {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let mut recovery = RecoveryProgress { dropped: state.recovery_dropped, ..RecoveryProgress::default() };
        for entry in state.recoveries.values() {
            match entry.health {
                RecoveryHealth::Pending => recovery.pending += 1,
                RecoveryHealth::InFlight => recovery.in_flight += 1,
                RecoveryHealth::Degraded => recovery.degraded += 1,
            }
        }
        let health = if !state.losses.is_empty()
            || state.network_overflow_generation.is_some()
            || state.local_overflow_count > 0
            || recovery.degraded > 0
            || recovery.dropped > 0
        {
            CoverageHealth::Degraded
        } else if recovery.pending > 0 || recovery.in_flight > 0 {
            CoverageHealth::Uncertain
        } else {
            CoverageHealth::NoKnownLoss
        };
        CoverageSnapshot {
            world_revision,
            generation: state.generation,
            health,
            losses: state.losses.clone(),
            network_overflow_generation: state.network_overflow_generation,
            local_overflow_count: state.local_overflow_count,
            recovery,
        }
    }
}

impl CoverageState {
    fn changed(&mut self, sender: &watch::Sender<u64>) {
        self.generation = self.generation.saturating_add(1);
        sender.send_replace(self.generation);
    }
}

fn same_scope(first: CoverageWatermark, second: CoverageWatermark) -> bool {
    first.simulator_id == second.simulator_id && first.source == second.source
}

#[cfg(test)]
mod tests {
    use std::{num::NonZeroU32, sync::PoisonError};

    use network::{CoverageLossReason, CoverageSource, CoverageWatermark, SimulatorId, testing::simulator_id};
    use world::{LocalAddress, Region, World, WorldRevision};

    use super::{CoverageHealth, MAX_SESSION_COVERAGE_SCOPES, RecoveryHealth, RecoveryKey, RecoveryKind, SessionCoverageState};
    use crate::session::world::viewer_world_limits;

    fn address(region: u64, local_id: u32) -> LocalAddress {
        LocalAddress::new(Region::new(region), NonZeroU32::new(local_id).expect("nonzero local ID"))
    }

    fn revision() -> WorldRevision {
        World::new(viewer_world_limits()).revision()
    }

    fn record_loss(coverage: &SessionCoverageState, simulator_id: SimulatorId, source: CoverageSource, generation: u64) {
        coverage.record_source_loss(
            CoverageWatermark { simulator_id, source, generation, overflow_generation: 0 },
            CoverageLossReason::DecodeRejected,
            false,
        );
    }

    fn recovery_health(coverage: &SessionCoverageState, key: RecoveryKey) -> RecoveryHealth {
        let state = coverage.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.recoveries.get(&key).expect("recovery entry").health
    }

    #[test]
    fn loss_scopes_keep_generations_separate() {
        let coverage = SessionCoverageState::new();
        let first = simulator_id();
        let second = simulator_id();
        record_loss(&coverage, first, CoverageSource::Lludp, 5);
        record_loss(&coverage, first, CoverageSource::EventQueue { incarnation: 1 }, 2);
        record_loss(&coverage, second, CoverageSource::Lludp, 1);

        let revision = revision();
        let snapshot = coverage.snapshot(revision);
        assert_eq!(snapshot.health, CoverageHealth::Degraded);
        assert_eq!(snapshot.world_revision, revision);
        assert_eq!(snapshot.losses.iter().map(|loss| loss.watermark.generation).collect::<Vec<_>>(), [5, 2, 1]);
    }

    #[test]
    fn recovery_changes_publish_without_world_revision() {
        let coverage = SessionCoverageState::new();
        let mut changes = coverage.subscribe();
        let revision = revision();
        let simulator_id = simulator_id();
        let key = RecoveryKey { address: address(7, 3), kind: RecoveryKind::Object };
        coverage.schedule_recovery(key, simulator_id, 1);
        let uncertain = coverage.snapshot(revision);
        assert_eq!(uncertain.health, CoverageHealth::Uncertain);
        assert_eq!(uncertain.recovery.pending, 1);
        assert_eq!(uncertain.world_revision, revision);
        assert_eq!(*changes.borrow_and_update(), uncertain.generation);

        coverage.start_recoveries([(key, 1)]);
        coverage.degrade_recoveries([(key, 1)]);
        let degraded = coverage.snapshot(revision);
        assert_eq!(degraded.health, CoverageHealth::Degraded);
        assert_eq!(degraded.recovery.in_flight, 0);
        assert_eq!(degraded.recovery.degraded, 1);
        assert_eq!(recovery_health(&coverage, key), RecoveryHealth::Degraded);
        assert!(degraded.generation > uncertain.generation);
    }

    #[test]
    fn sent_recovery_stays_pending() {
        let coverage = SessionCoverageState::new();
        let simulator_id = simulator_id();
        let key = RecoveryKey { address: address(7, 4), kind: RecoveryKind::Object };
        coverage.schedule_recovery(key, simulator_id, 1);
        coverage.start_recoveries([(key, 1)]);
        coverage.retry_recoveries([(key, 1)]);

        let snapshot = coverage.snapshot(revision());
        assert_eq!(snapshot.health, CoverageHealth::Uncertain);
        assert_eq!(snapshot.recovery.pending, 1);
        assert_eq!(recovery_health(&coverage, key), RecoveryHealth::Pending);
    }

    #[test]
    fn later_loss_survives_old_recovery_cancellation() {
        let coverage = SessionCoverageState::new();
        let simulator_id = simulator_id();
        let key = RecoveryKey { address: address(7, 5), kind: RecoveryKind::Object };
        let source = CoverageSource::EventQueue { incarnation: 1 };
        let watermark = |generation| CoverageWatermark { simulator_id, source, generation, overflow_generation: 0 };

        coverage.record_source_loss(watermark(4), CoverageLossReason::EventAdmission, false);
        coverage.schedule_recovery(key, simulator_id, 1);
        coverage.record_source_loss(watermark(5), CoverageLossReason::EventQueueAdmission, false);
        coverage.cancel_recovery(key);

        let snapshot = coverage.snapshot(revision());
        assert_eq!(snapshot.losses.len(), 1);
        assert_eq!(snapshot.losses[0].watermark.generation, 5);
        assert_eq!(snapshot.health, CoverageHealth::Degraded);
    }

    #[test]
    fn stale_recovery_completion_is_ignored() {
        let coverage = SessionCoverageState::new();
        let simulator_id = simulator_id();
        let key = RecoveryKey { address: address(7, 6), kind: RecoveryKind::Object };
        coverage.schedule_recovery(key, simulator_id, 1);
        coverage.start_recoveries([(key, 1)]);
        coverage.cancel_recovery(key);
        coverage.schedule_recovery(key, simulator_id, 2);

        coverage.retry_recoveries([(key, 1)]);
        coverage.degrade_recoveries([(key, 1)]);

        let snapshot = coverage.snapshot(revision());
        assert_eq!(snapshot.recovery.pending, 1);
        assert_eq!(snapshot.recovery.degraded, 0);
        assert_eq!(recovery_health(&coverage, key), RecoveryHealth::Pending);
    }

    #[test]
    fn retirement_cancels_the_current_route() {
        let coverage = SessionCoverageState::new();
        let source = simulator_id();
        let destination = simulator_id();
        let key = RecoveryKey { address: address(source.get(), 7), kind: RecoveryKind::Object };
        coverage.schedule_recovery(key, source, 1);
        coverage.schedule_recovery(key, destination, 2);

        coverage.cancel_simulator_recoveries(source);
        assert_eq!(coverage.snapshot(revision()).recovery.pending, 1);
        coverage.cancel_simulator_recoveries(destination);
        assert_eq!(coverage.snapshot(revision()).recovery.pending, 0);
    }

    #[test]
    fn event_queue_restart_keeps_old_loss() {
        let coverage = SessionCoverageState::new();
        let simulator_id = simulator_id();
        record_loss(&coverage, simulator_id, CoverageSource::EventQueue { incarnation: 1 }, 3);
        record_loss(&coverage, simulator_id, CoverageSource::EventQueue { incarnation: 2 }, 1);

        let snapshot = coverage.snapshot(revision());
        assert_eq!(snapshot.losses.len(), 2);
        assert_eq!(snapshot.losses[0].watermark.source, CoverageSource::EventQueue { incarnation: 1 });
        assert_eq!(snapshot.losses[1].watermark.source, CoverageSource::EventQueue { incarnation: 2 });
    }

    #[test]
    fn scope_overflow_is_degraded() {
        let coverage = SessionCoverageState::new();
        for _ in 0..=MAX_SESSION_COVERAGE_SCOPES {
            record_loss(&coverage, simulator_id(), CoverageSource::Lludp, 1);
        }

        let snapshot = coverage.snapshot(revision());
        assert_eq!(snapshot.losses.len(), MAX_SESSION_COVERAGE_SCOPES);
        assert_eq!(snapshot.local_overflow_count, 1);
        assert_eq!(snapshot.health, CoverageHealth::Degraded);
    }
}
