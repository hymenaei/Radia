use std::{
    collections::HashMap,
    fmt::{self, Display, Formatter},
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};

use futures::channel::oneshot;
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
use tokio::{runtime::Handle, time};
use tracing::Instrument;
use uuid::Uuid;
use world::RegionHandle;

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);
const SEND_RETRY_LIMIT: usize = 4;
const INITIAL_SEND_RETRY_DELAY: Duration = Duration::from_millis(20);
const METERS_PER_CELL: f64 = 256.0;

type SendFuture = Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>;
type SendMessage = dyn Fn(RegionLookupRequest, Delivery) -> SendFuture + Send + Sync;

#[derive(Clone, Debug)]
pub enum RegionLookupError {
    Timeout,
    Canceled,
    OutOfRange,
    InvalidHandle,
    GenerationExhausted,
    Send(Arc<Error>),
}

impl Display for RegionLookupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => formatter.write_str("Timed out looking up the region name."),
            Self::Canceled => formatter.write_str("The region lookup was canceled."),
            Self::OutOfRange => formatter.write_str("The region grid cell is outside the map request range."),
            Self::InvalidHandle => formatter.write_str("Second Life returned an invalid region handle."),
            Self::GenerationExhausted => formatter.write_str("The region lookup generation space is exhausted."),
            Self::Send(error) => Display::fmt(error, formatter),
        }
    }
}

impl std::error::Error for RegionLookupError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Send(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

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
        Self::new_inner(session.agent_id, session.session_id, session.simulator_address, executor, send_message)
    }
    pub async fn lookup_region_by_id(&self, region_id: Uuid) -> Result<String, RegionLookupError> {
        self.lookup(
            LookupKey::RegionId(region_id),
            RegionLookupRequest::RegionHandle(RegionHandleRequest { request_block: RequestBlock { region_id } }),
        )
        .await
    }
    pub(crate) async fn lookup_region_at(&self, cell: RegionGridCell) -> Result<String, RegionLookupError> {
        let request = self.map_block_request(cell).ok_or(RegionLookupError::OutOfRange)?;
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

    async fn lookup(&self, key: LookupKey, request: RegionLookupRequest) -> Result<String, RegionLookupError> {
        let (sender, receiver) = oneshot::channel();
        let (waiter_id, generation, should_send) = {
            let mut state = self.lock_state();
            let waiter_id = state.allocate_waiter_id();
            let Some(pending) = state.pending_mut(key) else {
                return Err(RegionLookupError::GenerationExhausted);
            };
            let should_send = pending.waiters.is_empty();
            let generation = pending.generation;
            pending.waiters.insert(waiter_id, sender);
            (waiter_id, generation, should_send)
        };

        let send_request = should_send.then_some(request);
        let guard = WaiterGuard { lookup: self.clone(), waiter_id };
        let result = async { receiver.await.map_err(|_| RegionLookupError::Canceled)? };
        let dispatch = self.clone();
        if let Some(request) = send_request {
            dispatch.dispatch(key, generation, request);
        }

        match time::timeout(LOOKUP_TIMEOUT, result).await {
            Ok(result) => {
                drop(guard);
                result
            }
            Err(_) => {
                drop(guard);
                Err(RegionLookupError::Timeout)
            }
        }
    }

    fn handle_region_id_reply(&self, region_id: Uuid, region_handle: u64) {
        let origin = RegionHandle::new(region_handle).origin();
        let Some(cell) = RegionGridCell::containing(origin.x, origin.y) else {
            self.complete(LookupKey::RegionId(region_id), Err(RegionLookupError::InvalidHandle));
            return;
        };
        let Some(request) = self.map_block_request(cell) else {
            self.complete(LookupKey::RegionId(region_id), Err(RegionLookupError::InvalidHandle));
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

    fn complete(&self, key: LookupKey, result: Result<String, RegionLookupError>) {
        let pending = self.lock_state().remove_pending(key);
        if let Some(pending) = pending {
            finish(pending, result);
        }
    }

    fn dispatch(&self, key: LookupKey, generation: u64, request: RegionLookupRequest) {
        let lookup = self.clone();
        let send_message = self.inner.send_message.clone();
        let task = async move {
            let active_lookup = lookup.clone();
            if let Err(error) = send_with_retry(send_message, request, move || active_lookup.is_pending(key, generation)).await {
                lookup.complete_if_generation(key, generation, Err(error));
            }
        }
        .in_current_span();
        drop(self.inner.executor.spawn(task));
    }

    fn is_pending(&self, key: LookupKey, generation: u64) -> bool {
        self.lock_state().pending(key).is_some_and(|pending| pending.generation == generation && !pending.waiters.is_empty())
    }

    fn complete_if_generation(&self, key: LookupKey, generation: u64, result: Result<String, RegionLookupError>) {
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
        agent_id: Uuid,
        session_id: Uuid,
        simulator_address: SocketAddr,
        executor: Handle,
        send_message: Arc<SendMessage>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner { agent_id, session_id, simulator_address, executor, send_message, state: Mutex::new(State::default()) }),
        }
    }

    #[cfg(test)]
    fn with_sender(
        agent_id: Uuid,
        session_id: Uuid,
        simulator_address: SocketAddr,
        executor: Handle,
        send_message: impl Fn(RegionLookupRequest, Delivery) -> SendFuture + Send + Sync + 'static,
    ) -> Self {
        Self::new_inner(agent_id, session_id, simulator_address, executor, Arc::new(send_message))
    }
}

