//! Asynchronous region-name lookup by UUID or map cell.

use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use base::UUID;
use futures::{channel::oneshot, future::select, pin_mut};
use gpui::BackgroundExecutor;
use network::{AgentSession, Delivery, SessionHandle};
use protocol::{
    messages::{
        MapBlockRequest, RegionHandleRequest,
        map_block_reply::Data as MapBlockData,
        map_block_request::{AgentData as MapAgentData, PositionData},
        region_handle_request::RequestBlock,
    },
    udp::Message,
};

const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);

type MapCell = (u16, u16);
type SendMessage = dyn Fn(RegionLookupRequest, Delivery) -> Result<(), String> + Send + Sync;
type TimeoutFuture = Pin<Box<dyn Future<Output = ()> + Send>>;
type MakeTimeout = dyn Fn(Duration) -> TimeoutFuture + Send + Sync;

/// Resolves simulator region IDs and grid cells to region names.
///
/// Concurrent lookups for the same key share one request and time out after 15 seconds. Dropping a caller cancels only its waiter.
#[derive(Clone)]
pub struct RegionLookup {
    inner: Arc<Inner>,
}

struct Inner {
    agent_id: UUID,
    session_id: UUID,
    simulator_address: SocketAddr,
    make_timeout: Arc<MakeTimeout>,
    send_message: Arc<SendMessage>,
    state: Mutex<State>,
    timeout: Duration,
}

#[derive(Default)]
struct State {
    next_waiter_id: u64,
    by_region_id: HashMap<UUID, PendingLookup>,
    by_map_cell: HashMap<MapCell, PendingLookup>,
}

#[derive(Default)]
struct PendingLookup {
    waiters: HashMap<u64, oneshot::Sender<Result<String, String>>>,
}

#[derive(Clone, Copy)]
enum LookupKey {
    RegionId(UUID),
    MapCell(MapCell),
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

impl RegionLookup {
    /// Creates a lookup for the session's primary simulator.
    ///
    /// Pass session messages to [`handle_message`](Self::handle_message) so replies can complete pending lookups.
    pub fn new(session_handle: SessionHandle, session: &AgentSession, executor: BackgroundExecutor) -> Self {
        let send_message = Arc::new(move |request, delivery| match request {
            RegionLookupRequest::RegionHandle(request) => session_handle.send(request, delivery).map_err(|error| error.to_string()),
            RegionLookupRequest::MapBlock(request) => session_handle.send(request, delivery).map_err(|error| error.to_string()),
        });
        let make_timeout = Arc::new(move |timeout| Box::pin(executor.timer(timeout)) as TimeoutFuture);
        Self::new_inner(session, make_timeout, send_message, LOOKUP_TIMEOUT)
    }

    /// # Errors
    ///
    /// Returns an error if sending fails, the reply is invalid, or it times out.
    pub async fn lookup_region_by_id(&self, region_id: UUID) -> Result<String, String> {
        self.lookup(
            LookupKey::RegionId(region_id),
            RegionLookupRequest::RegionHandle(RegionHandleRequest { request_block: RequestBlock { region_id } }),
        )
        .await
    }

    /// `x` and `y` are region-grid coordinates, not meter coordinates.
    ///
    /// # Errors
    ///
    /// Returns an error if sending fails or the reply times out.
    pub async fn lookup_region_at(&self, x: u16, y: u16) -> Result<String, String> {
        self.lookup(LookupKey::MapCell((x, y)), self.map_block_request(x, y)).await
    }

    /// Call this in the session message flow. Replies from another simulator or agent are ignored.
    pub fn handle_message(&self, source: SocketAddr, message: &Message) {
        if source != self.inner.simulator_address {
            return;
        }

        match message {
            Message::RegionIDAndHandleReply(message) => {
                self.handle_region_id_reply(message.reply_block.region_id, message.reply_block.region_handle);
            }
            Message::MapBlockReply(message) if message.agent_data.agent_id == self.inner.agent_id => {
                self.handle_map_block_reply(&message.data);
            }
            _ => {}
        }
    }

