use std::{collections::HashMap, future::Future, mem::size_of, time::Duration};

use futures::{FutureExt, future::BoxFuture};
use network::{
    Delivery, Error,
    Error::{
        CommandQueueFull, SessionClosed, SimulatorCommandQueueFull, SimulatorConnectionClosed, SimulatorNotReady, SimulatorSend,
        SimulatorUnavailable,
    },
    SessionHandle, SimulatorId,
};
use protocol::lludp::{Message, Packet};
use protocol::messages::{CacheMissType::Full, ObjectRequest, RequestAgentData, RequestMultipleObjects};
use tokio::time::Instant;
use world::LocalAddress;

const RECOVERY_RETRY_INTERVAL: Duration = Duration::from_secs(5);
const RECOVERY_MAX_SENDS: u8 = 4;
const RECOVERY_IDLE_WAIT: Duration = Duration::from_secs(60);
const RECOVERY_REQUEST_INTERVAL: Duration = Duration::from_millis(100);
const RECOVERY_MAX_AGE: Duration = Duration::from_secs(60);
const RECOVERY_MAX_BACKOFF: Duration = Duration::from_secs(30);
const RECOVERY_OVERFLOW_LOG_INTERVAL: Duration = Duration::from_secs(5);
const MAX_RECOVERY_DIAGNOSTIC_ADDRESSES: usize = 8;
pub(super) const MAX_RECOVERY_OBJECTS_PER_REQUEST: usize = 255;
const MAX_RECOVERY_TARGETS: usize = 65_536;
const MAX_RECOVERY_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECOVERY_REQUEST_BYTES: usize = 2 * 1024;
const RECOVERY_ENTRY_BYTES: usize = size_of::<(LocalAddress, RecoveryEntry)>() + 64;

#[derive(Clone, Copy, Debug)]
pub(super) struct ScheduledRecovery {
    pub(super) key: LocalAddress,
    pub(super) simulator_id: SimulatorId,
    token: u64,
    agent_data: RequestAgentData,
    retry: RecoveryRetryState,
}

impl ScheduledRecovery {
    fn new(simulator_id: SimulatorId, token: u64, agent_data: RequestAgentData, address: LocalAddress, now: Instant) -> Self {
        Self { key: address, simulator_id, token, agent_data, retry: RecoveryRetryState::new(now) }
    }

    pub(super) fn is_exhausted(&self) -> bool {
        self.retry.is_exhausted()
    }
}

#[derive(Clone, Copy, Debug)]
struct RecoveryRetryState {
    sends: u8,
    deadline: Instant,
    created_at: Instant,
    failures: u8,
}

impl RecoveryRetryState {
    fn new(now: Instant) -> Self {
        Self { sends: 0, deadline: now, created_at: now, failures: 0 }
    }

    fn is_due(self, now: Instant) -> bool {
        self.deadline <= now
    }

    fn is_exhausted(self) -> bool {
        self.sends >= RECOVERY_MAX_SENDS
    }

    fn is_expired(self, now: Instant) -> bool {
        now.saturating_duration_since(self.created_at) >= RECOVERY_MAX_AGE
    }

    fn mark_sent(&mut self, now: Instant) {
        self.sends += 1;
        self.deadline = now + RECOVERY_RETRY_INTERVAL;
        self.failures = 0;
    }

    fn mark_failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1);
        let exponent = u32::from(self.failures.saturating_sub(1).min(4));
        let delay = RECOVERY_RETRY_INTERVAL.saturating_mul(1u32 << exponent).min(RECOVERY_MAX_BACKOFF);
        self.deadline = now + delay;
    }
}

#[derive(Clone, Copy)]
enum RecoveryEntry {
    Pending(ScheduledRecovery),
    InFlight(ScheduledRecovery),
    Degraded { simulator_id: Option<SimulatorId> },
}

impl RecoveryEntry {
    fn simulator_id(&self) -> Option<SimulatorId> {
        match self {
            Self::Pending(recovery) | Self::InFlight(recovery) => Some(recovery.simulator_id),
            Self::Degraded { simulator_id } => *simulator_id,
        }
    }
}

