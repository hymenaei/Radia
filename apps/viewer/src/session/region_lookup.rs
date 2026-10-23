use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use futures::{channel::oneshot, future::select, pin_mut};
use network::{AgentSession, Delivery, Error, SessionHandle, SimulatorId};
use protocol::{
    lludp::Message,
    messages::{
        MapBlockRequest, RegionHandleRequest,
        map::map_block_reply,
        map::map_block_request::{self, PositionData},
        region::region_handle_request::RequestBlock,
    },
};
use tokio::runtime::Handle;
#[cfg(test)]
use tokio::spawn;
use tracing::Instrument;
use uuid::Uuid;
use world::RegionHandle;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);
const SEND_RETRY_LIMIT: usize = 4;
const INITIAL_SEND_RETRY_DELAY: Duration = Duration::from_millis(20);
const METERS_PER_CELL: f64 = 256.0;

type SendFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
type SendMessage = dyn Fn(RegionLookupRequest, Delivery) -> SendFuture + Send + Sync;
type TimeoutFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type MakeTimeout = dyn Fn(Duration) -> TimeoutFuture + Send + Sync;
type BackgroundTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type SpawnBackground = dyn Fn(BackgroundTask) + Send + Sync;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct RegionGridCell {
    x: u32,
    y: u32,
}

impl RegionGridCell {
    pub(crate) const METERS_PER_CELL: u32 = METERS_PER_CELL as u32;

    pub(crate) const fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }

    pub(crate) const fn x(self) -> u32 {
        self.x
    }

    pub(crate) const fn y(self) -> u32 {
        self.y
    }

    pub(crate) fn containing(x: f64, y: f64) -> Option<Self> {
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return None;
        }

        let x = (x / METERS_PER_CELL).floor();
        let y = (y / METERS_PER_CELL).floor();
        if x > f64::from(u32::MAX) || y > f64::from(u32::MAX) {
            return None;
        }
        Some(Self::new(x as u32, y as u32))
    }
}

#[derive(Clone)]
pub struct RegionLookup {
    inner: Arc<Inner>,
}

impl RegionLookup {
    pub fn new(session_handle: SessionHandle, simulator_id: SimulatorId, session: &AgentSession, executor: Handle) -> Self {
        let send_message = Arc::new(move |request, delivery| -> SendFuture {
            let session_handle = session_handle.clone();
            Box::pin(async move {
                match request {
                    RegionLookupRequest::RegionHandle(request) => session_handle.send_to(simulator_id, request, delivery).await,
                    RegionLookupRequest::MapBlock(request) => session_handle.send_to(simulator_id, request, delivery).await,
                }
            })
        });
        let timer_executor = executor.clone();
        let make_timeout = Arc::new(move |timeout| {
            Box::pin({
                let _entered = timer_executor.enter();
                tokio::time::sleep(timeout)
            }) as TimeoutFuture
        });
        let spawn_background = Arc::new(move |future: BackgroundTask| {
            drop(executor.spawn(future.in_current_span()));
        });
        Self::new_inner(session, make_timeout, send_message, spawn_background, LOOKUP_TIMEOUT)
    }
    pub async fn lookup_region_by_id(&self, region_id: Uuid) -> Result<String, String> {
        self.lookup(
            LookupKey::RegionId(region_id),
            RegionLookupRequest::RegionHandle(RegionHandleRequest { request_block: RequestBlock { region_id } }),
        )
        .await
    }
    pub(crate) async fn lookup_region_at(&self, cell: RegionGridCell) -> Result<String, String> {
        let request = self.map_block_request(cell).ok_or_else(|| "The region grid cell is outside the map request range.".to_string())?;
        self.lookup(LookupKey::MapCell(cell), request).await
    }
    pub fn handle_message(&self, source: SocketAddr, message: &Message) {
        if source != self.inner.simulator_address {
            return;
        }

        match message {
            Message::RegionIdAndHandleReply(message) => {
                self.handle_region_id_reply(message.reply_block.region_id, message.reply_block.region_handle);
            }
            Message::MapBlockReply(message) if message.agent_data.agent_id == self.inner.agent_id => {
                self.handle_map_block_reply(&message.data);
            }
            _ => {}
        }
    }

