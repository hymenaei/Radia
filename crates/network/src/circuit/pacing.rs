use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt,
    io::{self, ErrorKind},
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

#[derive(Clone, Debug)]
pub(crate) enum PacerError {
    Stopped,
    GrantCanceled,
    Writable(Arc<io::Error>),
    Send(Arc<io::Error>),
}

impl fmt::Display for PacerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stopped => formatter.write_str("The LLUDP byte pacer has stopped."),
            Self::GrantCanceled => formatter.write_str("The LLUDP byte pacer stopped before granting a send."),
            Self::Writable(error) => write!(formatter, "Could not wait for a simulator datagram send slot: {error}"),
            Self::Send(error) => write!(formatter, "Could not send a simulator datagram: {error}"),
        }
    }
}

impl StdError for PacerError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Writable(error) | Self::Send(error) => Some(error.as_ref()),
            Self::Stopped | Self::GrantCanceled => None,
        }
    }
}

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
}

impl PendingRequest {
    fn new(bytes: usize, class: TrafficClass, finish: u128, completed: oneshot::Sender<BudgetLease>) -> Self {
        Self { bytes, class, finish, completed, reserved: false, bypass_slots: [0; 3] }
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

pub(crate) struct OutboundBudget {
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
    worker: JoinHandle<()>,
}

impl OutboundBudget {
    pub(crate) fn new(rate_bytes_per_second: u64) -> Self {
        let (commands, receiver) = mpsc::channel(256);
        let bucket = Arc::new(Mutex::new(Bucket::new(rate_bytes_per_second.max(1), Instant::now())));
        let worker_bucket = Arc::clone(&bucket);
        let worker = spawn(run_arbiter(receiver, worker_bucket));
        Self { commands, bucket, worker }
    }

    pub(crate) fn handle(&self) -> OutboundBudgetHandle {
        OutboundBudgetHandle { commands: self.commands.clone(), bucket: Arc::clone(&self.bucket) }
    }

    pub(crate) async fn shutdown(mut self) {
        let (completed, response) = oneshot::channel();
        if self.commands.send(Command::Shutdown(completed)).await.is_err() {
            self.worker.abort();
            return;
        }
        if response.await.is_err() {
            self.worker.abort();
        }
        if let Err(error) = (&mut self.worker).await
            && !error.is_cancelled()
        {
            tracing::error!(%error, "LLUDP byte pacer task failed during shutdown");
        }
    }
}

impl Drop for OutboundBudget {
    fn drop(&mut self) {
        self.worker.abort();
    }
}

#[derive(Clone)]
pub(crate) struct OutboundBudgetHandle {
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
}

impl OutboundBudgetHandle {
    pub(crate) async fn register(&self, id: u64, primary: bool) -> Result<ConnectionPacer, PacerError> {
        let role = if primary { ConnectionRole::Primary } else { ConnectionRole::Neighbor };
        let active = Arc::new(AtomicBool::new(true));
        self.commands.send(Command::Register { id, role, active: Arc::clone(&active) }).await.map_err(|_| PacerError::Stopped)?;
        Ok(ConnectionPacer { id, role, commands: self.commands.clone(), bucket: Arc::clone(&self.bucket), active })
    }
}

pub(crate) struct ConnectionPacer {
    id: u64,
    role: ConnectionRole,
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
    active: Arc<AtomicBool>,
}

impl ConnectionPacer {
    pub(crate) fn handle(&self) -> PacerHandle {
        PacerHandle { id: self.id, commands: self.commands.clone(), bucket: Arc::clone(&self.bucket) }
    }

    pub(crate) async fn set_primary(&mut self, primary: bool) -> Result<(), PacerError> {
        let role = if primary { ConnectionRole::Primary } else { ConnectionRole::Neighbor };
        if self.role != role {
            self.commands.send(Command::SetRole { id: self.id, role }).await.map_err(|_| PacerError::Stopped)?;
            self.role = role;
        }
        Ok(())
    }
}

impl Drop for ConnectionPacer {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        match self.commands.try_send(Command::Unregister { id: self.id }) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) | Err(mpsc::error::TrySendError::Full(_)) => {}
        }
    }
}

#[derive(Clone)]
pub(crate) struct PacerHandle {
    id: u64,
    commands: mpsc::Sender<Command>,
    bucket: Arc<Mutex<Bucket>>,
}

impl PacerHandle {
    pub(crate) fn report_loss(&self) {
        if let Ok(mut bucket) = self.bucket.lock() {
            bucket.report_loss(Instant::now());
        }
    }

    pub(crate) async fn send(&self, socket: &UdpTransport, bytes: &[u8], class: TrafficClass) -> Result<Instant, PacerError> {
        loop {
            let (completed, response) = oneshot::channel();
            self.commands
                .send(Command::Request { id: self.id, bytes: bytes.len(), class, completed })
                .await
                .map_err(|_| PacerError::Stopped)?;
            let lease = response.await.map_err(|_| PacerError::GrantCanceled)?;
            match socket.try_send(bytes) {
                Ok(()) => return Ok(lease.consume()),
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    drop(lease);
                    socket.writable().await.map_err(|error| PacerError::Writable(Arc::new(error)))?;
                }
                Err(error) => return Err(PacerError::Send(Arc::new(error))),
            }
        }
    }
}

async fn run_arbiter(mut commands: mpsc::Receiver<Command>, bucket: Arc<Mutex<Bucket>>) {
    let mut connections = HashMap::<u64, ConnectionState>::new();
    let mut order = Vec::<u64>::new();
    let mut pending = HashMap::<u64, PendingRequest>::new();
    let mut class_cursor = 0usize;
    let mut connection_cursor = [0usize; 3];
    let mut virtual_time = [0u128; 3];

    loop {
        retire_inactive_connections(&mut connections, &mut order, &mut pending, &mut connection_cursor);
        let dispatched =
            dispatch_next(&bucket, &mut connections, &order, &mut pending, &mut class_cursor, &mut connection_cursor, &mut virtual_time);

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
            if completed.send(()).is_err() {
                return false;
            }
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

fn dispatch_next(
    bucket: &Arc<Mutex<Bucket>>,
    connections: &mut HashMap<u64, ConnectionState>,
    order: &[u64],
    pending: &mut HashMap<u64, PendingRequest>,
    class_cursor: &mut usize,
    connection_cursor: &mut [usize; 3],
    virtual_time: &mut [u128; 3],
) -> bool {
    pending.retain(|_, request| !request.completed.is_closed());
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
    virtual_time[class_index] = virtual_time[class_index].max(request.finish);
    let index = (connection_cursor[class_index] + offset) % order.len();
    connection_cursor[class_index] = (index + 1) % order.len();
    match request.completed.send(BudgetLease::new(Arc::clone(bucket), request.bytes)) {
        Ok(()) => {}
        Err(error) => drop(error),
    }
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
pub(crate) mod test_values {
    use super::PacerHandle;

    pub(crate) fn pacer_rate(handle: &PacerHandle) -> u64 {
        handle.bucket.lock().expect("session pacing bucket").rate_bytes_per_second
    }
}

#[cfg(test)]
#[path = "tests/pacing.rs"]
mod tests;
