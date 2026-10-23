use std::{
    collections::{HashMap, VecDeque},
    future::pending,
    mem,
    net::SocketAddr,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use ::world::{Applied, Entity, LocalAddress, ObjectObservation, OrderEvidence, OrderEvidence::Ordered, RegionHandle, UpdateSource};
use futures::{
    FutureExt,
    channel::{mpsc, oneshot},
    executor::block_on,
    pin_mut,
    stream::StreamExt,
};
use network::{
    AgentSession, CoverageLoss, CoverageSource, Delivery, Event, Grid, MfaChallenge, PacketOrder, SimulatorId, capabilities::Client,
};
use protocol::lludp::Message;
use scene::Camera;
use tokio::runtime::Handle;
use tokio::sync::watch;
use uuid::Uuid;

use crate::{
    asset_coordinator::{AssetCoordinator, AssetHandle},
    inventory::{FavoriteLocation, favorite_store, favorites::load_favorites},
    remembered_login::{self, RememberedLogin},
    session::{
        ChatMessage, RegionLookup, Session, SessionHandle, agent, display_message,
        environment::EnvironmentIntake,
        experience_environment::{self, EnvironmentCommand, EnvironmentInbox},
        inbound::{
            AvatarAppearanceMessage, ObjectBatch, ObjectOperation, SitTargetUpdate, decode_avatar_appearance_update, decode_object_message,
            decode_sit_target_update, object_media_requests,
        },
        object_media::{ObjectMediaFetcher, ObjectMediaResult, ObjectMediaTarget, response_is_stale},
        recovery::{self, RecoveryScheduler},
        terrain::TerrainDecodeQueue,
        world::{IngestStatus, IngestSummary, MAX_WORLD_ENTITIES, RegistrationError, SessionWorld, viewer_world_limits},
    },
    world_reader::{WorldReadHost, WorldReader},
};

const MAX_PENDING_CHAT_MESSAGES: usize = 64;
const MAX_PENDING_CHAT_BYTES: usize = 256 * 1024;
const MAX_HANDOFF_OBSERVATIONS: usize = 32;
const MAX_WAITING_OBJECT_MEDIA: usize = MAX_WORLD_ENTITIES;
const MAX_MEDIA_FLUSH_BATCH: usize = 64;
const PRESENTATION_WAKE_BUFFER: usize = 0;
const QUERY_STATE_MAINTENANCE_INTERVAL: Duration = Duration::from_millis(25);
const QUERY_STATE_MAINTENANCE_RETRY: Duration = Duration::from_millis(1);
const CAMERA_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const CAMERA_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

fn next_query_state_maintenance_delay(elapsed: Duration, has_dirty_pressure: bool) -> Duration {
    if has_dirty_pressure {
        return QUERY_STATE_MAINTENANCE_RETRY;
    }
    let elapsed_in_interval = Duration::from_nanos((elapsed.as_nanos() % QUERY_STATE_MAINTENANCE_INTERVAL.as_nanos()) as u64);
    QUERY_STATE_MAINTENANCE_INTERVAL - elapsed_in_interval
}

enum DrainInput {
    Cancelled,
    Event(Option<Event>),
    Media(Option<ObjectMediaResult>),
    QueryStateMaintenance,
}

async fn select_drain_input<E, M, C, T>(event: E, media: M, cancellation: C, maintenance: T) -> DrainInput
where
    E: std::future::Future<Output = Option<Event>>,
    M: std::future::Future<Output = Option<ObjectMediaResult>>,
    C: std::future::Future<Output = ()>,
    T: std::future::Future<Output = ()>,
{
    let event = event.fuse();
    let media = media.fuse();
    let cancellation = cancellation.fuse();
    let maintenance = maintenance.fuse();
    pin_mut!(event, media, cancellation, maintenance);
    futures::select! {
        _ = cancellation => DrainInput::Cancelled,
        event = event => DrainInput::Event(event),
        result = media => DrainInput::Media(result),
        _ = maintenance => DrainInput::QueryStateMaintenance,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingWorldHandoff {
    source: SimulatorId,
    target: SimulatorId,
}

#[derive(Default)]
struct WaitingObjectMedia {
    order: VecDeque<Entity>,
    targets: HashMap<Entity, ObjectMediaTarget>,
}

impl WaitingObjectMedia {
    fn len(&self) -> usize {
        self.targets.len()
    }

    fn update(&mut self, target: ObjectMediaTarget) -> bool {
        let Some(current) = self.targets.get_mut(&target.binding.entity()) else { return false };
        let same_binding = current.simulator_id == target.simulator_id && current.binding == target.binding;
        if !same_binding || target.is_newer_than(*current) {
            *current = target;
        }
        true
    }

    fn insert(&mut self, target: ObjectMediaTarget) -> bool {
        if self.update(target) {
            return true;
        }
        if self.len() >= MAX_WAITING_OBJECT_MEDIA {
            return false;
        }
        self.order.push_back(target.binding.entity());
        self.targets.insert(target.binding.entity(), target);
        true
    }

    fn retain_simulator(&mut self, simulator_id: SimulatorId) {
        self.targets.retain(|_, target| target.simulator_id != simulator_id);
        self.order.retain(|entity| self.targets.contains_key(entity));
    }

    fn pop_front(&mut self) -> Option<ObjectMediaTarget> {
        while let Some(entity) = self.order.pop_front() {
            if let Some(target) = self.targets.remove(&entity) {
                return Some(target);
            }
        }
        None
    }

    fn push_front(&mut self, target: ObjectMediaTarget) {
        self.order.push_front(target.binding.entity());
        self.targets.insert(target.binding.entity(), target);
    }

    fn push_back(&mut self, target: ObjectMediaTarget) {
        self.order.push_back(target.binding.entity());
        self.targets.insert(target.binding.entity(), target);
    }
}

enum ObjectMediaIntake {
    Enabled { fetcher: ObjectMediaFetcher, clients: HashMap<SimulatorId, Client>, waiting: WaitingObjectMedia },
    Disabled,
}

impl ObjectMediaIntake {
    fn new(fetcher: ObjectMediaFetcher) -> Self {
        Self::Enabled { fetcher, clients: HashMap::new(), waiting: WaitingObjectMedia::default() }
    }

    async fn next_result(&mut self) -> Option<ObjectMediaResult> {
        match self {
            Self::Enabled { fetcher, .. } => fetcher.next_result().await,
            Self::Disabled => pending().await,
        }
    }

    fn disable(&mut self) {
        *self = Self::Disabled;
    }

    fn set_client(&mut self, simulator_id: SimulatorId, client: Client) {
        if let Self::Enabled { clients, .. } = self {
            clients.insert(simulator_id, client);
        }
    }

    fn finish(&mut self, result: &ObjectMediaResult) -> Option<ObjectMediaTarget> {
        match self {
            Self::Enabled { fetcher, .. } => fetcher.finish(result),
            Self::Disabled => None,
        }
    }

    fn forget_simulator(&mut self, simulator_id: SimulatorId) {
        let Self::Enabled { fetcher, clients, waiting } = self else { return };
        clients.remove(&simulator_id);
        fetcher.forget_simulator(simulator_id);
        waiting.retain_simulator(simulator_id);
    }

    fn queue(&mut self, target: ObjectMediaTarget, world: &SessionWorld) {
        let Self::Enabled { fetcher, clients, waiting } = self else { return };
        if !world.object_media_target_is_current(target) {
            return;
        }
        if waiting.update(target) {
            return;
        }
        if let Some(client) = clients.get(&target.simulator_id)
            && fetcher.enqueue(target, client.clone())
        {
            return;
        }
        if !waiting.insert(target) {
            tracing::error!("ObjectMedia queue reached the live object limit");
        }
    }

    fn flush(&mut self, world: &SessionWorld) {
        let Self::Enabled { fetcher, clients, waiting } = self else { return };
        let attempts = waiting.len().min(MAX_MEDIA_FLUSH_BATCH);
        for _ in 0..attempts {
            let Some(target) = waiting.pop_front() else { break };
            if !world.object_media_target_is_current(target) {
                continue;
            }
            let Some(client) = clients.get(&target.simulator_id) else {
                waiting.push_back(target);
                continue;
            };
            if !fetcher.enqueue(target, client.clone()) {
                waiting.push_front(target);
                break;
            }
        }
    }
}

pub(crate) type WriteLock = Arc<futures::lock::Mutex<()>>;

#[derive(Clone)]
pub(crate) struct PresentationMailbox {
    state: Arc<Mutex<PendingPresentation>>,
    wake_sender: Arc<Mutex<mpsc::Sender<()>>>,
}

pub(crate) struct PresentationReceiver {
    generation: u64,
    state: Arc<Mutex<PendingPresentation>>,
    wake_receiver: mpsc::Receiver<()>,
}

pub(crate) struct PresentationSnapshot {
    pub(crate) generation: u64,
    pub(crate) authentication: Option<AuthenticationUpdate>,
    pub(crate) session_ready: Option<SessionReadyUpdate>,
    pub(crate) region_name: Option<String>,
    pub(crate) chat_messages: VecDeque<ChatMessage>,
    pub(crate) failure: Option<String>,
    pub(crate) favorites: Option<FavoritesUpdate>,
    pub(crate) favorite_store_saved: Option<(u64, Result<(), String>)>,
}

pub(crate) enum AuthenticationUpdate {
    MfaRequired(MfaChallenge),
    CredentialsUpdated { remembered_login: Option<RememberedLogin>, warning: Option<String> },
}

pub(crate) struct SessionReadyUpdate {
    pub(crate) environments: EnvironmentInbox,
    pub(crate) camera: watch::Sender<Option<Camera>>,
    pub(crate) assets: AssetHandle,
    pub(crate) session: AgentSession,
    pub(crate) region_lookup: RegionLookup,
    pub(crate) region_name: Option<String>,
    pub(crate) world_reader: WorldReader,
}

pub(crate) struct FavoritesUpdate {
    pub(crate) grid: Grid,
    pub(crate) agent_id: Uuid,
    pub(crate) session_id: Uuid,
    pub(crate) username: String,
    pub(crate) result: Result<Vec<FavoriteLocation>, String>,
}

#[derive(Default)]
struct PendingPresentation {
    authentication: Option<AuthenticationUpdate>,
    session_ready: Option<SessionReadyUpdate>,
    region_name: Option<String>,
    chat_messages: VecDeque<ChatMessage>,
    chat_bytes: usize,
    failure: Option<String>,
    favorites: Option<FavoritesUpdate>,
    favorite_store_saved: Option<(u64, Result<(), String>)>,
}

pub(crate) enum ViewerUpdate {
    Authentication(AuthenticationUpdate),
    SessionReady(SessionReadyUpdate),
    RegionName(String),
    ChatMessage(ChatMessage),
    Failed(String),
    Favorites(FavoritesUpdate),
    FavoriteStoreSaved { generation: u64, result: Result<(), String> },
}

#[derive(Clone, Copy)]
enum SimulatorScope {
    Primary,
    Neighbor,
}

impl PresentationMailbox {
    pub(crate) fn new(generation: u64) -> (Self, PresentationReceiver) {
        let state = Arc::new(Mutex::new(PendingPresentation::default()));
        let (wake_sender, wake_receiver) = mpsc::channel(PRESENTATION_WAKE_BUFFER);
        (
            Self { state: state.clone(), wake_sender: Arc::new(Mutex::new(wake_sender)) },
            PresentationReceiver { generation, state, wake_receiver },
        )
    }

    pub(crate) fn publish(&self, update: ViewerUpdate) {
        {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match update {
                ViewerUpdate::Authentication(update) => state.authentication = Some(update),
                ViewerUpdate::SessionReady(update) => {
                    state.region_name = None;
                    state.session_ready = Some(update);
                }
                ViewerUpdate::RegionName(name) => state.region_name = Some(name),
                ViewerUpdate::ChatMessage(message) => state.push_chat(message),
                ViewerUpdate::Failed(error) => {
                    state.chat_messages.clear();
                    state.chat_bytes = 0;
                    state.failure = Some(error);
                }
                ViewerUpdate::Favorites(update) => state.favorites = Some(update),
                ViewerUpdate::FavoriteStoreSaved { generation, result } => {
                    if state.favorite_store_saved.as_ref().is_none_or(|(current, _)| generation >= *current) {
                        state.favorite_store_saved = Some((generation, result));
                    }
                }
            }
        }

        self.wake();
    }
}

impl PresentationMailbox {
    fn wake(&self) {
        let mut wake_sender = self.wake_sender.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = wake_sender.try_send(());
    }
}

impl PresentationReceiver {
    pub(crate) async fn next(&mut self) -> Option<PresentationSnapshot> {
        self.wake_receiver.next().await?;
        Some(self.take_snapshot())
    }

    fn take_snapshot(&mut self) -> PresentationSnapshot {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            mem::take(&mut *state)
        };
        PresentationSnapshot::from_pending(self.generation, pending)
    }
}

impl PendingPresentation {
    fn push_chat(&mut self, message: ChatMessage) {
        let message_bytes = message.from_name.len().saturating_add(message.message.len());
        if message_bytes > MAX_PENDING_CHAT_BYTES {
            return;
        }

        while self.chat_messages.len() >= MAX_PENDING_CHAT_MESSAGES
            || self.chat_bytes.saturating_add(message_bytes) > MAX_PENDING_CHAT_BYTES
        {
            let Some(removed) = self.chat_messages.pop_front() else {
                break;
            };
            self.chat_bytes = self.chat_bytes.saturating_sub(removed.from_name.len().saturating_add(removed.message.len()));
        }

        self.chat_bytes += message_bytes;
        self.chat_messages.push_back(message);
    }
}

impl PresentationSnapshot {
    fn from_pending(generation: u64, pending: PendingPresentation) -> Self {
        let PendingPresentation {
            authentication, session_ready, region_name, chat_messages, failure, favorites, favorite_store_saved, ..
        } = pending;
        Self { generation, authentication, session_ready, region_name, chat_messages, failure, favorites, favorite_store_saved }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.authentication.is_none()
            && self.session_ready.is_none()
            && self.region_name.is_none()
            && self.chat_messages.is_empty()
            && self.failure.is_none()
            && self.favorites.is_none()
            && self.favorite_store_saved.is_none()
    }
}

pub(crate) async fn drain_session(
    mut session: Session,
    session_handle: SessionHandle,
    runtime: Handle,
    mailbox: PresentationMailbox,
    cancellation: oneshot::Receiver<()>,
    grid: Grid,
    username: String,
    password: String,
    remember_login: bool,
    credential_write_lock: WriteLock,
) {
    use Event::{
        CapabilityError, CapabilityEvent, CapabilityEventBatch, CaptureFinished, CaptureInterrupted, CoverageLost, EventQueueCoverageLost,
        Failed, LoginAccepted, MfaRequired, SendFailed, SessionReady, SimulatorCapabilities, SimulatorConnectFailed, SimulatorConnected,
        SimulatorDisconnected, SimulatorFailed, SimulatorMessage, SimulatorPromoted,
    };

    let cancellation = async move {
        let _ = cancellation.await;
    }
    .shared();
    let world_limits = viewer_world_limits();
    let mut session_world = SessionWorld::new(world_limits);
    let mut reads = WorldReadHost::new(session_world.world(), runtime.clone());
    let mut terrain = TerrainDecodeQueue::default();
    let mut environments = EnvironmentIntake::new(runtime.clone());
    let (environment_pushes, environment_inbox) = EnvironmentInbox::channel();
    let assets = AssetCoordinator::new(runtime.clone(), cancellation.clone());
    let mut terrain_received = false;
    let mut primary_simulator_id = None;
    let mut world_primary_simulator_id = None;
    let mut promoted_world_target = None;
    let mut handoff_observations = HashMap::new();
    let mut active_session: Option<AgentSession> = None;
    let mut active_login_data = None;
    let (camera, observations) = watch::channel(None::<Camera>);
    let mut last_camera = None;
    let mut camera_sent: Option<Instant> = None;
    let mut region_lookup = None;
    let query_state_maintenance_started = tokio::time::Instant::now().into_std();
    let mut query_state_maintenance = futures::future::ready(()).boxed().fuse();
    let mut media = ObjectMediaIntake::new(ObjectMediaFetcher::new(runtime.clone(), cancellation.clone()));
    let recovery_queue = Arc::new(Mutex::new(RecoveryScheduler::new()));
    let (mut recovery_wake, recovery_wake_receiver) = mpsc::channel(1);
    drop(runtime.clone().spawn(recovery::run_recovery_retries(
        recovery_wake_receiver,
        Arc::clone(&recovery_queue),
        session_handle.clone(),
        cancellation.clone(),
    )));

    loop {
        if cancellation.clone().now_or_never().is_some() {
            break;
        }
        reads.publish(session_world.world());
        media.flush(&session_world);
        let terrain_full = terrain.is_full();
        let event = async { if terrain_full { pending().await } else { session.recv_event().await } };
        let input = select_drain_input(event, media.next_result(), cancellation.clone(), &mut query_state_maintenance);
        let input = match reads.select(terrain.select(environments.select(input))).await {
            Ok(Err(decoded)) => {
                let applied = session_world.apply_terrain(decoded);
                if !terrain_received && applied > 0 {
                    terrain_received = true;
                    tracing::info!(patches = applied, "initial terrain patches received");
                }
                continue;
            }
            Ok(Ok(Ok(input))) => input,
            Ok(Ok(Err(completion))) => {
                if let Some(completion) = completion {
                    environments.complete(&mut session_world, *completion);
                }
                continue;
            }
            Err(request) => {
                request.serve(session_world.world());
                continue;
            }
        };
        let event = match input {
            DrainInput::Cancelled => break,
            DrainInput::QueryStateMaintenance => {
                if let (Some(simulator), Some(session), Some(view)) = (primary_simulator_id, &active_session, *observations.borrow())
                    && let Some(origin) = session_world
                        .region_for(simulator)
                        .and_then(|region| session_world.world().region(region))
                        .and_then(|info| info.resolved_origin())
                {
                    let state = (simulator, origin, view);
                    let interval = if last_camera == Some(state) { CAMERA_HEARTBEAT_INTERVAL } else { CAMERA_UPDATE_INTERVAL };
                    let now = tokio::time::Instant::now().into_std();
                    if camera_sent.is_none_or(|sent| now.saturating_duration_since(sent) >= interval)
                        && let Some(update) = agent::camera(session.agent_id, session.session_id, view, origin)
                    {
                        camera_sent = Some(now);
                        match session_handle.send(update, Delivery::Unreliable) {
                            Ok(()) => {
                                if last_camera.is_none_or(|(previous, _, camera): (SimulatorId, _, Camera)| {
                                    previous != simulator || camera.draw_distance() != view.draw_distance()
                                }) {
                                    tracing::debug!(draw_distance = view.draw_distance(), "simulator viewing distance updated");
                                }
                                last_camera = Some(state);
                            }
                            Err(error) => {
                                last_camera = None;
                                tracing::debug!(%error, "camera update deferred");
                            }
                        }
                    }
                }
                let has_dirty_pressure = session_world.maintain_query_state();
                let elapsed = tokio::time::Instant::now().into_std().duration_since(query_state_maintenance_started);
                query_state_maintenance =
                    tokio::time::sleep(next_query_state_maintenance_delay(elapsed, has_dirty_pressure)).boxed().fuse();
                continue;
            }
            DrainInput::Event(event) => event,
            DrainInput::Media(Some(result)) => {
                let retry = media.finish(&result);
                let requested_version = retry.map_or(result.target.version, |target| target.version);
                let response_version = result.update.as_ref().as_ref().ok().map(|update| update.version());
                if response_version.is_some_and(|version| response_is_stale(version, requested_version)) {
                    tracing::debug!(
                        simulator_id = ?result.target.simulator_id,
                        response_version,
                        requested_version,
                        "discarded superseded ObjectMedia response"
                    );
                } else {
                    match result.update {
                        Ok(update) => {
                            let address = result.target.binding.address();
                            if let Some(applied) = session_world.apply_object_media(result.target, update) {
                                let mut summary = IngestSummary::new(IngestStatus::Applied);
                                match applied {
                                    Ok(applied) => {
                                        if !applied.rejections.is_empty() {
                                            tracing::debug!(rejections = ?applied.rejections, "ObjectMedia update contained rejected fields");
                                        }
                                        summary.push_applied(applied, Some(address));
                                    }
                                    Err(error) => {
                                        tracing::debug!(?error, "ObjectMedia update was rejected by World");
                                        summary.push_error(error, Some(address));
                                    }
                                }
                                summary.log_incomplete_data(result.target.simulator_id);
                            }
                        }
                        Err(error) if session_world.object_media_target_is_current(result.target) => {
                            tracing::warn!(
                                simulator_id = ?result.target.simulator_id,
                                object_id = %result.target.binding.object_id(),
                                local_id = result.target.binding.address().local_id().get(),
                                failure = ?error,
                                "ObjectMedia could not be loaded"
                            );
                        }
                        Err(error) => {
                            tracing::debug!(simulator_id = ?result.target.simulator_id, failure = ?error, "discarded ObjectMedia failure for a stale object target");
                        }
                    }
                }
                if let Some(target) = retry {
                    media.queue(target, &session_world);
                }
                continue;
            }
            DrainInput::Media(None) => {
                if cancellation.clone().now_or_never().is_some() {
                    break;
                }
                tracing::warn!("ObjectMedia worker stopped; media fetching is disabled for this session");
                media.disable();
                continue;
            }
        };
        let Some(event) = event else {
            mailbox.publish(ViewerUpdate::Failed("The network session ended.".into()));
            break;
        };

        if let SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } = &event
            && let Some(region) = session_world.region_for(*simulator_id)
        {
            environments.observe(&event, &mut session_world, primary_simulator_id);
            assets.retire(region);
        }

        if let SimulatorMessage { simulator_id, message, packet_order: PacketOrder::Ordered(_), .. } = &event
            && Some(*simulator_id) == primary_simulator_id
            && let (Some(agent), Some(region)) = (active_session.as_ref(), session_world.region_for(*simulator_id))
        {
            match experience_environment::decode(message, agent.agent_id) {
                Ok(Some(push)) => match environment_pushes.try_send(EnvironmentCommand { region, push }) {
                    Ok(()) => {}
                    Err(error) => tracing::warn!(%error, "experience environment push could not be admitted"),
                },
                Ok(None) => {}
                Err(error) => tracing::warn!(?error, "invalid experience environment push"),
            }
        }

        let media_requests = match &event {
            SimulatorMessage { simulator_id, sequence, packet_order, message, .. } => session_world
                .region_for(*simulator_id)
                .map(|region| object_media_requests(region, *sequence, order_evidence(*packet_order), message))
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        let event = match event {
            CoverageLost { loss } => {
                match loss {
                    CoverageLoss::Source(record) => {
                        let source = match record.watermark.source {
                            CoverageSource::Lludp => "lludp",
                            CoverageSource::EventQueue { .. } => "event_queue",
                        };
                        tracing::warn!(source, reason = ?record.reason, "World data may be incomplete");
                        session.acknowledge_coverage_loss(record.watermark);
                    }
                    CoverageLoss::SessionOverflow { generation } => {
                        tracing::warn!(generation, "World event queue overflowed; World data may be incomplete");
                        session.acknowledge_coverage_overflow(generation);
                    }
                };
                continue;
            }
            event => event,
        };

        if let SimulatorMessage { simulator_id, sequence, packet_order, message, .. } = &event
            && let Some(region) = session_world.region_for(*simulator_id)
        {
            terrain.enqueue(message, UpdateSource::new(region, *sequence, order_evidence(*packet_order), 0), &runtime);
        }

        let mut process_object_chunk = |_simulator_id,
                                        _summary: &IngestSummary,
                                        recovery_targets: Vec<(Option<SimulatorId>, LocalAddress)>,
                                        updated: Vec<LocalAddress>| {
            recovery::schedule_recovery_requests(recovery_targets, updated, active_session.as_ref(), &recovery_queue, &mut recovery_wake);
        };
        if let Some((simulator_id, summary)) = apply_world_event_cooperatively(
            &mut session_world,
            &event,
            &mut world_primary_simulator_id,
            &mut promoted_world_target,
            &mut handoff_observations,
            active_session.as_ref().map(|session: &AgentSession| session.agent_id),
            &mut process_object_chunk,
        )
        .await
        {
            summary.log_incomplete_data(simulator_id);
            recovery::schedule_recovery_requests(
                route_recovery_targets(&session_world, summary.recovery_targets()),
                summary.recovery_cancellation_addresses().collect(),
                active_session.as_ref(),
                &recovery_queue,
                &mut recovery_wake,
            );
        }
        if let SimulatorPromoted { simulator_id, .. } = &event
            && let Some(region) = session_world.region_for(*simulator_id)
        {
            let push = scene::EnvironmentPush::Clear { experience: None, transition: Duration::ZERO };
            if let Err(error) = environment_pushes.try_send(EnvironmentCommand { region, push }) {
                tracing::error!(%error, "experience environment crossing reset could not be admitted");
                mailbox.publish(ViewerUpdate::Failed("The environment update queue is unavailable.".into()));
                break;
            }
        }
        if !matches!(event, SimulatorDisconnected { .. } | SimulatorFailed { .. }) {
            environments.observe(&event, &mut session_world, primary_simulator_id);
        }
        if let SimulatorMessage { simulator_id, .. } = &event {
            for request in media_requests {
                if let Some(target) = session_world.prepare_object_media(*simulator_id, request) {
                    media.queue(target, &session_world);
                }
            }
        }

        if let SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } = &event {
            recovery_queue.lock().unwrap_or_else(PoisonError::into_inner).cancel_simulator(*simulator_id);
        }

        match event {
            MfaRequired { challenge } => {
                mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::MfaRequired(challenge)));
            }
            LoginAccepted { mfa_hash } => {
                let mailbox = mailbox.clone();
                let write_lock = credential_write_lock.clone();
                let username = username.clone();
                let password = password.clone();
                drop(runtime.spawn_blocking(move || {
                    let _guard = block_on(write_lock.lock());
                    let mut warning = None;
                    let remembered_login = if remember_login {
                        let saved = RememberedLogin { username, password, mfa_hash: mfa_hash.unwrap_or_default() };
                        match remembered_login::save(grid, &saved) {
                            Ok(()) => Some(saved),
                            Err(error) => {
                                warning = Some(error);
                                None
                            }
                        }
                    } else {
                        if let Err(error) = remembered_login::clear(grid) {
                            warning = Some(error);
                        }
                        None
                    };
                    mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::CredentialsUpdated { remembered_login, warning }));
                }));
            }
            SessionReady { simulator_id, session, capabilities, login_data } => {
                if let Some(region) = session_world.region_for(simulator_id) {
                    assets.register(region, capabilities.clone());
                }
                let session = *session;
                media.set_client(simulator_id, capabilities.clone());
                primary_simulator_id = Some(simulator_id);
                active_session = Some(session.clone());
                active_login_data = Some(login_data.clone());
                let lookup = RegionLookup::new(session_handle.clone(), simulator_id, &session, runtime.clone());
                region_lookup = Some(lookup.clone());
                mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
                    environments: environment_inbox.clone(),
                    camera: camera.clone(),
                    assets: assets.handle(),
                    session: session.clone(),
                    region_lookup: lookup.clone(),
                    region_name: session_world.region_name(simulator_id).map(str::to_owned),
                    world_reader: reads.reader(),
                }));

                let mailbox = mailbox.clone();
                let username = username.clone();
                let runtime = runtime.clone();
                let cancelled = cancellation.clone();
                drop(runtime.spawn(async move {
                    let load = load_favorites(&capabilities, &lookup, &session, &login_data).fuse();
                    pin_mut!(load);
                    let cancelled = cancelled.fuse();
                    pin_mut!(cancelled);
                    futures::select! {
                        result = load => mailbox.publish(ViewerUpdate::Favorites(FavoritesUpdate {
                            grid: session.grid,
                            agent_id: session.agent_id,
                            session_id: session.session_id,
                            username,
                            result,
                        })),
                        _ = cancelled => {}
                    }
                }));
            }
            SimulatorPromoted { previous_simulator_id, simulator_id, session, capabilities } => {
                let session = *session;
                media.set_client(simulator_id, capabilities.clone());
                primary_simulator_id = Some(simulator_id);
                active_session = Some(session.clone());
                let lookup = RegionLookup::new(session_handle.clone(), simulator_id, &session, runtime.clone());
                region_lookup = Some(lookup.clone());
                let previous_target = promoted_world_target.map(|handoff| handoff.target);
                if begin_world_handoff(&mut world_primary_simulator_id, &mut promoted_world_target, previous_simulator_id, simulator_id) {
                    if let Some(previous_target) = previous_target.filter(|target| *target != simulator_id) {
                        handoff_observations.remove(&previous_target);
                    }
                    if let Some(observation) = handoff_observations.get(&simulator_id).cloned() {
                        let (completed, summary) = complete_promoted_handoff(
                            &mut session_world,
                            &mut world_primary_simulator_id,
                            &mut promoted_world_target,
                            simulator_id,
                            observation.clone(),
                        );
                        if completed {
                            handoff_observations.remove(&simulator_id);
                        }
                        if let Some(summary) = summary {
                            recovery::schedule_recovery_requests(
                                route_recovery_targets(&session_world, summary.recovery_targets()),
                                summary.recovery_cancellation_addresses().collect(),
                                active_session.as_ref(),
                                &recovery_queue,
                                &mut recovery_wake,
                            );
                        }
                    }
                }
                let favorites_session = session.clone();
                let favorites_lookup = lookup.clone();
                mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
                    environments: environment_inbox.clone(),
                    camera: camera.clone(),
                    assets: assets.handle(),
                    session: session.clone(),
                    region_lookup: lookup,
                    region_name: session_world.region_name(simulator_id).map(str::to_owned),
                    world_reader: reads.reader(),
                }));
                if let Some(login_data) = active_login_data.clone() {
                    let mailbox = mailbox.clone();
                    let username = username.clone();
                    let cancelled = cancellation.clone();
                    drop(runtime.clone().spawn(async move {
                        let load = load_favorites(&capabilities, &favorites_lookup, &favorites_session, &login_data).fuse();
                        pin_mut!(load);
                        let cancelled = cancelled.fuse();
                        pin_mut!(cancelled);
                        futures::select! {
                            result = load => mailbox.publish(ViewerUpdate::Favorites(FavoritesUpdate {
                                grid: favorites_session.grid,
                                agent_id: favorites_session.agent_id,
                                session_id: favorites_session.session_id,
                                username,
                                result,
                            })),
                            _ = cancelled => {}
                        }
                    }));
                }
            }
            SimulatorConnected { simulator_id, region_handle, .. } => {
                if region_handle.is_none() {
                    primary_simulator_id = Some(simulator_id);
                }
            }
            SimulatorMessage { simulator_id, source, message, .. } => {
                if Some(simulator_id) == primary_simulator_id
                    && observations.borrow().is_none()
                    && let Message::AgentMovementComplete(movement) = &message
                    && let Some(session) = &active_session
                    && let Some(update) = agent::arrival(session.agent_id, session.session_id, movement)
                    && let Err(error) = session_handle.send(update, Delivery::Reliable)
                {
                    tracing::debug!(%error, "arrival camera update deferred");
                }
                let scope = if Some(simulator_id) == primary_simulator_id { SimulatorScope::Primary } else { SimulatorScope::Neighbor };
                let region_name = session_world.region_name(simulator_id);
                handle_simulator_message(scope, source, message, active_session.as_ref(), region_lookup.as_ref(), region_name, &mailbox);
            }
            CaptureFinished { .. } => {}
            CaptureInterrupted { error, .. } => {
                tracing::warn!(operation = "capture_stop", outcome = "interrupted", error = ?error, "Capture interrupted")
            }
            SendFailed(error) => tracing::warn!(operation = "simulator_send", outcome = "failed", %error, "Send failed"),
            EventQueueCoverageLost { reason, .. } => tracing::warn!(
                operation = "event_queue",
                outcome = "coverage_lost",
                reason = ?reason,
                "Simulator event delivery may have a gap; world data may be incomplete"
            ),
            Failed(error) => {
                mailbox.publish(ViewerUpdate::Failed(error));
                break;
            }
            SimulatorDisconnected { simulator_id, .. } => {
                media.forget_simulator(simulator_id);
                if Some(simulator_id) == primary_simulator_id {
                    primary_simulator_id = None;
                }
            }
            SimulatorFailed { simulator_id, .. } => {
                media.forget_simulator(simulator_id);
                if Some(simulator_id) == primary_simulator_id {
                    primary_simulator_id = None;
                }
            }
            CapabilityEvent { .. } | CapabilityEventBatch { .. } | CapabilityError { .. } | SimulatorConnectFailed { .. } => {}
            SimulatorCapabilities { simulator_id, capabilities, .. } => {
                if let Some(region) = session_world.region_for(simulator_id) {
                    assets.register(region, capabilities.clone());
                }
                media.set_client(simulator_id, capabilities);
            }
            CoverageLost { .. } => unreachable!("coverage loss events are reconciled before world intake"),
        }
    }

    reads.finish();
    drop(environments);
    drop(terrain);
    assets.stop().await;
    for (_, error) in session_world.retire_all() {
        tracing::warn!(?error, "could not retire simulator world region on session shutdown");
    }
}

