use std::{
    collections::{HashMap, HashSet},
    future::pending,
    ops::ControlFlow,
    sync::{Arc, Mutex},
};

use ::inventory::{
    CategoryKey, ChangeSet, Inventory, InventoryChange, InventoryChild, InventoryDelta, InventoryError, InventoryNodeId, InventoryNodeKey,
    InventorySnapshot, ServerVersion,
};
use futures::{
    FutureExt,
    channel::oneshot,
    executor::block_on,
    future::{BoxFuture, Shared},
    pin_mut,
};
use network::{AgentSession, CoverageLoss, CoverageSource, Event, Grid, PacketOrder, SimulatorId, capabilities::Client};
use protocol::{lludp::Message, login::ResponseData, messages::RequestAgentData};
use tokio::{runtime::Handle, sync::watch};
use tracing::Instrument;
use world::UpdateSource;

use super::{
    WriteLock,
    events::{MovementCompleteOutcome, SimulatorScope, WorldHandoff, handle_simulator_message, order_evidence, route_recovery_targets},
    input::{Input, select_drain_input},
    inventory_bootstrap::{InventoryBootstrapReport, InventoryWorkerError, spawn_inventory_build},
    inventory_cache::{InventoryCacheEvent, InventoryCacheSession},
    inventory_intake::{self, InventoryIntakeError, InventoryIntakeOutcome},
    media::ObjectMediaIntake,
    presentation::{AuthenticationUpdate, FavoritesUpdate, PresentationMailbox, SessionReadyUpdate, ViewerUpdate},
};
use crate::{
    asset_coordinator::AssetCoordinator,
    inventory::{
        AisDelta, FolderApplyOutcome, FolderFetch, InventoryEditController, InventoryEditEvent, InventoryFetchController,
        InventoryFetchError, InventoryFetchEvent, InventoryLoadingStatus, MoveExpectation, favorites::load_favorites,
    },
    remembered_login::{self, RememberedLogin},
    session::{
        AgentMotionPublisher, CameraUplink, RegionLookup, Session, SessionHandle,
        environment::{Completion, EnvironmentIntake},
        inbound::object_media_requests,
        object_cache::{ObjectCacheEvent, ObjectCacheSession},
        object_media::{ObjectMediaFetcher, ObjectMediaResult},
        recovery::{RecoveryCompletion, RecoveryIntake, ScheduledRecovery, retryable_send_error, simulator_lost_error, warn_unrecovered},
        terrain::{DecodedTerrain, TerrainDecodeQueue},
        world::{IngestSummary, SessionWorld, viewer_world_limits},
    },
    world_reader::{ReadRequest, WorldReadHost},
};

#[cfg(any(target_os = "windows", target_os = "linux"))]
use crate::session::experience_environment::{EnvironmentInbox, EnvironmentSender};

type Cancellation = Shared<BoxFuture<'static, ()>>;

enum DriverInput {
    Session(Input),
    InventoryCache(Option<InventoryCacheEvent>),
    ObjectCache(Option<ObjectCacheEvent>),
    InventoryEdit(Option<InventoryEditEvent>),
    InventoryBootstrap(Result<Result<(Inventory, InventoryBootstrapReport), InventoryWorkerError>, tokio::task::JoinError>),
}

#[derive(Default)]
struct PendingHandshakeReplies {
    by_simulator: HashMap<SimulatorId, PendingHandshakeReply>,
}

struct PendingHandshakeReply {
    flags: u32,
    remaining: usize,
}

impl PendingHandshakeReplies {
    fn queue(&mut self, simulator_id: SimulatorId, flags: u32) -> bool {
        match self.by_simulator.get_mut(&simulator_id) {
            Some(reply) => {
                reply.flags = flags;
                reply.remaining = reply.remaining.saturating_add(1);
                false
            }
            None => {
                self.by_simulator.insert(simulator_id, PendingHandshakeReply { flags, remaining: 1 });
                true
            }
        }
    }

    fn take_next(&mut self, simulator_id: SimulatorId) -> Option<u32> {
        if self.by_simulator.get(&simulator_id).is_some_and(|reply| reply.remaining == 0) {
            self.by_simulator.remove(&simulator_id);
            return None;
        }
        let reply = self.by_simulator.get_mut(&simulator_id)?;
        reply.remaining -= 1;
        Some(reply.flags)
    }
}

pub(crate) struct CredentialPersistence {
    pub(crate) grid: Grid,
    pub(crate) username: String,
    pub(crate) password: String,
    pub(crate) remember_login: bool,
    pub(crate) write_lock: WriteLock,
}

struct SessionDriver {
    session: Session,
    session_handle: SessionHandle,
    runtime: Handle,
    mailbox: PresentationMailbox,
    cancellation: Cancellation,
    credentials: CredentialPersistence,
    session_world: SessionWorld,
    reads: WorldReadHost,
    terrain: TerrainDecodeQueue,
    environments: EnvironmentIntake,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    environment_pushes: EnvironmentSender,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    environment_inbox: EnvironmentInbox,
    assets: AssetCoordinator,
    terrain_received: bool,
    primary_simulator: Option<SimulatorId>,
    handoff: WorldHandoff,
    active_session: Option<AgentSession>,
    active_login_data: Option<ResponseData>,
    active_capabilities: Option<Client>,
    favorites_enabled: bool,
    inventory_bootstrap: Option<tokio::task::JoinHandle<Result<(Inventory, InventoryBootstrapReport), InventoryWorkerError>>>,
    inventory_bootstrap_dirty: bool,
    inventory: Option<Inventory>,
    inventory_cache: InventoryCacheSession,
    object_cache: ObjectCacheSession,
    pending_handshake_replies: Arc<Mutex<PendingHandshakeReplies>>,
    inventory_changes: watch::Sender<Option<InventorySnapshot>>,
    rejected_inventory_blocks: u64,
    inventory_fetch: InventoryFetchController,
    inventory_edits: InventoryEditController,
    inventory_pending_moves: HashMap<InventoryNodeKey, (MoveExpectation, tokio::sync::oneshot::Sender<()>)>,
    inventory_known: HashSet<CategoryKey>,
    inventory_complete: HashSet<CategoryKey>,
    inventory_failed: HashSet<CategoryKey>,
    inventory_background_done: bool,
    inventory_last_failure: Option<(CategoryKey, InventoryFetchError)>,
    camera: CameraUplink,
    agent_motion: AgentMotionPublisher,
    region_lookup: Option<RegionLookup>,
    media: ObjectMediaIntake,
    recovery: RecoveryIntake,
}