pub(super) struct RecoveryScheduler {
    entries: HashMap<LocalAddress, RecoveryEntry>,
    max_targets: usize,
    max_bytes: usize,
    next_token: u64,
    dropped_since_warning: usize,
    last_overflow_warning: Option<Instant>,
}

impl RecoveryScheduler {
    pub(super) fn new() -> Self {
        Self::with_limits(MAX_RECOVERY_TARGETS, MAX_RECOVERY_BYTES)
    }

    fn with_limits(max_targets: usize, max_bytes: usize) -> Self {
        Self { entries: HashMap::new(), max_targets, max_bytes, next_token: 0, dropped_since_warning: 0, last_overflow_warning: None }
    }

    fn has_capacity(&self) -> bool {
        self.entries.len() < self.max_targets && self.entries.len().saturating_add(1).saturating_mul(RECOVERY_ENTRY_BYTES) <= self.max_bytes
    }

    fn is_degraded(&self, address: LocalAddress) -> bool {
        matches!(self.entries.get(&address), Some(RecoveryEntry::Degraded { .. }))
    }

    fn schedule(&mut self, simulator_id: SimulatorId, agent_data: RequestAgentData, address: LocalAddress, now: Instant) -> Option<u64> {
        let has_capacity = self.has_capacity();
        match self.entries.get_mut(&address) {
            Some(RecoveryEntry::Degraded { .. }) => return None,
            Some(RecoveryEntry::Pending(current) | RecoveryEntry::InFlight(current)) if current.simulator_id == simulator_id => {
                current.agent_data = agent_data;
                return Some(current.token);
            }
            None if !has_capacity => return None,
            _ => {}
        }
        let token = self.next_token.checked_add(1)?;
        self.next_token = token;
        self.entries.insert(address, RecoveryEntry::Pending(ScheduledRecovery::new(simulator_id, token, agent_data, address, now)));
        Some(token)
    }

    fn mark_degraded(&mut self, address: LocalAddress, simulator_id: Option<SimulatorId>) {
        if self.entries.contains_key(&address) || self.has_capacity() {
            self.entries.insert(address, RecoveryEntry::Degraded { simulator_id });
        }
    }

    pub(super) fn cancel_simulator(&mut self, simulator_id: SimulatorId) {
        self.entries.retain(|_, entry| entry.simulator_id() != Some(simulator_id));
    }

    fn degrade_simulator(&mut self, simulator_id: SimulatorId) {
        for entry in self.entries.values_mut() {
            if entry.simulator_id() == Some(simulator_id) {
                *entry = RecoveryEntry::Degraded { simulator_id: Some(simulator_id) };
            }
        }
    }

    fn cancel(&mut self, address: LocalAddress) {
        self.entries.remove(&address);
    }

    fn next_deadline(&self) -> Option<Instant> {
        self.entries
            .values()
            .filter_map(|entry| match entry {
                RecoveryEntry::Pending(recovery) => Some(recovery.retry.deadline.min(recovery.retry.created_at + RECOVERY_MAX_AGE)),
                _ => None,
            })
            .min()
    }

    fn take_due(&mut self, now: Instant) -> Vec<ScheduledRecovery> {
        let Some(simulator_id) = self.entries.values().find_map(|entry| match entry {
            RecoveryEntry::Pending(recovery) if recovery.retry.is_due(now) && !recovery.retry.is_expired(now) => {
                Some(recovery.simulator_id)
            }
            _ => None,
        }) else {
            return Vec::new();
        };
        self.entries
            .values_mut()
            .filter_map(|entry| {
                let RecoveryEntry::Pending(recovery) = *entry else { return None };
                if recovery.simulator_id != simulator_id || !recovery.retry.is_due(now) || recovery.retry.is_expired(now) {
                    return None;
                }
                *entry = RecoveryEntry::InFlight(recovery);
                Some(recovery)
            })
            .take(MAX_RECOVERY_OBJECTS_PER_REQUEST)
            .collect()
    }