fn route_recovery_targets(world: &SessionWorld, addresses: impl Iterator<Item = LocalAddress>) -> Vec<(Option<SimulatorId>, LocalAddress)> {
    addresses.map(|address| (world.simulator_for_region(address.region()), address)).collect()
}

enum DecodedWorldUpdate {
    Object(ObjectBatch),
    AvatarAppearance { source: UpdateSource, update: AvatarAppearanceMessage },
    SitTarget { source: UpdateSource, update: SitTargetUpdate },
    Malformed { invalid_blocks: usize, invalid_fields: usize },
}

fn decode_simulator_world_update(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    sequence: u32,
    packet_order: PacketOrder,
    message: &Message,
) -> Option<DecodedWorldUpdate> {
    let region = world.region_for(simulator_id)?;
    if let Message::RegionHandshake(handshake) = message {
        if let Err(error) = world
            .update_water_height(UpdateSource::new(region, sequence, order_evidence(packet_order), 0), handshake.region_info1.water_height)
        {
            tracing::debug!(?error, "could not update region water height");
        }
        let region_id = handshake.region_info2.as_ref().map(|info| info.region_id).filter(|id| !id.is_nil());
        if let Err(error) = world.update_handshake_metadata(
            UpdateSource::new(region, sequence, order_evidence(packet_order), 0),
            region_id,
            handshake.region_info1.sim_name.clone(),
        ) {
            tracing::warn!(?error, "could not update region handshake metadata");
        }
    }

    if let Some(update) = decode_sit_target_update(message) {
        let source = UpdateSource::new(region, sequence, order_evidence(packet_order), 0);
        return Some(DecodedWorldUpdate::SitTarget { source, update });
    }
    match decode_avatar_appearance_update(message) {
        Ok(Some(update)) => {
            let source = UpdateSource::new(region, sequence, order_evidence(packet_order), 0);
            return Some(DecodedWorldUpdate::AvatarAppearance { source, update });
        }
        Ok(None) => {}
        Err(_) => return Some(DecodedWorldUpdate::Malformed { invalid_blocks: 1, invalid_fields: 0 }),
    }
    decode_object_message(region, sequence, order_evidence(packet_order), message).map(DecodedWorldUpdate::Object)
}