impl SessionDriver {
    fn new(
        session: Session,
        session_handle: SessionHandle,
        runtime: Handle,
        mailbox: PresentationMailbox,
        cancellation: oneshot::Receiver<()>,
        credentials: CredentialPersistence,
        cache: Option<Arc<assets::cache::AssetCache>>,
    ) -> Self {
        let cancellation = async move {
            match cancellation.await {
                Ok(()) | Err(_) => {}
            }
        }
        .boxed()
        .shared();
        let session_world = SessionWorld::new(viewer_world_limits());
        let reads = WorldReadHost::new(session_world.world(), runtime.clone());
        let terrain = TerrainDecodeQueue::default();
        let environments = EnvironmentIntake::new(runtime.clone());
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let (environment_pushes, environment_inbox) = EnvironmentInbox::channel();
        let assets = AssetCoordinator::new(runtime.clone(), cancellation.clone(), cache);
        let camera = CameraUplink::new();
        let media = ObjectMediaIntake::new(ObjectMediaFetcher::new(runtime.clone(), cancellation.clone()));
        let recovery = RecoveryIntake::new();
        let (inventory_changes, _) = watch::channel(None);
        let inventory_edits = InventoryEditController::new();
        let inventory_fetch = InventoryFetchController::new(inventory_edits.handle());
        let mut object_cache = ObjectCacheSession::new(credentials.grid);
        object_cache.start(&runtime);
        let pending_handshake_replies = Arc::new(Mutex::new(PendingHandshakeReplies::default()));

        Self {
            session,
            session_handle,
            runtime,
            mailbox,
            cancellation,
            credentials,
            session_world,
            reads,
            terrain,
            environments,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            environment_pushes,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            environment_inbox,
            assets,
            terrain_received: false,
            primary_simulator: None,
            handoff: WorldHandoff::default(),
            active_session: None,
            active_login_data: None,
            active_capabilities: None,
            favorites_enabled: false,
            inventory_bootstrap: None,
            inventory_bootstrap_dirty: false,
            inventory: None,
            inventory_cache: InventoryCacheSession::new(),
            object_cache,
            pending_handshake_replies,
            inventory_changes,
            rejected_inventory_blocks: 0,
            inventory_fetch,
            inventory_edits,
            inventory_pending_moves: HashMap::new(),
            inventory_known: HashSet::new(),
            inventory_complete: HashSet::new(),
            inventory_failed: HashSet::new(),
            inventory_background_done: false,
            inventory_last_failure: None,
            camera,
            agent_motion: AgentMotionPublisher::new(),
            region_lookup: None,
            media,
            recovery,
        }
    }

    async fn run(mut self) {
        loop {
            self.reads.publish(self.session_world.world());
            self.media.flush(&self.session_world);
            let terrain_full = self.terrain.is_full();
            let event = async { if terrain_full { pending::<Option<Event>>().await } else { self.session.recv_event().await } };
            let inventory_bootstrap = async {
                match self.inventory_bootstrap.as_mut() {
                    Some(task) => Some(task.await),
                    None => pending().await,
                }
            };
            let input = tokio::select! {
                input = select_drain_input(
                    &mut self.reads,
                    event,
                    self.media.next_result(),
                    self.terrain.next_completion(),
                    self.environments.next_completion(),
                    self.cancellation.clone(),
                    self.camera.next_tick(),
                    self.recovery.next_completion(),
                    self.inventory_fetch.next_event(),
                ) => DriverInput::Session(input),
                cache_event = self.inventory_cache.next_event() => DriverInput::InventoryCache(cache_event),
                cache_event = self.object_cache.next_event() => DriverInput::ObjectCache(cache_event),
                edit_event = self.inventory_edits.next_event() => DriverInput::InventoryEdit(edit_event),
                bootstrap = inventory_bootstrap => DriverInput::InventoryBootstrap(bootstrap.expect("bootstrap task must be pending")),
            };
            let flow = match input {
                DriverInput::InventoryBootstrap(result) => {
                    self.inventory_bootstrap = None;
                    match result {
                        Ok(Ok((inventory, report))) => self.finish_inventory_bootstrap(inventory, report),
                        Ok(Err(error)) => {
                            tracing::warn!(%error, "login inventory could not be initialized");
                            self.start_favorites_load_for_active();
                        }
                        Err(error) => {
                            tracing::warn!(%error, "login inventory worker failed");
                            self.start_favorites_load_for_active();
                        }
                    }
                    ControlFlow::Continue(())
                }
                DriverInput::InventoryCache(Some(event)) => {
                    let before = self.inventory.as_ref().map(Inventory::snapshot);
                    let outcome =
                        self.inventory.as_mut().map(|inventory| self.inventory_cache.handle_event(&self.runtime, event, inventory));
                    if let Some(outcome) = outcome
                        && let Some(snapshot) = outcome.snapshot
                    {
                        if let Some(before) = before {
                            let changes = cache_restore_move_changes(&self.inventory_pending_moves, &before, &snapshot);
                            self.process_inventory_changes(&changes, &snapshot);
                        }
                        self.inventory_changes.send_replace(Some(snapshot));
                    }
                    ControlFlow::Continue(())
                }
                DriverInput::InventoryCache(None) => ControlFlow::Continue(()),
                DriverInput::ObjectCache(Some(event)) => {
                    if let Some((simulator_id, flags)) = self.object_cache.handle_event(event) {
                        reply_region_handshake(
                            &self.runtime,
                            Arc::clone(&self.pending_handshake_replies),
                            self.session_handle.clone(),
                            simulator_id,
                            flags,
                        );
                    }
                    ControlFlow::Continue(())
                }
                DriverInput::ObjectCache(None) => ControlFlow::Continue(()),
                DriverInput::InventoryEdit(Some(event)) => {
                    self.handle_inventory_edit_event(event);
                    ControlFlow::Continue(())
                }
                DriverInput::InventoryEdit(None) => ControlFlow::Continue(()),
                DriverInput::Session(input) => match input {
                    Input::Cancelled => ControlFlow::Break(()),
                    Input::Event(event) => self.handle_network_event(event).await,
                    Input::InventoryFetch(Some(event)) => {
                        self.handle_inventory_fetch(event);
                        ControlFlow::Continue(())
                    }
                    Input::InventoryFetch(None) => ControlFlow::Continue(()),
                    Input::Media(result) => {
                        self.handle_media_result(result);
                        ControlFlow::Continue(())
                    }
                    Input::Terrain(decoded) => {
                        self.handle_terrain_completion(decoded);
                        ControlFlow::Continue(())
                    }
                    Input::Environment(completion) => {
                        self.handle_environment_completion(completion);
                        ControlFlow::Continue(())
                    }
                    Input::WorldRead(request) => {
                        self.handle_world_read(request);
                        ControlFlow::Continue(())
                    }
                    Input::CameraTick => {
                        self.handle_camera_tick();
                        ControlFlow::Continue(())
                    }
                    Input::Recovery(completion) => {
                        self.handle_recovery_completion(completion);
                        ControlFlow::Continue(())
                    }
                },
            };
            if flow.is_break() {
                break;
            }
        }
        self.finish().await;
    }