    async fn lookup(&self, key: LookupKey, request: RegionLookupRequest) -> Result<String, String> {
        let (sender, receiver) = oneshot::channel();
        let (waiter_id, generation, should_send) = {
            let mut state = self.lock_state();
            let waiter_id = state.allocate_waiter_id();
            let Some(pending) = state.pending_mut(key) else {
                return Err("The region lookup generation space is exhausted.".into());
            };
            let should_send = pending.waiters.is_empty();
            let generation = pending.generation;
            pending.waiters.insert(waiter_id, sender);
            (waiter_id, generation, should_send)
        };

        let timeout = self.inner.timeout;
        let timeout_factory = self.inner.make_timeout.clone();
        let send_request = should_send.then_some(request);
        let guard = WaiterGuard { lookup: self.clone(), waiter_id };
        let result = async move { receiver.await.map_err(|_| "The region lookup was canceled.".to_string())? };
        let dispatch = self.clone();
        if let Some(request) = send_request {
            dispatch.dispatch(key, generation, request);
        }

        pin_mut!(result);
        let timer = timeout_factory(timeout);
        pin_mut!(timer);
        match select(result, timer).await {
            futures::future::Either::Left((result, _)) => {
                drop(guard);
                result
            }
            futures::future::Either::Right(((), _)) => {
                drop(guard);
                Err("Timed out looking up the region name.".into())
            }
        }
    }

    fn handle_region_id_reply(&self, region_id: Uuid, region_handle: u64) {
        let origin = RegionHandle::new(region_handle).origin();
        let Some(cell) = RegionGridCell::containing(origin.x, origin.y) else {
            self.complete(LookupKey::RegionId(region_id), Err("Second Life returned an invalid region handle.".into()));
            return;
        };
        let Some(request) = self.map_block_request(cell) else {
            self.complete(LookupKey::RegionId(region_id), Err("Second Life returned an invalid region handle.".into()));
            return;
        };

        let dispatch_generation = {
            let mut state = self.lock_state();
            let Some(mut pending) = state.by_region_id.remove(&region_id) else {
                return;
            };
            if let Some(existing) = state.by_map_cell.get_mut(&cell) {
                existing.waiters.extend(pending.waiters.drain());
                None
            } else {
                let generation = pending.generation;
                state.by_map_cell.insert(cell, pending);
                Some(generation)
            }
        };

        if let Some(generation) = dispatch_generation {
            self.dispatch(LookupKey::MapCell(cell), generation, request);
        }
    }

    fn handle_map_block_reply(&self, blocks: &[map_block_reply::Data]) {
        let completed = {
            let mut state = self.lock_state();
            blocks
                .iter()
                .filter_map(|block| {
                    state.by_map_cell.remove(&RegionGridCell::new(u32::from(block.x), u32::from(block.y))).map(|pending| {
                        let name = block.name.clone();
                        (pending, name)
                    })
                })
                .collect::<Vec<_>>()
        };

        for (pending, name) in completed {
            finish(pending, Ok(name));
        }
    }

    fn map_block_request(&self, cell: RegionGridCell) -> Option<RegionLookupRequest> {
        let (x, y) = map_request_coordinates(cell)?;
        Some(RegionLookupRequest::MapBlock(MapBlockRequest {
            agent_data: map_block_request::AgentData {
                agent_id: self.inner.agent_id,
                session_id: self.inner.session_id,
                flags: 0,
                estate_id: 0,
                godlike: false,
            },
            position_data: PositionData { min_x: x, max_x: x, min_y: y, max_y: y },
        }))
    }

    fn complete(&self, key: LookupKey, result: Result<String, String>) {
        let pending = self.lock_state().remove_pending(key);
        if let Some(pending) = pending {
            finish(pending, result);
        }
    }