    fn lookup(&self, key: LookupKey, request: RegionLookupRequest) -> impl std::future::Future<Output = Result<String, String>> + 'static {
        let (sender, receiver) = oneshot::channel();
        let (waiter_id, should_send) = {
            let mut state = self.lock_state();
            let waiter_id = state.allocate_waiter_id();
            let pending = state.pending_mut(key);
            let should_send = pending.waiters.is_empty();
            pending.waiters.insert(waiter_id, sender);
            (waiter_id, should_send)
        };

        if should_send && let Err(error) = (self.inner.send_message)(request, Delivery::Reliable) {
            self.complete(key, Err(error));
        }

        let timeout = self.inner.timeout;
        let timeout_factory = self.inner.make_timeout.clone();
        let guard = WaiterGuard { lookup: self.clone(), waiter_id };
        let lookup = async move { receiver.await.map_err(|_| "The region lookup was canceled.".to_string())? };
        async move {
            pin_mut!(lookup);
            let timer = timeout_factory(timeout);
            pin_mut!(timer);
            match select(lookup, timer).await {
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
    }

    fn handle_region_id_reply(&self, region_id: UUID, region_handle: u64) {
        let Some(cell) = region_grid_coordinates(region_handle) else {
            self.complete(LookupKey::RegionId(region_id), Err("Second Life returned an invalid region handle.".into()));
            return;
        };

        let should_request_cell = {
            let mut state = self.lock_state();
            let Some(mut pending) = state.by_region_id.remove(&region_id) else {
                return;
            };
            if let Some(existing) = state.by_map_cell.get_mut(&cell) {
                existing.waiters.extend(pending.waiters.drain());
                false
            } else {
                state.by_map_cell.insert(cell, pending);
                true
            }
        };

        if should_request_cell && let Err(error) = (self.inner.send_message)(self.map_block_request(cell.0, cell.1), Delivery::Reliable) {
            self.complete(LookupKey::MapCell(cell), Err(error));
        }
    }

    fn handle_map_block_reply(&self, blocks: &[MapBlockData]) {
        let completed = {
            let mut state = self.lock_state();
            blocks
                .iter()
                .filter_map(|block| {
                    state.by_map_cell.remove(&(block.x, block.y)).map(|pending| {
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

    fn map_block_request(&self, x: u16, y: u16) -> RegionLookupRequest {
        RegionLookupRequest::MapBlock(MapBlockRequest {
            agent_data: MapAgentData {
                agent_id: self.inner.agent_id,
                session_id: self.inner.session_id,
                flags: 0,
                estate_id: 0,
                godlike: false,
            },
            position_data: PositionData { min_x: x, max_x: x, min_y: y, max_y: y },
        })
    }

    fn complete(&self, key: LookupKey, result: Result<String, String>) {
        let pending = {
            let mut state = self.lock_state();
            match key {
                LookupKey::RegionId(region_id) => state.by_region_id.remove(&region_id),
                LookupKey::MapCell(cell) => state.by_map_cell.remove(&cell),
            }
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
        self.inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn new_inner(session: &AgentSession, make_timeout: Arc<MakeTimeout>, send_message: Arc<SendMessage>, timeout: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                agent_id: session.agent_id,
                session_id: session.session_id,
                simulator_address: session.simulator_address,
                make_timeout,
                send_message,
                state: Mutex::new(State::default()),
                timeout,
            }),
        }
    }

    #[cfg(test)]
    fn for_test(
        agent_id: UUID,
        session_id: UUID,
        simulator_address: SocketAddr,
        make_timeout: impl Fn(Duration) -> TimeoutFuture + Send + Sync + 'static,
        timeout: Duration,
        send_message: impl Fn(RegionLookupRequest, Delivery) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                agent_id,
                session_id,
                simulator_address,
                make_timeout: Arc::new(make_timeout),
                send_message: Arc::new(send_message),
                state: Mutex::new(State::default()),
                timeout,
            }),
        }
    }
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

    fn pending_mut(&mut self, key: LookupKey) -> &mut PendingLookup {
        match key {
            LookupKey::RegionId(region_id) => self.by_region_id.entry(region_id).or_default(),
            LookupKey::MapCell(cell) => self.by_map_cell.entry(cell).or_default(),
        }
    }
}

fn region_grid_coordinates(region_handle: u64) -> Option<MapCell> {
    let origin_x = region_handle >> 32;
    let origin_y = region_handle & u64::from(u32::MAX);
    Some((u16::try_from(origin_x / 256).ok()?, u16::try_from(origin_y / 256).ok()?))
}

fn finish(pending: PendingLookup, result: Result<String, String>) {
    for sender in pending.waiters.into_values() {
        let _ = sender.send(result.clone());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{IpAddr, Ipv4Addr},
        sync::{Arc, Mutex},
    };

    use protocol::messages::map_block_reply::Data as MapBlockData;
    use protocol::messages::{RegionIDAndHandleReply, region_id_and_handle_reply::ReplyBlock};
    use tokio::task::JoinHandle;

    use super::*;

    const AGENT_ID: UUID = UUID::from_u128(1);
    const SESSION_ID: UUID = UUID::from_u128(2);
    const REGION_ID: UUID = UUID::from_u128(3);
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
            |_| Box::pin(std::future::pending()) as TimeoutFuture,
            TEST_TIMEOUT,
            move |message, _| {
                capture.lock().unwrap().push(message);
                Ok(())
            },
        );
        (lookup, sent)
    }

    fn requests(sent: &Arc<Mutex<Vec<RegionLookupRequest>>>) -> Vec<RegionLookupRequest> {
        sent.lock().unwrap().clone()
    }

    fn map_reply(agent_id: UUID, x: u16, y: u16, name: &str) -> Message {
        Message::MapBlockReply(protocol::messages::MapBlockReply {
            agent_data: protocol::messages::map_block_reply::AgentData { agent_id, flags: 0 },
            data: vec![MapBlockData {
                x,
                y,
                name: name.into(),
                access: 0,
                region_flags: 0,
                water_height: 0,
                agents: 0,
                map_image_id: UUID::nil(),
            }],
        })
    }

    async fn wait_for_requests(sent: &Arc<Mutex<Vec<RegionLookupRequest>>>, count: usize) -> Vec<RegionLookupRequest> {
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let requests = requests(sent);
                if requests.len() >= count {
                    return requests;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("expected network request")
    }

    async fn assert_pending(task: &JoinHandle<Result<String, String>>) {
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
    }

    #[tokio::test]
    async fn coalesces_and_checks_replies() {
        let (lookup, sent) = lookup();
        let first_lookup = lookup.clone();
        let first = tokio::spawn(async move { first_lookup.lookup_region_at(12, 34).await });
        let second_lookup = lookup.clone();
        let second = tokio::spawn(async move { second_lookup.lookup_region_at(12, 34).await });

        let requests = wait_for_requests(&sent, 1).await;
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0],
            RegionLookupRequest::MapBlock(request)
                if request.position_data.min_x == 12 && request.position_data.min_y == 34
        ));