    fn handle_terrain_completion(&mut self, decoded: DecodedTerrain) {
        let applied = self.session_world.apply_terrain(decoded);
        if !self.terrain_received && applied > 0 {
            self.terrain_received = true;
            tracing::info!(patches = applied, "initial terrain patches received");
        }
    }

    fn handle_environment_completion(&mut self, completion: Option<Box<Completion>>) {
        if let Some(completion) = completion {
            self.environments.complete(&mut self.session_world, *completion);
        }
    }

    fn handle_world_read(&self, request: ReadRequest) {
        request.serve(self.session_world.world());
    }

    fn handle_camera_tick(&mut self) {
        let simulator = self.primary_simulator;
        let origin = self.primary_region_origin();
        self.camera.tick(&self.session_handle, simulator, self.active_session.as_ref(), origin);
    }

    fn primary_region_origin(&self) -> Option<math::DVec3> {
        self.primary_simulator
            .and_then(|simulator| self.session_world.region_for(simulator))
            .and_then(|region| self.session_world.world().region(region))
            .and_then(|info| info.resolved_origin())
    }

    fn handle_media_result(&mut self, result: Option<ObjectMediaResult>) {
        if let Some(result) = result {
            self.media.apply_result(result, &mut self.session_world);
        } else if self.cancellation.clone().now_or_never().is_none() {
            tracing::warn!("object media worker stopped; media fetching is disabled for this session");
            self.media.disable();
        }
    }

    async fn handle_network_event(&mut self, event: Option<Event>) -> ControlFlow<()> {
        let Some(event) = event else {
            self.mailbox.publish(ViewerUpdate::Failed("The network session ended.".into()));
            return ControlFlow::Break(());
        };
        self.handle_event(event).await
    }

