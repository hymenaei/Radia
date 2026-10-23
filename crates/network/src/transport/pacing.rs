#[cfg(test)]
use std::{collections::BTreeMap, env};
use std::{
    collections::HashMap,
    io::ErrorKind,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::{
    sync::{mpsc, oneshot},
    task::{self, JoinHandle, spawn},
    time::{self, Instant},
};

use super::UdpTransport;

pub(crate) const MAX_BURST_BYTES: u64 = 8 * 1024;
const NANOS_PER_SECOND: u128 = 1_000_000_000;
const RATE_RECOVERY_INTERVAL: Duration = Duration::from_secs(1);
const RATE_RECOVERY_STEP_DENOMINATOR: u64 = 20;
const MIN_RATE_DIVISOR: u64 = 16;
const SERVICE_ORDER: [TrafficClass; 5] =
    [TrafficClass::Control, TrafficClass::Control, TrafficClass::Deadline, TrafficClass::Fair, TrafficClass::Fair];

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum TrafficClass {
    Control,
    Deadline,
    Fair,
}

impl TrafficClass {
    fn index(self) -> usize {
        match self {
            Self::Control => 0,
            Self::Deadline => 1,
            Self::Fair => 2,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionRole {
    Primary,
    Neighbor,
}

impl ConnectionRole {
    fn virtual_cost(self, bytes: usize) -> u128 {
        match self {
            Self::Primary => bytes as u128,
            Self::Neighbor => (bytes as u128).saturating_mul(2),
        }
    }
}

enum Command {
    Register { id: u64, role: ConnectionRole, active: Arc<AtomicBool> },
    Unregister { id: u64 },
    SetRole { id: u64, role: ConnectionRole },
    Request { id: u64, bytes: usize, class: TrafficClass, completed: oneshot::Sender<BudgetLease> },
    Shutdown(oneshot::Sender<()>),
}

struct PendingRequest {
    bytes: usize,
    class: TrafficClass,
    finish: u128,
    completed: oneshot::Sender<BudgetLease>,
    reserved: bool,
    bypass_slots: [usize; 3],
    #[cfg(test)]
    queued_at: Instant,
}

impl PendingRequest {
    fn new(bytes: usize, class: TrafficClass, finish: u128, completed: oneshot::Sender<BudgetLease>) -> Self {
        Self {
            bytes,
            class,
            finish,
            completed,
            reserved: false,
            bypass_slots: [0; 3],
            #[cfg(test)]
            queued_at: Instant::now(),
        }
    }
}

struct ConnectionState {
    role: ConnectionRole,
    finish: [u128; 3],
    active: Arc<AtomicBool>,
}

struct Bucket {
    rate_bytes_per_second: u64,
    max_rate_bytes_per_second: u64,
    capacity_bytes: u64,
    reserved_bytes: u64,
    credit_byte_nanos: u128,
    last_refill: Instant,
    next_loss_backoff: Instant,
    next_rate_recovery: Instant,
}

impl Bucket {
    fn new(rate_bytes_per_second: u64, now: Instant) -> Self {
        let rate_bytes_per_second = rate_bytes_per_second.max(1);
        let capacity_bytes = MAX_BURST_BYTES;
        Self {
            rate_bytes_per_second,
            max_rate_bytes_per_second: rate_bytes_per_second,
            capacity_bytes,
            reserved_bytes: 0,
            credit_byte_nanos: u128::from(capacity_bytes) * NANOS_PER_SECOND,
            last_refill: now,
            next_loss_backoff: now,
            next_rate_recovery: now + RATE_RECOVERY_INTERVAL,
        }
    }

    fn refill_credit(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last_refill).as_nanos();
        let added = elapsed.saturating_mul(u128::from(self.rate_bytes_per_second));
        let capacity = u128::from(self.capacity_bytes - self.reserved_bytes) * NANOS_PER_SECOND;
        self.credit_byte_nanos = self.credit_byte_nanos.saturating_add(added).min(capacity);
        self.last_refill = now;
    }

    fn refill(&mut self, now: Instant) {
        while self.rate_bytes_per_second < self.max_rate_bytes_per_second && now >= self.next_rate_recovery {
            self.refill_credit(self.next_rate_recovery);
            let step = (self.max_rate_bytes_per_second / RATE_RECOVERY_STEP_DENOMINATOR).max(1);
            self.rate_bytes_per_second = self.rate_bytes_per_second.saturating_add(step).min(self.max_rate_bytes_per_second);
            self.next_rate_recovery += RATE_RECOVERY_INTERVAL;
        }
        self.refill_credit(now);
    }

    fn report_loss(&mut self, now: Instant) {
        self.refill_credit(now);
        if now >= self.next_loss_backoff {
            let minimum = (self.max_rate_bytes_per_second / MIN_RATE_DIVISOR).max(1);
            self.rate_bytes_per_second = (self.rate_bytes_per_second / 2).max(minimum);
            self.next_loss_backoff = now + RATE_RECOVERY_INTERVAL;
        }
        self.next_rate_recovery = now + RATE_RECOVERY_INTERVAL;
    }

    fn take(&mut self, bytes: usize, now: Instant) -> bool {
        self.refill(now);
        let required = (bytes as u128).saturating_mul(NANOS_PER_SECOND);
        if self.credit_byte_nanos < required {
            return false;
        }
        self.credit_byte_nanos -= required;
        true
    }

    fn reserve(&mut self, bytes: usize, now: Instant) -> bool {
        if !self.take(bytes, now) {
            return false;
        }
        self.reserved_bytes = self.reserved_bytes.checked_add(bytes as u64).expect("pacing reservations stay within burst capacity");
        true
    }

    fn available_bytes(&mut self, now: Instant) -> u64 {
        self.refill(now);
        (self.credit_byte_nanos / NANOS_PER_SECOND).min(u128::from(u64::MAX)) as u64
    }

    fn settle_reservation(&mut self, bytes: usize, refund: bool, now: Instant) {
        self.refill(now);
        let bytes = bytes as u64;
        self.reserved_bytes = self.reserved_bytes.checked_sub(bytes).expect("a pacing lease settles its reservation once");
        if refund {
            let capacity = u128::from(self.capacity_bytes - self.reserved_bytes) * NANOS_PER_SECOND;
            self.credit_byte_nanos = self.credit_byte_nanos.saturating_add(u128::from(bytes) * NANOS_PER_SECOND).min(capacity);
        }
    }

    fn delay_until(&self, bytes: usize, now: Instant) -> Duration {
        let mut bucket = Bucket {
            rate_bytes_per_second: self.rate_bytes_per_second,
            max_rate_bytes_per_second: self.max_rate_bytes_per_second,
            capacity_bytes: self.capacity_bytes,
            reserved_bytes: self.reserved_bytes,
            credit_byte_nanos: self.credit_byte_nanos,
            last_refill: self.last_refill,
            next_loss_backoff: self.next_loss_backoff,
            next_rate_recovery: self.next_rate_recovery,
        };
        bucket.refill(now);
        let required = (bytes as u128) * NANOS_PER_SECOND;
        if bucket.credit_byte_nanos >= required {
            return Duration::from_millis(1);
        }
        let missing = required - bucket.credit_byte_nanos;
        let rate = u128::from(bucket.rate_bytes_per_second);
        let nanos = missing.saturating_add(rate - 1) / rate;
        let credit_delay = Duration::from_nanos(nanos.min(u128::from(u64::MAX)) as u64).max(Duration::from_millis(1));
        if bucket.rate_bytes_per_second < bucket.max_rate_bytes_per_second {
            credit_delay.min(bucket.next_rate_recovery.saturating_duration_since(now).max(Duration::from_millis(1)))
        } else {
            credit_delay
        }
    }
}

pub(crate) struct OutboundBudget {
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
    worker: JoinHandle<()>,
    #[cfg(test)]
    probe: Option<Arc<Mutex<ProbeStats>>>,
}

impl OutboundBudget {
    pub(crate) fn new(rate_bytes_per_second: u64) -> Self {
        let (commands, receiver) = mpsc::channel(256);
        let bucket = Arc::new(Mutex::new(Bucket::new(rate_bytes_per_second.max(1), Instant::now())));
        let worker_bucket = Arc::clone(&bucket);
        #[cfg(test)]
        let probe = env::var_os("RADIA_PACER_PROBE").map(|_| Arc::new(Mutex::new(ProbeStats::default())));
        let worker = spawn(run_arbiter(
            receiver,
            worker_bucket,
            #[cfg(test)]
            probe.clone(),
        ));
        Self {
            commands,
            bucket,
            worker,
            #[cfg(test)]
            probe,
        }
    }

    #[cfg(test)]
    pub(crate) async fn register(&self, id: u64, primary: bool) -> Result<ConnectionPacer, String> {
        self.handle().register(id, primary).await
    }

    pub(crate) fn handle(&self) -> OutboundBudgetHandle {
        OutboundBudgetHandle {
            commands: self.commands.clone(),
            bucket: Arc::clone(&self.bucket),
            #[cfg(test)]
            probe: self.probe.clone(),
        }
    }

    pub(crate) async fn shutdown(mut self) {
        let (completed, response) = oneshot::channel();
        let _ = self.commands.send(Command::Shutdown(completed)).await;
        let _ = response.await;
        let _ = (&mut self.worker).await;
    }
}

impl Drop for OutboundBudget {
    fn drop(&mut self) {
        #[cfg(test)]
        if let Some(probe) = &self.probe {
            report_probe(probe);
        }
        self.worker.abort();
    }
}

#[derive(Clone)]
pub(crate) struct OutboundBudgetHandle {
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
    #[cfg(test)]
    probe: Option<Arc<Mutex<ProbeStats>>>,
}

impl OutboundBudgetHandle {
    pub(crate) async fn register(&self, id: u64, primary: bool) -> Result<ConnectionPacer, String> {
        let role = if primary { ConnectionRole::Primary } else { ConnectionRole::Neighbor };
        let active = Arc::new(AtomicBool::new(true));
        self.commands
            .send(Command::Register { id, role, active: Arc::clone(&active) })
            .await
            .map_err(|_| "The LLUDP byte pacer has stopped.".to_string())?;
        Ok(ConnectionPacer {
            id,
            role,
            commands: self.commands.clone(),
            bucket: Arc::clone(&self.bucket),
            active,
            #[cfg(test)]
            probe: self.probe.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn probe(&self) -> Option<ProbeHandle> {
        self.probe.as_ref().map(|stats| ProbeHandle { stats: Arc::clone(stats) })
    }
}

pub(crate) struct ConnectionPacer {
    id: u64,
    role: ConnectionRole,
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
    active: Arc<AtomicBool>,
    #[cfg(test)]
    probe: Option<Arc<Mutex<ProbeStats>>>,
}

impl ConnectionPacer {
    pub(crate) fn handle(&self) -> PacerHandle {
        PacerHandle {
            id: self.id,
            commands: self.commands.clone(),
            bucket: Arc::clone(&self.bucket),
            #[cfg(test)]
            probe: self.probe.clone(),
        }
    }

    #[cfg(test)]
    pub(crate) fn probe(&self) -> Option<ProbeConnection> {
        self.probe.as_ref().map(|stats| ProbeConnection { id: self.id, stats: Arc::clone(stats) })
    }

    pub(crate) async fn set_primary(&mut self, primary: bool) -> Result<(), String> {
        let role = if primary { ConnectionRole::Primary } else { ConnectionRole::Neighbor };
        if self.role != role {
            self.commands
                .send(Command::SetRole { id: self.id, role })
                .await
                .map_err(|_| "The LLUDP byte pacer has stopped.".to_string())?;
            self.role = role;
        }
        Ok(())
    }
}

impl Drop for ConnectionPacer {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        let _ = self.commands.try_send(Command::Unregister { id: self.id });
    }
}

#[derive(Clone)]
pub(crate) struct PacerHandle {
    id: u64,
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
    #[cfg(test)]
    probe: Option<Arc<Mutex<ProbeStats>>>,
}

impl PacerHandle {
    pub(crate) fn report_loss(&self) {
        if let Ok(mut bucket) = self.bucket.lock() {
            bucket.report_loss(Instant::now());
        }
    }

    #[cfg(test)]
    pub(crate) fn rate_for_test(&self) -> u64 {
        self.bucket.lock().expect("session pacing bucket").rate_bytes_per_second
    }

    pub(crate) async fn send(&self, socket: &UdpTransport, bytes: &[u8], class: TrafficClass) -> Result<Instant, String> {
        self.send_with_kind(socket, bytes, class, false).await
    }

    pub(crate) async fn send_retransmission(&self, socket: &UdpTransport, bytes: &[u8], class: TrafficClass) -> Result<Instant, String> {
        self.send_with_kind(socket, bytes, class, true).await
    }

    async fn send_with_kind(
        &self,
        socket: &UdpTransport,
        bytes: &[u8],
        class: TrafficClass,
        retransmission: bool,
    ) -> Result<Instant, String> {
        #[cfg(not(test))]
        let _ = retransmission;
        #[cfg(test)]
        if retransmission && let Some(probe) = &self.probe {
            record_retry_attempt(probe, self.id, bytes.len());
        }
        loop {
            let (completed, response) = oneshot::channel();
            self.commands
                .send(Command::Request { id: self.id, bytes: bytes.len(), class, completed })
                .await
                .map_err(|_| "The LLUDP byte pacer has stopped.".to_string())?;
            let lease = response.await.map_err(|_| "The LLUDP byte pacer stopped before granting a send.".to_string())?;
            match socket.try_send(bytes) {
                Ok(()) => {
                    #[cfg(test)]
                    if let Some(probe) = &self.probe {
                        record_wire_send(probe, self.id, class, bytes.len(), retransmission);
                    }
                    return Ok(lease.consume());
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    drop(lease);
                    socket.writable().await.map_err(|error| format!("Could not wait for a simulator datagram send slot: {error}"))?;
                }
                Err(error) => {
                    #[cfg(test)]
                    if let Some(probe) = &self.probe {
                        record_send_error(probe, self.id, class);
                    }
                    return Err(format!("Could not send a simulator datagram: {error}"));
                }
            }
        }
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ProbeConnection {
    id: u64,
    stats: Arc<Mutex<ProbeStats>>,
}

#[cfg(test)]
impl ProbeConnection {
    pub(crate) fn new_for_test(id: u64) -> (Self, ProbeHandle) {
        let stats = Arc::new(Mutex::new(ProbeStats::default()));
        (Self { id, stats: Arc::clone(&stats) }, ProbeHandle { stats })
    }

    pub(crate) fn record_local_queue(
        &self,
        queued_messages: usize,
        queued_bytes: usize,
        oldest_age: Duration,
        class_ages: &[(TrafficClass, Duration)],
        at: Instant,
    ) {
        if let Ok(mut stats) = self.stats.lock() {
            let connection = stats.connections.entry(self.id).or_default();
            connection.max_queue_messages = connection.max_queue_messages.max(queued_messages);
            connection.max_queue_bytes = connection.max_queue_bytes.max(queued_bytes);
            connection.max_queue_age = connection.max_queue_age.max(oldest_age);
            connection.queue_samples.push(TimedQueueSnapshot { at, queued_messages, queued_bytes });
            for &(class, age) in class_ages {
                let metrics = connection.classes.entry(class).or_default();
                metrics.max_queue_age = metrics.max_queue_age.max(age);
                metrics.queue_age_samples.push(TimedDuration { at, duration: age });
            }
        }
    }

    pub(crate) fn record_queue_age(&self, class: TrafficClass, age: Duration, at: Instant) {
        if let Ok(mut stats) = self.stats.lock() {
            let connection = stats.connections.entry(self.id).or_default();
            connection.max_queue_age = connection.max_queue_age.max(age);
            let metrics = connection.classes.entry(class).or_default();
            metrics.max_queue_age = metrics.max_queue_age.max(age);
            metrics.queue_age_samples.push(TimedDuration { at, duration: age });
        }
    }

    pub(crate) fn record_outbound_queue_expiration(&self, class: TrafficClass, at: Instant) {
        if let Ok(mut stats) = self.stats.lock() {
            let metrics = stats.connections.entry(self.id).or_default().classes.entry(class).or_default();
            metrics.outbound_queue_expirations = metrics.outbound_queue_expirations.saturating_add(1);
            metrics.outbound_queue_expiration_samples.push(at);
        }
    }

    pub(crate) fn record_pending_send_expiration(&self, class: TrafficClass, at: Instant) {
        if let Ok(mut stats) = self.stats.lock() {
            let metrics = stats.connections.entry(self.id).or_default().classes.entry(class).or_default();
            metrics.pending_send_expirations = metrics.pending_send_expirations.saturating_add(1);
            metrics.pending_send_expiration_samples.push(at);
        }
    }

    pub(crate) fn record_deferred_retry_expirations(&self, count: usize, at: Instant) {
        if count == 0 {
            return;
        }
        if let Ok(mut stats) = self.stats.lock() {
            let connection = stats.connections.entry(self.id).or_default();
            connection.deferred_retry_expirations = connection.deferred_retry_expirations.saturating_add(count as u64);
            for _ in 0..count {
                connection.deferred_retry_expiration_samples.push(at);
            }
        }
    }

    pub(crate) fn record_active_retry_expiration(&self, at: Instant) {
        if let Ok(mut stats) = self.stats.lock() {
            let connection = stats.connections.entry(self.id).or_default();
            connection.active_retry_expirations = connection.active_retry_expirations.saturating_add(1);
            connection.active_retry_expiration_samples.push(at);
        }
    }
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct ProbeHandle {
    stats: Arc<Mutex<ProbeStats>>,
}

#[cfg(test)]
impl ProbeHandle {
    pub(crate) fn snapshot(&self, start: Instant, end: Instant) -> ProbeSnapshot {
        let Ok(stats) = self.stats.lock() else { return ProbeSnapshot::default() };
        let mut snapshot = ProbeSnapshot::default();
        if end < start {
            return snapshot;
        }

        for pair in stats.pending_samples.windows(2) {
            add_pending_duration(&mut snapshot.pending_duration, &pair[0].classes, pair[0].at, pair[1].at, start, end);
        }
        if let Some(last) = stats.pending_samples.last() {
            add_pending_duration(&mut snapshot.pending_duration, &last.classes, last.at, end, start, end);
        }

        for (&id, connection) in &stats.connections {
            let mut connection_snapshot = ProbeConnectionSnapshot::default();
            let queue_samples = connection.queue_samples.iter().filter(|sample| sample.at >= start && sample.at < end);
            for sample in queue_samples {
                connection_snapshot.peak_queue_messages = connection_snapshot.peak_queue_messages.max(sample.queued_messages);
                connection_snapshot.peak_queue_bytes = connection_snapshot.peak_queue_bytes.max(sample.queued_bytes);
            }
            let (attempts, attempt_bytes) = count_bytes(&connection.retry_attempt_samples, start, end);
            connection_snapshot.retry_attempts = attempts;
            connection_snapshot.retry_attempt_bytes = attempt_bytes;
            let (wire_datagrams, wire_bytes) = count_bytes(&connection.retry_wire_samples, start, end);
            connection_snapshot.retry_wire_datagrams = wire_datagrams;
            connection_snapshot.retry_wire_bytes = wire_bytes;
            connection_snapshot.deferred_retry_expirations = count_instants(&connection.deferred_retry_expiration_samples, start, end);
            connection_snapshot.active_retry_expirations = count_instants(&connection.active_retry_expiration_samples, start, end);

            for (&class, metrics) in &connection.classes {
                let (grants, granted_bytes) = count_bytes(&metrics.grant_samples, start, end);
                let (wire_datagrams, wire_bytes) = count_bytes(&metrics.wire_samples, start, end);
                let waits = metrics.waits.iter().filter(|sample| sample.at >= start && sample.at < end).copied().collect::<Vec<_>>();
                let peak_queue_age = metrics
                    .queue_age_samples
                    .iter()
                    .filter(|sample| sample.at >= start && sample.at < end)
                    .map(|sample| sample.duration)
                    .max()
                    .unwrap_or_default();
                connection_snapshot.peak_queue_age = connection_snapshot.peak_queue_age.max(peak_queue_age);
                connection_snapshot.classes.insert(
                    class,
                    ProbeClassSnapshot {
                        grants,
                        granted_bytes,
                        wire_datagrams,
                        wire_bytes,
                        wait_p50: percentile_waits(&waits, 50),
                        wait_p95: percentile_waits(&waits, 95),
                        wait_p99: percentile_waits(&waits, 99),
                        peak_queue_age,
                        outbound_queue_expirations: count_instants(&metrics.outbound_queue_expiration_samples, start, end),
                        pending_send_expirations: count_instants(&metrics.pending_send_expiration_samples, start, end),
                    },
                );
            }
            snapshot.connections.insert(id, connection_snapshot);
        }
        snapshot
    }
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct ProbeSnapshot {
    pub(crate) connections: BTreeMap<u64, ProbeConnectionSnapshot>,
    pub(crate) pending_duration: BTreeMap<TrafficClass, BTreeMap<usize, Duration>>,
}

#[cfg(test)]
#[derive(Clone, Debug, Default)]
pub(crate) struct ProbeConnectionSnapshot {
    pub(crate) classes: BTreeMap<TrafficClass, ProbeClassSnapshot>,
    pub(crate) peak_queue_messages: usize,
    pub(crate) peak_queue_bytes: usize,
    pub(crate) peak_queue_age: Duration,
    pub(crate) retry_attempts: u64,
    pub(crate) retry_attempt_bytes: u64,
    pub(crate) retry_wire_datagrams: u64,
    pub(crate) retry_wire_bytes: u64,
    pub(crate) deferred_retry_expirations: u64,
    pub(crate) active_retry_expirations: u64,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProbeClassSnapshot {
    pub(crate) grants: u64,
    pub(crate) granted_bytes: u64,
    pub(crate) wire_datagrams: u64,
    pub(crate) wire_bytes: u64,
    pub(crate) wait_p50: Duration,
    pub(crate) wait_p95: Duration,
    pub(crate) wait_p99: Duration,
    pub(crate) peak_queue_age: Duration,
    pub(crate) outbound_queue_expirations: u64,
    pub(crate) pending_send_expirations: u64,
}

#[cfg(test)]
#[derive(Default)]
struct ProbeStats {
    connections: BTreeMap<u64, ProbeConnectionStats>,
    pending_samples: Vec<TimedPendingCounts>,
    scheduler_pending_peak: usize,
    scheduler_oldest_wait: Duration,
    reported: bool,
}

#[cfg(test)]
#[derive(Clone)]
struct TimedPendingCounts {
    at: Instant,
    classes: BTreeMap<TrafficClass, usize>,
}

#[cfg(test)]
#[derive(Default)]
struct ProbeConnectionStats {
    role: Option<ConnectionRole>,
    classes: BTreeMap<TrafficClass, ProbeClassStats>,
    max_queue_messages: usize,
    max_queue_bytes: usize,
    max_queue_age: Duration,
    queue_samples: Vec<TimedQueueSnapshot>,
    retry_attempts: u64,
    retry_attempt_bytes: u64,
    retry_attempt_samples: Vec<TimedBytes>,
    retry_wire_datagrams: u64,
    retry_wire_bytes: u64,
    retry_wire_samples: Vec<TimedBytes>,
    deferred_retry_expirations: u64,
    deferred_retry_expiration_samples: Vec<Instant>,
    active_retry_expirations: u64,
    active_retry_expiration_samples: Vec<Instant>,
}

#[cfg(test)]
#[derive(Default)]
struct ProbeClassStats {
    grants: u64,
    granted_bytes: u64,
    grant_samples: Vec<TimedBytes>,
    wire_datagrams: u64,
    wire_bytes: u64,
    wire_samples: Vec<TimedBytes>,
    send_errors: u64,
    waits: Vec<TimedWait>,
    max_queue_age: Duration,
    queue_age_samples: Vec<TimedDuration>,
    outbound_queue_expirations: u64,
    outbound_queue_expiration_samples: Vec<Instant>,
    pending_send_expirations: u64,
    pending_send_expiration_samples: Vec<Instant>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TimedBytes {
    at: Instant,
    bytes: u64,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TimedWait {
    at: Instant,
    wait: Duration,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TimedDuration {
    at: Instant,
    duration: Duration,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct TimedQueueSnapshot {
    at: Instant,
    queued_messages: usize,
    queued_bytes: usize,
}

pub(crate) struct BudgetLease {
    bucket: Arc<Mutex<Bucket>>,
    bytes: usize,
    consumed: bool,
}

impl BudgetLease {
    fn new(bucket: Arc<Mutex<Bucket>>, bytes: usize) -> Self {
        Self { bucket, bytes, consumed: false }
    }

    fn consume(mut self) -> Instant {
        let now = Instant::now();
        if let Ok(mut bucket) = self.bucket.lock() {
            bucket.settle_reservation(self.bytes, false, now);
        }
        self.consumed = true;
        now
    }
}

impl Drop for BudgetLease {
    fn drop(&mut self) {
        if !self.consumed
            && let Ok(mut bucket) = self.bucket.lock()
        {
            bucket.settle_reservation(self.bytes, true, Instant::now());
        }
    }
}

#[cfg(test)]
fn record_pending(probe: &Arc<Mutex<ProbeStats>>, pending: &HashMap<u64, PendingRequest>) {
    let now = Instant::now();
    let oldest = pending.values().map(|request| now.saturating_duration_since(request.queued_at)).max().unwrap_or_default();
    let mut counts = BTreeMap::new();
    for request in pending.values() {
        let count = counts.entry(request.class).or_insert(0usize);
        *count = count.saturating_add(1);
    }
    if let Ok(mut stats) = probe.lock() {
        stats.scheduler_pending_peak = stats.scheduler_pending_peak.max(pending.len());
        stats.scheduler_oldest_wait = stats.scheduler_oldest_wait.max(oldest);
        push_pending_counts(&mut stats, counts, now);
    }
}

#[cfg(test)]
fn push_pending_counts(stats: &mut ProbeStats, classes: BTreeMap<TrafficClass, usize>, at: Instant) {
    if stats.pending_samples.last().is_none_or(|sample| sample.classes != classes) {
        stats.pending_samples.push(TimedPendingCounts { at, classes });
    }
}

#[cfg(test)]
fn record_grant(probe: &Arc<Mutex<ProbeStats>>, id: u64, role: ConnectionRole, class: TrafficClass, bytes: usize, wait: Duration) {
    record_grant_at(probe, id, role, class, bytes, wait, Instant::now());
}

#[cfg(test)]
fn record_grant_at(
    probe: &Arc<Mutex<ProbeStats>>,
    id: u64,
    role: ConnectionRole,
    class: TrafficClass,
    bytes: usize,
    wait: Duration,
    at: Instant,
) {
    if let Ok(mut stats) = probe.lock() {
        let connection = stats.connections.entry(id).or_default();
        connection.role = Some(role);
        let class = connection.classes.entry(class).or_default();
        class.grants += 1;
        let bytes = bytes as u64;
        class.granted_bytes = class.granted_bytes.saturating_add(bytes);
        class.grant_samples.push(TimedBytes { at, bytes });
        class.waits.push(TimedWait { at, wait });
    }
}

#[cfg(test)]
fn record_wire_send(probe: &Arc<Mutex<ProbeStats>>, id: u64, class: TrafficClass, bytes: usize, retransmission: bool) {
    record_wire_send_at(probe, id, class, bytes, retransmission, Instant::now());
}

#[cfg(test)]
fn record_wire_send_at(probe: &Arc<Mutex<ProbeStats>>, id: u64, class: TrafficClass, bytes: usize, retransmission: bool, at: Instant) {
    if let Ok(mut stats) = probe.lock() {
        let connection = stats.connections.entry(id).or_default();
        let class = connection.classes.entry(class).or_default();
        let bytes = bytes as u64;
        class.wire_datagrams = class.wire_datagrams.saturating_add(1);
        class.wire_bytes = class.wire_bytes.saturating_add(bytes);
        class.wire_samples.push(TimedBytes { at, bytes });
        if retransmission {
            connection.retry_wire_datagrams = connection.retry_wire_datagrams.saturating_add(1);
            connection.retry_wire_bytes = connection.retry_wire_bytes.saturating_add(bytes);
            connection.retry_wire_samples.push(TimedBytes { at, bytes });
        }
    }
}

#[cfg(test)]
fn record_retry_attempt(probe: &Arc<Mutex<ProbeStats>>, id: u64, bytes: usize) {
    record_retry_attempt_at(probe, id, bytes, Instant::now());
}

#[cfg(test)]
fn record_retry_attempt_at(probe: &Arc<Mutex<ProbeStats>>, id: u64, bytes: usize, at: Instant) {
    if let Ok(mut stats) = probe.lock() {
        let connection = stats.connections.entry(id).or_default();
        let bytes = bytes as u64;
        connection.retry_attempts = connection.retry_attempts.saturating_add(1);
        connection.retry_attempt_bytes = connection.retry_attempt_bytes.saturating_add(bytes);
        connection.retry_attempt_samples.push(TimedBytes { at, bytes });
    }
}

#[cfg(test)]
fn record_send_error(probe: &Arc<Mutex<ProbeStats>>, id: u64, class: TrafficClass) {
    if let Ok(mut stats) = probe.lock() {
        let class = stats.connections.entry(id).or_default().classes.entry(class).or_default();
        class.send_errors = class.send_errors.saturating_add(1);
    }
}

#[cfg(test)]
fn percentile_waits(values: &[TimedWait], percent: usize) -> Duration {
    let mut sorted = values.iter().map(|sample| sample.wait).collect::<Vec<_>>();
    sorted.sort_unstable();
    sorted.get((sorted.len().saturating_sub(1)) * percent / 100).copied().unwrap_or_default()
}

#[cfg(test)]
fn count_bytes(samples: &[TimedBytes], start: Instant, end: Instant) -> (u64, u64) {
    samples
        .iter()
        .filter(|sample| sample.at >= start && sample.at < end)
        .fold((0, 0), |(count, bytes), sample| (count.saturating_add(1), bytes.saturating_add(sample.bytes)))
}

#[cfg(test)]
fn count_instants(samples: &[Instant], start: Instant, end: Instant) -> u64 {
    samples.iter().filter(|&&at| at >= start && at < end).count() as u64
}

#[cfg(test)]
fn add_pending_duration(
    pending_duration: &mut BTreeMap<TrafficClass, BTreeMap<usize, Duration>>,
    classes: &BTreeMap<TrafficClass, usize>,
    interval_start: Instant,
    interval_end: Instant,
    start: Instant,
    end: Instant,
) {
    let from = interval_start.max(start);
    let to = interval_end.min(end);
    if to <= from {
        return;
    }
    let duration = to.duration_since(from);
    for (&class, &count) in classes {
        let total = pending_duration.entry(class).or_default().entry(count).or_default();
        *total = total.saturating_add(duration);
    }
}

#[cfg(test)]
fn report_probe(probe: &Arc<Mutex<ProbeStats>>) {
    let Ok(mut stats) = probe.lock() else { return };
    if stats.reported {
        return;
    }
    stats.reported = true;
    eprintln!(
        "pacer probe: scheduler-pending-peak={} scheduler-oldest-wait-peak={:?}",
        stats.scheduler_pending_peak, stats.scheduler_oldest_wait
    );
    for (id, connection) in &stats.connections {
        for (class, metrics) in &connection.classes {
            eprintln!(
                "pacer probe simulator={id} role={:?} class={class:?} grants={} granted-bytes={} datagram-bytes={} send-errors={} queue-peak-oldest-age={:?} queue-expired={} pending-send-expired={} scheduler-wait p50={:?} p95={:?} p99={:?}",
                connection.role,
                metrics.grants,
                metrics.granted_bytes,
                metrics.wire_bytes,
                metrics.send_errors,
                metrics.max_queue_age,
                metrics.outbound_queue_expirations,
                metrics.pending_send_expirations,
                percentile_waits(&metrics.waits, 50),
                percentile_waits(&metrics.waits, 95),
                percentile_waits(&metrics.waits, 99),
            );
        }
        eprintln!(
            "pacer probe simulator={id} local-queue peak-messages={} peak-charged-bytes={} peak-oldest-age={:?} retransmission-attempts={} attempted-bytes={} datagram-bytes={} deferred-retry-expired={} active-retry-expired={}",
            connection.max_queue_messages,
            connection.max_queue_bytes,
            connection.max_queue_age,
            connection.retry_attempts,
            connection.retry_attempt_bytes,
            connection.retry_wire_bytes,
            connection.deferred_retry_expirations,
            connection.active_retry_expirations,
        );
    }
}

async fn run_arbiter(
    mut commands: mpsc::Receiver<Command>,
    bucket: Arc<Mutex<Bucket>>,
    #[cfg(test)] probe: Option<Arc<Mutex<ProbeStats>>>,
) {
    let mut connections = HashMap::<u64, ConnectionState>::new();
    let mut order = Vec::<u64>::new();
    let mut pending = HashMap::<u64, PendingRequest>::new();
    let mut class_cursor = 0usize;
    let mut connection_cursor = [0usize; 3];
    let mut virtual_time = [0u128; 3];

    loop {
        retire_inactive_connections(&mut connections, &mut order, &mut pending, &mut connection_cursor);
        let dispatched = dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            #[cfg(test)]
            probe.as_ref(),
        );

        if dispatched {
            task::yield_now().await;
        }
        while let Ok(command) = commands.try_recv() {
            if !handle_command(command, &mut connections, &mut order, &mut pending, &mut connection_cursor, &virtual_time) {
                return;
            }
        }
        if dispatched {
            continue;
        }

        let next_wait = next_dispatch_delay(&bucket, &pending);
        let command = if pending.is_empty() {
            commands.recv().await
        } else {
            match time::timeout(next_wait, commands.recv()).await {
                Ok(command) => command,
                Err(_) => continue,
            }
        };
        match command {
            Some(command) => {
                if !handle_command(command, &mut connections, &mut order, &mut pending, &mut connection_cursor, &virtual_time) {
                    return;
                }
            }
            None => return,
        }
    }
}

fn handle_command(
    command: Command,
    connections: &mut HashMap<u64, ConnectionState>,
    order: &mut Vec<u64>,
    pending: &mut HashMap<u64, PendingRequest>,
    connection_cursor: &mut [usize; 3],
    virtual_time: &[u128; 3],
) -> bool {
    match command {
        Command::Register { id, role, active } => {
            if !connections.contains_key(&id) {
                order.push(id);
            }
            connections.insert(id, ConnectionState { role, finish: *virtual_time, active });
        }
        Command::Unregister { id } => {
            connections.remove(&id);
            pending.remove(&id);
            order.retain(|candidate| *candidate != id);
            connection_cursor.fill(0);
        }
        Command::SetRole { id, role } => {
            if let Some(connection) = connections.get_mut(&id) {
                connection.role = role;
                connection.finish = *virtual_time;
                if let Some(request) = pending.get_mut(&id) {
                    let class_index = request.class.index();
                    request.finish = virtual_time[class_index].saturating_add(role.virtual_cost(request.bytes));
                }
            }
        }
        Command::Request { id, bytes, class, completed } => {
            queue_request(id, bytes, class, completed, connections, pending, virtual_time);
        }
        Command::Shutdown(completed) => {
            let _ = completed.send(());
            return false;
        }
    }
    true
}

fn queue_request(
    id: u64,
    bytes: usize,
    class: TrafficClass,
    completed: oneshot::Sender<BudgetLease>,
    connections: &HashMap<u64, ConnectionState>,
    pending: &mut HashMap<u64, PendingRequest>,
    virtual_time: &[u128; 3],
) {
    if pending.get(&id).is_some_and(|request| request.completed.is_closed()) {
        pending.remove(&id);
    }
    if completed.is_closed()
        || !connections.get(&id).is_some_and(|connection| connection.active.load(Ordering::Acquire))
        || pending.contains_key(&id)
        || bytes == 0
        || bytes > MAX_BURST_BYTES as usize
    {
        return;
    }
    let class_index = class.index();
    let connection = &connections[&id];
    let start = connection.finish[class_index].max(virtual_time[class_index]);
    let finish = start.saturating_add(connection.role.virtual_cost(bytes));
    pending.insert(id, PendingRequest::new(bytes, class, finish, completed));
}

#[cfg(test)]
fn dispatch(
    bucket: &Arc<Mutex<Bucket>>,
    connections: &mut HashMap<u64, ConnectionState>,
    order: &[u64],
    pending: &mut HashMap<u64, PendingRequest>,
    class_cursor: &mut usize,
    connection_cursor: &mut [usize; 3],
    virtual_time: &mut [u128; 3],
) {
    while dispatch_with_probe(
        bucket,
        connections,
        order,
        pending,
        class_cursor,
        connection_cursor,
        virtual_time,
        #[cfg(test)]
        None,
    ) {}
}

fn dispatch_with_probe(
    bucket: &Arc<Mutex<Bucket>>,
    connections: &mut HashMap<u64, ConnectionState>,
    order: &[u64],
    pending: &mut HashMap<u64, PendingRequest>,
    class_cursor: &mut usize,
    connection_cursor: &mut [usize; 3],
    virtual_time: &mut [u128; 3],
    #[cfg(test)] probe: Option<&Arc<Mutex<ProbeStats>>>,
) -> bool {
    pending.retain(|_, request| !request.completed.is_closed());
    #[cfg(test)]
    if let Some(probe) = probe {
        record_pending(probe, pending);
    }
    if pending.is_empty() || order.is_empty() {
        return false;
    }

    if let Some((reserved_id, reserved_class, bypass_slots)) =
        pending.iter().find(|(_, request)| request.reserved).map(|(&id, request)| (id, request.class, request.bypass_slots))
    {
        let reserved_slot = *class_cursor;
        for offset in 1..SERVICE_ORDER.len() {
            let slot = (reserved_slot + offset) % SERVICE_ORDER.len();
            let class = SERVICE_ORDER[slot];
            let class_index = class.index();
            if class_index >= reserved_class.index() || bypass_slots[class_index] == 0 {
                continue;
            }
            let available = bucket.lock().map(|mut bucket| bucket.available_bytes(Instant::now())).unwrap_or(0);
            let Some((_, request_offset, id, _bytes)) =
                next_request_for_class(class, order, pending, connection_cursor, Some(available as usize))
            else {
                continue;
            };
            if grant_request(
                bucket,
                connections,
                order,
                pending,
                class_cursor,
                connection_cursor,
                virtual_time,
                id,
                class,
                reserved_slot,
                request_offset,
                false,
                false,
                #[cfg(test)]
                probe,
            ) {
                pending.get_mut(&reserved_id).expect("reserved request remains pending").bypass_slots[class_index] -= 1;
                return true;
            }
        }

        let class_index = reserved_class.index();
        let index = order.iter().position(|candidate| *candidate == reserved_id).expect("reserved request has a registered connection");
        let offset = (index + order.len() - connection_cursor[class_index]) % order.len();
        return grant_request(
            bucket,
            connections,
            order,
            pending,
            class_cursor,
            connection_cursor,
            virtual_time,
            reserved_id,
            reserved_class,
            reserved_slot,
            offset,
            true,
            false,
            #[cfg(test)]
            probe,
        );
    }

    let start_cursor = *class_cursor;
    for offset in 0..SERVICE_ORDER.len() {
        let slot = (start_cursor + offset) % SERVICE_ORDER.len();
        let class = SERVICE_ORDER[slot];
        if let Some((finish, offset, id, bytes)) = next_request_for_class(class, order, pending, connection_cursor, None) {
            let available = bucket.lock().map(|mut bucket| bucket.available_bytes(Instant::now())).unwrap_or(0);
            if bytes as u64 > available {
                let affordable = next_request_for_class(class, order, pending, connection_cursor, Some(available as usize));
                if !affordable.is_some_and(|candidate| candidate.0 <= finish) {
                    if !has_smaller_competitor(pending, id, bytes) {
                        continue;
                    }
                    let request = pending.get_mut(&id).expect("selected request remains pending");
                    request.reserved = true;
                    request.bypass_slots = higher_priority_slots(class, slot);
                    *class_cursor = slot;
                    return false;
                }
                let (_, offset, id, _) = affordable.expect("an affordable request was selected");
                return grant_request(
                    bucket,
                    connections,
                    order,
                    pending,
                    class_cursor,
                    connection_cursor,
                    virtual_time,
                    id,
                    class,
                    slot,
                    offset,
                    true,
                    true,
                    #[cfg(test)]
                    probe,
                );
            }
            return grant_request(
                bucket,
                connections,
                order,
                pending,
                class_cursor,
                connection_cursor,
                virtual_time,
                id,
                class,
                slot,
                offset,
                true,
                true,
                #[cfg(test)]
                probe,
            );
        }
    }
    *class_cursor = (start_cursor + SERVICE_ORDER.len()) % SERVICE_ORDER.len();
    false
}

fn next_request_for_class(
    class: TrafficClass,
    order: &[u64],
    pending: &HashMap<u64, PendingRequest>,
    connection_cursor: &[usize; 3],
    max_bytes: Option<usize>,
) -> Option<(u128, usize, u64, usize)> {
    let class_index = class.index();
    let mut selected: Option<(u128, usize, u64, usize)> = None;
    for offset in 0..order.len() {
        let id = order[(connection_cursor[class_index] + offset) % order.len()];
        let Some(request) = pending.get(&id) else { continue };
        if request.class != class || max_bytes.is_some_and(|max_bytes| request.bytes > max_bytes) {
            continue;
        }
        let candidate = (request.finish, offset, id, request.bytes);
        if selected.is_none_or(|best| (candidate.0, candidate.1) < (best.0, best.1)) {
            selected = Some(candidate);
        }
    }
    selected
}

fn higher_priority_slots(class: TrafficClass, slot: usize) -> [usize; 3] {
    let mut slots = [0; 3];
    for offset in 1..SERVICE_ORDER.len() {
        let higher_priority = SERVICE_ORDER[(slot + offset) % SERVICE_ORDER.len()];
        if higher_priority.index() < class.index() {
            slots[higher_priority.index()] += 1;
        }
    }
    slots
}

fn has_smaller_competitor(pending: &HashMap<u64, PendingRequest>, id: u64, bytes: usize) -> bool {
    pending.iter().any(|(&candidate, request)| candidate != id && request.bytes < bytes)
}

fn grant_request(
    bucket: &Arc<Mutex<Bucket>>,
    connections: &mut HashMap<u64, ConnectionState>,
    order: &[u64],
    pending: &mut HashMap<u64, PendingRequest>,
    class_cursor: &mut usize,
    connection_cursor: &mut [usize; 3],
    virtual_time: &mut [u128; 3],
    id: u64,
    class: TrafficClass,
    slot: usize,
    offset: usize,
    advance_class_cursor: bool,
    reserve_on_failure: bool,
    #[cfg(test)] probe: Option<&Arc<Mutex<ProbeStats>>>,
) -> bool {
    let class_index = class.index();
    let Some(request) = pending.remove(&id) else { return false };
    if !bucket.lock().is_ok_and(|mut bucket| bucket.reserve(request.bytes, Instant::now())) {
        let mut request = request;
        if reserve_on_failure {
            request.reserved = true;
            request.bypass_slots = higher_priority_slots(class, slot);
        }
        pending.insert(id, request);
        if reserve_on_failure {
            *class_cursor = slot;
        }
        return false;
    }
    let connection = connections.get_mut(&id).expect("pending request has a registered connection");
    connection.finish[class_index] = request.finish;
    if advance_class_cursor {
        *class_cursor = (slot + 1) % SERVICE_ORDER.len();
    }
    #[cfg(test)]
    let role = connection.role;
    virtual_time[class_index] = virtual_time[class_index].max(request.finish);
    let index = (connection_cursor[class_index] + offset) % order.len();
    connection_cursor[class_index] = (index + 1) % order.len();
    #[cfg(test)]
    if let Some(probe) = probe {
        record_grant(probe, id, role, class, request.bytes, Instant::now().saturating_duration_since(request.queued_at));
    }
    let _ = request.completed.send(BudgetLease::new(Arc::clone(bucket), request.bytes));
    true
}

fn retire_inactive_connections(
    connections: &mut HashMap<u64, ConnectionState>,
    order: &mut Vec<u64>,
    pending: &mut HashMap<u64, PendingRequest>,
    connection_cursor: &mut [usize; 3],
) {
    let inactive =
        connections.iter().filter_map(|(&id, connection)| (!connection.active.load(Ordering::Acquire)).then_some(id)).collect::<Vec<_>>();
    if inactive.is_empty() {
        return;
    }
    for id in inactive {
        connections.remove(&id);
        pending.remove(&id);
    }
    order.retain(|id| connections.contains_key(id));
    connection_cursor.fill(0);
}

fn next_dispatch_delay(bucket: &Arc<Mutex<Bucket>>, pending: &HashMap<u64, PendingRequest>) -> Duration {
    let Some(bytes) = pending.values().find(|request| request.reserved).map_or_else(
        || pending.values().map(|request| request.bytes).min(),
        |reserved| {
            pending
                .values()
                .filter(|request| {
                    request.reserved || (request.class.index() < reserved.class.index() && reserved.bypass_slots[request.class.index()] > 0)
                })
                .map(|request| request.bytes)
                .min()
        },
    ) else {
        return Duration::from_secs(24 * 60 * 60);
    };
    bucket.lock().map(|bucket| bucket.delay_until(bytes, Instant::now())).unwrap_or(Duration::from_millis(1))
}

#[cfg(test)]
mod tests {
    use super::{
        Bucket, Command, ConnectionRole, MAX_BURST_BYTES, NANOS_PER_SECOND, OutboundBudget, OutboundBudgetHandle, PendingRequest,
        ProbeStats, TrafficClass, dispatch, dispatch_with_probe, next_dispatch_delay, push_pending_counts, queue_request, record_grant_at,
        record_retry_attempt_at, record_wire_send_at,
    };
    use crate::simulator::MAX_NEIGHBOR_SIMULATORS;
    use crate::transport::UdpTransport;
    use std::{
        collections::{BTreeMap, HashMap, HashSet},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use tokio::{
        net::UdpSocket,
        sync::{mpsc, oneshot},
        task::{self, spawn},
        time::{Instant, advance, timeout},
    };

    #[tokio::test(flavor = "current_thread")]
    async fn dropping_or_cancelling_shutdown_stops_a_saturated_worker() {
        use std::{
            future::Future,
            pin::pin,
            task::{Context, Waker},
        };

        for cancel_shutdown in [false, true] {
            let budget = OutboundBudget::new(1);
            let handle = budget.handle();
            for _ in 0..handle.commands.capacity() {
                handle.commands.try_send(Command::Unregister { id: 1 }).expect("fill worker queue before it runs");
            }
            if cancel_shutdown {
                let mut shutdown = pin!(budget.shutdown());
                assert!(shutdown.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
            } else {
                drop(budget);
            }
            timeout(Duration::from_secs(1), handle.commands.closed())
                .await
                .unwrap_or_else(|_| panic!("worker outlived its owner; cancel_shutdown={cancel_shutdown}"));
        }
    }

    async fn queue_worker_request(pacer: &super::PacerHandle, bytes: usize) -> oneshot::Receiver<super::BudgetLease> {
        let (completed, receiver) = oneshot::channel();
        pacer
            .commands
            .send(super::Command::Request { id: pacer.id, bytes, class: TrafficClass::Fair, completed })
            .await
            .expect("pacing worker command channel");
        receiver
    }

    fn consume_ready(receiver: &mut oneshot::Receiver<super::BudgetLease>) -> bool {
        match receiver.try_recv() {
            Ok(lease) => {
                lease.consume();
                true
            }
            Err(_) => false,
        }
    }

    fn connection(role: ConnectionRole) -> super::ConnectionState {
        super::ConnectionState { role, finish: [0; 3], active: Arc::new(AtomicBool::new(true)) }
    }

    #[test]
    fn probe_window_filters_events() {
        let stats = Arc::new(Mutex::new(ProbeStats::default()));
        let (commands, _receiver) = mpsc::channel(1);
        let bucket = Arc::new(Mutex::new(Bucket::new(1, Instant::now())));
        let budget = OutboundBudgetHandle { commands, bucket, probe: Some(Arc::clone(&stats)) };
        let probe = budget.probe().expect("enabled probe");
        let base = Instant::now();
        let start = base + Duration::from_secs(1);
        let end = base + Duration::from_secs(3);

        record_grant_at(&stats, 7, ConnectionRole::Primary, TrafficClass::Fair, 1, Duration::from_millis(1), base);
        record_grant_at(&stats, 7, ConnectionRole::Primary, TrafficClass::Fair, 10, Duration::from_millis(10), start);
        record_grant_at(
            &stats,
            7,
            ConnectionRole::Primary,
            TrafficClass::Fair,
            20,
            Duration::from_millis(20),
            start + Duration::from_millis(500),
        );
        record_grant_at(
            &stats,
            7,
            ConnectionRole::Primary,
            TrafficClass::Fair,
            30,
            Duration::from_millis(30),
            start + Duration::from_secs(1),
        );
        record_grant_at(
            &stats,
            7,
            ConnectionRole::Primary,
            TrafficClass::Fair,
            40,
            Duration::from_millis(40),
            start + Duration::from_millis(1500),
        );
        record_grant_at(&stats, 7, ConnectionRole::Primary, TrafficClass::Fair, 50, Duration::from_millis(50), end);

        record_wire_send_at(&stats, 7, TrafficClass::Fair, 1, false, base);
        record_wire_send_at(&stats, 7, TrafficClass::Fair, 100, false, start);
        record_wire_send_at(&stats, 7, TrafficClass::Fair, 200, true, start + Duration::from_secs(1));
        record_wire_send_at(&stats, 7, TrafficClass::Fair, 999, false, end);
        record_retry_attempt_at(&stats, 7, 10, base);
        record_retry_attempt_at(&stats, 7, 200, start);
        record_retry_attempt_at(&stats, 7, 300, start + Duration::from_secs(1));
        record_retry_attempt_at(&stats, 7, 999, end);

        {
            let mut stats = stats.lock().expect("probe stats");
            push_pending_counts(&mut stats, BTreeMap::new(), base);
            push_pending_counts(&mut stats, BTreeMap::from([(TrafficClass::Fair, 5)]), start);
            push_pending_counts(&mut stats, BTreeMap::from([(TrafficClass::Fair, 2)]), start + Duration::from_secs(1));
            push_pending_counts(&mut stats, BTreeMap::new(), end);
        }

        let snapshot = probe.snapshot(start, end);
        let connection = snapshot.connections.get(&7).expect("recorded simulator");
        let fair = connection.classes.get(&TrafficClass::Fair).expect("recorded Fair traffic");
        assert_eq!((fair.grants, fair.granted_bytes), (4, 100));
        assert_eq!((fair.wire_datagrams, fair.wire_bytes), (2, 300));
        assert_eq!(fair.wait_p50, Duration::from_millis(20));
        assert_eq!(fair.wait_p95, Duration::from_millis(30));
        assert_eq!(fair.wait_p99, Duration::from_millis(30));
        assert_eq!((connection.retry_attempts, connection.retry_attempt_bytes), (2, 500));
        assert_eq!((connection.retry_wire_datagrams, connection.retry_wire_bytes), (1, 200));
        let fair_pending = snapshot.pending_duration.get(&TrafficClass::Fair).expect("fair queue samples");
        assert_eq!(fair_pending.get(&5), Some(&Duration::from_secs(1)));
        assert_eq!(fair_pending.get(&2), Some(&Duration::from_secs(1)));
        assert!(probe.snapshot(end, start).connections.is_empty(), "reversed windows should be empty");
    }

    #[test]
    fn dropped_registration_is_retired_when_command_queue_is_full() {
        let (commands, mut receiver) = mpsc::channel(1);
        let (completed, _) = oneshot::channel();
        assert!(commands.try_send(super::Command::Shutdown(completed)).is_ok());

        let active = Arc::new(AtomicBool::new(true));
        let pacer = super::ConnectionPacer {
            id: 7,
            role: ConnectionRole::Neighbor,
            commands: commands.clone(),
            bucket: Arc::new(Mutex::new(Bucket::new(1, Instant::now()))),
            active: Arc::clone(&active),
            probe: None,
        };
        let mut connections =
            HashMap::from([(7, super::ConnectionState { role: ConnectionRole::Neighbor, finish: [0; 3], active: Arc::clone(&active) })]);
        let mut order = vec![7];
        let (completed, mut response) = oneshot::channel();
        let mut pending = HashMap::from([(7, PendingRequest::new(1_000, TrafficClass::Fair, 0, completed))]);
        let mut connection_cursor = [0; 3];

        drop(pacer);
        assert!(!active.load(Ordering::Acquire));
        assert_eq!(commands.capacity(), 0, "the unregister command could not enter the full queue");

        super::retire_inactive_connections(&mut connections, &mut order, &mut pending, &mut connection_cursor);

        assert!(connections.is_empty());
        assert!(order.is_empty());
        assert!(pending.is_empty());
        assert!(matches!(response.try_recv(), Err(oneshot::error::TryRecvError::Closed)));
        assert!(matches!(receiver.try_recv(), Ok(super::Command::Shutdown(_))));
    }

    #[tokio::test(start_paused = true)]
    async fn bucket_charges_refunds_and_caps_bursts() {
        let now = Instant::now();
        let mut bucket = Bucket::new(8_000, now);
        assert!(bucket.take(MAX_BURST_BYTES as usize, now));
        assert!(!bucket.take(1, now));

        advance(Duration::from_millis(125)).await;
        assert!(bucket.reserve(1_000, Instant::now()));
        bucket.settle_reservation(1_000, true, Instant::now());
        assert!(bucket.take(1_000, Instant::now()));
        assert!(!bucket.take(1, Instant::now()));
    }

    #[test]
    fn bucket_keeps_a_packet_sized_bounded_burst_at_low_rates() {
        let bucket = Bucket::new(1, Instant::now());
        assert_eq!(bucket.capacity_bytes, MAX_BURST_BYTES);
    }

    #[test]
    fn reliable_retry_reduces_rate_and_quiet_period_recovers_toward_ceiling() {
        let start = Instant::now();
        let mut bucket = Bucket::new(20_000, start);

        bucket.report_loss(start + Duration::from_secs(1));
        assert_eq!(bucket.rate_bytes_per_second, 10_000);

        bucket.refill(start + Duration::from_secs(2));
        assert_eq!(bucket.rate_bytes_per_second, 11_000, "recovery adds five percent of the configured ceiling per second");

        for _ in 0..8 {
            bucket.report_loss(start + Duration::from_secs(2));
        }
        assert_eq!(bucket.rate_bytes_per_second, 5_500, "one retry burst should produce one decrease");

        bucket.report_loss(start + Duration::from_secs(3));
        assert_eq!(bucket.rate_bytes_per_second, 2_750, "a retry at the recovery boundary should still reduce the rate");
    }

    #[tokio::test(start_paused = true)]
    async fn worker_shares_configured_rate() {
        let budget = OutboundBudget::new(5_000);
        let primary = budget.register(1, true).await.expect("primary registration");
        let neighbor = budget.register(2, false).await.expect("neighbor registration");
        let primary_handle = primary.handle();
        let neighbor_handle = neighbor.handle();

        queue_worker_request(&primary_handle, MAX_BURST_BYTES as usize).await.await.expect("initial burst lease").consume();

        let mut primary_request = queue_worker_request(&primary_handle, 1_000).await;
        let mut neighbor_request = queue_worker_request(&neighbor_handle, 1_000).await;
        task::yield_now().await;
        assert!(primary_request.try_recv().is_err());
        assert!(neighbor_request.try_recv().is_err());

        advance(Duration::from_millis(199)).await;
        task::yield_now().await;
        assert!(primary_request.try_recv().is_err());
        assert!(neighbor_request.try_recv().is_err());

        advance(Duration::from_millis(1)).await;
        task::yield_now().await;
        let first_grants = usize::from(consume_ready(&mut primary_request)) + usize::from(consume_ready(&mut neighbor_request));
        assert_eq!(first_grants, 1, "the configured 5,000-byte rate yields one 1,000-byte send every 200 ms");

        advance(Duration::from_millis(199)).await;
        task::yield_now().await;
        assert!(primary_request.try_recv().is_err());
        assert!(neighbor_request.try_recv().is_err());

        advance(Duration::from_millis(1)).await;
        task::yield_now().await;
        let second_grants = usize::from(consume_ready(&mut primary_request)) + usize::from(consume_ready(&mut neighbor_request));
        assert_eq!(second_grants, 1, "both simulators share the session's configured byte budget");

        drop(primary_handle);
        drop(neighbor_handle);
        drop(primary);
        drop(neighbor);
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_reweights_after_promotion() {
        let budget = OutboundBudget::new(5_000);
        let mut pacers = Vec::new();
        for id in 0..=5 {
            pacers.push(budget.register(id, id == 1).await.expect("register simulator"));
        }
        let handles = pacers.iter().map(super::ConnectionPacer::handle).collect::<Vec<_>>();

        queue_worker_request(&handles[0], MAX_BURST_BYTES as usize).await.await.expect("drain initial burst").consume();
        let mut requests = Vec::new();
        for handle in &handles[1..] {
            requests.push(queue_worker_request(handle, 500).await);
        }

        for phase in 0..2 {
            let mut counts = [0; 5];
            for _ in 0..90 {
                advance(Duration::from_millis(100)).await;
                task::yield_now().await;

                let mut granted = None;
                for (index, request) in requests.iter_mut().enumerate() {
                    if let Ok(lease) = request.try_recv() {
                        lease.consume();
                        assert!(granted.replace(index).is_none(), "one 500-byte request should use each refill");
                    }
                }
                let index = granted.expect("each 100 ms refill should grant one queued request");
                counts[index] += 1;
                requests[index] = queue_worker_request(&handles[index + 1], 500).await;
            }

            let primary = if phase == 0 { 0 } else { 1 };
            assert_eq!(counts[primary], 30, "the active primary should receive twice a neighbor's share");
            for (index, count) in counts.iter().enumerate() {
                if index != primary {
                    assert_eq!(*count, 15, "neighbor {index} should receive its weighted share");
                }
            }

            if phase == 0 {
                pacers[1].set_primary(false).await.expect("demote the former primary");
                pacers[2].set_primary(true).await.expect("promote the selected neighbor");
                task::yield_now().await;
            }
        }

        drop(handles);
        drop(pacers);
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_weights_small_bursts() {
        const PACKET_BYTES: usize = 46;

        let budget = OutboundBudget::new(5_000);
        let dummy = budget.register(99, false).await.expect("register burst drain");
        let dummy_handle = dummy.handle();
        queue_worker_request(&dummy_handle, MAX_BURST_BYTES as usize).await.await.expect("drain initial burst").consume();
        drop(dummy_handle);
        drop(dummy);

        let mut pacers = Vec::new();
        for id in 1..=5 {
            pacers.push(budget.register(id, id == 1).await.expect("register simulator"));
        }
        let mut counters = Vec::new();
        let mut tasks = Vec::new();
        for pacer in &pacers {
            let handle = pacer.handle();
            let count = Arc::new(AtomicUsize::new(0));
            let task_count = Arc::clone(&count);
            tasks.push(spawn(async move {
                loop {
                    let Ok(lease) = queue_worker_request(&handle, PACKET_BYTES).await.await else {
                        break;
                    };
                    lease.consume();
                    task_count.fetch_add(1, Ordering::Relaxed);
                }
            }));
            counters.push(count);
        }

        for _ in 0..20 {
            advance(Duration::from_millis(100)).await;
            for _ in 0..8 {
                task::yield_now().await;
            }
        }

        for task in tasks {
            task.abort();
            let _ = task.await;
        }
        let primary = counters[0].load(Ordering::Relaxed);
        let neighbor_average = counters[1..].iter().map(|count| count.load(Ordering::Relaxed)).sum::<usize>() as f64 / 4.0;
        assert!(neighbor_average > 0.0);
        assert!(primary as f64 >= neighbor_average * 1.5, "primary={primary}, neighbor average={neighbor_average}");

        drop(pacers);
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn scheduler_shares_one_bucket_across_connections() {
        let bucket = Arc::new(Mutex::new(Bucket::new(8_000, Instant::now())));
        let mut connections = HashMap::new();
        let mut order = Vec::new();
        let mut pending = HashMap::new();
        let mut receivers = Vec::new();
        let virtual_time = [0; 3];
        for id in 1..=10 {
            order.push(id);
            connections.insert(id, connection(if id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor }));
            let (completed, receiver) = oneshot::channel();
            receivers.push(receiver);
            pending.insert(id, PendingRequest::new(1_000, TrafficClass::Fair, 0, completed));
        }

        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = virtual_time;
        dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

        let mut granted = 0;
        for receiver in &mut receivers {
            if let Ok(lease) = receiver.try_recv() {
                granted += 1;
                let _sent_at = lease.consume();
            }
        }
        assert_eq!(granted, 8);
        assert_eq!(pending.len(), 2);
        let remainder = u128::from(MAX_BURST_BYTES - 8_000) * 1_000_000_000;
        assert_eq!(bucket.lock().unwrap().credit_byte_nanos, remainder);

        for _ in 0..2 {
            advance(Duration::from_millis(125)).await;
            dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);
            let mut round_granted = 0;
            for receiver in &mut receivers {
                if let Ok(lease) = receiver.try_recv() {
                    round_granted += 1;
                    let _sent_at = lease.consume();
                }
            }
            assert_eq!(round_granted, 1);
            assert_eq!(bucket.lock().unwrap().credit_byte_nanos, remainder);
        }
        assert!(pending.is_empty());
    }

    async fn sustained_priority_service(primary_class: TrafficClass) {
        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));

        let mut connections = HashMap::new();
        let mut order = Vec::new();
        let mut pending = HashMap::new();
        let mut receivers = Vec::new();
        for id in 1..=5 {
            let role = if id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor };
            let class = if id == 1 { primary_class } else { TrafficClass::Fair };
            order.push(id);
            connections.insert(id, connection(role));
            let (completed, receiver) = oneshot::channel();
            receivers.push(receiver);
            pending.insert(id, PendingRequest::new(1_000, class, 0, completed));
        }

        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];
        let mut service = Vec::new();
        let mut neighbor_grants = [0; 4];

        for _ in 0..12 {
            advance(Duration::from_millis(125)).await;
            dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

            let mut granted_ids = Vec::new();
            for (index, receiver) in receivers.iter_mut().enumerate() {
                if let Ok(lease) = receiver.try_recv() {
                    lease.consume();
                    let id = index + 1;
                    service.push(id);
                    if id > 1 {
                        neighbor_grants[index - 1] += 1;
                    }
                    granted_ids.push(id);
                }
            }
            assert_eq!(granted_ids.len(), 1, "each refill grants one 1 KB datagram");

            for id in granted_ids {
                let (completed, receiver) = oneshot::channel();
                receivers[id - 1] = receiver;
                let class = if id == 1 { primary_class } else { TrafficClass::Fair };
                pending.insert(id as u64, PendingRequest::new(1_000, class, 0, completed));
            }
        }

        match primary_class {
            TrafficClass::Control => assert_eq!(&service[..4], &[1, 1, 2, 3], "Control leads, then Fair reaches neighbors"),
            TrafficClass::Deadline => assert_eq!(&service[..3], &[1, 2, 3], "Deadline work precedes Fair service without starving it"),
            TrafficClass::Fair => unreachable!("test covers only primary priority classes"),
        }
        assert!(neighbor_grants.iter().all(|grants| *grants > 0), "every queued neighbor receives service: {neighbor_grants:?}");
        assert_eq!(pending.len(), 5, "the sustained load remains queued after the measured rounds");
    }

    #[tokio::test(start_paused = true)]
    async fn primary_control_and_deadline_survive_neighbor_retries() {
        sustained_priority_service(TrafficClass::Control).await;
        sustained_priority_service(TrafficClass::Deadline).await;
    }

    #[test]
    fn cancelled_waiter_does_not_spend_budget() {
        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
        let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor))]);
        let order = vec![1];
        let (completed, receiver) = oneshot::channel();
        drop(receiver);
        let mut pending = HashMap::from([(1, PendingRequest::new(1_000, TrafficClass::Fair, 0, completed))]);
        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];

        dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

        assert!(pending.is_empty());
        assert_eq!(bucket.lock().unwrap().credit_byte_nanos, u128::from(MAX_BURST_BYTES) * 1_000_000_000);
    }

    #[test]
    fn cancelled_request_does_not_block_its_replacement() {
        let connections = HashMap::from([(1, connection(ConnectionRole::Neighbor))]);
        let mut pending = HashMap::new();
        let (old_completed, old_receiver) = oneshot::channel();
        pending.insert(1, PendingRequest::new(1_000, TrafficClass::Fair, 0, old_completed));
        drop(old_receiver);

        let (completed, _receiver) = oneshot::channel();
        queue_request(1, 500, TrafficClass::Fair, completed, &connections, &mut pending, &[0; 3]);

        assert_eq!(pending.get(&1).expect("replacement request").bytes, 500);
        assert!(!pending.get(&1).expect("replacement request").completed.is_closed());
    }

    #[test]
    fn cancelled_send_refunds_its_budget_lease() {
        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
        assert!(bucket.lock().unwrap().reserve(4_000, now));

        drop(super::BudgetLease::new(Arc::clone(&bucket), 4_000));

        assert_eq!(bucket.lock().unwrap().credit_byte_nanos, u128::from(MAX_BURST_BYTES) * 1_000_000_000);
    }

    #[tokio::test(start_paused = true)]
    async fn blocked_udp_send_refunds_credit_before_waiting_for_writability() {
        let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator socket");
        let address = server.local_addr().expect("simulator endpoint");
        let mut blocked_socket = UdpTransport::connect(address).await.expect("connect blocked transport");
        let (gate, gate_state, send_entered) = UdpTransport::new_send_gate();
        gate_state.send_replace(true);
        blocked_socket.set_send_gate(gate);
        let flowing_socket = UdpTransport::connect(address).await.expect("connect flowing transport");

        let budget = OutboundBudget::new(8_000);
        let blocked_connection = budget.register(1, true).await.expect("register blocked connection");
        let flowing_connection = budget.register(2, false).await.expect("register flowing connection");
        let blocked_pacer = blocked_connection.handle();
        let flowing_pacer = flowing_connection.handle();
        let blocked = spawn(async move {
            let bytes = [0; MAX_BURST_BYTES as usize];
            blocked_pacer.send_with_kind(&blocked_socket, &bytes, TrafficClass::Fair, false).await
        });
        timeout(Duration::from_secs(1), send_entered.notified()).await.expect("blocked socket should report backpressure");

        let flowing = spawn(async move {
            let bytes = [1; 100];
            flowing_pacer.send_with_kind(&flowing_socket, &bytes, TrafficClass::Fair, false).await
        });
        timeout(Duration::from_millis(1), flowing)
            .await
            .expect("unblocked connection should use refunded pacing credit")
            .expect("flowing send task")
            .expect("flowing datagram");

        let mut received = [0; MAX_BURST_BYTES as usize];
        let (received_bytes, _) = timeout(Duration::from_secs(1), server.recv_from(&mut received))
            .await
            .expect("receive unblocked simulator datagram")
            .expect("read unblocked simulator datagram");
        assert_eq!(&received[..received_bytes], &[1; 100]);

        advance(Duration::from_millis(13)).await;
        gate_state.send_replace(false);
        timeout(Duration::from_secs(1), blocked)
            .await
            .expect("blocked send should resume")
            .expect("blocked send task")
            .expect("blocked datagram");
        let (received_bytes, _) = timeout(Duration::from_secs(1), server.recv_from(&mut received))
            .await
            .expect("receive resumed simulator datagram")
            .expect("read resumed simulator datagram");
        assert_eq!(received_bytes, MAX_BURST_BYTES as usize);

        drop(blocked_connection);
        drop(flowing_connection);
        budget.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn retransmission_feedback_reduces_the_session_rate() {
        let server = UdpSocket::bind("127.0.0.1:0").await.expect("bind simulator socket");
        let socket = UdpTransport::connect(server.local_addr().expect("simulator endpoint")).await.expect("connect transport");
        let budget = OutboundBudget::new(20_000);
        let connection = budget.register(1, true).await.expect("register simulator");

        assert_eq!(budget.bucket.lock().expect("session pacing bucket").rate_bytes_per_second, 20_000);
        let handle = connection.handle();
        handle.report_loss();
        handle.send_retransmission(&socket, &[0; 100], TrafficClass::Fair).await.expect("send retry");
        assert_eq!(budget.bucket.lock().expect("session pacing bucket").rate_bytes_per_second, 10_000);

        drop(connection);
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn outstanding_leases_reserve_burst_capacity_until_send() {
        let budget = OutboundBudget::new(8_000);
        let first = budget.register(1, true).await.expect("register first simulator");
        let second = budget.register(2, false).await.expect("register second simulator");
        let first_handle = first.handle();
        let second_handle = second.handle();

        let delayed_request = queue_worker_request(&first_handle, MAX_BURST_BYTES as usize).await;
        let delayed_lease = delayed_request.await.expect("grant the initial burst");
        let mut next_request = queue_worker_request(&second_handle, 1).await;
        task::yield_now().await;

        advance(Duration::from_secs(1)).await;
        for _ in 0..8 {
            task::yield_now().await;
        }
        assert!(
            matches!(next_request.try_recv(), Err(oneshot::error::TryRecvError::Empty)),
            "refill must count the delayed lease against the burst ceiling"
        );

        delayed_lease.consume();
        advance(Duration::from_millis(1)).await;
        for _ in 0..8 {
            task::yield_now().await;
        }
        next_request.await.expect("the next request gets newly earned credit").consume();

        drop((first, second));
        budget.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn worker_wakes_for_rate_recovery_before_retry_deadline() {
        const MAX_RATE_BYTES_PER_SECOND: u64 = 1_000;

        let budget = OutboundBudget::new(MAX_RATE_BYTES_PER_SECOND);
        let first = budget.register(1, true).await.expect("register loss reporter");
        let second = budget.register(2, false).await.expect("register waiting simulator");
        let first_handle = first.handle();
        let second_handle = second.handle();

        queue_worker_request(&first_handle, MAX_BURST_BYTES as usize).await.await.expect("grant the initial burst").consume();
        first_handle.report_loss();
        for expected_rate in [250, 125, 62] {
            advance(Duration::from_secs(1)).await;
            first_handle.report_loss();
            assert_eq!(budget.bucket.lock().expect("session pacing bucket").rate_bytes_per_second, expected_rate);
        }

        queue_worker_request(&first_handle, 875).await.await.expect("drain credit earned during loss backoff").consume();
        let queued_at = Instant::now();
        let mut pending = queue_worker_request(&second_handle, 500).await;
        let mut granted = None;
        for _ in 0..4 {
            advance(Duration::from_secs(1)).await;
            for _ in 0..8 {
                task::yield_now().await;
            }
            if let Ok(lease) = pending.try_recv() {
                granted = Some(lease);
                break;
            }
        }

        let sent_at = granted.expect("recovery timers wake the worker before the five-second retry deadline").consume();
        assert!(sent_at.saturating_duration_since(queued_at) < Duration::from_secs(5));

        drop((first, second));
        budget.shutdown().await;
    }

    #[test]
    fn large_request_does_not_block_smaller_same_class_work() {
        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(8_000, now)));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize - 1_000, now));
        let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
        let order = vec![1, 2];
        let (large_completed, mut large_receiver) = oneshot::channel();
        let (small_completed, mut small_receiver) = oneshot::channel();
        let mut pending = HashMap::from([
            (1, PendingRequest::new(MAX_BURST_BYTES as usize, TrafficClass::Fair, 0, large_completed)),
            (2, PendingRequest::new(500, TrafficClass::Fair, 0, small_completed)),
        ]);
        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];

        dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

        small_receiver.try_recv().expect("smaller same-class send should use available credit").consume();
        assert!(large_receiver.try_recv().is_err());
        assert!(pending.contains_key(&1));
    }

    #[tokio::test(start_paused = true)]
    async fn large_request_reserves_its_service_turn_against_continuous_small_packets() {
        const RATE_BYTES_PER_SECOND: u64 = 8_000;
        const SMALL_PACKET_BYTES: usize = 500;
        const REFILL: Duration = Duration::from_nanos(62_500_000);

        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
        let order = vec![1, 2];
        let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
        let (large_completed, mut large_receiver) = oneshot::channel();
        let (small_completed, mut small_receiver) = oneshot::channel();
        let mut pending = HashMap::new();
        queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, large_completed, &connections, &mut pending, &[0; 3]);
        queue_request(2, SMALL_PACKET_BYTES, TrafficClass::Fair, small_completed, &connections, &mut pending, &[0; 3]);

        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];
        let mut total_granted_bytes = 0u64;
        let mut small_grants = 0usize;
        let mut large_grant_tick = None;

        for tick in 1..=33 {
            advance(REFILL).await;
            let granted = dispatch_with_probe(
                &bucket,
                &mut connections,
                &order,
                &mut pending,
                &mut class_cursor,
                &mut connection_cursor,
                &mut virtual_time,
                None,
            );

            let small_sent = consume_ready(&mut small_receiver);
            let large_sent = consume_ready(&mut large_receiver);
            assert_eq!(granted, small_sent || large_sent, "the arbiter reports each completed lease");
            assert!(!(small_sent && large_sent), "one dispatch grants at most one request");
            if small_sent {
                small_grants += 1;
                total_granted_bytes += SMALL_PACKET_BYTES as u64;
                let (completed, receiver) = oneshot::channel();
                small_receiver = receiver;
                queue_request(2, SMALL_PACKET_BYTES, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
            }
            if large_sent {
                total_granted_bytes += MAX_BURST_BYTES;
                large_grant_tick = Some(tick);
            }

            let elapsed = REFILL * tick;
            let bucket = bucket.lock().unwrap();
            let earned_byte_nanos = elapsed.as_nanos() * u128::from(RATE_BYTES_PER_SECOND);
            let spent_byte_nanos = u128::from(total_granted_bytes) * NANOS_PER_SECOND;
            assert!(bucket.credit_byte_nanos <= u128::from(bucket.capacity_bytes) * NANOS_PER_SECOND);
            assert!(spent_byte_nanos + bucket.credit_byte_nanos <= earned_byte_nanos, "grants must remain within the shared token budget");
        }

        assert_eq!(small_grants, 16, "smaller packets use their fair virtual service before the large request becomes next");
        assert_eq!(large_grant_tick, Some(33), "the reserved large packet is served after finite token accrual");
        assert_eq!(total_granted_bytes, 16 * SMALL_PACKET_BYTES as u64 + MAX_BURST_BYTES);
    }

    #[tokio::test(start_paused = true)]
    async fn control_uses_its_scheduled_turn_while_large_fair_request_waits() {
        const RATE_BYTES_PER_SECOND: u64 = 8_000;
        const CONTROL_PACKET_BYTES: usize = 500;
        const REFILL: Duration = Duration::from_nanos(62_500_000);

        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
        let order = vec![1, 2];
        let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
        let (large_completed, mut large_receiver) = oneshot::channel();
        let mut pending = HashMap::new();
        queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, large_completed, &connections, &mut pending, &[0; 3]);
        let (competitor_completed, competitor_receiver) = oneshot::channel();
        pending.insert(
            2,
            PendingRequest::new(
                MAX_BURST_BYTES as usize / 2,
                TrafficClass::Fair,
                u128::from(MAX_BURST_BYTES) * 2 + 1,
                competitor_completed,
            ),
        );

        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];
        assert!(!dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        assert!(pending.get(&1).expect("large Fair request remains pending").reserved);
        drop(competitor_receiver);

        let (control_completed, mut control_receiver) = oneshot::channel();
        queue_request(2, CONTROL_PACKET_BYTES, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);

        let mut total_granted_bytes = 0u64;
        let mut control_grant_tick = None;
        let mut fair_grant_tick = None;
        for tick in 1..=18 {
            advance(REFILL).await;
            let granted = dispatch_with_probe(
                &bucket,
                &mut connections,
                &order,
                &mut pending,
                &mut class_cursor,
                &mut connection_cursor,
                &mut virtual_time,
                None,
            );
            let control_sent = consume_ready(&mut control_receiver);
            let fair_sent = consume_ready(&mut large_receiver);
            assert_eq!(granted, control_sent || fair_sent);
            assert!(!(control_sent && fair_sent));
            if control_sent {
                total_granted_bytes += CONTROL_PACKET_BYTES as u64;
                control_grant_tick = Some(tick);
            }
            if fair_sent {
                total_granted_bytes += MAX_BURST_BYTES;
                fair_grant_tick = Some(tick);
            }

            let elapsed = REFILL * tick;
            let bucket = bucket.lock().unwrap();
            let earned_byte_nanos = elapsed.as_nanos() * u128::from(RATE_BYTES_PER_SECOND);
            let spent_byte_nanos = u128::from(total_granted_bytes) * NANOS_PER_SECOND;
            assert!(bucket.credit_byte_nanos <= u128::from(bucket.capacity_bytes) * NANOS_PER_SECOND);
            assert!(spent_byte_nanos + bucket.credit_byte_nanos <= earned_byte_nanos, "grants must remain within the shared token budget");
        }

        assert_eq!(control_grant_tick, Some(1), "Control uses its next scheduled opportunity");
        assert_eq!(fair_grant_tick, Some(18), "bounded Control service does not starve the reserved Fair request");
        assert_eq!(total_granted_bytes, CONTROL_PACKET_BYTES as u64 + MAX_BURST_BYTES);
    }

    #[tokio::test(start_paused = true)]
    async fn affordable_deadline_uses_its_turn_while_reserved_fair_waits() {
        const RATE_BYTES_PER_SECOND: u64 = 8_000;
        const SMALL_PACKET_BYTES: usize = 500;
        const REFILL: Duration = Duration::from_nanos(62_500_000);

        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
        let order = vec![1, 2, 3];
        let mut connections = HashMap::from([
            (1, connection(ConnectionRole::Neighbor)),
            (2, connection(ConnectionRole::Neighbor)),
            (3, connection(ConnectionRole::Neighbor)),
        ]);
        let (fair_completed, mut fair_receiver) = oneshot::channel();
        let mut pending = HashMap::new();
        queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, fair_completed, &connections, &mut pending, &[0; 3]);
        let (competitor_completed, competitor_receiver) = oneshot::channel();
        pending.insert(
            2,
            PendingRequest::new(
                MAX_BURST_BYTES as usize / 2,
                TrafficClass::Fair,
                u128::from(MAX_BURST_BYTES) * 2 + 1,
                competitor_completed,
            ),
        );

        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];
        assert!(!dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        drop(competitor_receiver);

        let (control_completed, mut control_receiver) = oneshot::channel();
        queue_request(2, MAX_BURST_BYTES as usize, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
        let (deadline_completed, mut deadline_receiver) = oneshot::channel();
        queue_request(3, SMALL_PACKET_BYTES, TrafficClass::Deadline, deadline_completed, &connections, &mut pending, &virtual_time);

        advance(REFILL).await;
        assert!(dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        assert!(control_receiver.try_recv().is_err(), "an unaffordable Control request should not block Deadline");
        deadline_receiver.try_recv().expect("affordable Deadline request uses its scheduled bypass").consume();
        assert!(fair_receiver.try_recv().is_err(), "the Fair request remains reserved until its full packet is funded");

        drop(control_receiver);
        let mut fair_grant_tick = None;
        for tick in 2..=18 {
            advance(REFILL).await;
            let granted = dispatch_with_probe(
                &bucket,
                &mut connections,
                &order,
                &mut pending,
                &mut class_cursor,
                &mut connection_cursor,
                &mut virtual_time,
                None,
            );
            let fair_sent = consume_ready(&mut fair_receiver);
            assert_eq!(granted, fair_sent);
            if fair_sent {
                fair_grant_tick = Some(tick);
            }
        }
        assert_eq!(fair_grant_tick, Some(18), "the unaffordable Control request cannot leave the Fair reservation stuck");
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_reserved_request_releases_its_service_slot() {
        const RATE_BYTES_PER_SECOND: u64 = 8_000;
        const REFILL: Duration = Duration::from_nanos(62_500_000);

        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
        let order = vec![1, 2];
        let mut connections = HashMap::from([(1, connection(ConnectionRole::Neighbor)), (2, connection(ConnectionRole::Neighbor))]);
        let (cancelled_completed, cancelled_receiver) = oneshot::channel();
        let mut pending = HashMap::new();
        queue_request(1, MAX_BURST_BYTES as usize, TrafficClass::Fair, cancelled_completed, &connections, &mut pending, &[0; 3]);
        let (competitor_completed, competitor_receiver) = oneshot::channel();
        pending.insert(
            2,
            PendingRequest::new(
                MAX_BURST_BYTES as usize / 2,
                TrafficClass::Fair,
                u128::from(MAX_BURST_BYTES) * 2 + 1,
                competitor_completed,
            ),
        );

        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];
        assert!(!dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        assert!(pending.get(&1).expect("large request is reserved").reserved);
        drop(cancelled_receiver);
        drop(competitor_receiver);

        let (control_completed, mut control_receiver) = oneshot::channel();
        queue_request(2, 500, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
        advance(REFILL).await;
        assert!(dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        control_receiver.try_recv().expect("cancellation removes the pinned reservation").consume();
        assert!(!pending.contains_key(&1));
    }

    #[tokio::test(start_paused = true)]
    async fn reserved_fair_send_consumes_its_service_slot_before_next_control() {
        const RATE_BYTES_PER_SECOND: u64 = 8_000;
        const SMALL_PACKET_BYTES: usize = 500;
        const REFILL: Duration = Duration::from_millis(1_024);

        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, now));
        let order = vec![1, 2, 3, 4];
        let mut connections = HashMap::from([
            (1, connection(ConnectionRole::Neighbor)),
            (2, connection(ConnectionRole::Neighbor)),
            (3, connection(ConnectionRole::Neighbor)),
            (4, connection(ConnectionRole::Neighbor)),
        ]);
        let (reserved_completed, mut reserved_receiver) = oneshot::channel();
        let (first_fair_completed, mut first_fair_receiver) = oneshot::channel();
        let mut pending = HashMap::from([
            (1, PendingRequest::new(MAX_BURST_BYTES as usize, TrafficClass::Fair, 0, reserved_completed)),
            (2, PendingRequest::new(SMALL_PACKET_BYTES, TrafficClass::Fair, 1, first_fair_completed)),
        ]);
        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];

        assert!(!dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        assert!(pending.get(&1).expect("large Fair request is reserved").reserved);
        assert_eq!(class_cursor, 3, "the reservation pins the selected Fair slot");

        advance(REFILL).await;
        assert!(dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        reserved_receiver.try_recv().expect("reserved Fair request is funded").consume();
        assert_eq!(class_cursor, 4, "the reserved send consumes Fair slot 3 and advances to slot 4");

        let (second_fair_completed, mut second_fair_receiver) = oneshot::channel();
        queue_request(3, SMALL_PACKET_BYTES, TrafficClass::Fair, second_fair_completed, &connections, &mut pending, &virtual_time);
        let (control_completed, mut control_receiver) = oneshot::channel();
        queue_request(4, SMALL_PACKET_BYTES, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
        advance(Duration::from_millis(250)).await;

        let mut granted = Vec::new();
        for _ in 0..3 {
            assert!(dispatch_with_probe(
                &bucket,
                &mut connections,
                &order,
                &mut pending,
                &mut class_cursor,
                &mut connection_cursor,
                &mut virtual_time,
                None,
            ));
            if let Ok(lease) = first_fair_receiver.try_recv() {
                lease.consume();
                granted.push("fair");
            } else if let Ok(lease) = control_receiver.try_recv() {
                lease.consume();
                granted.push("control");
            } else if let Ok(lease) = second_fair_receiver.try_recv() {
                lease.consume();
                granted.push("fair");
            } else {
                panic!("each successful dispatch grants exactly one queued request");
            }
        }

        assert_eq!(granted, ["fair", "control", "fair"], "slot 4 Fair may precede the next Control slot only once");
    }

    #[tokio::test(start_paused = true)]
    async fn reserved_wait_ignores_ineligible_small_requests_but_dispatches_new_control() {
        const RATE_BYTES_PER_SECOND: u64 = 8_000;
        const LARGE_PACKET_BYTES: usize = MAX_BURST_BYTES as usize;
        const SMALL_PACKET_BYTES: usize = 500;

        let now = Instant::now();
        let bucket = Arc::new(Mutex::new(Bucket::new(RATE_BYTES_PER_SECOND, now)));
        assert!(bucket.lock().unwrap().take(LARGE_PACKET_BYTES, now));
        let order = vec![1, 2, 3];
        let mut connections = HashMap::from([
            (1, connection(ConnectionRole::Neighbor)),
            (2, connection(ConnectionRole::Neighbor)),
            (3, connection(ConnectionRole::Neighbor)),
        ]);
        let (large_completed, mut large_receiver) = oneshot::channel();
        let (small_completed, mut small_receiver) = oneshot::channel();
        let mut pending = HashMap::from([
            (1, PendingRequest::new(LARGE_PACKET_BYTES, TrafficClass::Fair, 0, large_completed)),
            (2, PendingRequest::new(SMALL_PACKET_BYTES, TrafficClass::Fair, 1, small_completed)),
        ]);
        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];

        assert!(!dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        assert!(pending.get(&1).expect("large request remains pending").reserved);

        advance(Duration::from_millis(100)).await;
        let delay = next_dispatch_delay(&bucket, &pending);
        assert_eq!(delay, Duration::from_millis(924), "wait for the reserved 8 KiB request, not affordable 500 B Fair work");

        let (control_completed, mut control_receiver) = oneshot::channel();
        queue_request(3, SMALL_PACKET_BYTES, TrafficClass::Control, control_completed, &connections, &mut pending, &virtual_time);
        assert!(dispatch_with_probe(
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
            None,
        ));
        control_receiver.try_recv().expect("new eligible Control work uses accumulated credit immediately").consume();
        assert!(large_receiver.try_recv().is_err(), "Control uses only the reserved Fair turn's bounded bypass");
        assert!(small_receiver.try_recv().is_err(), "same-class small work cannot cause another short timer wake");
        assert!(pending.get(&1).expect("large request remains reserved").reserved);
        assert_eq!(next_dispatch_delay(&bucket, &pending), Duration::from_nanos(986_500_000));
    }

    async fn dispatch_one_under_pressure(
        active: &[u64],
        bucket: &Arc<Mutex<Bucket>>,
        connections: &mut HashMap<u64, super::ConnectionState>,
        order: &[u64],
        pending: &mut HashMap<u64, PendingRequest>,
        receivers: &mut HashMap<u64, oneshot::Receiver<super::BudgetLease>>,
        class_cursor: &mut usize,
        connection_cursor: &mut [usize; 3],
        virtual_time: &mut [u128; 3],
    ) -> u64 {
        advance(Duration::from_millis(200)).await;
        dispatch(bucket, connections, order, pending, class_cursor, connection_cursor, virtual_time);

        let granted = active
            .iter()
            .filter_map(|id| match receivers.get_mut(id).expect("active request receiver").try_recv() {
                Ok(lease) => {
                    lease.consume();
                    Some(*id)
                }
                Err(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(granted.len(), 1, "one packet should use each refill of the shared bucket");
        let id = granted[0];
        assert_eq!(bucket.lock().unwrap().credit_byte_nanos, 0);

        let (completed, receiver) = oneshot::channel();
        receivers.insert(id, receiver);
        queue_request(id, 1_000, TrafficClass::Fair, completed, connections, pending, virtual_time);
        id
    }

    async fn assert_service_window(
        active: &[u64],
        bucket: &Arc<Mutex<Bucket>>,
        connections: &mut HashMap<u64, super::ConnectionState>,
        order: &[u64],
        pending: &mut HashMap<u64, PendingRequest>,
        receivers: &mut HashMap<u64, oneshot::Receiver<super::BudgetLease>>,
        class_cursor: &mut usize,
        connection_cursor: &mut [usize; 3],
        virtual_time: &mut [u128; 3],
    ) {
        let mut served = HashSet::new();
        for _ in 0..active.len() * 2 {
            let id = dispatch_one_under_pressure(
                active,
                bucket,
                connections,
                order,
                pending,
                receivers,
                class_cursor,
                connection_cursor,
                virtual_time,
            )
            .await;
            served.insert(id);
            if served.len() == active.len() {
                break;
            }
        }
        assert_eq!(served, active.iter().copied().collect(), "every active connection should receive service within a bounded window");
    }

    #[tokio::test(start_paused = true)]
    async fn small_packets_follow_role_weights() {
        let bucket = Arc::new(Mutex::new(Bucket::new(5_000, Instant::now())));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, Instant::now()));
        let order = (1..=5).collect::<Vec<_>>();
        let mut connections = HashMap::new();
        let mut pending = HashMap::new();
        let mut receivers = HashMap::new();
        let mut virtual_time = [0; 3];
        for id in &order {
            connections.insert(*id, connection(if *id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor }));
            let (completed, receiver) = oneshot::channel();
            receivers.insert(*id, receiver);
            queue_request(*id, 500, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
        }

        let mut counts = HashMap::<u64, usize>::new();
        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        for _ in 0..90 {
            advance(Duration::from_millis(100)).await;
            dispatch(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

            let granted = order
                .iter()
                .filter_map(|id| match receivers.get_mut(id).expect("active connection receiver").try_recv() {
                    Ok(lease) => {
                        lease.consume();
                        *counts.entry(*id).or_default() += 1;
                        Some(*id)
                    }
                    Err(_) => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(granted.len(), 1, "one 500-byte packet should use each refill");

            for id in granted {
                let (completed, receiver) = oneshot::channel();
                receivers.insert(id, receiver);
                queue_request(id, 500, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
            }
        }

        assert_eq!(counts.get(&1), Some(&30), "the primary should receive twice each neighbor's bytes: {counts:?}");
        for id in 2..=5 {
            assert_eq!(counts.get(&id), Some(&15), "neighbor {id} should receive its weighted share");
        }
    }

    #[tokio::test(start_paused = true)]
    async fn shared_pressure_services_at_configured_limit() {
        let simulator_count = MAX_NEIGHBOR_SIMULATORS + 1;
        let bucket = Arc::new(Mutex::new(Bucket::new(5_000, Instant::now())));
        assert!(bucket.lock().unwrap().take(MAX_BURST_BYTES as usize, Instant::now()));

        let mut order = (1..=simulator_count as u64).collect::<Vec<_>>();
        let mut connections = HashMap::new();
        let mut pending = HashMap::new();
        let mut receivers = HashMap::new();
        for id in &order {
            connections.insert(*id, connection(if *id == 1 { ConnectionRole::Primary } else { ConnectionRole::Neighbor }));
            let (completed, receiver) = oneshot::channel();
            receivers.insert(*id, receiver);
            queue_request(*id, 1_000, TrafficClass::Fair, completed, &connections, &mut pending, &[0; 3]);
        }

        let mut class_cursor = 0;
        let mut connection_cursor = [0; 3];
        let mut virtual_time = [0; 3];
        for _ in 0..2 {
            assert_service_window(
                &order,
                &bucket,
                &mut connections,
                &order,
                &mut pending,
                &mut receivers,
                &mut class_cursor,
                &mut connection_cursor,
                &mut virtual_time,
            )
            .await;
        }

        connections.get_mut(&1).expect("original primary").role = ConnectionRole::Neighbor;
        connections.get_mut(&1).expect("original primary").finish = virtual_time;
        connections.get_mut(&3).expect("promoted neighbor").role = ConnectionRole::Primary;
        connections.get_mut(&3).expect("promoted neighbor").finish = virtual_time;
        assert_service_window(
            &order,
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut receivers,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
        )
        .await;

        let leaving_id = simulator_count as u64;
        connections.remove(&leaving_id);
        pending.remove(&leaving_id);
        receivers.remove(&leaving_id);
        order.retain(|id| *id != leaving_id);
        connection_cursor = [0; 3];
        let joining_id = leaving_id + 1;
        order.push(joining_id);
        connections.insert(
            joining_id,
            super::ConnectionState { role: ConnectionRole::Neighbor, finish: virtual_time, active: Arc::new(AtomicBool::new(true)) },
        );
        let (completed, receiver) = oneshot::channel();
        receivers.insert(joining_id, receiver);
        queue_request(joining_id, 1_000, TrafficClass::Fair, completed, &connections, &mut pending, &virtual_time);
        assert_service_window(
            &order,
            &bucket,
            &mut connections,
            &order,
            &mut pending,
            &mut receivers,
            &mut class_cursor,
            &mut connection_cursor,
            &mut virtual_time,
        )
        .await;
    }
}