fn apply_world_event_with_handoff(
    world: &mut SessionWorld,
    event: &Event,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    agent_id: Option<Uuid>,
) -> Option<(SimulatorId, IngestSummary)> {
    use Event::{SessionReady, SimulatorConnected, SimulatorDisconnected, SimulatorFailed, SimulatorMessage};

    match event {
        SessionReady { simulator_id, session, .. } => {
            world.set_self_agent(session.agent_id);
            *world_primary = Some(*simulator_id);
            match world.register_simulator(*simulator_id, None) {
                Ok(_) => {}
                Err(RegistrationError::HandleChanged) => {
                    tracing::error!("primary simulator handle changed during one connection lifetime");
                }
                Err(RegistrationError::Apply(error)) => tracing::error!(?error, "could not register primary world region"),
            }
            None
        }
        SimulatorConnected { simulator_id, region_handle, .. } => {
            let handle = region_handle.map(RegionHandle::new);
            match world.register_simulator(*simulator_id, handle) {
                Ok(_) => {}
                Err(RegistrationError::HandleChanged) => {
                    tracing::warn!("simulator handle changed during one connection lifetime");
                }
                Err(RegistrationError::Apply(error)) => tracing::warn!(?error, "could not register simulator world region"),
            }
            None
        }
        SimulatorMessage { simulator_id, sequence, packet_order, message, .. } => {
            let update = decode_simulator_world_update(world, *simulator_id, *sequence, *packet_order, message)?;
            apply_decoded_world_update(world, *simulator_id, update, world_primary, promoted_target, handoff_observations, agent_id)
        }
        SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } => {
            if let Err(error) = world.retire_simulator(*simulator_id) {
                tracing::warn!(?error, "could not retire simulator world region");
            }
            handoff_observations.remove(simulator_id);
            if promoted_target.is_some_and(|handoff| handoff.target == *simulator_id) {
                *promoted_target = None;
            }
            None
        }
        _ => None,
    }
}