    async fn handle_event(&mut self, event: Event) -> ControlFlow<()> {
        use Event::{
            CapabilityError, CapabilityEvent, CapabilityEventBatch, CoverageLost, EventQueueCoverageLost, Failed, LoginAccepted,
            MfaRequired, SendFailed, SessionReady, SimulatorCapabilities, SimulatorConnectFailed, SimulatorConnected,
            SimulatorDisconnected, SimulatorFailed, SimulatorMessage, SimulatorPromoted,
        };

        match event {
            CoverageLost { loss } => {
                self.handle_coverage_loss(&loss);
                ControlFlow::Continue(())
            }
            MfaRequired { challenge } => {
                self.mailbox.publish(ViewerUpdate::Authentication(AuthenticationUpdate::MfaRequired(challenge)));
                ControlFlow::Continue(())
            }
            LoginAccepted { mfa_hash } => {
                self.handle_login_accepted(mfa_hash);
                ControlFlow::Continue(())
            }
            SessionReady { simulator_id, session, capabilities, login_data } => {
                self.handoff.session_ready(&mut self.session_world, simulator_id, session.agent_id);
                if self.inventory.is_none() && self.inventory_bootstrap.is_none() {
                    self.inventory_bootstrap = Some(spawn_inventory_build(&self.runtime, session.agent_id, login_data.clone()));
                }
                self.favorites_enabled = true;
                self.environments.session_ready(&mut self.session_world, simulator_id, &capabilities, self.primary_simulator);
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    self.assets.register(region, capabilities.clone());
                }
                self.on_primary_changed(simulator_id, *session, capabilities, Some(login_data));
                ControlFlow::Continue(())
            }
            SimulatorPromoted { previous_simulator_id, simulator_id, session, capabilities } => {
                #[cfg(any(target_os = "windows", target_os = "linux"))]
                if let Some(region) = self.session_world.region_for(simulator_id)
                    && let Err(error) = self.environment_pushes.clear_region(region)
                {
                    tracing::error!(simulator = simulator_id.get(), region = region.get(), %error, "experience environment crossing reset could not be admitted");
                    self.mailbox.publish(ViewerUpdate::Failed("The environment update queue is unavailable.".into()));
                    return ControlFlow::Break(());
                }
                self.environments.simulator_promoted(
                    &mut self.session_world,
                    previous_simulator_id,
                    simulator_id,
                    &capabilities,
                    self.primary_simulator,
                );
                let summary = self.handoff.promote(&mut self.session_world, previous_simulator_id, simulator_id);
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                tracing::info!(simulator = simulator_id.get(), region, "primary simulator promoted");
                self.on_primary_changed(simulator_id, *session, capabilities, self.active_login_data.clone());
                if let Some(summary) = summary {
                    let touched_agent = self.summary_touches_agent(&summary);
                    self.process_ingest_summary(simulator_id, summary);
                    if touched_agent {
                        self.publish_world_agent_motion(std::time::Instant::now());
                    }
                }
                if let Some((movement, received_at)) = self.handoff.apply_promoted_movement_complete(
                    &mut self.session_world,
                    simulator_id,
                    self.active_session.as_ref().map(|session| (session.agent_id, session.session_id)),
                ) {
                    self.publish_arrival_agent_motion(simulator_id, &movement, received_at);
                }
                ControlFlow::Continue(())
            }
            SimulatorConnected { simulator_id, region_handle, .. } => {
                let primary_origin_before = (self.primary_simulator == Some(simulator_id)).then(|| self.primary_region_origin()).flatten();
                self.handoff.simulator_connected(&mut self.session_world, simulator_id, region_handle);
                if region_handle.is_none() {
                    self.primary_simulator = Some(simulator_id);
                    self.camera.request_update();
                }
                if self.primary_simulator == Some(simulator_id) {
                    self.camera.region_origin_changed(primary_origin_before, self.primary_region_origin());
                }
                ControlFlow::Continue(())
            }
            SimulatorMessage { simulator_id, sequence, packet_order, source, message } => {
                self.handle_inventory_udp(&message);
                self.handle_simulator_message(simulator_id, sequence, packet_order, source, message).await;
                ControlFlow::Continue(())
            }
            SendFailed(error) => {
                tracing::warn!(%error, "simulator send failed");
                ControlFlow::Continue(())
            }
            EventQueueCoverageLost { reason, simulator_id, .. } => {
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                tracing::warn!(simulator = simulator_id.get(), region, %reason, "simulator event delivery may have a gap; world data may be incomplete");
                ControlFlow::Continue(())
            }
            Failed(error) => {
                self.mailbox.publish(ViewerUpdate::Failed(error));
                ControlFlow::Break(())
            }
            SimulatorDisconnected { simulator_id, .. } | SimulatorFailed { simulator_id, .. } => {
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                tracing::info!(simulator = simulator_id.get(), region, "simulator disconnected");
                self.camera.simulator_disconnected(simulator_id);
                self.environments.simulator_disconnected(&mut self.session_world, simulator_id);
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    self.assets.retire(region);
                }
                self.handoff.simulator_disconnected(&mut self.session_world, simulator_id);
                self.object_cache.forget_simulator(simulator_id);
                self.recovery.cancel_simulator(simulator_id);
                self.media.forget_simulator(simulator_id);
                if Some(simulator_id) == self.primary_simulator {
                    self.primary_simulator = None;
                    self.camera.request_update();
                }
                ControlFlow::Continue(())
            }
            CapabilityEvent { simulator_id, message, body, .. } => {
                self.handle_inventory_event_queue(&message, &body);
                self.environments.capability(&mut self.session_world, simulator_id, &message, &body, self.primary_simulator);
                ControlFlow::Continue(())
            }
            CapabilityEventBatch { simulator_id, events, .. } => {
                for event in &events {
                    self.handle_inventory_event_queue(&event.message, &event.body);
                }
                self.environments.capability_batch(
                    &mut self.session_world,
                    simulator_id,
                    events.iter().map(|event| (event.message.as_str(), &event.body)),
                    self.primary_simulator,
                );
                ControlFlow::Continue(())
            }
            CapabilityError { .. } | SimulatorConnectFailed { .. } => ControlFlow::Continue(()),
            SimulatorCapabilities { simulator_id, capabilities, .. } => {
                self.environments.simulator_capabilities(&mut self.session_world, simulator_id, &capabilities, self.primary_simulator);
                if let Some(region) = self.session_world.region_for(simulator_id) {
                    self.assets.register(region, capabilities.clone());
                }
                self.media.set_client(simulator_id, capabilities);
                ControlFlow::Continue(())
            }
        }
    }

    fn handle_coverage_loss(&mut self, loss: &CoverageLoss) {
        match loss {
            CoverageLoss::Source(record) => {
                let simulator_id = record.watermark.simulator_id;
                let region = self.session_world.region_for(simulator_id).map(|region| region.get());
                let source = match record.watermark.source {
                    CoverageSource::Lludp => "lludp",
                    CoverageSource::EventQueue { .. } => "event_queue",
                };
                tracing::warn!(simulator = simulator_id.get(), region, source, reason = ?record.reason, "world data may be incomplete");
                self.session.acknowledge_coverage_loss(record.watermark);
            }
            CoverageLoss::SessionOverflow { generation } => {
                tracing::warn!(generation, "world event queue overflowed; world data may be incomplete");
                self.session.acknowledge_coverage_overflow(*generation);
            }
        }
    }

    fn handle_login_accepted(&self, mfa_hash: Option<String>) {
        let mailbox = self.mailbox.clone();
        let write_lock = self.credentials.write_lock.clone();
        let username = self.credentials.username.clone();
        let password = self.credentials.password.clone();
        let grid = self.credentials.grid;
        let remember_login = self.credentials.remember_login;
        let span = tracing::Span::current();
        drop(self.runtime.spawn_blocking(move || {
            let _span = span.enter();
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

    async fn handle_simulator_message(
        &mut self,
        simulator_id: SimulatorId,
        sequence: u32,
        packet_order: PacketOrder,
        source: std::net::SocketAddr,
        message: Message,
    ) {
        let received_at = std::time::Instant::now();
        let primary_origin_before = (self.primary_simulator == Some(simulator_id)).then(|| self.primary_region_origin()).flatten();
        let region = self.session_world.region_for(simulator_id);
        if let Message::RegionHandshake(handshake) = &message {
            self.object_cache.set_region_identity(simulator_id, handshake);
            if let Some(flags) = self.object_cache.begin_region_load(simulator_id, &self.runtime) {
                reply_region_handshake(
                    &self.runtime,
                    Arc::clone(&self.pending_handshake_replies),
                    self.session_handle.clone(),
                    simulator_id,
                    flags,
                );
            }
        }
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        if matches!(packet_order, PacketOrder::Ordered(_))
            && Some(simulator_id) == self.primary_simulator
            && let (Some(agent), Some(region)) = (self.active_session.as_ref(), region)
            && let Err(error) = self.environment_pushes.publish(&message, agent.agent_id, region, simulator_id)
        {
            tracing::warn!(simulator = simulator_id.get(), region = region.get(), %error, "experience environment push could not be admitted");
        }

        let mut media_requests =
            region.map(|region| object_media_requests(region, sequence, order_evidence(packet_order), &message)).unwrap_or_default();
        if let Some(region) = region {
            self.terrain.enqueue(&message, UpdateSource::new(region, sequence, order_evidence(packet_order), 0), &self.runtime);
        }

        let agent_id = self.active_session.as_ref().map(|session| session.agent_id);
        let agent_data =
            self.active_session.as_ref().map(|session| RequestAgentData { agent_id: session.agent_id, session_id: session.session_id });
        let result = {
            let recovery = &mut self.recovery;
            let mut on_object_chunk = |targets, updated| {
                recovery.schedule(targets, updated, agent_data);
            };
            self.handoff
                .apply_message_cooperatively(
                    &mut self.session_world,
                    simulator_id,
                    sequence,
                    packet_order,
                    &message,
                    agent_id,
                    &mut self.object_cache,
                    &mut on_object_chunk,
                    &mut media_requests,
                )
                .await
        };
        let mut touched_agent = false;
        if let Some(outcome) = result {
            touched_agent = outcome.touched_agent;
            if let Some((simulator_id, summary)) = outcome.summary {
                self.process_ingest_summary(simulator_id, summary);
            }
        }
        if touched_agent && self.primary_simulator == Some(simulator_id) {
            self.publish_world_agent_motion(received_at);
        }

        self.environments.simulator_message(&mut self.session_world, simulator_id, packet_order, &message, self.primary_simulator);
        for request in media_requests {
            if let Some(target) = self.session_world.prepare_object_media(simulator_id, request) {
                self.media.queue(target, &self.session_world);
            }
        }

        if let Message::AgentMovementComplete(movement) = &message {
            let outcome = self.handoff.movement_complete(
                &mut self.session_world,
                simulator_id,
                self.active_session.as_ref().map(|session| (session.agent_id, session.session_id)),
                movement,
                received_at,
            );
            if outcome == MovementCompleteOutcome::Applied {
                self.publish_arrival_agent_motion(simulator_id, movement, received_at);
            }
        }
        let scope = if Some(simulator_id) == self.primary_simulator { SimulatorScope::Primary } else { SimulatorScope::Neighbor };
        let region_name = region.and_then(|region| self.session_world.world().region(region)).and_then(|info| info.name.as_deref());
        handle_simulator_message(
            scope,
            source,
            &message,
            self.active_session.as_ref(),
            self.region_lookup.as_ref(),
            region_name,
            &self.mailbox,
        );
        self.object_cache.record_update(simulator_id, message);
        if self.primary_simulator == Some(simulator_id) {
            self.camera.region_origin_changed(primary_origin_before, self.primary_region_origin());
        }
    }

    fn handle_inventory_udp(&mut self, message: &Message) {
        let Some(agent_id) = self.active_session.as_ref().map(|session| session.agent_id) else { return };
        let Some(before) = self.inventory.as_ref().map(Inventory::snapshot) else {
            if self.inventory_bootstrap.is_some() && inventory_intake::is_inventory_udp_message(message, agent_id) {
                self.inventory_bootstrap_dirty = true;
            }
            return;
        };
        let Some(inventory) = self.inventory.as_mut() else { return };
        let result = inventory_intake::apply_udp(inventory, agent_id, message);
        self.apply_inventory_intake_result(result, before);
    }

    fn handle_inventory_event_queue(&mut self, name: &str, body: &llsd::Value) {
        let Some(agent_id) = self.active_session.as_ref().map(|session| session.agent_id) else { return };
        let Some(before) = self.inventory.as_ref().map(Inventory::snapshot) else {
            if self.inventory_bootstrap.is_some() && inventory_intake::is_valid_inventory_event(name, body, agent_id) {
                self.inventory_bootstrap_dirty = true;
            }
            return;
        };
        let Some(inventory) = self.inventory.as_mut() else { return };
        let result = inventory_intake::apply_event_queue(inventory, agent_id, name, body);
        self.apply_inventory_intake_result(result, before);
    }

    fn apply_inventory_intake_result(
        &mut self,
        result: Result<Option<InventoryIntakeOutcome>, InventoryIntakeError>,
        before: InventorySnapshot,
    ) {
        match result {
            Ok(Some(outcome)) => {
                self.note_rejected_inventory_blocks(outcome.rejected_blocks);
                if !outcome.changes.is_empty()
                    && let Some(snapshot) = self.inventory.as_ref().map(Inventory::snapshot)
                {
                    self.process_inventory_changes(&outcome.changes, &snapshot);
                    self.inventory_changes.send_replace(Some(snapshot.clone()));
                    self.request_inventory_refetches(&outcome.changes, &snapshot);
                    self.publish_inventory_status();
                    self.inventory_cache.record_change(&self.runtime, outcome.changes, before, snapshot);
                }
            }
            Ok(None) => {}
            Err(error) => self.note_rejected_inventory_blocks_with_error(error),
        }
    }

    fn note_rejected_inventory_blocks(&mut self, count: usize) {
        self.rejected_inventory_blocks = self.rejected_inventory_blocks.saturating_add(count as u64);
        if count > 0 && self.rejected_inventory_blocks / 64 != (self.rejected_inventory_blocks - count as u64) / 64 {
            tracing::warn!(rejected_blocks = self.rejected_inventory_blocks, "inventory updates contain rejected blocks");
        }
    }

    fn note_rejected_inventory_blocks_with_error(&mut self, error: InventoryIntakeError) {
        self.rejected_inventory_blocks = self.rejected_inventory_blocks.saturating_add(1);
        if self.rejected_inventory_blocks.is_multiple_of(64) {
            tracing::warn!(rejected_blocks = self.rejected_inventory_blocks, %error, "inventory updates were rejected");
        }
    }

    fn on_primary_changed(
        &mut self,
        simulator_id: SimulatorId,
        session: AgentSession,
        capabilities: Client,
        login_data: Option<ResponseData>,
    ) {
        self.camera.request_update();
        self.media.set_client(simulator_id, capabilities.clone());
        self.primary_simulator = Some(simulator_id);
        self.active_session = Some(session.clone());
        if let Some(login_data) = login_data {
            self.active_login_data = Some(login_data);
        }
        self.active_capabilities = Some(capabilities);
        let lookup = RegionLookup::new(self.session_handle.clone(), simulator_id, &session, self.runtime.clone());
        self.region_lookup = Some(lookup);
        self.publish_session_ready_update(simulator_id, session);
        self.start_favorites_load_for_active();
    }

    fn publish_session_ready_update(&self, simulator_id: SimulatorId, session: AgentSession) {
        self.mailbox.publish(ViewerUpdate::SessionReady(SessionReadyUpdate {
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            environments: self.environment_inbox.clone(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            camera: self.camera.sender(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            movement: self.camera.movement_sender(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            agent_motion: self.agent_motion.receiver(),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            assets: self.assets.handle(),
            session,
            region_name: self.session_world.region_name(simulator_id).map(str::to_owned),
            inventory_updates: self.inventory_changes.subscribe(),
            inventory_fetch: self.inventory.as_ref().map(|_| self.inventory_fetch.handle()),
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            world_reader: self.reads.reader(),
        }));
    }

    fn finish_inventory_bootstrap(&mut self, inventory: Inventory, report: InventoryBootstrapReport) {
        if report.rejected_personal_entries > 0 || report.rejected_library_entries > 0 {
            tracing::warn!(
                personal_rejected = report.rejected_personal_entries,
                library_rejected = report.rejected_library_entries,
                "login inventory skeleton contained rejected folders"
            );
        }
        if self.inventory_bootstrap_dirty {
            self.inventory_cache.invalidate_restore();
            tracing::warn!(
                "early inventory updates arrived before login inventory was ready; cached contents will be refreshed from the simulator"
            );
        }
        if let Some(session) = &self.active_session {
            let library_owner = inventory.roots().library().map(|root| root.owner().id());
            self.inventory_cache.start(&self.runtime, self.credentials.grid, session.agent_id, library_owner, &inventory);
        }
        self.inventory_changes.send_replace(Some(inventory.snapshot()));
        self.inventory = Some(inventory);
        self.inventory_bootstrap_dirty = false;
        if let Some(capabilities) = self.active_capabilities.clone() {
            self.start_inventory_edits(capabilities.clone());
            self.start_inventory_fetch(capabilities);
        }
        if let Some(simulator_id) = self.primary_simulator
            && let Some(session) = self.active_session.clone()
        {
            self.publish_session_ready_update(simulator_id, session);
        }
        self.start_favorites_load_for_active();
    }

    fn start_favorites_load_for_active(&self) {
        if !self.favorites_enabled {
            return;
        }
        let (Some(session), Some(capabilities), Some(lookup)) =
            (self.active_session.clone(), self.active_capabilities.clone(), self.region_lookup.clone())
        else {
            return;
        };
        let inventory = match self.inventory.as_ref() {
            Some(inventory) => Some(inventory.snapshot()),
            None if self.inventory_bootstrap.is_some() => return,
            None => None,
        };
        self.spawn_favorites_load(session, capabilities, lookup, inventory);
    }

    fn spawn_favorites_load(
        &self,
        session: AgentSession,
        capabilities: Client,
        lookup: RegionLookup,
        inventory: Option<InventorySnapshot>,
    ) {
        let mailbox = self.mailbox.clone();
        let username = self.credentials.username.clone();
        let cancelled = self.cancellation.clone();
        drop(
            self.runtime.spawn(
                async move {
                    let load = load_favorites(&capabilities, &lookup, &session, inventory.as_ref()).fuse();
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
                }
                .in_current_span(),
            ),
        );
    }

    fn process_ingest_summary(&mut self, simulator_id: SimulatorId, summary: IngestSummary) {
        summary.log_incomplete_data(simulator_id, self.session_world.region_for(simulator_id));
        let targets = route_recovery_targets(&self.session_world, summary.recovery_requests());
        let updated = summary.recovery_cancellation_addresses().collect();
        let agent_data =
            self.active_session.as_ref().map(|session| RequestAgentData { agent_id: session.agent_id, session_id: session.session_id });
        self.recovery.schedule(targets, updated, agent_data);
    }

    fn summary_touches_agent(&self, summary: &IngestSummary) -> bool {
        self.active_session
            .as_ref()
            .and_then(|session| self.session_world.world().entity(session.agent_id))
            .is_some_and(|entity| summary.touches_entity(entity))
    }

    fn publish_world_agent_motion(&self, received_at: std::time::Instant) {
        let Some(agent_id) = self.active_session.as_ref().map(|session| session.agent_id) else { return };
        self.agent_motion.publish_world(self.session_world.world(), agent_id, received_at);
    }

    fn publish_arrival_agent_motion(
        &mut self,
        simulator_id: SimulatorId,
        movement: &protocol::messages::AgentMovementComplete,
        received_at: std::time::Instant,
    ) {
        let Some(session) = self.active_session.as_ref() else { return };
        let Some(origin) = self
            .session_world
            .region_for(simulator_id)
            .and_then(|region| self.session_world.world().region(region))
            .and_then(world::RegionInfo::resolved_origin)
        else {
            return;
        };
        if self.agent_motion.publish_arrival(session.agent_id, session.session_id, movement, origin, received_at) {
            self.camera.request_update();
        }
    }

    fn handle_recovery_completion(&mut self, completion: RecoveryCompletion) {
        match completion {
            RecoveryCompletion::Deadline { aged_out, due } => {
                warn_unrecovered("age_limit", &aged_out);
                let (exhausted, recoveries): (Vec<_>, Vec<_>) = due.into_iter().partition(ScheduledRecovery::is_exhausted);
                self.recovery.degrade_recoveries(&exhausted);
                warn_unrecovered("send_limit", &exhausted);
                if let Err(oversized) = self.recovery.start_send(&self.session_handle, recoveries) {
                    self.recovery.degrade_recoveries(&oversized.recoveries);
                    if let Some(first) = oversized.recoveries.first() {
                        tracing::error!(
                            simulator = first.simulator_id.get(),
                            region = first.key.region().get(),
                            "object recovery request exceeded its byte budget"
                        );
                    }
                }
            }
            RecoveryCompletion::Send { recoveries, result, completed_at } => {
                let Some(first) = recoveries.first() else { return };
                let simulator_id = first.simulator_id;
                let region = first.key.region().get();
                match result {
                    Ok(()) => self.recovery.mark_sent(&recoveries, completed_at),
                    Err(error) if retryable_send_error(&error) => {
                        self.recovery.defer(&recoveries, completed_at);
                        tracing::debug!(simulator = simulator_id.get(), region, %error, "object recovery send deferred");
                    }
                    Err(error) => {
                        if simulator_lost_error(&error) {
                            self.recovery.degrade_simulator(simulator_id);
                        } else {
                            self.recovery.degrade_recoveries(&recoveries);
                        }
                        tracing::warn!(simulator = simulator_id.get(), region, %error, "could not request object updates");
                    }
                }
            }
        }
    }

    fn start_inventory_fetch(&mut self, capabilities: Client) {
        let Some(inventory) = &self.inventory else { return };
        let snapshot = inventory.snapshot();
        self.inventory_changes.send_replace(Some(snapshot.clone()));
        self.inventory_known = snapshot.categories().map(|category| category.key).collect();
        self.inventory_complete = self.inventory_known.iter().copied().filter(|key| snapshot.is_complete(*key)).collect();
        self.inventory_failed.clear();
        self.inventory_background_done = false;
        self.inventory_last_failure = None;
        self.publish_inventory_status();
        let model = self.inventory_changes.subscribe();
        self.inventory_fetch.start(&self.runtime, capabilities, snapshot, model, self.cancellation.clone());
    }

    fn start_inventory_edits(&mut self, capabilities: Client) {
        if self.inventory.is_none() {
            return;
        }
        let Some(session) = self.active_session.clone() else { return };
        let model = self.inventory_changes.subscribe();
        self.inventory_edits.start(&self.runtime, capabilities, self.session_handle.clone(), session, model, self.cancellation.clone());
    }

    fn handle_inventory_edit_event(&mut self, event: InventoryEditEvent) {
        match event {
            InventoryEditEvent::Apply { confirmation, applied } => {
                let result = self.apply_inventory_confirmation(confirmation);
                if applied.send(result).is_err() {
                    tracing::trace!("inventory edit worker stopped before apply acknowledgement");
                }
            }
            InventoryEditEvent::RefreshFolders { folders, refreshed } => {
                let folder_count = folders.len();
                let refreshed_inventory = self.mark_inventory_folders_incomplete(folders);
                if !refreshed_inventory {
                    tracing::debug!(folder_count, "inventory folder refresh could not be queued after an edit failure");
                }
                if refreshed.send(refreshed_inventory).is_err() {
                    tracing::trace!("inventory edit worker stopped before folder refresh acknowledgement");
                }
            }
            InventoryEditEvent::MoveStarted { expectation, confirmed, registered } => {
                self.inventory_pending_moves.insert(expectation.node, (expectation, confirmed));
                if registered.send(()).is_err() {
                    tracing::trace!("inventory edit worker stopped before move registration acknowledgement");
                }
            }
            InventoryEditEvent::MoveAborted { expectation, removed } => {
                if self.inventory_pending_moves.get(&expectation.node).is_some_and(|(current, _)| *current == expectation) {
                    self.inventory_pending_moves.remove(&expectation.node);
                }
                if removed.send(()).is_err() {
                    tracing::trace!("inventory edit worker stopped before move abort acknowledgement");
                }
            }
            InventoryEditEvent::MoveTimedOut { expectation, refreshed } => {
                if self.inventory_pending_moves.get(&expectation.node).is_some_and(|(current, _)| *current == expectation) {
                    self.inventory_pending_moves.remove(&expectation.node);
                }
                let applied = self.mark_inventory_folders_incomplete([expectation.from, expectation.to]);
                if refreshed.send(applied).is_err() {
                    tracing::trace!("inventory edit worker stopped before move timeout acknowledgement");
                }
            }
        }
    }

    fn apply_inventory_confirmation(&mut self, confirmation: AisDelta) -> bool {
        let Some(current) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        self.apply_inventory_edit_delta(confirmation.into_delta(&current))
    }

    fn apply_inventory_edit_delta(&mut self, delta: InventoryDelta) -> bool {
        let Some(before) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        let Some(inventory) = self.inventory.as_mut() else { return false };
        let changes = match inventory.apply_delta(delta) {
            Ok(changes) => changes,
            Err(error) => {
                tracing::warn!(%error, "inventory edit confirmation could not be applied");
                return false;
            }
        };
        if changes.is_empty() {
            return true;
        }
        let Some(snapshot) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        self.process_inventory_changes(&changes, &snapshot);
        self.inventory_changes.send_replace(Some(snapshot.clone()));
        self.request_inventory_refetches(&changes, &snapshot);
        self.publish_inventory_status();
        self.inventory_cache.record_change(&self.runtime, changes, before, snapshot);
        true
    }

    fn process_inventory_changes(&mut self, changes: &ChangeSet, snapshot: &InventorySnapshot) {
        resolve_pending_inventory_moves(&mut self.inventory_pending_moves, changes);
        self.track_inventory_changes(changes, snapshot);
    }

    fn mark_inventory_folders_incomplete(&mut self, folders: impl IntoIterator<Item = CategoryKey>) -> bool {
        let Some(snapshot) = self.inventory.as_ref().map(Inventory::snapshot) else { return false };
        let folders = folders.into_iter().filter(|folder| snapshot.category(*folder).is_some()).collect::<HashSet<_>>();
        if folders.is_empty() {
            return false;
        }
        let delta = InventoryDelta {
            updated_category_versions: folders.into_iter().map(|folder| (folder, ServerVersion::UNKNOWN)).collect(),
            ..InventoryDelta::default()
        };
        self.apply_inventory_edit_delta(delta)
    }

    fn handle_inventory_fetch(&mut self, event: InventoryFetchEvent) {
        match event {
            InventoryFetchEvent::Folder { fetch, applied } => {
                let folder = fetch.folder;
                let before = self.inventory.as_ref().map(Inventory::snapshot);
                let result = self.inventory.as_mut().map(|inventory| apply_inventory_fetch(inventory, fetch));
                let snapshot = self.inventory.as_ref().map(Inventory::snapshot);
                let mut rejected = false;
                match result {
                    Some(Ok(Some(changes))) => {
                        if let Some(snapshot) = snapshot.as_ref() {
                            self.process_inventory_changes(&changes, snapshot);
                            if snapshot.is_complete(folder) {
                                self.inventory_complete.insert(folder);
                            } else {
                                self.inventory_complete.remove(&folder);
                            }
                            self.inventory_failed.remove(&folder);
                            if self.inventory_last_failure.as_ref().is_some_and(|(failed, _)| *failed == folder) {
                                self.inventory_last_failure = None;
                            }
                            self.inventory_changes.send_replace(Some(snapshot.clone()));
                            if let Some(before) = before {
                                self.inventory_cache.record_change(&self.runtime, changes, before, snapshot.clone());
                            }
                        }
                    }
                    Some(Ok(None)) => {}
                    Some(Err(error)) => {
                        rejected = true;
                        self.record_inventory_failure(folder, InventoryFetchError::Model(error));
                    }
                    None => rejected = true,
                }
                let outcome = FolderApplyOutcome { rejected };
                if applied.send(outcome).is_err() {
                    tracing::debug!(folder = %folder.id().uuid(), "inventory fetch worker stopped before apply acknowledgement");
                }
                self.publish_inventory_status();
            }
            InventoryFetchEvent::Failed { folder, error } => {
                self.record_inventory_failure(folder, error);
                self.publish_inventory_status();
            }
            InventoryFetchEvent::BackgroundDone => {
                self.inventory_background_done = true;
                self.publish_inventory_status();
            }
        }
    }

    fn track_inventory_changes(&mut self, changes: &ChangeSet, snapshot: &InventorySnapshot) {
        for change in &changes.changes {
            match change {
                InventoryChange::CategoryUpserted(category) => {
                    self.inventory_known.insert(category.key);
                    if snapshot.is_complete(category.key) {
                        self.inventory_complete.insert(category.key);
                    } else {
                        self.inventory_complete.remove(&category.key);
                    }
                }
                InventoryChange::CategoryRemoved(key) => {
                    self.inventory_known.remove(key);
                    self.inventory_complete.remove(key);
                    self.inventory_failed.remove(key);
                }
                _ => {}
            }
        }
    }

    fn request_inventory_refetches(&mut self, changes: &ChangeSet, snapshot: &InventorySnapshot) {
        let handle = self.inventory_fetch.handle();
        let mut queue_failures = 0usize;
        for folder in &changes.refetch_categories {
            if snapshot.category(*folder).is_none() {
                continue;
            }
            self.inventory_complete.remove(folder);
            if handle.refetch_folder(*folder).is_err() {
                self.inventory_failed.insert(*folder);
                self.inventory_last_failure = Some((*folder, InventoryFetchError::RequestFailed));
                queue_failures = queue_failures.saturating_add(1);
            }
        }
        if queue_failures > 0 {
            tracing::warn!(queue_failures, "could not queue inventory folder refetches");
        }
    }

    fn record_inventory_failure(&mut self, folder: CategoryKey, error: InventoryFetchError) {
        self.inventory_failed.insert(folder);
        self.inventory_last_failure = Some((folder, error.clone()));
        tracing::warn!(owner = %folder.owner().id(), folder = %folder.id().uuid(), %error, "inventory folder fetch failed");
    }

    fn publish_inventory_status(&self) {
        self.inventory_fetch.publish(InventoryLoadingStatus {
            folders_known: self.inventory_known.len(),
            folders_complete: self.inventory_complete.len(),
            folders_failed: self.inventory_failed.len(),
            background_done: self.inventory_background_done,
            last_failure: self.inventory_last_failure.clone(),
        });
    }

    async fn finish(mut self) {
        if let Some(inventory_bootstrap) = self.inventory_bootstrap.take() {
            inventory_bootstrap.abort();
        }
        self.inventory_pending_moves.clear();
        self.inventory_edits.stop().await;
        self.inventory_fetch.stop().await;
        self.recovery.cancel();
        self.reads.finish();
        drop(self.environments);
        drop(self.terrain);
        self.assets.stop().await;
        self.inventory_cache.close().await;
        self.object_cache.close().await;
        for (simulator_id, error) in self.session_world.retire_all() {
            let region = self.session_world.region_for(simulator_id).map(|region| region.get());
            tracing::warn!(simulator = simulator_id.get(), region, %error, "could not retire simulator world region on session shutdown");
        }
    }
}

fn reply_region_handshake(
    runtime: &Handle,
    pending_replies: Arc<Mutex<PendingHandshakeReplies>>,
    session_handle: SessionHandle,
    simulator_id: SimulatorId,
    flags: u32,
) {
    let mut pending = pending_replies.lock().unwrap_or_else(|error| error.into_inner());
    if !pending.by_simulator.contains_key(&simulator_id) {
        match session_handle.try_reply_region_handshake(simulator_id, flags) {
            Ok(response) => {
                drop(pending);
                drop(runtime.spawn(async move {
                    match response.await {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                        }
                        Err(_) => {
                            tracing::warn!(simulator = simulator_id.get(), "region handshake reply ended with the network session");
                        }
                    }
                }));
                return;
            }
            Err(network::Error::CommandQueueFull) => {}
            Err(error) => {
                tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                return;
            }
        }
    }
    let should_drain = pending.queue(simulator_id, flags);
    drop(pending);
    if !should_drain {
        return;
    }
    drop(runtime.spawn(async move {
        loop {
            let next_flags = pending_replies.lock().unwrap_or_else(|error| error.into_inner()).take_next(simulator_id);
            let Some(next_flags) = next_flags else { break };
            if let Err(error) = session_handle.reply_region_handshake(simulator_id, next_flags).await {
                tracing::warn!(simulator = simulator_id.get(), %error, "region handshake reply could not be sent");
                pending_replies.lock().unwrap_or_else(|error| error.into_inner()).by_simulator.remove(&simulator_id);
                break;
            }
        }
    }));
}

pub(crate) fn resolve_pending_inventory_moves(
    pending: &mut HashMap<InventoryNodeKey, (MoveExpectation, tokio::sync::oneshot::Sender<()>)>,
    changes: &ChangeSet,
) {
    for change in &changes.changes {
        let InventoryChange::NodeMoved { node, from, to } = change else { continue };
        let Some((expectation, _)) = pending.get(node) else { continue };
        if expectation.from == *from
            && expectation.to == *to
            && let Some((_, confirmed)) = pending.remove(node)
            && confirmed.send(()).is_err()
        {
            tracing::trace!("inventory edit worker stopped before receiving move confirmation");
        }
    }
}

fn cache_restore_move_changes(
    pending: &HashMap<InventoryNodeKey, (MoveExpectation, tokio::sync::oneshot::Sender<()>)>,
    before: &InventorySnapshot,
    after: &InventorySnapshot,
) -> ChangeSet {
    let changes = pending
        .iter()
        .filter_map(|(node, (expectation, _))| {
            (inventory_node_parent(before, *node) == Some(expectation.from) && inventory_node_parent(after, *node) == Some(expectation.to))
                .then_some(InventoryChange::NodeMoved { node: *node, from: expectation.from, to: expectation.to })
        })
        .collect();
    ChangeSet { revision: after.revision(), changes, refetch_categories: Vec::new() }
}

fn inventory_node_parent(snapshot: &InventorySnapshot, node: InventoryNodeKey) -> Option<CategoryKey> {
    match node.id() {
        InventoryNodeId::Category(id) => snapshot.category(CategoryKey::new(node.owner(), id)).and_then(|category| category.parent),
        InventoryNodeId::Item(id) => snapshot.item(::inventory::ItemKey::new(node.owner(), id)).map(|item| item.parent),
    }
}

fn apply_inventory_fetch(inventory: &mut Inventory, mut fetch: FolderFetch) -> Result<Option<ChangeSet>, InventoryError> {
    let current = inventory.snapshot();
    let stale = current.is_complete(fetch.folder)
        && current.category(fetch.folder).is_some_and(|category| {
            category.server_version.is_known() && fetch.version.is_known() && category.server_version.raw() > fetch.version.raw()
        });
    if stale {
        return Ok(None);
    }
    for child in &mut fetch.children {
        let InventoryChild::Category(incoming) = child else { continue };
        let Some(existing) = current.category(incoming.key) else { continue };
        if current.is_complete(incoming.key)
            && existing.server_version.is_known()
            && incoming.server_version.is_known()
            && existing.server_version.raw() > incoming.server_version.raw()
        {
            incoming.server_version = existing.server_version;
            incoming.server_descendent_count = existing.server_descendent_count;
        }
    }
    inventory.apply_folder_fetch(fetch.folder, fetch.version, fetch.descendent_count, fetch.children).map(Some)
}

#[tracing::instrument(name = "viewer.session", level = "info", skip_all)]
pub(crate) async fn drain_session(
    session: Session,
    session_handle: SessionHandle,
    runtime: Handle,
    mailbox: PresentationMailbox,
    cancellation: oneshot::Receiver<()>,
    credentials: CredentialPersistence,
    cache: Option<Arc<assets::cache::AssetCache>>,
) {
    SessionDriver::new(session, session_handle, runtime, mailbox, cancellation, credentials, cache).run().await;
}

#[cfg(test)]
#[path = "driver_tests.rs"]
mod tests;