    fn dispatch(&self, key: LookupKey, generation: u64, request: RegionLookupRequest) {
        let lookup = self.clone();
        let send_message = self.inner.send_message.clone();
        let make_timeout = self.inner.make_timeout.clone();
        (self.inner.spawn_background)(Box::pin(async move {
            let active_lookup = lookup.clone();
            if let Err(error) =
                send_with_retry(send_message, make_timeout, request, move || active_lookup.is_pending(key, generation)).await
            {
                lookup.complete_if_generation(key, generation, Err(error.to_string()));
            }
        }));
    }

    fn is_pending(&self, key: LookupKey, generation: u64) -> bool {
        self.lock_state().pending(key).is_some_and(|pending| pending.generation == generation && !pending.waiters.is_empty())
    }

    fn complete_if_generation(&self, key: LookupKey, generation: u64, result: Result<String, String>) {
        let pending = {
            let mut state = self.lock_state();
            if state.pending(key).is_some_and(|pending| pending.generation == generation) { state.remove_pending(key) } else { None }
        };
        if let Some(pending) = pending {
            finish(pending, result);
        }
    }

    fn cancel_waiter(&self, waiter_id: u64) {
        let mut state = self.lock_state();
        state.by_region_id.retain(|_, pending| {
            pending.waiters.remove(&waiter_id);
            !pending.waiters.is_empty()
        });
        state.by_map_cell.retain(|_, pending| {
            pending.waiters.remove(&waiter_id);
            !pending.waiters.is_empty()
        });
    }

    fn lock_state(&self) -> MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn new_inner(
        session: &AgentSession,
        make_timeout: Arc<MakeTimeout>,
        send_message: Arc<SendMessage>,
        spawn_background: Arc<SpawnBackground>,
        timeout: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                agent_id: session.agent_id,
                session_id: session.session_id,
                simulator_address: session.simulator_address,
                make_timeout,
                send_message,
                spawn_background,
                state: Mutex::new(State::default()),
                timeout,
            }),
        }
    }

    #[cfg(test)]
    fn for_test(
        agent_id: Uuid,
        session_id: Uuid,
        simulator_address: SocketAddr,
        make_timeout: impl Fn(Duration) -> TimeoutFuture + Send + Sync + 'static,
        timeout: Duration,
        send_message: impl Fn(RegionLookupRequest, Delivery) -> SendFuture + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                agent_id,
                session_id,
                simulator_address,
                make_timeout: Arc::new(make_timeout),
                send_message: Arc::new(send_message),
                spawn_background: Arc::new(|future| {
                    drop(spawn(future));
                }),
                state: Mutex::new(State::default()),
                timeout,
            }),
        }
    }
}

struct Inner {
    agent_id: Uuid,
    session_id: Uuid,
    simulator_address: SocketAddr,
    make_timeout: Arc<MakeTimeout>,
    send_message: Arc<SendMessage>,
    spawn_background: Arc<SpawnBackground>,
    state: Mutex<State>,
    timeout: Duration,
}

#[derive(Default)]
struct State {
    next_waiter_id: u64,
    next_generation: u64,
    by_region_id: HashMap<Uuid, PendingLookup>,
    by_map_cell: HashMap<RegionGridCell, PendingLookup>,
}

impl State {
    fn allocate_waiter_id(&mut self) -> u64 {
        loop {
            let candidate = self.next_waiter_id;
            self.next_waiter_id = self.next_waiter_id.wrapping_add(1);
            let in_use =
                self.by_region_id.values().chain(self.by_map_cell.values()).any(|pending| pending.waiters.contains_key(&candidate));
            if !in_use {
                return candidate;
            }
        }
    }

    fn allocate_generation(&mut self) -> Option<u64> {
        let generation = self.next_generation.checked_add(1)?;
        self.next_generation = generation;
        Some(generation)
    }

    fn pending(&self, key: LookupKey) -> Option<&PendingLookup> {
        match key {
            LookupKey::RegionId(region_id) => self.by_region_id.get(&region_id),
            LookupKey::MapCell(cell) => self.by_map_cell.get(&cell),
        }
    }