async fn apply_world_event_cooperatively(
    world: &mut SessionWorld,
    event: &Event,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    agent_id: Option<Uuid>,
    on_object_chunk: &mut impl FnMut(SimulatorId, &IngestSummary, Vec<(Option<SimulatorId>, LocalAddress)>, Vec<LocalAddress>),
) -> Option<(SimulatorId, IngestSummary)> {
    if let Event::SimulatorMessage { simulator_id, sequence, packet_order, message, .. } = event {
        let update = decode_simulator_world_update(world, *simulator_id, *sequence, *packet_order, message)?;
        return match update {
            DecodedWorldUpdate::Object(batch) => {
                let batch = filter_handoff_batch(world, *simulator_id, *world_primary, agent_id, batch, handoff_observations)?;
                let _summary = world
                    .apply_object_batch_cooperatively(*simulator_id, batch, |summary, targets, cancellations| {
                        on_object_chunk(*simulator_id, summary, targets, cancellations);
                    })
                    .await?;
                complete_handoff_observation(world, *simulator_id, world_primary, promoted_target, handoff_observations)
                    .map(|summary| (*simulator_id, summary))
            }
            update => {
                apply_decoded_world_update(world, *simulator_id, update, world_primary, promoted_target, handoff_observations, agent_id)
            }
        };
    }
    apply_world_event_with_handoff(world, event, world_primary, promoted_target, handoff_observations, agent_id)
}

fn apply_decoded_world_update(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    update: DecodedWorldUpdate,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    agent_id: Option<Uuid>,
) -> Option<(SimulatorId, IngestSummary)> {
    match update {
        DecodedWorldUpdate::Object(batch) => {
            let batch = filter_handoff_batch(world, simulator_id, *world_primary, agent_id, batch, handoff_observations)?;
            let summary = world.apply_object_batch(simulator_id, batch);
            complete_batch_handoff(world, simulator_id, world_primary, promoted_target, handoff_observations, summary)
        }
        DecodedWorldUpdate::SitTarget { source, update } => {
            let address = world.object_address(simulator_id, update.object_id);
            let result = world.apply_sit_target_update(simulator_id, source, update);
            match result {
                Some(Ok(applied)) => {
                    if !applied.rejections.is_empty() {
                        tracing::debug!(rejections = ?applied.rejections, "avatar sit response contained rejected metadata");
                    }
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.push_applied(applied, address);
                    Some((simulator_id, summary))
                }
                Some(Err(error)) => {
                    tracing::debug!(?error, "could not apply avatar sit target");
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.push_error(error, address);
                    Some((simulator_id, summary))
                }
                None => None,
            }
        }
        DecodedWorldUpdate::AvatarAppearance { source, update, .. } => {
            let object_id = update.object_id;
            let invalid_fields = update.invalid_fields;
            match world.apply_avatar_appearance(simulator_id, source, update) {
                Some(Ok(applied)) => {
                    if !applied.rejections.is_empty() {
                        tracing::debug!(rejections = ?applied.rejections, "avatar appearance contained rejected fields");
                    }
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.invalid_fields = invalid_fields;
                    summary.push_applied(applied, world.object_address(simulator_id, object_id));
                    Some((simulator_id, summary))
                }
                Some(Err(error)) => {
                    tracing::debug!(?error, "could not apply avatar appearance");
                    let mut summary = IngestSummary::new(IngestStatus::Applied);
                    summary.invalid_fields = invalid_fields;
                    summary.push_error(error, world.object_address(simulator_id, object_id));
                    Some((simulator_id, summary))
                }
                None => None,
            }
        }
        DecodedWorldUpdate::Malformed { invalid_blocks, invalid_fields } => {
            let mut summary = IngestSummary::new(IngestStatus::Applied);
            summary.invalid_blocks = invalid_blocks;
            summary.invalid_fields = invalid_fields;
            Some((simulator_id, summary))
        }
    }
}

fn filter_handoff_batch(
    world: &SessionWorld,
    simulator_id: SimulatorId,
    world_primary: Option<SimulatorId>,
    agent_id: Option<Uuid>,
    mut batch: ObjectBatch,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
) -> Option<ObjectBatch> {
    if world_primary != Some(simulator_id)
        && let Some(agent_id) = agent_id
    {
        let agent_region = world.region_for(simulator_id)?;
        let mut retained = Vec::with_capacity(batch.blocks.len());
        for operation in batch.blocks.drain(..) {
            match operation {
                Some(ObjectOperation::Observe(observation)) if observation.id == agent_id => {
                    let replace = handoff_observations
                        .get(&simulator_id)
                        .is_none_or(|current| should_replace_handoff_observation(current, &observation));
                    if replace {
                        handoff_observations.insert(simulator_id, observation);
                    }
                }
                Some(operation) => match handoff_observations.get(&simulator_id) {
                    Some(observation)
                        if operation.address() == LocalAddress::new(agent_region, observation.local_id)
                            && conflicts_with_handoff_observation(&operation, observation) =>
                    {
                        if operation_is_newer(operation.source(), observation.source) {
                            handoff_observations.remove(&simulator_id);
                        }
                        retained.push(Some(operation));
                    }
                    Some(observation) if operation.address() == LocalAddress::new(agent_region, observation.local_id) => {}
                    _ => retained.push(Some(operation)),
                },
                operation => retained.push(operation),
            }
        }
        batch.blocks = retained;
        while handoff_observations.len() > MAX_HANDOFF_OBSERVATIONS {
            if let Some(stale) = handoff_observations.keys().next().copied() {
                handoff_observations.remove(&stale);
            }
        }
    }
    Some(batch)
}

fn complete_batch_handoff(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
    summary: Option<IngestSummary>,
) -> Option<(SimulatorId, IngestSummary)> {
    let mut summary = summary.map(|summary| (simulator_id, summary));
    if let Some(extra) = complete_handoff_observation(world, simulator_id, world_primary, promoted_target, handoff_observations) {
        let summary = summary.get_or_insert_with(|| (simulator_id, IngestSummary::new(IngestStatus::Applied)));
        summary.1.append(extra);
    }
    summary
}

fn complete_handoff_observation(
    world: &mut SessionWorld,
    simulator_id: SimulatorId,
    world_primary: &mut Option<SimulatorId>,
    promoted_target: &mut Option<PendingWorldHandoff>,
    handoff_observations: &mut HashMap<SimulatorId, ObjectObservation>,
) -> Option<IngestSummary> {
    if let Some(observation) = handoff_observations.get(&simulator_id).cloned() {
        let (completed, applied) =
            complete_promoted_world_handoff(world, world_primary, promoted_target, simulator_id, observation.clone());
        if completed {
            handoff_observations.remove(&simulator_id);
        }
        if let Some(applied) = applied {
            let mut summary = IngestSummary::new(IngestStatus::Applied);
            summary.push_applied(applied, Some(LocalAddress::new(observation.source.region(), observation.local_id)));
            return Some(summary);
        }
    }
    None
}

fn conflicts_with_handoff_observation(operation: &ObjectOperation, observation: &ObjectObservation) -> bool {
    match operation {
        ObjectOperation::Observe(candidate) => candidate.id != observation.id,
        ObjectOperation::Remove { .. } => true,
        ObjectOperation::Patch(_) | ObjectOperation::RenderMaterialsPatch(_) | ObjectOperation::Cached { .. } => false,
    }
}

fn operation_is_newer(candidate: UpdateSource, current: UpdateSource) -> bool {
    match (candidate.order(), current.order()) {
        (Ordered(candidate_order), Ordered(current_order)) => {
            (candidate_order, candidate.block_ordinal()) > (current_order, current.block_ordinal())
        }
        _ => false,
    }
}

#[cfg(test)]
fn apply_world_event(world: &mut SessionWorld, event: &Event) -> Option<(SimulatorId, IngestSummary)> {
    let mut world_primary = None;
    let mut promoted_target = None;
    let mut handoff_observations = HashMap::new();
    apply_world_event_with_handoff(world, event, &mut world_primary, &mut promoted_target, &mut handoff_observations, None)
}

fn should_replace_handoff_observation(current: &ObjectObservation, candidate: &ObjectObservation) -> bool {
    if current.source.region() != candidate.source.region() {
        return true;
    }
    match (current.source.order(), candidate.source.order()) {
        (OrderEvidence::Ordered(current_order), OrderEvidence::Ordered(candidate_order)) => {
            (candidate_order, candidate.source.block_ordinal()) > (current_order, current.source.block_ordinal())
        }
        (OrderEvidence::Ambiguous | OrderEvidence::Degraded, OrderEvidence::Ordered(_)) => true,
        (OrderEvidence::Ordered(_), OrderEvidence::Ambiguous | OrderEvidence::Degraded)
        | (OrderEvidence::Ambiguous | OrderEvidence::Degraded, OrderEvidence::Ambiguous | OrderEvidence::Degraded) => false,
    }
}

