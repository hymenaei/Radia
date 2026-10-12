//! Bounded scheduling and retry for object recovery requests.

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

use super::coordinator::{PresentationMailbox, ViewerUpdate};

pub(super) const RECOVERY_RETRY_INTERVAL: Duration = Duration::from_secs(5);
pub(super) const RECOVERY_MAX_SENDS: u8 = 4;
const RECOVERY_IDLE_WAIT: Duration = Duration::from_secs(60);
const RECOVERY_REQUEST_INTERVAL: Duration = Duration::from_millis(100);
const RECOVERY_MAX_AGE: Duration = Duration::from_secs(60);
const RECOVERY_MAX_BACKOFF: Duration = Duration::from_secs(30);
const RECOVERY_OVERFLOW_LOG_INTERVAL: Duration = Duration::from_secs(5);
pub(super) const MAX_RECOVERY_OBJECTS_PER_REQUEST: usize = 255;
pub(super) const MAX_PENDING_RECOVERY_TARGETS: usize = 65_536;
pub(super) const MAX_PENDING_RECOVERY_BYTES: usize = 16 * 1024 * 1024;
const MAX_RECOVERY_REQUEST_BYTES: usize = 2 * 1024;
const RECOVERY_QUEUE_ENTRY_BYTES: usize = size_of::<(RecoveryKey, ScheduledRecovery)>() + 64;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct RecoveryKey {
    pub(super) simulator_id: SimulatorId,
    pub(super) address: LocalAddress,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct ScheduledRecovery {
    pub(super) key: RecoveryKey,
    pub(super) token: u64,
    pub(super) agent_data: RequestAgentData,
    pub(super) retry: RecoveryRetryState,
    pub(super) in_flight: bool,
}

impl ScheduledRecovery {
    fn new(simulator_id: SimulatorId, token: u64, agent_data: RequestAgentData, address: LocalAddress, now: Instant) -> Self {
        Self { key: RecoveryKey { simulator_id, address }, token, agent_data, retry: RecoveryRetryState::new(now), in_flight: false }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RecoveryRetryState {
    pub(super) sends: u8,
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

    pub(super) fn is_exhausted(self) -> bool {
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

pub(super) struct RecoveryRetryQueue {
    pub(super) pending: HashMap<RecoveryKey, ScheduledRecovery>,
    max_targets: usize,
    max_bytes: usize,
    pending_bytes: usize,
    next_token: u64,
    dropped_since_warning: usize,
    last_overflow_warning: Option<Instant>,
}

impl RecoveryRetryQueue {
    pub(super) fn new(max_targets: usize, max_bytes: usize) -> Self {
        Self {
            pending: HashMap::new(),
            max_targets,
            max_bytes,
            pending_bytes: 0,
            next_token: 0,
            dropped_since_warning: 0,
            last_overflow_warning: None,
        }
    }

    pub(super) fn schedule(
        &mut self,
        simulator_id: SimulatorId,
        agent_data: RequestAgentData,
        address: LocalAddress,
        now: Instant,
    ) -> bool {
        let key = RecoveryKey { simulator_id, address };
        if let Some(recovery) = self.pending.get_mut(&key) {
            recovery.agent_data = agent_data;
            return true;
        }
        if self.pending.len() >= self.max_targets || self.pending_bytes.saturating_add(RECOVERY_QUEUE_ENTRY_BYTES) > self.max_bytes {
            return false;
        }
        let Some(token) = self.next_token.checked_add(1) else { return false };
        self.next_token = token;
        self.pending.insert(key, ScheduledRecovery::new(simulator_id, token, agent_data, address, now));
        self.pending_bytes += RECOVERY_QUEUE_ENTRY_BYTES;
        true
    }

    pub(super) fn cancel_simulator(&mut self, simulator_id: SimulatorId) {
        self.pending.retain(|key, _| key.simulator_id != simulator_id);
        self.update_pending_bytes();
    }

    pub(super) fn cancel(&mut self, simulator_id: SimulatorId, address: LocalAddress) {
        self.remove(RecoveryKey { simulator_id, address });
    }

    pub(super) fn next_deadline(&self) -> Option<Instant> {
        self.pending
            .values()
            .filter(|recovery| !recovery.in_flight)
            .map(|recovery| recovery.retry.deadline.min(recovery.retry.created_at + RECOVERY_MAX_AGE))
            .min()
    }

    pub(super) fn take_due(&mut self, now: Instant) -> Vec<ScheduledRecovery> {
        self.pending.retain(|_, recovery| !recovery.retry.is_expired(now));
        self.update_pending_bytes();

        let Some(simulator_id) = self
            .pending
            .values()
            .find(|recovery| !recovery.in_flight && recovery.retry.is_due(now))
            .map(|recovery| recovery.key.simulator_id)
        else {
            return Vec::new();
        };
        let due_keys = self
            .pending
            .iter()
            .filter(|(_, recovery)| recovery.key.simulator_id == simulator_id && !recovery.in_flight && recovery.retry.is_due(now))
            .take(MAX_RECOVERY_OBJECTS_PER_REQUEST)
            .map(|(key, _)| *key)
            .collect::<Vec<_>>();
        due_keys
            .into_iter()
            .filter_map(|key| {
                let recovery = self.pending.get_mut(&key)?;
                recovery.in_flight = true;
                Some(*recovery)
            })
            .collect()
    }

    pub(super) fn mark_sent(&mut self, recovery: ScheduledRecovery, now: Instant) {
        if let Some(current) = self.pending.get_mut(&recovery.key).filter(|current| current.token == recovery.token) {
            current.retry.mark_sent(now);
            current.in_flight = false;
        }
    }

    pub(super) fn finish(&mut self, recovery: ScheduledRecovery) {
        if self.pending.get(&recovery.key).is_some_and(|current| current.token == recovery.token) {
            self.remove(recovery.key);
        }
    }

    pub(super) fn defer(&mut self, recovery: ScheduledRecovery, now: Instant) {
        if let Some(current) = self.pending.get_mut(&recovery.key).filter(|current| current.token == recovery.token) {
            current.retry.mark_failed(now);
            current.in_flight = false;
        }
    }

    fn remove(&mut self, key: RecoveryKey) {
        if self.pending.remove(&key).is_some() {
            self.pending_bytes = self.pending_bytes.saturating_sub(RECOVERY_QUEUE_ENTRY_BYTES);
        }
    }

    fn update_pending_bytes(&mut self) {
        self.pending_bytes = self.pending.len().saturating_mul(RECOVERY_QUEUE_ENTRY_BYTES);
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
    simulator_id: SimulatorId,
    targets: Vec<LocalAddress>,
    updated: Vec<LocalAddress>,
    session: Option<&AgentSession>,
    queue: Arc<Mutex<RecoveryRetryQueue>>,
    wake_sender: &mut mpsc::Sender<()>,
) {
    if targets.is_empty() && updated.is_empty() {
        return;
    }
    let Some(session) = session else {
        return;
    };
    let agent_data = RequestAgentData { agent_id: session.agent_id, session_id: session.session_id };
    let now = Instant::now();
    let overflow_warning = {
        let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
        for address in updated {
            queue.cancel(simulator_id, address);
        }
        let dropped = targets.into_iter().filter(|address| !queue.schedule(simulator_id, agent_data, *address, now)).count();
        (dropped > 0).then(|| queue.record_overflow(dropped, now)).flatten()
    };
    if let Some(dropped) = overflow_warning {
        tracing::warn!(dropped, "world recovery budget reached; some targets were not queued");
    }
    let _ = wake_sender.try_send(());
}

pub(super) async fn run_recovery_retries(
    mut wake_receiver: mpsc::Receiver<()>,
    queue: Arc<Mutex<RecoveryRetryQueue>>,
    session_handle: SessionHandle,
    mailbox: PresentationMailbox,
    background: gpui::BackgroundExecutor,
    cancellation: impl Future<Output = ()> + Clone + Send + 'static,
) {
    use Delivery::Reliable;

    let mut next_send_at = None;
    loop {
        let now = Instant::now();
        let queue_deadline = {
            let queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            queue.next_deadline()
        };
        let deadline = queue_deadline.map(|deadline| next_send_at.map_or(deadline, |send_at| deadline.max(send_at)));
        let wait = deadline.map_or(RECOVERY_IDLE_WAIT, |deadline| deadline.saturating_duration_since(now));
        let can_wake_early = next_send_at.is_none_or(|send_at| now >= send_at);
        let timer = background.timer(wait).fuse();
        let wake = async { if can_wake_early { wake_receiver.next().await } else { pending::<Option<()>>().await } }.fuse();
        let cancelled = cancellation.clone().fuse();
        pin_mut!(timer, wake, cancelled);
        futures::select! {
            _ = cancelled => break,
            _ = timer => {},
            wake = wake => if wake.is_none() { break; },
        }

        let due = {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            queue.take_due(Instant::now())
        };
        if due.is_empty() {
            continue;
        }
        let (expired, recoveries): (Vec<_>, Vec<_>) = due.into_iter().partition(|recovery| recovery.retry.is_exhausted());
        {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            for recovery in expired {
                queue.finish(recovery);
            }
        }
        let Some(first) = recoveries.first() else {
            continue;
        };
        let simulator_id = first.key.simulator_id;
        let addresses = recoveries.iter().map(|recovery| recovery.key.address).collect::<Vec<_>>();
        let request = recovery_request(first.agent_data, &addresses);
        let packet_size = Packet::new(0, Message::RequestMultipleObjects(request.clone())).encode().map(|packet| packet.len());
        if !packet_size.is_ok_and(|size| size <= MAX_RECOVERY_REQUEST_BYTES) {
            let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
            for recovery in recoveries {
                queue.finish(recovery);
            }
            mailbox.publish(ViewerUpdate::SendError("Could not request object updates: recovery packet exceeded its byte budget.".into()));
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
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                for recovery in recoveries {
                    queue.mark_sent(recovery, now);
                }
            }
            Err(error) if retryable_send_error(&error) => {
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                for recovery in recoveries {
                    queue.defer(recovery, now);
                }
                tracing::debug!(%error, "object recovery send deferred");
            }
            Err(error) => {
                let mut queue = queue.lock().unwrap_or_else(PoisonError::into_inner);
                if simulator_lost_error(&error) {
                    queue.cancel_simulator(simulator_id);
                } else {
                    for recovery in recoveries {
                        queue.finish(recovery);
                    }
                }
                mailbox.publish(ViewerUpdate::SendError(format!("Could not request object updates: {error}")));
            }
        }
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
        num::NonZeroU32,
        time::{Duration, Instant},
    };

    use base::UUID;
    use network::test_support::simulator_id;
    use protocol::lludp::{Message, Packet};
    use protocol::messages::{CacheMissType::Full, RequestAgentData};
    use world::{LocalAddress, Region};

    use super::{
        MAX_PENDING_RECOVERY_BYTES, MAX_PENDING_RECOVERY_TARGETS, MAX_RECOVERY_OBJECTS_PER_REQUEST, MAX_RECOVERY_REQUEST_BYTES,
        RECOVERY_MAX_AGE, RECOVERY_MAX_SENDS, RECOVERY_QUEUE_ENTRY_BYTES, RECOVERY_RETRY_INTERVAL, RecoveryKey, RecoveryRetryQueue,
        recovery_request,
    };

    fn address(region: u64, local_id: u32) -> LocalAddress {
        LocalAddress::new(Region::new(region), NonZeroU32::new(local_id).expect("nonzero local ID"))
    }

    #[test]
    fn retry_queue_bounds_targets_and_expires() {
        let simulator_id = simulator_id();
        let target = address(simulator_id.get(), 8);
        let other_target = address(simulator_id.get(), 9);
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let mut now = Instant::now();
        let mut queue = RecoveryRetryQueue::new(1, RECOVERY_QUEUE_ENTRY_BYTES);
        assert!(queue.schedule(simulator_id, agent_data, target, now));
        assert!(queue.schedule(simulator_id, agent_data, target, now + Duration::from_secs(1)));
        assert!(!queue.schedule(simulator_id, agent_data, other_target, now));
        assert_eq!(queue.pending.len(), 1);

        for _ in 0..RECOVERY_MAX_SENDS {
            let due = queue.take_due(now);
            let [recovery] = due.as_slice() else { panic!("expected one due recovery") };
            assert_eq!(recovery.key.address, target);
            assert!(!recovery.retry.is_exhausted());
            queue.mark_sent(*recovery, now);
            assert!(queue.take_due(now + RECOVERY_RETRY_INTERVAL - Duration::from_nanos(1)).is_empty());
            now += RECOVERY_RETRY_INTERVAL;
        }

        let due = queue.take_due(now);
        let [expired] = due.as_slice() else { panic!("expected exhausted recovery") };
        assert_eq!(expired.retry.sends, RECOVERY_MAX_SENDS);
        queue.finish(*expired);
        assert!(queue.next_deadline().is_none());
    }

    #[test]
    fn stale_in_flight_retry_cannot_change_a_replacement() {
        let simulator_id = simulator_id();
        let target = address(simulator_id.get(), 10);
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let now = Instant::now();
        let mut queue = RecoveryRetryQueue::new(1, RECOVERY_QUEUE_ENTRY_BYTES);
        assert!(queue.schedule(simulator_id, agent_data, target, now));
        let due = queue.take_due(now);
        let [stale] = due.as_slice() else { panic!("expected in-flight recovery") };
        let stale = *stale;

        queue.cancel(simulator_id, target);
        assert!(queue.schedule(simulator_id, agent_data, target, now));
        let replacement = queue.pending.get(&RecoveryKey { simulator_id, address: target }).copied().expect("replacement recovery");
        assert_ne!(stale.token, replacement.token);

        queue.mark_sent(stale, now);
        queue.finish(stale);

        let current = queue.pending.get(&RecoveryKey { simulator_id, address: target }).expect("replacement remains queued");
        assert_eq!(current.token, replacement.token);
        assert_eq!(current.retry.sends, 0);
        assert!(!current.in_flight);
    }

    #[test]
    fn updates_and_retirement_cancel_retries() {
        let simulator_id = simulator_id();
        let target = address(simulator_id.get(), 9);
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let mut queue =
            RecoveryRetryQueue::new(MAX_RECOVERY_OBJECTS_PER_REQUEST, MAX_RECOVERY_OBJECTS_PER_REQUEST * RECOVERY_QUEUE_ENTRY_BYTES);
        queue.schedule(simulator_id, agent_data, target, Instant::now());
        queue.cancel(simulator_id, target);
        assert!(queue.next_deadline().is_none());

        queue.schedule(simulator_id, agent_data, target, Instant::now());
        queue.cancel_simulator(simulator_id);
        assert!(queue.next_deadline().is_none());
    }

    #[test]
    fn session_queue_holds_more_than_one_request_batch() {
        let simulator_id = simulator_id();
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let now = Instant::now();
        let mut queue = RecoveryRetryQueue::new(MAX_PENDING_RECOVERY_TARGETS, MAX_PENDING_RECOVERY_BYTES);

        for local_id in 1..=MAX_RECOVERY_OBJECTS_PER_REQUEST + 1 {
            assert!(queue.schedule(simulator_id, agent_data, address(simulator_id.get(), local_id as u32), now));
        }

        assert_eq!(queue.pending.len(), MAX_RECOVERY_OBJECTS_PER_REQUEST + 1);
    }

    #[test]
    fn due_dispatch_is_limited_to_one_simulator_packet() {
        let simulator_id = simulator_id();
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let now = Instant::now();
        let mut queue = RecoveryRetryQueue::new(300, 300 * RECOVERY_QUEUE_ENTRY_BYTES);
        for local_id in 1..=MAX_RECOVERY_OBJECTS_PER_REQUEST + 1 {
            assert!(queue.schedule(simulator_id, agent_data, address(simulator_id.get(), local_id as u32), now));
        }

        let first_batch = queue.take_due(now);
        assert_eq!(first_batch.len(), MAX_RECOVERY_OBJECTS_PER_REQUEST);
        assert!(first_batch.iter().all(|recovery| recovery.key.simulator_id == simulator_id));
        for recovery in first_batch {
            queue.mark_sent(recovery, now);
        }

        let remaining = queue.take_due(now);
        assert_eq!(remaining.len(), 1);
    }

    #[test]
    fn transient_failure_keeps_target_for_retry() {
        let simulator_id = simulator_id();
        let target = address(simulator_id.get(), 8);
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let now = Instant::now();
        let mut queue = RecoveryRetryQueue::new(1, RECOVERY_QUEUE_ENTRY_BYTES);
        assert!(queue.schedule(simulator_id, agent_data, target, now));

        let due = queue.take_due(now);
        let [recovery] = due.as_slice() else { panic!("expected one due recovery") };
        let recovery = *recovery;
        queue.defer(recovery, now);

        let current = queue.pending.get(&recovery.key).expect("failed send remains queued");
        assert!(!current.in_flight);
        assert_eq!(current.retry.sends, 0);
        assert_eq!(queue.next_deadline(), Some(now + RECOVERY_RETRY_INTERVAL));
        assert!(queue.take_due(now + RECOVERY_RETRY_INTERVAL - Duration::from_nanos(1)).is_empty());
        assert_eq!(queue.take_due(now + RECOVERY_RETRY_INTERVAL).len(), 1);
    }

    #[test]
    fn expired_pending_recoveries_release_their_budget() {
        let simulator_id = simulator_id();
        let target = address(simulator_id.get(), 8);
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let now = Instant::now();
        let mut queue = RecoveryRetryQueue::new(1, RECOVERY_QUEUE_ENTRY_BYTES);
        assert!(queue.schedule(simulator_id, agent_data, target, now));

        assert!(queue.take_due(now + RECOVERY_MAX_AGE).is_empty());
        assert!(queue.pending.is_empty());
        assert_eq!(queue.pending_bytes, 0);
    }

    #[test]
    fn overflow_is_aggregated_and_queue_is_bounded() {
        let simulator_id = simulator_id();
        let agent_data = RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) };
        let now = Instant::now();
        let mut queue = RecoveryRetryQueue::new(3, 2 * RECOVERY_QUEUE_ENTRY_BYTES);
        assert!(queue.schedule(simulator_id, agent_data, address(simulator_id.get(), 1), now));
        assert!(queue.schedule(simulator_id, agent_data, address(simulator_id.get(), 2), now));
        assert!(!queue.schedule(simulator_id, agent_data, address(simulator_id.get(), 3), now));
        assert_eq!(queue.pending_bytes, 2 * RECOVERY_QUEUE_ENTRY_BYTES);

        assert_eq!(queue.record_overflow(2, now), Some(2));
        assert_eq!(queue.record_overflow(3, now + Duration::from_secs(1)), None);
        assert_eq!(queue.record_overflow(4, now + Duration::from_secs(5)), Some(7));
    }

    #[test]
    fn requests_always_fetch_full_object_data() {
        let request = recovery_request(
            RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) },
            &[address(7, 10), address(7, 11)],
        );

        assert_eq!(request.objects.len(), 2);
        assert!(request.objects.iter().all(|object| object.cache_miss_type == Full));
        assert_eq!(request.objects.iter().map(|object| object.local_id).collect::<Vec<_>>(), [10, 11]);
    }

    #[test]
    fn max_request_fits_byte_budget() {
        let addresses = (1..=MAX_RECOVERY_OBJECTS_PER_REQUEST).map(|local_id| address(7, local_id as u32)).collect::<Vec<_>>();
        let request = recovery_request(RequestAgentData { agent_id: UUID::from_u128(1), session_id: UUID::from_u128(2) }, &addresses);
        let encoded = Packet::new(0, Message::RequestMultipleObjects(request)).encode().expect("bounded recovery packet");

        assert!(encoded.len() <= MAX_RECOVERY_REQUEST_BYTES);
    }
}
