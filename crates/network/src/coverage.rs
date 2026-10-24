use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::watch;

use crate::{SimulatorId, event::EventEnvelope};

const MAX_COVERAGE_SCOPES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CoverageSource {
    Lludp,
    EventQueue { incarnation: u64 },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CoverageWatermark {
    pub simulator_id: SimulatorId,
    pub source: CoverageSource,
    pub generation: u64,
    pub overflow_generation: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageLossReason {
    DecodeAdmission,
    PreflightRejected,
    DecodeLimit,
    AllocationEstimateMismatch,
    DecodeRejected,
    RetainedMemoryAdmission,
    AcknowledgementAdmission,
    EventAdmission,
    EventQueueAdmission,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageLossRecord {
    pub watermark: CoverageWatermark,
    pub(crate) pending_from_generation: u64,
    pub reason: CoverageLossReason,
    pub retired: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageLoss {
    Source(CoverageLossRecord),
    SessionOverflow { generation: u64 },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoverageSnapshot {
    pub losses: Vec<CoverageLoss>,
    pub overflow_generation: Option<u64>,
    pub(crate) acknowledged_overflow_generation: u64,
    pub(crate) overflow_exhausted: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CoverageRecord {
    Recorded(CoverageWatermark),
    Overflowed { generation: u64 },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Scope {
    simulator_id: SimulatorId,
    source: CoverageSource,
}

#[derive(Clone, Copy)]
struct ScopeState {
    scope: Scope,
    latest_generation: u64,
    acknowledged_generation: u64,
    latest_reason: CoverageLossReason,
    latest_overflow_generation: u64,
    retired: bool,
}

#[derive(Default)]
struct LedgerState {
    scopes: Vec<ScopeState>,
    overflow_generation: u64,
    acknowledged_overflow_generation: u64,
    overflow_exhausted: bool,
}

struct LedgerInner {
    state: Mutex<LedgerState>,
    changed: watch::Sender<u64>,
}

#[derive(Clone)]
pub struct CoverageLossLedger {
    inner: Arc<LedgerInner>,
}

impl Default for CoverageLossLedger {
    fn default() -> Self {
        Self::new()
    }
}

impl CoverageLossLedger {
    pub fn new() -> Self {
        let (changed, _) = watch::channel(0);
        Self { inner: Arc::new(LedgerInner { state: Mutex::new(LedgerState::default()), changed }) }
    }

    pub(crate) fn watermark(&self, simulator_id: SimulatorId, source: CoverageSource) -> CoverageWatermark {
        let state = self.lock();
        let scope = Scope { simulator_id, source };
        let generation = state.scopes.iter().find(|entry| entry.scope == scope).map_or(0, |entry| entry.latest_generation);
        CoverageWatermark { simulator_id, source, generation, overflow_generation: state.overflow_generation }
    }

    pub(crate) fn record(&self, simulator_id: SimulatorId, source: CoverageSource, reason: CoverageLossReason) -> CoverageRecord {
        let mut state = self.lock();
        let scope = Scope { simulator_id, source };
        let outcome = if let Some(index) = state.scopes.iter().position(|entry| entry.scope == scope) {
            let Some(generation) = state.scopes[index].latest_generation.checked_add(1) else {
                let generation = advance_overflow(&mut state);
                drop(state);
                self.notify();
                return CoverageRecord::Overflowed { generation };
            };
            let overflow_generation = state.overflow_generation;
            let entry = &mut state.scopes[index];
            entry.latest_generation = generation;
            entry.latest_reason = reason;
            entry.latest_overflow_generation = overflow_generation;
            CoverageRecord::Recorded(CoverageWatermark { simulator_id, source, generation, overflow_generation: state.overflow_generation })
        } else if state.scopes.len() < MAX_COVERAGE_SCOPES {
            let overflow_generation = state.overflow_generation;
            state.scopes.push(ScopeState {
                scope,
                latest_generation: 1,
                acknowledged_generation: 0,
                latest_reason: reason,
                latest_overflow_generation: overflow_generation,
                retired: false,
            });
            CoverageRecord::Recorded(CoverageWatermark {
                simulator_id,
                source,
                generation: 1,
                overflow_generation: state.overflow_generation,
            })
        } else {
            CoverageRecord::Overflowed { generation: advance_overflow(&mut state) }
        };
        drop(state);
        self.notify();
        outcome
    }

    pub fn snapshot(&self) -> CoverageSnapshot {
        let state = self.lock();
        let losses = state
            .scopes
            .iter()
            .filter(|entry| entry.latest_generation > entry.acknowledged_generation)
            .map(|entry| {
                CoverageLoss::Source(CoverageLossRecord {
                    watermark: CoverageWatermark {
                        simulator_id: entry.scope.simulator_id,
                        source: entry.scope.source,
                        generation: entry.latest_generation,
                        overflow_generation: entry.latest_overflow_generation,
                    },
                    pending_from_generation: entry.acknowledged_generation.saturating_add(1),
                    reason: entry.latest_reason,
                    retired: entry.retired,
                })
            })
            .collect();
        let overflow_generation = (state.overflow_exhausted || state.overflow_generation > state.acknowledged_overflow_generation)
            .then_some(state.overflow_generation);
        CoverageSnapshot {
            losses,
            overflow_generation,
            acknowledged_overflow_generation: state.acknowledged_overflow_generation,
            overflow_exhausted: state.overflow_exhausted,
        }
    }

    pub fn acknowledge(&self, watermark: CoverageWatermark) {
        let mut state = self.lock();
        let scope = Scope { simulator_id: watermark.simulator_id, source: watermark.source };
        if let Some(index) = state.scopes.iter().position(|entry| entry.scope == scope) {
            let entry = &mut state.scopes[index];
            if watermark.generation <= entry.latest_generation {
                entry.acknowledged_generation = entry.acknowledged_generation.max(watermark.generation);
            }
            if entry.retired && entry.latest_generation <= entry.acknowledged_generation {
                state.scopes.swap_remove(index);
            }
        }
        drop(state);
        self.notify();
    }

    pub fn acknowledge_overflow(&self, generation: u64) {
        let mut state = self.lock();
        if !state.overflow_exhausted && generation <= state.overflow_generation {
            state.acknowledged_overflow_generation = state.acknowledged_overflow_generation.max(generation);
        }
        drop(state);
        self.notify();
    }

    pub(crate) fn retire(&self, simulator_id: SimulatorId, source: CoverageSource) {
        let mut state = self.lock();
        let scope = Scope { simulator_id, source };
        if let Some(index) = state.scopes.iter().position(|entry| entry.scope == scope) {
            let entry = &mut state.scopes[index];
            entry.retired = true;
            if entry.latest_generation <= entry.acknowledged_generation {
                state.scopes.swap_remove(index);
            }
        }
        drop(state);
        self.notify();
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.changed.subscribe()
    }

    fn lock(&self) -> MutexGuard<'_, LedgerState> {
        self.inner.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn notify(&self) {
        self.inner.changed.send_modify(|revision| *revision = revision.saturating_add(1));
    }
}

pub(crate) fn record_coverage_loss(
    ledger: &CoverageLossLedger,
    simulator_id: SimulatorId,
    source: CoverageSource,
    reason: CoverageLossReason,
) {
    let _ = ledger.record(simulator_id, source, reason);
}

pub(crate) fn record_event_envelope_loss(ledger: &CoverageLossLedger, event: EventEnvelope, reason: CoverageLossReason) {
    if let Some(watermark) = event.coverage_watermark() {
        record_coverage_loss(ledger, watermark.simulator_id, watermark.source, reason);
    }
}

fn advance_overflow(state: &mut LedgerState) -> u64 {
    if let Some(generation) = state.overflow_generation.checked_add(1) {
        state.overflow_generation = generation;
    } else {
        state.overflow_exhausted = true;
    }
    state.overflow_generation
}

#[cfg(test)]
mod tests {
    use super::{CoverageLoss, CoverageLossLedger, CoverageLossReason, CoverageRecord, CoverageSource};
    use crate::fixtures::test_simulator_id;

    #[tokio::test]
    async fn newer_generation_survives_old_ack() {
        let ledger = CoverageLossLedger::new();
        let simulator_id = test_simulator_id(1);
        let source = CoverageSource::Lludp;
        let mut changed = ledger.subscribe();

        let first = ledger.record(simulator_id, source, CoverageLossReason::EventAdmission);
        let second = ledger.record(simulator_id, source, CoverageLossReason::DecodeAdmission);
        let (CoverageRecord::Recorded(first), CoverageRecord::Recorded(second)) = (first, second) else { panic!("scope was recorded") };
        changed.changed().await.expect("loss wakes subscriber");
        assert_eq!(*changed.borrow_and_update(), 2, "watch wake coalesces both updates without losing the latest ledger state");
        ledger.acknowledge(first);

        std::assert_matches!(
            ledger.snapshot().losses.as_slice(),
            [CoverageLoss::Source(loss)] if loss.watermark.generation == second.generation && loss.reason == CoverageLossReason::DecodeAdmission
        );
    }

    #[test]
    fn retired_scope_waits_for_ack_then_releases_its_slot() {
        let ledger = CoverageLossLedger::new();
        let simulator_id = test_simulator_id(2);
        let source = CoverageSource::Lludp;
        let CoverageRecord::Recorded(loss) = ledger.record(simulator_id, source, CoverageLossReason::EventAdmission) else {
            panic!("first scope has capacity")
        };

        ledger.retire(simulator_id, source);
        std::assert_matches!(ledger.snapshot().losses.as_slice(), [CoverageLoss::Source(loss)] if loss.retired);
        ledger.acknowledge(loss);
        assert!(ledger.snapshot().losses.is_empty());
        assert_eq!(ledger.watermark(simulator_id, source).generation, 0);
    }

    #[test]
    fn scope_capacity_exhaustion_is_a_durable_overflow_fact() {
        let ledger = CoverageLossLedger::new();
        for id in 1..=256 {
            std::assert_matches!(
                ledger.record(test_simulator_id(id), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
                CoverageRecord::Recorded(_)
            );
        }
        std::assert_matches!(
            ledger.record(test_simulator_id(257), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
            CoverageRecord::Overflowed { generation: 1 }
        );
        assert_eq!(ledger.snapshot().overflow_generation, Some(1));
        ledger.acknowledge_overflow(1);
        assert_eq!(ledger.snapshot().overflow_generation, None);
        std::assert_matches!(
            ledger.record(test_simulator_id(258), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
            CoverageRecord::Overflowed { generation: 2 }
        );
    }
}