fn begin_world_handoff(
    world_primary: &mut Option<SimulatorId>,
    pending: &mut Option<PendingWorldHandoff>,
    source: SimulatorId,
    target: SimulatorId,
) -> bool {
    if source == target {
        return false;
    }
    if let Some(current) = *pending
        && current.target == source
        && *world_primary == Some(current.source)
    {
        *pending = Some(PendingWorldHandoff { source: current.source, target });
        return true;
    }
    if *world_primary != Some(source) {
        return false;
    }
    *pending = Some(PendingWorldHandoff { source, target });
    true
}

fn complete_promoted_world_handoff(
    world: &mut SessionWorld,
    world_primary: &mut Option<SimulatorId>,
    pending: &mut Option<PendingWorldHandoff>,
    target: SimulatorId,
    observation: ObjectObservation,
) -> (bool, Option<Applied>) {
    let Some(handoff) = *pending else { return (false, None) };
    if handoff.target != target || *world_primary != Some(handoff.source) {
        return (false, None);
    }
    let (completed, applied) = world.complete_agent_handoff(handoff.source, target, observation);
    if !completed {
        return (false, applied);
    }
    *world_primary = Some(target);
    *pending = None;
    (true, applied)
}

fn complete_promoted_handoff(
    world: &mut SessionWorld,
    world_primary: &mut Option<SimulatorId>,
    pending: &mut Option<PendingWorldHandoff>,
    target: SimulatorId,
    observation: ObjectObservation,
) -> (bool, Option<IngestSummary>) {
    let address = LocalAddress::new(observation.source.region(), observation.local_id);
    let (completed, applied) = complete_promoted_world_handoff(world, world_primary, pending, target, observation);
    let summary = applied.map(|applied| {
        let mut summary = IngestSummary::new(IngestStatus::Applied);
        summary.push_applied(applied, Some(address));
        summary.log_incomplete_data(target);
        summary
    });
    (completed, summary)
}

fn order_evidence(packet_order: PacketOrder) -> OrderEvidence {
    match packet_order {
        PacketOrder::Ordered(order) => OrderEvidence::Ordered(order),
        PacketOrder::Ambiguous => OrderEvidence::Ambiguous,
        PacketOrder::Degraded => OrderEvidence::Degraded,
    }
}

fn handle_simulator_message(
    scope: SimulatorScope,
    source: SocketAddr,
    message: Message,
    session: Option<&AgentSession>,
    region_lookup: Option<&RegionLookup>,
    region_name: Option<&str>,
    mailbox: &PresentationMailbox,
) {
    use Message::{ChatFromSimulator, RegionHandshake};

    if matches!(scope, SimulatorScope::Primary) {
        if let Some(region_lookup) = region_lookup {
            region_lookup.handle_message(source, &message);
        }
        if matches!(&message, RegionHandshake(_))
            && let Some(name) = region_name
        {
            mailbox.publish(ViewerUpdate::RegionName(name.to_owned()));
        }
    }
    if let ChatFromSimulator(message) = message
        && let Some(chat_message) = display_message(message.chat_data, session)
    {
        mailbox.publish(ViewerUpdate::ChatMessage(chat_message));
    }
}

pub(crate) fn write_lock() -> WriteLock {
    Arc::new(futures::lock::Mutex::new(()))
}

pub(crate) async fn save_favorite_store(
    store: favorite_store::FavoriteStore,
    lock: WriteLock,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
) -> Option<Result<(), String>> {
    save_if_current(store, lock, generation, expected_generation, |store| favorite_store::save(&store)).await
}

async fn save_if_current<T>(
    value: T,
    lock: WriteLock,
    generation: Arc<AtomicU64>,
    expected_generation: u64,
    save: impl FnOnce(T) -> Result<(), String>,
) -> Option<Result<(), String>> {
    let _guard = lock.lock().await;
    (generation.load(Ordering::Acquire) == expected_generation).then(|| save(value))
}

