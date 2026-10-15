//! Bounded loss accounting for simulator state sources.

use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::watch;

use crate::SimulatorId;

const MAX_COVERAGE_SCOPES: usize = 256;

/// Identifies an ordered source of simulator state within a network session.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CoverageSource {
    /// Messages admitted from the simulator's LLUDP circuit.
    Lludp,
    /// Messages admitted by one EventQueue poll-task incarnation.
    EventQueue { incarnation: u64 },
}

/// Source-local progress attached to an admitted event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CoverageWatermark {
    pub simulator_id: SimulatorId,
    pub source: CoverageSource,
    /// The latest loss generation from this source when the event was admitted.
    pub generation: u64,
    /// The latest session-wide ledger-overflow generation when the event was admitted.
    pub overflow_generation: u64,
}

/// Why an application item from a source could not be admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageLossReason {
    DecodeAdmission,
    /// Packet framing or fields failed allocation preflight.
    PreflightRejected,
    /// The decoded-memory limit or supported allocation arithmetic was exceeded.
    DecodeLimit,
    /// Decoded storage exceeded its admitted allocation estimate, an internal defect.
    AllocationEstimateMismatch,
    DecodeRejected,
    RetainedMemoryAdmission,
    AcknowledgementAdmission,
    EventAdmission,
    EventQueueAdmission,
}

/// A coalesced, unacknowledged loss from one source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageLossRecord {
    pub watermark: CoverageWatermark,
    /// Oldest source-local generation not yet acknowledged in this scope.
    pub(crate) pending_from_generation: u64,
    pub reason: CoverageLossReason,
    pub retired: bool,
}

/// A loss that must be reconciled before covered events are applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoverageLoss {
    Source(CoverageLossRecord),
    /// The bounded ledger could not preserve a per-source loss record.
    /// The viewer must treat session coverage as uncertain and recover conservatively.
    SessionOverflow {
        generation: u64,
    },
}

/// Read-only view of the losses currently awaiting acknowledgement.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CoverageSnapshot {
    pub losses: Vec<CoverageLoss>,
    pub overflow_generation: Option<u64>,
    /// Latest session-overflow generation already acknowledged by the consumer.
    pub(crate) acknowledged_overflow_generation: u64,
    pub(crate) overflow_exhausted: bool,
}

/// Result of recording one source-local loss.
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

/// Shared, bounded loss accounting owned by one Network session.
///
/// Source losses coalesce to the latest generation per simulator/source scope. If the bounded
/// scope table fills, the ledger records a session-wide overflow generation instead of dropping
/// the fact that coverage was lost. Changes wake subscribers independently of the event queue.
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

    /// Returns the current source and ledger-overflow generations for an admitted event.
    pub(crate) fn watermark(&self, simulator_id: SimulatorId, source: CoverageSource) -> CoverageWatermark {
        let state = self.lock();
        let scope = Scope { simulator_id, source };
        let generation = state.scopes.iter().find(|entry| entry.scope == scope).map_or(0, |entry| entry.latest_generation);
        CoverageWatermark { simulator_id, source, generation, overflow_generation: state.overflow_generation }
    }

    /// Records an admission loss and wakes subscribers, even when the event queue is full.
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

    /// Returns pending losses without changing the ledger.
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

    /// Acknowledges only the source-local generations observed by the caller.
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

    /// Acknowledges only the session-overflow generation observed by the caller.
    pub fn acknowledge_overflow(&self, generation: u64) {
        let mut state = self.lock();
        if !state.overflow_exhausted && generation <= state.overflow_generation {
            state.acknowledged_overflow_generation = state.acknowledged_overflow_generation.max(generation);
        }
        drop(state);
        self.notify();
    }

    /// Marks a source as retired, retaining any unacknowledged loss until it is acknowledged.
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

    /// Subscribes to ledger changes independently of the bounded event queue.
    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.inner.changed.subscribe()
    }

    fn lock(&self) -> MutexGuard<'_, LedgerState> {
        self.inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn notify(&self) {
        self.inner.changed.send_modify(|revision| *revision = revision.saturating_add(1));
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
    use crate::SimulatorId;

    #[tokio::test]
    async fn newer_generation_survives_old_ack() {
        let ledger = CoverageLossLedger::new();
        let simulator_id = SimulatorId::new(1);
        let source = CoverageSource::Lludp;
        let mut changed = ledger.subscribe();

        let first = ledger.record(simulator_id, source, CoverageLossReason::EventAdmission);
        let second = ledger.record(simulator_id, source, CoverageLossReason::DecodeAdmission);
        let (CoverageRecord::Recorded(first), CoverageRecord::Recorded(second)) = (first, second) else { panic!("scope was recorded") };
        changed.changed().await.expect("loss wakes subscriber");
        assert_eq!(*changed.borrow_and_update(), 2, "watch wake coalesces both updates without losing the latest ledger state");
        ledger.acknowledge(first);

        assert!(matches!(
            ledger.snapshot().losses.as_slice(),
            [CoverageLoss::Source(loss)] if loss.watermark.generation == second.generation && loss.reason == CoverageLossReason::DecodeAdmission
        ));
    }

    #[test]
    fn retired_scope_waits_for_ack_then_releases_its_slot() {
        let ledger = CoverageLossLedger::new();
        let simulator_id = SimulatorId::new(2);
        let source = CoverageSource::Lludp;
        let CoverageRecord::Recorded(loss) = ledger.record(simulator_id, source, CoverageLossReason::EventAdmission) else {
            panic!("first scope has capacity")
        };

        ledger.retire(simulator_id, source);
        assert!(matches!(ledger.snapshot().losses.as_slice(), [CoverageLoss::Source(loss)] if loss.retired));
        ledger.acknowledge(loss);
        assert!(ledger.snapshot().losses.is_empty());
        assert_eq!(ledger.watermark(simulator_id, source).generation, 0);
    }

    #[test]
    fn scope_capacity_exhaustion_is_a_durable_overflow_fact() {
        let ledger = CoverageLossLedger::new();
        for id in 1..=256 {
            assert!(matches!(
                ledger.record(SimulatorId::new(id), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
                CoverageRecord::Recorded(_)
            ));
        }
        assert!(matches!(
            ledger.record(SimulatorId::new(257), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
            CoverageRecord::Overflowed { generation: 1 }
        ));
        assert_eq!(ledger.snapshot().overflow_generation, Some(1));
        ledger.acknowledge_overflow(1);
        assert_eq!(ledger.snapshot().overflow_generation, None);
        assert!(matches!(
            ledger.record(SimulatorId::new(258), CoverageSource::Lludp, CoverageLossReason::EventAdmission),
            CoverageRecord::Overflowed { generation: 2 }
        ));
    }
}