    fn take_expired(&mut self, now: Instant) -> Vec<ScheduledRecovery> {
        self.entries
            .values_mut()
            .filter_map(|entry| {
                let RecoveryEntry::Pending(recovery) = *entry else { return None };
                if !recovery.retry.is_expired(now) {
                    return None;
                }
                *entry = RecoveryEntry::Degraded { simulator_id: Some(recovery.simulator_id) };
                Some(recovery)
            })
            .take(MAX_RECOVERY_OBJECTS_PER_REQUEST)
            .collect()
    }

    fn mark_sent(&mut self, recovery: ScheduledRecovery, now: Instant) {
        if let Some(entry) = self.entries.get_mut(&recovery.key)
            && let RecoveryEntry::InFlight(mut current) = *entry
            && current.token == recovery.token
        {
            current.retry.mark_sent(now);
            *entry = RecoveryEntry::Pending(current);
        }
    }

    fn degrade(&mut self, recovery: ScheduledRecovery) {
        if let Some(entry) = self.entries.get_mut(&recovery.key)
            && matches!(entry, RecoveryEntry::Pending(current) | RecoveryEntry::InFlight(current) if current.token == recovery.token)
        {
            *entry = RecoveryEntry::Degraded { simulator_id: Some(recovery.simulator_id) };
        }
    }

    fn defer(&mut self, recovery: ScheduledRecovery, now: Instant) {
        if let Some(entry) = self.entries.get_mut(&recovery.key)
            && let RecoveryEntry::InFlight(mut current) = *entry
            && current.token == recovery.token
        {
            current.retry.mark_failed(now);
            *entry = RecoveryEntry::Pending(current);
        }
    }

    fn record_overflow(&mut self, dropped: usize, now: Instant) -> Option<usize> {
        self.dropped_since_warning = self.dropped_since_warning.saturating_add(dropped);
        let should_warn = self
            .last_overflow_warning
            .is_none_or(|last_warning| now.saturating_duration_since(last_warning) >= RECOVERY_OVERFLOW_LOG_INTERVAL);
        if !should_warn {
            return None;
        }
        self.last_overflow_warning = Some(now);
        Some(std::mem::take(&mut self.dropped_since_warning))
    }
}

pub(super) enum RecoveryCompletion {
    Deadline { aged_out: Vec<ScheduledRecovery>, due: Vec<ScheduledRecovery> },
    Send { recoveries: Vec<ScheduledRecovery>, result: Result<(), Error>, completed_at: Instant },
}

pub(super) struct OversizedRecoveryRequest {
    pub(super) recoveries: Vec<ScheduledRecovery>,
}

struct PendingRecoverySend {
    recoveries: Vec<ScheduledRecovery>,
    future: BoxFuture<'static, Result<(), Error>>,
}

pub(super) struct RecoveryIntake {
    scheduler: RecoveryScheduler,
    next_send_at: Option<Instant>,
    in_flight: Option<PendingRecoverySend>,
}

impl RecoveryIntake {
    pub(super) fn new() -> Self {
        Self { scheduler: RecoveryScheduler::new(), next_send_at: None, in_flight: None }
    }

    pub(super) fn schedule(
        &mut self,
        targets: Vec<(Option<SimulatorId>, LocalAddress)>,
        updated: Vec<LocalAddress>,
        agent_data: Option<RequestAgentData>,
    ) {
        if targets.is_empty() && updated.is_empty() {
            return;
        }
        let simulator = targets.first().and_then(|(simulator, _)| simulator.map(SimulatorId::get));
        let region = targets.first().map(|(_, address)| address.region().get());
        let now = Instant::now();
        let mut dropped = 0;
        for address in updated {
            self.scheduler.cancel(address);
        }
        for (simulator_id, address) in targets {
            if self.scheduler.is_degraded(address) {
                continue;
            }
            let Some(simulator_id) = simulator_id else {
                self.scheduler.mark_degraded(address, None);
                dropped += 1;
                continue;
            };
            let Some(agent_data) = agent_data else {
                self.scheduler.mark_degraded(address, Some(simulator_id));
                dropped += 1;
                continue;
            };
            match self.scheduler.schedule(simulator_id, agent_data, address, now) {
                Some(_) => {}
                None => {
                    self.scheduler.mark_degraded(address, Some(simulator_id));
                    dropped += 1;
                }
            }
        }
        if dropped > 0
            && let Some(dropped) = self.scheduler.record_overflow(dropped, now)
        {
            tracing::warn!(simulator, region, dropped, "some object recovery targets could not be queued");
        }
    }