    fn remove_pending(&mut self, key: LookupKey) -> Option<PendingLookup> {
        match key {
            LookupKey::RegionId(region_id) => self.by_region_id.remove(&region_id),
            LookupKey::MapCell(cell) => self.by_map_cell.remove(&cell),
        }
    }

    fn pending_mut(&mut self, key: LookupKey) -> Option<&mut PendingLookup> {
        let missing = self.pending(key).is_none();
        let generation = if missing { Some(self.allocate_generation()?) } else { None };
        match key {
            LookupKey::RegionId(region_id) => {
                if let Some(generation) = generation {
                    self.by_region_id.insert(region_id, PendingLookup { generation, waiters: HashMap::new() });
                }
                self.by_region_id.get_mut(&region_id)
            }
            LookupKey::MapCell(cell) => {
                if let Some(generation) = generation {
                    self.by_map_cell.insert(cell, PendingLookup { generation, waiters: HashMap::new() });
                }
                self.by_map_cell.get_mut(&cell)
            }
        }
    }
}

struct PendingLookup {
    generation: u64,
    waiters: HashMap<u64, oneshot::Sender<Result<String, String>>>,
}

#[derive(Clone, Copy)]
enum LookupKey {
    RegionId(Uuid),
    MapCell(RegionGridCell),
}

#[derive(Clone)]
enum RegionLookupRequest {
    RegionHandle(RegionHandleRequest),
    MapBlock(MapBlockRequest),
}

struct WaiterGuard {
    lookup: RegionLookup,
    waiter_id: u64,
}

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        self.lookup.cancel_waiter(self.waiter_id);
    }
}

pub(crate) fn map_request_coordinates(cell: RegionGridCell) -> Option<(u16, u16)> {
    Some((u16::try_from(cell.x()).ok()?, u16::try_from(cell.y()).ok()?))
}

fn finish(pending: PendingLookup, result: Result<String, String>) {
    for sender in pending.waiters.into_values() {
        if sender.send(result.clone()).is_err() {
            tracing::trace!("region lookup receiver closed");
        }
    }
}