        lookup.handle_message(OTHER_SIMULATOR, &map_reply(AGENT_ID, 12, 34, "Wrong source"));
        lookup.handle_message(SIMULATOR, &map_reply(UUID::from_u128(99), 12, 34, "Wrong agent"));
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
        let first = tokio::spawn(async move { first_lookup.lookup_region_by_id(REGION_ID).await });
        let second_lookup = lookup.clone();
        let second = tokio::spawn(async move { second_lookup.lookup_region_by_id(REGION_ID).await });

        let requests = wait_for_requests(&sent, 1).await;
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            &requests[0],
            RegionLookupRequest::RegionHandle(request) if request.request_block.region_id == REGION_ID
        ));

        let handle = ((42u64 * 256) << 32) | (73u64 * 256);
        lookup.handle_message(
            SIMULATOR,
            &Message::RegionIDAndHandleReply(RegionIDAndHandleReply {
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
    async fn times_out_and_clears_pending() {
        let lookup = RegionLookup::for_test(
            AGENT_ID,
            SESSION_ID,
            SIMULATOR,
            |_| Box::pin(std::future::ready(())) as TimeoutFuture,
            Duration::from_millis(20),
            |_, _| Ok(()),
        );
        assert_eq!(lookup.lookup_region_at(1, 2).await, Err("Timed out looking up the region name.".into()));
        assert!(lookup.lock_state().by_map_cell.is_empty());
    }
}