    pub(super) fn cancel_simulator(&mut self, simulator_id: SimulatorId) {
        self.scheduler.cancel_simulator(simulator_id);
    }

    pub(super) fn cancel(&mut self) {
        self.in_flight = None;
        self.scheduler.entries.clear();
    }

    pub(super) async fn next_completion(&mut self) -> RecoveryCompletion {
        if self.in_flight.is_some() {
            let result = self.in_flight.as_mut().expect("in-flight recovery send").future.as_mut().await;
            let completed_at = Instant::now();
            let send = self.in_flight.take().expect("in-flight recovery send");
            self.next_send_at = Some(completed_at + RECOVERY_REQUEST_INTERVAL);
            return RecoveryCompletion::Send { recoveries: send.recoveries, result, completed_at };
        }

        let now = Instant::now();
        let deadline = self
            .scheduler
            .next_deadline()
            .map(|deadline| self.next_send_at.map_or(deadline, |send_at| deadline.max(send_at)))
            .unwrap_or(now + RECOVERY_IDLE_WAIT);
        tokio::time::sleep_until(deadline).await;
        let now = Instant::now();
        RecoveryCompletion::Deadline { aged_out: self.scheduler.take_expired(now), due: self.scheduler.take_due(now) }
    }

    pub(super) fn start_send(
        &mut self,
        session_handle: &SessionHandle,
        recoveries: Vec<ScheduledRecovery>,
    ) -> Result<(), OversizedRecoveryRequest> {
        let Some(first) = recoveries.first().copied() else { return Ok(()) };
        assert!(self.in_flight.is_none(), "recovery sends are serialized");
        let simulator_id = first.simulator_id;
        let addresses = recoveries.iter().map(|recovery| recovery.key).collect::<Vec<_>>();
        let request = recovery_request(first.agent_data, &addresses);
        let packet_size = Packet::new(0, Message::RequestMultipleObjects(request.clone())).encode().map(|packet| packet.len());
        if !packet_size.is_ok_and(|size| size <= MAX_RECOVERY_REQUEST_BYTES) {
            return Err(OversizedRecoveryRequest { recoveries });
        }
        let session_handle = session_handle.clone();
        self.start_send_future(recoveries, async move { session_handle.send_to(simulator_id, request, Delivery::Reliable).await });
        Ok(())
    }

    pub(super) fn mark_sent(&mut self, recoveries: &[ScheduledRecovery], now: Instant) {
        for recovery in recoveries {
            self.scheduler.mark_sent(*recovery, now);
        }
    }

    pub(super) fn defer(&mut self, recoveries: &[ScheduledRecovery], now: Instant) {
        for recovery in recoveries {
            self.scheduler.defer(*recovery, now);
        }
    }

    pub(super) fn degrade_recoveries(&mut self, recoveries: &[ScheduledRecovery]) {
        for recovery in recoveries {
            self.scheduler.degrade(*recovery);
        }
    }

    pub(super) fn degrade_simulator(&mut self, simulator_id: SimulatorId) {
        self.scheduler.degrade_simulator(simulator_id);
    }

    fn start_send_future<F>(&mut self, recoveries: Vec<ScheduledRecovery>, future: F)
    where
        F: Future<Output = Result<(), Error>> + Send + 'static,
    {
        self.in_flight = Some(PendingRecoverySend { recoveries, future: future.boxed() });
    }
}