async fn send_with_retry(
    send_message: Arc<SendMessage>,
    make_timeout: Arc<MakeTimeout>,
    request: RegionLookupRequest,
    mut is_pending: impl FnMut() -> bool,
) -> Result<(), Error> {
    let mut retry_delay = INITIAL_SEND_RETRY_DELAY;
    for attempt in 1..=SEND_RETRY_LIMIT {
        if !is_pending() {
            return Ok(());
        }
        match send_message(request.clone(), Delivery::Reliable).await {
            Ok(()) => return Ok(()),
            Err(error) if retryable_queue_error(&error) && attempt < SEND_RETRY_LIMIT => {
                if !is_pending() {
                    return Ok(());
                }
                make_timeout(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2);
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("the final send attempt always returns")
}

fn retryable_queue_error(error: &Error) -> bool {
    matches!(error, Error::CommandQueueFull | Error::SimulatorCommandQueueFull)
}

#[cfg(test)]
mod tests {
    use std::{
        future::{pending, ready},
        net::{IpAddr, Ipv4Addr},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
    };

    use protocol::{
        lludp::Message::MapBlockReply,
        messages::{RegionIdAndHandleReply, map::map_block_reply, region::region_id_and_handle_reply::ReplyBlock},
    };
    use tokio::{
        spawn,
        task::{JoinHandle, yield_now},
        time::timeout,
    };

    use super::*;

    const AGENT_ID: Uuid = Uuid::from_u128(1);
    const SESSION_ID: Uuid = Uuid::from_u128(2);
    const REGION_ID: Uuid = Uuid::from_u128(3);
    const SIMULATOR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 13000);
    const OTHER_SIMULATOR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 13001);
    const TEST_TIMEOUT: Duration = Duration::from_secs(1);

    fn lookup() -> (RegionLookup, Arc<Mutex<Vec<RegionLookupRequest>>>) {
        let sent = Arc::new(Mutex::new(Vec::new()));
        let capture = sent.clone();
        let lookup = RegionLookup::for_test(
            AGENT_ID,
            SESSION_ID,
            SIMULATOR,
            |_| Box::pin(pending()) as TimeoutFuture,
            TEST_TIMEOUT,
            move |message, _| {
                let capture = capture.clone();
                Box::pin(async move {
                    capture.lock().unwrap().push(message);
                    Ok(())
                }) as SendFuture
            },
        );
        (lookup, sent)
    }

    fn requests(sent: &Arc<Mutex<Vec<RegionLookupRequest>>>) -> Vec<RegionLookupRequest> {
        sent.lock().unwrap().clone()
    }

    fn map_reply(agent_id: Uuid, x: u16, y: u16, name: &str) -> Message {
        MapBlockReply(map_block_reply::MapBlockReply {
            agent_data: map_block_reply::AgentData { agent_id, flags: 0 },
            data: vec![map_block_reply::Data {
                x,
                y,
                name: name.into(),
                access: 0,
                region_flags: 0,
                water_height: 0,
                agents: 0,
                map_image_id: Uuid::nil(),
            }],
        })
    }

    async fn wait_for_requests(sent: &Arc<Mutex<Vec<RegionLookupRequest>>>, count: usize) -> Vec<RegionLookupRequest> {
        timeout(Duration::from_secs(1), async {
            loop {
                let requests = requests(sent);
                if requests.len() >= count {
                    return requests;
                }
                yield_now().await;
            }
        })
        .await
        .expect("expected network request")
    }

    async fn assert_pending(task: &JoinHandle<Result<String, String>>) {
        yield_now().await;
        assert!(!task.is_finished());
    }

    #[tokio::test]
    async fn coalesces_and_checks_replies() {
        let (lookup, sent) = lookup();
        let first_lookup = lookup.clone();
        let first = spawn(async move { first_lookup.lookup_region_at(RegionGridCell::new(12, 34)).await });
        let second_lookup = lookup.clone();
        let second = spawn(async move { second_lookup.lookup_region_at(RegionGridCell::new(12, 34)).await });

        let requests = wait_for_requests(&sent, 1).await;
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0],
            RegionLookupRequest::MapBlock(request)
                if request.position_data.min_x == 12 && request.position_data.min_y == 34
        ));

        lookup.handle_message(OTHER_SIMULATOR, &map_reply(AGENT_ID, 12, 34, "Wrong source"));
        lookup.handle_message(SIMULATOR, &map_reply(Uuid::from_u128(99), 12, 34, "Wrong agent"));
        assert_pending(&first).await;
        assert_pending(&second).await;

        lookup.handle_message(SIMULATOR, &map_reply(AGENT_ID, 12, 34, "Known Region"));
        assert_eq!(first.await.expect("first lookup task"), Ok("Known Region".into()));
        assert_eq!(second.await.expect("second lookup task"), Ok("Known Region".into()));
    }

    #[tokio::test]
    async fn id_lookup_coalesces_requests() {
        let (lookup, sent) = lookup();
        let first_lookup = lookup.clone();
        let first = spawn(async move { first_lookup.lookup_region_by_id(REGION_ID).await });
        let second_lookup = lookup.clone();
        let second = spawn(async move { second_lookup.lookup_region_by_id(REGION_ID).await });

        let requests = wait_for_requests(&sent, 1).await;
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0],
            RegionLookupRequest::RegionHandle(request) if request.request_block.region_id == REGION_ID
        ));

        let handle = ((42u64 * 256 + 200) << 32) | (73u64 * 256 + 255);
        lookup.handle_message(
            SIMULATOR,
            &Message::RegionIdAndHandleReply(RegionIdAndHandleReply {
                reply_block: ReplyBlock { region_id: REGION_ID, region_handle: handle },
            }),
        );
        let requests = wait_for_requests(&sent, 2).await;
        assert_eq!(requests.len(), 2);
        assert!(matches!(
            &requests[1],
            RegionLookupRequest::MapBlock(request)
                if request.position_data.min_x == 42
                    && request.position_data.max_x == 42
                    && request.position_data.min_y == 73
                    && request.position_data.max_y == 73
        ));

        lookup.handle_message(SIMULATOR, &map_reply(AGENT_ID, 42, 73, "Resolved Region"));
        assert_eq!(first.await.expect("first lookup task"), Ok("Resolved Region".into()));
        assert_eq!(second.await.expect("second lookup task"), Ok("Resolved Region".into()));
    }

    #[tokio::test]
    async fn queue_full_retries_region_id_request_before_timing_out() {
        let lookup_slot = Arc::new(Mutex::new(None::<RegionLookup>));
        let send_attempts = Arc::new(AtomicUsize::new(0));
        let slot = lookup_slot.clone();
        let attempts = send_attempts.clone();
        let lookup = RegionLookup::for_test(
            AGENT_ID,
            SESSION_ID,
            SIMULATOR,
            |duration| {
                if duration == TEST_TIMEOUT { Box::pin(pending()) as TimeoutFuture } else { Box::pin(ready(())) as TimeoutFuture }
            },
            TEST_TIMEOUT,
            move |request, _| {
                if attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                    return Box::pin(async { Err(Error::SimulatorCommandQueueFull) }) as SendFuture;
                }
                let lookup = slot.lock().unwrap().as_ref().cloned();
                Box::pin(async move {
                    if let Some(lookup) = lookup {
                        match request {
                            RegionLookupRequest::RegionHandle(_) => {
                                let region_handle = (u64::from(12 * RegionGridCell::METERS_PER_CELL) << 32)
                                    | u64::from(34 * RegionGridCell::METERS_PER_CELL);
                                lookup.handle_message(
                                    SIMULATOR,
                                    &Message::RegionIdAndHandleReply(RegionIdAndHandleReply {
                                        reply_block: ReplyBlock { region_id: REGION_ID, region_handle },
                                    }),
                                );
                            }
                            RegionLookupRequest::MapBlock(_) => {
                                lookup.handle_message(SIMULATOR, &map_reply(AGENT_ID, 12, 34, "Queued Region"));
                            }
                        }
                    }
                    Ok(())
                }) as SendFuture
            },
        );
        *lookup_slot.lock().unwrap() = Some(lookup.clone());

        assert_eq!(lookup.lookup_region_by_id(REGION_ID).await, Ok("Queued Region".into()));
        assert_eq!(send_attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn queue_full_region_lookup_retries_are_bounded() {
        let send_attempts = Arc::new(AtomicUsize::new(0));
        let attempts = send_attempts.clone();
        let lookup = RegionLookup::for_test(
            AGENT_ID,
            SESSION_ID,
            SIMULATOR,
            |duration| {
                if duration == TEST_TIMEOUT { Box::pin(pending()) as TimeoutFuture } else { Box::pin(ready(())) as TimeoutFuture }
            },
            TEST_TIMEOUT,
            move |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(Error::SimulatorCommandQueueFull) }) as SendFuture
            },
        );

        assert_eq!(lookup.lookup_region_at(RegionGridCell::new(12, 34)).await, Err(Error::SimulatorCommandQueueFull.to_string()));
        assert_eq!(send_attempts.load(Ordering::SeqCst), SEND_RETRY_LIMIT);
        assert!(lookup.lock_state().by_map_cell.is_empty());
    }

    #[tokio::test]
    async fn permanent_send_error_is_not_retried() {
        let send_attempts = Arc::new(AtomicUsize::new(0));
        let attempts = send_attempts.clone();
        let lookup = RegionLookup::for_test(
            AGENT_ID,
            SESSION_ID,
            SIMULATOR,
            |_| Box::pin(pending()) as TimeoutFuture,
            TEST_TIMEOUT,
            move |_, _| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Err(Error::SessionClosed) }) as SendFuture
            },
        );

        assert_eq!(lookup.lookup_region_at(RegionGridCell::new(12, 34)).await, Err(Error::SessionClosed.to_string()));
        assert_eq!(send_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn stale_send_failure_does_not_complete_a_replacement_lookup() {
        let (lookup, _) = lookup();
        let key = LookupKey::MapCell(RegionGridCell::new(12, 34));
        let (old_sender, _old_receiver) = oneshot::channel();
        let old_generation = {
            let mut state = lookup.lock_state();
            let pending = state.pending_mut(key).expect("allocate old lookup");
            pending.waiters.insert(1, old_sender);
            pending.generation
        };

        lookup.cancel_waiter(1);

        let (replacement_sender, replacement_receiver) = oneshot::channel();
        let replacement_generation = {
            let mut state = lookup.lock_state();
            let pending = state.pending_mut(key).expect("allocate replacement lookup");
            pending.waiters.insert(2, replacement_sender);
            pending.generation
        };
        assert_ne!(old_generation, replacement_generation);

        lookup.complete_if_generation(key, old_generation, Err(Error::SessionClosed.to_string()));
        lookup.handle_map_block_reply(&[map_block_reply::Data {
            x: 12,
            y: 34,
            name: "Replacement Region".into(),
            access: 0,
            region_flags: 0,
            water_height: 0,
            agents: 0,
            map_image_id: Uuid::nil(),
        }]);

        assert_eq!(replacement_receiver.await.expect("replacement waiter"), Ok("Replacement Region".into()));
    }

    #[tokio::test]
    async fn queue_full_retry_stops_after_the_last_waiter_leaves() {
        let send_attempts = Arc::new(AtomicUsize::new(0));
        let retry_timers = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicBool::new(true));
        let send_attempts_for_send = send_attempts.clone();
        let active_for_send = active.clone();
        let retry_timers_for_timeout = retry_timers.clone();
        let send_message = Arc::new(move |_, _| {
            send_attempts_for_send.fetch_add(1, Ordering::SeqCst);
            let active = active_for_send.clone();
            Box::pin(async move {
                active.store(false, Ordering::SeqCst);
                Err(Error::SimulatorCommandQueueFull)
            }) as SendFuture
        });
        let make_timeout = Arc::new(move |_| {
            retry_timers_for_timeout.fetch_add(1, Ordering::SeqCst);
            Box::pin(ready(())) as TimeoutFuture
        });
        let request = lookup().0.map_block_request(RegionGridCell::new(12, 34)).expect("valid map request");

        let result = send_with_retry(send_message, make_timeout, request, move || active.load(Ordering::SeqCst)).await;

        assert!(result.is_ok());
        assert_eq!(send_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(retry_timers.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn map_range_rejects_region_handle() {
        let (lookup, sent) = lookup();
        let request_lookup = lookup.clone();
        let pending = spawn(async move { request_lookup.lookup_region_by_id(REGION_ID).await });
        let initial_requests = wait_for_requests(&sent, 1).await;
        assert!(matches!(&initial_requests[0], RegionLookupRequest::RegionHandle(_)));

        let handle = (u64::from(u16::MAX) + 1) * u64::from(RegionGridCell::METERS_PER_CELL);
        lookup.handle_message(
            SIMULATOR,
            &Message::RegionIdAndHandleReply(RegionIdAndHandleReply {
                reply_block: ReplyBlock { region_id: REGION_ID, region_handle: handle << 32 },
            }),
        );

        assert_eq!(pending.await.expect("lookup task"), Err("Second Life returned an invalid region handle.".into()));
        assert_eq!(requests(&sent).len(), 1);
    }

    #[tokio::test]
    async fn times_out_and_clears_pending() {
        let lookup = RegionLookup::for_test(
            AGENT_ID,
            SESSION_ID,
            SIMULATOR,
            |_| Box::pin(ready(())) as TimeoutFuture,
            Duration::from_millis(20),
            |_, _| Box::pin(async { Ok(()) }) as SendFuture,
        );
        assert_eq!(lookup.lookup_region_at(RegionGridCell::new(1, 2)).await, Err("Timed out looking up the region name.".into()));
        assert!(lookup.lock_state().by_map_cell.is_empty());
    }
}
