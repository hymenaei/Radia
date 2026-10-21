use std::{
    collections::HashMap,
    future::{Future, pending},
    mem::size_of,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

use futures::{FutureExt, channel::mpsc, pin_mut, stream::StreamExt};
use network::{
    AgentSession, Delivery, Error,
    Error::{
        CommandQueueFull, SessionClosed, SimulatorCommandQueueFull, SimulatorConnectionClosed, SimulatorNotReady, SimulatorSend,
        SimulatorUnavailable,
    },
    SessionHandle, SimulatorId,
};
use protocol::lludp::{Message, Packet};
use protocol::messages::{CacheMissType::Full, ObjectRequest, RequestAgentData, RequestMultipleObjects};
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
struct ScheduledRecovery {
    key: LocalAddress,
    simulator_id: SimulatorId,
    token: u64,
    agent_data: RequestAgentData,
    retry: RecoveryRetryState,
}

impl ScheduledRecovery {
    fn new(simulator_id: SimulatorId, token: u64, agent_data: RequestAgentData, address: LocalAddress, now: Instant) -> Self {
        Self { key: address, simulator_id, token, agent_data, retry: RecoveryRetryState::new(now) }
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

pub(super) fn schedule_recovery_requests(
    targets: Vec<(Option<SimulatorId>, LocalAddress)>,
    updated: Vec<LocalAddress>,
    session: Option<&AgentSession>,
    queue: &Mutex<RecoveryScheduler>,
    wake_sender: &mut mpsc::Sender<()>,
) {
    if targets.is_empty() && updated.is_empty() {
        return;
    }
    let agent_data = session.map(|session| RequestAgentData { agent_id: session.agent_id, session_id: session.session_id });
    let now = Instant::now();
    let mut dropped = 0;
    let overflow_warning = {
        let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
        for address in updated {
            queue.cancel(address);
        }
        for (simulator_id, address) in targets {
            if queue.is_degraded(address) {
                continue;
            }
            let Some(simulator_id) = simulator_id else {
                queue.mark_degraded(address, None);
                dropped += 1;
                continue;
            };
            let Some(agent_data) = agent_data else {
                queue.mark_degraded(address, Some(simulator_id));
                dropped += 1;
                continue;
            };
            match queue.schedule(simulator_id, agent_data, address, now) {
                Some(_) => {}
                None => {
                    queue.mark_degraded(address, Some(simulator_id));
                    dropped += 1;
                }
            }
        }
        (dropped > 0).then(|| queue.record_overflow(dropped, now)).flatten()
    };
    if let Some(dropped) = overflow_warning {
        tracing::warn!(dropped, "some object recovery targets could not be queued");
    }
    let _ = wake_sender.try_send(());
}

pub(super) async fn run_recovery_retries(
    mut wake_receiver: mpsc::Receiver<()>,
    queue: Arc<Mutex<RecoveryScheduler>>,
    session_handle: SessionHandle,
    cancellation: impl Future<Output = ()> + Clone + Send + 'static,
) {
    use Delivery::Reliable;

    let mut next_send_at = None;
    loop {
        let now = Instant::now();
        let queue_deadline = recovery_deadline(&queue);
        let deadline = queue_deadline.map(|deadline| next_send_at.map_or(deadline, |send_at| deadline.max(send_at)));
        let wait = deadline.map_or(RECOVERY_IDLE_WAIT, |deadline| deadline.saturating_duration_since(now));
        let can_wake_early = next_send_at.is_none_or(|send_at| now >= send_at);
        let timer = tokio::time::sleep(wait).fuse();
        let wake = async { if can_wake_early { wake_receiver.next().await } else { pending::<Option<()>>().await } }.fuse();
        let cancelled = cancellation.clone().fuse();
        pin_mut!(timer, wake, cancelled);
        futures::select! {
            _ = cancelled => break,
            _ = timer => {},
            wake = wake => if wake.is_none() { break; },
        }

        let aged_out = take_expired_recoveries(&queue, Instant::now());
        warn_unrecovered("age_limit", &aged_out);

        let due = take_due_recoveries(&queue, Instant::now());
        if due.is_empty() {
            continue;
        }
        let (exhausted, recoveries): (Vec<_>, Vec<_>) = due.into_iter().partition(|recovery| recovery.retry.is_exhausted());
        degrade_recoveries(&queue, &exhausted);
        warn_unrecovered("send_limit", &exhausted);
        let Some(first) = recoveries.first() else {
            continue;
        };
        let simulator_id = first.simulator_id;
        let addresses = recoveries.iter().map(|recovery| recovery.key).collect::<Vec<_>>();
        let request = recovery_request(first.agent_data, &addresses);
        let packet_size = Packet::new(0, Message::RequestMultipleObjects(request.clone())).encode().map(|packet| packet.len());
        if !packet_size.is_ok_and(|size| size <= MAX_RECOVERY_REQUEST_BYTES) {
            degrade_recoveries(&queue, &recoveries);
            tracing::error!("object recovery request exceeded its byte budget");
            continue;
        }

        let send = session_handle.send_to(simulator_id, request, Reliable).fuse();
        let cancelled = cancellation.clone().fuse();
        pin_mut!(send, cancelled);
        let result = futures::select! {
            result = send => Some(result),
            _ = cancelled => None,
        };
        let Some(result) = result else { return };
        let now = Instant::now();
        next_send_at = Some(now + RECOVERY_REQUEST_INTERVAL);

        match result {
            Ok(()) => {
                mark_recoveries_sent(&queue, &recoveries, now);
            }
            Err(error) if retryable_send_error(&error) => {
                defer_recoveries(&queue, &recoveries, now);
                tracing::debug!(%error, "object recovery send deferred");
            }
            Err(error) => {
                if simulator_lost_error(&error) {
                    queue.lock().unwrap_or_else(PoisonError::into_inner).degrade_simulator(simulator_id);
                } else {
                    degrade_recoveries(&queue, &recoveries);
                }
                tracing::warn!(%error, "could not request object updates");
            }
        }
    }
}

fn warn_unrecovered(reason: &'static str, recoveries: &[ScheduledRecovery]) {
    if recoveries.is_empty() {
        return;
    }
    let addresses = recoveries.iter().take(MAX_RECOVERY_DIAGNOSTIC_ADDRESSES).map(|recovery| recovery.key).collect::<Vec<_>>();
    tracing::warn!(
        reason,
        count = recoveries.len(),
        addresses = ?addresses,
        "Object recovery stopped; objects may remain missing"
    );
}

fn recovery_deadline(queue: &Mutex<RecoveryScheduler>) -> Option<Instant> {
    queue.lock().unwrap_or_else(PoisonError::into_inner).next_deadline()
}

fn take_expired_recoveries(queue: &Mutex<RecoveryScheduler>, now: Instant) -> Vec<ScheduledRecovery> {
    queue.lock().unwrap_or_else(PoisonError::into_inner).take_expired(now)
}

fn take_due_recoveries(queue: &Mutex<RecoveryScheduler>, now: Instant) -> Vec<ScheduledRecovery> {
    queue.lock().unwrap_or_else(PoisonError::into_inner).take_due(now)
}

fn degrade_recoveries(queue: &Mutex<RecoveryScheduler>, recoveries: &[ScheduledRecovery]) {
    let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
    for recovery in recoveries {
        queue.degrade(*recovery);
    }
}

fn mark_recoveries_sent(queue: &Mutex<RecoveryScheduler>, recoveries: &[ScheduledRecovery], now: Instant) {
    let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
    for recovery in recoveries {
        queue.mark_sent(*recovery, now);
    }
}

fn defer_recoveries(queue: &Mutex<RecoveryScheduler>, recoveries: &[ScheduledRecovery], now: Instant) {
    let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
    for recovery in recoveries {
        queue.defer(*recovery, now);
    }
}

fn retryable_send_error(error: &Error) -> bool {
    matches!(error, CommandQueueFull | SimulatorCommandQueueFull | SimulatorNotReady | SimulatorSend(_))
}

fn simulator_lost_error(error: &Error) -> bool {
    matches!(error, SessionClosed | SimulatorConnectionClosed | SimulatorUnavailable)
}

pub(super) fn recovery_request(agent_data: RequestAgentData, addresses: &[LocalAddress]) -> RequestMultipleObjects {
    let objects = addresses.iter().map(|address| ObjectRequest { cache_miss_type: Full, local_id: address.local_id().get() }).collect();
    RequestMultipleObjects { agent_data, objects }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        num::NonZeroU32,
        sync::{Mutex, PoisonError},
        time::{Duration, Instant},
    };

    use base::UUID;
    use futures::channel::mpsc;
    use network::testing::simulator_id;
    use protocol::lludp::{Message, Packet};
    use protocol::messages::{CacheMissType::Full, RequestAgentData};
    use world::{LocalAddress, Region};

    use super::{
        MAX_RECOVERY_OBJECTS_PER_REQUEST, MAX_RECOVERY_REQUEST_BYTES, RECOVERY_ENTRY_BYTES, RECOVERY_MAX_AGE, RECOVERY_MAX_SENDS,
        RECOVERY_RETRY_INTERVAL, RecoveryScheduler, recovery_request, schedule_recovery_requests,
    };

    fn address(region: u64, local_id: u32) -> LocalAddress {
        LocalAddress::new(Region::new(region), NonZeroU32::new(local_id).expect("nonzero local ID"))
    }

    fn agent_data() -> RequestAgentData {
        RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) }
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
            let mut scheduler = RecoveryScheduler::with_limits(1, RECOVERY_ENTRY_BYTES);
            assert!(scheduler.schedule(simulator, agent_data(), target, Instant::now()).is_some());
            let scheduler = Mutex::new(scheduler);
            let (mut wake, _) = mpsc::channel(1);

            schedule_recovery_requests(vec![(route, target)], vec![], None, &scheduler, &mut wake);

            let scheduler = scheduler.lock().unwrap_or_else(PoisonError::into_inner);
            assert!(scheduler.next_deadline().is_none());
            assert!(scheduler.is_degraded(target));
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
            agent_id: UUID::from_bytes([0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255, 0, 255]),
            ..agent_data()
        };
        let request = recovery_request(agent_data, &targets);
        let encoded = Packet::new(0, Message::RequestMultipleObjects(request)).encode().expect("bounded recovery packet");
        assert!(encoded.len() <= MAX_RECOVERY_REQUEST_BYTES);
    }
}