pub(super) fn warn_unrecovered(reason: &'static str, recoveries: &[ScheduledRecovery]) {
    if recoveries.is_empty() {
        return;
    }
    let simulator = recoveries.first().map(|recovery| recovery.simulator_id.get());
    let region = recoveries.first().map(|recovery| recovery.key.region().get());
    let local_ids =
        recoveries.iter().take(MAX_RECOVERY_DIAGNOSTIC_ADDRESSES).map(|recovery| recovery.key.local_id().get()).collect::<Vec<_>>();
    tracing::warn!(
        reason,
        simulator,
        region,
        count = recoveries.len(),
        local_ids = ?local_ids,
        "object recovery stopped; objects may remain missing"
    );
}

pub(super) fn retryable_send_error(error: &Error) -> bool {
    matches!(error, CommandQueueFull | SimulatorCommandQueueFull | SimulatorNotReady | SimulatorSend(_))
}

pub(super) fn simulator_lost_error(error: &Error) -> bool {
    matches!(error, SessionClosed | SimulatorConnectionClosed | SimulatorUnavailable)
}

pub(super) fn recovery_request(agent_data: RequestAgentData, addresses: &[LocalAddress]) -> RequestMultipleObjects {
    let objects = addresses.iter().map(|address| ObjectRequest { cache_miss_type: Full, local_id: address.local_id().get() }).collect();
    RequestMultipleObjects { agent_data, objects }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, num::NonZeroU32, time::Duration};

    use network::{Error, testing::simulator_id};
    use protocol::lludp::{Message, Packet};
    use protocol::messages::{CacheMissType::Full, RequestAgentData};
    use tokio::time::Instant;
    use uuid::Uuid;
    use world::{LocalAddress, Region};

    use super::{
        MAX_RECOVERY_OBJECTS_PER_REQUEST, MAX_RECOVERY_REQUEST_BYTES, RECOVERY_ENTRY_BYTES, RECOVERY_IDLE_WAIT, RECOVERY_MAX_AGE,
        RECOVERY_MAX_SENDS, RECOVERY_REQUEST_INTERVAL, RECOVERY_RETRY_INTERVAL, RecoveryCompletion, RecoveryIntake, RecoveryScheduler,
        recovery_request,
    };

    fn address(region: u64, local_id: u32) -> LocalAddress {
        LocalAddress::new(Region::new(region), NonZeroU32::new(local_id).expect("nonzero local ID"))
    }

    fn agent_data() -> RequestAgentData {
        RequestAgentData { agent_id: Uuid::from_u128(1), session_id: Uuid::from_u128(2) }
    }

    #[test]
    fn retries_coalesce_and_stop_after_the_send_limit() {
        let simulator = simulator_id();
        let target = address(simulator.get(), 8);
        let mut now = Instant::now();
        let mut scheduler = RecoveryScheduler::with_limits(1, RECOVERY_ENTRY_BYTES);
        assert!(scheduler.schedule(simulator, agent_data(), target, now).is_some());

        for _ in 0..RECOVERY_MAX_SENDS {
            assert!(scheduler.schedule(simulator, agent_data(), target, now + Duration::from_secs(1)).is_some());
            let due = scheduler.take_due(now);
            assert_eq!(due.len(), 1);
            assert_eq!(due[0].key, target);
            assert!(!due[0].retry.is_exhausted());
            assert!(scheduler.take_due(now).is_empty(), "in-flight work is not dispatched twice");
            scheduler.mark_sent(due[0], now);
            assert!(scheduler.take_due(now + RECOVERY_RETRY_INTERVAL - Duration::from_nanos(1)).is_empty());
            now += RECOVERY_RETRY_INTERVAL;
        }

        let exhausted = scheduler.take_due(now);
        assert_eq!(exhausted.len(), 1);
        assert!(exhausted[0].retry.is_exhausted());
        scheduler.degrade(exhausted[0]);
        assert!(scheduler.schedule(simulator, agent_data(), target, now).is_none());
        assert!(scheduler.next_deadline().is_none());
        scheduler.cancel(target);
        assert!(scheduler.schedule(simulator, agent_data(), target, now).is_some());
        assert!(!scheduler.take_due(now)[0].retry.is_exhausted());
    }

    #[test]
    fn stale_completions_cannot_change_replaced_or_degraded_work() {
        let source = simulator_id();
        let destination = simulator_id();
        let target = address(source.get(), 10);
        let now = Instant::now();

        for replacement in [Some(source), Some(destination), None] {
            let mut scheduler = RecoveryScheduler::with_limits(1, RECOVERY_ENTRY_BYTES);
            assert!(scheduler.schedule(source, agent_data(), target, now).is_some());
            let stale = scheduler.take_due(now)[0];

            if replacement == Some(source) {
                scheduler.cancel(target);
            }
            if let Some(simulator) = replacement {
                assert!(scheduler.schedule(simulator, agent_data(), target, now).is_some());
            } else {
                scheduler.mark_degraded(target, Some(source));
            }

            scheduler.mark_sent(stale, now);
            scheduler.defer(stale, now);
            scheduler.degrade(stale);

            let due = scheduler.take_due(now);
            if let Some(simulator) = replacement {
                assert_eq!(due.len(), 1);
                assert_eq!(due[0].simulator_id, simulator);
                assert_eq!(due[0].retry.sends, 0);
            } else {
                assert!(due.is_empty());
                assert!(scheduler.is_degraded(target));
            }
        }
    }

    #[test]
    fn simulator_retirement_clears_only_its_current_routes() {
        let source = simulator_id();
        let destination = simulator_id();
        let target = address(source.get(), 9);
        let failed = address(source.get(), 10);
        let now = Instant::now();
        let mut scheduler = RecoveryScheduler::with_limits(2, 2 * RECOVERY_ENTRY_BYTES);
        assert!(scheduler.schedule(source, agent_data(), target, now).is_some());
        assert!(scheduler.schedule(destination, agent_data(), target, now).is_some());
        scheduler.mark_degraded(failed, Some(source));

        scheduler.cancel_simulator(source);
        assert!(!scheduler.is_degraded(failed));
        let due = scheduler.take_due(now);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].simulator_id, destination);
        scheduler.degrade_simulator(destination);
        assert!(scheduler.is_degraded(target));
        assert!(scheduler.next_deadline().is_none());

        scheduler.cancel_simulator(destination);
        assert!(scheduler.schedule(source, agent_data(), target, now).is_some());
    }

    #[test]
    fn losing_route_or_session_context_stops_an_existing_recovery() {
        let simulator = simulator_id();
        let target = address(simulator.get(), 9);

        for route in [Some(simulator), None] {
            let mut intake = RecoveryIntake::new();
            assert!(intake.scheduler.schedule(simulator, agent_data(), target, Instant::now()).is_some());

            intake.schedule(vec![(route, target)], vec![], None);

            assert!(intake.scheduler.next_deadline().is_none());
            assert!(intake.scheduler.is_degraded(target));
        }
    }

    #[test]
    fn dispatch_batches_are_bounded_disjoint_and_simulator_local() {
        let simulators = [simulator_id(), simulator_id()];
        let now = Instant::now();
        let mut scheduler = RecoveryScheduler::new();
        let mut expected = HashSet::new();
        for simulator in simulators {
            for local_id in 1..=MAX_RECOVERY_OBJECTS_PER_REQUEST + 1 {
                let target = address(simulator.get(), local_id as u32);
                assert!(scheduler.schedule(simulator, agent_data(), target, now).is_some());
                expected.insert(target);
            }
        }

        while !expected.is_empty() {
            let batch = scheduler.take_due(now);
            assert!(!batch.is_empty());
            assert!(batch.len() <= MAX_RECOVERY_OBJECTS_PER_REQUEST);
            assert!(batch.iter().all(|recovery| recovery.simulator_id == batch[0].simulator_id));
            for recovery in batch {
                assert!(expected.remove(&recovery.key), "each target is dispatched once");
                scheduler.mark_sent(recovery, now);
            }
        }
        assert!(scheduler.take_due(now).is_empty());
    }

    #[test]
    fn transient_failures_back_off_without_spending_the_send_limit() {
        let simulator = simulator_id();
        let target = address(simulator.get(), 8);
        let mut now = Instant::now();
        let mut scheduler = RecoveryScheduler::with_limits(1, RECOVERY_ENTRY_BYTES);
        assert!(scheduler.schedule(simulator, agent_data(), target, now).is_some());

        for delay in [5, 10, 20, 30] {
            let recovery = scheduler.take_due(now)[0];
            assert_eq!(recovery.retry.sends, 0);
            scheduler.defer(recovery, now);
            let deadline = now + Duration::from_secs(delay);
            assert_eq!(scheduler.next_deadline(), Some(deadline.min(recovery.retry.created_at + RECOVERY_MAX_AGE)));
            assert!(scheduler.take_due(deadline - Duration::from_nanos(1)).is_empty());
            now = deadline;
        }
    }

    #[test]
    fn age_limit_suppresses_repeated_misses_until_an_object_update() {
        let simulator = simulator_id();
        let target = address(simulator.get(), 8);
        let other = address(simulator.get(), 9);
        let now = Instant::now();
        let mut scheduler = RecoveryScheduler::with_limits(1, RECOVERY_ENTRY_BYTES);
        assert!(scheduler.schedule(simulator, agent_data(), target, now).is_some());

        let expired = scheduler.take_expired(now + RECOVERY_MAX_AGE);
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].key, target);
        assert!(scheduler.take_due(now + RECOVERY_MAX_AGE).is_empty());
        assert!(scheduler.schedule(simulator, agent_data(), target, now).is_none());
        assert!(scheduler.schedule(simulator, agent_data(), other, now).is_none(), "suppression uses bounded storage");
        scheduler.cancel(target);
        assert!(scheduler.schedule(simulator, agent_data(), other, now).is_some());
    }

    #[test]
    fn dispatch_never_sends_expired_work_left_by_bounded_cleanup() {
        let simulator = simulator_id();
        let now = Instant::now();
        let mut scheduler = RecoveryScheduler::new();
        for local_id in 1..=MAX_RECOVERY_OBJECTS_PER_REQUEST + 1 {
            let target = address(simulator.get(), local_id as u32);
            assert!(scheduler.schedule(simulator, agent_data(), target, now).is_some());
        }
        let now = now + RECOVERY_MAX_AGE;
        assert_eq!(scheduler.take_expired(now).len(), MAX_RECOVERY_OBJECTS_PER_REQUEST);
        assert!(scheduler.take_due(now).is_empty());
        assert_eq!(scheduler.take_expired(now).len(), 1);
        assert!(scheduler.next_deadline().is_none());
    }

    #[test]
    fn admission_bounds_active_and_degraded_targets_together() {
        let simulator = simulator_id();
        let now = Instant::now();
        for (max_targets, max_bytes) in [(2, 3 * RECOVERY_ENTRY_BYTES), (3, 2 * RECOVERY_ENTRY_BYTES)] {
            let mut scheduler = RecoveryScheduler::with_limits(max_targets, max_bytes);
            let first = address(simulator.get(), 1);
            let second = address(simulator.get(), 2);
            let excess = address(simulator.get(), 3);
            assert!(scheduler.schedule(simulator, agent_data(), first, now).is_some());
            scheduler.mark_degraded(second, Some(simulator));
            assert!(scheduler.schedule(simulator, agent_data(), excess, now).is_none());
            scheduler.mark_degraded(excess, Some(simulator));
            assert!(!scheduler.is_degraded(excess), "rejected work cannot allocate a suppression entry");
            scheduler.cancel(first);
            assert!(scheduler.schedule(simulator, agent_data(), excess, now).is_some());

            assert_eq!(scheduler.record_overflow(2, now), Some(2));
            assert_eq!(scheduler.record_overflow(3, now + Duration::from_secs(1)), None);
            assert_eq!(scheduler.record_overflow(4, now + Duration::from_secs(5)), Some(7));
        }
    }

    #[test]
    fn requests_fetch_full_data_and_fit_the_wire_budget() {
        let targets = [address(7, 10), address(7, 11)];
        let request = recovery_request(agent_data(), &targets);
        assert_eq!(request.objects.iter().map(|object| object.local_id).collect::<Vec<_>>(), [10, 11]);
        assert!(request.objects.iter().all(|object| object.cache_miss_type == Full));

        let targets = vec![address(7, 0xff00_ff00); MAX_RECOVERY_OBJECTS_PER_REQUEST];
        let agent_data = RequestAgentData {
            agent_id: Uuid::from_bytes([0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255]),
            ..agent_data()
        };
        let request = recovery_request(agent_data, &targets);
        let encoded = Packet::new(0, Message::RequestMultipleObjects(request)).encode().expect("bounded recovery packet");
        assert!(encoded.len() <= MAX_RECOVERY_REQUEST_BYTES);
    }

    #[tokio::test(start_paused = true)]
    async fn pacing_delays_next_simulator_batch() {
        let first = simulator_id();
        let second = simulator_id();
        let mut intake = RecoveryIntake::new();
        intake.schedule(vec![(Some(first), address(first.get(), 8)), (Some(second), address(second.get(), 9))], vec![], Some(agent_data()));

        let RecoveryCompletion::Deadline { due, aged_out } = intake.next_completion().await else { panic!("first batch is due") };
        assert!(aged_out.is_empty());
        assert_eq!(due.len(), 1);
        let (send, sent) = futures::channel::oneshot::channel();
        intake.start_send_future(due, async move { sent.await.expect("send result") });
        let completion = {
            let send_completion = intake.next_completion();
            tokio::pin!(send_completion);
            assert!(futures::poll!(send_completion.as_mut()).is_pending());
            send.send(Ok(())).expect("send is pending");
            send_completion.await
        };
        let RecoveryCompletion::Send { recoveries, result, completed_at } = completion else { panic!("send completion is returned") };
        std::assert_matches!(result, Ok(()));
        let sent_simulator = recoveries[0].simulator_id;
        intake.mark_sent(&recoveries, completed_at);

        let next_batch = intake.next_completion();
        tokio::pin!(next_batch);
        assert!(futures::poll!(next_batch.as_mut()).is_pending());
        tokio::time::advance(RECOVERY_REQUEST_INTERVAL - Duration::from_millis(1)).await;
        assert!(futures::poll!(next_batch.as_mut()).is_pending());
        tokio::time::advance(Duration::from_millis(1)).await;

        let RecoveryCompletion::Deadline { due, aged_out } = next_batch.await else { panic!("paced batch is due") };
        assert!(aged_out.is_empty());
        assert_eq!(due.len(), 1);
        assert_ne!(due[0].simulator_id, sent_simulator);
    }

    #[tokio::test]
    async fn cancellation_drops_pending_send() {
        struct DropSignal(Option<futures::channel::oneshot::Sender<()>>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                if let Some(sender) = self.0.take() {
                    sender.send(()).expect("drop signal receiver stays open");
                }
            }
        }

        let mut intake = RecoveryIntake::new();
        let (signal, dropped) = futures::channel::oneshot::channel();
        let signal = DropSignal(Some(signal));
        intake.start_send_future(vec![], async move {
            let _signal = signal;
            futures::future::pending::<Result<(), Error>>().await
        });

        intake.cancel();

        dropped.await.expect("cancellation drops the in-flight future");
    }

    #[tokio::test(start_paused = true)]
    async fn idle_rechecks_after_idle_interval() {
        let mut intake = RecoveryIntake::new();
        let next = intake.next_completion();
        tokio::pin!(next);
        assert!(futures::poll!(next.as_mut()).is_pending());
        tokio::time::advance(RECOVERY_IDLE_WAIT - Duration::from_millis(1)).await;
        assert!(futures::poll!(next.as_mut()).is_pending());
        tokio::time::advance(Duration::from_millis(1)).await;

        let RecoveryCompletion::Deadline { aged_out, due } = next.await else { panic!("idle deadline is returned") };
        assert!(aged_out.is_empty());
        assert!(due.is_empty());
    }
}