#[cfg(test)]
fn fixture_packet(fixture: &str) -> protocol::lludp::Packet {
    let bytes = test_support::parse_hex_fixture(fixture).expect("valid LLUDP fixture");
    protocol::lludp::Packet::decode(&bytes).expect("decode LLUDP fixture")
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        future::{pending, ready},
        num::NonZeroU32,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
        time::Duration,
    };
    use tokio::runtime::Handle;

    use ::world::{
        Appearance,
        ApplyOutcome::{Deferred, Introduced, TombstoneRecorded, Unchanged},
        DeferredReason::AmbiguousOrder,
        FieldUpdate, Kind, LocalAddress, ObjectObservation, OrderEvidence, Parent, ParentUpdate, Pose, Region, RegionHandle, UpdateSource,
    };
    use futures::{FutureExt, StreamExt, channel::oneshot, executor::block_on, stream};
    use math::{Quat, Vec3};
    use network::{
        Event,
        Event::{SimulatorConnected, SimulatorDisconnected, SimulatorMessage},
        Grid, PacketOrder,
        PacketOrder::{Ambiguous, Ordered},
        SimulatorId,
        testing::simulator_id,
    };
    use protocol::{
        Audible, ChatSourceType, ChatType,
        lludp::Message,
        lludp::Message::{ImprovedTerseObjectUpdate, KillObject, ObjectUpdate, ObjectUpdateCached},
        messages::{
            CacheMissType::Full,
            ChatFromSimulator, RequestAgentData,
            chat_from_simulator::ChatData,
            kill_object,
            region_handshake::{RegionHandshake, RegionInfo1},
        },
    };
    use tempfile::tempdir;
    use test_support::lludp::vectors;
    use uuid::Uuid;

    use crate::inventory::{FavoriteLocation, favorite_store::FavoriteStore};

    use crate::session::{
        ChatMessage,
        recovery::recovery_request,
        world::{IngestStatus, SessionWorld, viewer_world_limits},
    };

    use super::{
        MAX_PENDING_CHAT_BYTES, MAX_PENDING_CHAT_MESSAGES, ObjectMediaIntake, ObjectMediaResult, PresentationMailbox, SimulatorScope,
        ViewerUpdate, WaitingObjectMedia, apply_world_event, apply_world_event_with_handoff, begin_world_handoff,
        complete_promoted_handoff, complete_promoted_world_handoff, fixture_packet, handle_simulator_message, save_if_current,
        select_drain_input, write_lock,
    };
    use crate::session::{
        inbound::{ObjectBatch, ObjectOperation},
        object_media::{FetchError, ObjectMediaFetcher, ObjectMediaTarget},
    };

    fn region_handshake(name: &str) -> Message {
        Message::RegionHandshake(RegionHandshake {
            region_info1: RegionInfo1 { sim_name: name.into(), ..RegionInfo1::default() },
            region_info2: None,
            region_info3: None,
            region_info4: Vec::new(),
        })
    }

    #[tokio::test]
    async fn floods_preserve_reads_and_intake() {
        use crate::world_reader::{MAX_INTAKE_TURNS, WorldReadHost};
        use futures::poll;
        use std::{collections::VecDeque, time::Instant};

        const EVENTS: u32 = 512;
        let simulator = simulator_id();
        let events = (1..=EVENTS).map(|sequence| agent_update(Uuid::from_u128(u128::from(sequence)), sequence, 0)).collect::<Vec<_>>();
        let mut baseline = SessionWorld::new(viewer_world_limits());
        baseline.register_simulator(simulator, None).unwrap();
        let mut primary = Some(simulator);
        let mut promoted = None;
        let mut observations = HashMap::new();
        let started = Instant::now();
        for (index, message) in events.iter().enumerate() {
            let event = simulator_message(simulator, index as u32 + 1, message.clone());
            super::apply_world_event_cooperatively(
                &mut baseline,
                &event,
                &mut primary,
                &mut promoted,
                &mut observations,
                None,
                &mut |_, _, _, _| {},
            )
            .await;
        }
        let baseline_time = started.elapsed();

        let mut world = SessionWorld::new(viewer_world_limits());
        world.register_simulator(simulator, None).unwrap();
        let mut reads = WorldReadHost::new(world.world(), Handle::current());
        let reader = reads.reader();
        let mut queued_reads = VecDeque::new();
        for _ in 0..8 {
            let mut read = Box::pin(reader.objects(None, 256));
            assert!(poll!(read.as_mut()).is_pending());
            queued_reads.push_back((0, Instant::now(), read));
        }
        let mut completed = 0;
        let mut processed = 0;
        let mut since_read = 0;
        let mut latencies = Vec::new();
        let started = Instant::now();
        while processed < events.len() {
            reads.publish(world.world());
            let event = simulator_message(simulator, processed as u32 + 1, events[processed].clone());
            let input = super::select_drain_input(ready(Some(event)), pending(), pending(), pending());
            match reads.select(input).await {
                Ok(super::DrainInput::Event(Some(event))) => {
                    super::apply_world_event_cooperatively(
                        &mut world,
                        &event,
                        &mut primary,
                        &mut promoted,
                        &mut observations,
                        None,
                        &mut |_, _, _, _| {},
                    )
                    .await;
                    processed += 1;
                    since_read += 1;
                    assert!(since_read <= MAX_INTAKE_TURNS, "a ready read must receive its reserved turn");
                }
                Err(request) => {
                    assert_eq!(since_read, MAX_INTAKE_TURNS, "reader floods must not displace ready intake turns");
                    request.serve(world.world());
                    since_read = 0;
                    let (at_event, queued, read) = queued_reads.pop_front().unwrap();
                    let page = read.await.unwrap();
                    latencies.push(queued.elapsed());
                    assert!(processed - at_event <= 8 * MAX_INTAKE_TURNS, "each admitted FIFO request has bounded turn latency");
                    assert_eq!(page.hydration_revision, world.world().revision());
                    assert_eq!(page.objects.len(), processed.min(256));
                    assert_eq!(page.objects[0].id, Uuid::from_u128(1));
                    assert_eq!(page.objects.last().unwrap().id, Uuid::from_u128(processed.min(256) as u128));
                    assert_eq!(reader.revision().unwrap(), page.hydration_revision);
                    completed += 1;
                    let mut read = Box::pin(reader.objects(None, 256));
                    assert!(poll!(read.as_mut()).is_pending());
                    queued_reads.push_back((processed, Instant::now(), read));
                }
                _ => panic!("only simulator intake and reader requests are ready"),
            }
        }
        let flooded_time = started.elapsed();
        assert_eq!(completed, EVENTS as usize / MAX_INTAKE_TURNS - 1);
        assert_eq!(world.world().object_count(), EVENTS as usize);
        assert_eq!(world.world().object_count(), baseline.world().object_count());
        drop(queued_reads);
        reads.finish();
        latencies.sort();
        eprintln!(
            "World intake: {EVENTS} updates, baseline {baseline_time:?}, with {completed} reads {flooded_time:?}; read latency p50 {:?}, p95 {:?}, max {:?}",
            latencies[latencies.len() / 2],
            latencies[latencies.len() * 95 / 100],
            latencies.last().unwrap()
        );
    }

    fn object_media_target(world: &mut SessionWorld, simulator_id: SimulatorId, order: u32, version: u32) -> ObjectMediaTarget {
        let region = world.register_simulator(simulator_id, None).expect("register simulator");
        let local_id = NonZeroU32::new(7).expect("nonzero local ID");
        let source = UpdateSource::new(region, order, OrderEvidence::Ordered(u64::from(order)), 0);
        let observation = ObjectObservation {
            appearance: FieldUpdate::Set(Appearance::Surface),
            ..ObjectObservation::new(source, Uuid::from_u128(7), local_id, Kind::Object, ParentUpdate::Root)
        };
        let summary = world
            .apply_object_batch(
                simulator_id,
                ObjectBatch {
                    region,
                    region_handle: None,
                    blocks: vec![Some(ObjectOperation::Observe(observation))],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .expect("object batch applies");
        let result = summary.blocks[0].result.as_ref().expect("object result").as_ref().expect("object is valid");
        let Introduced(entity) = &result.outcome else { panic!("object is introduced") };
        let binding = world.world().object_binding_target(*entity).expect("current object binding");
        ObjectMediaTarget { simulator_id, binding, source, version }
    }

    #[tokio::test]
    async fn media_exit_preserves_intake() {
        let simulator = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let target = object_media_target(&mut world, simulator, 1, 1);
        let mut media = ObjectMediaIntake::new(ObjectMediaFetcher::new(Handle::current(), ready(())));
        media.queue(target, &world);
        assert!(matches!(&media, ObjectMediaIntake::Enabled { waiting, .. } if waiting.len() == 1));
        assert!(media.next_result().await.is_none());

        media.disable();
        media.queue(target, &world);
        assert!(matches!(media, ObjectMediaIntake::Disabled), "disabled fetching releases queued work and rejects new work");
        let event = simulator_message(
            simulator,
            2,
            KillObject(kill_object::KillObject {
                object_data: vec![kill_object::ObjectData { local_id: target.binding.address().local_id().get() }],
            }),
        );
        let input = select_drain_input(ready(Some(event)), media.next_result(), pending(), pending()).await;
        let super::DrainInput::Event(Some(event)) = input else { panic!("network intake continues after media failure") };
        let _ = apply_world_event(&mut world, &event).expect("object removal is applied");
        assert!(world.world().object(target.binding.entity()).is_none());

        let input = select_drain_input(pending(), media.next_result(), pending(), ready(())).await;
        assert!(matches!(input, super::DrainInput::QueryStateMaintenance));
        let input = select_drain_input(pending(), media.next_result(), ready(()), pending()).await;
        assert!(matches!(input, super::DrainInput::Cancelled));
    }

    #[test]
    fn waiting_media_coalesces_by_entity() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let old = object_media_target(&mut world, simulator_id, 1, 1);
        let newer =
            ObjectMediaTarget { source: UpdateSource::new(old.source.region(), 2, OrderEvidence::Ordered(2), 0), version: 2, ..old };
        let later_marker_with_lower_version =
            ObjectMediaTarget { source: UpdateSource::new(old.source.region(), 3, OrderEvidence::Ordered(3), 0), version: 1, ..old };
        let mut waiting = WaitingObjectMedia::default();

        assert!(waiting.insert(old));
        assert!(waiting.insert(newer));
        assert!(waiting.insert(later_marker_with_lower_version));
        assert_eq!(waiting.len(), 1);
        assert_eq!(waiting.pop_front(), Some(newer));
        assert_eq!(waiting.pop_front(), None);
    }

    #[test]
    fn media_failure_is_ignored_after_its_object_binding_changes() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let target = object_media_target(&mut world, simulator_id, 1, 1);
        assert!(world.object_media_target_is_current(target));

        let source = UpdateSource::new(target.source.region(), 2, OrderEvidence::Ordered(2), 0);
        world
            .apply_object_batch(
                simulator_id,
                ObjectBatch {
                    region: target.source.region(),
                    region_handle: None,
                    blocks: vec![Some(ObjectOperation::Remove { source, local_id: target.binding.address().local_id() })],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .expect("remove object");

        assert!(!world.object_media_target_is_current(target));
    }

    #[test]
    fn media_progresses_during_continuous_events() {
        let simulator_id = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        let target = object_media_target(&mut world, simulator_id, 1, 1);
        let expected_entity = target.binding.entity();
        let (media_ready, media_gate) = oneshot::channel();
        let media = stream::once(async move {
            media_gate.await.expect("first event releases the queued media result");
            ObjectMediaResult { target, update: Err(FetchError::Transport) }
        })
        .chain(stream::pending());
        futures::pin_mut!(media);
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut media_ready = Some(media_ready);
        let mut event_count = 0;

        let media_serviced = block_on(async {
            while event_count < 128 {
                match select_drain_input(
                    std::future::ready(Some(Event::SimulatorConnected { simulator_id, region_handle: None, endpoint })),
                    media.next(),
                    std::future::pending(),
                    std::future::pending(),
                )
                .await
                {
                    super::DrainInput::Media(Some(result)) => {
                        assert_eq!(result.target.binding.entity(), expected_entity);
                        return true;
                    }
                    super::DrainInput::Event(Some(_)) => {
                        event_count += 1;
                        if event_count == 1 {
                            media_ready.take().expect("media gate is released once").send(()).expect("media receiver is waiting");
                        }
                    }
                    _ => panic!("unexpected intake result"),
                }
            }
            false
        });

        assert!(media_serviced, "a queued media response should progress while simulator events remain ready");
        assert!(event_count < 128, "the event stream must remain ready until media is serviced");
    }

    #[test]
    fn dirty_query_repair_retries_without_busy_polling() {
        let elapsed = Duration::from_millis(15);
        assert_eq!(super::next_query_state_maintenance_delay(elapsed, false), Duration::from_millis(10));
        assert_eq!(super::next_query_state_maintenance_delay(elapsed, true), Duration::from_millis(1));
    }

    #[tokio::test(start_paused = true)]
    async fn query_state_maintenance_keeps_phase() {
        let maintenance_started = tokio::time::Instant::now().into_std();
        let mut maintenance = ready(()).boxed().fuse();
        assert_eq!(super::next_query_state_maintenance_delay(Duration::ZERO, false), Duration::from_millis(25));

        let result = select_drain_input(std::future::pending(), std::future::pending(), std::future::pending(), &mut maintenance).await;
        assert!(matches!(result, super::DrainInput::QueryStateMaintenance));

        tokio::time::advance(Duration::from_millis(30)).await;
        let delay =
            super::next_query_state_maintenance_delay(tokio::time::Instant::now().into_std().duration_since(maintenance_started), false);
        assert_eq!(delay, Duration::from_millis(20));
        maintenance = tokio::time::sleep(delay).boxed().fuse();
        let simulator_id = simulator_id();
        let mut events = stream::iter([SimulatorConnected {
            simulator_id,
            region_handle: None,
            endpoint: "127.0.0.1:13000".parse().expect("endpoint"),
        }]);
        let result =
            select_drain_input(events.next(), stream::pending::<ObjectMediaResult>().next(), std::future::pending(), &mut maintenance)
                .await;
        assert!(matches!(result, super::DrainInput::Event(Some(_))));

        let result = select_drain_input(std::future::pending(), std::future::pending(), std::future::pending(), &mut maintenance).await;
        assert!(matches!(result, super::DrainInput::QueryStateMaintenance));

        tokio::time::advance(Duration::from_millis(45)).await;
        let delay =
            super::next_query_state_maintenance_delay(tokio::time::Instant::now().into_std().duration_since(maintenance_started), false);
        assert_eq!(delay, Duration::from_millis(5));
        maintenance = tokio::time::sleep(delay).boxed().fuse();
        let result = select_drain_input(std::future::pending(), std::future::pending(), std::future::pending(), &mut maintenance).await;

        assert!(matches!(result, super::DrainInput::QueryStateMaintenance));
    }

    fn agent_update(agent_id: Uuid, local_id: u32, region_handle: u64) -> Message {
        let mut message = fixture_packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut message else { panic!("expected full object update") };
        update.region_data.region_handle = region_handle;
        update.objects.truncate(1);
        update.objects[0].id = agent_id;
        update.objects[0].local_id = local_id;
        update.objects[0].pcode = 47;
        update.objects[0].parent_local_id = 0;
        message
    }

    fn agent_observation(source: UpdateSource, local_id: u32, agent_id: Uuid, pose: Option<Pose>) -> ObjectObservation {
        ObjectObservation {
            pose,
            ..ObjectObservation::new(
                source,
                agent_id,
                NonZeroU32::new(local_id).expect("nonzero local ID"),
                Kind::Avatar,
                ParentUpdate::Root,
            )
        }
    }

    fn simulator_message(simulator_id: SimulatorId, sequence: u32, message: Message) -> Event {
        simulator_message_at_order(simulator_id, sequence, Ordered(u64::from(sequence)), message)
    }

    fn simulator_message_at_order(simulator_id: SimulatorId, sequence: u32, packet_order: PacketOrder, message: Message) -> Event {
        SimulatorMessage { simulator_id, source: "127.0.0.1:13000".parse().expect("simulator address"), sequence, packet_order, message }
    }

    fn chat_message(from_name: &str) -> Message {
        Message::ChatFromSimulator(ChatFromSimulator {
            chat_data: ChatData {
                from_name: from_name.into(),
                source_id: Uuid::nil(),
                owner_id: Uuid::nil(),
                source_type: ChatSourceType::Agent,
                chat_type: ChatType::Normal,
                audible: Audible::Fully,
                position: Vec3::default(),
                message: "hello".into(),
            },
        })
    }

    #[test]
    fn mailbox_coalesces_state_and_bounds_chat() {
        let (mailbox, mut receiver) = PresentationMailbox::new(7);
        for index in 0..1000 {
            mailbox.publish(ViewerUpdate::RegionName(format!("Region {index}")));
        }
        for index in 0..MAX_PENDING_CHAT_MESSAGES + 20 {
            mailbox.publish(ViewerUpdate::ChatMessage(ChatMessage { from_name: format!("Agent {index}"), message: "Hello".into() }));
        }

        let snapshot = futures::executor::block_on(receiver.next()).expect("presentation wake");
        assert_eq!(snapshot.generation, 7);
        assert_eq!(snapshot.region_name.as_deref(), Some("Region 999"));
        assert_eq!(snapshot.chat_messages.len(), MAX_PENDING_CHAT_MESSAGES);
        assert!(snapshot.chat_messages.front().is_some_and(|message| message.from_name == "Agent 20"));
        let bytes = snapshot.chat_messages.iter().map(|message| message.from_name.len() + message.message.len()).sum::<usize>();
        assert!(bytes <= MAX_PENDING_CHAT_BYTES);
        assert!(receiver.wake_receiver.try_recv().is_err());
    }

    #[test]
    fn failure_discards_pending_chat() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        for _ in 0..MAX_PENDING_CHAT_MESSAGES {
            mailbox.publish(ViewerUpdate::ChatMessage(ChatMessage { from_name: "Agent".into(), message: "Hello".into() }));
        }
        mailbox.publish(ViewerUpdate::Failed("connection closed".into()));

        let snapshot = futures::executor::block_on(receiver.next()).expect("failure wake");
        assert!(snapshot.chat_messages.is_empty());
        assert_eq!(snapshot.failure.as_deref(), Some("connection closed"));
    }

    #[test]
    fn region_connection_lifecycle() {
        let simulator_id = simulator_id();
        let handle = 0x1111_0000_0000_0001;
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut intake = SessionWorld::new(viewer_world_limits());

        apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: Some(handle), endpoint });
        let region = intake.region_for(simulator_id).expect("registered region");
        assert_eq!(region, Region::new(simulator_id.get()));
        assert_eq!(intake.world().region(region).unwrap().handle, Some(RegionHandle::new(handle)));

        apply_world_event(&mut intake, &SimulatorDisconnected { simulator_id, region_handle: Some(handle), endpoint });
        assert!(intake.region_for(simulator_id).is_none());
        assert!(intake.world().region(region).is_none());
        assert!(apply_world_event(&mut intake, &simulator_message(simulator_id, 1, region_handshake("late"))).is_none());
    }

    #[test]
    fn gaining_agent_waits_for_primary_promotion() {
        let agent_id = Uuid::from_u128(0x1234);
        let source_id = simulator_id();
        let destination_id = simulator_id();
        let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
        let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
        let mut intake = SessionWorld::new(viewer_world_limits());
        intake.set_self_agent(agent_id);
        let source_region = intake.register_simulator(source_id, Some(source_handle)).expect("register source region");
        let destination_region = intake.register_simulator(destination_id, Some(destination_handle)).expect("register destination region");
        let mut world_primary = Some(source_id);
        let mut promoted_target = None;
        let mut observations = HashMap::new();

        let source = agent_update(agent_id, 10, source_handle.get());
        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message(source_id, 1, source),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply source agent observation");

        let destination = agent_update(agent_id, 20, destination_handle.get());
        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 2, Ordered(20), destination),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("ingest destination packet");

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 3, Ordered(19), agent_update(agent_id, 21, destination_handle.get())),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("ingest late destination packet");
        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 4, Ambiguous, agent_update(agent_id, 22, destination_handle.get())),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("ingest ambiguous destination packet");
        assert_eq!(observations.get(&destination_id).unwrap().source.order(), ::world::OrderEvidence::Ordered(20));

        let state = intake.world();
        let agent = state.object(state.entity(agent_id).expect("source agent remains present")).expect("agent object");
        assert_eq!(agent.address().region(), source_region);
        assert!(observations.contains_key(&destination_id));
        drop(agent);

        assert!(begin_world_handoff(&mut world_primary, &mut promoted_target, source_id, destination_id));
        let destination = agent_update(agent_id, 20, destination_handle.get());
        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message(destination_id, 3, destination),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("complete explicit destination handoff");

        let state = intake.world();
        let agent = state.object(state.entity(agent_id).expect("agent survives handoff")).expect("agent object");
        assert_eq!(agent.address().region(), destination_region);
        assert_eq!(world_primary, Some(destination_id));
        assert_eq!(promoted_target, None);
        assert!(!observations.contains_key(&destination_id));
        drop(agent);
        apply_world_event_with_handoff(
            &mut intake,
            &SimulatorDisconnected {
                simulator_id: source_id,
                region_handle: Some(source_handle.get()),
                endpoint: "127.0.0.1:13000".parse().expect("simulator address"),
            },
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        );
        assert_eq!(world_primary, Some(destination_id));
        assert!(intake.region_for(destination_id).is_some());
        let state = intake.world();
        let agent = state.object(state.entity(agent_id).expect("promoted agent remains present")).expect("agent object");
        assert_eq!(agent.address().region(), destination_region);
    }

    #[test]
    fn promoted_handoff_preserves_field_rejections() {
        let agent_id = Uuid::from_u128(0x1234);
        let source_id = simulator_id();
        let destination_id = simulator_id();
        let mut intake = SessionWorld::new(viewer_world_limits());
        intake.set_self_agent(agent_id);
        let source_region = intake.register_simulator(source_id, None).expect("register source region");
        let destination_region = intake.register_simulator(destination_id, None).expect("register destination region");
        let source_observation =
            agent_observation(UpdateSource::new(source_region, 1, ::world::OrderEvidence::Ordered(1), 0), 10, agent_id, None);
        intake
            .apply_object_batch(
                source_id,
                ObjectBatch {
                    region: source_region,
                    region_handle: None,
                    blocks: vec![Some(ObjectOperation::Observe(source_observation))],
                    invalid_blocks: 0,
                    invalid_fields: 0,
                },
            )
            .expect("observe source avatar");

        let mut world_primary = Some(source_id);
        let mut pending = None;
        assert!(begin_world_handoff(&mut world_primary, &mut pending, source_id, destination_id));
        let observation = agent_observation(
            UpdateSource::new(destination_region, 2, ::world::OrderEvidence::Ordered(2), 0),
            20,
            agent_id,
            Some(Pose { position: None, rotation: Some(Quat::from_xyzw(f32::NAN, 0.0, 0.0, 1.0)) }),
        );

        let (completed, summary) = complete_promoted_handoff(&mut intake, &mut world_primary, &mut pending, destination_id, observation);

        assert!(completed);
        let summary = summary.expect("promoted handoff produces an ingest summary");
        let applied = summary.blocks[0].result.as_ref().expect("handoff result is recorded").as_ref().expect("handoff applies");
        assert!(applied.rejections.iter().any(|rejection| matches!(rejection.reason, ::world::FieldRejectionReason::InvalidRotation)));
    }

    #[test]
    fn newer_handoff_conflicts_invalidate_observation() {
        let agent_id = Uuid::from_u128(0x1234);
        let replacement_id = Uuid::from_u128(0x5678);
        let source_id = simulator_id();
        let destination_id = simulator_id();
        let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
        let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
        let mut intake = SessionWorld::new(viewer_world_limits());
        intake.set_self_agent(agent_id);
        let source_region = intake.register_simulator(source_id, Some(source_handle)).expect("register source");
        let destination_region = intake.register_simulator(destination_id, Some(destination_handle)).expect("register destination");
        let mut world_primary = Some(source_id);
        let mut promoted_target = None;
        let mut observations = HashMap::new();

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(source_id, 1, Ordered(1), agent_update(agent_id, 10, source_handle.get())),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply source agent");
        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 2, Ordered(20), agent_update(agent_id, 20, destination_handle.get())),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("buffer destination agent");

        let removed_address = LocalAddress::new(destination_region, NonZeroU32::new(20).expect("nonzero local ID"));
        let removal = Message::KillObject(kill_object::KillObject { object_data: vec![kill_object::ObjectData { local_id: 20 }] });
        let (_, summary) = apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 3, Ordered(19), removal.clone()),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply stale kill");
        let result = summary.blocks[0].result.as_ref().expect("valid kill").as_ref().expect("recorded kill");
        assert!(matches!(&result.outcome, TombstoneRecorded(address) if *address == removed_address));
        assert!(observations.contains_key(&destination_id));

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 4, Ordered(21), removal),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply newer kill");
        assert!(!observations.contains_key(&destination_id));

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 5, Ordered(22), agent_update(agent_id, 21, destination_handle.get())),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("buffer replacement-address agent");
        let replacement = agent_update(replacement_id, 21, destination_handle.get());
        let (_, summary) = apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 6, Ordered(23), replacement),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply newer replacement");
        assert!(summary.blocks[0].result.as_ref().expect("valid replacement").is_ok());
        assert!(!observations.contains_key(&destination_id));

        assert!(begin_world_handoff(&mut world_primary, &mut promoted_target, source_id, destination_id));
        assert_eq!(world_primary, Some(source_id));
        assert_eq!(promoted_target, Some(super::PendingWorldHandoff { source: source_id, target: destination_id }));
        let state = intake.world();
        let agent = state.object(state.entity(agent_id).expect("source agent remains present")).expect("agent object");
        assert_eq!(agent.address().region(), source_region);
        assert_eq!(
            state.object(state.entity(replacement_id).expect("replacement was applied")).unwrap().address(),
            LocalAddress::new(destination_region, NonZeroU32::new(21).unwrap())
        );
    }

    #[test]
    fn deferred_handoff_keeps_source_primary() {
        let agent_id = Uuid::from_u128(0x1234);
        let source_id = simulator_id();
        let destination_id = simulator_id();
        let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
        let destination_handle = RegionHandle::new(0x2222_0000_0000_0002);
        let mut intake = SessionWorld::new(viewer_world_limits());
        intake.set_self_agent(agent_id);
        let source_region = intake.register_simulator(source_id, Some(source_handle)).expect("register source region");
        intake.register_simulator(destination_id, Some(destination_handle)).expect("register destination region");
        let mut world_primary = Some(source_id);
        let mut promoted_target = None;
        let mut observations = HashMap::new();

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message(source_id, 1, agent_update(agent_id, 10, source_handle.get())),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply source agent observation");
        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(destination_id, 2, Ambiguous, agent_update(agent_id, 20, destination_handle.get())),
            &mut world_primary,
            &mut promoted_target,
            &mut observations,
            Some(agent_id),
        )
        .expect("buffer ambiguous destination observation");
        assert!(begin_world_handoff(&mut world_primary, &mut promoted_target, source_id, destination_id));

        let observation = observations.get(&destination_id).cloned().expect("buffered destination observation");
        assert!(!complete_promoted_world_handoff(&mut intake, &mut world_primary, &mut promoted_target, destination_id, observation).0);
        assert_eq!(world_primary, Some(source_id));
        assert!(promoted_target.is_some());

        let state = intake.world();
        let agent = state.object(state.entity(agent_id).expect("source agent remains present")).expect("agent object");
        assert_eq!(agent.address().region(), source_region);
    }

    #[test]
    fn stale_promotion_is_ignored() {
        let source = simulator_id();
        let stale_source = simulator_id();
        let target = simulator_id();
        let mut world_primary = Some(source);
        let mut pending = None;

        assert!(!begin_world_handoff(&mut world_primary, &mut pending, stale_source, target));
        assert_eq!(world_primary, Some(source));
        assert_eq!(pending, None);
    }

    #[test]
    fn chained_promotion_retargets_handoff() {
        let agent_id = Uuid::from_u128(0x1234);
        let source_id = simulator_id();
        let middle_id = simulator_id();
        let target_id = simulator_id();
        let source_handle = RegionHandle::new(0x1111_0000_0000_0001);
        let middle_handle = RegionHandle::new(0x2222_0000_0000_0002);
        let target_handle = RegionHandle::new(0x3333_0000_0000_0003);
        let mut intake = SessionWorld::new(viewer_world_limits());
        intake.set_self_agent(agent_id);
        intake.register_simulator(source_id, Some(source_handle)).expect("register source");
        intake.register_simulator(middle_id, Some(middle_handle)).expect("register first destination");
        intake.register_simulator(target_id, Some(target_handle)).expect("register latest destination");
        let mut world_primary = Some(source_id);
        let mut pending = None;
        let mut observations = HashMap::new();

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(source_id, 1, Ordered(1), agent_update(agent_id, 10, source_handle.get())),
            &mut world_primary,
            &mut pending,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply source agent");
        assert!(begin_world_handoff(&mut world_primary, &mut pending, source_id, middle_id));
        assert!(begin_world_handoff(&mut world_primary, &mut pending, middle_id, target_id));

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(middle_id, 2, Ordered(2), agent_update(agent_id, 20, middle_handle.get())),
            &mut world_primary,
            &mut pending,
            &mut observations,
            Some(agent_id),
        )
        .expect("ignore stale intermediate avatar observation");
        assert_eq!(world_primary, Some(source_id));

        apply_world_event_with_handoff(
            &mut intake,
            &simulator_message_at_order(target_id, 3, Ordered(3), agent_update(agent_id, 30, target_handle.get())),
            &mut world_primary,
            &mut pending,
            &mut observations,
            Some(agent_id),
        )
        .expect("apply latest destination avatar observation");

        assert_eq!(world_primary, Some(target_id));
        assert_eq!(pending, None);
        let state = intake.world();
        assert_eq!(state.object(state.entity(agent_id).unwrap()).unwrap().address().region(), intake.region_for(target_id).unwrap());
    }

    #[test]
    fn session_exit_retires_world_regions() {
        let simulator_id = simulator_id();
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut intake = SessionWorld::new(viewer_world_limits());
        apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });
        let region = intake.region_for(simulator_id).expect("registered region");

        assert!(intake.retire_all().is_empty());

        let state = intake.world();
        assert!(state.regions().next().is_none());
        assert!(state.objects().iter().next().is_none());
        assert!(state.region(region).is_none());
    }

    #[test]
    fn unknown_removal_cancels_recovery() {
        let simulator_id = simulator_id();
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut intake = SessionWorld::new(viewer_world_limits());
        apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });
        let region = intake.region_for(simulator_id).expect("registered region");
        let message = fixture_packet(vectors::KILL_OBJECT).message().clone();
        let KillObject(kill) = &message else { panic!("expected object removal fixture") };
        let addresses = kill
            .object_data
            .iter()
            .map(|object| LocalAddress::new(region, NonZeroU32::new(object.local_id).expect("valid local ID")))
            .collect::<Vec<_>>();

        let (_, summary) = apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message)).expect("registered simulator");
        assert_eq!(summary.blocks.len(), addresses.len());
        for (block, address) in summary.blocks.iter().zip(&addresses) {
            let result = block.result.as_ref().expect("valid object removal").as_ref().expect("recorded tombstone");
            assert_eq!(result.revision, None);
            assert!(matches!(&result.outcome, TombstoneRecorded(recorded) if recorded == address));
        }
        assert_eq!(summary.recovery_cancellation_addresses().collect::<Vec<_>>(), addresses);
    }

    #[test]
    fn repeated_removal_keeps_recovery() {
        let simulator_id = simulator_id();
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut intake = SessionWorld::new(viewer_world_limits());
        apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });
        let message = fixture_packet(vectors::KILL_OBJECT).message().clone();

        let (_, first) =
            apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message.clone())).expect("registered simulator");
        let first_result = first.blocks[0].result.as_ref().expect("valid object removal").as_ref().expect("recorded tombstone");
        assert!(matches!(&first_result.outcome, TombstoneRecorded(_)));

        let (_, repeated) = apply_world_event(&mut intake, &simulator_message(simulator_id, 1, message)).expect("registered simulator");
        let repeated_result = repeated.blocks[0].result.as_ref().expect("valid repeated removal").as_ref().expect("unchanged removal");

        assert_eq!(repeated_result.revision, None);
        assert!(matches!(&repeated_result.outcome, Unchanged));
        assert_eq!(repeated.recovery_cancellation_addresses().count(), 0);
    }

    #[test]
    fn object_update_order_schedules_recovery() {
        let simulator_id = simulator_id();
        let region_handle = 0x2222_0000_0000_0002;
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut intake = SessionWorld::new(viewer_world_limits());
        apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: Some(region_handle), endpoint });

        let mut full = fixture_packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut full else { panic!("expected full object fixture") };
        update.region_data.region_handle = region_handle;
        let parent_local_id = update.objects[0].local_id;
        let parent_id = update.objects[0].id;
        let child_id = update.objects[1].id;
        update.objects[1].parent_local_id = parent_local_id;

        let (_, full_summary) =
            apply_world_event(&mut intake, &simulator_message(simulator_id, 1, full.clone())).expect("registered simulator");
        assert_eq!(full_summary.status, IngestStatus::Applied);
        assert_eq!(full_summary.blocks.len(), 2);
        assert!(full_summary.blocks.iter().all(|block| matches!(&block.result.as_ref().unwrap().as_ref().unwrap().outcome, Introduced(_))));
        let region = intake.region_for(simulator_id).unwrap();
        {
            let state = intake.world();
            let child = state.object(state.entity(child_id).unwrap()).unwrap();
            assert!(matches!(
                child.parent(),
                Parent::Resolved(parent) if state.object(parent).unwrap().id() == parent_id
            ));
        }

        let mut terse = fixture_packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
        let ImprovedTerseObjectUpdate(update) = &mut terse else { panic!("expected terse fixture") };
        update.region_data.region_handle = region_handle;
        let object = update.objects[0].as_decoded_mut().expect("fixture block is structurally valid");
        object.local_id = parent_local_id;
        object.motion.position = Vec3::new(31.0, 42.0, 53.0);
        object.motion.rotation = Quat::IDENTITY;
        apply_world_event(&mut intake, &simulator_message(simulator_id, 2, terse.clone())).expect("registered simulator");
        let state = intake.world();
        let parent = state.object(state.entity(parent_id).unwrap()).unwrap();
        assert_eq!(parent.transform().position(), Some(Vec3::new(31.0, 42.0, 53.0)));
        let before_cached_revision = state.revision();
        drop(parent);

        let mut cached = fixture_packet(vectors::OBJECT_UPDATE_CACHED).message().clone();
        let ObjectUpdateCached(update) = &mut cached else { panic!("expected cached object fixture") };
        update.region_data.region_handle = region_handle;
        let (_, cached_summary) =
            apply_world_event(&mut intake, &simulator_message(simulator_id, 3, cached)).expect("registered simulator");
        let targets = cached_summary.recovery_targets().collect::<Vec<_>>();
        assert!(!targets.is_empty());
        assert!(targets.iter().all(|target| target.region() == region));
        assert_eq!(intake.world().revision(), before_cached_revision);
        let request = recovery_request(RequestAgentData { agent_id: Uuid::from_u128(1), session_id: Uuid::from_u128(2) }, &targets);
        assert!(request.objects.iter().all(|object| object.cache_miss_type == Full));

        let mut unknown = fixture_packet(vectors::IMPROVED_TERSE_OBJECT_UPDATE).message().clone();
        let ImprovedTerseObjectUpdate(update) = &mut unknown else { unreachable!() };
        update.region_data.region_handle = region_handle;
        update.objects[0].as_decoded_mut().expect("fixture block is structurally valid").local_id = 0x7654_3210;
        let (_, ambiguous) =
            apply_world_event(&mut intake, &simulator_message_at_order(simulator_id, 4, Ambiguous, unknown)).expect("registered simulator");
        assert_eq!(&ambiguous.blocks[0].result.as_ref().unwrap().as_ref().unwrap().outcome, &Deferred(AmbiguousOrder));
        assert_eq!(ambiguous.recovery_targets().count(), 1);
    }

    #[test]
    fn invalid_update_leaves_region_unbound() {
        let simulator_id = simulator_id();
        let endpoint = "127.0.0.1:13000".parse().expect("endpoint");
        let mut intake = SessionWorld::new(viewer_world_limits());
        apply_world_event(&mut intake, &SimulatorConnected { simulator_id, region_handle: None, endpoint });

        let mut invalid = fixture_packet(vectors::OBJECT_UPDATE).message().clone();
        let ObjectUpdate(update) = &mut invalid else { panic!("expected full update fixture") };
        update.region_data.region_handle = 0x3333_0000_0000_0003;
        update.objects.truncate(1);
        update.objects[0].id = Uuid::nil();
        let (_, summary) = apply_world_event(&mut intake, &simulator_message(simulator_id, 10, invalid)).expect("registered simulator");

        assert_eq!(summary.invalid_blocks, 1);
        assert_eq!(summary.accepted_region_handle, None);
        let region = intake.region_for(simulator_id).unwrap();
        assert_eq!(intake.world().region(region).unwrap().handle, None);
    }

    #[test]
    fn stale_favorite_snapshot_is_ignored() {
        const AGENT_ID: Uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let test_directory = tempdir().expect("create temporary favorite directory");
        let path = test_directory.path().join("favorites.json");

        let mut old_store = FavoriteStore::default();
        old_store.replace_favorites(
            Grid::Agni,
            AGENT_ID,
            "Example Resident",
            vec![FavoriteLocation { name: "Old".into(), region_name: "Old Region".into(), x: 1, y: 2, z: 3 }],
        );
        let mut new_store = FavoriteStore::default();
        new_store.replace_favorites(
            Grid::Agni,
            AGENT_ID,
            "Example Resident",
            vec![FavoriteLocation { name: "Newest".into(), region_name: "New Region".into(), x: 4, y: 5, z: 6 }],
        );

        let (old_result, new_result) = futures::executor::block_on(async {
            let lock = write_lock();
            let guard = lock.lock().await;
            let generation = Arc::new(AtomicU64::new(1));

            let old_path = path.clone();
            let mut old_save = Box::pin(save_if_current(old_store, lock.clone(), Arc::clone(&generation), 1, move |store| {
                let contents = serde_json::to_vec(&store).map_err(|error| error.to_string())?;
                fs::write(old_path, contents).map_err(|error| error.to_string())
            }));
            assert!(futures::poll!(old_save.as_mut()).is_pending());

            let new_path = path.clone();
            let mut new_save = Box::pin(save_if_current(new_store, lock.clone(), Arc::clone(&generation), 2, move |store| {
                let contents = serde_json::to_vec(&store).map_err(|error| error.to_string())?;
                fs::write(new_path, contents).map_err(|error| error.to_string())
            }));
            assert!(futures::poll!(new_save.as_mut()).is_pending());

            generation.store(2, Ordering::Release);
            drop(guard);
            futures::join!(old_save, new_save)
        });

        assert!(old_result.is_none());
        assert_eq!(new_result, Some(Ok(())));
        let saved: FavoriteStore =
            serde_json::from_slice(&fs::read(&path).expect("saved latest favorites")).expect("saved snapshot is valid JSON");
        assert_eq!(saved.favorites_for(Grid::Agni, AGENT_ID)[0].name, "Newest");
    }

    #[test]
    fn presentation_uses_the_name_accepted_by_world() {
        let simulator = simulator_id();
        let mut world = SessionWorld::new(viewer_world_limits());
        world.register_simulator(simulator, None).unwrap();
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        let address = "127.0.0.1:13000".parse().unwrap();
        for (order, name) in [(10, "Initial"), (20, "Renamed"), (15, "Stale")] {
            let message = region_handshake(name);
            super::decode_simulator_world_update(&mut world, simulator, order, Ordered(u64::from(order)), &message);
            handle_simulator_message(SimulatorScope::Primary, address, message, None, None, world.region_name(simulator), &mailbox);
        }
        let snapshot = futures::executor::block_on(receiver.next()).expect("presentation wake");
        assert_eq!(snapshot.region_name.as_deref(), Some("Renamed"));
        assert_eq!(world.region_name(simulator), Some("Renamed"));
    }

    #[test]
    fn neighbor_chat_survives_primary_updates() {
        let (mailbox, mut receiver) = PresentationMailbox::new(1);
        let source = "127.0.0.1:13000".parse().expect("simulator address");

        for index in 0..100 {
            handle_simulator_message(
                SimulatorScope::Neighbor,
                source,
                region_handshake(&format!("Neighbor {index}")),
                None,
                None,
                Some("Neighbor"),
                &mailbox,
            );
            handle_simulator_message(SimulatorScope::Neighbor, source, chat_message("Neighbor"), None, None, None, &mailbox);
            handle_simulator_message(
                SimulatorScope::Primary,
                source,
                region_handshake("Unaccepted"),
                None,
                None,
                Some(&format!("Primary {index}")),
                &mailbox,
            );
            handle_simulator_message(SimulatorScope::Primary, source, chat_message("Primary"), None, None, None, &mailbox);
        }

        let snapshot = futures::executor::block_on(receiver.next()).expect("presentation wake");
        assert_eq!(snapshot.region_name.as_deref(), Some("Primary 99"));
        assert_eq!(snapshot.chat_messages.len(), MAX_PENDING_CHAT_MESSAGES);
        let names = snapshot.chat_messages.iter().map(|message| message.from_name.as_str()).collect::<Vec<_>>();
        let expected = (0..MAX_PENDING_CHAT_MESSAGES / 2).flat_map(|_| ["Neighbor", "Primary"]).collect::<Vec<_>>();
        assert_eq!(names, expected);
        assert!(receiver.wake_receiver.try_recv().is_err());
    }
}