struct Inner {
    agent_id: Uuid,
    session_id: Uuid,
    simulator_address: SocketAddr,
    executor: Handle,
    send_message: Arc<SendMessage>,
    state: Mutex<State>,
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
    waiters: HashMap<u64, oneshot::Sender<Result<String, RegionLookupError>>>,
}

#[derive(Clone, Copy)]
enum LookupKey {
    RegionId(Uuid),
    MapCell(RegionGridCell),
}

#[derive(Clone, Debug)]
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

fn finish(pending: PendingLookup, result: Result<String, RegionLookupError>) {
    for sender in pending.waiters.into_values() {
        if sender.send(result.clone()).is_err() {
            tracing::trace!("region lookup receiver closed");
        }
    }
}

async fn send_with_retry(
    send_message: Arc<SendMessage>,
    request: RegionLookupRequest,
    mut is_pending: impl FnMut() -> bool,
) -> Result<(), RegionLookupError> {
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
                time::sleep(retry_delay).await;
                retry_delay = retry_delay.saturating_mul(2);
            }
            Err(error) => return Err(RegionLookupError::Send(Arc::new(error))),
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
        assert_matches,
        future::poll_fn,
        net::{IpAddr, Ipv4Addr},
        sync::{
            Arc, Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        task::Poll,
    };

    use protocol::{
        lludp::Message::MapBlockReply,
        messages::{RegionIdAndHandleReply, map::map_block_reply, region::region_id_and_handle_reply::ReplyBlock},
    };
    use tokio::{
        spawn,
        sync::mpsc::{UnboundedReceiver, unbounded_channel},
        task::yield_now,
        time::advance,
    };

    use super::*;

    const AGENT_ID: Uuid = Uuid::from_u128(1);
    const SESSION_ID: Uuid = Uuid::from_u128(2);
    const REGION_ID: Uuid = Uuid::from_u128(3);
    const SIMULATOR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 13000);
    const OTHER_SIMULATOR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 13001);

    fn with_sender(send_message: impl Fn(RegionLookupRequest, Delivery) -> SendFuture + Send + Sync + 'static) -> RegionLookup {
        RegionLookup::with_sender(AGENT_ID, SESSION_ID, SIMULATOR, Handle::current(), send_message)
    }

    fn lookup() -> (RegionLookup, UnboundedReceiver<RegionLookupRequest>) {
        let (sender, receiver) = unbounded_channel();
        let lookup = with_sender(move |request, _| {
            let sender = sender.clone();
            Box::pin(async move { sender.send(request).map_err(|_| Error::SessionClosed) }) as SendFuture
        });
        (lookup, receiver)
    }

    async fn take_requests(receiver: &mut UnboundedReceiver<RegionLookupRequest>, count: usize) -> Vec<RegionLookupRequest> {
        let mut requests = Vec::with_capacity(count);
        for _ in 0..count {
            requests.push(receiver.recv().await.expect("lookup request"));
        }
        requests
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

    fn poll_pending<F: Future>(future: Pin<&mut F>, context: &mut std::task::Context<'_>)
    where
        F::Output: fmt::Debug,
    {
        assert_matches!(future.poll(context), Poll::Pending);
    }

    #[tokio::test]
    async fn coalesces_and_checks_replies() {
        let (lookup, mut sent) = lookup();
        let first = lookup.lookup_region_at(RegionGridCell::new(12, 34));
        let second = lookup.lookup_region_at(RegionGridCell::new(12, 34));
        tokio::pin!(first, second);
        poll_fn(|context| {
            poll_pending(first.as_mut(), context);
            poll_pending(second.as_mut(), context);
            Poll::Ready(())
        })
        .await;

        let requests = take_requests(&mut sent, 1).await;
        assert_matches!(
            &requests[0],
            RegionLookupRequest::MapBlock(request)
                if request.position_data.min_x == 12 && request.position_data.min_y == 34
        );
        lookup.handle_message(OTHER_SIMULATOR, &map_reply(AGENT_ID, 12, 34, "Wrong source"));
        lookup.handle_message(SIMULATOR, &map_reply(Uuid::from_u128(99), 12, 34, "Wrong agent"));
        lookup.handle_message(SIMULATOR, &map_reply(AGENT_ID, 12, 34, "Known Region"));

        let (first, second) = tokio::join!(first, second);
        assert_matches!(first, Ok(name) if name == "Known Region");
        assert_matches!(second, Ok(name) if name == "Known Region");
    }

    #[tokio::test]
    async fn id_lookup_coalesces_requests() {
        let (lookup, mut sent) = lookup();
        let first = lookup.lookup_region_by_id(REGION_ID);
        let second = lookup.lookup_region_by_id(REGION_ID);
        tokio::pin!(first, second);
        poll_fn(|context| {
            poll_pending(first.as_mut(), context);
            poll_pending(second.as_mut(), context);
            Poll::Ready(())
        })
        .await;

        let requests = take_requests(&mut sent, 1).await;
        assert_matches!(&requests[0], RegionLookupRequest::RegionHandle(request) if request.request_block.region_id == REGION_ID);
        let handle = ((42_u64 * 256 + 200) << 32) | (73_u64 * 256 + 255);
        lookup.handle_message(
            SIMULATOR,
            &Message::RegionIdAndHandleReply(RegionIdAndHandleReply {
                reply_block: ReplyBlock { region_id: REGION_ID, region_handle: handle },
            }),
        );
        let requests = take_requests(&mut sent, 1).await;
        assert_matches!(
            &requests[0],
            RegionLookupRequest::MapBlock(request)
                if request.position_data.min_x == 42 && request.position_data.max_x == 42
                    && request.position_data.min_y == 73 && request.position_data.max_y == 73
        );
        lookup.handle_message(SIMULATOR, &map_reply(AGENT_ID, 42, 73, "Resolved Region"));
        let (first, second) = tokio::join!(first, second);
        assert_matches!(first, Ok(name) if name == "Resolved Region");
        assert_matches!(second, Ok(name) if name == "Resolved Region");
    }

    #[tokio::test]
    async fn id_promotion_merges_cell_waiters() {
        let (lookup, mut sent) = lookup();
        let cell_lookup = lookup.lookup_region_at(RegionGridCell::new(42, 73));
        let id_lookup = lookup.lookup_region_by_id(REGION_ID);
        tokio::pin!(cell_lookup, id_lookup);
        poll_fn(|context| {
            poll_pending(cell_lookup.as_mut(), context);
            poll_pending(id_lookup.as_mut(), context);
            Poll::Ready(())
        })
        .await;

        let requests = take_requests(&mut sent, 2).await;
        assert_eq!(requests.iter().filter(|request| matches!(request, RegionLookupRequest::RegionHandle(_))).count(), 1);
        assert_eq!(requests.iter().filter(|request| matches!(request, RegionLookupRequest::MapBlock(_))).count(), 1);
        let handle = ((42_u64 * 256) << 32) | (73_u64 * 256);
        lookup.handle_message(
            SIMULATOR,
            &Message::RegionIdAndHandleReply(RegionIdAndHandleReply {
                reply_block: ReplyBlock { region_id: REGION_ID, region_handle: handle },
            }),
        );
        yield_now().await;
        assert_matches!(sent.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
        lookup.handle_message(SIMULATOR, &map_reply(AGENT_ID, 42, 73, "Merged Region"));
        let (cell_name, id_name) = tokio::join!(cell_lookup, id_lookup);
        assert_matches!(cell_name, Ok(name) if name == "Merged Region");
        assert_matches!(id_name, Ok(name) if name == "Merged Region");
    }

    #[tokio::test(start_paused = true)]
    async fn queue_full_retries_region_id_lookup() {
        let lookup_slot = Arc::new(Mutex::new(None::<RegionLookup>));
        let slot = lookup_slot.clone();
        let attempts = Arc::new(AtomicUsize::new(0));
        let send_attempts = attempts.clone();
        let (attempt_sender, mut attempt_receiver) = unbounded_channel();
        let lookup = with_sender(move |request, _| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let attempt_sender = attempt_sender.clone();
            let lookup = slot.lock().unwrap().as_ref().cloned();
            Box::pin(async move {
                attempt_sender.send(attempt).map_err(|_| Error::SessionClosed)?;
                if attempt == 0 {
                    return Err(Error::SimulatorCommandQueueFull);
                }
                if let Some(lookup) = lookup {
                    match request {
                        RegionLookupRequest::RegionHandle(_) => {
                            let region_handle = ((12_u64 * 256) << 32) | (34_u64 * 256);
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
        });
        *lookup_slot.lock().unwrap() = Some(lookup.clone());

        let task = spawn(async move { lookup.lookup_region_by_id(REGION_ID).await });
        assert_eq!(attempt_receiver.recv().await, Some(0));
        advance(INITIAL_SEND_RETRY_DELAY).await;
        assert_eq!(attempt_receiver.recv().await, Some(1));
        assert_eq!(attempt_receiver.recv().await, Some(2));
        assert_matches!(task.await.expect("lookup task"), Ok(name) if name == "Queued Region");
        assert_eq!(send_attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test(start_paused = true)]
    async fn queue_full_retries_are_bounded_and_exponential() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let send_attempts = attempts.clone();
        let (attempt_sender, mut attempt_receiver) = unbounded_channel();
        let lookup = with_sender(move |_, _| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let attempt_sender = attempt_sender.clone();
            Box::pin(async move {
                attempt_sender.send(attempt).map_err(|_| Error::SessionClosed)?;
                Err(Error::SimulatorCommandQueueFull)
            }) as SendFuture
        });

        let task_lookup = lookup.clone();
        let task = spawn(async move { task_lookup.lookup_region_at(RegionGridCell::new(12, 34)).await });
        assert_eq!(attempt_receiver.recv().await, Some(0));
        let mut delay = INITIAL_SEND_RETRY_DELAY;
        for attempt in 1..SEND_RETRY_LIMIT {
            advance(delay - Duration::from_millis(1)).await;
            assert_matches!(attempt_receiver.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
            advance(Duration::from_millis(1)).await;
            assert_eq!(attempt_receiver.recv().await, Some(attempt));
            delay = delay.saturating_mul(2);
        }

        assert_matches!(
            task.await.expect("lookup task"),
            Err(RegionLookupError::Send(error)) if matches!(error.as_ref(), Error::SimulatorCommandQueueFull)
        );
        assert_eq!(send_attempts.load(Ordering::SeqCst), SEND_RETRY_LIMIT);
        assert!(lookup.lock_state().by_map_cell.is_empty());
    }

    #[tokio::test]
    async fn permanent_send_error_is_not_retried() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let send_attempts = attempts.clone();
        let (attempt_sender, mut attempt_receiver) = unbounded_channel();
        let lookup = with_sender(move |_, _| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let attempt_sender = attempt_sender.clone();
            Box::pin(async move {
                attempt_sender.send(attempt).map_err(|_| Error::SessionClosed)?;
                Err(Error::SessionClosed)
            }) as SendFuture
        });

        let task_lookup = lookup.clone();
        let task = spawn(async move { task_lookup.lookup_region_at(RegionGridCell::new(12, 34)).await });
        assert_eq!(attempt_receiver.recv().await, Some(0));
        assert_matches!(task.await.expect("lookup task"), Err(RegionLookupError::Send(error)) if matches!(error.as_ref(), Error::SessionClosed));
        assert_eq!(send_attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_stops_retry_and_allows_replacement() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let send_attempts = attempts.clone();
        let (attempt_sender, mut attempt_receiver) = unbounded_channel();
        let lookup = with_sender(move |_, _| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let attempt_sender = attempt_sender.clone();
            Box::pin(async move {
                attempt_sender.send(attempt).map_err(|_| Error::SessionClosed)?;
                Err(Error::SimulatorCommandQueueFull)
            }) as SendFuture
        });

        let first_lookup = lookup.clone();
        let first = spawn(async move { first_lookup.lookup_region_at(RegionGridCell::new(12, 34)).await });
        assert_eq!(attempt_receiver.recv().await, Some(0));
        first.abort();
        assert_matches!(first.await, Err(error) if error.is_cancelled());
        advance(Duration::from_secs(1)).await;
        yield_now().await;
        assert_matches!(attempt_receiver.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
        assert_eq!(send_attempts.load(Ordering::SeqCst), 1);

        let replacement_lookup = lookup.clone();
        let replacement = spawn(async move { replacement_lookup.lookup_region_at(RegionGridCell::new(12, 34)).await });
        assert_eq!(attempt_receiver.recv().await, Some(1));
        replacement.abort();
        assert_matches!(replacement.await, Err(error) if error.is_cancelled());
    }

    #[tokio::test]
    async fn stale_send_failure_preserves_replacement() {
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

        lookup.complete_if_generation(key, old_generation, Err(RegionLookupError::Send(Arc::new(Error::SessionClosed))));
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
        assert_matches!(replacement_receiver.await.expect("replacement waiter"), Ok(name) if name == "Replacement Region");
    }

    #[tokio::test]
    async fn map_range_rejects_region_handle() {
        let (lookup, mut sent) = lookup();
        let request = lookup.lookup_region_by_id(REGION_ID);
        tokio::pin!(request);
        poll_fn(|context| {
            poll_pending(request.as_mut(), context);
            Poll::Ready(())
        })
        .await;
        assert_matches!(sent.recv().await, Some(RegionLookupRequest::RegionHandle(_)));

        let handle = (u64::from(u16::MAX) + 1) * u64::from(RegionGridCell::METERS_PER_CELL);
        lookup.handle_message(
            SIMULATOR,
            &Message::RegionIdAndHandleReply(RegionIdAndHandleReply {
                reply_block: ReplyBlock { region_id: REGION_ID, region_handle: handle << 32 },
            }),
        );
        assert_matches!(request.await, Err(RegionLookupError::InvalidHandle));
        assert_matches!(sent.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
    }

    #[tokio::test]
    async fn rejects_out_of_range_cells() {
        let (lookup, mut sent) = lookup();
        assert_matches!(lookup.lookup_region_at(RegionGridCell::new(u32::from(u16::MAX) + 1, 0)).await, Err(RegionLookupError::OutOfRange));
        assert_matches!(sent.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
    }

    #[tokio::test(start_paused = true)]
    async fn times_out_and_clears_pending() {
        let (lookup, mut sent) = lookup();
        let request_lookup = lookup.clone();
        let task = spawn(async move { request_lookup.lookup_region_at(RegionGridCell::new(1, 2)).await });
        assert_matches!(sent.recv().await, Some(RegionLookupRequest::MapBlock(_)));
        advance(LOOKUP_TIMEOUT).await;
        assert_matches!(task.await.expect("lookup task"), Err(RegionLookupError::Timeout));
        assert!(lookup.lock_state().by_map_cell.is_empty());
    }

    #[tokio::test]
    async fn generation_exhaustion_is_reported() {
        let (lookup, mut sent) = lookup();
        lookup.lock_state().next_generation = u64::MAX;
        assert_matches!(lookup.lookup_region_by_id(REGION_ID).await, Err(RegionLookupError::GenerationExhausted));
        assert_matches!(sent.try_recv(), Err(tokio::sync::mpsc::error::TryRecvError::Empty));
    }
}
